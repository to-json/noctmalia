//! The bridge from the other end: a fake shim connects, and we check what crosses the socket.

use noctmalia_bridge::{Bridge, Error, Event, MAX_REQUEST_BYTES, REPLACE_AFTER};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// A socket path that no other test shares, removed when the guard drops.
struct Socket(PathBuf);

impl Socket {
    fn new(name: &str) -> Socket {
        let mut path = std::env::temp_dir();
        path.push(format!("noctmalia-{}-{}.sock", name, std::process::id()));
        Socket(path)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stands in for nm-shim: connects to noctmalia and speaks line JSON.
struct Peer {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

impl Peer {
    async fn connect(bridge: &Bridge) -> Peer {
        let stream = UnixStream::connect(bridge.path()).await.expect("connect");
        let (reader, writer) = stream.into_split();
        Peer { lines: BufReader::new(reader).lines(), writer }
    }

    async fn request(&mut self) -> Value {
        let line = self.lines.next_line().await.expect("read").expect("a request");
        serde_json::from_str(&line).expect("json")
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.writer.write_all(line.as_bytes()).await.expect("write");
    }
}

/// Waits for the bridge to notice the peer, so a test's first call cannot race the accept.
async fn attach(bridge: &Bridge) -> (Peer, noctmalia_bridge::Events) {
    let mut events = bridge.subscribe();
    let mut peer = Peer::connect(bridge).await;
    peer.send(json!({ "event": "bridge.hello", "data": { "protocol": 1 } })).await;
    // A `Lost` from a previous connection can still be on its way; a real client skips past it too.
    loop {
        match events.next().await {
            Some(Event::Hello(data)) => {
                assert_eq!(data["protocol"], 1);
                break;
            }
            Some(Event::Lost) => continue,
            other => panic!("expected hello, got {other:?}"),
        }
    }
    (peer, events)
}

#[tokio::test]
async fn calls_fail_fast_while_nothing_is_attached() {
    let socket = Socket::new("unattached");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    // Hanging here instead would leave the UI on a spinner with nothing to wait for.
    let result = bridge.call_raw("bridge.ping", json!({})).await;
    assert!(matches!(result, Err(Error::NotConnected)), "{result:?}");
}

#[tokio::test]
async fn a_call_reaches_the_peer_and_its_result_comes_back() {
    let socket = Socket::new("roundtrip");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;

    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("addressBooks.list", json!({ "complete": false })).await }
    });

    let request = peer.request().await;
    assert_eq!(request["method"], "addressBooks.list");
    assert_eq!(request["params"]["complete"], false);
    let id = request["id"].clone();
    peer.send(json!({ "id": id, "result": [{ "id": "personal" }] })).await;

    let result = calling.await.expect("join").expect("result");
    assert_eq!(result[0]["id"], "personal");
}

#[tokio::test]
async fn replies_are_matched_by_id_not_by_arrival_order() {
    let socket = Socket::new("outoforder");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;

    let first = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("slow", json!({})).await }
    });
    let first_request = peer.request().await;
    let second = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("fast", json!({})).await }
    });
    let second_request = peer.request().await;

    // Answer the second one first, as a real Thunderbird would for a cheaper call.
    peer.send(json!({ "id": second_request["id"], "result": "second" })).await;
    peer.send(json!({ "id": first_request["id"], "result": "first" })).await;

    assert_eq!(second.await.expect("join").expect("result"), "second");
    assert_eq!(first.await.expect("join").expect("result"), "first");
}

#[tokio::test]
async fn an_error_reply_keeps_its_name_and_message() {
    let socket = Socket::new("error");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;

    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("contacts.get", json!({ "contactId": "nope" })).await }
    });
    let request = peer.request().await;
    let error = json!({ "name": "ExtensionError", "message": "contact not found" });
    peer.send(json!({ "id": request["id"], "error": error })).await;

    match calling.await.expect("join") {
        Err(Error::Remote(error)) => {
            assert_eq!(error.name, "ExtensionError");
            assert_eq!(error.message, "contact not found");
        }
        other => panic!("expected a remote error, got {other:?}"),
    }
}

#[tokio::test]
async fn events_reach_subscribers_and_hello_is_its_own_variant() {
    let socket = Socket::new("events");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, mut events) = attach(&bridge).await;

    peer.send(json!({ "event": "contacts.onUpdated", "data": { "contact": { "id": "c1" } } })).await;
    match events.next().await {
        Some(Event::Notify { name, data }) => {
            assert_eq!(name, "contacts.onUpdated");
            assert_eq!(data["contact"]["id"], "c1");
        }
        other => panic!("expected a notify, got {other:?}"),
    }
}

#[tokio::test]
async fn a_dropped_connection_fails_calls_in_flight_and_announces_itself() {
    let socket = Socket::new("dropped");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, mut events) = attach(&bridge).await;

    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("messages.list", json!({})).await }
    });
    peer.request().await;
    drop(peer); // Thunderbird restarting takes the shim with it.

    let result = calling.await.expect("join");
    assert!(matches!(result, Err(Error::NotConnected)), "{result:?}");
    assert!(matches!(events.next().await, Some(Event::Lost)));
}

#[tokio::test]
async fn the_socket_is_reusable_after_thunderbird_restarts() {
    let socket = Socket::new("reconnect");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (peer, _events) = attach(&bridge).await;
    drop(peer);

    // The shim reconnects forever, so a second hello has to be served on the same listener.
    let (mut peer, _events) = attach(&bridge).await;
    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("bridge.ping", json!({})).await }
    });
    let request = peer.request().await;
    peer.send(json!({ "id": request["id"], "result": { "pong": 1 } })).await;
    assert_eq!(calling.await.expect("join").expect("result")["pong"], 1);
}

#[tokio::test]
async fn an_oversized_request_is_refused_here_rather_than_by_the_shim() {
    let socket = Socket::new("toolarge");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;

    let huge = "x".repeat(MAX_REQUEST_BYTES + 1);
    let result = bridge.call_raw("messages.import", json!({ "base64": huge })).await;
    match result {
        Err(Error::TooLarge { bytes, method }) => {
            assert!(bytes > MAX_REQUEST_BYTES);
            assert_eq!(method, "messages.import");
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }

    // The connection survives it: the next call still works.
    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("bridge.ping", json!({})).await }
    });
    let request = peer.request().await;
    assert_eq!(request["method"], "bridge.ping");
    peer.send(json!({ "id": request["id"], "result": { "pong": 2 } })).await;
    assert_eq!(calling.await.expect("join").expect("result")["pong"], 2);
}

#[tokio::test]
async fn a_malformed_line_is_ignored_rather_than_killing_the_session() {
    let socket = Socket::new("garbage");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;

    peer.send(json!("not an object")).await;
    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("bridge.ping", json!({})).await }
    });
    let request = peer.request().await;
    peer.send(json!({ "id": request["id"], "result": { "pong": 3 } })).await;
    assert_eq!(calling.await.expect("join").expect("result")["pong"], 3);
}

#[tokio::test]
async fn a_call_nobody_answers_times_out_here_and_the_connection_survives() {
    let socket = Socket::new("timeout");
    let bridge = Bridge::spawn_with_timeout(&socket.0, std::time::Duration::from_millis(200)).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;

    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("gloda.search", json!({})).await }
    });
    // The request crosses; nobody answers it.
    assert_eq!(peer.request().await["method"], "gloda.search");
    match calling.await.expect("join") {
        Err(Error::TimedOut { method, after }) => {
            assert_eq!(method, "gloda.search");
            assert!(after >= std::time::Duration::from_millis(200));
        }
        other => panic!("expected TimedOut, got {other:?}"),
    }
    let stats = bridge.stats();
    assert_eq!((stats.timed_out, stats.failed, stats.in_flight), (1, 1, 0));

    // The peer is still attached, and the next call still works.
    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("bridge.ping", json!({})).await }
    });
    let request = peer.request().await;
    peer.send(json!({ "id": request["id"], "result": { "pong": 4 } })).await;
    assert_eq!(calling.await.expect("join").expect("result")["pong"], 4);
}

#[tokio::test]
async fn a_newer_connection_replaces_the_one_being_served() {
    let socket = Socket::new("replace");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut old, mut events) = attach(&bridge).await;
    assert_eq!(bridge.stats().connection, Some(1));

    // A second shim connects while the first is still up: the first is dropped, not left to serve
    // the socket while the second sits in the backlog saying "connected". Not at once, though —
    // a newcomer inside the first seconds is refused, so two shims cannot swap forever.
    let early = Peer::connect(&bridge).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(bridge.stats().connection, Some(1), "a newcomer that soon is refused");
    drop(early);
    tokio::time::sleep(REPLACE_AFTER).await;
    let mut new = Peer::connect(&bridge).await;
    new.send(json!({ "event": "bridge.hello", "data": { "protocol": 1 } })).await;
    assert!(matches!(events.next().await, Some(Event::Lost)), "the old connection is announced as lost");
    assert!(matches!(events.next().await, Some(Event::Hello(_))), "the new one says hello");
    assert_eq!(bridge.stats().connection, Some(2));

    let calling = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.call_raw("bridge.ping", json!({})).await }
    });
    let request = new.request().await;
    assert_eq!(request["method"], "bridge.ping");
    new.send(json!({ "id": request["id"], "result": { "pong": 5 } })).await;
    assert_eq!(calling.await.expect("join").expect("result")["pong"], 5);
    // The old peer's end is closed.
    assert!(old.lines.next_line().await.ok().flatten().is_none());
}

#[tokio::test]
async fn stats_count_calls_and_remember_the_slowest() {
    let socket = Socket::new("stats");
    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let (mut peer, _events) = attach(&bridge).await;
    assert!(bridge.stats().attached_for.is_some());

    for pong in 0..2 {
        let calling = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.call_raw("bridge.ping", json!({})).await }
        });
        let request = peer.request().await;
        assert_eq!(bridge.stats().in_flight, 1);
        peer.send(json!({ "id": request["id"], "result": { "pong": pong } })).await;
        calling.await.expect("join").expect("result");
    }
    let stats = bridge.stats();
    assert_eq!((stats.calls, stats.failed, stats.in_flight, stats.connections), (2, 0, 0, 1));
    assert_eq!(stats.last.as_ref().map(|sample| sample.method.as_str()), Some("bridge.ping"));
    assert!(stats.slowest.is_some());

    drop(peer);
    let _ = _events;
    // Give the actor a moment to notice the close.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let stats = bridge.stats();
    assert_eq!(stats.connection, None);
    assert!(stats.attached_for.is_none());
}
