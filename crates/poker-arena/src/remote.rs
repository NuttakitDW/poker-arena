//! Out-of-process bots: the arena side of the wire protocol.
//!
//! [`WireBot`] adapts a bot speaking `WIRE_PROTOCOL.md` over a byte
//! stream to the in-process [`Bot`] trait, so [`crate::runner::run_match`]
//! never learns the difference. Two transports ship here — a TCP listener
//! ([`WireBot::listen_tcp`]) and a spawned subprocess talking over its stdio
//! ([`WireBot::spawn_cmd`]) — but both funnel into the transport-agnostic
//! [`WireBot::from_transport`], which is all the protocol logic there is.
//! The byte-stream mechanics (reader thread, mpsc channel, process/socket
//! teardown) live in [`crate::transport::LineTransport`]; `act` enforces the
//! per-action deadline with `recv_timeout` on its channel instead of socket
//! timeouts, which keeps the same code working for pipes.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use poker_core::game::Action;
use poker_wire::framing::{WireError, write_msg};
use poker_wire::message::{ArenaMsg, BotMsg, WireDecision};

use crate::bot::{ActionRequest, Bot, BotFault, HandEnd, HandStart};
use crate::transport::LineTransport;

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

/// A bot that lives behind a byte stream: a socket peer or a child process.
pub struct WireBot {
    name: String,
    transport: LineTransport<BotMsg>,
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
        let transport = LineTransport::from_io(reader, writer);
        WireBot::handshake(transport, hello, handshake_timeout)
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
        let transport = LineTransport::listen_tcp_on(listener).map_err(WireBotError::Accept)?;
        WireBot::handshake(transport, hello, handshake_timeout)
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
        let transport =
            LineTransport::spawn_cmd(command).map_err(|source| WireBotError::Spawn {
                command: command.to_string(),
                source,
            })?;
        // The child never became a bot if this fails, but nothing else will
        // reap it: `transport`'s `Drop` (kill-before-wait) runs right here.
        WireBot::handshake(transport, hello, handshake_timeout)
    }

    /// Send `hello` over an already-constructed transport and wait for
    /// `join`, up to `handshake_timeout`.
    fn handshake(
        mut transport: LineTransport<BotMsg>,
        hello: ArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<WireBot, WireBotError> {
        debug_assert!(
            matches!(hello, ArenaMsg::Hello { .. }),
            "from_transport expects ArenaMsg::Hello"
        );

        write_msg(&mut transport.writer, &hello)?;

        let deadline = Instant::now() + handshake_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match transport.rx.recv_timeout(remaining) {
                Ok(Ok(BotMsg::Join {})) => break,
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
        }

        Ok(WireBot {
            // Placeholder until the operator assigns the real name via
            // `set_name` (bots carry no identity of their own).
            name: "wire-bot".to_string(),
            transport,
            timeout: None,
            hand_no: 0,
        })
    }

    /// Per-action deadline used by [`Bot::act`]; `None` waits forever.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Override the joined name. The CLI uses this to apply the same
    /// duplicate-name disambiguation builtin bots get (two bots both calling
    /// themselves `caller` become `caller` and `caller-2`).
    /// Assign the bot's final competition name and inform it with an
    /// [`ArenaMsg::Joined`] acknowledgment. Called once per wire bot after
    /// every seat has connected (duplicate names are disambiguated across
    /// the whole field, so final names exist only then).
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.send(&ArenaMsg::Joined {
            name: self.name.clone(),
        });
    }

    /// Write one message, marking the bot dead if the transport rejects it.
    fn send(&mut self, msg: &ArenaMsg) {
        self.transport.send(msg);
    }

    /// Turn a reader-thread error into a fault, marking the bot dead when the
    /// failure is transport-level rather than a bad message.
    fn fault_from(&mut self, err: WireError) -> BotFault {
        match err {
            WireError::Parse { .. } | WireError::TooLong { .. } => {
                BotFault::Protocol(err.to_string())
            }
            WireError::Closed | WireError::Io(_) => {
                self.transport.dead = true;
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
            match self.transport.rx.try_recv() {
                Ok(Ok(_)) => continue,
                Ok(Err(err)) => return Some(self.fault_from(err)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.transport.dead = true;
                    return Some(BotFault::Disconnected);
                }
            }
        }
    }
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
            ev: event.clone(),
        };
        self.send(&msg);
    }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        if self.transport.dead {
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
        if self.transport.dead {
            return Err(BotFault::Disconnected);
        }

        let deadline = self.timeout.map(|t| Instant::now() + t);
        loop {
            let received = match deadline {
                Some(at) => self
                    .transport
                    .rx
                    .recv_timeout(at.saturating_duration_since(Instant::now())),
                None => self
                    .transport
                    .rx
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(Ok(BotMsg::Action { action })) => return Ok(action),
                // Stale or forward-compatible noise; keep waiting for a real
                // answer within the same deadline.
                Ok(Ok(BotMsg::Join { .. } | BotMsg::Unknown)) => continue,
                Ok(Err(err)) => return Err(self.fault_from(err)),
                Err(RecvTimeoutError::Timeout) => return Err(BotFault::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    self.transport.dead = true;
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
        // may already be gone, so failures are expected and ignored. This
        // must run before `transport` tears the child/socket down below, or
        // a spawned child could already be dead and unable to see it — since
        // `transport` is a field of `WireBot`, its `Drop` (kill-before-wait
        // the child, then socket shutdown) only runs *after* this method
        // body returns, which keeps that ordering without any extra code.
        if !self.transport.dead {
            let _ = write_msg(&mut self.transport.writer, &ArenaMsg::MatchEnd {});
        }
    }
}
