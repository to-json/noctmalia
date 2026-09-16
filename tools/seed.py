#!/usr/bin/env python3
"""Write the development fixture into the running window's Thunderbird.

Talks to the bridge through noctmalia's own control socket (`noctmalia-ctl.py call`), which
passes raw bridge methods through only when the window was started with `--dev`:

    just dev            # in one terminal
    tools/seed.py       # in another; or `just seed`
    tools/seed.py --flood 500

Idempotent. Address books are matched by name and contacts by FN, so running it twice leaves one
of each; --reset deletes the fixture's own contacts first and writes them fresh.
"""

import argparse
import importlib.util
import os
import sys

import fixture

_ctl_spec = importlib.util.spec_from_file_location("noctmalia_ctl", os.path.join(os.path.dirname(__file__), "noctmalia-ctl.py"))
noctmalia_ctl = importlib.util.module_from_spec(_ctl_spec)
_ctl_spec.loader.exec_module(noctmalia_ctl)


class BridgeError(RuntimeError):
    pass


def call(method, params=None, timeout=60):
    del timeout  # the bridge's own call timeout applies
    reply = noctmalia_ctl.request("call", {"method": method, "params": params or {}})
    if reply.get("error"):
        detail = reply.get("result", {}).get("error") if isinstance(reply.get("result"), dict) else None
        raise BridgeError(f"{method}: {detail or reply['error']}")
    return reply["result"]


def ensure_books():
    """Fixture key -> address book id, creating the books Thunderbird does not already have."""
    existing = {book["name"]: book["id"] for book in call("addressBooks.list")}
    ids = {}
    for book in fixture.BOOKS:
        name = book["name"]
        if name in existing:
            ids[book["key"]] = existing[name]
            print(f"book {name!r}: have it")
        else:
            ids[book["key"]] = call("addressBooks.create", {"name": name})
            print(f"book {name!r}: created")
    return ids


def seed_contacts(reset):
    ids = ensure_books()
    wanted = {}
    for key, card in fixture.CONTACTS:
        wanted.setdefault(key, []).append(card)

    created = skipped = removed = 0
    for key, cards in wanted.items():
        parent = ids[key]
        present = {}
        for contact in call("contacts.list", {"parentId": parent}):
            present[fixture.field(contact["vCard"], "FN")] = contact["id"]

        for card in cards:
            name = fixture.field(card, "FN")
            if name in present:
                if not reset:
                    skipped += 1
                    continue
                call("contacts.delete", {"contactId": present[name]})
                removed += 1
            call("contacts.create", {"parentId": parent, "vCard": card})
            created += 1
            print(f"contact {name!r} -> {key}")

    print(f"contacts: {created} created, {skipped} already there, {removed} replaced")


def seed_accounts():
    have = {account.get("name") for account in call("accounts.list")}
    for account in fixture.ACCOUNTS:
        if account["name"] in have:
            print(f"account {account['name']!r}: have it")
            continue
        try:
            # Provisioning drives Thunderbird's account manager, which is slower than a normal call.
            result = call("dev.provisionAccount", account, timeout=120)
        except BridgeError as error:
            if "MethodNotFound" in str(error) or "noctmalia" in str(error):
                raise BridgeError(
                    f"{error}\n"
                    "dev.provisionAccount needs the bridge's dev pref: start the window with `just dev`."
                ) from None
            raise
        print(f"account {account['name']!r}: created {result.get('accountId')}")


def seed_test_account():
    """A real account to test against, alongside GreenMail — see `test.secret` at the repo root
    (gitignored, an email on one line and a password on the next, read by nothing else). `seed.sh`
    reads it on the host and hands it down as two environment variables, so the credential is typed
    once, ever, into a file nothing commits — not into this script, not into a shell history, and
    not into anything printed here. A no-op wherever those variables are not set, which is every
    machine that has not been set up for this.
    """
    email = os.environ.get("TEST_ACCOUNT_EMAIL")
    password = os.environ.get("TEST_ACCOUNT_PASSWORD")
    if not email or not password:
        return
    name = f"test-{email}"
    have = {account.get("name") for account in call("accounts.list")}
    if name in have:
        print(f"account {name!r}: have it")
        return

    # Gmail's own hosts; anything else is a guess at the usual `imap./smtp.<domain>` convention,
    # which is right often enough to be worth trying before asking for more configuration.
    domain = email.rsplit("@", 1)[-1]
    imap_host = "imap.gmail.com" if domain == "gmail.com" else f"imap.{domain}"
    smtp_host = "smtp.gmail.com" if domain == "gmail.com" else f"smtp.{domain}"
    config = {
        "name": name,
        "email": email,
        "fullName": "Test",
        "imap": {"host": imap_host, "port": 993, "socketType": "tls", "auth": "cleartext",
                 "username": email, "password": password},
        "smtp": {"host": smtp_host, "port": 465, "socketType": "tls", "auth": "cleartext", "password": password},
    }
    try:
        # Real IMAP providers often refuse a plain password outright (Gmail wants OAuth2 or an
        # app password); that shows up here as a normal BridgeError, not a crash.
        result = call("dev.provisionAccount", config, timeout=120)
    except BridgeError as error:
        print(f"account {name!r}: could not provision it — {error}", file=sys.stderr)
        return
    print(f"account {name!r}: created {result.get('accountId')}")


def folders_of(account):
    """Every folder under an account, flattened, whatever the build calls the special ones."""
    found = []

    def walk(folder):
        found.append(folder)
        for child in folder.get("subFolders") or []:
            walk(child)

    for folder in account.get("folders") or []:
        walk(folder)
    return found


def purpose(folder):
    special = folder.get("specialUse") or []
    return special[0] if special else (folder.get("type") or "")


# What a special folder is called when we have to make it ourselves. A fresh IMAP account has an
# Inbox and a Trash and nothing else — Sent, Drafts and the rest appear the first time Thunderbird
# needs them, which on a profile nobody has sent from is never.
DISPLAY = {"archives": "Archives", "junk": "Junk", "sent": "Sent", "drafts": "Drafts",
           "templates": "Templates", "outbox": "Outbox"}


def ensure_folders(account, wanted):
    """Fixture folder name -> folder id, creating whatever the account does not already have."""
    folders = folders_of(account)
    # An IMAP account's `folders` is [Inbox, Trash, ...] with no unnamed root, so the root to create
    # under is the account's own, and `folders.create` takes the account id in its place.
    root = next((f for f in folders if not f.get("name")), None)
    parent = root["id"] if root else account["id"]
    by_purpose = {purpose(f): f for f in folders if purpose(f)}
    by_name = {f.get("name", "").lower(): f for f in folders}

    ids = {}
    for name in wanted:
        found = by_purpose.get(name) or by_name.get(name.lower()) or by_name.get(DISPLAY.get(name, "").lower())
        if found:
            ids[name] = found["id"]
            continue
        label = DISPLAY.get(name, name)
        try:
            created = call("folders.create", {"parentId": parent, "name": label})
        except BridgeError as error:
            print(f"folder {label!r}: cannot create it ({error}); skipping its mail")
            continue
        ids[name] = created["id"] if isinstance(created, dict) else created
        print(f"folder {label!r}: created")
    return ids


def seed_mail(corpus, reset):
    """Write messages straight into folders with `messages.import` — no SMTP, no IDLE, no waiting."""
    import base64

    accounts = call("accounts.list", {"includeSubFolders": True})
    account = next((a for a in accounts if a.get("name") == fixture.ACCOUNTS[0]["name"]), None)
    if not account:
        raise BridgeError(f"no account named {fixture.ACCOUNTS[0]['name']!r} — seed accounts first")

    wanted = sorted({message["folder"] for message in corpus})
    ids = ensure_folders(account, wanted)

    present = {}
    for name, folder_id in ids.items():
        listing = call("messages.list", {"folderId": folder_id})
        held = list(listing.get("messages") or [])
        while listing.get("id"):
            listing = call("messages.continueList", {"listId": listing["id"]})
            held.extend(listing.get("messages") or [])
        present[name] = ({message.get("headerMessageId") for message in held},
                         {message.get("subject") for message in held})
        if reset and held:
            call("messages.delete", {"messageIds": [m["id"] for m in held], "deletePermanently": True})
            present[name] = (set(), set())
            print(f"folder {name!r}: cleared {len(held)} messages")

    written = skipped = 0
    for message in corpus:
        folder = message["folder"]
        if folder not in ids:
            continue
        by_id, by_subject = present.get(folder, (set(), set()))
        durable = (message.get("message-id") or "").strip("<>")
        # One fixture message has no Message-ID on purpose — which is the thing a threader trips
        # over, and also the thing that would make this re-import it on every run. Fall back to the
        # subject, which is enough to recognise a fixture.
        already = durable in by_id if durable else message["subject"] in by_subject
        if already:
            skipped += 1
            continue
        raw = fixture.eml(message).encode("utf-8", "surrogateescape")
        properties = {"read": message["read"], "flagged": message["flagged"], "new": False}
        if message.get("junk"):
            properties["junk"] = True
        call("messages.import", {
            "folderId": ids[folder],
            "base64": base64.b64encode(raw).decode(),
            "properties": properties,
        }, timeout=120)
        written += 1
    print(f"mail: {written} written, {skipped} already there")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--contacts", action="store_true", help="only the address books and contacts")
    parser.add_argument("--accounts", action="store_true", help="only the mail accounts")
    parser.add_argument("--mail", action="store_true", help="only the mail corpus")
    parser.add_argument("--flood", type=int, metavar="N",
                        help="also write N unremarkable messages, for a folder worth windowing")
    parser.add_argument("--reset", action="store_true",
                        help="replace what is already there rather than leaving it")
    options = parser.parse_args()
    both = not (options.contacts or options.accounts or options.mail or options.flood)

    try:
        if options.accounts or options.mail or options.flood or both:
            seed_accounts()
            seed_test_account()
        if options.contacts or both:
            seed_contacts(options.reset)
        if options.mail or both:
            seed_mail(fixture.messages(), options.reset)
        if options.flood:
            seed_mail(fixture.flood(options.flood), False)
    except BridgeError as error:
        print(f"seed: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"seed: cannot reach the control socket at {noctmalia_ctl.socket_path()}: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
