#!/bin/sh
set -eu

PROFILE=/data/profile
NM_HOSTS="$HOME/.mozilla/native-messaging-hosts"

mkdir -p "$PROFILE/extensions" "$NM_HOSTS"

# Managed prefs are rewritten on every start; TBD_EXTRA_PREFS appends raw user_pref() lines.
cp /opt/noctmalia/user.js "$PROFILE/user.js"
if [ -n "${TBD_EXTRA_PREFS:-}" ]; then
  printf '%s\n' "$TBD_EXTRA_PREFS" >> "$PROFILE/user.js"
fi

# Sideload the bridge; a fresh copy each start lets Thunderbird pick up image upgrades.
cp /opt/noctmalia/noctmalia-bridge.xpi "$PROFILE/extensions/bridge@noctmalia.xpi"

cat > "$NM_HOSTS/noctmalia.bridge.json" <<EOF
{
  "name": "noctmalia.bridge",
  "description": "noctmalia bridge shim",
  "path": "/opt/noctmalia/nm-shim",
  "type": "stdio",
  "allowed_extensions": ["bridge@noctmalia"]
}
EOF

# The profile belongs to this container alone; a lock left by an unclean stop is stale.
rm -f "$PROFILE/lock" "$PROFILE/.parentlock"

case "$TBD_MODE" in
  headless)
    set -- --headless
    ;;
  gui)
    # Interactive bootstrap (e.g. OAuth sign-in) against a mounted Wayland socket.
    export MOZ_ENABLE_WAYLAND=1
    set --
    ;;
  *)
    echo "tbd: unknown TBD_MODE '$TBD_MODE' (headless|gui)" >&2
    exit 2
    ;;
esac

# Headless GTK logs a three-line icon-theme assertion for every icon lookup, which
# buries everything else. Thunderbird runs as a child so its output can pass through
# a filter; TERM/INT from the init process are forwarded to it.
rm -f /tmp/tb-output
mkfifo /tmp/tb-output
grep --line-buffered -vE 'gtk_icon_theme|Gtk-CRITICAL|^[[:space:]]*$' < /tmp/tb-output &

/opt/thunderbird/thunderbird "$@" --profile "$PROFILE" --no-remote > /tmp/tb-output 2>&1 &
tb_pid=$!
trap 'kill -TERM "$tb_pid" 2>/dev/null' TERM INT

status=0
wait "$tb_pid" || status=$?
if kill -0 "$tb_pid" 2>/dev/null; then
  # The first wait was interrupted by a forwarded signal; wait for the real exit.
  status=0
  wait "$tb_pid" || status=$?
fi
exit "$status"
