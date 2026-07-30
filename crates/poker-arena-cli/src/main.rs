//! `poker-arena` CLI — list game variants and run matches between bots.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use poker_arena::Bot;
use poker_arena::builtin::{Caller, Folder, Random, Shover};
use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
use poker_arena::log::{EventSink, JsonLog};
use poker_arena::remote::WireBot;
use poker_arena::runner::{Progress, run_match};
use poker_core::game::{GameSpec, Stakes};
use poker_wire::message::{ArenaMsg, GameInfo};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Games => {
            print_games();
            ExitCode::SUCCESS
        }
        Command::Run(args) => match run(*args) {
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
    Run(Box<RunArgs>),
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

    /// Per-player ante, in chips. Stud games only; defaults to bb/5 (min 1).
    /// Passing this for a non-stud game is an error.
    #[arg(long)]
    ante: Option<u64>,

    /// Forced bring-in, in chips. Stud games only; defaults to bb/2 (min 1).
    /// Passing this for a non-stud game is an error.
    #[arg(long)]
    bring_in: Option<u64>,

    /// Small bet tier, in chips. Stud games only; defaults to bb. Passing
    /// this for a non-stud game is an error.
    #[arg(long)]
    small_bet: Option<u64>,

    /// Big bet tier, in chips. Stud games only; defaults to 2 × small bet.
    /// Passing this for a non-stud game is an error.
    #[arg(long)]
    big_bet: Option<u64>,

    /// Number of decks to play: one hand per deck in seeded mode, or
    /// decks of duplicate rotations (one hand per seat rotation, all bots
    /// see every deck from every seat) in duplicate mode.
    #[arg(long, default_value_t = 10_000)]
    hands: u64,

    /// RNG seed for deck shuffling; the whole match is reproducible from
    /// this seed given deterministic bots. Defaults to a fresh random seed
    /// (always printed with the results) so repeated runs explore new deals.
    #[arg(long)]
    seed: Option<u64>,

    /// How decks are dealt across seat rotations.
    #[arg(long, value_enum, default_value = "duplicate")]
    dealing: DealingArg,

    /// Starting stack, in big blinds (reset every hand).
    #[arg(long, default_value_t = 100)]
    stack_bb: u64,

    /// What happens when a bot returns a non-conforming action.
    #[arg(long, value_enum, default_value = "check-fold")]
    fault_policy: FaultPolicyArg,

    /// Per-action deadline in milliseconds (0 = no deadline). Enforced as a
    /// hard deadline for wire bots; in-process bots are not preemptible.
    #[arg(long, default_value_t = 1000)]
    timeout_ms: u64,

    /// A competitor: `builtin:folder` | `builtin:caller` | `builtin:shover`
    /// | `builtin:random[:seed]` | `tcp:PORT` (wait for a bot to connect on
    /// 127.0.0.1:PORT) | `cmd:COMMAND` (spawn a bot and talk over its stdio).
    /// Repeat once per seat (count must fit the game's seat range).
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
    let placeholder_stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
    };
    for id in GameSpec::known_ids() {
        let spec = GameSpec::by_id(id, placeholder_stakes)
            .expect("known_ids() entries must resolve via by_id");
        println!("{:<12} {}", spec.id, spec.display_name);
    }
}

/// A parsed `--bot` spec. Nothing is named or connected yet: naming happens
/// once every spec is known (duplicate base names get `-2`, `-3`… suffixes),
/// and a wire bot's base name only exists after it has joined.
enum BotSpec {
    Builtin(BuiltinKind),
    /// Wait for a bot to connect on `127.0.0.1:PORT`.
    Tcp(u16),
    /// Spawn a bot process and talk over its stdio.
    Cmd(String),
}

/// A builtin kind.
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

fn parse_bot_spec(spec: &str) -> Result<BotSpec, String> {
    if let Some(port) = spec.strip_prefix("tcp:") {
        let port = port
            .parse::<u16>()
            .map_err(|_| format!("invalid port {port:?} in --bot {spec:?}"))?;
        return Ok(BotSpec::Tcp(port));
    }
    if let Some(command) = spec.strip_prefix("cmd:") {
        if command.trim().is_empty() {
            return Err(format!("empty command in --bot {spec:?}"));
        }
        return Ok(BotSpec::Cmd(command.to_string()));
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
        "folder" => Ok(BotSpec::Builtin(BuiltinKind::Folder)),
        "caller" => Ok(BotSpec::Builtin(BuiltinKind::Caller)),
        "shover" => Ok(BotSpec::Builtin(BuiltinKind::Shover)),
        "random" => {
            let seed = arg
                .map(|s| {
                    s.parse::<u64>()
                        .map_err(|_| format!("invalid seed {s:?} in --bot {spec:?}"))
                })
                .transpose()?;
            Ok(BotSpec::Builtin(BuiltinKind::Random(seed)))
        }
        other => Err(format!(
            "unknown builtin bot {other:?} in --bot {spec:?} (expected folder/caller/shover/random)"
        )),
    }
}

/// A bot that exists but isn't named yet.
enum Pending {
    Builtin(BuiltinKind),
    Wire(WireBot),
}

/// Connects/spawns every wire bot (sequentially, in `--bot` order, so the
/// order connections are expected in is predictable) and names the whole
/// field, disambiguating duplicates with `-2`, `-3`… suffixes.
///
/// A wire bot's base name is the one it gave in its `join`, so the same
/// disambiguation covers builtin and wire bots alike.
fn build_bots(
    specs: Vec<BotSpec>,
    hello: &ArenaMsg,
    timeout: Option<Duration>,
) -> Result<Vec<Box<dyn Bot>>, String> {
    // A bot that has to be started (or a human that has to start it) deserves
    // more slack than a single decision does.
    let handshake = timeout.unwrap_or_default().max(Duration::from_secs(10));

    let mut pending = Vec::with_capacity(specs.len());
    for spec in specs {
        pending.push(match spec {
            BotSpec::Builtin(kind) => Pending::Builtin(kind),
            BotSpec::Tcp(port) => {
                eprintln!("waiting for a bot on 127.0.0.1:{port} ...");
                let mut bot = WireBot::listen_tcp(port, hello.clone(), handshake)
                    .map_err(|e| e.to_string())?;
                bot.set_timeout(timeout);
                Pending::Wire(bot)
            }
            BotSpec::Cmd(command) => {
                let mut bot = WireBot::spawn_cmd(&command, hello.clone(), handshake)
                    .map_err(|e| e.to_string())?;
                bot.set_timeout(timeout);
                Pending::Wire(bot)
            }
        });
    }

    let base_names: Vec<String> = pending
        .iter()
        .map(|p| match p {
            Pending::Builtin(kind) => kind.base_name().to_string(),
            Pending::Wire(bot) => bot.name().to_string(),
        })
        .collect();

    Ok(pending
        .into_iter()
        .zip(disambiguate(&base_names))
        .enumerate()
        .map(|(i, (p, name))| match p {
            Pending::Builtin(kind) => kind.build(name, i),
            Pending::Wire(mut bot) => {
                bot.set_name(name);
                Box::new(bot) as Box<dyn Bot>
            }
        })
        .collect())
}

/// Names in `--bot` order, with the second and later use of a base name
/// suffixed `-2`, `-3`, …
fn disambiguate(base_names: &[String]) -> Vec<String> {
    let mut seen: HashMap<&str, u32> = HashMap::new();
    base_names
        .iter()
        .map(|base| {
            let count = seen.entry(base.as_str()).or_insert(0);
            *count += 1;
            if *count == 1 {
                base.clone()
            } else {
                format!("{base}-{count}")
            }
        })
        .collect()
}

fn run(args: RunArgs) -> Result<ExitCode, String> {
    let blind_stakes = Stakes::Blinds {
        small_blind: args.sb,
        big_blind: args.bb,
    };
    let mut spec = GameSpec::by_id(&args.game, blind_stakes)
        .ok_or_else(|| format!("unknown game {:?} (see `poker-arena games`)", args.game))?;

    let stud_flags_given = args.ante.is_some()
        || args.bring_in.is_some()
        || args.small_bet.is_some()
        || args.big_bet.is_some();
    match spec.stakes {
        Stakes::Stud { .. } => {
            // Rebuild with explicit stud numbers layered over the same
            // derivation `Stakes::to_stud` would have used, so omitted
            // flags fall back to exactly today's defaults.
            let default_stud = blind_stakes.to_stud();
            let Stakes::Stud {
                ante: default_ante,
                bring_in: default_bring_in,
                small_bet: default_small_bet,
                ..
            } = default_stud
            else {
                unreachable!("to_stud() always returns Stakes::Stud")
            };
            let small_bet = args.small_bet.unwrap_or(default_small_bet);
            let stud_stakes = Stakes::Stud {
                ante: args.ante.unwrap_or(default_ante),
                bring_in: args.bring_in.unwrap_or(default_bring_in),
                small_bet,
                big_bet: args.big_bet.unwrap_or(small_bet * 2),
            };
            spec = GameSpec::by_id(&args.game, stud_stakes)
                .ok_or_else(|| format!("unknown game {:?} (see `poker-arena games`)", args.game))?;
        }
        Stakes::Blinds { .. } if stud_flags_given => {
            return Err(
                "--ante/--bring-in/--small-bet/--big-bet apply only to stud games".to_string(),
            );
        }
        Stakes::Blinds { .. } => {}
    }
    let stakes = spec.stakes;

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

    let specs: Vec<BotSpec> = args
        .bots
        .iter()
        .map(|s| parse_bot_spec(s))
        .collect::<Result<_, _>>()?;

    let timeout = (args.timeout_ms > 0).then(|| Duration::from_millis(args.timeout_ms));
    let starting_stack = args.stack_bb * args.bb;
    let hello = ArenaMsg::Hello {
        proto: poker_wire::PROTO_VERSION,
        game: GameInfo {
            id: spec.id.to_string(),
            display_name: spec.display_name.to_string(),
            stakes,
        },
        seat_count: args.bots.len(),
        starting_stack,
        timeout_ms: timeout.map(|d| d.as_millis() as u64),
    };
    let mut bots = build_bots(specs, &hello, timeout)?;

    let seed = args.seed.unwrap_or_else(entropy_seed);
    if args.seed.is_none() {
        // Surface the generated seed up front too, so long or aborted runs
        // are still reproducible.
        eprintln!("seed: {seed} (pass --seed {seed} to reproduce this match)");
    }
    let config = MatchConfig {
        spec,
        decks: args.hands,
        seed,
        dealing: args.dealing.into(),
        starting_stack,
        fault_policy: args.fault_policy.into(),
        timeout,
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

    println!("seed: {seed}");
    print_report(&result);

    if let Some(offender) = result.forfeited_by {
        eprintln!("{} forfeited the match", result.outcomes[offender].name);
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

/// A fresh seed for runs that didn't pin one: system time and PID stirred
/// through splitmix64. Match seeding, not cryptography.
fn entropy_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = nanos ^ ((std::process::id() as u64) << 32);
    // splitmix64 finalizer, same constants as poker-core's RNG seeding.
    state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Prints the behavioral profile table: VPIP, PFR, AF, WTSD, WSD, and fold
/// rate per bot, 3 decimals for rate fields (2 for AF, "inf" when calls are
/// zero but aggression isn't).
fn print_behavior_report(result: &poker_arena::MatchResult) {
    println!();
    println!(
        "{:<16} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "bot", "vpip", "pfr", "af", "wtsd", "wsd", "fold"
    );
    for o in &result.outcomes {
        let b = &o.behavior;
        let af = b.af();
        let af_field = if af.is_infinite() {
            "inf".to_string()
        } else {
            format!("{af:.2}")
        };
        println!(
            "{:<16} {:>6.3} {:>6.3} {:>6} {:>6.3} {:>6.3} {:>6.3}",
            o.name,
            b.vpip(),
            b.pfr(),
            af_field,
            b.wtsd(),
            b.wsd(),
            b.fold_rate(),
        );
    }
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

    print_behavior_report(result);

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
