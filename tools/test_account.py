#!/usr/bin/env python3
"""Checks that `test.secret`'s account actually authenticates, before blaming anything else.

Doesn't touch Thunderbird or Docker — plain stdlib `imaplib` straight to the provider. Reads
`test.secret` itself (an email on one line, a password on the next; gitignored, read by nothing
else) and never prints the password, only whether login worked.

    tools/test_account.py
"""

import imaplib
import os
import sys

SECRET = os.path.join(os.path.dirname(__file__), "..", "test.secret")


def imap_host(email):
    domain = email.rsplit("@", 1)[-1]
    return "imap.gmail.com" if domain == "gmail.com" else f"imap.{domain}"


def main():
    if not os.path.exists(SECRET):
        print(f"test_account: no {SECRET}", file=sys.stderr)
        return 1
    with open(SECRET) as handle:
        lines = [line.strip() for line in handle if line.strip()]
    if len(lines) < 2:
        print("test_account: test.secret needs an email on line 1 and a password on line 2", file=sys.stderr)
        return 1
    email, password = lines[0], lines[1]
    host = imap_host(email)

    print(f"connecting to {host}:993 as {email} ...")
    try:
        with imaplib.IMAP4_SSL(host, 993, timeout=20) as connection:
            connection.login(email, password)
            status, data = connection.select("INBOX", readonly=True)
            count = data[0].decode() if status == "OK" else "?"
            print(f"login OK — INBOX has {count} messages")
        return 0
    except (imaplib.IMAP4.error, OSError) as error:
        print(f"login FAILED: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
