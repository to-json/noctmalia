#!/usr/bin/env python3
"""The development fixture: the people and accounts our test data is made of.

One definition, two consumers. `fake-bridge.py` serves it from memory so the UI runs with no
container; `seed.py` writes the same set into a real Thunderbird profile. Keeping them identical
means what you see against the stand-in is what you get against Thunderbird.

    tools/fixture.py greenmail-users   # the user list GreenMail needs for ACCOUNTS
    tools/fixture.py eml DIRECTORY     # write the mail corpus out as .eml files
"""

import sys


def vcard(fn, last, first, org="", title="", emails=(), tels=(), note="", extra=()):
    lines = ["BEGIN:VCARD", "VERSION:4.0", f"FN:{fn}", f"N:{last};{first};;;"]
    if org:
        lines.append(f"ORG:{org}")
    if title:
        lines.append(f"TITLE:{title}")
    for kind, value in emails:
        lines.append(f"EMAIL;TYPE={kind}:{value}")
    for kind, value in tels:
        lines.append(f"TEL;TYPE={kind}:{value}")
    if note:
        lines.append(f"NOTE:{note}")
    lines.extend(extra)
    lines.append("END:VCARD")
    return "\r\n".join(lines) + "\r\n"


# `name` is what both sides match on: the first two are Thunderbird's own default books, so seeding
# a real profile lands in them rather than creating duplicates. `readOnly` and `remote` are for the
# stand-in only — they exist to exercise the badges in the UI. Thunderbird reports its own.
BOOKS = [
    {"key": "personal", "name": "Personal Address Book", "readOnly": False, "remote": False},
    {"key": "collected", "name": "Collected Addresses", "readOnly": True, "remote": False},
    {"key": "work", "name": "Work", "readOnly": False, "remote": True},
]

# Alice carries a UID and an X- property on purpose: Thunderbird must hand them back unchanged
# after an edit, and that is the round trip `docs/findings.md` §8 asks us to confirm.
CONTACTS = [
    ("personal", vcard("Alice Chen", "Chen", "Alice", "Noctalia", "Compositor wrangler",
                       [("work", "alice@noctalia.dev"), ("home", "alice@example.com")],
                       [("cell", "+1 555 0100")], "Owes me a compositor patch.",
                       ["UID:urn:uuid:alice-0001", "X-THUNDERBIRD-KEEP:round-tripped"])),
    ("personal", vcard("Bob Builder", "Builder", "Bob", "", "",
                       [("home", "bob@example.com")], [("work", "+1 555 0111")])),
    ("personal", vcard("Dana Okoro", "Okoro", "Dana", "Thunderbird", "Release engineer",
                       [("work", "dana@noctmalia.test")], [],
                       "Ask about the calendar Experiment.",
                       ["ADR;TYPE=work:;;1 Long Street;Springfield;OR;97477;USA",
                        "URL:https://thunderbird.example/dana"])),
    ("personal", vcard("Ezra Vance", "Vance", "Ezra", "Mozilla", "",
                       [("work", "ezra@example.org")], [("cell", "+44 20 7946 0000")])),
    ("work", vcard("Support Desk", "", "", "Acme", "",
                   [("work", "support@acme.example")], [("work", "+1 555 0199")])),
    ("work", vcard("Priya Raman", "Raman", "Priya", "Acme", "Account manager",
                   [("work", "priya@acme.example")], [("cell", "+1 555 0123")])),
    ("collected", vcard("noreply@lists.example", "", "", "", "",
                        [("internet", "noreply@lists.example")])),
]

# Two mailboxes, so mail between them can be tested without leaving the stack. These are the exact
# parameters `dev.provisionAccount` takes; the hostnames are compose service names. `seed.sh`
# exports `greenmail-users` from here, so adding an account needs no change to compose.yaml.
ACCOUNTS = [
    {
        "name": "greenmail", "email": "j@noctmalia.test", "fullName": "J",
        "imap": {"host": "greenmail", "port": 3143, "socketType": "plain",
                 "username": "j", "password": "secret"},
        "smtp": {"host": "greenmail", "port": 3025, "socketType": "plain", "auth": "none"},
    },
    {
        "name": "greenmail-dana", "email": "dana@noctmalia.test", "fullName": "Dana Okoro",
        "imap": {"host": "greenmail", "port": 3143, "socketType": "plain",
                 "username": "dana", "password": "secret"},
        "smtp": {"host": "greenmail", "port": 3025, "socketType": "plain", "auth": "none"},
    },
]


# ── Mail ────────────────────────────────────────────────────────────────────────────────────────
#
# One corpus, deliberately nasty. Everything here exists to make some part of the client prove
# itself: a thread deep enough to collapse, a newsletter that would track you, a relay chain that
# does not join up, a display name wearing somebody else's address, a subject that is not ASCII, an
# attachment, a body too big to be polite, a reply with no Message-ID, and a letter somebody wrote
# in Markdown on purpose.
#
# `fake-bridge.py` serves it as `messages.getFull` so the UI runs with no container; `seed.py`
# writes the same set into a real profile with `messages.import`. Same mail on screen either way.

FOLDERS = ["inbox", "archives", "sent", "junk", "trash", "Lists"]

_NEWSLETTER = """\
<!doctype html><html><head><style>.x{color:#fff}</style>
<script>fetch('https://track.shopfront.example/open?u=j')</script></head>
<body style="background:#fff;color:#111">
<table width="100%"><tr><td align="center">
  <img src="https://track.shopfront.example/pixel.gif?u=j" width="1" height="1" alt="">
  <table width="600"><tr><td>
    <h1>Half price, this week only</h1>
    <p>Dear <b>valued customer</b>,</p>
    <p>Everything in the shop is <i>half price</i> until Sunday &mdash; including
       the <a href="https://shopfront.example/kettles">kettles</a> you looked at.</p>
    <ul><li>Kettles</li><li>Toasters</li><li>Things that are neither</li></ul>
    <table><tr><th>Item</th><th>Was</th><th>Now</th></tr>
      <tr><td>Kettle</td><td>40</td><td>20</td></tr>
      <tr><td>Toaster</td><td>30</td><td>15</td></tr></table>
    <p><a href="https://shopfront.example.evil.ru/claim">shopfront.example</a></p>
    <img src="https://cdn.shopfront.example/kettle.png" alt="a very good kettle" width="400">
  </td></tr></table>
</td></tr></table>
</body></html>"""

_MARKDOWN = """\
Morning —

Three things, in order of how much they will annoy you:

1. The **maildir idea is dead**. Thunderbird keeps no bodies as files, but it
   keeps everything *about* them: threads as `conversationID`, bodies as indexed
   text in Gloda. So we deleted a milestone.
2. `messages.import` is the test lever. No SMTP, no waiting, no IDLE.
3. The renderer is `iced::widget::markdown`. One road for plain, md and html.

> we make mail a directory full of files like it used to be in unix
> i'm not actually rock solid on this

You were right to hedge. See <https://noctalia.dev/notes> for the rest.

— J"""

_FLOWED = """\
This one is format=flowed, which means the lines you are reading were \
wrapped by my mail client rather than by me, and your mail client is \
supposed to put them back together at whatever width you are actually \
reading at.

    This bit is indented and should stay that way.

-- \
Dana Okoro
Thunderbird release engineering"""

_HOSTILE_RECEIVED = [
    "from mail-out.shopfront.example (mail-out.shopfront.example [203.0.113.9]) "
    "by mx.noctmalia.test with ESMTPS id 4X1; Tue, 09 Sep 2026 08:02:11 +0000",
    "from unknown (unknown [198.51.100.77]) by relay.somewhere-else.example "
    "with SMTP id 9Q7; Tue, 09 Sep 2026 08:02:04 +0000",
]


def _thread(subject, count, day):
    """A conversation deep enough to be worth collapsing."""
    people = [("Priya Raman", "priya@acme.example"), ("J", "j@noctmalia.test")]
    messages = []
    for step in range(count):
        who, address = people[step % 2]
        first = step == 0
        messages.append({
            "folder": "inbox",
            "from": f"{who} <{address}>",
            "to": ["j@noctmalia.test" if step % 2 else "priya@acme.example"],
            "subject": subject if first else f"Re: {subject}",
            "message-id": f"<thread-{step}@acme.example>",
            "in-reply-to": None if first else f"<thread-{step - 1}@acme.example>",
            "date": _when(day, 9 + step),
            "read": step < count - 1,
            "thread": "budget",
            "body": ("plain", _THREAD_BODIES[step % len(_THREAD_BODIES)]),
        })
    return messages


_THREAD_BODIES = [
    "Can we move the review to Thursday? I have the numbers by then.",
    "> Can we move the review to Thursday?\n\nThursday works. Same room?",
    "> Thursday works. Same room?\n\nSame room. I will bring the projector\nthat does not work.",
    "> I will bring the projector that does not work.\n\nPerfect. I will bring the one that does.",
    "> Perfect. I will bring the one that does.\n\nBetween us that is one working projector.",
]


def _when(days_ago, hour=10, minute=17):
    """An ISO 8601 timestamp `days_ago` days back, which is what a JS Date crosses the bridge as."""
    import datetime
    moment = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=days_ago)
    return moment.replace(hour=hour % 24, minute=minute, second=0, microsecond=0).isoformat().replace("+00:00", "Z")


def messages():
    """The corpus. A function rather than a constant because the dates are relative to now."""
    corpus = _thread("Budget review", 5, 2)
    corpus += [
        {
            "folder": "inbox",
            "from": "Shopfront <deals@shopfront.example>",
            "to": ["j@noctmalia.test"],
            "subject": "Half price, this week only",
            "message-id": "<news-1@shopfront.example>",
            "date": _when(0, 8, 2),
            "read": False,
            "received": _HOSTILE_RECEIVED,
            "list-id": "<deals.shopfront.example>",
            "list-unsubscribe": "<https://shopfront.example/u?x=1>, <mailto:unsub@shopfront.example>",
            "authentication-results": "mx.noctmalia.test; spf=pass smtp.mailfrom=shopfront.example; dkim=pass; dmarc=pass",
            "body": ("alternative", "This email requires HTML.", _NEWSLETTER),
        },
        {
            "folder": "inbox",
            # The oldest trick there is: the name is an address, and it is not this one.
            "from": '"security@your-bank.example" <collections@mail.example.ru>',
            "to": ["j@noctmalia.test"],
            "reply-to": "helpdesk@free-mail.example",
            "subject": "Urgent: verify your account \u202eexe.fdp\u202c",
            "message-id": "<phish-1@mail.example.ru>",
            "date": _when(0, 6, 44),
            "read": False,
            "received": _HOSTILE_RECEIVED,
            "authentication-results": "mx.noctmalia.test; spf=fail smtp.mailfrom=mail.example.ru; dkim=none; dmarc=fail",
            "body": ("html", '<p>Your account is locked. <a href="https://mail.example.ru/verify">'
                             "your-bank.example</a> to unlock it.</p>"),
        },
        {
            "folder": "inbox",
            "from": "Dana Okoro <dana@noctmalia.test>",
            "to": ["j@noctmalia.test"],
            "subject": "Re: caf\u00e9, sm\u00f8rrebr\u00f8d, \u30b3\u30fc\u30d2\u30fc",
            "message-id": "<utf8-1@noctmalia.test>",
            "date": _when(1, 14, 5),
            "read": False,
            "flagged": True,
            "authentication-results": "mx.noctmalia.test; dkim=pass; dmarc=pass",
            "body": ("plain", _FLOWED),
            "flowed": True,
        },
        {
            "folder": "inbox",
            "from": "Alice Chen <alice@noctalia.dev>",
            "to": ["j@noctmalia.test"],
            "subject": "Notes from the maildir argument",
            "message-id": "<md-1@noctalia.dev>",
            "date": _when(1, 9, 30),
            "read": False,
            "authentication-results": "mx.noctmalia.test; dkim=pass; dmarc=pass",
            "body": ("markdown", _MARKDOWN),
        },
        {
            "folder": "inbox",
            "from": "Ezra Vance <ezra@example.org>",
            "to": ["j@noctmalia.test"],
            "subject": "The specification, as promised",
            "message-id": "<att-1@example.org>",
            "date": _when(3, 11, 0),
            "read": True,
            "authentication-results": "mx.noctmalia.test; dkim=pass; dmarc=pass",
            "body": ("attached", "Attached. It is shorter than it looks.",
                     [("protocol.txt", "text/plain", "bridge protocol v1\nNDJSON, one object per line.\n" * 40),
                      ("kettle.png", "image/png", "\u0089PNG\r\n\u001a\n" + "pretend" * 200)]),
        },
        {
            "folder": "inbox",
            "from": "logs@build.example",
            "to": ["j@noctmalia.test"],
            "subject": "nightly build output",
            # No Message-ID at all, which is legal-ish and is what a threader trips over.
            "message-id": None,
            "date": _when(4, 3, 12),
            "read": True,
            "body": ("plain", "\n".join(f"[{n:05}] compiled crate number {n}" for n in range(1, 900))),
        },
        {
            "folder": "inbox",
            "from": "Bulk Sender <noreply@lists.example>",
            "to": ["j@noctmalia.test"],
            "subject": "Half a megabyte of nothing in particular",
            "message-id": "<big-1@lists.example>",
            "date": _when(6, 17, 45),
            "read": True,
            # Half a megabyte rather than two: `messages.import` crosses the bridge as base64
            # inside one JSON line, and Gecko caps what reaches the extension at 1 MiB. This is
            # still a brutal thing to hand a Markdown parser.
            "body": ("plain", ("The same sentence, over and over, to see what a long body does to "
                               "the renderer and to the scrollbar. ") * 6000),
        },
        {
            "folder": "archives",
            "from": "Priya Raman <priya@acme.example>",
            "to": ["j@noctmalia.test"],
            "subject": "Last quarter",
            "message-id": "<arch-1@acme.example>",
            "date": _when(96, 10, 0),
            "read": True,
            "body": ("plain", "Filed. Nothing to do."),
        },
        {
            "folder": "sent",
            "from": "J <j@noctmalia.test>",
            "to": ["priya@acme.example"],
            "subject": "Re: Budget review",
            "message-id": "<sent-1@noctmalia.test>",
            "date": _when(2, 13, 0),
            "read": True,
            "body": ("plain", "Sent from the surface that sends things."),
        },
        {
            "folder": "junk",
            "from": "winner@prize.example",
            "to": ["j@noctmalia.test"],
            "subject": "YOU HAVE WON",
            "message-id": "<junk-1@prize.example>",
            "date": _when(5, 2, 2),
            "read": False,
            "junk": True,
            "body": ("plain", "Click here to claim: http://prize.example/claim"),
        },
    ]
    for index, message in enumerate(corpus, start=1):
        message.setdefault("read", False)
        message.setdefault("flagged", False)
        message.setdefault("junk", False)
        message["index"] = index
    return corpus


def flood(count, folder="inbox"):
    """`count` unremarkable messages, for pointing a windowed index at a folder that deserves one."""
    people = [("Priya Raman", "priya@acme.example"), ("Ezra Vance", "ezra@example.org"),
              ("Dana Okoro", "dana@noctmalia.test"), ("build", "logs@build.example")]
    subjects = ["Weekly report", "Re: the thing", "Deployment finished", "Invoice", "Standup notes",
                "Re: Re: the other thing", "Reminder", "Your receipt"]
    made = []
    for index in range(1, count + 1):
        who, address = people[index % len(people)]
        made.append({
            "folder": folder,
            "from": f"{who} <{address}>",
            "to": ["j@noctmalia.test"],
            "subject": f"{subjects[index % len(subjects)]} #{index}",
            "message-id": f"<flood-{index}@example>",
            "date": _when(index // 8, 8 + index % 12, index % 60),
            "read": index % 4 != 0,
            "flagged": index % 37 == 0,
            "index": 10_000 + index,
            "body": ("plain", f"Message number {index}. Nothing about it is interesting, which is\n"
                              "the point: an index has to survive a folder full of these."),
        })
    return made


# ── Calendar ────────────────────────────────────────────────────────────────────────────────────
#
# Two calendars and five events: a daily repeater with a reminder, a plain meeting, a one-off with
# a reminder, an all-day trip spanning several days, and a weekly repeater in the past — enough to
# put something on screen in month, week, day and agenda alike. Dates are relative to now, the same
# way the mail corpus's are, so the fixture always looks like "this week" whenever it is run.
#
# `fake-bridge.py` serves this from memory; nothing here is written into a real Thunderbird profile
# yet (`tools/seed.py` seeds mail and contacts only) — the calendar Experiment's own CRUD is what
# `tools/smoke.sh`'s "calendar round trip" exercises against the real thing.

CALENDARS = [
    {"id": "personal", "name": "Personal", "color": "#7c6df2"},
    {"id": "work", "name": "Work", "color": "#2f9e6e"},
]


def _cal_stamp(days_offset, hour=9, minute=0):
    """A compact UTC instant, `days_offset` days from now — `DTSTART`/`DTEND`'s own format."""
    import datetime
    moment = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(days=days_offset)
    return moment.replace(hour=hour, minute=minute, second=0, microsecond=0).strftime("%Y%m%dT%H%M%SZ")


def _cal_date(days_offset):
    """A bare date, `days_offset` days from now — an all-day `DTSTART`/`DTEND`'s own format."""
    import datetime
    day = datetime.datetime.now(datetime.timezone.utc).date() + datetime.timedelta(days=days_offset)
    return day.strftime("%Y%m%d")


def ical(uid, summary, start, end=None, all_day=False, location="", description="", rrule=None, alarm_minutes=None):
    """One `VEVENT` wrapped in its `VCALENDAR` — the shape `calendar.items.create` takes and
    `calendar.items.query` hands back; see `crate::ical`."""
    lines = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//noctmalia//fixture//EN",
              "BEGIN:VEVENT", f"UID:{uid}", "DTSTAMP:20260901T000000Z", f"SUMMARY:{summary}"]
    if all_day:
        lines.append(f"DTSTART;VALUE=DATE:{start}")
        lines.append(f"DTEND;VALUE=DATE:{end}")
    else:
        lines.append(f"DTSTART:{start}")
        lines.append(f"DTEND:{end}")
    if location:
        lines.append(f"LOCATION:{location}")
    if description:
        lines.append(f"DESCRIPTION:{description}")
    if rrule:
        lines.append(f"RRULE:{rrule}")
    if alarm_minutes is not None:
        lines += ["BEGIN:VALARM", "ACTION:DISPLAY", f"DESCRIPTION:{summary}", f"TRIGGER:-PT{alarm_minutes}M", "END:VALARM"]
    lines += ["END:VEVENT", "END:VCALENDAR"]
    return "\r\n".join(lines) + "\r\n"


def events():
    """The calendar corpus. A function rather than a constant, like `messages()`, so it is always
    relative to now."""
    return [
        {"calendar": "work", "id": "standup", "item": ical(
            "standup-1@noctmalia.test", "Standup", _cal_stamp(0, 9, 0), _cal_stamp(0, 9, 15),
            rrule="FREQ=DAILY", alarm_minutes=10)},
        {"calendar": "work", "id": "review", "item": ical(
            "review-1@noctmalia.test", "Budget review", _cal_stamp(1, 14, 0), _cal_stamp(1, 15, 0),
            location="Room 4", description="Bring the numbers")},
        {"calendar": "personal", "id": "dentist", "item": ical(
            "dentist-1@noctmalia.test", "Dentist", _cal_stamp(3, 10, 30), _cal_stamp(3, 11, 0),
            alarm_minutes=60)},
        {"calendar": "personal", "id": "trip", "item": ical(
            "trip-1@noctmalia.test", "Long weekend", _cal_date(5), _cal_date(8), all_day=True)},
        {"calendar": "work", "id": "allhands", "item": ical(
            "allhands-1@noctmalia.test", "All hands", _cal_stamp(-2, 16, 0), _cal_stamp(-2, 17, 0),
            rrule="FREQ=WEEKLY")},
    ]


def eml(message):
    """One message as RFC 822, for `messages.import`."""
    from email.message import EmailMessage
    from email.utils import format_datetime
    import datetime

    kind = message["body"][0]
    note = EmailMessage()
    note["From"] = message["from"]
    note["To"] = ", ".join(message["to"])
    note["Subject"] = message["subject"]
    if message.get("message-id"):
        note["Message-ID"] = message["message-id"]
    if message.get("in-reply-to"):
        note["In-Reply-To"] = message["in-reply-to"]
        note["References"] = message["in-reply-to"]
    stamp = datetime.datetime.fromisoformat(message["date"].replace("Z", "+00:00"))
    note["Date"] = format_datetime(stamp)
    for header in ("reply-to", "list-id", "list-unsubscribe", "authentication-results"):
        if message.get(header):
            note[header.title()] = message[header]
    # Received headers stack newest first, which is what makes a broken chain visible.
    for hop in message.get("received", []):
        note["Received"] = hop

    if kind == "plain":
        note.set_content(message["body"][1])
    elif kind == "markdown":
        note.set_content(message["body"][1])
        note.replace_header("Content-Type", 'text/markdown; charset="utf-8"')
    elif kind == "html":
        note.set_content("", subtype="plain")
        note.add_alternative(message["body"][1], subtype="html")
    elif kind == "alternative":
        note.set_content(message["body"][1])
        note.add_alternative(message["body"][2], subtype="html")
    elif kind == "attached":
        note.set_content(message["body"][1])
        for name, content_type, payload in message["body"][2]:
            major, _, minor = content_type.partition("/")
            note.add_attachment(payload.encode("utf-8", "surrogateescape"),
                                maintype=major, subtype=minor, filename=name)
    else:
        raise ValueError(f"unknown body kind: {kind}")
    if message.get("flowed") and note.get_content_type() == "text/plain":
        note.set_param("format", "flowed")
    return note.as_string()


def greenmail_users():
    """The `-Dgreenmail.users` value that gives ACCOUNTS somewhere to log in."""
    return ",".join(
        f"{account['imap']['username']}:{account['imap']['password']}@{account['email'].split('@')[1]}"
        for account in ACCOUNTS
    )


def field(card, name):
    """One unfolded property value out of a vCard, enough for matching our own fixture."""
    unfolded = card.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "")
    for line in unfolded.split("\n"):
        head, _, value = line.partition(":")
        if head == name or head.startswith(f"{name};"):
            return value
    return ""


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "greenmail-users":
        print(greenmail_users())
    elif len(sys.argv) == 3 and sys.argv[1] == "eml":
        import os
        os.makedirs(sys.argv[2], exist_ok=True)
        for message in messages():
            name = os.path.join(sys.argv[2], f"{message['index']:02}-{message['folder']}.eml")
            with open(name, "w", encoding="utf-8") as handle:
                handle.write(eml(message))
            print(name)
    else:
        print(__doc__.strip(), file=sys.stderr)
        sys.exit(2)
