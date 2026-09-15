# noctmalia — command palette & quick-open

Date: 2026-09-14
Depends on: None

---

## Context

want.md's core ask: movement and action should feel like lazyvim — fzf-oriented, a command
palette, and a clean split between "commands from the surface I'm in" (no prefix) and "commands
from the other two" (prefixed). `docs/design.md`'s old daemon-era sketch imagined a `⌘K palette`
and `search_picker`/`keybind_recorder`, but neither was ever built — `mail-plan.md` shipped the
mutt-shaped modal keymap instead, which is complementary, not a substitute: a keymap is for the
handful of bindings you've memorized, a palette is for everything else and for discovery.

Two genuinely different pickers hide under "fzf-oriented": a **command palette** (verbs — run an
action) and a **quick-open** (nouns — jump straight to a message/person/event by typing part of
it). Both want the same fuzzy-match core; they differ only in what they list and what selecting a
row does.

**The codebase is already command-pattern shaped — this plan does not need to invent that.** Every
surface (`mail.rs`, `contacts.rs`, `calendar.rs`) already resolves a `Binding` enum through one
`match` into a flat `Message` enum, which `update()` consumes — e.g. `Binding::Archive =>
Message::Archive`, `Binding::New(...) => Message::New(...)`. That match arm already *is* a command
table in everything but name. Building the registry (Stream 3) is reflection over this existing
shape, not new dispatch architecture.

**Neither `noctalia-iced` primitive this plan needs exists yet.** Checked directly: no picker
widget, no fuzzy-match code, nothing named `search_picker` or `keybind_recorder` anywhere in the
sibling repo (`design.md`'s sketch was never built). Both are new.

Surface prefixes: `m` (Mail), `p` (People — see Stream 1), `k` (Calendar, since `c` is taken).
Typing a scope letter followed by a query narrows to that surface's commands or data; a bare query
searches the current surface's commands/data with no prefix, per want.md.

## Stream 1: Rename Contacts → People

**Problem:** the palette prefix scheme wants a `p` letter, which reads badly as "contacts." Settled
in interview: the full rename, not a palette-only alias.

**Files:** `crates/noctmalia/src/surfaces/{mod.rs,contacts.rs → people.rs}`,
`crates/noctmalia/src/contacts.rs`, `src/app.rs`, `README.md`, `crates/noctmalia/tests/contacts.rs`

### 1.1 Rename the surface and its module
`Surface::Contacts` → `Surface::People`, `src/surfaces/contacts.rs` → `src/surfaces/people.rs`,
titlebar text "Contacts" → "People", switcher hint `Contacts · g c` → `People · g p`.

### 1.2 Decide the `g` binding
`g c` currently means Contacts; moving it to `g p` frees `c` for something else (nothing claims it
yet) but breaks a binding someone may have already built muscle memory for. Move to `g p`, keep
`g c` as a silent alias for one release, drop it once the palette makes discovery cheap enough that
aliases aren't load-bearing.

### 1.3 Don't retcon `mail-plan.md`
`docs/mail-plan.md` is a historical build record ("**It has been built.**") of the app as it was
named and shipped. Add a dated addendum note at its top — the same convention `docs/design.md`'s
own header already uses for superseding decisions — rather than rewriting its "Contacts" prose to
say "People."

### 1.4 Test
`tests/contacts.rs` → rename/keep, assert the titlebar and switcher hint say "People", assert both
`g p` and (temporarily) `g c` switch surfaces.

## Stream 2: The fuzzy-match core

**Problem:** no fuzzy matching exists anywhere in the stack, and no picker widget exists in
`noctalia-iced` to put it in.

**Files:** new module in `../noctalia-iced`, e.g. `noctalia_iced::fuzzy` and `noctalia_iced::picker`

### 2.1 Pick and hand-roll an algorithm
A small subsequence-with-bonus matcher (the `fzf`/`skim` scoring shape: contiguous runs, word-start
bonus, penalize gaps) is enough — this does not need to out-rank `fzf` itself, it needs to feel
like it. Hand-roll rather than add a crate, matching the project's `ammonia`-avoidance taste in
mail-plan §1. Because `noctalia-iced` is a path dependency to a sibling checkout (`Cargo.toml`),
not a published crate, iterating on scoring taste is edit-both-repos-and-rebuild, not a publish
cycle — expect to revisit the scoring function several times once it's actually being typed into,
not vendor it once and move on.

### 2.2 A generic picker widget
`noctalia_iced::picker::Picker<T>`: an overlay, a text input, a scored+sorted list, `j`/`k`/arrows
plus type-to-filter, `Enter` to act, `Escape` to dismiss — the widget knows nothing about mail,
people, or commands, only `T: Display` (or a `label()` trait) and a scoring closure.

## Stream 3: The command registry

**Problem:** "commands from the app I'm in" needs a list of commands to exist as data.

**Files:** `crates/noctmalia/src/commands.rs` (new)

### 3.1 The `Command` shape
A `Command` is a name, a short description, an optional current-keybinding-for-display, a way to
produce that surface's `Message` when run, and a field reserved now for Plan 5's use —
`exposed_to_socket: bool`, **default `false`** — even though nothing reads it until Plan 5 lands.
Deciding the field's existence here, rather than retrofitting it once the socket plan needs it,
means the registry's shape never has to change under a plan that depends on it.

Some commands need context that isn't available standalone (a selected row, the current folder).
Rather than inventing a general applicability-predicate system now, a command whose context isn't
met is disabled/greyed in the palette rather than crashing — keep this simple, extend it only if a
concrete command needs more.

### 3.2 Backfill from the existing keymaps
Walk each surface's already-keybound `Binding` table (mail-plan §5's, people's, calendar's) and
register the ones whose effect stands on its own without a count or a held key — archive, search,
compose, reply/forward, jump-to-folder, propose-a-rule, save, delete, and so on.

**Narrowed during implementation, noted here rather than silently done:** pure cursor movement
(`j`/`k`/`gg`/`G`, half/full-page scroll) and `Open`/`Back`/`Fold`, which exist to be repeated or
held rather than invoked once from a list, are left out. A palette entry that just says "down" is
discoverable clutter, not discoverability — nobody fuzzy-types their way to an arrow key — and the
interview's own answer here ("not really sure … focus on sequencing rather than scope") left the
exact cut a judgment call rather than a mandate to include them. Actions that exist only as a
button's `on_press` with no `Binding` are still out of scope for this plan; don't let this grow
into auditing every `on_press` in the app.

## Stream 4: The palette and quick-open surfaces

**Files:** `crates/noctmalia/src/app.rs`, `crates/noctmalia/src/surfaces/mod.rs`

### 4.1 Command palette (verbs)
`ctrl+k` opens `Picker<Command>` seeded with the current surface's commands unprefixed. Typing `m `,
`p `, or `k ` at the start of the query re-seeds the list from that surface's commands instead —
implemented as a prefix strip before the fuzzy match runs, not a mode switch.

### 4.2 Quick-open (nouns)
`ctrl+p` opens `Picker<Item>` over whatever the current surface's rows are (messages, people,
events); same `m`/`p`/`k` prefix convention jumps to the other surfaces' data. Selecting a row
navigates to it and switches surface if needed.

### 4.3 Test
Headless widget tests (`tests/mail.rs`'s pattern) opening the palette, typing a query, asserting the
filtered+sorted list, and asserting `Enter` dispatches the right message. Golden-path plus: empty
query, no matches, prefix with no query yet.

## Sequence integration

Ships second, right after Reminders, per the interview's explicit priority. Its command registry
(Stream 3) and People rename (Stream 1) are load-bearing for Plan 5 (Socket) and referenced by
Plan 6 (Mode & Visual). Palette-open is a real mode with no visual indicator until Plan 6 lands —
an acknowledged, bounded gap for however long Plan 6 is deferred, not an oversight.

## Risks

- The fuzzy matcher's scoring taste is genuinely iterative, not a fixed-size task — budget for
  revisiting it after real use, not a one-shot implementation.
- `Picker<T>` risks growing surface-specific special cases across two call sites; keep it generic
  and push all surface knowledge into the closures each call site provides.
- The `g c` → `g p` alias period is a real UX inconsistency window; keep it short.
