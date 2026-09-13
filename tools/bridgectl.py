#!/usr/bin/env python3
"""CLI for mailnd-stub's control socket.

  bridgectl.py status
  bridgectl.py call METHOD [JSON_PARAMS] [--timeout S]
  bridgectl.py wait EVENT [--after SEQ] [--timeout S]
"""

import argparse
import json
import os
import socket
import sys

CTL_SOCK = os.path.join(os.environ.get("NOCTMALIA_RUN", "/run/noctmalia"), "ctl.sock")


def request(payload):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.connect(CTL_SOCK)
        sock.sendall((json.dumps(payload) + "\n").encode())
        return json.loads(sock.makefile("rb").readline())


def main():
    parser = argparse.ArgumentParser(prog="bridgectl")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("status")
    call = sub.add_parser("call")
    call.add_argument("method")
    call.add_argument("params", nargs="?", default="{}")
    call.add_argument("--timeout", type=float, default=60)
    wait = sub.add_parser("wait")
    wait.add_argument("event")
    wait.add_argument("--after", type=int, default=0)
    wait.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()

    if args.command == "status":
        reply = request({"op": "status"})
    elif args.command == "call":
        reply = request({"op": "call", "method": args.method, "params": json.loads(args.params), "timeout": args.timeout})
    else:
        reply = request({"op": "wait", "event": args.event, "after": args.after, "timeout": args.timeout})

    if "error" in reply:
        print(json.dumps(reply["error"], indent=2), file=sys.stderr)
        return 1
    print(json.dumps(reply["result"], indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
