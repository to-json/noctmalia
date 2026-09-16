# One program — noctmalia spawns its own Thunderbird

Date: 2026-09-15
Depends on: None. `docs/oauth-plan.md` Stream 2.4 will depend on this (the windowed restart).

> **Built, 2026-09-15.** Streams 0 to 3 are in the tree; `docs/findings.md` §11 records what
> Stream 0 found and what building it changed. Three departures from the text below:
> `scripts/with-docker.sh` stays, because `smoke.sh` still needs Docker for GreenMail; the control
> socket gained `wait` and a numbered event log alongside `call`, because smoke's event-driven
> steps needed what the deleted stub used to provide; and the bridge gained a five-second guard on
> replacing a connection, after the first live run met the old container and the new native
> Thunderbird pointed at the same socket. "Left for after" is still left.

---

## Context

Today noctmalia is half of a two-program dance. Thunderbird runs headless in a Docker container
(`tbd/`), a Python shim relays native messaging to a unix socket, and the user is expected to know
all of it: `just up`, `just run`'s "no hello, bouncing the container" heuristic, "only one client
may hold the socket", uid 1000, `newgrp docker`. That was the right shape for finding out whether
Thunderbird could be a backend at all (`docs/findings.md` §2, §9). It is the wrong shape for a
program somebody opens to read mail.

The decision (2026-09-15): **noctmalia starts and stops like a normal program.** It spawns its
own headless Thunderbird from a pinned Mozilla build it fetches once, against a private profile
it owns, in a transient systemd user scope so the whole tree is one unit; it takes it down when
the window closes. The user never sees Thunderbird. The container tooling is deleted, not kept as
a mode. There is no technical reason not to just spawn the process from ourselves; this plan is
the shortest path to doing exactly that, and a list of what is deliberately left for after.

What does not change: the bridge extension, the protocol, the "app listens, shim connects"
direction, and Thunderbird keeping all the state.

Facts the plan rests on, verified against the container and this machine during review:

- Bringing Thunderbird up is: a profile directory, `user.js`, the XPI at
  `profile/extensions/bridge@noctmalia.xpi`, a native-messaging host manifest in the user-global
  `~/.mozilla/native-messaging-hosts/`, and `thunderbird --headless --profile DIR --no-remote`.
  The manifest is honoured for `--profile` launches (the container already does exactly this).
- The socket path reaches the shim only through `NOCTMALIA_BRIDGE_SOCKET`, inherited from
  Thunderbird. The shim's `/tmp/nm-shim.state` is read by nothing but the Docker healthcheck.
- Native messaging launches the host as `[path, manifest-path, extension-id]`.
- **SIGTERM is an instant death for Gecko.** Every close of the container so far has been a
  crash-close: the `lock` symlink stays, WAL files stay open. A clean stop has to go through
  Thunderbird's own quit.
- **The pinned build un-pins itself outside the container.** `app.update.enabled` no longer
  exists in 155; the updater ships in the tarball and its timers already fire in `prefs.js`. The
  container was safe only because `/opt/thunderbird` is root-owned.
- The nix devshell's `LD_LIBRARY_PATH`, `LIBGL_DRIVERS_PATH`, `__EGL_VENDOR_LIBRARY_DIRS`,
  `LIBGL_ALWAYS_SOFTWARE` and `WGPU_BACKEND` reach any child. A Mozilla tarball links system GTK;
  nix Mesa underneath it is a mixed-ABI stack.
- `PR_SET_PDEATHSIG` fires when the *thread* that spawned the child exits, not the process.
- `Bridge::spawn` and `control::spawn` unlink and rebind their sockets unconditionally.
- `systemd-run --user --scope --collect --quiet` works here and refuses a duplicate unit name.
- Mozilla ships Linux builds for x86_64 only; the pinned tarball fetches at 86 MB with the SHA-512
  the Dockerfile records.
- `tools/seed.py` and `smoke.sh` provision through `dev.provisionAccount`, gated by the
  `extensions.noctmalia.dev` pref, via `mailnd-stub.py` holding the socket.

Decisions closed during review: the fetch shells out to `curl` (the README's "no fetch" is about
mail, not install); GreenMail stays as a smoke fixture, in the only container left; the Python
shim stays for this iteration; the account wizard is not this plan's problem.

---

## Stream 0: Spike — the native bring-up, by hand, in a morning

**Problem**: Every step has only ever been done inside a container with a fixed layout.

**File(s)**: `spike/native-probe/run.sh`, throwaway.

### 0.1 Native, headless, attached

Fetch 155.0.1, verify, untar under `$XDG_DATA_HOME/noctmalia/thunderbird/155.0.1/`. Run it
headless against `$XDG_DATA_HOME/noctmalia/profile` with the existing `user.js`, XPI, and Python
shim, `NOCTMALIA_BRIDGE_SOCKET` exported, **under `env -i` plus an allowlist** (`HOME`, `PATH`,
`XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `WAYLAND_DISPLAY`, `LANG`, `NOCTMALIA_BRIDGE_SOCKET`),
wrapped in `systemd-run --user --scope --collect --quiet --unit noctmalia-thunderbird`.
`scripts/run.sh` on the other end. Pass: `bridge.hello` arrives; `noctmalia-ctl.py status` says
connection 1; `systemctl --user status noctmalia-thunderbird.scope` shows the tree.

### 0.2 A clean stop exists

Add `bridge.quit` to the Experiment (`Services.startup.quit(Ci.nsIAppStartup.eForceQuit)`).
Call it. Pass: the process exits, the scope collects itself, `profile/lock` is gone, and
`sessionCheckpoints.json` records `profile-before-change`. Then confirm `systemctl --user stop`
on the scope is a working fallback.

### 0.3 The updater stays quiet

Write `distribution/policies.json` (`DisableAppUpdate`, `DisableTelemetry`) into the install
before first launch. Pass after one run: no `updates/` directory under the install, no update
timer prefs newly written.

### 0.4 Windowed on the same profile

Start it without `--headless`, same allowlisted environment plus `MOZ_ENABLE_WAYLAND=1`. Pass: a
Thunderbird window paints. This is the primitive the OAuth plan will build the wizard on; nothing
here drives the wizard.

### 0.5 Exit criteria

If 0.1 or 0.2 fails by lunch, stop and write down what broke. The remaining candidates for
failure are `bridge.quit` not being reachable from the Experiment's context, and sideloading
refusing a rewritten XPI; both are cheap to see.

---

## Stream 1: The launcher

**Problem**: `tbd/Dockerfile` and `tbd/entrypoint.sh` are the launcher. They become a module.

**File(s)**: `crates/noctmalia/src/thunderbird.rs`, `crates/noctmalia/build.rs`,
`crates/noctmalia/assets/{user.js,policies.json,nm-shim.py}`, `bridge/` (moved from
`tbd/bridge/`), `crates/noctmalia/src/main.rs`

### 1.1 Where things live

- Binary: `$XDG_DATA_HOME/noctmalia/thunderbird/155.0.1/thunderbird`, or whatever
  `NOCTMALIA_THUNDERBIRD` names (a packager's escape hatch; also the arm64 answer).
- Profile: `$XDG_DATA_HOME/noctmalia/profile`.
- Shim: `$XDG_DATA_HOME/noctmalia/nm-shim.py`, written from `include_str!` every start.
- Log: `$XDG_STATE_HOME/noctmalia/thunderbird.log`, truncated at start.
- Socket: `$XDG_RUNTIME_DIR/noctmalia/bridge.sock`, as now.

### 1.2 Fetch, once

No usable binary: run `curl -fL -o` into `$XDG_CACHE_HOME/noctmalia/`, verify SHA-512 in Rust,
extract with `tar`, write `policies.json`. `curl` missing: say the URL and the destination and
stop. Progress is the partial file's size, polled once a second.

### 1.3 Provision, every start

Write `user.js` from the asset (the container's, minus the dev pref; `--dev` adds
`extensions.noctmalia.dev=true`). Write the XPI only when its bytes differ; `build.rs` zips
`bridge/` and the crate embeds it. Write the native-messaging manifest with `path` set to the
shim's written location, every start. Remove `lock` and `.parentlock` only after 1.5's stale
scope check.

### 1.4 Spawn

From a dedicated supervisor thread that lives as long as the app: `systemd-run --user --scope
--collect --quiet --unit noctmalia-thunderbird -- <binary> --headless --profile <dir>
--no-remote`, environment built from the allowlist in 0.1, `PR_SET_PDEATHSIG=SIGTERM` on the
child as a backstop. If `systemd-run` fails to launch, spawn the binary directly in its own
process group. Stdout and stderr go to the log through a filter that drops the GTK icon-theme
noise the container's FIFO used to.

### 1.5 Stale scope

Before provisioning: if `noctmalia-thunderbird.scope` exists, `systemctl --user stop` it and wait
until it is gone. This is the "previous noctmalia was SIGKILLed" path and it is also why 1.3 may
remove locks.

### 1.6 Stop

When `iced::application::run` returns, or on SIGTERM/SIGINT (`tokio`'s `signal` feature, a
subscription that ends in `iced::exit`): call `bridge.quit` and wait up to 10 s for the scope to
be gone; then `systemctl --user stop`; then, without systemd, SIGTERM and SIGKILL the process
group. Exit after that and not before.

### 1.7 Crash

If Thunderbird exits while the window is open, restart it once. A second exit inside a minute is
`Failed` (Stream 2.1) with the log path on screen. No backoff ladder yet.

---

## Stream 2: The app

**Problem**: `ui::waiting` says "Waiting for Thunderbird" with a socket path.

**File(s)**: `crates/noctmalia/src/app.rs`, `ui.rs`, `main.rs`, `control.rs`

### 2.1 Three states on screen

`Starting` (with "Downloading Thunderbird, N of 86 MB" while 1.2 runs), `Ready`, and
`Failed { log }`. The window opens immediately in `Starting`. Nothing about sockets is on screen.

### 2.2 `--backend external`

Binds the socket and spawns nothing; for `just fake` and the tests. `App::new` takes the backend
as a value so tests keep constructing it with a bare bridge.

### 2.3 Control socket

`status` grows `thunderbird: { pid, scope, uptime_seconds, state }`. A `call` method passes
`{method, params}` through to the bridge, **only** with `--dev`. Seeding needs it (3.2). The
socket is `0600`; the trust boundary is the app's own.

### 2.4 Tests

`thunderbird::provision` against a temp directory: files written, XPI rewritten only on change,
manifest rewritten every time. Spawn and stop with `sleep` standing in for Thunderbird, skipped
where `systemd-run` is absent. The three states through the widget tree, as `tests/mail.rs` does.

---

## Stream 3: Delete the dance

**Problem**: Every user-facing document and script describes two programs.

**File(s)**: `tbd/`, `compose*.yaml`, `scripts/with-docker.sh`, `tools/`, `justfile`, `flake.nix`,
`README.md`, `docs/findings.md`, `docs/design.md`

### 3.1 Move first

`tbd/bridge/` becomes `bridge/`; `tbd/README.md`'s protocol reference becomes
`docs/bridge-protocol.md`; `tbd/prefs/user.js` and `tbd/shim/nm-shim.py` become assets (1.1).

### 3.2 Tooling through the app

`tools/seed.py` speaks `noctmalia-ctl.py call` instead of the stub's `ctl.sock`; `just seed`
needs a running `noctmalia --dev`. `tools/smoke.sh` starts GreenMail with one `docker run`, starts
`noctmalia --dev` natively, and runs its existing steps through `call`. This lands and passes
**before** 3.3.

### 3.3 Then delete

`tbd/Dockerfile`, `tbd/entrypoint.sh`, `compose.yaml`, `compose.ui.yaml`, `compose.gui.yaml`,
`scripts/with-docker.sh`, `tools/mailnd-stub.py`, `tools/bridgectl.py`. `flake.nix` drops Docker.
`just` runs the window; `fake`, `seed`, `smoke`, `shots`, `palette`, `test` remain; `up`, `down`,
`ui`, `reset`, `flood`, `mail` go. README's "Running it" becomes three lines and loses every
container section; `docs/findings.md` gets a dated §11 with what Stream 0 found;
`docs/design.md`'s header gets the convention's addendum note.

---

## Left for after, on purpose

- **The shim in Rust, in the same binary** (branch on `argv[2] == "bridge@noctmalia"` before any
  iced code). Drops the `python3` dependency. A day; changes nothing the user sees.
- **Single instance.** Needs the ordering the review found: probe the control socket, bind it,
  then the scope check, then the bridge. Today a second launch steals both sockets, exactly as it
  does with the container.
- **Restart backoff** beyond one retry.
- **Import from an existing Thunderbird.** A wholesale profile clone, minus locks, caches,
  `extensions/`, `extensions.json`, `addonStartup.json.lz4`, `extension-preferences.json`,
  `startupCache/`, `storage/`, and every `*-wal`/`*-shm`; refused while the lock symlink's pid is
  alive; sized honestly, because real profiles carry gigabytes of mbox.
- **Add an account.** Windowed Thunderbird is the whole three-pane, closing its last window
  quits the process, and `accounts.onCreated` fires before SMTP and token storage finish. That is
  `docs/oauth-plan.md`'s Stream 2.4 problem; this plan hands it the windowed restart (0.4).

## Sequence integration

Stands alone. Supersedes `docs/oauth-plan.md` 1.1's `TBD_MODE=gui` compose override with Stream
0.4 here. Order: 0, then 1 and 2 together, then 3.1, 3.2, 3.3 in that order.

## Risks

- **`bridge.quit` needs a live bridge.** If Thunderbird is up but the extension is not attached,
  the stop falls through to `systemctl --user stop`, which is a crash-close. Acceptable: it is
  what every close has been so far, and the profile is never the only copy of mail.
- **x86_64 only.** `NOCTMALIA_THUNDERBIRD` is the escape hatch; the manifest minimum is 153.
- **Sideload refresh.** Thunderbird reinstalls on a changed file; writing only on changed bytes
  is enough. If 0.1 shows otherwise, delete `profile/extensions.json` when the XPI changes.
- **The startup `connect()` in `background.js` logs an `InvalidStateError` on every boot** that
  nothing catches. Harmless so far; the supervisor's "attached" timing must not read it as failure.
