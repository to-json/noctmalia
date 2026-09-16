//! A local scripting socket — `docs/scripting-socket-plan.md`.
//!
//! Distinct from `noctmalia_bridge::Bridge`, which is the *Thunderbird* transport and stays
//! exactly what it is: only one client may hold that socket. This one is ours to be as
//! permissive with as we like on the wire, because we are the only thing on either end of it that
//! matters for trust — but not on *which commands it can run*: `run` only ever executes something
//! `docs/command-palette-plan.md`'s registry has deliberately marked `exposed_to_socket`, so the
//! palette's own exhaustiveness never becomes this socket's trust boundary by accident.
//!
//! Personal automation, not a multi-client daemon: file permissions (`0600`, this uid only) are
//! the whole auth story, the same shape `docs/design.md` chose for the bridge socket and rejected
//! a token or a port for. NDJSON, one request per line, mirroring
//! `noctmalia_bridge::protocol`'s own `{id, method, params}` / `{id, result|error}` shape.
//!
//! What "run" actually does once accepted is deliberately thin: it hands the command's name to
//! `run_tx` and replies once that succeeds — *queued*, not *completed*. Waiting for the real
//! result would mean threading a reply channel back through iced's own update loop for a
//! fire-and-forget feature that has no need of one yet; `app::Message::ControlRun` is what
//! actually resolves the name to a message and runs it, on the application's own thread.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// Tighter than the bridge socket's `0660` (`noctmalia_bridge`): there is no second container
/// user to admit here, only us.
const SOCKET_MODE: u32 = 0o600;

/// Answers `status`: what the process is doing right now, as JSON. Handed in by whoever spawns
/// the socket, so this module knows nothing about the bridge it is reporting on.
pub type Status = Arc<dyn Fn() -> Value + Send + Sync>;

/// Answers `call {method, params}` by passing it straight through to Thunderbird — every bridge
/// method, unfiltered — and so handed over only by `noctmalia --dev`. Seeding and smoke are what
/// need it; the socket is `0600`, so the trust boundary is the app's own, but it is still a
/// choice somebody makes at the command line rather than a door that is always open.
pub type Raw = Option<noctmalia_bridge::Bridge>;

/// The last thousand bridge events, numbered, so a script can `wait` for the one after the
/// thing it just did. Kept only with `--dev`, like `call`.
#[derive(Default)]
struct Events {
    seq: u64,
    recent: std::collections::VecDeque<(u64, String, Value)>,
}

const EVENT_LOG: usize = 1000;
type EventLog = Arc<std::sync::Mutex<Events>>;

/// One thing a script may ask to run, by name — what `list` reports and what `run` checks a name
/// against before ever touching `run_tx`.
#[derive(Debug, Clone)]
pub struct Exposed {
    pub name: String,
    pub label: String,
}

/// `$XDG_RUNTIME_DIR/noctmalia/control.sock`, or a temp directory when `XDG_RUNTIME_DIR` is
/// unset — the same fallback `noctmalia_bridge::Bridge`'s own default socket path uses.
pub fn path() -> PathBuf {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    run.join("noctmalia").join("control.sock")
}

#[derive(Deserialize)]
struct Incoming {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct Reply {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

/// Binds the socket and serves it on its own thread with its own runtime, mirroring
/// `noctmalia_bridge::Bridge::spawn`'s own shape — so iced's executor stays free and the caller
/// needs no async context. `registry` is fixed for the life of the process: computed once, from
/// whatever surfaces looked like at startup, before any bridge data has loaded — which is exactly
/// what keeps it to state-independent commands without having to name them twice.
pub fn spawn(
    registry: Vec<Exposed>,
    run_tx: mpsc::UnboundedSender<String>,
    status: Status,
    raw: Raw,
) -> io::Result<PathBuf> {
    let socket_path = path();
    if let Some(dir) = socket_path.parent() {
        fs::create_dir_all(dir)?;
    }
    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(SOCKET_MODE))?;

    let served_path = socket_path.clone();
    std::thread::Builder::new().name("noctmalia-control".into()).spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("control runtime");
        runtime.block_on(serve(listener, registry, run_tx, status, raw));
    })?;
    Ok(served_path)
}

async fn serve(
    listener: std::os::unix::net::UnixListener,
    registry: Vec<Exposed>,
    run_tx: mpsc::UnboundedSender<String>,
    status: Status,
    raw: Raw,
) {
    let listener = UnixListener::from_std(listener).expect("control listener");
    let events: EventLog = Arc::default();
    if let Some(bridge) = &raw {
        tokio::spawn(record(bridge.subscribe(), Arc::clone(&events)));
    }
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        tokio::spawn(handle(
            stream,
            registry.clone(),
            run_tx.clone(),
            Arc::clone(&status),
            raw.clone(),
            Arc::clone(&events),
        ));
    }
}

async fn record(mut subscription: noctmalia_bridge::Events, events: EventLog) {
    while let Some(event) = subscription.next().await {
        let (name, data) = match event {
            noctmalia_bridge::Event::Hello(data) => ("bridge.hello".to_string(), data),
            noctmalia_bridge::Event::Notify { name, data } => (name, data),
            noctmalia_bridge::Event::Lost => ("bridge.lost".to_string(), Value::Null),
            noctmalia_bridge::Event::Lagged(_) => continue,
        };
        let Ok(mut events) = events.lock() else { continue };
        events.seq += 1;
        let seq = events.seq;
        events.recent.push_back((seq, name, data));
        while events.recent.len() > EVENT_LOG {
            events.recent.pop_front();
        }
    }
}

async fn handle(
    stream: UnixStream,
    registry: Vec<Exposed>,
    run_tx: mpsc::UnboundedSender<String>,
    status: Status,
    raw: Raw,
    events: EventLog,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let reply = match serde_json::from_str::<Incoming>(&line) {
            Ok(incoming) if incoming.method == "call" => call(&incoming, raw.as_ref()).await,
            Ok(incoming) if incoming.method == "wait" => wait(&incoming, raw.is_some(), &events).await,
            Ok(incoming) if incoming.method == "status" => {
                let mut result = status();
                if let Ok(events) = events.lock() {
                    result["seq"] = json!(events.seq);
                }
                Reply { id: incoming.id, result: Some(result), error: None }
            }
            Ok(incoming) => respond(&incoming, &registry, &run_tx, &*status),
            // A line that doesn't even parse as `{id, method, ...}` has no id to answer with —
            // drop it rather than guess one, the same way a malformed bridge frame is dropped.
            Err(_) => continue,
        };
        let Ok(mut text) = serde_json::to_string(&reply) else { continue };
        text.push('\n');
        if writer.write_all(text.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// `call`: the bridge method named in `params.method`, with `params.params`, or a refusal when
/// the app was not started with `--dev`.
async fn call(incoming: &Incoming, raw: Option<&noctmalia_bridge::Bridge>) -> Reply {
    let Some(bridge) = raw else {
        return Reply { id: incoming.id, result: None, error: Some("call needs `noctmalia --dev`") };
    };
    let method = incoming.params.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let params = incoming.params.get("params").cloned().unwrap_or(json!({}));
    if method.is_empty() {
        return Reply { id: incoming.id, result: None, error: Some("call needs params.method") };
    }
    match bridge.call_raw(&method, params).await {
        Ok(result) => Reply { id: incoming.id, result: Some(result), error: None },
        // The bridge's own wording, carried whole: a script reads the text, not a code.
        Err(error) => {
            Reply { id: incoming.id, result: Some(json!({ "error": error.to_string() })), error: Some("bridge error") }
        }
    }
}

/// `wait {event, after, timeout}`: the first event named `event` with a sequence number past
/// `after`, or a timeout. Only with `--dev`, since only then is anything recorded.
async fn wait(incoming: &Incoming, dev: bool, events: &EventLog) -> Reply {
    if !dev {
        return Reply { id: incoming.id, result: None, error: Some("wait needs `noctmalia --dev`") };
    }
    let name = incoming.params.get("event").and_then(Value::as_str).unwrap_or_default().to_string();
    let after = incoming.params.get("after").and_then(Value::as_u64).unwrap_or(0);
    let timeout = incoming.params.get("timeout").and_then(Value::as_f64).unwrap_or(60.0);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs_f64(timeout.max(0.0));
    loop {
        if let Ok(events) = events.lock()
            && let Some((seq, found, data)) =
                events.recent.iter().find(|(seq, found, _)| *seq > after && *found == name)
        {
            return Reply {
                id: incoming.id,
                result: Some(json!({ "seq": seq, "event": found, "data": data })),
                error: None,
            };
        }
        if tokio::time::Instant::now() >= deadline {
            return Reply { id: incoming.id, result: None, error: Some("no such event before the timeout") };
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn respond(
    incoming: &Incoming,
    registry: &[Exposed],
    run_tx: &mpsc::UnboundedSender<String>,
    status: &dyn Fn() -> Value,
) -> Reply {
    match incoming.method.as_str() {
        "status" => Reply { id: incoming.id, result: Some(status()), error: None },
        "list" => {
            let items: Vec<Value> =
                registry.iter().map(|item| json!({"name": item.name, "label": item.label})).collect();
            Reply { id: incoming.id, result: Some(Value::Array(items)), error: None }
        }
        "run" => {
            let name = incoming.params.get("name").and_then(Value::as_str).unwrap_or_default();
            if !registry.iter().any(|item| item.name == name) {
                return Reply { id: incoming.id, result: None, error: Some("no such command, or not exposed") };
            }
            match run_tx.send(name.to_string()) {
                Ok(()) => Reply { id: incoming.id, result: Some(json!("queued")), error: None },
                Err(_) => Reply { id: incoming.id, result: None, error: Some("noctmalia is shutting down") },
            }
        }
        _ => Reply { id: incoming.id, result: None, error: Some("unknown method") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<Exposed> {
        vec![Exposed { name: "Refresh".to_string(), label: "Refresh".to_string() }]
    }

    fn status() -> Status {
        Arc::new(|| json!({ "connection": 1 }))
    }

    #[test]
    fn status_reports_whatever_the_spawner_said_it_would() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 9, method: "status".to_string(), params: Value::Null };
        let reply = respond(&incoming, &registry(), &tx, &*status());
        assert_eq!(reply.result, Some(json!({ "connection": 1 })));
    }

    #[test]
    fn list_reports_only_the_registered_names() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 1, method: "list".to_string(), params: Value::Null };
        let reply = respond(&incoming, &registry(), &tx, &*status());
        assert_eq!(reply.id, 1);
        assert_eq!(reply.result, Some(json!([{"name": "Refresh", "label": "Refresh"}])));
        assert!(reply.error.is_none());
    }

    #[test]
    fn running_a_registered_name_queues_it_and_replies_queued() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 2, method: "run".to_string(), params: json!({"name": "Refresh"}) };
        let reply = respond(&incoming, &registry(), &tx, &*status());
        assert_eq!(reply.result, Some(json!("queued")));
        assert_eq!(rx.try_recv(), Ok("Refresh".to_string()));
    }

    #[test]
    fn running_an_unregistered_name_is_an_error_and_queues_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 3, method: "run".to_string(), params: json!({"name": "Delete everything"}) };
        let reply = respond(&incoming, &registry(), &tx, &*status());
        assert!(reply.result.is_none());
        assert!(reply.error.is_some());
        assert!(rx.try_recv().is_err(), "nothing not in the registry ever reaches run_tx");
    }

    #[test]
    fn an_unknown_method_is_an_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 4, method: "delete-everything".to_string(), params: Value::Null };
        let reply = respond(&incoming, &registry(), &tx, &*status());
        assert!(reply.error.is_some());
    }

    #[tokio::test]
    async fn wait_finds_the_event_after_the_sequence_it_was_given() {
        let events: EventLog = Arc::default();
        {
            let mut log = events.lock().unwrap();
            log.seq = 2;
            log.recent.push_back((1, "messages.onUpdated".into(), json!({"a": 1})));
            log.recent.push_back((2, "messages.onUpdated".into(), json!({"a": 2})));
        }
        let incoming = Incoming {
            id: 7,
            method: "wait".into(),
            params: json!({"event": "messages.onUpdated", "after": 1, "timeout": 0.2}),
        };
        let reply = wait(&incoming, true, &events).await;
        assert_eq!(reply.result.unwrap()["data"], json!({"a": 2}));
        let incoming =
            Incoming { id: 8, method: "wait".into(), params: json!({"event": "nothing", "after": 0, "timeout": 0.2}) };
        assert!(wait(&incoming, true, &events).await.error.is_some());
        assert!(wait(&incoming, false, &events).await.error.is_some_and(|e| e.contains("--dev")));
    }

    #[tokio::test]
    async fn call_is_refused_without_dev() {
        let incoming = Incoming { id: 5, method: "call".to_string(), params: json!({"method": "bridge.ping"}) };
        let reply = call(&incoming, None).await;
        assert!(reply.error.is_some_and(|error| error.contains("--dev")));
    }

    /// The real end-to-end path: bind, connect as a plain client would, write a line, read one
    /// back. Everything above this is a unit test of `respond`; this is the one that proves the
    /// framing and the socket permissions actually work.
    #[tokio::test]
    async fn a_client_over_the_real_socket_can_list_and_run() {
        let path = std::env::temp_dir().join(format!("noctmalia-control-test-{}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_MODE)).expect("chmod");
        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, SOCKET_MODE, "the control socket is not group- or world-accessible");

        let (run_tx, mut run_rx) = mpsc::unbounded_channel();
        tokio::spawn(serve(listener, registry(), run_tx, status(), None));

        let client = UnixStream::connect(&path).await.expect("connect");
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();

        writer.write_all(b"{\"id\":1,\"method\":\"list\"}\n").await.expect("write");
        let line = lines.next_line().await.expect("read").expect("a line");
        let reply: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(reply["result"], json!([{"name": "Refresh", "label": "Refresh"}]));

        writer.write_all(b"{\"id\":2,\"method\":\"run\",\"params\":{\"name\":\"Refresh\"}}\n").await.expect("write");
        let line = lines.next_line().await.expect("read").expect("a line");
        let reply: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(reply["result"], json!("queued"));
        assert_eq!(run_rx.recv().await, Some("Refresh".to_string()));

        let _ = fs::remove_file(&path);
    }
}
