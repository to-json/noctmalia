# noctmalia — context commands

Date: 2026-09-14
Depends on: `config-plan.md` (hard — templates are config entries)

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

**Files:** a throwaway probe against `iced::widget::markdown::Viewer`, not shipped code.

### 0.1 Verify
Confirm whether the markdown `Viewer` (or the underlying widget it renders through) exposes a
selection span the app can read on right-click. Check both a plain-paragraph selection and one
crossing element boundaries (e.g. spanning into a list item).

### 0.2 If yes
Proceed with Streams 1–3 as drafted below, selection scoped to whatever span the Viewer reports.

### 0.3 If no
Fall back to operating on **the whole open message** rather than a highlighted span for mail-body
contexts specifically — person-field and event-description contexts (Stream 1) are ordinary iced
text widgets and are unaffected either way. Update Stream 1/2 below to match before proceeding;
don't discover this mid-implementation.

## Stream 1: Selection plumbing

**Problem:** none of the three surfaces currently expose "what text is selected" as state a menu
action could read (contingent on Stream 0's answer for the mail pager specifically).

**Files:** `crates/noctmalia/src/surfaces/{mail,people,calendar}.rs`

### 1.1 A selection type
Wherever text is already selectable (a person card's fields, an event's description, and the
pager if Stream 0 confirms it), capture the selected span as a `String`, surfaced through each
surface's existing state — not a new cross-cutting selection manager, since iced's own text widgets
already track selection internally and this only needs to read it out on right-click.

## Stream 2: The context menu

**Problem:** no context-menu primitive exists anywhere in the stack today — checked directly:
`noctalia-iced`'s `chrome.rs` has only the OS-level `SystemMenu` bound to a right-click on the
*titlebar*, nothing for content regions.

**Files:** `../noctalia-iced` (a new generic context-menu primitive, following the same "juice
lives in the library" precedent as `command-palette-plan.md`'s `Picker<T>`)

### 2.1 Right-click surfaces a menu
Populated from `config.templates` (`config-plan.md`) filtered to ones whose `contexts` field
matches the region right-clicked (mail-body, person-field, event-description).

### 2.2 Menu region vs. titlebar region
This is a second, independent right-click surface from `chrome.rs`'s existing `SystemMenu` — one
triggers over content (the pager, a card, a description field), the other over the titlebar drag
region. They don't overlap in screen space and don't need to share implementation, but document the
boundary explicitly so a future reader doesn't assume there's one "the" context menu.

### 2.3 Running a template
Shell-quote the selection into the configured argv as a single `Command::arg()` element — **never**
through `sh -c` or any string-interpolated shell invocation; that distinction is what keeps
command-injection out of this feature entirely, not something left to the implementer's judgment.
Spawn, capture stdout, show it (toast for short output, a small scrollable panel for long). Give the
spawned process a timeout so a hanging script can't freeze the UI waiting on it.

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

- Stream 0 failing (no selection API) narrows this plan's mail-body scope; decide the fallback
  before Streams 1–3, not during them.
- Shell-quoting is exactly the kind of place a security bug hides — Stream 2.3's argv-only rule is
  the mitigation, stated explicitly rather than assumed.
