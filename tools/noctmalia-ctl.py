#!/usr/bin/env python3
"""A thin CLI over noctmalia's control socket — `docs/scripting-socket-plan.md`.

The socket does the real work; this just speaks its NDJSON, one line in, one line out, the same
`{id, method, params}` shape `noctmalia-bridge`'s own protocol uses. `run` is fire-and-forget: a
"queued" reply means noctmalia accepted it, not that it finished.

    noctmalia-ctl.py list
    noctmalia-ctl.py run "Refresh"
"""

import argparse
import json
import os
import socket
import sys


def socket_path():
    run = os.environ.get("XDG_RUNTIME_DIR", "/tmp")
    return os.path.join(run, "noctmalia", "control.sock")


def request(method, params=None):
    payload = {"id": 1, "method": method}
    if params is not None:
        payload["params"] = params
    path = socket_path()
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.connect(path)
            sock.sendall((json.dumps(payload) + "\n").encode())
            line = sock.makefile("rb").readline()
    except (ConnectionRefusedError, FileNotFoundError):
        print(f"noctmalia-ctl: no control socket at {path} — is noctmalia running?", file=sys.stderr)
        sys.exit(1)
    return json.loads(line)


def main():
    parser = argparse.ArgumentParser(prog="noctmalia-ctl")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list", help="every command this noctmalia has exposed, by name")
    run = sub.add_parser("run", help="run one, by the name `list` gave it")
    run.add_argument("name")
    args = parser.parse_args()

    if args.command == "list":
        reply = request("list")
    else:
        reply = request("run", {"name": args.name})

    if reply.get("error"):
        print(f"noctmalia-ctl: {reply['error']}", file=sys.stderr)
        return 1
    print(json.dumps(reply["result"], indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
