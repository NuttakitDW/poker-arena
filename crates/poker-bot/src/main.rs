//! The `poker-bot` binary: a wire bot for every poker-arena game.
//!
//! Usage:
//! - `poker-bot [--tcp HOST:PORT] [--seed N] [--blueprints DIR]` — play.
//! - `poker-bot train [--game ID] [--seconds N] [--out DIR] [--seed N]` —
//!   train MCCFR blueprints; without `--game`, every betting game trains
//!   in ascending abstract-tree-size order (smallest first).
//! - `poker-bot plans` — print the per-game abstraction table.
//!
//! Default transport is stdio (arena → stdin, bot → stdout; stderr free
//! for logging), so `--bot cmd:"poker-bot"` works as-is; `--tcp` connects
//! out instead. The first `hello` line decides which protocol this session
//! speaks: a betting hello carries `stakes`, an OFC hello does not, and
//! the bot answers whichever arrives — one binary for both families.
//! At play time, `--blueprints` (default `./blueprints` when it exists)
//! names the directory of trained strategies.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;

use poker_bot::abstraction::{TREE_BUDGET, all_plans};

fn main() -> ExitCode {
    let mut seed = 1u64;
    let mut tcp: Option<String> = None;
    let mut plans = false;
    let mut train = false;
    let mut game: Option<String> = None;
    let mut seconds = 60u64;
    let mut out = PathBuf::from("blueprints");
    let mut blueprints: Option<PathBuf> = None;
    let mut trust_unvalidated = false;
    let mut validate_only = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "plans" => plans = true,
            "train" => train = true,
            "--game" => game = Some(next_value(&mut args, "--game")),
            "--seconds" => {
                seconds = next_value(&mut args, "--seconds")
                    .parse()
                    .unwrap_or_else(|_| fail("--seconds expects a number"));
            }
            "--out" => out = PathBuf::from(next_value(&mut args, "--out")),
            "--validate-only" => validate_only = true,
            "--blueprints" => {
                blueprints = Some(PathBuf::from(next_value(&mut args, "--blueprints")))
            }
            // For the trainer's validation matches only: play an
            // unvalidated blueprint so it can earn (or fail) its stamp.
            "--trust-blueprints" => trust_unvalidated = true,
            "--tcp" => tcp = Some(next_value(&mut args, "--tcp")),
            "--seed" => {
                seed = next_value(&mut args, "--seed")
                    .parse()
                    .unwrap_or_else(|_| fail("--seed expects a number"));
            }
            other => fail(&format!("unknown argument {other:?}")),
        }
    }

    if plans {
        print_plans();
        return ExitCode::SUCCESS;
    }
    if train {
        return run_training(game.as_deref(), seconds, &out, seed, validate_only);
    }

    // Playing: default to ./blueprints when present and not overridden.
    let blueprints = blueprints.or_else(|| {
        let default = PathBuf::from("blueprints");
        default.is_dir().then_some(default)
    });

    let outcome = match &tcp {
        Some(addr) => match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                match stream.try_clone() {
                    Ok(reader) => {
                        let mut writer = stream;
                        play(
                            &mut BufReader::new(reader),
                            &mut writer,
                            seed,
                            blueprints.as_deref(),
                            trust_unvalidated,
                        )
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(format!("could not connect to {addr}: {e}")),
        },
        None => play(
            &mut BufReader::new(std::io::stdin()),
            &mut std::io::stdout(),
            seed,
            blueprints.as_deref(),
            trust_unvalidated,
        ),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("poker-bot: {msg}");
            ExitCode::from(1)
        }
    }
}

/// Read the first line, decide which protocol is being spoken, and hand the
/// session to the right loop.
fn play<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    seed: u64,
    blueprints: Option<&std::path::Path>,
    trust_unvalidated: bool,
) -> Result<(), String> {
    let mut first = String::new();
    loop {
        first.clear();
        let n = reader.read_line(&mut first).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(()); // closed before hello: nothing to do
        }
        if !first.trim().is_empty() {
            break;
        }
    }

    let probe: serde_json::Value =
        serde_json::from_str(first.trim()).map_err(|e| format!("malformed hello: {e}"))?;
    if probe.get("t").and_then(|t| t.as_str()) != Some("hello") {
        return Err(format!("expected a hello first, got: {}", first.trim()));
    }

    if probe.get("stakes").is_some() {
        let hello = serde_json::from_str(first.trim()).map_err(|e| e.to_string())?;
        poker_bot::betting::run(reader, writer, hello, seed, blueprints, trust_unvalidated)
    } else {
        let hello = serde_json::from_str(first.trim()).map_err(|e| e.to_string())?;
        poker_bot::ofc::run(reader, writer, hello)
    }
}

/// Train blueprints: one game, or every betting game smallest-tree-first.
/// `validate_only` skips training and re-stamps the existing files (e.g.
/// to re-validate with a bigger sample after a gate change).
fn run_training(
    game: Option<&str>,
    seconds: u64,
    out: &std::path::Path,
    seed: u64,
    validate_only: bool,
) -> ExitCode {
    use poker_bot::blueprint::Blueprint;
    use poker_bot::cfr::Trainer;
    use poker_core::game::spec::GameSpec;
    use poker_wire::game::Stakes;

    let stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
        ante: 0,
    };
    let mut order: Vec<_> = all_plans();
    order.sort_by(|a, b| a.tree_size().total_cmp(&b.tree_size()));
    let queue: Vec<&str> = match game {
        Some(id) => vec![
            order
                .iter()
                .map(|plan| plan.game_id)
                .find(|known| *known == id)
                .unwrap_or_else(|| fail(&format!("unknown game {id:?}"))),
        ],
        None => order.iter().map(|plan| plan.game_id).collect(),
    };

    for game_id in queue {
        let spec = GameSpec::by_id(game_id, stakes).expect("registry id");
        let started = std::time::Instant::now();
        let path = out.join(Blueprint::file_name(game_id));
        let mut blueprint = if validate_only {
            match Blueprint::load(&path) {
                Ok(blueprint) => blueprint,
                Err(e) => fail(&format!("loading {}: {e}", path.display())),
            }
        } else {
            let mut trainer = Trainer::new(spec.clone(), 10_000, seed);
            trainer.run_for(std::time::Duration::from_secs(seconds));
            trainer.blueprint()
        };
        if let Err(e) = blueprint.save(&path) {
            fail(&format!("saving {}: {e}", path.display()));
        }

        // The blueprint has to beat the bot it would replace: play it
        // against the plain equity heuristic and stamp the measured edge
        // with its confidence interval. Only a statistically significant
        // win activates it at match time.
        let (edge, ci) = match validate(&spec, out) {
            Ok(measured) => measured,
            Err(e) => fail(&format!("{game_id}: validation match failed: {e}")),
        };
        blueprint.validated_edge = Some(edge);
        blueprint.validated_ci = Some(ci);
        if let Err(e) = blueprint.save(&path) {
            fail(&format!("saving {}: {e}", path.display()));
        }
        println!(
            "{game_id:<18} {:>9} iterations  {:>7} infosets  edge {edge:>+9.1} ± {ci:>7.1}/100 {}  {:>6.1}s",
            blueprint.iterations,
            blueprint.strategy.len(),
            if blueprint.trusted() {
                "TRUSTED "
            } else {
                "fallback"
            },
            started.elapsed().as_secs_f64(),
        );
    }
    ExitCode::SUCCESS
}

/// Play the just-saved (unstamped) blueprint against the equity fallback
/// over a duplicate-dealt match; returns `(edge, ci95)` for the blueprint
/// side in the game's rate unit per 100 hands. Big-bet games get a much
/// larger sample — their per-hand variance is an order of magnitude higher
/// than fixed limit, and an undersized sample would stamp noise.
fn validate(
    spec: &poker_core::game::GameSpec,
    blueprints: &std::path::Path,
) -> Result<(f64, f64), String> {
    use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
    use poker_arena::remote::WireBot;
    use poker_arena::runner::run_match;
    use poker_wire::message::ArenaMsg;
    use std::time::Duration;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let timeout = Duration::from_secs(10);
    let hello = |_: &str| ArenaMsg::Hello {
        proto: poker_wire::PROTO_VERSION,
        game_id: spec.id.to_string(),
        stakes: spec.stakes,
        betting: spec.betting,
        seat_count: 2,
        starting_stack: 10_000,
        timeout_ms: Some(timeout.as_millis() as u64),
    };
    let candidate = format!(
        "{} --trust-blueprints --blueprints {}",
        exe.display(),
        blueprints.display()
    );
    let fallback = format!("{} --blueprints /nonexistent", exe.display());
    let spawn = |command: &str, name: &str| -> Result<WireBot, String> {
        let mut bot =
            WireBot::spawn_cmd(command, hello(name), timeout).map_err(|e| e.to_string())?;
        bot.set_name(name);
        bot.set_timeout(Some(timeout));
        Ok(bot)
    };
    let mut bots: Vec<Box<dyn poker_arena::bot::Bot>> = vec![
        Box::new(spawn(&candidate, "candidate")?),
        Box::new(spawn(&fallback, "fallback")?),
    ];
    let decks = match spec.betting {
        poker_wire::game::BettingKind::FixedLimit { .. } => 250,
        poker_wire::game::BettingKind::NoLimit | poker_wire::game::BettingKind::PotLimit => 1_000,
    };
    let config = MatchConfig {
        spec: spec.clone(),
        decks,
        seed: 424_242,
        dealing: DealingMode::Duplicate,
        starting_stack: 10_000,
        fault_policy: FaultPolicy::Substitute,
        timeout: Some(timeout),
    };
    let result = run_match(&config, &mut bots, None, None).map_err(|e| e.to_string())?;
    let (divisor, _) = spec.rate_unit();
    let per100 = 100.0 / divisor as f64;
    let stats = &result.outcomes[0].stats;
    let edge = stats.mean() * per100;
    let ci = stats.ci95_half_width().unwrap_or(f64::INFINITY) * per100;
    Ok((edge, ci))
}

/// The abstraction table: one row per game, and the budget every tree must
/// fit inside.
fn print_plans() {
    println!(
        "{:<18} {:>12}  streets (buckets x sequences x draws)",
        "game", "tree size"
    );
    for plan in all_plans() {
        let streets: Vec<String> = plan
            .streets
            .iter()
            .map(|s| {
                format!(
                    "{}:{}x{}x{}",
                    s.label, s.buckets, s.bet_sequences, s.draw_options
                )
            })
            .collect();
        println!(
            "{:<18} {:>12.3e}  {}",
            plan.game_id,
            plan.tree_size(),
            streets.join(" ")
        );
    }
    println!("budget: {TREE_BUDGET:.0e} nodes per game (single-perspective abstract tree)");
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    args.next()
        .unwrap_or_else(|| fail(&format!("{flag} expects a value")))
}

fn fail(msg: &str) -> ! {
    eprintln!("poker-bot: {msg}");
    std::process::exit(1)
}
