#!/usr/bin/env python3
"""A Thunderbird stand-in for developing the UI.

Connects to noctmalia's socket the way tbd's nm-shim does, says hello, and serves the contacts and
mail methods out of memory. It exists so the front end can be run and tested without Docker, a
profile or a mail account; it is not a protocol conformance test. Everything it does not implement
comes back as MethodNotFound, exactly as the real bridge would.

The mail it serves is `fixture.messages()`, which is the same corpus `seed.py` writes into a real
profile — so the same deliberately nasty mail is on screen either way. It pages `messages.list` in
small batches on purpose: the windowed index and the progressive load are the two things a
stand-in that handed over everything at once would never exercise.

    tools/fake-bridge.py [--socket PATH] [--empty]
"""

import argparse
import base64
import itertools
import json
import os
import socket
import sys
import time

import fixture

# Small enough that `messages.list` always needs continuing, which is the path that matters.
PAGE = 5

DEFAULT_SOCKET = os.path.join(
    os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "noctmalia", "bridge.sock"
)


BOOKS = [
    {"id": book["key"], "name": book["name"], "type": "addressBook",
     "readOnly": book["readOnly"], "remote": book["remote"]}
    for book in fixture.BOOKS
]


ACCOUNT = "account1"


def folder_id(name):
    return f"{ACCOUNT}://{name}"


def folder_node(name, special=None):
    return {
        "id": folder_id(name),
        "name": name.capitalize() if special else name,
        "path": f"/{name}",
        "accountId": ACCOUNT,
        "specialUse": [special] if special else [],
        "type": special,
        "subFolders": [],
        "isFavorite": False,
    }


SPECIAL = {"inbox": "inbox", "archives": "archives", "sent": "sent", "junk": "junk", "trash": "trash"}


# The markers Thunderbird takes off a subject before storing it.
PREFIXES = ("re:", "aw:", "fwd:", "fw:", "vs:", "sv:", "antw:")


def strip_prefix(subject):
    rest = subject.strip()
    while True:
        lowered = rest.lower()
        marker = next((p for p in PREFIXES if lowered.startswith(p)), None)
        if not marker:
            return rest
        rest = rest[len(marker):].lstrip()


def part_tree(message):
    """The fixture's body spec as a `messages.getFull` tree."""
    kind = message["body"]
    headers = {
        "from": [message["from"]],
        "to": message["to"],
        "subject": [message["subject"]],
        "date": [message["date"]],
    }
    for header in ("reply-to", "list-id", "list-unsubscribe", "authentication-results"):
        if message.get(header):
            headers[header] = [message[header]]
    if message.get("message-id"):
        headers["message-id"] = [message["message-id"]]
    if message.get("in-reply-to"):
        headers["in-reply-to"] = [message["in-reply-to"]]
    if message.get("received"):
        headers["received"] = list(message["received"])

    def leaf(content_type, body, part, name=None):
        node = {"contentType": content_type, "partName": part, "body": body, "size": len(body)}
        if name:
            node["name"] = name
        return node

    flowed = "; format=flowed" if message.get("flowed") else ""
    if kind[0] == "plain":
        root = leaf(f"text/plain; charset=UTF-8{flowed}", kind[1], "1")
    elif kind[0] == "markdown":
        root = leaf("text/markdown; charset=UTF-8", kind[1], "1")
    elif kind[0] == "html":
        root = leaf("text/html; charset=UTF-8", kind[1], "1")
    elif kind[0] == "alternative":
        root = {"contentType": "multipart/alternative", "partName": "", "parts": [
            leaf("text/plain; charset=UTF-8", kind[1], "1"),
            leaf("text/html; charset=UTF-8", kind[2], "2"),
        ]}
    elif kind[0] == "attached":
        parts = [leaf("text/plain; charset=UTF-8", kind[1], "1")]
        for index, (name, content_type, payload) in enumerate(kind[2], start=2):
            parts.append(leaf(content_type, payload, str(index), name))
        root = {"contentType": "multipart/mixed", "partName": "", "parts": parts}
    else:
        raise ValueError(kind[0])
    root["headers"] = headers
    return root


class Store:
    def __init__(self, empty):
        self.books = [dict(book) for book in BOOKS]
        self.contacts = {}
        self.ids = itertools.count(1)
        self.listings = {}
        self.list_ids = itertools.count(1)
        self.filters = []
        self.messages = {}
        self.folders = {}
        if not empty:
            for parent, card in fixture.CONTACTS:
                self.add(parent, card)
            self.load_mail()

    # ── Mail ────────────────────────────────────────────────────────────────
    def load_mail(self):
        for name in fixture.FOLDERS:
            self.folders[folder_id(name)] = folder_node(name, SPECIAL.get(name))
        for message in fixture.messages():
            where = folder_id(message["folder"])
            self.messages[message["index"]] = {
                "id": message["index"],
                "headerMessageId": (message.get("message-id") or f"<none-{message['index']}@local>")
                    .strip("<>"),
                # Thunderbird's message database strips a `Re:`/`Fwd:` prefix off the subject and
                # keeps it as a flag, so a `MessageHeader` never carries one. Do the same, or the
                # stand-in shows an index that a real profile never would.
                "subject": strip_prefix(message["subject"]),
                "author": message["from"],
                "recipients": message["to"],
                "date": message["date"],
                "read": message["read"],
                "flagged": message["flagged"],
                "junk": message["junk"],
                "new": not message["read"],
                "tags": [],
                "size": len(fixture.eml(message)),
                "folder": {"id": where, "name": self.folders[where]["name"]},
                "_source": message,
            }

    def header(self, message):
        return {key: value for key, value in message.items() if not key.startswith("_")}

    def in_folder(self, folder):
        found = [m for m in self.messages.values() if m["folder"]["id"] == folder]
        # Natural order, oldest first, exactly as a real folder hands them over.
        return sorted(found, key=lambda m: m["date"])

    def open_listing(self, items):
        rest = items[PAGE:]
        if not rest:
            return {"id": None, "messages": [self.header(m) for m in items]}
        list_id = f"list{next(self.list_ids)}"
        self.listings[list_id] = rest
        return {"id": list_id, "messages": [self.header(m) for m in items[:PAGE]]}

    def continue_listing(self, list_id):
        rest = self.listings.pop(list_id, [])
        return self.open_listing(rest)

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

    # ── Mail ────────────────────────────────────────────────────────────────
    if method == "accounts.list":
        root = folder_node("", None)
        root["name"] = ""
        root["subFolders"] = [store.folders[folder_id(name)] for name in fixture.FOLDERS]
        return [{"id": ACCOUNT, "name": "greenmail", "type": "imap", "folders": [root]}]
    if method == "identities.list":
        return [{"id": "id1", "email": "j@noctmalia.test", "name": "J", "accountId": ACCOUNT}]
    if method == "folders.getFolderInfo":
        held = store.in_folder(params["folderId"])
        return {"totalMessageCount": len(held),
                "unreadMessageCount": sum(1 for m in held if not m["read"]),
                "newMessageCount": 0}
    if method == "messages.list":
        return store.open_listing(store.in_folder(params["folderId"]))
    if method == "messages.continueList":
        return store.continue_listing(params["listId"])
    if method == "messages.abortList":
        store.listings.pop(params.get("listId"), None)
        return None
    if method == "messages.query":
        found = list(store.messages.values())
        if params.get("folderId"):
            found = [m for m in found if m["folder"]["id"] == params["folderId"]]
        if params.get("headerMessageId"):
            found = [m for m in found if m["headerMessageId"] == params["headerMessageId"]]
        if params.get("author"):
            found = [m for m in found if params["author"].lower() in m["author"].lower()]
        if params.get("subject"):
            found = [m for m in found if params["subject"].lower() in m["subject"].lower()]
        return {"id": None, "messages": [store.header(m) for m in found]}
    if method == "messages.get":
        return store.header(store.messages[params["messageId"]])
    if method == "messages.getFull":
        return part_tree(store.messages[params["messageId"]]["_source"])
    if method == "messages.getRaw":
        raw = fixture.eml(store.messages[params["messageId"]]["_source"])
        return {"base64": base64.b64encode(raw.encode("utf-8", "surrogateescape")).decode()}
    if method == "messages.getAttachment":
        source = store.messages[params["messageId"]]["_source"]
        index = int(params["partName"]) - 2
        _, _, payload = source["body"][2][index]
        return {"base64": base64.b64encode(payload.encode("utf-8", "surrogateescape")).decode()}
    if method == "messages.update":
        for message_id in params["messageIds"]:
            store.messages[message_id].update(params["properties"])
            emit("messages.onUpdated", {"message": store.header(store.messages[message_id]),
                                        "changed": params["properties"]})
        return None
    if method in ("messages.move", "messages.archive", "messages.delete"):
        target = params.get("folderId") or folder_id("archives" if method.endswith("archive") else "trash")
        for message_id in params["messageIds"]:
            message = store.messages[message_id]
            message["folder"] = {"id": target, "name": store.folders[target]["name"]}
        emit("messages.onMoved", {"from": {"messages": []}, "to": {"messages": []}})
        return None
    if method == "gloda.conversations":
        # The stand-in threads on the fixture's own `thread` marker, which is what Gloda would have
        # worked out from References — the point is to exercise the collapsing, not to reimplement
        # a threader that Thunderbird already has.
        groups = {}
        wanted = set(params.get("headerMessageIds") or [])
        for message in store.messages.values():
            thread = message["_source"].get("thread")
            if not thread or message["headerMessageId"] not in wanted:
                continue
            groups.setdefault(thread, []).append(message["headerMessageId"])
        return [{"id": index, "subject": name, "messages": ids}
                for index, (name, ids) in enumerate(sorted(groups.items()), start=1)]
    if method == "gloda.search":
        needle = params.get("query", "").lower()
        found = [store.header(m) for m in store.messages.values()
                 if needle in json.dumps(m["_source"], default=str).lower()]
        return found[: params.get("limit") or 200]
    if method in ("compose.begin", "compose.reply"):
        details = params.get("details") or {}
        return {"mode": params.get("mode", "sendNow"), "headerMessageId": "<fake-sent@noctmalia.test>",
                "messages": [], "to": details.get("to", [])}
    if method == "filters.list":
        return store.filters
    if method == "filters.create":
        rule = {"name": params["name"], "enabled": True,
                "summary": f"{params['header']} contains {params['value']}"}
        store.filters.insert(0, rule)
        return {"name": rule["name"]}

    # ── Contacts ────────────────────────────────────────────────────────────
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
