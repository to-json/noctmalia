# noctmalia — Thunderbird as a daemon, Noctalia-toolkit UI

> **2026-09-13:** see `docs/findings.md` for everything verified. Two things below are now settled
> differently: **mailnd is gone** — the UI binds the socket itself (findings §2) — and **the UI is
> Rust on iced**, built on the sibling repository `../noctalia-iced`, not the C++ toolkit. The
> product direction, the principles and the hard problems all still stand; the imagined C++ API
> sketches are kept only as a record of the shape that was wanted.
>
> **2026-09-14: the mail half of this document is superseded by `docs/mail-plan.md`, which was
> built.** Three answers here turned out to be wrong rather than merely unfinished. There is no
> HTML engine and there will not be one: every letter becomes Markdown (mail-plan §1). There is no
> threader and no search index of ours: Thunderbird has both, in Gloda, and has all along
> (mail-plan §2, §3). And mail is not a directory of files, because Thunderbird keeps no bodies as
> files to make a directory out of — `findings.md` "What the profile retains" is the measurement
> that settled it.
>
> **2026-09-14: the Calendar section below is built**, against the calendar Experiment already
> vendored in `tbd/bridge`. It shipped as a UI-and-glue pass, the same shape mail and contacts were:
> `src/ical.rs` (a hand-rolled `VEVENT` parser/builder, modelled on `src/vcard.rs`'s approach to the
> same RFC 5545/6350 grammar rather than a new crate), `src/calendar.rs` (bridge calls), and
> `src/surfaces/calendar.rs` (the rail, month/week/day/agenda, and the event editor). Two answers
> below turned out differently once there was code: a calendar's colour is *assigned* from the
> sixteen palette roles rather than drawn from Thunderbird's own arbitrary hex (design language,
> not a gap), and recurrence editing is five gcal-style presets rather than the "agenda strip in
> the reader rail" sketched below — a full month/week/day view was what got asked for once mail and
> contacts existed to compare it against. Cut from this pass, deliberately: invite RSVP (attendees
> are shown read-only and round-tripped, never edited) and a Tasks view — see the plan this was
> built from for the reasoning, same shape as mail-plan.md's own cuts.
>
> **2026-09-15: there is one program.** noctmalia spawns and stops its own Thunderbird natively, in
> a systemd user scope, from a pinned build it fetches once — `docs/one-program-plan.md`, and
> `findings.md` §11 for what the native bring-up found. The container this document assumes
> everywhere below (`tbd`, compose overlays, uid 1000, "only one client may hold the socket" as a
> thing the user meets) is gone. The transport and the extension are unchanged.
>
> **2026-09-15: OAuth2 (Gmail, IMAP) is in progress** — see `docs/oauth-plan.md`. The plan's central
> question is answered: Thunderbird's own baked-in Gmail OAuth client works with no Google Cloud
> project of our own, confirmed by completing real consent through the account wizard in a
> `TBD_MODE=gui` session and then reattaching headlessly against the same profile. What's left is
> the bridge/tooling work (Streams 2-4) plus confirming a token survives a fresh, never-consented
> profile. Real Gmail mail also surfaced and led to fixing a genuine mail-surface bug along the
> way: `message/rfc822` parts (forwards, bounces) weren't recognized as containers — see
> `crates/noctmalia/src/mime/mod.rs`.
>
> **2026-09-15: real HTML rendering ("original formatting") landed** — see
> `docs/html-mail-plan.md`. Mail's default renderer is still the Markdown downconversion this
> document originally settled on (§1 above); a new opt-in per-letter mode, toggled with `o` or the
> eye icon (`Showing::Original`), lays the letter out with actual CSS via `litehtml` instead. New
> crate `crates/litehtml-sys` is the FFI boundary — a C++ shim implementing litehtml's
> `document_container` and one flat `extern "C"` render call, linked against nixpkgs' own
> prebuilt `litehtml`+`gumbo` (no vendoring, no cmake source build) — and `surfaces/html_view.rs`
> paints its output onto an iced `canvas`, with line-wrapping driven by iced's own text shaper
> (`iced_graphics::text::Paragraph`) rather than a guess, so it wraps against the same font that
> actually paints it. litehtml still never touches the network — remote images stay unloaded in
> both render modes, same as before — so this changes rendering fidelity only, not the "we load
> nothing by default" property. Selective network access (the fetch chokepoint / trust-grant UI
> the plan also describes) is not built yet.

Status: the transport, contacts, **mail** and **calendar** exist and are tested against a real
headless Thunderbird (`tools/smoke.sh`) or, for calendar's UI-level bridge calls, against
`tools/fake-bridge.py` (`tests/calendar.rs`) — recurrence expansion itself is only proven against
the real Experiment, since the stand-in does not implement it (`tools/fake-bridge.py`'s own
`Store.items_in_range` says so).

## Verified in a container (2026-09-13)

| Claim | Result |
|---|---|
| TB runs `--headless` in `debian:trixie-slim` on arm64 | yes — TB 140.15.0esr builds a full profile |
| Sideloaded MailExtension runs headless | yes — `profile/extensions/<id>.xpi` plus `autoDisableScopes=0` |
| Background script reaches a host process | yes — `fetch()` to `127.0.0.1` |
| `accounts.list`, `folders.create`, `messages.import`, `query`, `getFull` | all work |
| A Local Folders account can be pre-seeded from `user.js` | yes |
| Marionette answers on :2828 | yes |
| Native messaging works under headless TB | yes — two-way; the host can push unprompted |
| Calendar API in core TB (140.15 → 156.0b3, comm-central tip) | **no** — `ext-mail.json` registers no calendar namespace |
| Calendar via the upstream Experiment, headless, on TB 140.15 arm64 and 155.0.1 amd64 | yes — calendars query/create; items create/get/update/remove; range query with recurrence expansion; onCreated/onUpdated/onRemoved; timezones |

Not verified yet:
- IMAP sync and OAuth in headless mode
- `messages.sendMessage`, which needs TB 153; Debian ships 140

## Architecture

As built (2026-09-13):

```
┌──────────────── container: tbd ────────────────┐
│ thunderbird --headless (MV2, persistent bg)    │
│   └ noctmalia-bridge.xpi ── native messaging ──┐│
│       nm-shim (stdio ↔ unix socket, stateless) ┘│
│         │ connects out                         │
└─────────┼──────────────────────────────────────┘
          │  $XDG_RUNTIME_DIR/noctmalia/bridge.sock   (bind mount, compose.ui.yaml)
┌─────────▼──── host ────────────────────────────┐
│ noctmalia (Rust, iced + noctalia-iced)         │
│   noctmalia-bridge: listens, matches replies   │
│   by id, streams events                        │
│   native Wayland window, Noctalia chrome       │
└────────────────────────────────────────────────┘
```

The daemon in the middle is gone: the app is the socket's owner. One client at a time, which is
enough until a second surface exists (findings §2).

### Why a daemon was planned, and what deferring it costs
The gaps below are real; the decision was that none of them block contacts, calendar or a mail read
path, and that a hub can be grown out of `crates/noctmalia-bridge` when a second client needs one.

- TB's extension API has gaps that a daemon would fill:
  - **No thread API.** mailnd builds threads itself (JWZ threading over `References`/`In-Reply-To`).
  - **`messages.query({fullText})` is a linear MIME scan, not Gloda.** mailnd keeps a tantivy index, fed incrementally from `getRaw` and `onNewMailReceived`.
  - **No snooze, undo-send, send-later UX or muting.** mailnd owns these and stores state as TB tags or hidden folders, so it survives on the IMAP server.
- **Several clients share one daemon.** The main window, bar widget, triage panel and launcher all attach to one socket and get one stream of events.
- **Thunderbird can restart without the UI noticing.** mailnd caches and replays state.
- **Transport is native messaging through a stateless shim.**
  - Thunderbird launches the shim itself. Only the extension ID listed in `allowed_extensions` can connect, so no port or token is needed.
  - The shim just pipes stdio to mailnd's socket. mailnd's lifetime stays independent of Thunderbird's.
  - Shim and mailnd are split because a native-messaging host is Thunderbird's child process and dies whenever Thunderbird restarts.
  - Rejected: a WebSocket from the extension out to mailnd (needs a token, and other local processes can connect to the port) and an Experiment server (privileged code).
  - Firefox's documented limit on host → extension messages is 1 MB. Send attachments from mailnd in chunks and let the extension reassemble them (not yet tested).
- **Prior art that is not a daemon:** community-plugins `thunderbird-companion/native-host/host.py`.
  - The extension polls a file-based command queue every 1.5 s, and the Luau service polls the files every 2 s.
  - Each change rebuilds a snapshot of all unread mail and ships the newest 50 headers.
  - There are 7 fixed commands, results come back only as the latest one written to a file, and message bodies are never read.
  - Reply and open go through Thunderbird's own windows, which a headless Thunderbird doesn't have.
  - Worth copying: the native-messaging registration and the extension-ID allowlist.
- **Why MV2:** MV3 event pages are killed when idle, which drops the socket.

### RPC surface a daemon would expose (not built; the app calls the bridge directly)
```
accounts.list  folders.tree  counts.get
threads.list {view, query?, cursor, limit}      # views: unified-inbox, flagged, snoozed, newsletters, folder:<id>
thread.get {threadId}                            # message headers + collapsed/expanded state
message.body {id, prefer: html|text, remote: block|allow}
message.raw {id}   attachment.get {id, part}
act {ids[], op: archive|trash|move|read|unread|flag|tag|snooze|mute, arg?}  → {undoToken}
undo {undoToken}
compose.send {draft, delaySec}  → {sendToken}    # undo-send window held in mailnd
compose.saveDraft  contacts.suggest {prefix}
search {q}   # from: to: has:attachment is:unread before: after: in: "phrase"
events: mail.new  thread.changed  counts.changed  sync.state  send.progress  bridge.state
```
- **Durable message IDs:** mailnd keys messages by `headerMessageId` plus folder. Extension message IDs are per-session only.

## UI

### Main window
```
┌ rail ─────┬ threads ───────────────────────┬ reader ──────────────────────────────┐
│ ◉ Inbox 12│ ▌A  Alice Chen        10:02 ⎘ 3│ probe message                         │
│ ★ Flagged │    probe message — hello from…  │ ┌ Alice Chen · 10:02 ───────── ⋯ ┐   │
│ ⏾ Snoozed │ ▌G  GitHub             09:40   │ │ hello from inside the daemon    │   │
│ ✉ Newslet.│    [noctalia] PR #412 merged…  │ └─────────────────────────────────┘   │
│ ── acct ──│ ▌B  Bank                 Mon   │ ┌ You · 10:05 (collapsed) ────────┐   │
│ ▸ work    │    Statement ready             │ ├──────────────────────────────────┤  │
│ ▸ personal│                                │ │ reply inline…            ⏎ Send │   │
└───────────┴────────────────────────────────┴───────────────────────────────────────┘
 ⌘K palette · j/k move · e archive · s snooze · r reply · / search · g i inbox
```

**Principles**
- **Threads first; no three-pane message soup.** Replies are written inline at the bottom of the thread, not in a separate compose window. A full compose window exists only for new mail.
- **Keyboard first, mouse complete.** Every action is in the palette (`search_picker`), and bindings can be changed with `keybind_recorder`.
- **Blends in by construction.** Only the 16 palette roles and `Style::*` tokens are used, never fixed colors, so wallpaper and palette changes apply live.
- **Optimistic actions.** The row animates out immediately and the RPC runs behind it. A failed RPC rolls back with an error toast. Every destructive action gets an undo toast.

### Widget mapping (iced + noctalia-iced)
| Surface | Controls |
|---|---|
| Rail | `column` of `button`s with `theme::button_style(Tab/Selected)`; `widgets::collapsible` per account |
| Thread list | `scrollable` of row buttons, as `contact_row` does today. iced has no virtual list; large folders will need paging or a custom widget. |
| Reader | `scrollable` of `widgets::collapsible` cards; body in **an HTML view that does not exist yet** |
| Inline reply | iced `text_editor` plus `widgets::action`; `pick_list` for the from-identity |
| Search | `text_input` with a leading icon (`text_input::Icon`), results reuse the list |
| Snooze | a menu of presets; iced has no calendar widget, so a custom date needs one |
| Undo send / undo archive | a notice bar with `widgets::countdown_ring` |
| Attachments | a wrapping row of tiles; iced drag-and-drop is limited to window file drops |
| Settings | `widgets::toggle`, `widgets::segmented`, `widgets::stepper`, `widgets::setting` rows |
| Sync state | `widgets::spinner` in the rail footer |

### Sketch in the imagined C++ API (historical — the app is iced now)
```cpp
auto row = std::make_unique<ThreadRow>();
row->setOnSwipe([this, id = t.id](SwipeDir d) {
  auto op = d == SwipeDir::Left ? Op::Archive : Op::Snooze;
  anim_.animate(row->height(), 0, Style::animNormal, Easing::EaseOutCubic,
                [r = row.get()](float h) { r->setFixedHeight(h); },
                [this, id, op] { list_->remove(id); });
  mail_.act({id}, op).then([this](UndoToken tok) {
    toasts_.show(Toast{}.text(opLabel(op)).countdown(5s).action("Undo", [=] { mail_.undo(tok); }));
  }).fail([this, id](auto) { list_->restore(id); });
});
mail_.onEvent<ThreadChanged>([this](auto& e) { list_->patch(e.threadId, e.delta); });
```

### Shell integration (noctalia plugin; prior art: community-plugins `thunderbird-companion`)
- **Bar widget:** unread glyph with a badge. Left-click opens the triage panel; middle-click focuses the main window.
- **Triage panel** (layer-shell): today's unread threads. Offers archive, snooze and a one-line quick reply without opening the app.
- **Notifications:** sender and subject, with Archive / Reply / Mark read actions. Muted threads and newsletters never notify.
- **Launcher provider:** `m <query>` searches mail through mailnd; Enter opens the thread.
- The plugin API is Luau `ui.*`. It talks to mailnd over its HTTP/stream API, which is a second listener on the same daemon.

## Calendar

> Built as `src/surfaces/calendar.rs`, directly against the bridge — see the note at the top of
> this document. The backend research below (the Experiment, its API surface, its gotchas) is
> exactly what shipped and is still accurate; the **RPC additions** and **UI** sketches below are
> not — there is no `mailnd` to add an RPC to (it was cut, see the note above the "Status" line),
> and the UI is month/week/day/agenda plus an event editor, not an agenda strip.

**No release ships a calendar extension API.**
- Checked the `omni.ja` of TB 140.15esr, 153.2.0esr, 154.0, 155.0.1 and 156.0b3, plus `ext-mail.json` at comm-central tip.
- Tracking: meta bug 1627205 (NEW). Dependencies 2035321 (calendars/items) and 2042709 (timezones) are ASSIGNED with no milestone and no activity since 2026-07-03.
- The March 2026 digest aimed to "enhance the Calendar API ahead of the next ESR"; nothing landed in 153.

**Use the upstream Experiment** (`thunderbird/webext-experiments/calendar`, v2.2.0; pinned commit in `spike/calendar-experiment-probe/UPSTREAM_COMMIT`).
- Release TB still loads Experiments; the Monthly-channel deprecation was pushed back a year (June 2026 digest).
- Vendor it into the bridge XPI.

**API surface** (MV2):
- `calendar.calendars`: query, get, create, update, remove, clear, synchronize, plus events.
- `calendar.items`: get, query `{calendarId, type, rangeStart, rangeEnd, expand}`, create, update, move, remove, plus onCreated/onUpdated/onRemoved/onAlarm.
- `calendar.timezones.getDefinition`.
- `calendar.provider` is only needed if we implement our own calendar backend.

**Gotchas found in the spike:**
- `calendar_provider` must be declared even if unused. Its startup hook registers the `resource://experiments-calendar-<uuid>` alias that the calendars/items modules import; without it every call fails with "An unexpected error occurred".
- Items are raw iCal or jCal: `{type, format: "ical"|"jcal", item}`. The upstream `background.js` example using `title`/`startDate` is stale.
- `expand: true` returns one entry per occurrence, each with `instance` set. mailnd can serve agenda views directly; no RRULE engine needed in the UI.
- The Experiment is privileged code. It ships in our own XPI in our own container, so accept it, but pin the upstream commit and re-run the spike on each TB bump.

**RPC additions:**
```
cal.list  cal.items {calendarIds[], start, end}  cal.item.put {calendarId, ical}  cal.item.remove
events: cal.changed  cal.alarm
```

**UI:**
- Agenda strip in the reader rail: today and next events via `VirtualListView`.
- Invites in mail (text/calendar parts) get inline accept/decline cards that write through `cal.item.put`.
- Noctalia surfaces: a bar clock popover with the agenda, and alarm notifications from `cal.alarm`.

## Hard problems

1. **HTML mail rendering.** The toolkit has no HTML engine.
   - Plan: an `HtmlView` widget wrapping **litehtml**, a C++ HTML/CSS layout engine with a pluggable drawing backend. It fits cairo/pango.
   - mailnd sanitizes the HTML (ammonia): scripts and forms removed, remote images blocked by default and proxied when allowed.
   - Dark mode: recolor only messages that set no explicit background. Newsletters keep their own light card on a `surface` backdrop.
   - Fallback: an "as text" toggle that uses `MarkdownView`.
   - Rejected: screenshotting Gecko output. It gives no text selection or links.
2. **Composer.** Needs a multi-line rich `TextEdit`: IME, selection, undo, paste as plain text. The draft model is markdown-ish; mailnd produces multipart/alternative (text + HTML). This is the biggest toolkit addition.
3. **First login / OAuth headless.** Bootstrap once by running the same container non-headless with the Wayland socket mounted and doing TB's own account setup. After that, run headless. The profile volume holds `logins.json` and `key4.db`. The alternative is `mailnews.oauth.useExternalBrowser` (TB 151+).
4. **TB version.**
   - `messages.sendMessage` (windowless send) needs TB ≥153. Debian trixie ships 140.15 ESR, which would need a compose-tab workaround.
   - Plan: install a Mozilla release tarball in the image.
   - Check whether an official linux-aarch64 build exists. Otherwise use amd64 under emulation on this Mac.
5. **Initial sync and indexing cost.** The first index over large IMAP accounts is slow through `getRaw`. mailnd indexes newest messages first and serves `search` from the partial index with a "still indexing" hint.

## Toolkit asks

Most of the original list is answered by noctalia-iced: the app window and chrome, theming, scroll
views, text inputs, toggles, segmented controls, the countdown ring and the spinner all exist, and
`Task::perform` is the async → UI adaptor. What is still missing:

- **An HTML mail view.** Still the hard problem; iced has no HTML engine. litehtml behind a custom
  widget remains the plan, now with an iced `Renderer` backend rather than cairo.
- **A rich multi-line editor.** iced's `text_editor` gives editing, selection and IME but not rich
  text; the draft model being markdown-ish makes that survivable.
- **Swipe gestures on rows.** Needs a custom widget wrapping the row's mouse events.
- **A virtual list.** Fine to skip for contacts; not fine for a 50k-message folder.

## Build order
1. ~~`tbd` image~~ — **done 2026-09-13** (`tbd/`, `compose.yaml`, `tools/smoke.sh` PASS).
   - TB 155.0.1 x86_64 tarball, SHA-512 pinned.
   - Bridge XPI: native messaging, calendar Experiment, `noctmalia` Experiment.
   - Stateless shim; mailnd stub.
   - Verified against GreenMail: headless IMAP fetch and `onNewMailReceived`, `getFull`, `messages.sendMessage` over SMTP, flag updates and events, calendar CRUD with recurrence expansion.
   - Still open: a real provider account, including OAuth via `TBD_MODE=gui`.
   - Gotchas found:
     - `messages.send` is an *optional-only* permission. The bridge grants it through `ExtensionPermissions` and reloads once, because a headless add-on can't show a permission prompt.
     - `MailServices.accounts.localFoldersServer` throws on a fresh profile instead of returning null.
     - Gecko replaces any error that isn't an `ExtensionError` with "An unexpected error occurred". The `noctmalia` Experiment re-wraps errors so the real message and stack come through.
2. ~~Transport~~ — **done 2026-09-13**. `crates/noctmalia-bridge`: the client binds the socket, matches
   replies by id, streams events. `tests/transport.rs` covers reconnect, out-of-order replies,
   in-flight failure on disconnect and the 1 MiB cap.
3. ~~Contacts~~ — **done 2026-09-13**. Bridge methods for address books, contacts and mailing lists;
   a vCard parser that preserves what it does not model; the rolodex window. Not yet run against a
   real Thunderbird.
4. **Contacts, finished:** run it against tbd, then postal-address editing, photos
   (`contacts.getPhoto`/`setPhoto`, both on the bridge already) and mailing lists in the rail.
5. Mail read path: rail, message list, reader in text mode. Threading is not available from
   Thunderbird's API, so the first pass lists messages, not threads.
6. An HTML view, then the composer, then send.
7. A second surface (bar badge, notifications) — this is what forces the hub in findings §2.
