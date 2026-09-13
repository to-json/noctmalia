set -e
thunderbird --version
mkdir -p /p/extensions
cd /s/ext && zip -qr /p/extensions/probe@noctmalia.xpi .
cat > /p/user.js <<'P'
user_pref("extensions.autoDisableScopes", 0);
user_pref("extensions.enabledScopes", 15);
user_pref("xpinstall.signatures.required", false);
user_pref("mail.shell.checkDefaultClient", false);
user_pref("mail.accountmanager.accounts", "account1");
user_pref("mail.accountmanager.localfoldersserver", "server1");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.server.server1.type", "none");
user_pref("mail.server.server1.hostname", "Local Folders");
user_pref("mail.server.server1.name", "Local Folders");
user_pref("mail.server.server1.userName", "nobody");
user_pref("mail.server.server1.directory-rel", "[ProfD]Mail/Local Folders");
P
python3 /s/srv.py &
timeout 40 thunderbird --headless --profile /p --no-remote >/tmp/tb.log 2>&1 || true
echo "--- tb log (filtered)"; grep -vi "gtk\|glib\|^$" /tmp/tb.log | head -20
