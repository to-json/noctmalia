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

import fixture

DEFAULT_SOCKET = os.path.join(
    os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "noctmalia", "bridge.sock"
)


BOOKS = [
    {"id": book["key"], "name": book["name"], "type": "addressBook",
     "readOnly": book["readOnly"], "remote": book["remote"]}
    for book in fixture.BOOKS
]


class Store:
    def __init__(self, empty):
        self.books = [dict(book) for book in BOOKS]
        self.contacts = {}
        self.ids = itertools.count(1)
        if not empty:
            for parent, card in fixture.CONTACTS:
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
        return store.books
    if method == "addressBooks.create":
        book = {"id": f"book{len(store.books) + 1}", "name": params["name"],
                "type": "addressBook", "readOnly": False, "remote": False}
        store.books.append(book)
        emit("addressBooks.onCreated", {"addressBook": book})
        return book["id"]
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
