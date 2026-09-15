# noctmalia — context commands

Date: 2026-09-14
Depends on: `config-plan.md` (hard — templates are config entries)
Status: **Shipped 2026-09-15.**

---

## Context

want.md: highlight text, right-click, run a command template against it, "trivially" addable.
Interview settled the model precisely: a template is a shell command (argv, not a shell string —
consistent with `config-plan.md`'s schema), the selection is passed as a shell-quoted argument
substituted into it, and the output is always shown (toast or side panel) — it never rewrites the
selection, since most selection happens in the read-only pager.

**The riskiest assumption in this plan is untested and comes first, deliberately** (adversarial
review of the draft this plan came from): the mail pager renders through `iced::widget::markdown`'s
custom `Viewer` (mail-plan §1), not a plain `text_input`/`text_editor`. Whether that widget exposes
a readable selection span at all — across headings, links, lists — is unverified, and mail-plan §10
already records one prior surprise of exactly this shape ("looked simple until built," re: HTML
rendering). Stream 0 below resolves this before the rest of the plan is built on top of it.

## Stream 0: Spike — can the pager report a selection?

**Resolved during implementation, 2026-09-15 — the answer is no, and it is bigger than the pager.**
Checked directly against the vendored `iced_widget` 0.14.2 source: selection is implemented in
exactly two widgets, `text_input` and `text_editor`. Plain `text`/`rich_text` — which is what
`iced::widget::markdown::view_with` builds, and what the mail-plan §1 pager, a person's card in
*read* view, and calendar's read-only rows all draw through — has no click-and-drag selection at
all in this iced version. It is not a gap specific to the markdown `Viewer`; the plan's own
assumption that "person-field and event-description contexts are ordinary iced text widgets and
are unaffected" was optimistic in the wrong direction — those are `text()` too when merely
*viewing* (only the editor forms, while actively being edited, use `text_input`/`text_editor`),
so they have exactly the same gap the pager does.

**What this changes:** "highlight arbitrary text" is not buildable today without a custom
selectable-text widget — real click-to-character hit-testing against shaped glyphs, which is a
project on the scale of this plan's *other* four streams combined and belongs in its own plan if
it's ever worth it, not folded into this one under time pressure.

**What ships instead:** the same right-click-a-template experience, over a coarser unit than a
free character span — **the row or field you right-click, or the whole open message in the
pager** — rather than whatever's between two click points. This is still "highlight text, right
click, run a command on it" in spirit: the "selection" is just resolved by *which widget* was
clicked rather than by *where inside it*. It also composes cleanly with a future real selection
widget: only Stream 1's "what is selected" plumbing would need to change, not Stream 2's menu or
Stream 2.3's execution.

## Stream 1: Selection plumbing

**Problem:** none of the three surfaces currently expose "what was right-clicked" as state a menu
action could read.

**Files:** `crates/noctmalia/src/surfaces/{mail,people,calendar}.rs`

### 1.1 A selection type
Per Stream 0's finding, "selected" is the text of whatever was right-clicked, not an arbitrary
span: the mail pager's whole rendered body, a person card's one field value, a calendar item's
title or description. Each surface exposes this as a plain `String` alongside its existing state —
still not a new cross-cutting selection manager, just a different (coarser) answer to "what is
selected" than Stream 1 originally assumed.

**One real exception, found during implementation:** mail's compose body is a `text_editor`, not
plain `text` — the one widget in the app edited through something other than `text_input`/a
read-only renderer — and `text_editor::Content::selection()` genuinely returns the highlighted
span. Compose (tagged `"mail-compose"`, distinct from the pager's `"mail-body"`) uses the real
selection when there is one and falls back to the whole draft when there isn't, rather than always
taking the coarse whole-field answer every other context is stuck with.

## Stream 2: The context menu

**Simplified during implementation: no new primitive needed.** A right-click menu is "pick one
labeled action from a fuzzy-filterable list," which is exactly what `command-palette-plan.md`'s
`Overlay`/`Picker<Command<Message>>` already is — so a context menu is a second *caller* of that
same machinery (seeded from matching templates instead of keybound commands or loaded rows), not a
new widget. This is the "juice composes" payoff the palette plan predicted rather than a plan of
its own.

**Files:** `crates/noctmalia/src/app.rs`

### 2.1 Right-click surfaces a menu
Populated from `config.templates` (`config-plan.md`) filtered to ones whose `contexts` field
matches the region right-clicked (mail-body, mail-compose, person-field, event-description), or
names none. No matching templates opens no menu, rather than an empty one.

### 2.2 Menu region vs. titlebar region
This is a second, independent right-click surface from `chrome.rs`'s existing `SystemMenu` — one
triggers over content (the pager, a card, a description field), the other over the titlebar drag
region. They don't overlap in screen space and don't need to share implementation, but document the
boundary explicitly so a future reader doesn't assume there's one "the" context menu.

### 2.3 Running a template
Substitute the selection into `{selection}` and pass each argv element to `Command::arg()`
directly — **never** through `sh -c` or any string-interpolated shell invocation; that distinction
is what keeps command-injection out of this feature entirely, not something left to the
implementer's judgment. Spawn via `tokio::process::Command` (not `std::process::Command` — capturing
output and racing a timeout wants the async `Child`), capture stdout, and show it. Give the spawned
process a timeout so a hanging script can't freeze the UI waiting on it.

**Simplified during implementation:** always a toast, never a separate scrollable panel — output
past a fixed character cap is cut with a `…` rather than growing a second UI surface for it. A
panel for genuinely long output is real future work if a template ever needs one; nothing built so
far does.

## Stream 3: Test

### 3.1 Fake commands
Tests use `/bin/echo` or a tiny fixture script, never a real network-touching tool, keeping with
"internal mocks extensively." Assert: selection captured correctly, quoting survives a selection
containing quotes/spaces/newlines, output reaches the toast/panel state, a hung script is killed by
the timeout rather than blocking the test.

## Sequence integration

Cannot start before `config-plan.md`'s schema exists. No hard dependency on
`command-palette-plan.md`, though "run a context command" could later be exposed as a palette
command for free once its registry exists — worth doing, not required.

## Risks

- Shell-quoting is exactly the kind of place a security bug hides — Stream 2.3's argv-only rule is
  the mitigation, stated explicitly rather than assumed.
- The row/field-level "selection" (Stream 0's resolution) is a real scope reduction from what
  want.md pictured — free-text highlighting. Worth revisiting as its own plan if a real selectable
  text widget is ever built for `noctalia-iced`; not blocking this one.
