#!/usr/bin/env bash
# End-to-end check of tbd against GreenMail, run from a fresh stack:
# bridge handshake, account provisioning, headless IMAP fetch and onNewMailReceived,
# SMTP send through Thunderbird, calendar round trip.
# Wipes the compose volumes (the Thunderbird profile included).
set -euo pipefail
cd "$(dirname "$0")/.."

export TBD_EXTRA_PREFS='user_pref("extensions.noctmalia.dev", true);'

dc() { docker compose --profile dev "$@"; }
ctl() { dc exec -T mailnd python /tools/bridgectl.py "$@"; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
step() { printf '\n== %s\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*" >&2; exit 1; }

inject_mail() {
  dc exec -T mailnd python - "$1" <<'EOF'
import smtplib, sys
from email.message import EmailMessage
m = EmailMessage()
m["From"], m["To"], m["Subject"] = "Alice <alice@example.com>", "j@noctmalia.test", sys.argv[1]
m.set_content("hello from greenmail")
with smtplib.SMTP("greenmail", 3025) as s:
    s.send_message(m)
EOF
}

# Checks mail until an onNewMailReceived newer than $1 carries subject $2; prints the event.
await_new_mail() {
  local after=$1 subject=$2 account=$3 event
  for _ in $(seq 1 12); do
    ctl call mail.checkNow "{\"accountId\":\"$account\"}" >/dev/null
    if event=$(ctl wait messages.onNewMailReceived --after "$after" --timeout 15 2>/dev/null); then
      if echo "$event" | py "sys.exit(0 if any(m['subject']=='$subject' for m in d['data']['messages']) else 1)"; then
        echo "$event"
        return 0
      fi
      after=$(echo "$event" | py 'print(d["seq"])')
    fi
  done
  return 1
}

step "fresh stack"
dc down -v --remove-orphans >/dev/null 2>&1 || true
dc up -d --build --wait --wait-timeout 400

step "bridge handshake"
ctl wait bridge.hello --timeout 300 \
  | py 'h=d["data"]; print("Thunderbird %s, bridge %s, protocol %s, calendar=%s" % (h["browser"]["version"], h["bridgeVersion"], h["protocol"], h["calendar"])); sys.exit(0 if h["calendar"] else 1)' \
  || fail "no bridge.hello or calendar API missing"

step "provision GreenMail account"
ACCOUNT=$(ctl call dev.provisionAccount '{
  "name": "greenmail", "email": "j@noctmalia.test", "fullName": "J",
  "imap": {"host": "greenmail", "port": 3143, "socketType": "plain", "username": "j", "password": "secret"},
  "smtp": {"host": "greenmail", "port": 3025, "socketType": "plain", "auth": "none"}
}' | py 'print(d["accountId"])')
echo "account $ACCOUNT"
IDENTITY=$(ctl call identities.list "{\"accountId\":\"$ACCOUNT\"}" | py 'print(d[0]["id"])')
echo "identity $IDENTITY"

step "inbound: SMTP into GreenMail → headless IMAP fetch → onNewMailReceived"
SEQ=$(ctl status | py 'print(d["seq"])')
inject_mail "smoke inbound"
EVENT=$(await_new_mail "$SEQ" "smoke inbound" "$ACCOUNT") || fail "no onNewMailReceived for inbound mail"
INBOX=$(echo "$EVENT" | py 'print(d["data"]["folder"]["id"])')
MESSAGE=$(echo "$EVENT" | py 'print(next(m["id"] for m in d["data"]["messages"] if m["subject"]=="smoke inbound"))')
echo "inbox $INBOX, message $MESSAGE"
ctl call messages.getFull "{\"messageId\":$MESSAGE}" \
  | py 'body=d["parts"][0].get("body") or d["parts"][0]["parts"][0]["body"]; print("body:", body.strip()); sys.exit(0 if "hello from greenmail" in body else 1)' \
  || fail "getFull body mismatch"
ctl call messages.list "{\"folderId\":\"$INBOX\"}" | py 'print("inbox subjects:", [m["subject"] for m in d["messages"]])'

step "outbound: messages.send via Thunderbird SMTP → back into inbox"
SEQ=$(ctl status | py 'print(d["seq"])')
ctl call messages.send "{\"details\":{\"identityId\":\"$IDENTITY\",\"to\":[\"j@noctmalia.test\"],\"subject\":\"smoke outbound\",\"plainTextBody\":\"sent by tbd\",\"isPlainText\":true}}" \
  | py 'print("send:", d["mode"], d.get("headerMessageId"))' || fail "messages.send failed"
await_new_mail "$SEQ" "smoke outbound" "$ACCOUNT" >/dev/null || fail "sent message never arrived"
echo "sent message received"

step "flags + events"
SEQ=$(ctl status | py 'print(d["seq"])')
ctl call messages.update "{\"messageIds\":[$MESSAGE],\"properties\":{\"read\":true,\"flagged\":true}}" >/dev/null
ctl wait messages.onUpdated --after "$SEQ" --timeout 30 | py 'print("onUpdated:", d["data"]["changed"])' || fail "no onUpdated"

step "calendar round trip"
CAL=$(ctl call calendar.calendars.create '{"type":"storage","url":"moz-storage-calendar://","name":"smoke"}' | py 'print(d["id"])')
NOW=$(date -u +%Y%m%dT%H%M%SZ)
ctl call calendar.items.create "{\"calendarId\":\"$CAL\",\"id\":\"smoke-1\",\"type\":\"event\",\"format\":\"ical\",\"item\":\"BEGIN:VCALENDAR\\r\\nVERSION:2.0\\r\\nPRODID:-//noctmalia//smoke//EN\\r\\nBEGIN:VEVENT\\r\\nUID:smoke-1\\r\\nSUMMARY:standup\\r\\nDTSTART:$NOW\\r\\nDURATION:PT15M\\r\\nRRULE:FREQ=DAILY;COUNT=3\\r\\nEND:VEVENT\\r\\nEND:VCALENDAR\\r\\n\"}" >/dev/null
ctl call calendar.items.query "{\"calendarId\":\"$CAL\",\"expand\":true,\"rangeStart\":\"$NOW\",\"rangeEnd\":\"$(date -u -v+7d +%Y%m%dT%H%M%SZ 2>/dev/null || date -u -d '+7 days' +%Y%m%dT%H%M%SZ)\"}" \
  | py 'print("occurrences:", [i["instance"] for i in d]); sys.exit(0 if len(d)==3 else 1)' || fail "expected 3 occurrences"

printf '\nPASS\n'
