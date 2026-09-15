# noctmalia — config system

Date: 2026-09-14
Depends on: `command-palette-plan.md` (soft — the custom-command schema should match the
registry's `Command` shape)

---

## Context

Nothing in the app reads a config file today — theme comes from Noctalia's own palette template
mechanism (README, a different thing), and everything else is compiled in. want.md wants "most
configuration" to move to files: well-named values, a complete example, no explainer comments. The
immediate consumer is `context-commands-plan.md`; this plan builds the loader once so that plan,
and future user-defined commands or keybindings, don't each invent one.

One TOML file, sectioned, per the interview: `$XDG_CONFIG_HOME/noctmalia/config.toml`. `serde` is
already a workspace dependency; add `toml`.

## Stream 1: The loader

**Files:** `crates/noctmalia/src/config.rs` (new)

### 1.1 Schema
```toml
[[template]]
name = "define"
command = ["dict", "{selection}"]
contexts = ["mail-body"]

[[template]]
name = "open-in-browser"
command = ["xdg-open", "{selection}"]
contexts = ["mail-body", "person-field"]
```
Field names carry their own meaning — `command` is an argv array, not a shell string, so there is
no quoting ambiguity to document in a comment. `contexts` is settled jointly with
`context-commands-plan.md`, which is this schema's first real consumer.

### 1.2 Load, validate, default
Missing file → built-in defaults (empty template list, nothing else configurable yet in this plan).
Malformed file → a startup toast naming the file and the line, never a silent fallback that hides a
typo.

### 1.3 The example file
`config.example.toml`, shipped in the repo (not installed), covering every section this plan and
`context-commands-plan.md` define — complete, not "see docs for more."

### 1.4 Test
Round-trip: write a config, load it, assert the struct. Malformed-file toast path. Missing-file
default path.

## Sequence integration

No hard dependency on the palette plan, but sequenced after it so the schema for "a named,
invokable thing" doesn't diverge from `commands.rs`'s `Command` shape. Parallelizable with
`scripting-socket-plan.md` and `mode-visual-plan.md`. If this plan ships and
`context-commands-plan.md` stalls afterward, the shipped `config.toml` schema does nothing yet —
document that plainly in the app (an empty templates list has no visible effect) rather than
implying configurability that isn't wired up.

## Risks

- Scope creep: "most configuration" invites pulling in keybindings and theme too. This plan
  deliberately covers only what `context-commands-plan.md` needs; a keybinding-config plan is real
  future work, not this one.
