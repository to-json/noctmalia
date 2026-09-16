#!/usr/bin/env bash
# Put the development fixture into the running window's Thunderbird.
#
#   just dev                   # the window, with the bridge's dev methods on — in another terminal
#   tools/seed.sh              # accounts, contacts, mail
#   tools/seed.sh --reset      # ...replacing what is already there
#   tools/seed.sh --mail       # only the mail corpus
#   tools/seed.sh --flood 500  # ...plus 500 unremarkable messages, for a folder worth windowing
#
# The GreenMail accounts in the fixture need a GreenMail to log in to: `tools/smoke.sh` runs one.
# Without it the accounts are created and sit there failing to connect, which is fine for looking
# at the mail that `messages.import` wrote straight into their folders.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! tools/noctmalia-ctl.py status >/dev/null 2>&1; then
  echo "seed: no running noctmalia. Start one with \`just dev\` first." >&2
  exit 1
fi
if ! tools/noctmalia-ctl.py call bridge.ping >/dev/null 2>&1; then
  echo "seed: the running noctmalia will not pass calls through — it was not started with --dev, or Thunderbird is not attached yet." >&2
  exit 1
fi

echo "== seeding"
# test.secret — an email on one line, a password on the next — is read here and handed down as
# two environment variables rather than a file, so the one place the credential is typed stays
# the one place it is read. Absent on every machine that has not been set up with one.
if [ -f test.secret ]; then
  TEST_ACCOUNT_EMAIL="$(sed -n '1p' test.secret)" TEST_ACCOUNT_PASSWORD="$(sed -n '2p' test.secret)" \
    python3 tools/seed.py "$@"
else
  python3 tools/seed.py "$@"
fi
