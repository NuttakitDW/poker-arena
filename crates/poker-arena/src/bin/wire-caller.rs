//! Reference wire bot: the check/call strategy, spoken over the wire
//! protocol. Doubles as the fixture the arena's wire tests play against.
//!
//! Usage: `wire-caller [--tcp HOST:PORT] [--name NAME] [--sleep-ms N]`.
//! Default transport is stdio (arena → stdin, bot → stdout); `--sleep-ms`
//! stalls before every action, which is how tests provoke timeouts.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

use poker_core::game::Action;
use poker_wire::framing::{WireError, read_msg, write_msg};
use poker_wire::message::{ArenaMsg, BotMsg};

fn main() -> ExitCode {
    let mut name = "wire-caller".to_string();
    let mut sleep_ms = 0u64;
    let mut tcp: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => name = next_value(&mut args, "--name"),
            "--tcp" => tcp = Some(next_value(&mut args, "--tcp")),
            "--sleep-ms" => {
                sleep_ms = next_value(&mut args, "--sleep-ms")
                    .parse()
                    .unwrap_or_else(|_| fail("--sleep-ms expects a number"));
            }
            other => fail(&format!("unknown argument {other:?}")),
        }
    }

    let outcome = match &tcp {
        Some(addr) => match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                match stream.try_clone() {
                    Ok(reader) => play(BufReader::new(reader), stream, &name, sleep_ms),
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(format!("could not connect to {addr}: {e}")),
        },
        None => play(
            BufReader::new(std::io::stdin()),
            std::io::stdout(),
            &name,
            sleep_ms,
        ),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("wire-caller: {msg}");
            ExitCode::from(1)
        }
    }
}

/// Read arena messages until the match ends or the stream closes.
fn play<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    name: &str,
    sleep_ms: u64,
) -> Result<(), String> {
    loop {
        let msg = match read_msg::<_, ArenaMsg>(&mut reader) {
            Ok(msg) => msg,
            Err(WireError::Closed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        let reply = match msg {
            ArenaMsg::Hello { .. } => BotMsg::Join {
                name: name.to_string(),
            },
            ArenaMsg::Act { legal, .. } => {
                if sleep_ms > 0 {
                    std::thread::sleep(Duration::from_millis(sleep_ms));
                }
                let action = if legal.check {
                    Action::Check
                } else if legal.call.is_some() {
                    Action::Call
                } else {
                    Action::Fold
                };
                BotMsg::Action { action }
            }
            ArenaMsg::MatchEnd {} => return Ok(()),
            // hand-start / event / hand-end / anything newer: nothing to say.
            _ => continue,
        };
        write_msg(&mut writer, &reply).map_err(|e| e.to_string())?;
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    args.next()
        .unwrap_or_else(|| fail(&format!("{flag} expects a value")))
}

fn fail(msg: &str) -> ! {
    eprintln!("wire-caller: {msg}");
    std::process::exit(1)
}
