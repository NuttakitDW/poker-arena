//! `poker-arena` CLI — list game variants and run matches between bots.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use poker_arena::Bot;
use poker_arena::builtin::{Caller, Folder, Random, Shover};
use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
use poker_arena::log::{EventSink, JsonLog};
use poker_arena::runner::{Progress, run_match};
use poker_core::game::{GameSpec, Stakes};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Games => {
            print_games();
            ExitCode::SUCCESS
        }
        Command::Run(args) => match run(args) {
            Ok(code) => code,
            Err(msg) => {
                eprintln!("error: {msg}");
                ExitCode::from(1)
            }
        },
    }
}

#[derive(Parser)]
#[command(name = "poker-arena", version, about = "Poker bot competition arena")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List known game variants (registry id and display name).
    Games,
    /// Run a match between bots.
    Run(RunArgs),
}

#[derive(clap::Args)]
struct RunArgs {
    /// Game variant id (see `poker-arena games`).
    #[arg(long)]
    game: String,

    /// Small blind, in chips.
    #[arg(long, default_value_t = 50)]
    sb: u64,

    /// Big blind, in chips.
    #[arg(long, default_value_t = 100)]
    bb: u64,

    /// Number of decks to play: one hand per deck in seeded mode, or
    /// decks of duplicate rotations (one hand per seat rotation, all bots
    /// see every deck from every seat) in duplicate mode.
    #[arg(long, default_value_t = 10_000)]
    hands: u64,

    /// RNG seed for deck shuffling; the whole match is reproducible from
    /// this seed given deterministic bots.
    #[arg(long, default_value_t = 1)]
    seed: u64,

    /// How decks are dealt across seat rotations.
    #[arg(long, value_enum, default_value = "duplicate")]
    dealing: DealingArg,

    /// Starting stack, in big blinds (reset every hand).
    #[arg(long, default_value_t = 100)]
    stack_bb: u64,

    /// What happens when a bot returns a non-conforming action.
    #[arg(long, value_enum, default_value = "check-fold")]
    fault_policy: FaultPolicyArg,

    /// A competitor: `builtin:folder` | `builtin:caller` | `builtin:shover`
    /// | `builtin:random[:seed]`. Repeat once per seat (count must fit the
    /// game's seat range). `tcp:`/`cmd:` bots arrive in M2.
    #[arg(long = "bot")]
    bots: Vec<String>,

    /// Write the unredacted hand history as JSON lines to this file.
    #[arg(long)]
    log: Option<PathBuf>,

    /// Print progress to stderr every N decks (0 = off).
    #[arg(long, default_value_t = 0)]
    progress_every: u64,
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum DealingArg {
    Seeded,
    Duplicate,
}

impl From<DealingArg> for DealingMode {
    fn from(a: DealingArg) -> Self {
        match a {
            DealingArg::Seeded => DealingMode::Seeded,
            DealingArg::Duplicate => DealingMode::Duplicate,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum FaultPolicyArg {
    CheckFold,
    Forfeit,
}

impl From<FaultPolicyArg> for FaultPolicy {
    fn from(a: FaultPolicyArg) -> Self {
        match a {
            FaultPolicyArg::CheckFold => FaultPolicy::CheckFold,
            FaultPolicyArg::Forfeit => FaultPolicy::Forfeit,
        }
    }
}

fn print_games() {
    let placeholder_stakes = Stakes {
        small_blind: 50,
        big_blind: 100,
    };
    for id in GameSpec::known_ids() {
        let spec = GameSpec::by_id(id, placeholder_stakes)
            .expect("known_ids() entries must resolve via by_id");
        println!("{:<12} {}", spec.id, spec.display_name);
    }
}

/// A `--bot` spec, parsed but not yet named (naming happens after every
/// `--bot` is parsed, since duplicate base names get `-2`, `-3`… suffixes).
enum BuiltinKind {
    Folder,
    Caller,
    Shover,
    Random(Option<u64>),
}

impl BuiltinKind {
    fn base_name(&self) -> &'static str {
        match self {
            BuiltinKind::Folder => "folder",
            BuiltinKind::Caller => "caller",
            BuiltinKind::Shover => "shover",
            BuiltinKind::Random(_) => "random",
        }
    }

    fn build(self, name: String, bot_index: usize) -> Box<dyn Bot> {
        match self {
            BuiltinKind::Folder => Box::new(Folder::new(name)),
            BuiltinKind::Caller => Box::new(Caller::new(name)),
            BuiltinKind::Shover => Box::new(Shover::new(name)),
            BuiltinKind::Random(seed) => {
                Box::new(Random::new(name, seed.unwrap_or(bot_index as u64)))
            }
        }
    }
}

fn parse_bot_kind(spec: &str) -> Result<BuiltinKind, String> {
    if spec.starts_with("tcp:") || spec.starts_with("cmd:") {
        return Err("wire bots arrive in M2".to_string());
    }
    let Some(rest) = spec.strip_prefix("builtin:") else {
        return Err(format!(
            "unrecognized --bot spec {spec:?} (expected builtin:<name>, tcp:*, or cmd:*)"
        ));
    };
    let mut parts = rest.splitn(2, ':');
    let name = parts.next().unwrap_or("");
    let arg = parts.next();
    match name {
        "folder" => Ok(BuiltinKind::Folder),
        "caller" => Ok(BuiltinKind::Caller),
        "shover" => Ok(BuiltinKind::Shover),
        "random" => {
            let seed = arg
                .map(|s| {
                    s.parse::<u64>()
                        .map_err(|_| format!("invalid seed {s:?} in --bot {spec:?}"))
                })
                .transpose()?;
            Ok(BuiltinKind::Random(seed))
        }
        other => Err(format!(
            "unknown builtin bot {other:?} in --bot {spec:?} (expected folder/caller/shover/random)"
        )),
    }
}

/// Builds bots from parsed specs, disambiguating duplicate names with
/// `-2`, `-3`… suffixes (per-kind, in `--bot` order).
fn build_bots(kinds: Vec<BuiltinKind>) -> Vec<Box<dyn Bot>> {
    let mut seen: HashMap<&'static str, u32> = HashMap::new();
    kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| {
            let base = kind.base_name();
            let count = seen.entry(base).or_insert(0);
            *count += 1;
            let name = if *count == 1 {
                base.to_string()
            } else {
                format!("{base}-{count}")
            };
            kind.build(name, i)
        })
        .collect()
}

fn run(args: RunArgs) -> Result<ExitCode, String> {
    let stakes = Stakes {
        small_blind: args.sb,
        big_blind: args.bb,
    };
    let spec = GameSpec::by_id(&args.game, stakes)
        .ok_or_else(|| format!("unknown game {:?} (see `poker-arena games`)", args.game))?;

    let (seat_min, seat_max) = (*spec.seats.start() as usize, *spec.seats.end() as usize);
    if args.bots.len() < seat_min || args.bots.len() > seat_max {
        return Err(format!(
            "{} --bot entries given, but {} supports {}..={} seats",
            args.bots.len(),
            spec.display_name,
            seat_min,
            seat_max
        ));
    }

    let kinds: Vec<BuiltinKind> = args
        .bots
        .iter()
        .map(|s| parse_bot_kind(s))
        .collect::<Result<_, _>>()?;
    let mut bots = build_bots(kinds);

    let config = MatchConfig {
        spec,
        decks: args.hands,
        seed: args.seed,
        dealing: args.dealing.into(),
        starting_stack: args.stack_bb * args.bb,
        fault_policy: args.fault_policy.into(),
        timeout: None,
    };

    let mut log_writer = match &args.log {
        Some(path) => {
            let file = File::create(path)
                .map_err(|e| format!("failed to create log file {}: {e}", path.display()))?;
            Some(JsonLog::new(BufWriter::new(file)))
        }
        None => None,
    };
    let sink: Option<&mut dyn EventSink> = log_writer.as_mut().map(|l| l as &mut dyn EventSink);

    let progress_every = args.progress_every;
    let mut report_progress = move |p: Progress| {
        if progress_every > 0 && p.decks_done.is_multiple_of(progress_every) {
            eprintln!("{} decks, {} hands played", p.decks_done, p.hands_done);
        }
    };
    let on_progress: Option<&mut dyn FnMut(Progress)> = if args.progress_every > 0 {
        Some(&mut report_progress)
    } else {
        None
    };

    let result = run_match(&config, &mut bots, sink, on_progress).map_err(|e| e.to_string())?;

    print_report(&result);

    if let Some(offender) = result.forfeited_by {
        eprintln!("{} forfeited the match", result.outcomes[offender].name);
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

fn print_report(result: &poker_arena::MatchResult) {
    println!(
        "{:<16} {:>10} {:>14} {:>24} {:>7}",
        "bot", "hands", "total chips", "bb/100 (±ci95)", "faults"
    );
    for o in &result.outcomes {
        let mean100 = o.stats.mean() * 100.0;
        let bb_field = match o.stats.ci95_half_width() {
            Some(h) => format!("{mean100:+.3} ± {:.3}", h * 100.0),
            None => format!("{mean100:+.3} ± n/a"),
        };
        println!(
            "{:<16} {:>10} {:>14} {:>24} {:>7}",
            o.name, result.hands_played, o.total_net_chips, bb_field, o.faults
        );
    }

    if result.forfeited_by.is_none() && result.outcomes.len() == 2 {
        let leader = if result.outcomes[0].stats.mean() >= result.outcomes[1].stats.mean() {
            0
        } else {
            1
        };
        let mean100 = result.outcomes[leader].stats.mean() * 100.0;
        let significant = result.outcomes[leader]
            .stats
            .ci95_half_width()
            .is_some_and(|h| mean100.abs() > h * 100.0);
        if significant {
            println!(
                "WINNER: {} (statistically significant at 95%)",
                result.outcomes[leader].name
            );
        } else {
            println!("No statistically significant winner.");
        }
    }
}
