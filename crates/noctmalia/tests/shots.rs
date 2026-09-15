//! Pictures of the surfaces, for looking at.
//!
//! Not assertions: this renders each surface headlessly and writes a PNG, so the design can be
//! judged by eye without a compositor, a Thunderbird or a mouse. Ignored by default because it
//! writes files and proves nothing.
//!
//! ```sh
//! just shots                      # writes target/shots/*.png
//! NOCTMALIA_SHOTS=/tmp/look cargo test -p noctmalia --test shots -- --ignored
//! ```

use chrono::{Local, NaiveTime};
use iced::{Element, Settings, Size, Theme};
use noctmalia::calendar::{Cal, Item};
use noctmalia::ical::{self, Event, When};
use noctmalia::mail::{self, Reply};
use noctmalia::shell::Shell;
use noctmalia::surfaces::calendar::{Calendar, Message as CalMessage};
use noctmalia::surfaces::mail::{Mail, Message, Showing};
use noctmalia::{font, mime, palette};
use noctmalia_bridge::Bridge;
use serde_json::json;
use std::time::{Duration, Instant};

const WINDOW: Size = Size::new(1180.0, 720.0);

/// Far enough past every animation that the picture is of the settled interface rather than of the
/// first frame of its arrival. A shot taken at `Instant::now()` catches the staggered reveal at
/// zero, which looks like an empty list.
fn settled() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn bridge(which: u32) -> Bridge {
    let path = std::env::temp_dir().join(format!("noctmalia-shot-{}-{which}.sock", std::process::id()));
    Bridge::spawn(path).expect("a socket in the temp directory")
}

fn shot<'a>(name: &str, element: Element<'a, ()>) {
    // The workspace's own target directory, not a second one under the crate: an integration test
    // runs with the package root as its working directory, so a relative path would make one.
    let directory = std::env::var("NOCTMALIA_SHOTS")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots").to_string());
    std::fs::create_dir_all(&directory).expect("somewhere to write to");
    let settings = Settings {
        default_font: font::ui(),
        // Without the icon font every glyph is a replacement box, which is not what the window
        // looks like.
        fonts: vec![noctalia_iced::theme::ICON_FONT_BYTES.into()],
        ..Settings::default()
    };
    let mut simulator = iced_test::Simulator::with_size(settings, WINDOW, element);
    let snapshot = simulator.snapshot(&Theme::Dark).expect("it draws");
    // `matches_image` writes the file only when it is missing, and names it after the renderer it
    // drew with — so an old picture has to go before a new one can be taken.
    let path = std::path::Path::new(&directory).join(format!("{name}.png"));
    for stale in std::fs::read_dir(&directory).into_iter().flatten().flatten() {
        if stale.file_name().to_string_lossy().starts_with(&format!("{name}-")) {
            let _ = std::fs::remove_file(stale.path());
        }
    }
    assert!(snapshot.matches_image(&path).expect("write the png"));
    eprintln!("wrote {}", path.display());
}

/// The surface inside the window chrome, exactly as `App::build` assembles it: the title names
/// where you are, and the surfaces you are not in sit beside it as capsules.
fn dressed<'a>(title: &'a str, content: Element<'a, ()>) -> Element<'a, ()> {
    use iced::widget::row;
    use noctalia_iced::chrome;
    use noctmalia::surfaces::Surface;
    let mut switcher = row![].spacing(noctalia_iced::theme::SPACE_XS).align_y(iced::Alignment::Center);
    for surface in Surface::ALL.into_iter().filter(|surface| surface.title() != title) {
        switcher = switcher.push(chrome::capsule(surface.glyph(), (), false));
    }
    chrome::frame_with(chrome::initial(), title, switcher, content, |_| ())
}

fn accounts() -> serde_json::Value {
    let folder = |name: &str, label: &str, special: &str| {
        json!({"id": format!("account1://{name}"), "name": label, "path": format!("/{name}"),
               "accountId": "account1", "specialUse": if special.is_empty() { vec![] } else { vec![special] },
               "subFolders": []})
    };
    json!([{
        "id": "account1", "name": "greenmail", "type": "imap",
        "folders": [{
            "id": "account1://", "name": "", "path": "/", "accountId": "account1",
            "subFolders": [
                folder("inbox", "Inbox", "inbox"),
                folder("drafts", "Drafts", "drafts"),
                folder("sent", "Sent", "sent"),
                folder("archives", "Archive", "archives"),
                folder("lists", "Lists", ""),
                folder("projects", "Projects", ""),
                folder("junk", "Junk", "junk"),
                folder("trash", "Trash", "trash"),
            ],
        }],
    }])
}

fn header(id: u64, subject: &str, author: &str, day: u32, hour: u32, read: bool, flagged: bool) -> serde_json::Value {
    json!({
        "id": id,
        "headerMessageId": format!("m{id}@example"),
        "subject": subject,
        "author": author,
        "recipients": ["J <j@noctmalia.test>"],
        "date": format!("2026-09-{day:02}T{hour:02}:17:00Z"),
        "read": read,
        "flagged": flagged,
        "folder": {"id": "account1://inbox", "name": "Inbox"},
    })
}

fn inbox() -> Vec<serde_json::Value> {
    vec![
        header(1, "Half price, this week only", "Shopfront <deals@shopfront.example>", 14, 8, false, false),
        header(
            2,
            "Urgent: verify your account",
            "\"security@your-bank.example\" <collect@mail.example.ru>",
            14,
            6,
            false,
            false,
        ),
        header(3, "Notes from the maildir argument", "Alice Chen <alice@noctalia.dev>", 13, 9, false, true),
        // Thunderbird strips the `Re:` off a subject before it reaches a MessageHeader, so a
        // thread is five messages under one name. Writing the fixture any other way would be
        // drawing an index that no real profile produces.
        header(4, "café, smørrebrød, コーヒー", "Dana Okoro <dana@noctmalia.test>", 13, 14, false, false),
        header(5, "Budget review", "Priya Raman <priya@acme.example>", 12, 9, true, false),
        header(6, "Budget review", "J <j@noctmalia.test>", 12, 10, true, false),
        header(7, "Budget review", "Priya Raman <priya@acme.example>", 12, 11, true, false),
        header(8, "Budget review", "J <j@noctmalia.test>", 12, 12, true, false),
        header(9, "Budget review", "Priya Raman <priya@acme.example>", 12, 13, false, false),
        header(10, "The specification, as promised", "Ezra Vance <ezra@example.org>", 11, 11, true, false),
        header(11, "nightly build output", "logs@build.example", 10, 3, true, false),
        header(12, "Half a megabyte of nothing in particular", "Bulk <noreply@lists.example>", 8, 17, true, false),
    ]
}

const NEWSLETTER: &str = concat!(
    "<script>fetch('https://track.shopfront.example/open?u=j')</script>",
    "<style>.x{color:#fff}</style>",
    "<img src=\"https://track.shopfront.example/pixel.gif\" width=\"1\" height=\"1\" alt=\"\">",
    "<table width=\"100%\"><tr><td><table width=\"600\"><tr><td>",
    "<h1>Half price, this week only</h1>",
    "<p>Dear <b>valued customer</b>,</p>",
    "<p>Everything in the shop is <i>half price</i> until Sunday &mdash; including the ",
    "<a href=\"https://shopfront.example/kettles\">kettles</a> you looked at.</p>",
    "<ul><li>Kettles</li><li>Toasters</li><li>Things that are neither</li></ul>",
    "<table><tr><th>Item</th><th>Was</th><th>Now</th></tr>",
    "<tr><td>Kettle</td><td>40</td><td>20</td></tr>",
    "<tr><td>Toaster</td><td>30</td><td>15</td></tr></table>",
    "<p>Claim it at <a href=\"https://shopfront.example.evil.ru/claim\">shopfront.example</a>.</p>",
    "<img src=\"https://cdn.shopfront.example/kettle.png\" alt=\"a very good kettle\" width=\"400\">",
);

fn newsletter() -> mime::Part {
    serde_json::from_value(json!({
        "contentType": "multipart/mixed", "partName": "",
        "headers": {
            "from": ["Shopfront <deals@shopfront.example>"],
            "to": ["J <j@noctmalia.test>"],
            "subject": ["Half price, this week only"],
            "date": ["2026-09-14T08:02:00Z"],
            "list-id": ["<deals.shopfront.example>"],
            "list-unsubscribe": ["<https://shopfront.example/u?x=1>, <mailto:unsub@shopfront.example>"],
            "authentication-results": ["mx.noctmalia.test; spf=pass; dkim=pass; dmarc=pass"],
        },
        "parts": [
            {"contentType": "multipart/alternative", "partName": "1", "parts": [
                {"contentType": "text/plain", "partName": "1.1", "body": "This email requires HTML."},
                {"contentType": "text/html", "partName": "1.2", "body": NEWSLETTER},
            ]},
            {"contentType": "application/pdf", "partName": "2", "name": "catalogue.pdf", "size": 284_160},
        ],
    }))
    .expect("a message")
}

fn phish() -> mime::Part {
    serde_json::from_value(json!({
        "contentType": "text/html", "partName": "1",
        "body": "<p>Your account is locked. Visit <a href=\"https://mail.example.ru/verify\">your-bank.example</a> to unlock it.</p>",
        "headers": {
            "from": ["\"security@your-bank.example\" <collect@mail.example.ru>"],
            "to": ["J <j@noctmalia.test>"],
            "subject": ["Urgent: verify your account"],
            "date": ["2026-09-14T06:44:00Z"],
            "reply-to": ["helpdesk@free-mail.example"],
            "authentication-results": ["mx.noctmalia.test; spf=fail; dkim=none; dmarc=fail"],
            "received": [
                "from mail-out.shopfront.example (mail-out.shopfront.example [203.0.113.9]) by mx.noctmalia.test with ESMTPS",
                "from unknown (unknown [198.51.100.77]) by relay.somewhere-else.example with SMTP",
            ],
        },
    }))
    .expect("a message")
}

fn markdown_letter() -> mime::Part {
    serde_json::from_value(json!({
        "contentType": "text/markdown", "partName": "1",
        "body": "Morning —\n\nThree things, in order of how much they will annoy you:\n\n\
                 1. The **maildir idea is dead**. Thunderbird keeps no bodies as files, but it\n\
                 keeps everything *about* them: threads as `conversationID`, bodies as indexed\n\
                 text in Gloda. So we deleted a milestone.\n\
                 2. `messages.import` is the test lever. No SMTP, no waiting, no IDLE.\n\
                 3. The renderer is `iced::widget::markdown`. One road for plain, md and html.\n\n\
                 > we make mail a directory full of files like it used to be in unix\n\
                 > i'm not actually rock solid on this\n\n\
                 You were right to hedge. See <https://noctalia.dev/notes> for the rest.\n\n— J",
        "headers": {
            "from": ["Alice Chen <alice@noctalia.dev>"],
            "to": ["J <j@noctmalia.test>"],
            "subject": ["Notes from the maildir argument"],
            "date": ["2026-09-13T09:30:00Z"],
            "authentication-results": ["mx.noctmalia.test; dkim=pass; dmarc=pass"],
        },
    }))
    .expect("a message")
}

fn ready(which: u32) -> (Mail, Shell) {
    let mut shell = Shell::new(bridge(which), WINDOW.width);
    shell.set_connected(true);
    let mut mail = Mail::new();
    let now = Instant::now();
    let accounts = serde_json::from_value(accounts()).expect("accounts");
    let _ = mail.update(Message::Accounts(Ok(accounts)), &mut shell, now);
    let identities = serde_json::from_value(json!([
        {"id": "id1", "email": "j@noctmalia.test", "name": "J", "accountId": "account1"}
    ]))
    .expect("identities");
    let _ = mail.update(Message::Identities(Ok(identities)), &mut shell, now);
    let page = serde_json::from_value(json!({"id": null, "messages": inbox()})).expect("a page");
    let _ = mail.update(Message::Page(1, Ok(page)), &mut shell, now);
    let threads = serde_json::from_value(json!([
        {"id": 1, "subject": "Budget review",
         "messages": ["m5@example", "m6@example", "m7@example", "m8@example", "m9@example"]}
    ]))
    .expect("conversations");
    let _ = mail.update(Message::Conversations(1, Ok(threads)), &mut shell, now);
    for (folder, unread, total) in
        [("inbox", 5, 12), ("drafts", 0, 1), ("archives", 0, 412), ("lists", 23, 96), ("junk", 2, 2)]
    {
        let counts = serde_json::from_value(
            json!({"totalMessageCount": total, "unreadMessageCount": unread, "newMessageCount": 0}),
        )
        .expect("counts");
        let _ = mail.update(Message::Counts(Ok((format!("account1://{folder}"), counts))), &mut shell, now);
    }
    for (id, part) in [(1, newsletter()), (2, phish()), (3, markdown_letter())] {
        let _ = mail.update(Message::Letter(id, Ok(mail::read(id, &part))), &mut shell, now);
    }
    (mail, shell)
}

/// A calendar with two calendars and a handful of events, all placed relative to today the same
/// way the mail fixture places its messages relative to now.
fn cal_ready(which: u32) -> (Calendar, Shell) {
    let mut shell = Shell::new(bridge(which), WINDOW.width);
    shell.set_connected(true);
    let mut calendar = Calendar::new();
    let now = Instant::now();

    let cals = vec![
        Cal { id: "personal".to_string(), name: "Personal".to_string(), color: None, hidden: false, read_only: false },
        Cal { id: "work".to_string(), name: "Work".to_string(), color: None, hidden: false, read_only: false },
    ];
    let _ = calendar.update(CalMessage::Cals(Ok(cals)), &mut shell, now);

    let today = Local::now().date_naive();
    let at = |day_offset: i64, hour: u32, minute: u32| {
        ical::local_from_naive(
            (today + chrono::Duration::days(day_offset)).and_time(NaiveTime::from_hms_opt(hour, minute, 0).unwrap()),
        )
    };
    let timed = |calendar_id: &str, id: &str, title: &str, start, end, rrule: Option<&str>, alarm: Option<i64>| {
        let mut event = Event::blank(When::Time(start), When::Time(end));
        event.summary = title.to_string();
        event.rrule = rrule.map(str::to_string);
        if let Some(minutes) = alarm {
            event.alarms.push(chrono::Duration::minutes(minutes));
        }
        Item { id: id.to_string(), calendar_id: calendar_id.to_string(), instance: None, event }
    };
    let mut items = vec![
        timed("work", "standup", "Standup", at(0, 9, 0), at(0, 9, 15), Some("FREQ=DAILY"), Some(10)),
        timed("work", "review", "Budget review", at(0, 14, 0), at(0, 15, 0), None, None),
        timed("work", "overlap", "1:1 with Priya", at(0, 14, 30), at(0, 15, 0), None, None),
        timed("personal", "dentist", "Dentist", at(3, 10, 30), at(3, 11, 0), None, Some(60)),
    ];
    let mut trip =
        Event::blank(When::Date(today + chrono::Duration::days(5)), When::Date(today + chrono::Duration::days(8)));
    trip.summary = "Long weekend".to_string();
    items.push(Item { id: "trip".to_string(), calendar_id: "personal".to_string(), instance: None, event: trip });
    let _ = calendar.update(CalMessage::Items(1, Ok(items)), &mut shell, now);
    (calendar, shell)
}

/// The dates are relative, so a shot taken tomorrow differs from one taken today; these are for
/// looking at rather than for diffing.
#[test]
#[ignore = "writes PNGs rather than asserting"]
fn every_surface_has_its_picture_taken() {
    // The same two things `main` does before opening a window, so the pictures are of the real
    // typeface and the palette the shell is actually running.
    font::adopt_system_families();
    noctalia_iced::theme::set_font(font::ui());
    if let Some(found) = palette::load() {
        noctalia_iced::theme::set_palette(found);
    }

    let now = Instant::now();
    let then = settled();

    // The whole window, chrome and switcher included, which is what somebody actually sees.
    {
        let (mut mail, mut shell) = ready(9);
        let _ = mail.update(Message::Select(3), &mut shell, now);
        shot("window", dressed("Mail", mail.view(&shell, then).map(|_| ())));
    }

    let (mut mail, mut shell) = ready(0);
    let _ = mail.update(Message::Select(3), &mut shell, now);
    shot("mail-markdown", mail.view(&shell, then).map(|_| ()));

    let _ = mail.update(Message::Select(1), &mut shell, now);
    shot("mail-newsletter", mail.view(&shell, then).map(|_| ()));

    let _ = mail.update(Message::Show(Showing::Original), &mut shell, now);
    shot("mail-original", mail.view(&shell, then).map(|_| ()));

    let _ = mail.update(Message::Show(Showing::Headers), &mut shell, now);
    shot("mail-headers", mail.view(&shell, then).map(|_| ()));

    let (mut mail, mut shell) = ready(1);
    let _ = mail.update(Message::Select(2), &mut shell, now);
    let _ = mail.update(Message::Show(Showing::Security), &mut shell, now);
    shot("mail-security", mail.view(&shell, then).map(|_| ()));

    let (mut mail, mut shell) = ready(2);
    let _ = mail.update(Message::Select(3), &mut shell, now);
    let _ = mail.update(Message::Compose(Some(Reply::Sender)), &mut shell, now);
    shot("mail-compose", mail.view(&shell, then).map(|_| ()));

    let (mut mail, mut shell) = ready(3);
    let _ = mail.update(Message::Select(1), &mut shell, now);
    let _ = mail.update(Message::Screen, &mut shell, now);
    shot("mail-screen", mail.view(&shell, then).map(|_| ()));

    let (mut mail, mut shell) = ready(4);
    let _ = mail.update(Message::Query("budget".to_string()), &mut shell, now);
    shot("mail-filter", mail.view(&shell, then).map(|_| ()));

    let (mail, shell) = ready(5);
    shot("mail-index", mail.view(&shell, then).map(|_| ()));

    let (calendar, shell) = cal_ready(6);
    shot("calendar-month", calendar.view(&shell, then).map(|_| ()));

    let (mut calendar, mut shell) = cal_ready(7);
    let _ = calendar.update(CalMessage::View(noctmalia::surfaces::calendar::ViewKind::Week), &mut shell, now);
    shot("calendar-week", calendar.view(&shell, then).map(|_| ()));

    let (mut calendar, mut shell) = cal_ready(8);
    let _ = calendar.update(CalMessage::View(noctmalia::surfaces::calendar::ViewKind::Day), &mut shell, now);
    shot("calendar-day", calendar.view(&shell, then).map(|_| ()));

    let (mut calendar, mut shell) = cal_ready(10);
    let _ = calendar.update(CalMessage::View(noctmalia::surfaces::calendar::ViewKind::Agenda), &mut shell, now);
    shot("calendar-agenda", calendar.view(&shell, then).map(|_| ()));

    let (mut calendar, mut shell) = cal_ready(11);
    let _ = calendar.update(CalMessage::Open("standup".to_string()), &mut shell, now);
    shot("calendar-editor", calendar.view(&shell, then).map(|_| ()));
}
