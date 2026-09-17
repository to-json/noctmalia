# HTML mail — real fidelity, selective network

Date: 2026-09-15
Depends on: `docs/mail-plan.md` (partially reopens its §1 bet)

---

## Context

`docs/mail-plan.md` §1 made HTML mail render as downconverted Markdown, on purpose, because that
made "we load nothing from the internet" *structural* — there is no fetch path anywhere in the
pipeline, so remote content can't load, full stop (`mail.rs:13`, `mime/mod.rs`). That bet is worth
keeping for the default path: it's what lets compose stay symmetric (draft is Markdown, sent as
Markdown) and it's cheap.

But flattening real HTML mail to Markdown is lossy enough that it stopped looking like the email it
is. The ask now: a second, **opt-in**, per-letter "original formatting" mode with real CSS layout,
alongside making network access **selective** rather than impossible — a fetch gate the user can
open, instead of a wall with no door.

Real layout means an actual HTML/CSS engine, which iced does not have. `litehtml` was scoped once
already (mail-plan.md rejected it at ~a month of work) and is the right *shape* of tool — real CSS
layout, no JavaScript engine at all, unlike embedding an actual browser (`wry`/CEF/WebKitGTK/
Ultralight), which would reintroduce script execution against hostile sender-controlled input and
is the wrong tool regardless of cost. But "a month" was an estimate, not a plan, and **this repo has
never done FFI**: no `bindgen`/`cxx` crate anywhere in the workspace, and `flake.nix` has
`pkg-config` but no C++ stdenv/cmake wiring. The single biggest unknown isn't "how do we map
litehtml's output to iced" — it's "can we get litehtml linked and callable at all, in this
toolchain." That unknown is unproven, not merely hard, which is exactly why this plan starts by
proving or disproving it in a day, not by assuming it and building on top.

The `N remote images, none loaded` chip (`mail.rs:2203`) already exists as the *naming* half of a
trust gate — it currently triggers nothing. It becomes the *trigger* half too.

---

## Stream 0: Spike — is litehtml linkable, and what does it cost? (hard 1-day timebox)

**Problem**: The FFI cost is an old, unverified estimate. Don't build Streams 1-4 on a guess.

### 0.1 Get it compiling

Vendor `litehtml` (git submodule, pinned commit), add a minimal `cc`/`cmake` build step, add the
C++ stdenv + cmake to `flake.nix`'s devshell. Nothing else — just get the library to link into a
throwaway `fn main()`.

### 0.2 Smallest possible round trip

A minimal `document_container` FFI shim: hand litehtml a static HTML+CSS string (no images, no
fetch, no interactivity), call layout, get *something* paintable out (litehtml's own bitmap
canvas, or its draw-call list — don't decide the real rendering strategy yet, just prove a
rectangle comes out the other end).

### 0.3 Exit criteria — this is the whole point of the stream

If 0.1 and 0.2 are not both done by end of day: **stop**. Do not continue into Stream 1. Write down
exactly what broke (build system fight? binding complexity? something litehtml itself can't do?)
and fall back to the custom-bounded-renderer alternative that was on the table before litehtml was
picked — hand-rolled HTML→iced elements, pure Rust, no build-system risk, more code to own. Bring
that finding back before writing more of this plan's later streams.

### 0.4 If it works

Blit whatever came out of 0.2 into an iced `image::Handle` in a throwaway test surface — static,
no scrolling, no interactivity. This proves the round trip end to end (litehtml → bytes → iced
paints something) before any real integration work starts.

---

## Stream 1: Real litehtml integration (gated on Stream 0 passing)

**Problem**: Stream 0 proves it's possible. This makes it real.

**File(s)**: new `crates/litehtml-sys` (or similar), `flake.nix`, `Cargo.toml`

### 1.1 Build integration

Proper `build.rs`, pinned litehtml version, checked into the flake so `nix develop` and CI both
get it — no "works on my machine" C++ toolchain.

### 1.2 `document_container`

Font metrics (map to whatever font noctalia already ships — no new font loading), and the
resource-loading callback. This callback is the network chokepoint: litehtml must ask *us* for
bytes and never open a socket itself. See Stream 3 — this is where it plugs in.

### 1.3 Layout → paint (open question, decide here, not later)

Two options, pick one after Stream 0's spike shows which is realistic:
- Rasterize litehtml's own canvas to a bitmap, blit as one `image` widget (simpler, but text
  quality is whatever litehtml's own rasterizer gives you, and it double-rasterizes text iced could
  otherwise draw natively).
- Walk litehtml's draw calls and emit iced primitives directly (crisper text, hit-testing for free,
  more binding surface to write).

---

## Stream 2: Surface integration

**Problem**: Wire the new render mode into the actual mail reader without disturbing the default.

**File(s)**: `crates/noctmalia/src/surfaces/mail.rs`

### 2.1 Per-letter opt-in

A toggle in the letter header (next to the existing badges/chips) — "original formatting." Applies
only to the currently-open letter, computed on demand, never precomputed for the index or any other
letter. Matches the existing windowed-list discipline (only the open letter is ever fully
rendered).

### 2.2 Markdown stays the only path for the index/preview

No change to `Header`/list rendering, no change to compose. This plan only touches the single-letter
reading pane, and only when the toggle is on.

### 2.3 Link handling

litehtml's own hit-testing for links needs to feed the same `on_link_click`-shaped message markdown
mode already uses (`Reader::on_link_click`), so `\` and click-through behavior stay one code path
regardless of render mode.

---

## Stream 3: Selective network — the fetch chokepoint

**Problem**: Network access needs to go from "structurally impossible" to "off by default,
explicitly grantable," without becoming "the sanitizer's job" (it isn't one — nothing sanitizes a
live HTTP response).

**File(s)**: new module, likely `crates/noctmalia/src/mime/fetch.rs` or similar

### 3.1 One fetch function

Default-deny. Used for both remote images in either render mode AND anything litehtml's resource
callback (1.2) asks for. No second path anywhere that can reach the network.

### 3.2 Trust decision UI

Reuse the existing chip (`mail.rs:2203`) as the grant trigger instead of pure decoration. Grant is
per-message. **Open question, not resolved here**: does a grant get remembered per-sender (so the
same newsletter doesn't re-prompt every issue), and if so, where does that preference live —
Thunderbird already has no concept of this, so it would be new state this app owns, which cuts
against the "nothing is stored outside Thunderbird" principle mail.rs's own doc comment states.
Needs a decision before 3.2 is built, not during.

### 3.3 Two grants, not one

"View original formatting" (real layout, sender-controlled visual structure, fonts/spacing/hidden
text — a smaller leak) and "load remote images" (an actual outbound HTTP request, the classic
read-receipt beacon) are different risk levels. Keep them as two separate, separately-grantable
actions rather than one button that does both.

### 3.4 Verify litehtml's own boundaries, don't assume them

litehtml doesn't execute JavaScript or forms by design, but confirm that holds for whatever feature
set actually gets compiled in (1.1's build flags), and still strip anything script/form/frame-
adjacent at the same sanitizer boundary `mime/html.rs` already applies for the Markdown path before
HTML reaches litehtml at all. Belt and suspenders, not "the engine promises so we're done."

---

## Stream 4: Tracker/security surface stays honest

**Problem**: `trackers` (computed today in `mime/mod.rs` by scanning the never-loaded image/link
list) and the security panel's "blocked" language both assume nothing ever loads. Once Streams 1-3
land, that assumption is false for opted-in letters.

**File(s)**: `crates/noctmalia/src/mime/mod.rs`, `crates/noctmalia/src/surfaces/mail.rs`

### 4.1 Tracker count is mode-independent

Compute `trackers` from the raw HTML the same way regardless of which render mode the user picks —
HTML mode doesn't get a pass on detection just because it renders itself.

### 4.2 Security panel language

Once a grant has been made, "blocked" is no longer true. The panel needs to say what *was* loaded
under an explicit grant, not just what would have been blocked by default.

---

## Sequence integration

Reopens part of `docs/mail-plan.md` §1's structural-safety bet, but only for letters where the user
opts in — the default index/compose/everything-else path is untouched. No other active plan
depends on or blocks this one.

## Risks

- **The FFI estimate is old and unverified.** Zero prior FFI in this repo. Mitigation: Stream 0 is
  a hard 1-day gate with a named fallback — this plan does not authorize spending a month finding
  out the hard way.
- **litehtml's "no JS" is being trusted, not verified.** Mitigation: 3.4 checks actual build flags
  and keeps the existing sanitizer allowlist as a second layer regardless.
- **Per-sender grant memory is new state outside Thunderbird**, cutting against an explicit design
  principle. Left as an open decision (3.2), not quietly resolved either way.
- **Rendering-path choice (1.3) affects text quality and how much litehtml binding work is
  needed.** Left open, to be settled once Stream 0's spike shows what's actually realistic.

---

## Findings (2026-09-15 — Streams 0-2 done, ahead of the "not a month" constraint)

**Stream 0 passed the same day**, not the 1-day budget's worst case: nixpkgs at the pinned
`flake.nix` revision already carries a prebuilt `litehtml` (0.9) and its `gumbo` HTML5-parser
dependency — no vendoring, no cmake source build, no month-long FFI-from-scratch fight. It ships
headers and a static lib but no `.pc`/pkg-config file, so `flake.nix` sets `LITEHTML_ROOT`/
`GUMBO_ROOT` directly and `litehtml-sys/build.rs` reads those rather than shelling out to
pkg-config or cmake. `crates/litehtml-sys/src/shim.cpp` implements the whole
`litehtml::document_container` interface and exposes one `extern "C" litehtml_render` function
returning a flat list of background/border/text primitives — proven with two unit tests
(`a_coloured_box_and_a_paragraph_produce_primitives`, `empty_document_still_renders_without_crashing`)
that actually link and run litehtml, not just compile against its headers.

**Stream 1.3's open question resolved itself**: translate-to-primitives, not rasterize-to-bitmap.
The shim already had to walk litehtml's draw calls to prove Stream 0's exit criterion, and those
calls turn directly into iced `canvas::Frame::fill_rectangle`/`fill_text` operations — no double
rasterization, no bitmap blit.

**Stream 1.2's font-metrics gap turned out to matter immediately, not later.** The first working
render (fixed per-character width estimate for `text_width`) laid text out with real colors and
structure but wrapped words wrong badly enough to overflow the reading pane — visibly broken, not
a footnote. Fixed by extending the shim's `extern "C"` surface with an optional measurement
callback (`litehtml_render(..., measure, measure_ctx)`; safe wrapper `render_with_measure` in
`litehtml-sys`), and wiring it in `surfaces/html_view.rs` to `iced_graphics::text::Paragraph` —
iced's own shaper, the same font the rest of the app uses, called synchronously during layout
with no live renderer required. Verified both that the callback fires
(`a_measure_callback_changes_the_layout`) and, visually, via `tests/shots.rs`'s new
`mail-original` shot: text now wraps within the pane using the app's real typeface. A tempting
alternative — clamping each text run's `canvas::Text::max_width` to litehtml's own box — was tried
and made things *worse* (mid-word breaks, because the box was sized by the pre-measurement guess
and is reliably narrower than the real shaped text); left in the code as a comment so nobody
retries it.

**Stream 2 (surface integration) landed as `Showing::Original`**, not a separate boolean — it
reuses every bit of plumbing `Showing::Security`/`Headers`/`Source` already had (toggle-off
behavior, the action-bar icon button, the right-click command entry, the `o` key in both INDEX_KEYS
and PAGER_KEYS). The button only appears when `letter.body.raw_html` is `Some` (i.e. the letter
actually arrived as HTML — `mime::Flavour::Html`), so plain-text and Markdown mail never offer a
mode that would show nothing.

**Streams 3 (network chokepoint) and 4 (tracker/security wording) are not built.** 4.1 already
holds by construction — `trackers`/`images`/`misleading` are computed once in `mime::render`
regardless of which mode later displays the letter, so detection was never mode-dependent to begin
with. Stream 3 is a materially separate feature (an HTTP client, async image fetch, a consent
model, wiring litehtml's `load_image`/`get_image_size` to a second layout pass once bytes arrive)
and was treated as out of scope for "litehtml support" specifically — litehtml itself still never
touches the network (`shim.cpp`'s `load_image`/`import_css` remain deliberate no-ops), so nothing
about landing this weakened "we load nothing by default." Real image loading in either render mode
is future work.

## A real crash, found the same day it shipped (2026-09-15)

Live-testing against a real Gmail account (a Google "you granted Thunderbird access" notification,
`multipart/alternative` with a substantive plain-text part *and* an HTML one — see the "plain wins
by default" fix above) segfaulted litehtml three times in a row, always inside litehtml's own
`html_tag::draw_background`, deep in *its* code, nowhere near the actual bug.

Isolating the exact captured HTML into a standalone repro (`litehtml_sys::render`) didn't reproduce
it at all — not at any viewport width from 60px to 900px. The live app and the isolated repro
differ in exactly one way that matters: the live app measures text through
`iced_graphics::text::Paragraph`, which takes a lock on iced's global font-system `RwLock`; the
repro's plain `render()` doesn't measure anything, it guesses. That pointed at the real bug: if
that lock ever panics on the live render thread (contention, poisoning, anything), the panic fires
*inside* `litehtml_sys`'s measurement trampoline — a `extern "C" fn` called by litehtml, from
litehtml's own C++ call stack. A Rust panic unwinding through foreign stack frames is undefined
behaviour, not a clean abort, which is exactly why the crash surfaced somewhere else entirely: not
where the panic happened, but wherever litehtml's now-corrupted internal state next did something
that touched memory.

Fixed at the one place it can be fixed for good: the trampoline in `litehtml-sys/src/lib.rs` wraps
the caller's `measure` in `std::panic::catch_unwind` and falls back to the fixed per-character
guess on panic, so a caller's bug (whatever it turns out to be, and the actual lock-contention
trigger was never fully chased down) degrades to a wrong width instead of a segfault. Verified with
a test that panics on purpose (`a_panicking_measure_callback_does_not_crash_the_process`) in both
debug and `--release` — this repo has no `panic = "abort"` set anywhere, so `catch_unwind` is live
in the shipped binary too. The actual captured email is now a permanent fixture
(`crates/noctmalia/src/surfaces/fixtures/security-alert.html`) exercised by
`a_real_captured_gmail_message_renders_without_crashing` — real Gmail markup, real `<img>` tags,
real nested tables, not just the synthetic corpus.

**Still not root-caused**: *why* the font-system lock panics in the live app at all. The fix makes
it survivable, not diagnosed. If it recurs, that's the next thread to pull.

## The recurrence: catching the panic isn't the same as un-poisoning the lock (2026-09-16)

"It recurs" arrived as "we crash a lot" — worse than the original bug, not the same one. The
trampoline's `catch_unwind` does exactly what it says: it stops that one panic from unwinding into
litehtml's C++ stack. It does nothing about `iced::advanced::graphics::text::font_system()` itself
— a `std::sync::RwLock`, shared by every `iced::widget::text` in the window, not just this reader —
which poisons itself the moment a panic unwinds through a held guard, whether or not something
downstream catches that panic afterward. `Paragraph::with_text` takes that lock with
`.expect("Write font system")`; once poisoned, every ordinary widget that calls it — the message
list, a subject line, anything with text — panics too, unprotected, on its very next redraw. One
bad HTML message poisoning the lock once was enough to turn every following screen into a crash,
which is what "a lot" actually meant: not litehtml crashing repeatedly, the rest of the app doing it
after litehtml poisoned something it doesn't even know exists.

Reproducing the original trigger under `cargo test` — including against the real system font
(`the_real_system_font_against_the_same_fixture_does_not_crash`, since the existing regression test
measured against `Font::DEFAULT`, not what `main.rs` actually installs via
`font::adopt_system_families()`) — still doesn't panic outside the live app. That question is
exactly where the last pass left it: open. What changed is that it no longer needs answering to
stop the cascade. `measure` in `crates/noctmalia/src/surfaces/html_view.rs` now wraps its own call
in `catch_unwind` and, on panic, calls `font_system().clear_poison()` before falling back to the
guessed width — the one place in the call chain that both sees the panic and knows the lock exists.
`measure_clears_a_poisoned_font_system_lock_instead_of_leaving_it_for_everyone_else` poisons the
real global lock from another thread, calls `measure`, and then constructs an ordinary
`Paragraph::with_text` exactly as any unrelated widget would — proving the rest of the window
survives past the one bad call, independent of ever diagnosing what the first panic actually was.

**Still not root-caused, still open**: the same question as before, now provably separate from the
cascade it used to cause. If it recurs, only the one HTML message will glitch — and this is where to
look for what triggers the underlying panic in the first place.
