# The want.md sequence

`want.md` is the wishlist; this is what building it looks like. Six plans, each independently
landable and reviewable, in the shape `docs/mail-plan.md` already established for this project.

## The plans, in order

| # | Plan | Depends on | Status |
|---|---|---|---|
| 1 | [`reminders-plan.md`](reminders-plan.md) | None | Shipped 2026-09-15 |
| 2 | [`command-palette-plan.md`](command-palette-plan.md) | None | Shipped 2026-09-15 |
| 3 | [`config-plan.md`](config-plan.md) | 2 (soft) | Not started |
| 4 | [`context-commands-plan.md`](context-commands-plan.md) | 3 (hard) | Not started |
| 5 | [`scripting-socket-plan.md`](scripting-socket-plan.md) | 2 (hard) | Not started |
| 6 | [`mode-visual-plan.md`](mode-visual-plan.md) | 2 (soft) | Not started |

```
Reminders (no deps) ─────────────────────────────────────────► ships first

Command Palette & Quick-Open (no deps)
   │  produces: fuzzy-match core (noctalia-iced), command registry, People rename
   ├──► Config System            (soft: schema should match the registry's Command shape)
   │        └──► Context Commands (hard: templates live in config.toml)
   ├──► Scripting Socket          (hard: fronts the same command registry)
   └──► Mode & Visual Consistency (soft: palette-open is itself a mode worth designing for)
```

Reminders ships first: near-zero risk, already half-built. Command Palette ships second because
it's the priority pick from want.md's own emphasis, and Config/Socket/Mode can then proceed in
parallel once it lands; Context Commands waits on Config.

## Cross-cutting rules

These apply to every plan below and are not restated per-plan:

- Every user-facing action added gets a test. Prefer the project's existing mock pattern
  (`tools/fake-bridge.py` for the Thunderbird bridge; an equivalent in-process fake for anything
  new) so the suite stays fast.
- Config and command names carry their own meaning; no explainer-comment blocks. An example file
  is complete, not a stub.
- Every new visual affordance uses the sixteen palette roles and existing motion primitives
  (`bouncy`, `motion::Replay`) — nothing bespoke-colored.
- Historical build-record docs (`docs/mail-plan.md`, and this file once a plan below ships) are
  never edited to retcon their own past tense. A rename or superseding decision gets a dated
  addendum note, the same way `docs/design.md`'s header already does — not a rewrite of what was
  actually built and named at the time.

## Definition of done

want.md's own closing line is the umbrella goal none of the six plans states directly: *"a mail
experience good enough to leave mail.app from macos."* That's not a feature to build, it's the bar
the other six are for. Once Plan 6 ships, that's the moment to actually ask the question — daily
driving mail here, does it beat Mail.app — rather than assuming six shipped plans add up to yes.
