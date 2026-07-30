//! Out-of-process bots: the arena side of the wire protocol.
//!
//! [`WireBot`] adapts a bot speaking `WIRE_PROTOCOL.md` over a byte
//! stream to the in-process [`Bot`] trait, so [`crate::runner::run_match`]
//! never learns the difference. Two transports ship here — a TCP listener
//! ([`WireBot::listen_tcp`]) and a spawned subprocess talking over its stdio
//! ([`WireBot::spawn_cmd`]) — but both funnel into the transport-agnostic
//! [`WireBot::from_transport`], which is all the protocol logic there is.
//!
//! Reads happen on a dedicated thread that pushes every decoded [`BotMsg`]
//! (or the [`WireError`] that ended the stream) down an mpsc channel; `act`
//! then enforces the per-action deadline with `recv_timeout` instead of
//! socket timeouts, which keeps the same code working for pipes. Writes are
//! synchronous on the calling thread.

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::time::{Duration, Instant};

use poker_core::game::Action;
use poker_wire::framing::{WireError, read_msg, write_msg};
use poker_wire::message::{ArenaMsg, BotMsg, WireDecision, WireEvent};

use crate::bot::{ActionRequest, Bot, BotFault, HandEnd, HandStart};

/// Everything that can go wrong bringing a wire bot up. Once a bot is
/// connected and joined, failures are per-action [`BotFault`]s instead, and
/// the arena's fault policy decides what they cost.
#[derive(Debug, thiserror::Error)]
pub enum WireBotError {
    #[error("failed to listen on 127.0.0.1:{port}: {source}")]
    Bind { port: u16, source: std::io::Error },
    #[error("failed to accept a bot connection: {0}")]
    Accept(std::io::Error),
    #[error("failed to spawn bot command {command:?}: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("the bot did not join within {0:?}")]
    HandshakeTimeout(Duration),
    #[error("invalid join: {0}")]
    BadJoin(String),
}

/// Longest name a bot may claim in its `join` (see `WIRE_PROTOCOL.md`).
const MAX_NAME_CHARS: usize = 32;

/// A bot that lives behind a byte stream: a socket peer or a child process.
pub struct WireBot {
    name: String,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Result<BotMsg, WireError>>,
    /// Set once the transport is known to be gone; every later `act` fails
    /// fast with [`BotFault::Disconnected`] instead of waiting out a deadline
    /// per decision for the rest of the match.
    dead: bool,
    /// Present for [`WireBot::spawn_cmd`]: killed and reaped on drop.
    child: Option<Child>,
    /// Present for the TCP transports: a spare handle used only to shut the
    /// socket down on drop, so the reader thread parked in `read` wakes up
    /// even when the peer never closes its end.
    shutdown: Option<TcpStream>,
    timeout: Option<Duration>,
    /// Hand number from the most recent `hand_start`, needed to stamp `event`
    /// messages (a bare `Event` doesn't carry one).
    hand_no: u64,
}

impl WireBot {
    /// Perform the `hello` → `join` handshake over an arbitrary transport,
    /// spawn the reader thread, and return the ready bot.
    ///
    /// `hello` is sent verbatim and must be [`ArenaMsg::Hello`]; the caller
    /// owns the game description, so nothing is synthesized here.
    pub fn from_transport(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        hello: ArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<WireBot, WireBotError> {
        debug_assert!(
            matches!(hello, ArenaMsg::Hello { .. }),
            "from_transport expects ArenaMsg::Hello"
        );

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                match read_msg::<_, BotMsg>(&mut reader) {
                    Ok(msg) => {
                        if tx.send(Ok(msg)).is_err() {
                            return; // the WireBot is gone; stop reading.
                        }
                    }
                    // Any read error is terminal for the stream: framing is
                    // desynced (or the peer is gone), so report it once and
                    // let the channel close behind us.
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                }
            }
        });

        let mut writer: Box<dyn Write + Send> = Box::new(writer);
        write_msg(&mut writer, &hello)?;

        let deadline = Instant::now() + handshake_timeout;
        let name = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(Ok(BotMsg::Join { name })) => break validate_name(&name)?,
                // Forward compatibility: an unrecognized message before the
                // join is a no-op, not an error.
                Ok(Ok(BotMsg::Unknown)) => continue,
                Ok(Ok(BotMsg::Action { .. })) => {
                    return Err(WireBotError::BadJoin(
                        "the bot sent an action before joining".to_string(),
                    ));
                }
                Ok(Err(err)) => return Err(WireBotError::Wire(err)),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(WireBotError::HandshakeTimeout(handshake_timeout));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(WireBotError::BadJoin(
                        "the bot disconnected before joining".to_string(),
                    ));
                }
            }
        };

        Ok(WireBot {
            name,
            writer,
            rx,
            dead: false,
            child: None,
            shutdown: None,
            timeout: None,
            hand_no: 0,
        })
    }

    /// Bind `127.0.0.1:port`, accept exactly one connection, and handshake.
    pub fn listen_tcp(
        port: u16,
        hello: ArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<WireBot, WireBotError> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|source| WireBotError::Bind { port, source })?;
        WireBot::listen_tcp_on(listener, hello, handshake_timeout)
    }

    /// Like [`WireBot::listen_tcp`], but takes a listener that is already
    /// bound. Lets a caller pick an ephemeral port (`:0`), learn it from
    /// `local_addr`, and hand the bot the real port without a bind/connect
    /// race in between.
    pub fn listen_tcp_on(
        listener: TcpListener,
        hello: ArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<WireBot, WireBotError> {
        let (stream, _peer) = listener.accept().map_err(WireBotError::Accept)?;
        // JSON lines are tiny and strictly request/response; Nagle would add
        // a round-trip's worth of delay to every decision.
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone().map_err(WireBotError::Accept)?;
        let shutdown = stream.try_clone().map_err(WireBotError::Accept)?;
        let mut bot = WireBot::from_transport(reader, stream, hello, handshake_timeout)?;
        bot.shutdown = Some(shutdown);
        Ok(bot)
    }

    /// Spawn `sh -c command` with stdin/stdout piped (stderr inherited, so
    /// bot logging lands on the arena's stderr) and handshake over its stdio.
    /// The child is killed and reaped on drop, so a bot that ignores
    /// `match-end` never becomes a zombie.
    pub fn spawn_cmd(
        command: &str,
        hello: ArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<WireBot, WireBotError> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|source| WireBotError::Spawn {
                command: command.to_string(),
                source,
            })?;

        // `piped()` above guarantees both handles exist.
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        match WireBot::from_transport(stdout, stdin, hello, handshake_timeout) {
            Ok(mut bot) => {
                bot.child = Some(child);
                Ok(bot)
            }
            Err(err) => {
                // The child never became a bot, so nothing else will reap it.
                let _ = child.kill();
                let _ = child.wait();
                Err(err)
            }
        }
    }

    /// Per-action deadline used by [`Bot::act`]; `None` waits forever.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Override the joined name. The CLI uses this to apply the same
    /// duplicate-name disambiguation builtin bots get (two bots both calling
    /// themselves `caller` become `caller` and `caller-2`).
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Write one message, marking the bot dead if the transport rejects it.
    fn send(&mut self, msg: &ArenaMsg) {
        if self.dead {
            return;
        }
        if write_msg(&mut self.writer, msg).is_err() {
            self.dead = true;
        }
    }

    /// Turn a reader-thread error into a fault, marking the bot dead when the
    /// failure is transport-level rather than a bad message.
    fn fault_from(&mut self, err: WireError) -> BotFault {
        match err {
            WireError::Parse { .. } | WireError::TooLong { .. } => {
                BotFault::Protocol(err.to_string())
            }
            WireError::Closed | WireError::Io(_) => {
                self.dead = true;
                BotFault::Disconnected
            }
        }
    }

    /// Discard anything already queued before asking a new question.
    ///
    /// A [`BotFault::Timeout`] must not desync the connection: the bot may
    /// still be computing, and its (now useless) answer to request N can land
    /// at any time. Without this drain that answer would be read as the
    /// answer to request N+1 and every later decision would be off by one —
    /// the bot would appear to play a hand behind for the rest of the match.
    /// Nothing legitimate is ever waiting here, because a well-behaved bot
    /// only speaks when asked and each `act` consumes exactly one answer.
    fn drain_stale(&mut self) -> Option<BotFault> {
        loop {
            match self.rx.try_recv() {
                Ok(Ok(_)) => continue,
                Ok(Err(err)) => return Some(self.fault_from(err)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.dead = true;
                    return Some(BotFault::Disconnected);
                }
            }
        }
    }
}

/// Validate a joined name per `WIRE_PROTOCOL.md`: 1–32 characters after
/// trimming, no control characters.
fn validate_name(raw: &str) -> Result<String, WireBotError> {
    let name = raw.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_NAME_CHARS {
        return Err(WireBotError::BadJoin(format!(
            "name must be 1..={MAX_NAME_CHARS} characters, got {raw:?}"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(WireBotError::BadJoin(format!(
            "name must not contain control characters, got {raw:?}"
        )));
    }
    Ok(name.to_string())
}

impl Bot for WireBot {
    fn name(&self) -> &str {
        &self.name
    }

    fn hand_start(&mut self, info: &HandStart) {
        self.hand_no = info.hand_no;
        // Just "new hand, you sit here" — stacks/button/deals travel in the
        // event stream (the arena always seats the button at seat 0).
        let msg = ArenaMsg::HandStart {
            hand_no: info.hand_no,
            seat: info.seat,
        };
        self.send(&msg);
    }

    fn event(&mut self, event: &poker_core::game::Event) {
        let msg = ArenaMsg::Event {
            hand_no: self.hand_no,
            ev: WireEvent::from(event),
        };
        self.send(&msg);
    }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        if self.dead {
            return Err(BotFault::Disconnected);
        }
        if let Some(fault) = self.drain_stale() {
            return Err(fault);
        }

        // Table state travels only via the event stream; `act` carries just
        // the decision context (see ArenaMsg::Act docs).
        let msg = ArenaMsg::Act {
            hand_no: req.hand_no,
            seat: req.seat,
            decision: WireDecision::from(req.legal),
            deadline_ms: self.timeout.map(|d| d.as_millis() as u64),
        };
        self.send(&msg);
        if self.dead {
            return Err(BotFault::Disconnected);
        }

        let deadline = self.timeout.map(|t| Instant::now() + t);
        loop {
            let received = match deadline {
                Some(at) => self
                    .rx
                    .recv_timeout(at.saturating_duration_since(Instant::now())),
                None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(Ok(BotMsg::Action { action })) => return Ok(action),
                // Stale or forward-compatible noise; keep waiting for a real
                // answer within the same deadline.
                Ok(Ok(BotMsg::Join { .. } | BotMsg::Unknown)) => continue,
                Ok(Err(err)) => return Err(self.fault_from(err)),
                Err(RecvTimeoutError::Timeout) => return Err(BotFault::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    self.dead = true;
                    return Err(BotFault::Disconnected);
                }
            }
        }
    }

    fn hand_end(&mut self, result: &HandEnd) {
        let msg = ArenaMsg::HandEnd {
            hand_no: result.hand_no,
            nets: result.nets.clone(),
        };
        self.send(&msg);
    }
}

impl Drop for WireBot {
    fn drop(&mut self) {
        // Courtesy notice so a well-behaved bot can exit on its own; the peer
        // may already be gone, so failures are expected and ignored.
        if !self.dead {
            let _ = write_msg(&mut self.writer, &ArenaMsg::MatchEnd {});
        }
        // Kill before wait: a bot that ignores `match-end` would otherwise
        // hang the drop forever. Reaping here is what keeps `sh -c` children
        // from piling up as zombies across a tournament.
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // The reader thread exits on EOF. A child's EOF comes from the kill
        // above; a socket peer that never closes needs this nudge.
        if let Some(socket) = &self.shutdown {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_are_trimmed() {
        assert_eq!(validate_name("  bob  ").unwrap(), "bob");
        assert_eq!(
            validate_name(&"n".repeat(MAX_NAME_CHARS)).unwrap().len(),
            32
        );
    }

    #[test]
    fn empty_or_oversized_names_are_rejected() {
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"n".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn control_characters_are_rejected() {
        assert!(validate_name("bo\u{7}b").is_err());
        assert!(validate_name("bo\u{7f}b").is_err());
    }
}
