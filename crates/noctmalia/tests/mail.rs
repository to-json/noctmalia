//! The mail surface, laid out headlessly.
//!
//! These are not unit tests of the pieces — those live beside them. These build the real widget
//! tree the window would draw and look at what came out, which is the only place two claims can
//! actually be checked:
//!
//! - **Nothing remote reaches the screen.** `mime::html` is tested on its own, but the thing worth
//!   proving is about the *rendered interface*: open a newsletter full of beacons and there is no
//!   tracker URL anywhere in the widget tree.
//! - **The index is windowed.** A folder with five thousand messages in it has to build a
//!   screenful, not five thousand rows, and the only way to know is to count the rows that exist.

use iced::Theme;
use noctmalia::mail;
use noctmalia::mime;
use noctmalia::shell::Shell;
use noctmalia::surfaces::mail::{Mail, Message, Region, Showing};
use noctmalia_bridge::Bridge;
use serde_json::json;
use std::time::Instant;

/// A bridge bound to a socket nothing will ever connect to. The surface only needs one to clone.
///
/// One path per call: the test harness runs these in parallel, and two of them binding the same
/// socket would have one quietly unlinking the other's.
fn bridge() -> Bridge {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let which = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("noctmalia-test-{}-{which}.sock", std::process::id()));
    Bridge::spawn(path).expect("a socket in the temp directory")
}

fn ready() -> (Mail, Shell) {
    let mut shell = Shell::new(bridge(), 1180.0);
    shell.set_connected(true);
    (Mail::new(), shell)
}

fn accounts() -> serde_json::Value {
    json!([{
        "id": "account1", "name": "greenmail", "type": "imap",
        "folders": [{
            "id": "account1://", "name": "", "path": "/", "accountId": "account1",
            "subFolders": [
                {"id": "account1://inbox", "name": "Inbox", "path": "/inbox",
                 "accountId": "account1", "specialUse": ["inbox"], "subFolders": []},
                {"id": "account1://archives", "name": "Archives", "path": "/archives",
                 "accountId": "account1", "specialUse": ["archives"], "subFolders": []},
            ],
        }],
    }])
}

fn header(id: u64, subject: &str, author: &str, read: bool) -> serde_json::Value {
    json!({
        "id": id,
        "headerMessageId": format!("m{id}@example"),
        "subject": subject,
        "author": author,
        "recipients": ["j@noctmalia.test"],
        "date": format!("2026-09-{:02}T09:00:00Z", (id % 28) + 1),
        "read": read,
        "folder": {"id": "account1://inbox", "name": "Inbox"},
    })
}

/// Drives the surface to a loaded inbox.
fn loaded(mail: &mut Mail, shell: &mut Shell, messages: Vec<serde_json::Value>) {
    let now = Instant::now();
    let accounts = serde_json::from_value(accounts()).expect("accounts");
    let _ = mail.update(Message::Accounts(Ok(accounts)), shell, now);
    let page = serde_json::from_value(json!({ "id": null, "messages": messages })).expect("a page");
    // The generation is 1: opening the inbox started the first listing.
    let _ = mail.update(Message::Page(1, Ok(page)), shell, now);
}

/// The one claim the whole renderer exists to make, checked against the widget tree.
#[test]
fn a_newsletter_full_of_beacons_puts_nothing_remote_on_the_screen() {
    let (mut mail, mut shell) = ready();
    loaded(
        &mut mail,
        &mut shell,
        vec![header(1, "Half price, this week only", "Shop <deals@shopfront.example>", false)],
    );

    let hostile = json!({
        "contentType": "multipart/alternative",
        "partName": "",
        "headers": {
            "from": ["Shop <deals@shopfront.example>"],
            "subject": ["Half price, this week only"],
            "list-unsubscribe": ["<https://shopfront.example/u>, <mailto:unsub@shopfront.example>"],
            "authentication-results": ["mx.noctmalia.test; dkim=pass; dmarc=pass"],
        },
        "parts": [
            {"contentType": "text/plain", "partName": "1", "body": "This email requires HTML."},
            {"contentType": "text/html", "partName": "2", "body": concat!(
                "<script>fetch('https://track.shopfront.example/open')</script>",
                "<style>body{background:url(https://track.shopfront.example/bg)}</style>",
                "<img src=\"https://track.shopfront.example/pixel.gif\" width=\"1\" height=\"1\" alt=\"\">",
                "<h1>Half price</h1><p>Everything is <b>half price</b> until Sunday.</p>",
                "<a href=\"https://shopfront.example.evil.ru/claim\">shopfront.example</a>",
            )},
        ],
    });
    let part: mime::Part = serde_json::from_value(hostile).expect("a message");
    let letter = mail::read(1, &part);
    // Everything the sender wanted fetched is known about, and none of it is fetched.
    assert_eq!(letter.body.images, ["https://track.shopfront.example/pixel.gif"]);
    assert_eq!(letter.body.trackers, 1);
    assert!(letter.findings.iter().any(|finding| finding.headline == "Link goes elsewhere"));

    let now = Instant::now();
    let _ = mail.update(Message::Select(1), &mut shell, now);
    let _ = mail.update(Message::Letter(1, Ok(letter)), &mut shell, now);

    let drawn = render(&mail, &shell);
    assert!(drawn.iter().any(|line| line.contains("Half price")), "the letter is on screen: {drawn:#?}");
    for forbidden in ["track.shopfront.example", "<script", "fetch(", "url(https"] {
        assert!(!drawn.iter().any(|line| line.contains(forbidden)), "{forbidden} reached the screen:\n{drawn:#?}");
    }
}

/// The security surface says what the message gives away, in words rather than in a shield glyph.
#[test]
fn a_message_that_lies_about_itself_says_so_on_the_security_surface() {
    let (mut mail, mut shell) = ready();
    loaded(
        &mut mail,
        &mut shell,
        vec![header(
            1,
            "Urgent: verify your account",
            "\"security@your-bank.example\" <collect@mail.example.ru>",
            false,
        )],
    );
    let phish: mime::Part = serde_json::from_value(json!({
        "contentType": "text/plain",
        "partName": "1",
        "body": "Your account is locked.",
        "headers": {
            "from": ["\"security@your-bank.example\" <collect@mail.example.ru>"],
            "reply-to": ["helpdesk@free-mail.example"],
            "authentication-results": ["mx.noctmalia.test; spf=fail; dkim=none; dmarc=fail"],
        },
    }))
    .expect("a message");

    let now = Instant::now();
    let _ = mail.update(Message::Select(1), &mut shell, now);
    let _ = mail.update(Message::Letter(1, Ok(mail::read(1, &phish))), &mut shell, now);
    let _ = mail.update(Message::Show(Showing::Security), &mut shell, now);

    let drawn = render(&mail, &shell).join("\n");
    assert!(drawn.contains("collect@mail.example.ru"), "the real address is named:\n{drawn}");
    assert!(drawn.contains("Name is another address"), "the headline is on screen:\n{drawn}");
    assert!(drawn.contains("DMARC failed"), "each check is named:\n{drawn}");
    assert!(drawn.contains("mx.noctmalia.test reports DMARC fail"), "the mailhost's claim is reported:\n{drawn}");
    // Attributed to whoever made it, never asserted as ours.
    assert!(drawn.contains("mx.noctmalia.test reports"), "{drawn}");
}

/// The reason [`noctalia_iced::list`] exists.
#[test]
fn a_folder_with_five_thousand_messages_builds_a_screenful() {
    let (mut mail, mut shell) = ready();
    let many: Vec<serde_json::Value> =
        (1..=5_000).map(|id| header(id, &format!("message {id}"), "someone@example.com", id % 3 == 0)).collect();
    loaded(&mut mail, &mut shell, many);

    let drawn = render(&mail, &shell);
    let rows = drawn.iter().filter(|line| line.starts_with("message ")).count();
    assert!(rows > 0, "the index drew something");
    assert!(rows < 60, "the index built {rows} rows for a 768px window");
}

/// The keymap is the interface. `j` has to move the selection with no pointer anywhere near it.
#[test]
fn the_index_is_driven_from_the_keyboard() {
    use iced::keyboard::{Key, Modifiers};
    use noctmalia::surfaces::Pressed;

    let (mut mail, mut shell) = ready();
    loaded(
        &mut mail,
        &mut shell,
        vec![header(1, "first", "a@example.com", false), header(2, "second", "b@example.com", false)],
    );

    let press = |mail: &mut Mail, shell: &mut Shell, key: &str| {
        let pressed = mail.press(&Key::Character(key.into()), Modifiers::empty());
        if let Pressed::Act(message) = pressed {
            let _ = mail.update(message, shell, Instant::now());
            return true;
        }
        matches!(pressed, Pressed::Pending)
    };

    assert!(press(&mut mail, &mut shell, "j"), "j is bound");
    let first = render(&mail, &shell);
    assert!(first.iter().any(|line| line == "first" || line == "second"));

    // `g` alone does nothing and waits; `gg` goes to the top.
    assert!(press(&mut mail, &mut shell, "g"));
    assert!(press(&mut mail, &mut shell, "g"));

    // `l` walks into the pager and `h` walks back out, which is the whole of the modality.
    assert!(press(&mut mail, &mut shell, "l"));
    assert_eq!(mail.region(), Region::Pager);
    assert!(press(&mut mail, &mut shell, "h"));
    assert_eq!(mail.region(), Region::Index);
}

/// Every string in the laid-out widget tree.
///
/// A [`Selector`](iced_test::Selector) that returns nothing walks the whole tree, because `find`
/// stops only when something is selected — so this collects on the way past and lets the search
/// come back empty-handed, which is the shortest way to "what words are on screen".
fn render(mail: &Mail, shell: &Shell) -> Vec<String> {
    use iced_selector::Candidate;
    use std::sync::{Arc, Mutex};

    let element = mail.view(shell, Instant::now()).map(|_| ());
    let mut simulator = iced_test::simulator(element);
    // Laying it out is half the test: a windowed list that measured itself wrong panics here.
    let _ = simulator.snapshot(&Theme::Dark).expect("the surface lays out and draws");

    let found: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let collecting = Arc::clone(&found);
    let _: Result<(), _> = simulator.find(move |candidate: Candidate<'_>| -> Option<()> {
        let mut seen = collecting.lock().expect("nothing else holds this");
        match candidate {
            Candidate::Text { content, .. } => seen.push(content.to_string()),
            Candidate::TextInput { state, .. } => seen.push(state.text().to_string()),
            _ => {}
        }
        None
    });
    let words = found.lock().expect("nothing else holds this");
    words.clone()
}
