//! Out-of-process OFC bots: the arena side of the OFC wire protocol.
//!
//! [`OfcWireBot`] is [`crate::remote::WireBot`]'s counterpart for OFC — same
//! handshake, same stale-answer drain, same deadline handling, same
//! match-end-on-drop ordering, over the same
//! [`crate::transport::LineTransport`] — carrying [`OfcArenaMsg`] /
//! [`OfcBotMsg`] instead of the betting protocol's messages. Bring-up
//! failures share [`WireBotError`]: there is only one way for a wire bot to
//! fail to start, whichever protocol it speaks.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use poker_core::ofc::{OfcAction, OfcArenaMsg, OfcBotMsg, OfcDecision, OfcEvent};
use poker_wire::framing::{WireError, write_msg};

use crate::bot::BotFault;
use crate::ofc::bot::{OfcActionRequest, OfcBot, OfcHandEnd, OfcHandStart};
use crate::remote::WireBotError;
use crate::transport::LineTransport;

/// An OFC bot that lives behind a byte stream: a socket peer or a child
/// process.
pub struct OfcWireBot {
    name: String,
    transport: LineTransport<OfcBotMsg>,
    timeout: Option<Duration>,
    /// Hand number from the most recent `hand_start`, needed to stamp `event`
    /// messages (a bare `OfcEvent` doesn't carry one).
    hand_no: u64,
}

impl OfcWireBot {
    /// Perform the `hello` → `join` handshake over an arbitrary transport,
    /// spawn the reader thread, and return the ready bot.
    ///
    /// `hello` is sent verbatim and must be [`OfcArenaMsg::Hello`]; the
    /// caller owns the game description, so nothing is synthesized here.
    pub fn from_transport(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        hello: OfcArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<OfcWireBot, WireBotError> {
        let transport = LineTransport::from_io(reader, writer);
        OfcWireBot::handshake(transport, hello, handshake_timeout)
    }

    /// Bind `127.0.0.1:port`, accept exactly one connection, and handshake.
    pub fn listen_tcp(
        port: u16,
        hello: OfcArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<OfcWireBot, WireBotError> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|source| WireBotError::Bind { port, source })?;
        OfcWireBot::listen_tcp_on(listener, hello, handshake_timeout)
    }

    /// Like [`OfcWireBot::listen_tcp`], but takes a listener that is already
    /// bound. Lets a caller pick an ephemeral port (`:0`), learn it from
    /// `local_addr`, and hand the bot the real port without a bind/connect
    /// race in between.
    pub fn listen_tcp_on(
        listener: TcpListener,
        hello: OfcArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<OfcWireBot, WireBotError> {
        let transport = LineTransport::listen_tcp_on(listener).map_err(WireBotError::Accept)?;
        OfcWireBot::handshake(transport, hello, handshake_timeout)
    }

    /// Spawn `sh -c command` with stdin/stdout piped (stderr inherited, so
    /// bot logging lands on the arena's stderr) and handshake over its stdio.
    /// The child is killed and reaped on drop, so a bot that ignores
    /// `match-end` never becomes a zombie.
    pub fn spawn_cmd(
        command: &str,
        hello: OfcArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<OfcWireBot, WireBotError> {
        let transport =
            LineTransport::spawn_cmd(command).map_err(|source| WireBotError::Spawn {
                command: command.to_string(),
                source,
            })?;
        // The child never became a bot if this fails, but nothing else will
        // reap it: `transport`'s `Drop` (kill-before-wait) runs right here.
        OfcWireBot::handshake(transport, hello, handshake_timeout)
    }

    /// Send `hello` over an already-constructed transport and wait for
    /// `join`, up to `handshake_timeout`.
    fn handshake(
        mut transport: LineTransport<OfcBotMsg>,
        hello: OfcArenaMsg,
        handshake_timeout: Duration,
    ) -> Result<OfcWireBot, WireBotError> {
        debug_assert!(
            matches!(hello, OfcArenaMsg::Hello { .. }),
            "from_transport expects OfcArenaMsg::Hello"
        );

        write_msg(&mut transport.writer, &hello)?;

        let deadline = Instant::now() + handshake_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match transport.rx.recv_timeout(remaining) {
                Ok(Ok(OfcBotMsg::Join {})) => break,
                // Forward compatibility: an unrecognized message before the
                // join is a no-op, not an error.
                Ok(Ok(OfcBotMsg::Unknown)) => continue,
                Ok(Ok(OfcBotMsg::Action { .. })) => {
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

        Ok(OfcWireBot {
            // Placeholder until the operator assigns the real name via
            // `set_name` (bots carry no identity of their own).
            name: "ofc-wire-bot".to_string(),
            transport,
            timeout: None,
            hand_no: 0,
        })
    }

    /// Per-action deadline used by [`OfcBot::place`]; `None` waits forever.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Assign the bot's final competition name and inform it with an
    /// [`OfcArenaMsg::Joined`] acknowledgment. Called once per wire bot after
    /// every seat has connected (duplicate names are disambiguated across
    /// the whole field, so final names exist only then).
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.send(&OfcArenaMsg::Joined {
            name: self.name.clone(),
        });
    }

    /// Write one message, marking the bot dead if the transport rejects it.
    fn send(&mut self, msg: &OfcArenaMsg) {
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
    /// answer to request N+1 and every later decision would be off by one.
    /// Nothing legitimate is ever waiting here, because a well-behaved bot
    /// only speaks when asked and each `place` consumes exactly one answer.
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

impl OfcBot for OfcWireBot {
    fn name(&self) -> &str {
        &self.name
    }

    fn hand_start(&mut self, info: &OfcHandStart) {
        self.hand_no = info.hand_no;
        // Just "new hand, you sit here" — fantasyland, deals and placements
        // all travel in the event stream.
        let msg = OfcArenaMsg::HandStart {
            hand_no: info.hand_no,
            seat: info.seat,
        };
        self.send(&msg);
    }

    fn event(&mut self, event: &OfcEvent) {
        let msg = OfcArenaMsg::Event {
            hand_no: self.hand_no,
            ev: event.clone(),
        };
        self.send(&msg);
    }

    fn place(&mut self, req: &OfcActionRequest<'_>) -> Result<OfcAction, BotFault> {
        if self.transport.dead {
            return Err(BotFault::Disconnected);
        }
        if let Some(fault) = self.drain_stale() {
            return Err(fault);
        }

        // Board state travels only via the event stream; `act` carries just
        // the decision context (see OfcArenaMsg::Act docs).
        let msg = OfcArenaMsg::Act {
            hand_no: req.hand_no,
            seat: req.seat,
            decision: OfcDecision::Place {
                place: req.place,
                discard: req.discard,
            },
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
                Ok(Ok(OfcBotMsg::Action { action })) => return Ok(action),
                // Stale or forward-compatible noise; keep waiting for a real
                // answer within the same deadline.
                Ok(Ok(OfcBotMsg::Join { .. } | OfcBotMsg::Unknown)) => continue,
                Ok(Err(err)) => return Err(self.fault_from(err)),
                Err(RecvTimeoutError::Timeout) => return Err(BotFault::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    self.transport.dead = true;
                    return Err(BotFault::Disconnected);
                }
            }
        }
    }

    fn hand_end(&mut self, result: &OfcHandEnd) {
        let msg = OfcArenaMsg::HandEnd {
            hand_no: result.hand_no,
            points: result.points.clone(),
        };
        self.send(&msg);
    }
}

impl Drop for OfcWireBot {
    fn drop(&mut self) {
        // Courtesy notice so a well-behaved bot can exit on its own; the peer
        // may already be gone, so failures are expected and ignored. This
        // must run before `transport` tears the child/socket down — since
        // `transport` is a field, its `Drop` only runs after this method body
        // returns, which keeps that ordering without any extra code.
        if !self.transport.dead {
            let _ = write_msg(&mut self.transport.writer, &OfcArenaMsg::MatchEnd {});
        }
    }
}
