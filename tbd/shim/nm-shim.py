#!/usr/bin/env python3
"""Stateless relay between the bridge extension and mailnd.

Thunderbird spawns this process when the extension calls runtime.connectNative.
Extension side: native-messaging framing (u32 length + JSON) on stdio.
mailnd side:    newline-delimited JSON over a unix socket; reconnects forever.

Messages pass through untouched. The shim only injects {"shim": "connected"} and
{"shim": "disconnected"} toward the extension so it knows when mailnd attaches.
"""

import json
import os
import socket
import struct
import sys
import threading
import time

SOCKET_PATH = os.environ.get("NOCTMALIA_BRIDGE_SOCKET", "/run/noctmalia/bridge.sock")
STATE_PATH = "/tmp/nm-shim.state"
MAX_TO_EXTENSION = 1024 * 1024  # Gecko's cap on host -> extension messages
RECONNECT_SECONDS = 1.0

stdout_lock = threading.Lock()
mailnd = None  # connected socket or None
mailnd_lock = threading.Lock()


def log(*parts):
    print("nm-shim:", *parts, file=sys.stderr, flush=True)


def write_state(state):
    with open(STATE_PATH + ".tmp", "w") as f:
        f.write(f"{os.getpid()} {state}\n")
    os.replace(STATE_PATH + ".tmp", STATE_PATH)


def to_extension(payload: bytes):
    with stdout_lock:
        sys.stdout.buffer.write(struct.pack("@I", len(payload)) + payload)
        sys.stdout.buffer.flush()


def to_mailnd(payload: bytes):
    with mailnd_lock:
        sock = mailnd
    if sock is None:
        return False
    try:
        sock.sendall(payload + b"\n")
        return True
    except OSError:
        return False


def read_exact(n):
    buf = b""
    while len(buf) < n:
        chunk = sys.stdin.buffer.read(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return buf


def extension_loop():
    dropped = 0
    while True:
        header = read_exact(4)
        if header is None:
            return
        body = read_exact(struct.unpack("@I", header)[0])
        if body is None:
            return
        if not to_mailnd(body):
            dropped += 1
            if dropped == 1 or dropped % 100 == 0:
                log(f"mailnd not connected; dropped {dropped} message(s)")


def mailnd_loop():
    global mailnd
    while True:
        try:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.connect(SOCKET_PATH)
        except OSError:
            sock.close()
            time.sleep(RECONNECT_SECONDS)
            continue

        with mailnd_lock:
            mailnd = sock
        write_state("connected")
        log("connected to", SOCKET_PATH)
        to_extension(b'{"shim":"connected"}')

        try:
            for line in sock.makefile("rb"):
                line = line.rstrip(b"\n")
                if not line:
                    continue
                if len(line) > MAX_TO_EXTENSION:
                    reply_too_large(line)
                    continue
                to_extension(line)
        except OSError:
            pass
        finally:
            with mailnd_lock:
                mailnd = None
            sock.close()
            write_state("waiting")
            log("mailnd disconnected")
            to_extension(b'{"shim":"disconnected"}')


def reply_too_large(line: bytes):
    try:
        request_id = json.loads(line).get("id")
    except ValueError:
        request_id = None
    error = {"id": request_id, "error": {"name": "ShimError", "message": f"request exceeds {MAX_TO_EXTENSION} bytes"}}
    to_mailnd(json.dumps(error).encode())


def main():
    write_state("waiting")
    threading.Thread(target=mailnd_loop, daemon=True).start()
    extension_loop()  # returns when Thunderbird closes the port
    return 0


if __name__ == "__main__":
    sys.exit(main())
