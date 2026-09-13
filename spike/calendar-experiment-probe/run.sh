mkdir -p /p/extensions && cd /s/ext && zip -qr /p/extensions/calprobe@noctmalia.xpi .
printf 'user_pref("extensions.autoDisableScopes", 0);\nuser_pref("mail.shell.checkDefaultClient", false);\n' > /p/user.js
python3 /x/tbtest/srv.py &
thunderbird --version
timeout ${TMO:-60} thunderbird --headless --profile /p --no-remote >/tmp/tb.log 2>&1
echo "-- log:"; grep -iE "calend|experiment|error" /tmp/tb.log | grep -v Sandbox | head -15
