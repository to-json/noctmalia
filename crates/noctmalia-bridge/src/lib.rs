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
use std::sync::Arc;
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
        let serve_events = events.clone();

        std::thread::Builder::new().name("noctmalia-bridge".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("bridge runtime");
            runtime.block_on(async move {
                let listener = UnixListener::from_std(listener).expect("bridge listener");
                serve(listener, call_rx, serve_events).await;
            });
        })?;

        Ok(Bridge { calls, events, path: Arc::new(path) })
    }

    /// The socket this bridge listens on.
    pub fn path(&self) -> &Path {
        &self.path
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

/// Accepts one connection at a time. Calls made while nothing is attached fail immediately rather
/// than queueing, so the UI can say "waiting for Thunderbird" instead of hanging on a spinner.
async fn serve(listener: UnixListener, mut calls: mpsc::UnboundedReceiver<Call>, events: broadcast::Sender<Event>) {
    let mut next_id = 1u64;
    loop {
        let stream = loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => break stream,
                    // A failed accept is per-connection (EMFILE, a peer gone between poll and
                    // accept); the listener is still good.
                    Err(_) => continue,
                },
                call = calls.recv() => match call {
                    Some(call) => { let _ = call.reply.send(Err(Error::NotConnected)); }
                    None => return,
                },
            }
        };

        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let mut pending: HashMap<u64, oneshot::Sender<Result<Value, Error>>> = HashMap::new();

        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => receive(&line, &mut pending, &events),
                    // End of stream, or a malformed frame we cannot resynchronise from.
                    Ok(None) | Err(_) => break,
                },
                call = calls.recv() => match call {
                    Some(call) => {
                        if !dispatch(call, &mut next_id, &mut writer, &mut pending).await {
                            break;
                        }
                    }
                    None => return,
                },
            }
        }

        for reply in pending.into_values() {
            let _ = reply.send(Err(Error::NotConnected));
        }
        let _ = events.send(Event::Lost);
    }
}

/// Writes one request. Returns false if the connection is finished.
async fn dispatch(
    call: Call,
    next_id: &mut u64,
    writer: &mut (impl AsyncWriteExt + Unpin),
    pending: &mut HashMap<u64, oneshot::Sender<Result<Value, Error>>>,
) -> bool {
    let id = *next_id;
    *next_id += 1;

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

    if writer.write_all(&line).await.is_err() {
        let _ = call.reply.send(Err(Error::NotConnected));
        return false;
    }
    pending.insert(id, call.reply);
    true
}

/// Routes one incoming line to its waiting call, or to the event stream.
fn receive(
    line: &str,
    pending: &mut HashMap<u64, oneshot::Sender<Result<Value, Error>>>,
    events: &broadcast::Sender<Event>,
) {
    let Ok(message) = serde_json::from_str::<Incoming>(line) else {
        return;
    };

    if let Some(name) = message.event {
        let data = message.data.unwrap_or(Value::Null);
        let event = if name == "bridge.hello" { Event::Hello(data) } else { Event::Notify { name, data } };
        let _ = events.send(event);
        return;
    }

    // The shim reports an oversized request with a null id when it cannot parse one out.
    let Some(reply) = message.id.and_then(|id| pending.remove(&id)) else {
        return;
    };
    let result = match (message.result, message.error) {
        (_, Some(error)) => Err(Error::Remote(error)),
        (Some(result), None) => Ok(result),
        (None, None) => Ok(Value::Null),
    };
    let _ = reply.send(result);
}
