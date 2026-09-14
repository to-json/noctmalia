#!/usr/bin/env python3
"""The development fixture: the people and accounts our test data is made of.

One definition, two consumers. `fake-bridge.py` serves it from memory so the UI runs with no
container; `seed.py` writes the same set into a real Thunderbird profile. Keeping them identical
means what you see against the stand-in is what you get against Thunderbird.

    tools/fixture.py greenmail-users   # the user list GreenMail needs for ACCOUNTS
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
    else:
        print(__doc__.strip(), file=sys.stderr)
        sys.exit(2)
