#!/usr/bin/env bash
# docs/one-program-plan.md Stream 0: Thunderbird natively, headless, under a systemd user scope,
# with the stub as the client — no container, no window, no docker.
#
# Ran and passed 2026-09-15 (docs/findings.md §11). The stub it drove (tools/mailnd-stub.py,
# tools/bridgectl.py) and the tbd/ tree it read from have since been deleted — the sequence lives
# in crates/noctmalia/src/thunderbird/ now — so this is a record of what was done by hand, not a
# tool that still runs.
#
#   spike/native-probe/run.sh up         # provision, spawn, wait for hello, print status
#   spike/native-probe/run.sh quit       # bridge.quit, then check the lock is gone
#   spike/native-probe/run.sh windowed   # same profile, with a window
#   spike/native-probe/run.sh stop       # systemctl --user stop the scope (the crash-close)
#   spike/native-probe/run.sh down       # stop everything, stub included
set -euo pipefail

here=$(cd "$(dirname "$0")/../.." && pwd)
version=155.0.1
install="${XDG_DATA_HOME:-$HOME/.local/share}/noctmalia/thunderbird/$version"
data="${XDG_DATA_HOME:-$HOME/.local/share}/noctmalia"
profile="$data/profile"
state="${XDG_STATE_HOME:-$HOME/.local/state}/noctmalia"
run="${XDG_RUNTIME_DIR:?}/noctmalia-spike"
unit=noctmalia-thunderbird
export NOCTMALIA_RUN="$run"
mkdir -p "$profile/extensions" "$state" "$run" "$HOME/.mozilla/native-messaging-hosts"

provision() {
  # policies.json: the tarball ships its own updater, and outside a root-owned /opt it would use it.
  mkdir -p "$install/distribution"
  cat > "$install/distribution/policies.json" <<'JSON'
{ "policies": { "DisableAppUpdate": true, "DisableTelemetry": true, "DisableSystemAddonUpdate": true } }
JSON
  # The XPI, freshly zipped from the source tree, written only when it changed.
  local xpi="$profile/extensions/bridge@noctmalia.xpi" fresh="$run/bridge.xpi"
  (cd "$here/tbd/bridge" && rm -f "$fresh" && zip -qr -X "$fresh" .)
  if ! cmp -s "$fresh" "$xpi"; then cp "$fresh" "$xpi"; echo "== xpi updated"; fi
  # The shim, and the manifest that names it.
  install -m 0755 "$here/tbd/shim/nm-shim.py" "$data/nm-shim.py"
  cat > "$HOME/.mozilla/native-messaging-hosts/noctmalia.bridge.json" <<JSON
{ "name": "noctmalia.bridge", "description": "noctmalia bridge shim", "path": "$data/nm-shim.py",
  "type": "stdio", "allowed_extensions": ["bridge@noctmalia"] }
JSON
  # Prefs: the container's, with the dev pref on for the spike's own calls.
  { cat "$here/tbd/prefs/user.js"; echo 'user_pref("extensions.noctmalia.dev", true);'; } > "$profile/user.js"
}

spawn() {
  local mode=$1
  local flags=(--profile "$profile" --no-remote)
  local env=(HOME="$HOME" PATH="$PATH" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" LANG="${LANG:-C.UTF-8}"
             NOCTMALIA_BRIDGE_SOCKET="$run/bridge.sock" MOZ_CRASHREPORTER_DISABLE=1)
  [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ] && env+=(DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS")
  if [ "$mode" = headless ]; then
    flags+=(--headless)
  else
    env+=(WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}" MOZ_ENABLE_WAYLAND=1)
  fi
  if systemctl --user is-active --quiet "$unit.scope" 2>/dev/null; then
    echo "== stale scope $unit.scope is alive; stopping it"; systemctl --user stop "$unit.scope"; sleep 1
  fi
  echo "== spawning ($mode) under $unit.scope, log: $state/thunderbird.log"
  # An allowlisted environment: the nix devshell's LD_LIBRARY_PATH must not reach a system-GTK build.
  systemd-run --user --scope --collect --quiet --unit "$unit" \
    env -i "${env[@]}" "$install/thunderbird" "${flags[@]}" \
    > >(grep --line-buffered -vE 'gtk_icon_theme|Gtk-CRITICAL|^[[:space:]]*$' >> "$state/thunderbird.log") 2>&1 &
  echo $! > "$run/systemd-run.pid"
}

stub() {
  if [ -S "$run/ctl.sock" ] && python3 "$here/tools/bridgectl.py" status >/dev/null 2>&1; then return; fi
  python3 "$here/tools/mailnd-stub.py" >> "$state/stub.log" 2>&1 &
  echo $! > "$run/stub.pid"
  for _ in $(seq 20); do [ -S "$run/ctl.sock" ] && return; sleep 0.25; done
  echo "stub did not come up" >&2; exit 1
}

wait_hello() {
  for i in $(seq 90); do
    if python3 "$here/tools/bridgectl.py" status 2>/dev/null | grep -q '"connected": true' \
       && python3 "$here/tools/bridgectl.py" status | grep -q '"protocol"'; then
      echo "== hello after ${i}s"; python3 "$here/tools/bridgectl.py" status; return 0
    fi
    sleep 1
  done
  echo "== no hello in 90s; tail of the log:"; tail -20 "$state/thunderbird.log"; return 1
}

case "${1:-up}" in
  up)       provision; stub; spawn headless; wait_hello; systemctl --user status "$unit.scope" --no-pager | head -12 ;;
  windowed) provision; stub; spawn windowed; wait_hello ;;
  quit)
    python3 "$here/tools/bridgectl.py" call bridge.quit '{}' || true
    for i in $(seq 20); do systemctl --user is-active --quiet "$unit.scope" || break; sleep 0.5; done
    echo "== scope active: $(systemctl --user is-active "$unit.scope" || true)"
    echo "== lock present: $([ -e "$profile/lock" ] && echo yes || echo no), .parentlock: $([ -e "$profile/.parentlock" ] && echo yes || echo no)"
    ls "$profile"/*-wal 2>/dev/null | sed 's/^/== wal left: /' || true
    grep -o '"profile-before-change":[^,}]*' "$profile/sessionCheckpoints.json" 2>/dev/null | sed 's/^/== checkpoint /' || echo "== no sessionCheckpoints.json"
    ;;
  stop)     systemctl --user stop "$unit.scope"; echo "== lock present after stop: $([ -e "$profile/lock" ] && echo yes || echo no)" ;;
  down)     systemctl --user stop "$unit.scope" 2>/dev/null || true; [ -f "$run/stub.pid" ] && kill "$(cat "$run/stub.pid")" 2>/dev/null || true; rm -f "$run"/*.sock; echo "== down" ;;
  *) echo "usage: $0 up|windowed|quit|stop|down" >&2; exit 2 ;;
esac
