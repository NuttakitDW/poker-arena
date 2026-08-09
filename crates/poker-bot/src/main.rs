//! The `poker-bot` binary: a wire bot for every poker-arena game.
//!
//! Usage: `poker-bot [--tcp HOST:PORT] [--seed N]`, or `poker-bot plans`.
//!
//! Default transport is stdio (arena → stdin, bot → stdout; stderr free
//! for logging), so `--bot cmd:"poker-bot"` works as-is; `--tcp` connects
//! out instead. The first `hello` line decides which protocol this session
//! speaks: a betting hello carries `stakes`, an OFC hello does not, and
//! the bot answers whichever arrives — one binary for both families.
//!
//! `poker-bot plans` prints the per-game abstraction plan (buckets,
//! betting sequences, draw options, abstract tree size) and exits.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::ExitCode;

use poker_bot::abstraction::{TREE_BUDGET, all_plans};

fn main() -> ExitCode {
    let mut seed = 1u64;
    let mut tcp: Option<String> = None;
    let mut plans = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "plans" => plans = true,
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

    let outcome = match &tcp {
        Some(addr) => match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                match stream.try_clone() {
                    Ok(reader) => {
                        let mut writer = stream;
                        play(&mut BufReader::new(reader), &mut writer, seed)
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
fn play<R: BufRead, W: Write>(reader: &mut R, writer: &mut W, seed: u64) -> Result<(), String> {
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
        poker_bot::betting::run(reader, writer, hello, seed)
    } else {
        let hello = serde_json::from_str(first.trim()).map_err(|e| e.to_string())?;
        poker_bot::ofc::run(reader, writer, hello)
    }
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
