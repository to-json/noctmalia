#!/usr/bin/env bash
# (Re-)establish Gmail OAuth consent — the one deliberate, human step this needs.
#
# Opens Thunderbird's own account-setup window inside the noctmalia that is already running,
# walks you through Google's real sign-in and consent screen, and leaves the account in
# noctmalia's own profile. There is nothing to copy out afterwards and no separate credential
# file: the profile itself is the credential store (logins.json/key4.db), and it survives
# ordinary restarts — `docs/oauth-plan.md` Stream 1 confirmed both that Thunderbird's own
# baked-in Gmail OAuth client needs no Google Cloud project of ours, and that the login keeps
# working after the container/process behind it restarts. Run this once per profile, or again
# after `just reset` throws the profile away.
#
#   just                        # noctmalia, in one terminal — must already be running
#   tools/oauth_bootstrap.sh
#
# This goes through Thunderbird's own account wizard, which sets up SMTP with OAuth2 too — unlike
# the scripted `dev.provisionAccount` path `tools/seed.py` uses for the throwaway test account,
# which deliberately stays IMAP-only (`docs/oauth-plan.md` Stream 2.3). An account added this way
# should read and send.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! tools/noctmalia-ctl.py status >/dev/null 2>&1; then
  echo "oauth_bootstrap: no running noctmalia. Start one with \`just\` first." >&2
  exit 1
fi

echo "== opening Thunderbird settings"
tools/noctmalia-ctl.py run "Open Thunderbird settings" >/dev/null

cat <<'EOF'

Thunderbird is switching to a real window. In it:
  1. Start adding a new mail account and enter the Gmail address.
  2. When it asks how to sign in, choose OAuth2 (or "Sign in with Google").
  3. Complete Google's real sign-in and consent screen.
  4. Once Thunderbird reports the account is set up, close that window.

noctmalia goes back to its normal headless self on its own as soon as the window closes — nothing
else to run.
EOF
