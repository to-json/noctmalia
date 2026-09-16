//! The client side of the noctmalia bridge.
//!
//! Thunderbird runs headless with the bridge MailExtension, which talks to a native-messaging shim.
//! The shim connects *out* to a unix socket and relays newline-delimited JSON, so this crate is the
//! listener: [`Bridge::spawn`] binds the socket and waits for Thunderbird to attach.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use noctmalia_bridge::{Bridge, Event};
//! use serde_json::json;
//!
//! let bridge = Bridge::spawn("/run/user/1000/noctmalia/bridge.sock")?;
//! let mut events = bridge.subscribe();
//! // Nothing can be called until Thunderbird says hello.
//! while !matches!(events.next().await, Some(Event::Hello(_))) {}
//! let books: serde_json::Value = bridge.call("addressBooks.list", json!({})).await?;
//! # Ok(())
//! # }
//! ```
//!
//! One connection is served at a time, which is all Thunderbird opens. Several *clients* sharing one
//! Thunderbird is the job of a hub in front of this; see `docs/findings.md` §2.

mod protocol;

pub use protocol::RemoteError;

use protocol::{Incoming, Request};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Gecko caps messages toward the extension at 1 MiB; the shim rejects anything larger, so catch it
/// here where the method name is still known.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Buffered events. A client that falls this far behind is told to resync rather than shown stale
/// state ([`Event::Lagged`]).
const EVENT_CAPACITY: usize = 256;

/// The socket is ours and the container attaches as the same uid; no reason to let the world in.
const SOCKET_MODE: u32 = 0o660;

/// A connection that arrives while one is being served replaces it, but not one that arrives
/// this soon after the last attach: see the accept branch in [`Server::serve`].
pub const REPLACE_AFTER: Duration = Duration::from_secs(5);

/// How long a call may wait for its reply before it is failed here, as [`Error::TimedOut`].
///
/// Without this a call whose reply never comes is a spinner forever, and there is no way to
/// tell that from Thunderbird being slow. The slowest thing the extension does on purpose — driving
/// a compose window to readiness — gives up after ten seconds of its own, so anything past this
/// is a reply that is not coming.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Something that happened on the bridge, independent of any request.
#[derive(Debug, Clone)]
pub enum Event {
    /// Thunderbird attached and sent `bridge.hello`. Its payload is `bridge.info`. Resync all state
    /// on every hello: it also fires when Thunderbird restarts under a still-open socket.
    Hello(Value),
    /// The connection dropped. Every call in flight has already failed with [`Error::NotConnected`].
    Lost,
    /// A forwarded Thunderbird event, e.g. `contacts.onUpdated`.
    Notify { name: String, data: Value },
    /// The receiver fell behind and missed `count` events. Resync.
    Lagged(u64),
}

/// Why a call failed.
#[derive(Debug, Clone)]
pub enum Error {
    /// Thunderbird is not attached, or dropped while the call was in flight.
    NotConnected,
    /// Thunderbird (or the shim) reported an error.
    Remote(RemoteError),
    /// The request is over [`MAX_REQUEST_BYTES`].
    TooLarge { bytes: usize, method: String },
    /// The reply did not fit the expected shape.
    Decode(String),
    /// No reply came within the call timeout ([`CALL_TIMEOUT`], or what the bridge was spawned
    /// with). The connection is still up; the call is simply not going to be answered.
    TimedOut { method: String, after: Duration },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotConnected => f.write_str("Thunderbird is not connected"),
            Error::Remote(error) => write!(f, "{error}"),
            Error::TooLarge { bytes, method } => {
                write!(f, "{method}: request is {bytes} bytes, over the {MAX_REQUEST_BYTES} byte limit")
            }
            Error::Decode(message) => write!(f, "unexpected reply: {message}"),
            Error::TimedOut { method, after } => write!(f, "{method}: no reply after {}s", after.as_secs()),
        }
    }
}

impl std::error::Error for Error {}

/// A subscription to [`Event`]s. Falling behind is reported as [`Event::Lagged`] rather than
/// silently skipping, so a client always knows when to resync.
pub struct Events(broadcast::Receiver<Event>);

impl Events {
    /// The next event, or `None` once the bridge is gone for good.
    pub async fn next(&mut self) -> Option<Event> {
        match self.0.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(count)) => Some(Event::Lagged(count)),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

/// One call's timing, for [`Stats`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub method: String,
    pub took: Duration,
}

/// What the bridge is doing right now, and what it has done since it started. Read with
/// [`Bridge::stats`]; a control socket can hand it to whoever asks, which is how "it is stuck" gets
/// to be a question with an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// The connection being served, numbered from 1 for the life of the process, or `None` while
    /// nothing is attached. A number that keeps climbing is a shim that keeps reconnecting.
    pub connection: Option<u64>,
    /// How many connections have ever attached.
    pub connections: u64,
    /// How long the current connection has been attached.
    pub attached_for: Option<Duration>,
    /// Calls sent and not yet answered.
    pub in_flight: usize,
    /// Calls sent, over every connection.
    pub calls: u64,
    /// Calls that came back as an error, or never came back.
    pub failed: u64,
    pub timed_out: u64,
    /// The slowest call answered so far, and the most recent one.
    pub slowest: Option<Sample>,
    pub last: Option<Sample>,
}

struct Call {
    method: String,
    params: Value,
    reply: oneshot::Sender<Result<Value, Error>>,
}

/// A handle to the bridge. Cheap to clone; every clone talks to the same socket.
#[derive(Clone)]
pub struct Bridge {
    calls: mpsc::UnboundedSender<Call>,
    events: broadcast::Sender<Event>,
    path: Arc<PathBuf>,
    shared: Arc<Mutex<Shared>>,
}

/// What the actor thread and [`Bridge::stats`] both touch.
#[derive(Default)]
struct Shared {
    stats: Stats,
    attached_at: Option<Instant>,
}

impl fmt::Debug for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bridge").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Bridge {
    /// Binds `path` and serves it on a background thread with its own runtime, so the caller needs
    /// no async context and iced's executor stays free.
    ///
    /// A socket file left behind by an earlier run is replaced: it is a dead inode nothing can
    /// connect to, and the shim retries forever, so it will find the new one.
    pub fn spawn(path: impl AsRef<Path>) -> io::Result<Bridge> {
        Bridge::spawn_with_timeout(path, CALL_TIMEOUT)
    }

    /// [`spawn`](Bridge::spawn), with a call timeout other than [`CALL_TIMEOUT`].
    pub fn spawn_with_timeout(path: impl AsRef<Path>, call_timeout: Duration) -> io::Result<Bridge> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_MODE))?;

        let (calls, call_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let server = Server {
            events: events.clone(),
            shared: Arc::clone(&shared),
            call_timeout,
            // Every call, with its latency, on stderr. For the moments "it feels slow" needs a
            // number and "it is stuck" needs a method name.
            trace: std::env::var_os("NOCTMALIA_TRACE").is_some(),
        };

        std::thread::Builder::new().name("noctmalia-bridge".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("bridge runtime");
            runtime.block_on(async move {
                let listener = UnixListener::from_std(listener).expect("bridge listener");
                server.serve(listener, call_rx).await;
            });
        })?;

        Ok(Bridge { calls, events, path: Arc::new(path), shared })
    }

    /// The socket this bridge listens on.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the bridge is doing, right now.
    pub fn stats(&self) -> Stats {
        let shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stats = shared.stats.clone();
        stats.attached_for = shared.attached_at.map(|at| at.elapsed());
        stats
    }

    /// Events from now on. Existing events are not replayed; a fresh subscriber should wait for the
    /// next [`Event::Hello`] or ask for state directly.
    pub fn subscribe(&self) -> Events {
        Events(self.events.subscribe())
    }

    /// Calls a bridge method and deserialises the result.
    pub async fn call<R: DeserializeOwned>(&self, method: &str, params: Value) -> Result<R, Error> {
        let result = self.call_raw(method, params).await?;
        serde_json::from_value(result).map_err(|error| Error::Decode(error.to_string()))
    }

    /// Calls a bridge method and returns the raw result.
    pub async fn call_raw(&self, method: &str, params: Value) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        let call = Call { method: method.to_string(), params, reply };
        // The only way the actor is gone is the process shutting down.
        self.calls.send(call).map_err(|_| Error::NotConnected)?;
        answer.await.unwrap_or(Err(Error::NotConnected))
    }
}

/// The actor: everything the thread behind [`Bridge::spawn`] needs that is not the listener.
struct Server {
    events: broadcast::Sender<Event>,
    shared: Arc<Mutex<Shared>>,
    call_timeout: Duration,
    trace: bool,
}

/// A call that has been written and not yet answered.
struct Pending {
    reply: oneshot::Sender<Result<Value, Error>>,
    method: String,
    since: Instant,
}

/// One attached connection: its number, its unanswered calls, and how to write to it.
struct Session<'a> {
    server: &'a Server,
    connection: u64,
    next_id: &'a mut u64,
    pending: HashMap<u64, Pending>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl Server {
    /// Serves one connection at a time. Calls made while nothing is attached fail immediately
    /// rather than queueing, so the UI can say "waiting for Thunderbird" instead of hanging on a
    /// spinner.
    ///
    /// A connection that arrives while one is being served replaces it. Only one shim is ever
    /// meant to exist, so a second one is the first one's replacement — a Thunderbird that
    /// restarted faster than its old socket closed, or an extension that reconnected twice — and
    /// serving the old one while the new one sits in the listen backlog is exactly the "attached
    /// but no hello" stall this used to have.
    async fn serve(&self, listener: UnixListener, mut calls: mpsc::UnboundedReceiver<Call>) {
        let mut next_id = 1u64;
        let mut connections = 0u64;
        let mut replacement: Option<tokio::net::UnixStream> = None;
        loop {
            let stream = match replacement.take() {
                Some(stream) => stream,
                None => loop {
                    tokio::select! {
                        accepted = listener.accept() => match accepted {
                            Ok((stream, _)) => break stream,
                            // A failed accept is per-connection (EMFILE, a peer gone between poll
                            // and accept); the listener is still good.
                            Err(_) => continue,
                        },
                        call = calls.recv() => match call {
                            Some(call) => { let _ = call.reply.send(Err(Error::NotConnected)); }
                            None => return,
                        },
                    }
                },
            };

            connections += 1;
            let (reader, writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut session = Session {
                server: self,
                connection: connections,
                next_id: &mut next_id,
                pending: HashMap::new(),
                writer,
            };
            let attached = Instant::now();
            let mut refused_at: Option<Instant> = None;
            self.update(|shared| {
                shared.stats.connection = Some(connections);
                shared.stats.connections = connections;
                shared.stats.in_flight = 0;
                shared.attached_at = Some(attached);
            });
            eprintln!("noctmalia-bridge: connection #{connections} attached");

            // Expiry is checked on a clock rather than per reply, so a call nobody answers still
            // times out on a connection that has gone quiet.
            let mut sweep = tokio::time::interval((self.call_timeout / 4).max(Duration::from_millis(50)));
            sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let why = loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => session.receive(&line),
                        // End of stream, or a malformed frame we cannot resynchronise from.
                        Ok(None) | Err(_) => break "closed",
                    },
                    call = calls.recv() => match call {
                        Some(call) => {
                            if !session.dispatch(call).await {
                                break "write failed";
                            }
                        }
                        None => return,
                    },
                    accepted = listener.accept() => match accepted {
                        // A newcomer replaces the connection being served — unless that one
                        // only just attached. Two shims pointed at one socket would otherwise
                        // replace each other forever, each reconnecting the instant it is
                        // dropped; the guard makes that a newcomer refused once a second and
                        // said so once a minute, instead of a storm.
                        Ok((stream, _)) if attached.elapsed() >= REPLACE_AFTER => {
                            replacement = Some(stream);
                            break "replaced by a newer connection";
                        }
                        Ok(_) => {
                            if refused_at.is_none_or(|at: Instant| at.elapsed() > Duration::from_secs(60)) {
                                eprintln!(
                                    "noctmalia-bridge: a second connection arrived within {}s of #{connections}; refused. \
                                     Is another Thunderbird pointed at this socket?",
                                    REPLACE_AFTER.as_secs()
                                );
                                refused_at = Some(Instant::now());
                            }
                        }
                        Err(_) => continue,
                    },
                    _ = sweep.tick() => session.expire(),
                }
            };

            let unanswered = session.close();
            self.update(|shared| {
                shared.stats.connection = None;
                shared.stats.in_flight = 0;
                shared.attached_at = None;
            });
            eprintln!(
                "noctmalia-bridge: connection #{connections} {why} after {:.1}s, {unanswered} call(s) unanswered",
                attached.elapsed().as_secs_f32()
            );
            let _ = self.events.send(Event::Lost);
        }
    }

    fn update(&self, change: impl FnOnce(&mut Shared)) {
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut shared);
    }
}

impl Session<'_> {
    /// Writes one request. Returns false if the connection is finished.
    async fn dispatch(&mut self, call: Call) -> bool {
        let id = *self.next_id;
        *self.next_id += 1;

        let request = Request { id, method: &call.method, params: &call.params };
        let mut line = match serde_json::to_vec(&request) {
            Ok(line) => line,
            Err(error) => {
                let _ = call.reply.send(Err(Error::Decode(error.to_string())));
                return true;
            }
        };
        if line.len() > MAX_REQUEST_BYTES {
            let error = Error::TooLarge { bytes: line.len(), method: call.method };
            let _ = call.reply.send(Err(error));
            return true;
        }
        line.push(b'\n');

        if self.writer.write_all(&line).await.is_err() {
            let _ = call.reply.send(Err(Error::NotConnected));
            return false;
        }
        self.server.update(|shared| {
            shared.stats.calls += 1;
            shared.stats.in_flight += 1;
        });
        self.pending.insert(id, Pending { reply: call.reply, method: call.method, since: Instant::now() });
        true
    }

    /// Routes one incoming line to its waiting call, or to the event stream.
    fn receive(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<Incoming>(line) else {
            return;
        };

        if let Some(name) = message.event {
            let data = message.data.unwrap_or(Value::Null);
            let event = if name == "bridge.hello" { Event::Hello(data) } else { Event::Notify { name, data } };
            let _ = self.server.events.send(event);
            return;
        }

        // The shim reports an oversized request with a null id when it cannot parse one out.
        let Some(pending) = message.id.and_then(|id| self.pending.remove(&id)) else {
            return;
        };
        let result = match (message.result, message.error) {
            (_, Some(error)) => Err(Error::Remote(error)),
            (Some(result), None) => Ok(result),
            (None, None) => Ok(Value::Null),
        };
        let took = pending.since.elapsed();
        let failed = result.is_err();
        if self.server.trace {
            let outcome = if failed { "error" } else { "ok" };
            eprintln!(
                "noctmalia-bridge: #{} {} {:.1}ms {outcome}",
                self.connection,
                pending.method,
                took.as_secs_f64() * 1e3
            );
        }
        let sample = Sample { method: pending.method, took };
        self.server.update(|shared| {
            let stats = &mut shared.stats;
            stats.in_flight = stats.in_flight.saturating_sub(1);
            stats.failed += u64::from(failed);
            if stats.slowest.as_ref().is_none_or(|slowest| slowest.took < took) {
                stats.slowest = Some(sample.clone());
            }
            stats.last = Some(sample);
        });
        let _ = pending.reply.send(result);
    }

    /// Fails every call that has waited longer than the timeout. Said on stderr regardless of
    /// tracing: a call that never comes back is the thing worth knowing about.
    fn expire(&mut self) {
        let timeout = self.server.call_timeout;
        let expired: Vec<u64> =
            self.pending.iter().filter(|(_, pending)| pending.since.elapsed() >= timeout).map(|(id, _)| *id).collect();
        for id in expired {
            let Some(pending) = self.pending.remove(&id) else { continue };
            let after = pending.since.elapsed();
            eprintln!(
                "noctmalia-bridge: #{} {} timed out after {:.1}s",
                self.connection,
                pending.method,
                after.as_secs_f32()
            );
            self.server.update(|shared| {
                shared.stats.in_flight = shared.stats.in_flight.saturating_sub(1);
                shared.stats.failed += 1;
                shared.stats.timed_out += 1;
            });
            let _ = pending.reply.send(Err(Error::TimedOut { method: pending.method, after }));
        }
    }

    /// Fails whatever is still in flight, and says how much that was.
    fn close(self) -> usize {
        let unanswered = self.pending.len();
        for pending in self.pending.into_values() {
            let _ = pending.reply.send(Err(Error::NotConnected));
        }
        unanswered
    }
}
