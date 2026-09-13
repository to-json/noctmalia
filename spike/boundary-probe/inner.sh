#!/bin/sh
# Runs inside a throwaway container from the tbd image: fresh profile, spike add-on only.
P=/tmp/spike-profile
mkdir -p "$P/extensions"
cp /spike/spike.xpi "$P/extensions/spike@noctmalia.xpi"
cp /opt/noctmalia/user.js "$P/user.js"

python3 /spike/srv2.py &
srv=$!
/opt/thunderbird/thunderbird --headless --profile "$P" --no-remote > /tmp/tb.log 2>&1 &
tb=$!

wait "$srv"
status=$?
kill "$tb" 2>/dev/null
if [ "$status" -ne 0 ]; then
  echo "--- thunderbird log (filtered)"
  grep -vE 'gtk_icon_theme|Gtk-CRITICAL|^[[:space:]]*$|GFX|D-Bus' /tmp/tb.log | tail -30
fi
exit "$status"
