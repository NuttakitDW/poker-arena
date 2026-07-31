//! `poker-arena-ofc` CLI — list OFC variants and run matches between bots.
//!
//! Structure mirrors `main.rs` (the betting arena's CLI) throughout: same
//! subcommand shape, same `--bot` spec grammar, same progress/log/output
//! plumbing. What differs is entirely OFC vocabulary — no blinds or chips,
//! points instead, a different builtin set (`greedy`/`filler`/`random`), and
//! [`poker_arena::ofc::run_ofc_match`] in place of the betting runner.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use poker_arena::ofc::{
    OfcBot, OfcEventSink, OfcFaultPolicy, OfcFiller, OfcGreedy, OfcJsonLog, OfcLogSelection,
    OfcMatchConfig, OfcMatchResult, OfcProgress, OfcRandom, OfcSelectiveLog, OfcWireBot,
    ofc_match_report, ofc_progress_report, run_ofc_match,
};
use poker_core::ofc::{MiddleKind, OfcArenaMsg, OfcSpec, PROTO_VERSION, find, registry};

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
#[command(
    name = "poker-arena-ofc",
    version,
    about = "Open Face Chinese bot competition arena"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List known OFC variants (registry id, name, seats, structure).
    Games,
    /// Run an OFC match between bots.
    Run(Box<RunArgs>),
}

#[derive(clap::Args)]
struct RunArgs {
    /// OFC game variant id: ofc | ofc-pineapple | ofc-progressive | ofc-27
    /// (see `poker-arena-ofc games`).
    #[arg(long)]
    game: String,

    /// Hands to play. Fixed: fantasyland changes how a hand is dealt, never
    /// how many hands there are.
    #[arg(long, default_value_t = 1_000)]
    hands: u64,

    /// RNG seed for the per-hand deck stream; the whole match is
    /// reproducible from this seed given deterministic bots. Defaults to a
    /// fresh random seed (always printed to stderr) so repeated runs
    /// explore new deals.
    #[arg(long)]
    seed: Option<u64>,

    /// What happens when a bot returns a non-conforming placement.
    #[arg(long, value_enum, default_value = "substitute")]
    fault_policy: FaultPolicyArg,

    /// Per-placement deadline in milliseconds (0 = no deadline). Enforced as
    /// a hard deadline for wire bots; in-process bots are not preemptible.
    #[arg(long, default_value_t = 5_000)]
    timeout_ms: u64,

    /// A competitor: `builtin:greedy` | `builtin:filler` |
    /// `builtin:random[:seed]` | `tcp:PORT` (wait for a bot to connect on
    /// 127.0.0.1:PORT) | `cmd:COMMAND` (spawn a bot and talk over its
    /// stdio). Prefix any spec with `NAME@` to assign the competition name
    /// (`alice@cmd:python3 bot.py`); bots carry no identity of their own.
    /// Unnamed bots default to the builtin kind, `tcp-PORT`, or `bot-N`.
    /// Repeat once per seat (count must fit the variant's seat range).
    #[arg(long = "bot")]
    bots: Vec<String>,

    /// Write the unredacted hand history as JSON lines to this file. By
    /// default every hand is written as it's played. If any of
    /// --log-sample / --log-top-swings / --log-faults is given, selective
    /// mode kicks in instead: only the chosen hands are kept, and the whole
    /// file is written at once when the match ends (nothing appears until
    /// then). Requires --log.
    #[arg(long)]
    log: Option<PathBuf>,

    /// Selective log: keep the first N hands. N >= 1. Requires --log.
    #[arg(long)]
    log_sample: Option<u64>,

    /// Selective log: keep the K biggest point-swing hands over the whole
    /// match (global top K, largest absolute per-seat result). Requires
    /// --log.
    #[arg(long)]
    log_top_swings: Option<usize>,

    /// Selective log: cap on hands kept as fault evidence (the first K
    /// hands any bot faulted in); forfeited hands are always kept
    /// regardless of this cap. Only meaningful once selective mode is on
    /// (--log-sample and/or --log-top-swings, or this flag itself);
    /// defaults to 100 once selective mode is on. Requires --log.
    #[arg(long)]
    log_faults: Option<u64>,

    /// Print progress to stderr every N hands (0 = off).
    #[arg(long, default_value_t = 0)]
    progress_every: u64,

    /// Emit progress at most every S seconds (fractional ok; 0 = off).
    /// Combines with --progress-every: either trigger emits.
    #[arg(long, default_value_t = 0.0)]
    progress_secs: f64,

    /// Emit progress as JSON lines (interim standings incl. per-bot rate
    /// and CI) on stderr instead of the human progress line. Requires a
    /// cadence: --progress-every and/or --progress-secs.
    #[arg(long, default_value_t = false)]
    progress_json: bool,

    /// Result format on stdout: aligned human tables, or a single JSON
    /// document (see poker_wire::ofc::report::OfcMatchReport) for
    /// programmatic consumers.
    #[arg(long, value_enum, default_value = "human")]
    output: OutputArg,
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum OutputArg {
    Human,
    Json,
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum FaultPolicyArg {
    Substitute,
    Forfeit,
}

impl From<FaultPolicyArg> for OfcFaultPolicy {
    fn from(a: FaultPolicyArg) -> Self {
        match a {
            FaultPolicyArg::Substitute => OfcFaultPolicy::Substitute,
            FaultPolicyArg::Forfeit => OfcFaultPolicy::Forfeit,
        }
    }
}

fn print_games() {
    for spec in registry() {
        let seats = format!("{}-{}", spec.min_seats, spec.max_seats);
        let structure = format!(
            "deal {} then {}x(deal {}, place {})",
            spec.initial_deal, spec.rounds, spec.round_deal, spec.round_place
        );
        println!(
            "{:<16} {:<26} {:<7} {}",
            spec.id, spec.name, seats, structure
        );
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
    Greedy,
    Filler,
    Random(Option<u64>),
}

impl BuiltinKind {
    fn base_name(&self) -> &'static str {
        match self {
            BuiltinKind::Greedy => "greedy",
            BuiltinKind::Filler => "filler",
            BuiltinKind::Random(_) => "random",
        }
    }

    fn build(self, name: String, bot_index: usize, middle: MiddleKind) -> Box<dyn OfcBot> {
        match self {
            BuiltinKind::Greedy => Box::new(OfcGreedy::new(name, middle)),
            BuiltinKind::Filler => Box::new(OfcFiller::new(name)),
            BuiltinKind::Random(seed) => {
                Box::new(OfcRandom::new(name, seed.unwrap_or(bot_index as u64)))
            }
        }
    }
}

/// The bot-spec kind string carried into the machine-readable report (e.g.
/// `"builtin:greedy"`, `"tcp"`, `"cmd"`) — the CLI owns this vocabulary, per
/// [`poker_arena::ofc::report::ofc_match_report`]'s `kinds` parameter.
fn spec_kind(spec: &BotSpec) -> String {
    match spec {
        BotSpec::Builtin(kind) => format!("builtin:{}", kind.base_name()),
        BotSpec::Tcp(_) => "tcp".to_string(),
        BotSpec::Cmd(_) => "cmd".to_string(),
    }
}

/// Parse a `--bot` spec: split the optional `NAME@` prefix (shared with
/// `poker-arena`; see [`poker_arena_cli::split_named_spec`]) and parse the
/// rest as this binary's own spec kind.
fn parse_named_bot_spec(spec: &str) -> Result<(Option<String>, BotSpec), String> {
    let (name, rest) = poker_arena_cli::split_named_spec(spec)?;
    Ok((name, parse_bot_spec(rest)?))
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
        "greedy" => Ok(BotSpec::Builtin(BuiltinKind::Greedy)),
        "filler" => Ok(BotSpec::Builtin(BuiltinKind::Filler)),
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
            "unknown builtin bot {other:?} in --bot {spec:?} (expected greedy/filler/random)"
        )),
    }
}

/// A bot that exists but isn't named yet.
enum Pending {
    Builtin(BuiltinKind),
    Wire(OfcWireBot),
}

/// Connects/spawns every wire bot (sequentially, in `--bot` order, so the
/// order connections are expected in is predictable) and names the whole
/// field, disambiguating duplicates with `-2`, `-3`… suffixes.
///
/// Names are operator-assigned (`NAME@spec`); bots carry no identity of
/// their own. Unnamed specs default to the builtin kind, `tcp-PORT`, or
/// positional `bot-N`.
fn build_bots(
    specs: Vec<(Option<String>, BotSpec)>,
    hello: &OfcArenaMsg,
    timeout: Option<Duration>,
    middle: MiddleKind,
) -> Result<Vec<Box<dyn OfcBot>>, String> {
    // A bot that has to be started (or a human that has to start it) deserves
    // more slack than a single decision does.
    let handshake = timeout.unwrap_or_default().max(Duration::from_secs(10));

    let mut names = Vec::with_capacity(specs.len());
    let mut pending = Vec::with_capacity(specs.len());
    for (i, (name, spec)) in specs.into_iter().enumerate() {
        let default_name = match &spec {
            BotSpec::Builtin(kind) => kind.base_name().to_string(),
            BotSpec::Tcp(port) => format!("tcp-{port}"),
            BotSpec::Cmd(_) => format!("bot-{}", i + 1),
        };
        names.push(name.unwrap_or(default_name));
        pending.push(match spec {
            BotSpec::Builtin(kind) => Pending::Builtin(kind),
            BotSpec::Tcp(port) => {
                eprintln!("waiting for a bot on 127.0.0.1:{port} ...");
                let mut bot = OfcWireBot::listen_tcp(port, hello.clone(), handshake)
                    .map_err(|e| e.to_string())?;
                bot.set_timeout(timeout);
                Pending::Wire(bot)
            }
            BotSpec::Cmd(command) => {
                let mut bot = OfcWireBot::spawn_cmd(&command, hello.clone(), handshake)
                    .map_err(|e| e.to_string())?;
                bot.set_timeout(timeout);
                Pending::Wire(bot)
            }
        });
    }

    Ok(pending
        .into_iter()
        .zip(poker_arena_cli::disambiguate(&names))
        .enumerate()
        .map(|(i, (p, name))| match p {
            Pending::Builtin(kind) => kind.build(name, i, middle),
            Pending::Wire(mut bot) => {
                bot.set_name(name);
                Box::new(bot) as Box<dyn OfcBot>
            }
        })
        .collect())
}

fn run(args: RunArgs) -> Result<ExitCode, String> {
    let spec: OfcSpec = *find(&args.game)
        .ok_or_else(|| format!("unknown game {:?} (see `poker-arena-ofc games`)", args.game))?;

    let (seat_min, seat_max) = (spec.min_seats, spec.max_seats);
    if args.bots.len() < seat_min || args.bots.len() > seat_max {
        return Err(format!(
            "{} --bot entries given, but {} supports {}..={} seats",
            args.bots.len(),
            spec.name,
            seat_min,
            seat_max
        ));
    }

    let specs: Vec<(Option<String>, BotSpec)> = args
        .bots
        .iter()
        .map(|s| parse_named_bot_spec(s))
        .collect::<Result<_, _>>()?;
    let kinds: Vec<String> = specs.iter().map(|(_, spec)| spec_kind(spec)).collect();

    let timeout = (args.timeout_ms > 0).then(|| Duration::from_millis(args.timeout_ms));
    let hello = OfcArenaMsg::Hello {
        proto: PROTO_VERSION,
        game_id: spec.id.to_string(),
        seat_count: args.bots.len(),
        timeout_ms: timeout.map(|d| d.as_millis() as u64),
    };
    let mut bots = build_bots(specs, &hello, timeout, spec.middle)?;

    let seed = args.seed.unwrap_or_else(poker_arena_cli::entropy_seed);
    // The seed always lands on stderr, whether pinned or generated, so it is
    // visible regardless of --output; a generated seed additionally spells
    // out how to reproduce it, exactly like the betting binary's message.
    if args.seed.is_none() {
        eprintln!("seed: {seed} (pass --seed {seed} to reproduce this match)");
    } else {
        eprintln!("seed: {seed}");
    }
    let config = OfcMatchConfig {
        spec,
        hands: args.hands,
        seed,
        fault_policy: args.fault_policy.into(),
        timeout,
    };

    let selective =
        args.log_sample.is_some() || args.log_top_swings.is_some() || args.log_faults.is_some();
    if selective && args.log.is_none() {
        return Err("--log-sample/--log-top-swings/--log-faults require --log FILE".to_string());
    }
    if args.log_sample == Some(0) {
        return Err("--log-sample must be >= 1".to_string());
    }

    let mut log_sink: Option<Box<dyn OfcEventSink>> = match &args.log {
        Some(path) => {
            let file = File::create(path)
                .map_err(|e| format!("failed to create log file {}: {e}", path.display()))?;
            let writer = BufWriter::new(file);
            if selective {
                let selection = OfcLogSelection {
                    sample_first_hands: args.log_sample,
                    top_swings: args.log_top_swings,
                    fault_hands: args.log_faults.unwrap_or(100),
                };
                Some(Box::new(OfcSelectiveLog::new(writer, selection)))
            } else {
                Some(Box::new(OfcJsonLog::new(writer)))
            }
        }
        None => None,
    };
    let mut sinks: Vec<&mut dyn OfcEventSink> = match &mut log_sink {
        Some(boxed) => vec![boxed.as_mut()],
        None => Vec::new(),
    };

    if args.progress_secs < 0.0 || !args.progress_secs.is_finite() {
        return Err("--progress-secs must be a finite number >= 0".to_string());
    }
    let cadence_set = args.progress_every > 0 || args.progress_secs > 0.0;
    if args.progress_json && !cadence_set {
        return Err(
            "--progress-json requires a cadence: --progress-every and/or --progress-secs"
                .to_string(),
        );
    }
    let progress_every = args.progress_every;
    let progress_secs = args.progress_secs;
    let progress_json = args.progress_json;
    let mut last_emit = std::time::Instant::now();
    let mut report_progress = move |p: &OfcProgress<'_>| {
        let hand_due = progress_every > 0 && p.hands_done.is_multiple_of(progress_every);
        let time_due = progress_secs > 0.0 && last_emit.elapsed().as_secs_f64() >= progress_secs;
        if !hand_due && !time_due {
            return;
        }
        last_emit = std::time::Instant::now();
        if progress_json {
            let line = ofc_progress_report(p);
            eprintln!(
                "{}",
                serde_json::to_string(&line).expect("progress serialization is infallible")
            );
        } else {
            eprintln!("{} hands played", p.hands_done);
        }
    };
    let on_progress: Option<&mut dyn FnMut(&OfcProgress<'_>)> = if cadence_set {
        Some(&mut report_progress)
    } else {
        None
    };

    let result =
        run_ofc_match(&config, &mut bots, &mut sinks, on_progress).map_err(|e| e.to_string())?;
    drop(sinks);
    drop(log_sink);

    match args.output {
        OutputArg::Human => {
            print_report(&spec, seed, &result);
        }
        OutputArg::Json => {
            let report = ofc_match_report(&config, seed, &kinds, &result);
            println!(
                "{}",
                serde_json::to_string(&report).expect("report serialization is infallible")
            );
        }
    }

    if let Some(offender) = result.forfeited_by {
        eprintln!("{} forfeited the match", result.outcomes[offender].name);
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

fn print_report(spec: &OfcSpec, seed: u64, result: &OfcMatchResult) {
    println!(
        "game: {} | hands: {} | seed: {}",
        spec.name, result.hands_played, seed
    );
    println!(
        "{:<16} {:>10} {:>10} {:>24} {:>7} {:>6} {:>7} {:>10} {:>7}",
        "bot",
        "hands",
        "points",
        "pts/hand (±ci95)",
        "fouls",
        "fls",
        "scoops",
        "royalties",
        "faults"
    );
    for o in &result.outcomes {
        let field = match o.stats.ci95_half_width() {
            Some(h) => format!("{:+.3} ± {:.3}", o.stats.mean(), h),
            None => format!("{:+.3} ± n/a", o.stats.mean()),
        };
        println!(
            "{:<16} {:>10} {:>10} {:>24} {:>7} {:>6} {:>7} {:>10} {:>7}",
            o.name,
            result.hands_played,
            o.total_points,
            field,
            o.fouls,
            o.fantasylands,
            o.scoops,
            o.royalties,
            o.faults
        );
    }

    if result.forfeited_by.is_none() && result.outcomes.len() == 2 {
        let leader = if result.outcomes[0].stats.mean() >= result.outcomes[1].stats.mean() {
            0
        } else {
            1
        };
        let significant = result.outcomes[leader]
            .stats
            .ci95_half_width()
            .is_some_and(|h| result.outcomes[leader].stats.mean().abs() > h);
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
