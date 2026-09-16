#!/usr/bin/env bash
# End-to-end check of the whole thing against a real mail server, from a fresh profile:
# noctmalia starts its own Thunderbird, the bridge says hello, an account is provisioned, mail
# genuinely arrives (SMTP into GreenMail, headless IMAP fetch, onNewMailReceived), a send goes out
# through Thunderbird's SMTP, Gloda threads and searches, a reply threads, a filter reaches
# msgFilterRules.dat, a calendar event round-trips, and the window's exit takes Thunderbird down
# cleanly. Wipes our Thunderbird profile.
#
# GreenMail is the one container left: a mail server that genuinely delivers, run only here, as a
# fixture. Everything else is native.
#
# The Gloda and filter steps ride Thunderbird internals rather than the WebExtension API
# (docs/mail-plan.md risk 4), so they are the ones to run on every Thunderbird bump.
set -euo pipefail
cd "$(dirname "$0")/.."

profile="${XDG_DATA_HOME:-$HOME/.local/share}/noctmalia/profile"
log="${XDG_STATE_HOME:-$HOME/.local/state}/noctmalia/smoke-noctmalia.log"
greenmail=noctmalia-greenmail

dk() { scripts/with-docker.sh docker "$@"; }
ctl() { tools/noctmalia-ctl.py "$@"; }
py() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
step() { printf '\n== %s\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*" >&2; exit 1; }
seq_now() { ctl status | py 'print(d["seq"])'; }

cleanup() {
  pkill -TERM -x noctmalia >/dev/null 2>&1 || true
  for _ in $(seq 40); do pgrep -x noctmalia >/dev/null || break; sleep 0.25; done
  systemctl --user stop noctmalia-thunderbird.scope >/dev/null 2>&1 || true
  dk rm -f "$greenmail" >/dev/null 2>&1 || true
}
trap cleanup EXIT

inject_mail() {
  python3 - "$1" <<'PY'
import smtplib, sys
from email.message import EmailMessage
m = EmailMessage()
m["From"], m["To"], m["Subject"] = "Alice <alice@example.com>", "j@noctmalia.test", sys.argv[1]
# A Message-ID, because a reply can only thread on one that exists: without it Thunderbird
# synthesises `md5:...` for its own index and writes no In-Reply-To at all.
m["Message-ID"] = "<%s@smoke.example>" % sys.argv[1].replace(" ", "-")
m.set_content("hello from greenmail")
with smtplib.SMTP("127.0.0.1", 3025) as s:
    s.send_message(m)
PY
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

step "GreenMail, on the loopback"
dk rm -f "$greenmail" >/dev/null 2>&1 || true
dk run -d --rm --name "$greenmail" -p 127.0.0.1:3025:3025 -p 127.0.0.1:3143:3143 \
  -e GREENMAIL_OPTS="-Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=$(python3 tools/fixture.py greenmail-users)" \
  greenmail/standalone:2.1.13 >/dev/null
for _ in $(seq 60); do python3 -c 'import socket; socket.create_connection(("127.0.0.1", 3025), 1)' 2>/dev/null && break; sleep 1; done

step "fresh profile, and the window starts its own Thunderbird"
pkill -TERM -x noctmalia >/dev/null 2>&1 || true
for _ in $(seq 40); do pgrep -x noctmalia >/dev/null || break; sleep 0.25; done
systemctl --user stop noctmalia-thunderbird.scope >/dev/null 2>&1 || true
rm -rf "$profile"
mkdir -p "$(dirname "$log")"
scripts/run.sh --dev > "$log" 2>&1 &
for _ in $(seq 600); do ctl status >/dev/null 2>&1 && break; sleep 1; done
ctl status >/dev/null 2>&1 || fail "no control socket after ten minutes (a cold release build?) — see $log"

step "bridge handshake"
ctl wait bridge.hello --after 0 --timeout 300 \
  | py 'h=d["data"]; print("Thunderbird %s, bridge %s, protocol %s, calendar=%s" % (h["browser"]["version"], h["bridgeVersion"], h["protocol"], h["calendar"])); sys.exit(0 if h["calendar"] else 1)' \
  || fail "no bridge.hello or calendar API missing"
ctl status | py 'tb=d["thunderbird"]; print("thunderbird pid %s in %s, connection %s" % (tb["pid"], tb["scope"], d["connection"])); sys.exit(0 if tb["scope"] else 1)' \
  || fail "Thunderbird is not in its scope"

step "provision GreenMail account"
ACCOUNT=$(ctl call dev.provisionAccount "$(python3 -c 'import json, sys; sys.path.insert(0, "tools"); import fixture; print(json.dumps(fixture.ACCOUNTS[0]))')" | py 'print(d["accountId"])')
echo "account $ACCOUNT"
IDENTITY=$(ctl call identities.list "{\"accountId\":\"$ACCOUNT\"}" | py 'print(d[0]["id"])')
echo "identity $IDENTITY"

step "inbound: SMTP into GreenMail → headless IMAP fetch → onNewMailReceived"
SEQ=$(seq_now)
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
SEQ=$(seq_now)
ctl call messages.send "{\"details\":{\"identityId\":\"$IDENTITY\",\"to\":[\"j@noctmalia.test\"],\"subject\":\"smoke outbound\",\"plainTextBody\":\"sent by noctmalia\",\"isPlainText\":true}}" \
  | py 'print("send:", d["mode"], d.get("headerMessageId"))' || fail "messages.send failed"
await_new_mail "$SEQ" "smoke outbound" "$ACCOUNT" >/dev/null || fail "sent message never arrived"
echo "sent message received"

step "flags + events"
SEQ=$(seq_now)
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
}" | py 'print(d["headerMessageId"])') || fail "compose.reply failed"
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
if cat "$profile"/ImapMail/*/msgFilterRules.dat 2>/dev/null | grep -q "smoke rule"; then
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
ctl call calendar.items.query "{\"calendarId\":\"$CAL\",\"expand\":true,\"rangeStart\":\"$NOW\",\"rangeEnd\":\"$(date -u -d '+7 days' +%Y%m%dT%H%M%SZ)\"}" \
  | py 'print("occurrences:", [i["instance"] for i in d]); sys.exit(0 if len(d)==3 else 1)' || fail "expected 3 occurrences"

step "the window closes, and Thunderbird leaves with it"
pkill -TERM -x noctmalia
for _ in $(seq 80); do pgrep -x noctmalia >/dev/null || break; sleep 0.25; done
pgrep -x noctmalia >/dev/null && fail "noctmalia is still running 20s after SIGTERM"
systemctl --user is-active --quiet noctmalia-thunderbird.scope && fail "Thunderbird's scope outlived the window"
[ -e "$profile/lock" ] && fail "the profile is still locked: Thunderbird was killed, not asked"
grep -q "stopping Thunderbird" "$log" || fail "the window did not stop Thunderbird on the way out"
echo "scope gone, profile unlocked"

printf '\nPASS\n'
