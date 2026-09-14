#!/usr/bin/env bash
# End-to-end check of tbd against GreenMail, run from a fresh stack:
# bridge handshake, account provisioning, headless IMAP fetch and onNewMailReceived,
# SMTP send through Thunderbird, Gloda threading and search, a reply that threads,
# a message filter, and a calendar round trip.
# Wipes the compose volumes (the Thunderbird profile included).
#
# The Gloda and filter steps ride Thunderbird internals rather than the WebExtension API
# (docs/mail-plan.md risk 4), so they are the ones to run on every Thunderbird bump.
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
# A Message-ID, because a reply can only thread on one that exists: without it Thunderbird
# synthesises `md5:...` for its own index and writes no In-Reply-To at all.
m["Message-ID"] = "<%s@smoke.example>" % sys.argv[1].replace(" ", "-")
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

step "Gloda: the conversation Thunderbird put it in"
MSGID=$(ctl call messages.get "{\"messageId\":$MESSAGE}" | py 'print(d["headerMessageId"])')
echo "headerMessageId $MSGID"
# Gloda indexes asynchronously — a message is listable before it is searchable.
CONVERSATION=""
for _ in $(seq 1 20); do
  CONVERSATION=$(ctl call gloda.conversations "{\"headerMessageIds\":[\"$MSGID\"]}" 2>/dev/null || echo "[]")
  if echo "$CONVERSATION" | py 'sys.exit(0 if d else 1)'; then break; fi
  sleep 3
done
echo "$CONVERSATION" | py 'sys.exit(0 if d and d[0]["messages"] else 1)' \
  || fail "Gloda never put the message in a conversation — its schema may have moved"
echo "$CONVERSATION" | py 'print("conversation %s: %s" % (d[0]["id"], d[0]["subject"]))'

step "Gloda: ranked full-text search over the body"
FOUND=""
for _ in $(seq 1 10); do
  FOUND=$(ctl call gloda.search '{"query":"greenmail","limit":10}' 2>/dev/null || echo "[]")
  if echo "$FOUND" | py 'sys.exit(0 if d else 1)'; then break; fi
  sleep 3
done
echo "$FOUND" | py 'print("search hits:", [m["subject"] for m in d]); sys.exit(0 if d else 1)' \
  || fail "Gloda full-text search returned nothing for a word in the body"

step "reply that threads"
# Thunderbird's message database strips the `Re:` off a subject and keeps it as a flag, so the
# reply does not arrive under the subject it was sent with. Follow its Message-ID instead.
REPLY_ID=$(ctl call compose.reply "{
  \"messageId\": $MESSAGE, \"type\": \"replyToSender\", \"mode\": \"sendNow\",
  \"details\": {\"identityId\": \"$IDENTITY\", \"to\": [\"j@noctmalia.test\"],
                \"subject\": \"Re: smoke inbound\",
                \"plainTextBody\": \"a **markdown** reply\", \"isPlainText\": true}
}" --timeout 180 | py 'print(d["headerMessageId"])') || fail "compose.reply failed"
echo "reply $REPLY_ID"

SENT=$(ctl call messages.query "{\"headerMessageId\":\"$REPLY_ID\",\"autoPaginationTimeout\":0}" \
  | py 'print(d["messages"][0]["id"] if d["messages"] else "")')
[ -n "$SENT" ] || fail "the sent copy is nowhere"
ctl call messages.getFull "{\"messageId\":$SENT}" | python3 -c '
import json, sys
message = json.load(sys.stdin)
headers = {name.lower(): value for name, value in (message.get("headers") or {}).items()}
threading = " ".join(headers.get("in-reply-to", []) + headers.get("references", []))
# The root part of getFull is the message/rfc822 wrapper; the body is the part under it.
body = (message.get("parts") or [{}])[0]
print("threads on:", threading or "(nothing)")
print("sent as:", body.get("contentType"))
print("body:", (body.get("body") or "").strip()[:60])
sys.exit(0 if sys.argv[1] in threading and "text/plain" in (body.get("contentType") or "") else 1)
' "$MSGID" || fail "the reply does not thread, or did not go out as plain text"

for _ in $(seq 1 12); do
  ctl call mail.checkNow "{\"accountId\":\"$ACCOUNT\"}" >/dev/null
  ARRIVED=$(ctl call messages.query "{\"headerMessageId\":\"$REPLY_ID\",\"autoPaginationTimeout\":0}" \
    | py 'print(" ".join(m["folder"]["path"] for m in d["messages"]))')
  case "$ARRIVED" in *INBOX*) break ;; esac
  sleep 5
done
case "$ARRIVED" in
  *INBOX*) echo "the reply came back round: $ARRIVED" ;;
  *) fail "the reply never arrived (found in: ${ARRIVED:-nowhere})" ;;
esac

step "a rule Thunderbird keeps"
ROOT=$(ctl call accounts.get "{\"accountId\":\"$ACCOUNT\",\"includeSubFolders\":true}" | py 'print(d["folders"][0]["id"])')
# `accounts.get(...).folders` on an IMAP account has no unnamed root: it is [Inbox, Trash, ...],
# so a folder created under folders[0] lands inside the Inbox. Take the path it reports.
MADE=$(ctl call folders.create "{\"parentId\":\"$ROOT\",\"name\":\"Screened\"}")
SCREENED=$(echo "$MADE" | py 'print(d["id"] if isinstance(d, dict) else d)')
SCREENED_PATH=$(echo "$MADE" | py 'print(d.get("path", "") if isinstance(d, dict) else "")')
echo "screened $SCREENED at $SCREENED_PATH"
ctl call filters.create "{\"accountId\":\"$ACCOUNT\",\"name\":\"smoke rule\",\"header\":\"from\",
  \"value\":\"alice@example.com\",\"folderId\":\"$SCREENED\",\"folderPath\":\"$SCREENED_PATH\"}" \
  | py 'print("rule:", d["name"])' || fail "filters.create failed"
ctl call filters.list "{\"accountId\":\"$ACCOUNT\"}" \
  | py 'print("rules:", [(r["name"], r["summary"]) for r in d]); sys.exit(0 if any(r["name"]=="smoke rule" for r in d) else 1)' \
  || fail "the rule is not in the filter list"
if dc exec -T tbd sh -c 'cat /data/profile/ImapMail/*/msgFilterRules.dat 2>/dev/null' | grep -q "smoke rule"; then
  echo "and it is in msgFilterRules.dat, which is where it lives"
else
  fail "the rule is not in msgFilterRules.dat"
fi

step "and the rule files the next message that matches"
inject_mail "smoke screened"
FILED=""
for _ in $(seq 1 12); do
  ctl call mail.checkNow "{\"accountId\":\"$ACCOUNT\"}" >/dev/null
  FILED=$(ctl call messages.query '{"subject":"smoke screened","autoPaginationTimeout":0}' \
    | py 'print(" ".join(m["folder"]["path"] for m in d["messages"]))')
  case "$FILED" in *Screened*) break ;; esac
  sleep 5
done
case "$FILED" in
  *Screened*) echo "filed into $FILED without anybody touching it" ;;
  *) fail "the rule did not file the message (it is in: ${FILED:-nowhere})" ;;
esac

step "calendar round trip"
CAL=$(ctl call calendar.calendars.create '{"type":"storage","url":"moz-storage-calendar://","name":"smoke"}' | py 'print(d["id"])')
NOW=$(date -u +%Y%m%dT%H%M%SZ)
ctl call calendar.items.create "{\"calendarId\":\"$CAL\",\"id\":\"smoke-1\",\"type\":\"event\",\"format\":\"ical\",\"item\":\"BEGIN:VCALENDAR\\r\\nVERSION:2.0\\r\\nPRODID:-//noctmalia//smoke//EN\\r\\nBEGIN:VEVENT\\r\\nUID:smoke-1\\r\\nSUMMARY:standup\\r\\nDTSTART:$NOW\\r\\nDURATION:PT15M\\r\\nRRULE:FREQ=DAILY;COUNT=3\\r\\nEND:VEVENT\\r\\nEND:VCALENDAR\\r\\n\"}" >/dev/null
ctl call calendar.items.query "{\"calendarId\":\"$CAL\",\"expand\":true,\"rangeStart\":\"$NOW\",\"rangeEnd\":\"$(date -u -v+7d +%Y%m%dT%H%M%SZ 2>/dev/null || date -u -d '+7 days' +%Y%m%dT%H%M%SZ)\"}" \
  | py 'print("occurrences:", [i["instance"] for i in d]); sys.exit(0 if len(d)==3 else 1)' || fail "expected 3 occurrences"

printf '\nPASS\n'
