# noctmalia

A mail, calendar and contacts client that looks native on a [Noctalia](https://github.com/noctalia-dev/noctalia)
desktop, with **Thunderbird as the backend**. Thunderbird runs headless in a container and keeps
doing accounts, protocols, storage, sync and sending. We replace only the UI.

Today that UI is a **contacts rolodex**. Mail is next; see `docs/design.md`.

```
thunderbird --headless ── bridge (MailExtension) ── nm-shim ──►  noctmalia
        in a container                                   NDJSON over a unix socket
                                                         (the app listens; the shim connects in)
```

| Path | What |
|---|---|
| `flake.nix`, `scripts/` | The devshell, and the cargo/run wrappers around it |
| `crates/noctmalia-bridge` | The protocol: binds the socket, matches replies by id, streams events |
| `crates/noctmalia` | The app: vCard parsing, contacts calls, theme/font, the rolodex window |
| `tbd/` | The Thunderbird container and the bridge extension. Protocol reference: `tbd/README.md` |
| `tools/` | `fixture.py` (the test data), `seed.sh`/`seed.py` (put it in a real profile), `fake-bridge.py` (a Thunderbird stand-in), `bridgectl.py` + `mailnd-stub.py` (a CLI path), `smoke.sh` |
| `docs/findings.md` | What has actually been verified, and the gotchas behind it |
| `docs/design.md` | Where this is going |

## Build and run

Neither cargo nor rustc is installed here, so everything runs in this repository's nix devshell
(`flake.nix`), which also carries the Wayland and GL libraries iced needs.

```sh
scripts/run.sh                  # build release and run it
scripts/cargo.sh test --workspace
```

Use `scripts/run.sh`, not `cargo run`. Both the profile and the library path matter: a debug build
of iced is unusably slow, and a devshell without Mesa on it falls back to software rendering, which
looks like the application being broken rather than like a driver problem. `scripts/run.sh --debug`
switches profile when you want a backtrace.

`nix develop` reads the git tree, so `git add` a new file before the shell will see it.

`scripts/run.sh` forces software rendering, because this machine's Ivy Bridge GPU renders iced as
torn frames on both of Mesa's hardware paths (noctalia-iced's clock demo included — it is the stack,
not this app). Set `NOCTMALIA_GPU=1` to use the GPU on hardware that works. `NOCTMALIA_FPS=1`
reports redraws/s and `view()` time; `NOCTMALIA_FPS=drive` redraws continuously to show the ceiling.

### Against a fake Thunderbird (no container)

```sh
scripts/run.sh                        # binds $XDG_RUNTIME_DIR/noctmalia/bridge.sock
tools/fake-bridge.py                  # connects to it and serves contacts from memory
```

`tools/fake-bridge.py --empty` serves an empty store, for the empty states.

### Against the real thing

Docker needs to be usable first. `docker.socket` is socket-activated on Arch, so group membership is
usually the only thing missing:

```sh
sudo usermod -aG docker "$USER"    # then log out and back in, or `newgrp docker` for one shell
```

Boot a headless Thunderbird and put the test data in it:

```sh
tools/seed.sh                         # stack up, wait for the bridge, seed accounts + contacts
tools/seed.sh --reset                 # ...replacing fixture contacts already there
```

Both the accounts and the contacts come from `tools/fixture.py`, which is also what
`tools/fake-bridge.py` serves — so the same people are on screen either way. Seeding is idempotent
and keeps the profile volume; `docker compose down -v` starts over.

Then hand the socket to the UI — only one client may hold it, and seeding used the CLI stub:

```sh
docker compose -f compose.yaml -f compose.ui.yaml up -d --build tbd
scripts/run.sh
```

The overlay bind-mounts the socket directory into `$XDG_RUNTIME_DIR` — a host process cannot reach a
named Docker volume. tbd runs as uid 1000 and the socket is mode 0660, so the desktop user must be
uid 1000 (`id -u`). Either process can start first; the shim retries forever.

## Looking like the rest of the desktop

The window follows the palette your Noctalia shell is running — read from
`$XDG_STATE_HOME/noctalia/settings.toml` and the palette file it names, re-read when you change
theme — and draws in the family fontconfig resolves `sans-serif` to. Neither is iced's default:
iced's text stack hardcodes `Open Sans` as its sans-serif and falls back to a serif when that is
missing, and
noctalia-iced used to hardcode one palette. Both now come from the system.

## Status

- **Verified by running it:** the transport (reconnect, out-of-order replies, in-flight failure on
  disconnect, the 1 MiB cap), vCard round-tripping, and contacts list/search/create/update/delete —
  the last against `tools/fake-bridge.py`, not against Thunderbird itself.
- **Not yet run against a real Thunderbird.** That is the next step, and the one most likely to turn
  up surprises: `docs/findings.md` §8 lists what to watch for. `tools/seed.sh` is written and its
  logic is tested against the stand-ins, but no container has been started on this machine.
