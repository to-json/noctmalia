#!/usr/bin/env python3
"""Write the development fixture into a real Thunderbird profile.

Talks to the bridge through the mailnd stub's control socket, beside `bridgectl.py`:

    docker compose exec -T mailnd python /tools/seed.py

Idempotent. Address books are matched by name and contacts by FN, so running it twice leaves one
of each; --reset deletes the fixture's own contacts first and writes them fresh. Only one client
can hold the bridge socket at a time, so seed with the stub attached, then bring the UI up against
the same profile volume (`compose.ui.yaml`) to look at the result.
"""

import argparse
import sys

import bridgectl
import fixture


class BridgeError(RuntimeError):
    pass


def call(method, params=None, timeout=60):
    reply = bridgectl.request({"op": "call", "method": method, "params": params or {}, "timeout": timeout})
    if "error" in reply:
        error = reply["error"]
        raise BridgeError(f"{method}: {error.get('name', 'Error')}: {error.get('message', '')}")
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
                    "dev.provisionAccount needs the dev pref. Start the stack with\n"
                    "  TBD_EXTRA_PREFS='user_pref(\"extensions.noctmalia.dev\", true);'\n"
                    "which tools/seed.sh does for you."
                ) from None
            raise
        print(f"account {account['name']!r}: created {result.get('accountId')}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--contacts", action="store_true", help="only the address books and contacts")
    parser.add_argument("--mail", action="store_true", help="only the mail accounts")
    parser.add_argument("--reset", action="store_true", help="replace fixture contacts that are already there")
    options = parser.parse_args()
    both = not (options.contacts or options.mail)

    try:
        if options.mail or both:
            seed_accounts()
        if options.contacts or both:
            seed_contacts(options.reset)
    except BridgeError as error:
        print(f"seed: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"seed: cannot reach the control socket at {bridgectl.CTL_SOCK}: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
