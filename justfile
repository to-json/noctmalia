# noctmalia — a Noctalia-native mail, contacts and calendar window, with its own Thunderbird behind it.
#
#   just              the window. It starts its Thunderbird and stops it when closed.
#   just fake         the window against a stand-in: no Thunderbird at all, the same mail.
#   just --list       everything else.

set shell := ["bash", "-euo", "pipefail", "-c"]

default: run

# The window, with the window chrome that squares its corners when tiled.
run *args:
    @just _quit
    scripts/run.sh --chrome {{ args }}

# The window with the bridge's dev methods on and the control socket's raw `call`: what `just seed`
# and `just smoke` talk to.
dev *args:
    @just _quit
    scripts/run.sh --chrome --dev {{ args }}

# The window against tools/fake-bridge.py. Nothing else has to be running.
fake *args:
    #!/usr/bin/env bash
    set -euo pipefail
    just _quit
    sock="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}/noctmalia/bridge.sock"
    rm -f "$sock"
    scripts/run.sh --chrome --backend external &
    ui=$!
    trap 'kill $ui >/dev/null 2>&1 || true' EXIT
    # The app is the listener; the stand-in connects in, so it waits for the socket to appear.
    for _ in $(seq 240); do [ -S "$sock" ] && break; sleep 0.5; done
    tools/fake-bridge.py {{ args }} &
    bridge=$!
    trap 'kill $ui $bridge >/dev/null 2>&1 || true' EXIT
    wait $ui

# Put the development fixture — accounts, contacts, mail — into the running window's Thunderbird.
# Needs `just dev` up. `just seed --reset` replaces what is there.
seed *args:
    tools/seed.sh {{ args }}

# A folder worth windowing: `just flood` for 500 unremarkable messages, `just flood 5000` for more.
flood count="500":
    tools/seed.sh --flood {{ count }}

# The whole round trip against a real mail server: GreenMail in the one container left, mail
# genuinely arriving, Gloda threading and search, a reply that threads, a filter, a calendar event.
smoke:
    tools/smoke.sh

# Pictures of every surface, drawn headlessly. Look at them; they assert nothing.
shots:
    scripts/cargo.sh test -p noctmalia --test shots -- --ignored --nocapture

# The mail corpus as .eml files, for reading with something else.
eml directory="target/eml":
    tools/fixture.py eml {{ directory }}

# Throw our Thunderbird profile away. The next start begins from nothing.
reset:
    @just _quit
    rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/noctmalia/profile"
    @echo "profile removed"

# What is running, and what the bridge says about it.
status:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "noctmalia"
    pid=$(pgrep -x noctmalia | paste -sd' ' -)
    if [ -n "$pid" ]; then echo "  running (pid $pid)"; else echo "  not running"; fi
    echo "thunderbird"
    systemctl --user is-active --quiet noctmalia-thunderbird.scope && echo "  up in noctmalia-thunderbird.scope" || echo "  not running"
    echo "bridge"
    tools/noctmalia-ctl.py status 2>/dev/null | sed 's/^/  /' || echo "  no answer from the control socket"

# Close a running window; its Thunderbird leaves with it.
[private]
_quit:
    pkill -x noctmalia >/dev/null 2>&1 || true
    sleep 0.4

# Ask Noctalia to render its palette where the window can read it. Once per machine.
palette:
    scripts/cargo.sh run -q -p noctmalia --bin noctmalia -- --install-palette-template

test:
    scripts/cargo.sh test --workspace

fmt:
    scripts/cargo.sh fmt -p noctmalia -p noctmalia-bridge

lint:
    scripts/cargo.sh clippy --workspace --all-targets
