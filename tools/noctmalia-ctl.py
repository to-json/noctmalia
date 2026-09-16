#!/usr/bin/env python3
"""A thin CLI over noctmalia's control socket — `docs/scripting-socket-plan.md`.

The socket does the real work; this just speaks its NDJSON, one line in, one line out, the same
`{id, method, params}` shape `noctmalia-bridge`'s own protocol uses. `run` is fire-and-forget: a
"queued" reply means noctmalia accepted it, not that it finished.

    noctmalia-ctl.py list
    noctmalia-ctl.py run "Refresh"
    noctmalia-ctl.py status      # is Thunderbird attached, and is anything stuck
    noctmalia-ctl.py call messages.list '{"folderId": "..."}'   # any bridge method; needs `noctmalia --dev`
    noctmalia-ctl.py wait messages.onUpdated --after 12        # the next such event past sequence 12
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
    sub.add_parser("status", help="the bridge: which connection is attached, calls in flight, the slowest call")
    call = sub.add_parser("call", help="one bridge method, straight through; needs `noctmalia --dev`")
    call.add_argument("method")
    call.add_argument("params", nargs="?", default="{}")
    wait = sub.add_parser("wait", help="the next bridge event of this name past a sequence number; needs `noctmalia --dev`")
    wait.add_argument("event")
    wait.add_argument("--after", type=int, default=0, help="a `seq` from `status` or an earlier wait")
    wait.add_argument("--timeout", type=float, default=60)
    run = sub.add_parser("run", help="run one, by the name `list` gave it")
    run.add_argument("name")
    args = parser.parse_args()

    if args.command in ("list", "status"):
        reply = request(args.command)
    elif args.command == "call":
        reply = request("call", {"method": args.method, "params": json.loads(args.params)})
    elif args.command == "wait":
        reply = request("wait", {"event": args.event, "after": args.after, "timeout": args.timeout})
    else:
        reply = request("run", {"name": args.name})

    if reply.get("error"):
        detail = reply.get("result", {}).get("error") if isinstance(reply.get("result"), dict) else None
        print(f"noctmalia-ctl: {reply['error']}" + (f": {detail}" if detail else ""), file=sys.stderr)
        return 1
    print(json.dumps(reply["result"], indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
