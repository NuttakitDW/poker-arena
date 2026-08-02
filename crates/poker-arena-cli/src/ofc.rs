//! The Open Face Chinese driver: `--bot` vocabulary, match setup and report
//! table for the four variants of [`poker_core::ofc`].
//!
//! Structure mirrors [`crate::bet`] throughout: same spec grammar, same
//! log/progress/output plumbing. What differs is entirely OFC vocabulary —
//! no blinds or chips, points instead, a different builtin set
//! (`greedy`/`filler`/`random`), and [`run_ofc_match`] in place of the
//! betting runner.

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;
use std::time::Duration;

use poker_arena::ofc::{
    OfcBot, OfcEventSink, OfcFiller, OfcGreedy, OfcJsonLog, OfcLogSelection, OfcMatchConfig,
    OfcMatchResult, OfcProgress, OfcRandom, OfcSelectiveLog, OfcWireBot, ofc_match_report,
    ofc_progress_report, run_ofc_match,
};
use poker_core::ofc::{MiddleKind, OfcArenaMsg, OfcSpec, PROTO_VERSION};

use crate::spec::{disambiguate, resolve_seed, split_named_spec};
use crate::{OutputArg, RunArgs};

/// Defaults for the shared flags this family reads differently from
/// [`crate::bet`]; both are `Option` in the clap surface for that reason.
const DEFAULT_HANDS: u64 = 1_000;
const DEFAULT_TIMEOUT_MS: u64 = 5_000;

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

/// Parse a `--bot` spec: split the optional `NAME@` prefix (shared with the
/// betting driver; see [`split_named_spec`]) and parse the rest as this
/// family's own spec kind.
fn parse_named_bot_spec(spec: &str) -> Result<(Option<String>, BotSpec), String> {
    let (name, rest) = split_named_spec(spec)?;
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
        .zip(disambiguate(&names))
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

pub fn run(args: RunArgs, spec: OfcSpec) -> Result<ExitCode, String> {
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

    let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let timeout = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms));
    let hello = OfcArenaMsg::Hello {
        proto: PROTO_VERSION,
        game_id: spec.id.to_string(),
        seat_count: args.bots.len(),
        timeout_ms: timeout.map(|d| d.as_millis() as u64),
    };
    let mut bots = build_bots(specs, &hello, timeout, spec.middle)?;

    let seed = resolve_seed(args.seed);
    let config = OfcMatchConfig {
        spec,
        hands: args.hands.unwrap_or(DEFAULT_HANDS),
        seed,
        fault_policy: args.fault_policy.into(),
        timeout,
    };

    args.check_log()?;
    let mut log_sink: Option<Box<dyn OfcEventSink>> = match &args.log {
        Some(path) => {
            let file = File::create(path)
                .map_err(|e| format!("failed to create log file {}: {e}", path.display()))?;
            let writer = BufWriter::new(file);
            if args.selective() {
                let selection = OfcLogSelection {
                    sample_first_hands: args.log_sample,
                    top_swings: args.log_top,
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

    let cadence_set = args.check_progress()?;
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

/// Compact `p50/p99/max` decision-timing field, one decimal each; `-` when
/// the bot never decided (`decision_stats.count() == 0`).
fn decision_field(stats: &poker_arena::stat::DecisionStats) -> String {
    match (stats.quantile(0.5), stats.quantile(0.99), stats.max_ms()) {
        (Some(p50), Some(p99), Some(max)) => format!("{p50:.1}/{p99:.1}/{max:.1}"),
        _ => "-".to_string(),
    }
}

fn print_report(spec: &OfcSpec, seed: u64, result: &OfcMatchResult) {
    println!(
        "game: {} | hands: {} | seed: {}",
        spec.name, result.hands_played, seed
    );
    println!(
        "{:<16} {:>10} {:>10} {:>24} {:>7} {:>6} {:>7} {:>10} {:>7} {:>16}",
        "bot",
        "hands",
        "points",
        "pts/hand (±ci95)",
        "fouls",
        "fls",
        "scoops",
        "royalties",
        "faults",
        "ms p50/p99/max"
    );
    for o in &result.outcomes {
        let field = match o.stats.ci95_half_width() {
            Some(h) => format!("{:+.3} ± {:.3}", o.stats.mean(), h),
            None => format!("{:+.3} ± n/a", o.stats.mean()),
        };
        println!(
            "{:<16} {:>10} {:>10} {:>24} {:>7} {:>6} {:>7} {:>10} {:>7} {:>16}",
            o.name,
            result.hands_played,
            o.total_points,
            field,
            o.fouls,
            o.fantasylands,
            o.scoops,
            o.royalties,
            o.faults,
            decision_field(&o.decision_stats)
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
