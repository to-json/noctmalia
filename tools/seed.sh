#!/usr/bin/env bash
# Boot a real headless Thunderbird and put the development fixture in it.
#
#   tools/seed.sh              # bring the stack up, wait for the bridge, seed accounts, contacts, mail
#   tools/seed.sh --reset      # ...replacing what is already there
#   tools/seed.sh --mail       # only the mail corpus
#   tools/seed.sh --flood 500  # ...plus 500 unremarkable messages, for a folder worth windowing
#
# Keeps the profile: run it as often as you like. To wipe and start over, `docker compose down -v`.
#
# Afterwards, to look at the result in the UI — only one client may hold the bridge socket, and
# this used the stub, so hand it over:
#
#   docker compose -f compose.yaml -f compose.ui.yaml up -d tbd
#   scripts/run.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# dev.provisionAccount is gated on this pref, and GreenMail needs somewhere for our users to log in.
export TBD_EXTRA_PREFS="${TBD_EXTRA_PREFS:-}user_pref(\"extensions.noctmalia.dev\", true);"
export GREENMAIL_USERS="$(python3 tools/fixture.py greenmail-users)"

echo "== stack (Thunderbird takes about a minute to start, longer on first build)"
docker compose --profile dev up -d --build --wait --wait-timeout 400

echo "== waiting for the bridge"
docker compose exec -T mailnd python /tools/bridgectl.py wait bridge.hello --timeout 300 \
  | python3 -c 'import json,sys; h=json.load(sys.stdin)["data"]; print("Thunderbird %s, bridge %s" % (h["browser"]["version"], h["bridgeVersion"]))'

echo "== seeding"
docker compose exec -T mailnd python /tools/seed.py "$@"
