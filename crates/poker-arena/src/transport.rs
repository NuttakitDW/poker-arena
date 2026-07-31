//! Generic JSON-lines transport shared by every wire-protocol adapter.
//!
//! [`LineTransport`] owns the byte-stream mechanics common to any wire bot:
//! a reader thread that decodes every line as the protocol's own message
//! type (or the [`WireError`] that ended the stream) onto an mpsc channel,
//! a synchronous writer, and the process/socket handles needed to tear the
//! peer down cleanly. A caller enforces its own per-action deadline with
//! `recv_timeout` on [`LineTransport::rx`] instead of socket timeouts, which
//! keeps the same code working for pipes as for TCP. Handshakes, fault
//! mapping, and every other protocol-level decision belong to the caller
//! (see `remote.rs`).

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};

use poker_wire::framing::{WireError, read_msg, write_msg};

/// A JSON-lines peer: a boxed writer plus a reader thread's channel.
pub(crate) struct LineTransport<In> {
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) rx: Receiver<Result<In, WireError>>,
    /// Set once the transport is known to be gone: `send` becomes a no-op
    /// and the reader thread's channel will not produce a fresh answer.
    pub(crate) dead: bool,
    /// Present for a spawned subprocess: killed and reaped on drop.
    child: Option<Child>,
    /// Present for the TCP transports: a spare handle used only to shut the
    /// socket down on drop, so the reader thread parked in `read` wakes up
    /// even when the peer never closes its end.
    shutdown: Option<TcpStream>,
}

impl<In> LineTransport<In>
where
    In: serde::de::DeserializeOwned + Send + 'static,
{
    /// Wrap an arbitrary byte-stream reader/writer pair. Spawns a thread
    /// that decodes every line on `reader` as `In` and pushes it (or the
    /// [`WireError`] that ended the stream) down an mpsc channel read
    /// through [`Self::rx`]; writes go straight through on the caller's
    /// thread via [`Self::send`].
    pub(crate) fn from_io(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                match read_msg::<_, In>(&mut reader) {
                    Ok(msg) => {
                        if tx.send(Ok(msg)).is_err() {
                            return; // the transport is gone; stop reading.
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

        LineTransport {
            writer: Box::new(writer),
            rx,
            dead: false,
            child: None,
            shutdown: None,
        }
    }

    /// Accept one connection off an already-bound listener and wrap it.
    pub(crate) fn listen_tcp_on(listener: TcpListener) -> std::io::Result<Self> {
        let (stream, _peer) = listener.accept()?;
        // JSON lines are tiny and strictly request/response; Nagle would add
        // a round-trip's worth of delay to every decision.
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone()?;
        let shutdown = stream.try_clone()?;
        let mut transport = LineTransport::from_io(reader, stream);
        transport.shutdown = Some(shutdown);
        Ok(transport)
    }

    /// Spawn `sh -c command` with stdin/stdout piped (stderr inherited, so
    /// bot logging lands on the arena's stderr) and wrap its stdio. If the
    /// caller's later handshake fails, the returned transport's `Drop`
    /// (kill-before-wait) reaps the child — there is no separate cleanup
    /// path for that failure.
    pub(crate) fn spawn_cmd(command: &str) -> std::io::Result<Self> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        // `piped()` above guarantees both handles exist.
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let mut transport = LineTransport::from_io(stdout, stdin);
        transport.child = Some(child);
        Ok(transport)
    }

    /// Write one message, marking the transport dead if the peer rejects it.
    pub(crate) fn send<M: serde::Serialize>(&mut self, msg: &M) {
        if self.dead {
            return;
        }
        if write_msg(&mut self.writer, msg).is_err() {
            self.dead = true;
        }
    }
}

impl<In> Drop for LineTransport<In> {
    fn drop(&mut self) {
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
