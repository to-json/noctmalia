# noctmalia — mode & visual consistency

Date: 2026-09-14
Depends on: `command-palette-plan.md` (soft — palette-open is a mode too)

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

**Files:** `crates/noctmalia/src/ui.rs`, `crates/noctmalia/src/shell.rs`

### 1.1 Enumerate the actual states
Confirm which of the four states above are live in the tree at implementation time (palette is
certain; menu-open depends on `context-commands-plan.md`'s status) rather than assuming a fixed
count.

### 1.2 The affordance
Candidate: a thin accent-colored bar or dot in the titlebar (which already hosts the surface
switcher, per mail-plan §4) that changes color/state between the live modes — using existing
palette roles and the existing motion primitives for the transition, not a new bespoke indicator
style. Settle the exact visual in implementation, not here — this is a plan, not a mockup.

## Stream 2: Visual language audit

**Files:** wherever surfaces diverge

### 2.1 Sweep for one-offs, with a bounded list
"Audit the whole UI" is scope-creep-shaped by construction. Before starting, write down the
specific list of suspected drift (e.g. spacing that doesn't match `widgets::ROW_HEIGHT`, a color
that isn't one of the sixteen roles) found while drafting this plan's implementation — the sweep
fixes that list, not an open-ended search for more.

### 2.2 Test
Widget-tree assertions (`tests/mail.rs`'s style) that mode-affects-rendering: opening the composer
changes the indicator state; opening the palette does too; closing returns to browse.

## Sequence integration

Soft dependency on `command-palette-plan.md` for Stream 1's palette state; a real but softer
dependency on `context-commands-plan.md` for the menu-open state, resolved by 1.1 checking what's
actually shipped rather than assuming. Stream 2 can start anytime.

## Risks

- Scope creep on the audit — bounded explicitly by 2.1 rather than left open.
- If `context-commands-plan.md` ships after this plan, Stream 1 will need a follow-up pass for the
  fourth state; that's an acceptable, named gap rather than a reason to block on sequencing.
