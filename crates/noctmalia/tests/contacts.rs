//! Contacts against `tools/fake-bridge.py`, over a real socket.
//!
//! This is where the method names and parameter spellings get checked: `parentId` and `contactId`
//! and `vCard` are what the bridge expects, and a typo in any of them only shows up as a
//! MethodNotFound or a silently empty list. Thunderbird itself is not involved — see `tools/smoke.sh`
//! for that — but everything between the UI and the wire is.

use noctmalia::contacts::{self, AddressBook};
use noctmalia::vcard::{self, Entry};
use noctmalia_bridge::{Bridge, Event};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The fake bridge, killed when the test ends however it ends.
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
async fn attached(name: &str) -> Option<(Bridge, Vec<AddressBook>, Fake, Socket)> {
    let mut path = std::env::temp_dir();
    path.push(format!("noctmalia-contacts-{}-{}.sock", name, std::process::id()));
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

    let hello = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(Event::Hello(_)) = events.next().await {
                return;
            }
        }
    })
    .await;
    hello.expect("the fake bridge should say hello");

    let books = contacts::books(bridge.clone()).await.expect("books");
    Some((bridge, books, fake, socket))
}

#[tokio::test]
async fn lists_books_and_every_contact_in_them() {
    let Some((bridge, books, _fake, _socket)) = attached("list").await else { return };

    assert_eq!(books.len(), 3);
    assert_eq!(books[0].id, "personal");
    // The rail marks these; getting the flags wrong would offer writes into a read-only book.
    assert!(!books[0].read_only && !books[0].remote);
    assert!(books[1].remote, "the CardDAV book is remote");
    assert!(books[2].read_only, "collected addresses are read-only");

    let all = contacts::list(bridge.clone(), None, books.clone()).await.expect("list");
    assert_eq!(all.len(), 7);
    // Sorted by family name, so the list reads like a rolodex rather than like insertion order.
    let names: Vec<String> = all.iter().map(|contact| contact.card.display_name()).collect();
    assert_eq!(names[0], "Bob Builder");
    assert_eq!(names[1], "Alice Chen");

    let one_book = contacts::list(bridge, Some("work".to_string()), books).await.expect("list");
    assert_eq!(one_book.len(), 2);
    assert!(one_book.iter().all(|contact| contact.book.as_deref() == Some("work")));
}

#[tokio::test]
async fn parses_the_vcards_it_gets_back() {
    let Some((bridge, books, _fake, _socket)) = attached("parse").await else { return };

    let all = contacts::list(bridge, None, books).await.expect("list");
    let alice = all.iter().find(|contact| contact.card.display_name() == "Alice Chen").expect("alice");
    let card = &alice.card;
    assert_eq!(card.name.family, "Chen");
    assert_eq!(card.organisation, "Noctalia");
    assert_eq!(card.role, "Compositor wrangler");
    assert_eq!(card.emails.len(), 2);
    assert_eq!(card.emails[0].value, "alice@noctalia.dev");
    assert_eq!(card.emails[0].kind, "work");
    assert_eq!(card.phones[0].value, "+1 555 0100");
    assert_eq!(card.initials(), "AC");
}

#[tokio::test]
async fn search_goes_through_thunderbird_rather_than_filtering_locally() {
    let Some((bridge, _books, _fake, _socket)) = attached("search").await else { return };

    let hits = contacts::search(bridge.clone(), "okoro".to_string(), None).await.expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].card.display_name(), "Dana Okoro");

    // Scoped to one book, a match in another book must not come back.
    let scoped = contacts::search(bridge, "okoro".to_string(), Some("work".to_string())).await.expect("search");
    assert!(scoped.is_empty());
}

#[tokio::test]
async fn creates_updates_and_deletes() {
    let Some((bridge, books, _fake, _socket)) = attached("write").await else { return };

    let mut card = vcard::blank();
    card.name.given = "Nadia".to_string();
    card.name.family = "Okonkwo".to_string();
    card.emails = vec![Entry { kind: "work".to_string(), value: "nadia@example.com".to_string(), preferred: true }];
    card.phones.clear();
    card.organisation = "Noctalia".to_string();

    let id = contacts::create(bridge.clone(), "personal".to_string(), card.to_vcard()).await.expect("create");
    let all = contacts::list(bridge.clone(), None, books.clone()).await.expect("list");
    assert_eq!(all.len(), 8);
    let made = all.iter().find(|contact| contact.id == id).expect("the new contact");
    // FN is derived from the name components, since the editor never asks for it directly.
    assert_eq!(made.card.display_name(), "Nadia Okonkwo");
    assert_eq!(made.card.emails[0].value, "nadia@example.com");
    assert_eq!(made.card.organisation, "Noctalia");

    let mut edited = made.card.clone();
    edited.role = "Maintainer".to_string();
    edited.emails[0].value = "nadia@noctalia.dev".to_string();
    contacts::update(bridge.clone(), id.clone(), edited.to_vcard()).await.expect("update");

    let all = contacts::list(bridge.clone(), None, books.clone()).await.expect("list");
    let changed = all.iter().find(|contact| contact.id == id).expect("still there");
    assert_eq!(changed.card.role, "Maintainer");
    assert_eq!(changed.card.emails[0].value, "nadia@noctalia.dev");

    contacts::delete(bridge.clone(), id.clone()).await.expect("delete");
    let all = contacts::list(bridge, None, books).await.expect("list");
    assert_eq!(all.len(), 7);
    assert!(all.iter().all(|contact| contact.id != id));
}

#[tokio::test]
async fn an_edit_keeps_the_properties_thunderbird_owns() {
    let Some((bridge, books, _fake, _socket)) = attached("preserve").await else { return };

    let all = contacts::list(bridge.clone(), None, books.clone()).await.expect("list");
    let alice = all.iter().find(|contact| contact.card.display_name() == "Alice Chen").expect("alice");

    let mut edited = alice.card.clone();
    edited.note = "Edited by noctmalia".to_string();
    contacts::update(bridge.clone(), alice.id.clone(), edited.to_vcard()).await.expect("update");

    let all = contacts::list(bridge, None, books).await.expect("list");
    let saved = all.iter().find(|contact| contact.id == alice.id).expect("alice");
    assert_eq!(saved.card.note, "Edited by noctmalia");
    // UID and the X- properties are Thunderbird's; losing them on save would orphan the contact.
    let kept: Vec<&str> = saved.card.rest.iter().map(|property| property.name.as_str()).collect();
    assert!(kept.contains(&"UID"), "UID was dropped: {kept:?}");
    assert!(kept.contains(&"X-THUNDERBIRD-KEEP"), "an X- property was dropped: {kept:?}");
}

#[tokio::test]
async fn an_unimplemented_method_surfaces_as_an_error_not_a_hang() {
    let Some((bridge, _books, _fake, _socket)) = attached("unknown").await else { return };

    let result = bridge.call_raw("messages.list", serde_json::json!({ "folderId": "x" })).await;
    let error = result.expect_err("the fake bridge serves contacts only");
    assert!(error.to_string().contains("MethodNotFound"), "{error}");
}
