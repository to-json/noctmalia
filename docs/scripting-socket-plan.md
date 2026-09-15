# noctmalia — scripting socket

Date: 2026-09-14
Depends on: `command-palette-plan.md` (hard — fronts the same command registry)

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

### 1.3 Dispatching into the app
**This is the plan's actual hard part, and it's a real design question, not a solved one by
analogy.** The bridge's existing event streaming is one-way push into iced's `Subscription` —
useful precedent for *receiving* Thunderbird events, but socket RPC is different: it needs
request/response correlation by id, routed into iced's `Task`/`update()` loop, with a defined
answer for what happens when a socket command and a keypress try to mutate app state at the same
time. Design this explicitly (a channel from the socket's tokio task into a `Message` the app
already knows how to handle, replies correlated by id and sent back once `update()` produces a
result) rather than assuming it falls out of the bridge's existing shape.

### 1.4 Command surface
Every `Command` in the registry with `exposed_to_socket = true` becomes callable by name, plus a
`list` verb returning names, descriptions, and exposure status — so a script can discover what's
callable without reading source.

## Stream 2: A CLI

**Files:** `tools/noctmalia-ctl` (new, a thin script or small Rust bin)

### 2.1 The shape
`noctmalia-ctl list`, `noctmalia-ctl run <command> [args...]` — keep it thin, the socket does the
real work. Commit to a stable stdout format (e.g. one JSON object per line, matching the NDJSON
wire format rather than inventing a human-readable format that then becomes a second contract to
maintain) since scripts written against this will depend on its shape.

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
- The dispatch-into-`update()` design (1.3) is genuinely unsolved above the level of "here's the
  shape" — budget real design time for it, not just implementation time.
- Whatever stdout format `noctmalia-ctl` commits to becomes something scripts depend on; changing
  it later is a breaking change the same way renaming `g c` was — pick something you'd be fine
  keeping.
