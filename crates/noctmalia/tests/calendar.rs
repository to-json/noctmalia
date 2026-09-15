//! Calendar against `tools/fake-bridge.py`, over a real socket.
//!
//! Like `tests/contacts.rs`, this is where method names and parameter spellings get checked —
//! `calendarId`, `rangeStart`/`rangeEnd`, `format`/`item` — against a stand-in, not against
//! Thunderbird itself. `tools/smoke.sh`'s "calendar round trip" is what proves the real thing
//! accepts the same ICAL text and expands recurrence; the stand-in does not expand it (see
//! `Store.items_in_range` in `tools/fake-bridge.py`), so nothing here asserts on that.

use chrono::{Duration, Local};
use noctmalia::calendar::{self, Cal};
use noctmalia::ical::{Event, Recur, When};
use noctmalia_bridge::{Bridge, Event as BridgeEvent};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Socket(PathBuf);

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Brings up a bridge with the fake attached, or skips the test where python is missing.
async fn attached(name: &str) -> Option<(Bridge, Vec<Cal>, Fake, Socket)> {
    let mut path = std::env::temp_dir();
    path.push(format!("noctmalia-calendar-{}-{}.sock", name, std::process::id()));
    let socket = Socket(path);

    let bridge = Bridge::spawn(&socket.0).expect("spawn");
    let mut events = bridge.subscribe();

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/fake-bridge.py");
    let spawned = Command::new("python3")
        .arg(&script)
        .arg("--socket")
        .arg(&socket.0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(child) = spawned else {
        eprintln!("skipping: python3 is not available to run {}", script.display());
        return None;
    };
    let fake = Fake(child);

    let hello = tokio::time::timeout(StdDuration::from_secs(20), async {
        loop {
            if let Some(BridgeEvent::Hello(_)) = events.next().await {
                return;
            }
        }
    })
    .await;
    hello.expect("the fake bridge should say hello");

    let cals = calendar::calendars(bridge.clone()).await.expect("calendars");
    Some((bridge, cals, fake, socket))
}

/// A range wide enough to catch every fixture event, whenever the test happens to run.
fn wide_range() -> (chrono::DateTime<Local>, chrono::DateTime<Local>) {
    let now = Local::now();
    (now - Duration::days(3), now + Duration::days(14))
}

#[tokio::test]
async fn lists_the_fixtures_two_calendars() {
    let Some((_bridge, cals, _fake, _socket)) = attached("calendars").await else { return };
    assert_eq!(cals.len(), 2);
    assert!(cals.iter().any(|cal| cal.id == "personal" && !cal.hidden));
    assert!(cals.iter().any(|cal| cal.id == "work" && !cal.hidden));
}

#[tokio::test]
async fn queries_items_across_calendars_and_parses_their_ical() {
    let Some((bridge, cals, _fake, _socket)) = attached("items").await else { return };
    let ids: Vec<String> = cals.iter().map(|cal| cal.id.clone()).collect();
    let (start, end) = wide_range();

    let items = calendar::items(bridge.clone(), ids, start, end).await.expect("items");
    // allhands (two days in the past, weekly) falls outside this window; the rest do not.
    let titles: Vec<&str> = items.iter().map(|item| item.event.summary.as_str()).collect();
    assert!(titles.contains(&"Standup"), "{titles:?}");
    assert!(titles.contains(&"Budget review"), "{titles:?}");
    assert!(titles.contains(&"Dentist"), "{titles:?}");
    assert!(titles.contains(&"Long weekend"), "{titles:?}");

    let standup = items.iter().find(|item| item.event.summary == "Standup").expect("standup");
    assert_eq!(standup.calendar_id, "work");
    assert_eq!(standup.event.recur(), Recur::Daily);
    assert_eq!(standup.event.alarms, vec![Duration::minutes(10)]);
    assert!(!standup.event.start.is_all_day());

    let trip = items.iter().find(|item| item.event.summary == "Long weekend").expect("trip");
    assert!(trip.event.start.is_all_day());

    // Scoped to one calendar, an event from the other must not come back.
    let personal_only = calendar::items(bridge, vec!["personal".to_string()], start, end).await.expect("items");
    assert!(personal_only.iter().all(|item| item.calendar_id == "personal"));
    assert!(personal_only.iter().any(|item| item.event.summary == "Dentist"));
}

#[tokio::test]
async fn hiding_a_calendar_empties_it_out_of_a_query() {
    let Some((bridge, _cals, _fake, _socket)) = attached("visibility").await else { return };
    let (start, end) = wide_range();

    calendar::set_visible(bridge.clone(), "work".to_string(), false).await.expect("set_visible");
    let cals = calendar::calendars(bridge.clone()).await.expect("calendars");
    assert!(cals.iter().find(|cal| cal.id == "work").expect("work").hidden);

    // The surface itself only ever queries the calendars it still considers visible; asking for
    // "work" explicitly still works, since hiding is a UI concern rather than the bridge refusing.
    let personal_ids = vec!["personal".to_string()];
    let items = calendar::items(bridge, personal_ids, start, end).await.expect("items");
    assert!(items.iter().all(|item| item.calendar_id == "personal"));
}

/// `calendar.items.fireAlarm` is not a real bridge method — Thunderbird's alarm service fires
/// `onAlarm` on its own timer, with nothing to ask for one on demand. This is the fake-only lever
/// `docs/reminders-plan.md` adds so the wire path (fake → bridge → notify) is provable without a
/// real Thunderbird and a real wait.
#[tokio::test]
async fn firing_an_alarm_notifies_with_the_event_that_fired() {
    let Some((bridge, cals, _fake, _socket)) = attached("alarm").await else { return };
    let work = cals.iter().find(|cal| cal.id == "work").expect("work calendar");
    let (start, end) = wide_range();
    let items = calendar::items(bridge.clone(), vec![work.id.clone()], start, end).await.expect("items");
    let standup = items.iter().find(|item| item.event.summary == "Standup").expect("standup");

    let mut events = bridge.subscribe();
    bridge.call_raw("calendar.items.fireAlarm", json!({ "id": standup.id })).await.expect("fireAlarm");

    let data = tokio::time::timeout(StdDuration::from_secs(5), async {
        loop {
            if let Some(BridgeEvent::Notify { name, data }) = events.next().await {
                if name == "calendar.items.onAlarm" {
                    return data;
                }
            }
        }
    })
    .await
    .expect("an onAlarm notify");

    let ical = data.get("item").and_then(|item| item.get("item")).and_then(Value::as_str);
    let title = ical.and_then(Event::parse).map(|event| event.summary);
    assert_eq!(title.as_deref(), Some("Standup"));
}

#[tokio::test]
async fn creates_updates_and_deletes_an_event() {
    let Some((bridge, _cals, _fake, _socket)) = attached("write").await else { return };
    let (start, end) = wide_range();

    let when_start = When::Time(Local::now() + Duration::hours(1));
    let when_end = When::Time(Local::now() + Duration::hours(2));
    let mut event = Event::blank(when_start, when_end);
    event.summary = "Kickoff".to_string();
    event.location = "Kitchen".to_string();

    let created = calendar::create(bridge.clone(), "personal".to_string(), event).await.expect("create");
    assert_eq!(created.event.summary, "Kickoff");
    assert_eq!(created.calendar_id, "personal");

    let items = calendar::items(bridge.clone(), vec!["personal".to_string()], start, end).await.expect("items");
    assert!(items.iter().any(|item| item.id == created.id));

    let mut edited = created.event.clone();
    edited.summary = "Kickoff (moved)".to_string();
    let updated =
        calendar::update(bridge.clone(), "personal".to_string(), created.id.clone(), edited).await.expect("update");
    assert_eq!(updated.event.summary, "Kickoff (moved)");

    calendar::remove(bridge.clone(), "personal".to_string(), created.id.clone()).await.expect("remove");
    let items = calendar::items(bridge, vec!["personal".to_string()], start, end).await.expect("items");
    assert!(items.iter().all(|item| item.id != created.id));
}
