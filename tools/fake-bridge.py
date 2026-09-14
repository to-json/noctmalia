#!/usr/bin/env python3
"""A Thunderbird stand-in for developing the UI.

Connects to noctmalia's socket the way tbd's nm-shim does, says hello, and serves the contacts
methods out of memory. It exists so the front end can be run and tested without Docker, a profile
or a mail account; it is not a protocol conformance test. Everything it does not implement comes
back as MethodNotFound, exactly as the real bridge would.

    tools/fake-bridge.py [--socket PATH] [--empty]
"""

import argparse
import itertools
import json
import os
import socket
import sys
import time

DEFAULT_SOCKET = os.path.join(
    os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "noctmalia", "bridge.sock"
)


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


SEED = [
    ("personal", vcard("Alice Chen", "Chen", "Alice", "Noctalia", "Compositor wrangler",
                       [("work", "alice@noctalia.dev"), ("home", "alice@example.com")],
                       [("cell", "+1 555 0100")], "Owes me a compositor patch.",
                       ["UID:urn:uuid:alice-0001", "X-THUNDERBIRD-KEEP:round-tripped"])),
    ("personal", vcard("Bob Builder", "Builder", "Bob", "", "",
                       [("home", "bob@example.com")], [("work", "+1 555 0111")])),
    ("personal", vcard("Dana Okoro", "Okoro", "Dana", "Thunderbird", "Release engineer",
                       [("work", "dana@thunderbird.example")], [],
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

BOOKS = [
    {"id": "personal", "name": "Personal", "type": "addressBook", "readOnly": False, "remote": False},
    {"id": "work", "name": "Work (CardDAV)", "type": "addressBook", "readOnly": False, "remote": True},
    {"id": "collected", "name": "Collected Addresses", "type": "addressBook", "readOnly": True, "remote": False},
]


class Store:
    def __init__(self, empty):
        self.contacts = {}
        self.ids = itertools.count(1)
        if not empty:
            for parent, card in SEED:
                self.add(parent, card)

    def add(self, parent, card):
        contact_id = f"contact{next(self.ids)}"
        self.contacts[contact_id] = {"id": contact_id, "parentId": parent, "type": "contact",
                                     "properties": {"vCard": card}, "vCard": card}
        return contact_id

    def matching(self, needle, parent=None):
        needle = needle.lower()
        found = []
        for contact in self.contacts.values():
            if parent and contact["parentId"] != parent:
                continue
            if needle in contact["vCard"].lower():
                found.append(contact)
        return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default=os.environ.get("NOCTMALIA_BRIDGE_SOCKET", DEFAULT_SOCKET))
    parser.add_argument("--empty", action="store_true", help="serve no contacts, to see the empty states")
    options = parser.parse_args()

    store = Store(options.empty)
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for attempt in range(30):
        try:
            connection.connect(options.socket)
            break
        except OSError:
            if attempt == 29:
                raise
            time.sleep(0.5)
    print(f"fake-bridge: connected to {options.socket}", file=sys.stderr)

    stream = connection.makefile("rwb")

    def send(message):
        stream.write((json.dumps(message) + "\n").encode())
        stream.flush()

    def emit(event, data=None):
        send({"event": event, "data": data or {}})

    emit("bridge.hello", {"protocol": 1, "bridgeVersion": "fake", "calendar": False})

    for line in stream:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        request_id, method, params = request.get("id"), request.get("method"), request.get("params") or {}
        print(f"fake-bridge: #{request_id} {method} {json.dumps(params)[:120]}", file=sys.stderr)
        try:
            result = handle(store, method, params, emit)
        except KeyError:
            send({"id": request_id, "error": {"name": "MethodNotFound", "message": f"unknown method: {method}"}})
            continue
        except Exception as error:  # surfaced in the UI, like a real bridge error
            send({"id": request_id, "error": {"name": type(error).__name__, "message": str(error)}})
            continue
        print(f"fake-bridge: #{request_id} -> {json.dumps(result)[:120]}", file=sys.stderr)
        send({"id": request_id, "result": result})

    print("fake-bridge: noctmalia closed the connection", file=sys.stderr)


def handle(store, method, params, emit):
    if method == "bridge.ping":
        return {"pong": int(time.time() * 1000)}
    if method == "addressBooks.list":
        return BOOKS
    if method == "contacts.list":
        return [c for c in store.contacts.values() if c["parentId"] == params.get("parentId")]
    if method == "contacts.quickSearch":
        return store.matching(params.get("searchString", ""), params.get("parentId"))
    if method == "contacts.get":
        return store.contacts[params["contactId"]]
    if method == "contacts.create":
        contact_id = store.add(params["parentId"], params["vCard"])
        emit("contacts.onCreated", {"contact": store.contacts[contact_id]})
        return contact_id
    if method == "contacts.update":
        contact = store.contacts[params["contactId"]]
        contact["vCard"] = params["vCard"]
        contact["properties"]["vCard"] = params["vCard"]
        emit("contacts.onUpdated", {"contact": contact, "changed": {}})
        return None
    if method == "contacts.delete":
        contact = store.contacts.pop(params["contactId"])
        emit("contacts.onDeleted", {"parentId": contact["parentId"], "contactId": contact["id"]})
        return None
    raise KeyError(method)


if __name__ == "__main__":
    main()
