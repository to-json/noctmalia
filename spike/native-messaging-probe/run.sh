mkdir -p /p/extensions ~/.mozilla/native-messaging-hosts ~/.thunderbird/native-messaging-hosts
cd /s/ext && zip -qr /p/extensions/nm@noctmalia.xpi .
for d in ~/.mozilla/native-messaging-hosts; do
  echo '{"name":"noctmalia.probe","description":"p","path":"/s/host.py","type":"stdio","allowed_extensions":["nm@noctmalia"]}' > $d/noctmalia.probe.json
done
printf 'user_pref("extensions.autoDisableScopes", 0);\nuser_pref("mail.shell.checkDefaultClient", false);\n' > /p/user.js
python3 /x/tbtest/srv.py &
timeout 25 thunderbird --headless --profile /p --no-remote >/tmp/tb.log 2>&1
grep -i "native\|host.py" /tmp/tb.log | head
