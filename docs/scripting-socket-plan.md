# noctmalia — scripting socket

Date: 2026-09-14
Depends on: `command-palette-plan.md` (hard — fronts the same command registry)
Status: **Shipped 2026-09-15**, verified against the real running app (`just` + `noctmalia-ctl.py`), not just the test suite.

---

## Context

want.md: noctmalia should own its own socket(s) so mail operations can be driven from a shell
command language — a `notmuch`-style CLI, a status-bar widget, a launcher script. Interview settled
the trust model: personal automation, trust the local user, filesystem permissions are the whole
auth story — not the multi-client daemon `design.md` once sketched (and explicitly rejected a
WebSocket-based version of, for the same "no port or token" reasoning this plan reuses).

This is a new listener, not a repurposing of `crates/noctmalia-bridge`, which is the *Thunderbird*
transport and stays exactly what it is (mail-plan §4: "only one client may hold the bridge
socket"). The new socket is ours to be as permissive with as we like on the *transport* — but not
on *which commands it can run*, which is where this plan differs from the first draft.

**Command exposure is default-deny, per command, decided in the interview after review.**
`command-palette-plan.md`'s registry backfills *every* keybound action, including
archive/delete/send. "Trust the local user" was meant to mean the socket is exactly as trusted as
you are — it was not meant to mean any local process gets to send mail with zero confirmation as a
side effect of the palette's registry being exhaustive. So: `Command::exposed_to_socket` (added to
the struct in `command-palette-plan.md` §3.1, defaulting `false`) is the actual gate. Read/nav
commands (archive, search, list, jump, filter) are the ones expected to flip it on; send/delete/
discard stay off by default and are opted into individually, deliberately, in this plan's own work
— not inherited for free from the registry's exhaustiveness.

## Stream 1: The listener

**Files:** `crates/noctmalia/src/control.rs` (new)

### 1.1 Bind and permission
`$XDG_RUNTIME_DIR/noctmalia/control.sock`, mode `0600`, uid-only — tighter than the bridge socket's
`0660` (README), since there's no second container user to admit here.

### 1.2 Wire protocol
NDJSON, mirroring `noctmalia-bridge/src/protocol.rs`'s existing shape (`{id, method, params}`
request; `{id, result|error}` or `{event, data}` reply) — a genuinely mirrorable shape, checked
directly against the source rather than assumed.

### 1.3 Dispatching into the app — resolved: fire-and-forget, not request/response
**Simplified during implementation, deliberately, rather than solved in the shape the plan
expected.** True request/response correlation — waiting for `update()` to actually run the command
and reporting its result back over the socket — would need a reply channel threaded from the
socket's own tokio task through to the app's single-threaded event loop and back, per request. That
is real complexity for a feature whose commands (search, refresh, switch view, jump to today) have
no meaningful "result" to report beyond "it ran." So `run` replies `"queued"` the moment the name
is validated and handed to a channel — not `"done"` — and the app resolves and runs it
independently, on its own thread, at its own pace. The channel itself is exactly the bridge's own
push shape: `control::spawn` takes an `mpsc::UnboundedSender<String>`, and the app turns names
arriving on the matching receiver into `Message::ControlRun(name)` through a `Subscription`, the
same way `Bridge`'s events already become `Message::Bridge(event)` — the one difference is the
receiver is single-consumer (`Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>>`, locked
once per item) rather than a broadcast channel, since there is exactly one sender and one logical
reader rather than many subscribers.

A socket command and a keypress racing to mutate app state is therefore a non-issue: both arrive as
ordinary `Message`s into the same single-threaded `update()`, one at a time, the way two keypresses
already do.

### 1.4 Command surface
Every `Entry` any surface's `commands()` marks `.exposed()` (Stream 3.1's addition to
`command-palette-plan.md`'s `Entry`/`Command` types) becomes callable by its label, plus a `list`
verb returning `{name, label}` pairs — since only what's already exposed is ever listed, there's no
separate "exposure status" to report. **The registry is fixed at startup**, built from `Mail`,
`People` and `Calendar` the moment they're constructed — before anything has loaded from the
bridge. That is what keeps the exposed set to commands that don't need live state to build their
message (search, refresh, jump to today, switch view): a fresh surface has no folders yet, so
`Message::OpenFolder(id)`-shaped commands are never in it at all, without having to name and
exclude them by hand. `run` itself still checks the *current* `all_commands()` before dispatching,
so a command that somehow stopped being available is a clean miss rather than a stale message.

## Stream 2: A CLI

**Files:** `tools/noctmalia-ctl.py` (named `.py`, matching every other script in `tools/` —
`fake-bridge.py`, `bridgectl.py`, `seed.py` — rather than the plan's original extension-less guess)

### 2.1 The shape
`noctmalia-ctl.py list`, `noctmalia-ctl.py run <name>` — a thin `argparse` + `socket.AF_UNIX`
script, the same shape `bridgectl.py` already uses for a different socket. Prints the JSON `result`
pretty-printed on success; a present `error` goes to stderr and the process exits 1. That is the
stable contract scripts depend on — the wire format underneath can still gain fields.

## Stream 3: Test

### 3.1 In-process fake client
A test harness connects to the real socket the running app binds in test mode and asserts a round
trip for a handful of commands, including one exposed and one deliberately not — same "internal
mocks extensively" instinct as `fake-bridge.py`, just on our side of the wire this time.

## Sequence integration

Depends on `command-palette-plan.md`'s registry existing as a concrete `Vec<Command>` to iterate,
and on that plan's `Command` struct already carrying the `exposed_to_socket` field. Otherwise
independent of `config-plan.md`/`context-commands-plan.md`/`mode-visual-plan.md`; can run in
parallel with them once the palette plan lands.

## Risks

- Default-deny narrows but does not eliminate the exposure question — flipping `exposed_to_socket`
  on for `mail.send` at some point is still a real decision this plan's implementer will face
  directly rather than one already made by "everything's exposed." Treat each flip as its own
  small, deliberate choice.
- Fire-and-forget (1.3) means a script that runs `search` and immediately expects search results
  back over the socket will be disappointed — `run` only ever confirms *queued*. Worth a real
  request/response design if a command ever needs to report something back; nothing exposed so far
  does.
- Whatever stdout format `noctmalia-ctl.py` commits to becomes something scripts depend on;
  changing it later is a breaking change the same way renaming `g c` was — pick something you'd be
  fine keeping.
- **Observed against the real app, 2026-09-15:** mail and people both expose a command labeled
  "Search", and the socket's `name` is the label — `run "Search"` reaches whichever one
  `all_commands()` happens to list first (mail), never people's. Harmless today since nothing
  exposed collides in a way that matters, but a real fix (a surface-prefixed name, mirroring the
  palette's own `m`/`c`/`p`) is worth doing before exposing more.
