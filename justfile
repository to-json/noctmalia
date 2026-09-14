# noctmalia — a Noctalia-native rolodex, with Thunderbird headless in a container behind it.
#
#   just              Thunderbird up, then the rolodex against it. The whole thing, from cold.
#   just fake         the rolodex against a stand-in: no Docker, no Thunderbird, the same people.
#   just --list       everything else.
#
# Two things worth knowing, both handled here so nobody has to remember them:
#
#  - Only one client may hold the bridge socket, so opening the rolodex closes any other one first.
#  - Docker group membership is not live in a shell that was already open, so anything touching
#    Docker goes through scripts/with-docker.sh.

set shell := ["bash", "-euo", "pipefail", "-c"]

# The UI overlay bind-mounts the bridge socket into $XDG_RUNTIME_DIR, where a host process can
# reach it, and drops the mailnd stub that would otherwise hold the socket first.
compose := "docker compose -f compose.yaml -f compose.ui.yaml"

default: run

# Thunderbird up, then the rolodex against it.
run: up
    #!/usr/bin/env bash
    set -euo pipefail
    just _quit
    log=$(mktemp -t noctmalia-run.XXXXXX)
    scripts/run.sh --chrome > "$log" 2>&1 &
    ui=$!
    tail -n +1 -f "$log" --pid="$ui" & tailer=$!
    trap 'kill $ui $tailer >/dev/null 2>&1 || true; rm -f "$log"' EXIT

    # Thunderbird's extension says hello when the shim attaches to our socket. Restarting only the
    # window can leave the shim attached but the extension never announcing itself again, and then
    # the window sits on "Waiting for Thunderbird" forever with nothing visibly wrong. Bouncing tbd
    # makes it say hello; this waits a reasonable while first rather than doing it every time.
    for _ in $(seq 25); do
      grep -q "Thunderbird attached" "$log" && break
      kill -0 "$ui" 2>/dev/null || break
      sleep 1
    done
    if kill -0 "$ui" 2>/dev/null && ! grep -q "Thunderbird attached" "$log"; then
      echo "== no hello from Thunderbird — bouncing it"
      scripts/with-docker.sh {{ compose }} restart tbd
    fi
    wait "$ui"

# The rolodex on its own, against a Thunderbird that is already up.
ui:
    @just _quit
    scripts/run.sh --chrome

# The rolodex against tools/fake-bridge.py. Nothing else has to be running.
fake *args:
    #!/usr/bin/env bash
    set -euo pipefail
    just _quit
    sock="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}/noctmalia/bridge.sock"
    rm -f "$sock"
    scripts/run.sh --chrome &
    ui=$!
    trap 'kill $ui >/dev/null 2>&1 || true' EXIT
    # The app is the listener; the stand-in connects in, so it waits for the socket to appear.
    for _ in $(seq 240); do [ -S "$sock" ] && break; sleep 0.5; done
    tools/fake-bridge.py {{ args }} &
    bridge=$!
    trap 'kill $ui $bridge >/dev/null 2>&1 || true' EXIT
    wait $ui

# Bring Thunderbird up. Takes about a minute cold, longer on the first build.
up:
    scripts/with-docker.sh {{ compose }} up -d --build --wait --wait-timeout 400 tbd

# Stop Thunderbird. The profile volume survives.
down:
    scripts/with-docker.sh {{ compose }} down

# Put the development fixture into a real profile — `just seed` or `just seed --reset`.
seed *args:
    # Seeding drives the bridge through the CLI stub, so hand the socket back with `just run`.
    scripts/with-docker.sh tools/seed.sh {{ args }}

# Throw the Thunderbird profile away. The next `just seed` starts from nothing.
reset:
    scripts/with-docker.sh {{ compose }} down -v

# What is running, and whether anything is on the other end of the bridge.
status:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "containers"
    scripts/with-docker.sh {{ compose }} ps --format '{{{{.Name}}\t{{{{.Status}}' 2>/dev/null | sed 's/^/  /' | grep . \
      || echo "  none up"
    sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/noctmalia/bridge.sock"
    echo "bridge socket"
    if [ -S "$sock" ]; then echo "  bound at $sock"; else echo "  not bound — the rolodex is not running"; fi
    echo "rolodex"
    pid=$(pgrep -x noctmalia | paste -sd' ' -)
    if [ -n "$pid" ]; then echo "  running (pid $pid)"; else echo "  not running"; fi

# Close a running rolodex, so the next one can have the socket.
[private]
_quit:
    pkill -x noctmalia >/dev/null 2>&1 || true
    sleep 0.4

# Ask Noctalia to render its palette where the rolodex can read it. Once per machine.
palette:
    scripts/cargo.sh run -q -p noctmalia --bin noctmalia -- --install-palette-template

test:
    scripts/cargo.sh test --workspace

fmt:
    scripts/cargo.sh fmt -p noctmalia -p noctmalia-bridge

lint:
    scripts/cargo.sh clippy --workspace --all-targets
