# noctmalia — mode & visual consistency

Date: 2026-09-14
Depends on: `command-palette-plan.md` (soft — palette-open is a mode too)
Status: **Shipped 2026-09-15.**

---

## Context

Two related want.md asks bundled here: "a cool and clear visible delineation between browse/normal
mode and compose/insert mode," and general "consistency with our visual language." Mail-plan §5
already settled the *interaction* model (mutt's three-focus-region shape, not vim's — "no insert
mode outside text widgets"); what's missing is that the *mode itself is invisible* — nothing on
screen today says whether a keypress will scroll the pager or type into a reply.

By the time this plan starts, there are at least **three** transient states to account for, not
two — enumerated explicitly here so none gets missed the way an earlier draft of this plan missed
the third:

1. **Browse** — index/pager focus, keymap active.
2. **Compose** — a focused text field (reply box, editor field, composer). Mail-plan §5: "exactly
   one iced-focused text widget at a time; every other focus is app state."
3. **Palette-open** (`command-palette-plan.md`) — the picker overlay has focus and swallows keys
   until `Escape`/`Enter`.
4. **Menu-open** (`context-commands-plan.md`, if it has shipped by the time this plan runs) — a
   context menu is showing and swallows the next click/key. If `context-commands-plan.md` hasn't
   shipped yet, this state doesn't exist yet either; check before assuming all four apply.

Designing the indicator after the palette exists (rather than before) avoids building it once for
two states and reworking it for three or four.

## Stream 1: A mode indicator

**Files:** `crates/noctmalia/src/app.rs`, `crates/noctmalia/src/surfaces/{mail,people,calendar}.rs`

### 1.1 Enumerate the actual states — resolved to three, not four
`context-commands-plan.md` shipped before this plan, but its "menu-open" state turned out not to
be a fourth state at all: Stream 2 of that plan reused `command-palette-plan.md`'s own
`Overlay`/`Picker` machinery for the context menu rather than building a separate primitive, so a
context menu *is* a palette-open state as far as the window is concerned — same `self.overlay`
field, same swallow-every-key behavior. The real three: **Browse** (nothing focused but the
keymap), **Compose** (a surface's own `composing()` — mail's open draft, people's or calendar's
open editor — is `Some`), and **Overlay** (`self.overlay.is_some()`, covering the palette,
quick-open, and every context menu alike).

### 1.2 The affordance — built
A small badge beside the surface switcher in the titlebar, styled with `theme::track` — the exact
container style the half-typed keymap sequence badge already used, so this reads as the same kind
of thing rather than a new visual idiom. `Browse` shows nothing at all: a badge that's always on
screen stops meaning anything. `Compose` shows "Compose" in `theme::palette().primary`; `Overlay`
shows "Command" in `theme::palette().tertiary`. Overlay wins when both are true at once (a palette
open over a draft) — what's on top of the stack is what the badge should describe.

**Simplified from the plan's own "existing motion primitives for the transition" language:** no
animated transition. The badge appears and disappears instantly. `docs/command-palette-plan.md`
and `docs/context-commands-plan.md` both introduced real motion elsewhere in this pass (the
picker's own layout); an animated mode badge is a legitimate future polish pass, not something
this one needed to block on.

## Stream 2: Visual language audit

**Files:** wherever surfaces diverge

### 2.1 Swept — no drift found worth fixing
Checked directly rather than assumed: every `Color` construction in `crates/noctmalia/src` outside
`noctalia-iced`'s own `theme.rs` goes through a palette role — there is no hardcoded hex or
`Color::from_rgb` anywhere in the app crate to begin with. The one thing that looked like drift on
first grep — `spacing(1)`/`spacing(2)`/`padding(0)` hairline values scattered across mail, people
and calendar rather than named constants — turned out to be the *same* value used identically
everywhere a label sits tight against its content, which is consistency without a name rather than
three surfaces disagreeing. Worth promoting to a named constant (`theme::HAIRLINE_GAP` or similar)
as a pure refactor sometime; not a bug this plan needs to fix, and inventing a change here only to
have swept something would be exactly the scope creep §2.1's own caution warned about.

### 2.2 Test
Widget-tree assertions (`tests/mail.rs`'s `render`-style pattern, aimed at the whole `App`):
browse shows no badge, opening the composer shows "Compose" and closing it returns to no badge,
opening the palette shows "Command", and a palette opened over an existing draft shows "Command"
rather than "Compose".

## Sequence integration

Landed last in the sequence, after every other plan had shipped — which is what let Stream 1.1
discover that context-commands-plan.md's "menu-open" folded into "overlay" instead of adding a
fourth state, rather than guessing at that ahead of time.

## Risks

- None outstanding. The scope-creep risk on the audit (2.1) resolved itself: the audit found
  nothing to fix, which is a valid outcome, not a sign the sweep wasn't thorough enough.
