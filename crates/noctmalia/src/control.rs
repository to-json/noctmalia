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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// Tighter than the bridge socket's `0660` (`noctmalia_bridge`): there is no second container
/// user to admit here, only us.
const SOCKET_MODE: u32 = 0o600;

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
pub fn spawn(registry: Vec<Exposed>, run_tx: mpsc::UnboundedSender<String>) -> io::Result<PathBuf> {
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
        runtime.block_on(serve(listener, registry, run_tx));
    })?;
    Ok(served_path)
}

async fn serve(listener: std::os::unix::net::UnixListener, registry: Vec<Exposed>, run_tx: mpsc::UnboundedSender<String>) {
    let listener = UnixListener::from_std(listener).expect("control listener");
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        tokio::spawn(handle(stream, registry.clone(), run_tx.clone()));
    }
}

async fn handle(stream: UnixStream, registry: Vec<Exposed>, run_tx: mpsc::UnboundedSender<String>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let reply = match serde_json::from_str::<Incoming>(&line) {
            Ok(incoming) => respond(&incoming, &registry, &run_tx),
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

fn respond(incoming: &Incoming, registry: &[Exposed], run_tx: &mpsc::UnboundedSender<String>) -> Reply {
    match incoming.method.as_str() {
        "list" => {
            let items: Vec<Value> = registry.iter().map(|item| json!({"name": item.name, "label": item.label})).collect();
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

    #[test]
    fn list_reports_only_the_registered_names() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 1, method: "list".to_string(), params: Value::Null };
        let reply = respond(&incoming, &registry(), &tx);
        assert_eq!(reply.id, 1);
        assert_eq!(reply.result, Some(json!([{"name": "Refresh", "label": "Refresh"}])));
        assert!(reply.error.is_none());
    }

    #[test]
    fn running_a_registered_name_queues_it_and_replies_queued() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 2, method: "run".to_string(), params: json!({"name": "Refresh"}) };
        let reply = respond(&incoming, &registry(), &tx);
        assert_eq!(reply.result, Some(json!("queued")));
        assert_eq!(rx.try_recv(), Ok("Refresh".to_string()));
    }

    #[test]
    fn running_an_unregistered_name_is_an_error_and_queues_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 3, method: "run".to_string(), params: json!({"name": "Delete everything"}) };
        let reply = respond(&incoming, &registry(), &tx);
        assert!(reply.result.is_none());
        assert!(reply.error.is_some());
        assert!(rx.try_recv().is_err(), "nothing not in the registry ever reaches run_tx");
    }

    #[test]
    fn an_unknown_method_is_an_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let incoming = Incoming { id: 4, method: "delete-everything".to_string(), params: Value::Null };
        let reply = respond(&incoming, &registry(), &tx);
        assert!(reply.error.is_some());
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
        tokio::spawn(serve(listener, registry(), run_tx));

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
