# noctmalia — the mail surface

A plan, against `notes.md`. It supersedes the mail half of `docs/design.md`: the HTML problem is
answered differently below, and the build order is re-cut around what the bridge already does.

**It has been built.** M0 to M5 are in the tree; §10 is what the building changed about the plan,
which is the part worth reading second.

The short version: **the backend is done, and it is doing more than we thought.**
`tbd/bridge/background.js` already exposes list, page, getFull, getRaw, attachments, flags, tags,
move, copy, delete, archive, import and send, with events for all of it — so nothing in the read
path needs a new Thunderbird capability. And Gloda, which is running unasked in the profile,
already holds the threading and the full-text index this plan's first draft proposed to build
(§2, §3). What is left is a renderer, a list widget, a keymap, two small Experiment calls, and
taste. That is the sense in which this shouldn't be that hard — and §1 is where it isn't.

## 1. One renderer, and it is markdown

> *we render md · we render html in a cautious way · we load nothing from the internet by default*

`docs/design.md` planned an `HtmlView` over litehtml. Drop it. iced 0.14 ships
`iced::widget::markdown` — `parse()` to `Item`s, `view_with()` to draw them through a `Viewer` we
control. Turning on the `markdown` feature is a one-word change to `Cargo.toml`.

So every body takes the same road:

| Part | Road |
|---|---|
| `text/plain` | quote-depth split, then markdown (plain mail is already markdown-shaped) |
| `text/markdown` | markdown |
| `text/html` | parse → write Markdown from an allowlist of elements (`src/mime/html.rs`) |

This is the whole privacy story, and it comes for free rather than as policy. There is no image
fetch in the pipeline, so remote content cannot load — not "is blocked", *cannot*. No scripts, no
forms, no beacons, no CSS that can phone home, no engine to have a CVE. Trackers die in the parse
and the ones that survive as markdown links are inert until clicked. A `Viewer` we own means every
glyph on screen is one of the sixteen palette roles, like the rest of the app, and a newsletter
cannot paint its own white card over a dark desktop.

The ordering is what makes that true rather than hopeful, and it is worth being explicit about.
A sanitizer starts from the sender's document and subtracts what it knows is dangerous, so whatever
it fails to recognise survives. This starts from nothing and adds the dozen elements a letter needs,
so whatever it fails to recognise is *already gone*. Which is also why `ammonia` is not in the
dependency list: a sanitizer plus a downconverter is two passes to reach a place one pass starts
at.

It is lossy, and the loss is the point: a marketing email flattened to markdown is a marketing
email you can read. Where the loss matters there are two escape hatches, both explicit and both
one key: **`\`** shows the raw source (`messages.getRaw`, already on the bridge), and **`O`** hands
the message to Thunderbird or `$BROWSER`, which is where a user who genuinely wants to render an
advertisement should be.

Composing runs the same model backwards: the draft is markdown, and we build
`multipart/alternative` from it in Rust. One representation, read and write.

The cost of this over litehtml is layout fidelity on newsletters. The gain is roughly a month of
work, an entire C++ dependency, and the only remote-content attack surface in the program.

## 2. What Thunderbird actually retains

> *i'm not actually rock solid on this*

Measured, not guessed — against the running container, 2026-09-14.

Nobody keeps a maildir here. `ImapMail/greenmail/` holds `INBOX.msf`, `Trash.msf` and
`msgFilterRules.dat`, and there is no `INBOX` beside them. All three accounts are
`storeContractID = @mozilla.org/msgstore/berkeleystore;1` — mbox — and offline download is off, so
not one message body exists as a file. The 23 MB profile contains no mail.

But **Thunderbird retains almost everything about that mail anyway**, just not as files:

| What | Where | Readable |
|---|---|---|
| Headers, flags, tags, folder membership | `<folder>.msf` (Mork, per folder) | via the bridge |
| **Threads** | `global-messages-db.sqlite` → `conversations` + `messages.conversationID` | SQLite |
| **Body text, subject, author, recipients, attachment names** | same → `messagesText_content` | SQLite |
| Full-text index over that | same → `messagesText` (FTS3) | **no** — needs Gecko's `mozporter` tokenizer |
| Contacts | `abook.sqlite` | via the bridge |
| Calendar | `calendar-data/local.sqlite` | via the bridge |
| Filters | `msgFilterRules.dat` (flat file) | file, or Experiment |
| Accounts, passwords | `prefs.js`, `logins.db`, `key4.db` | — |

That is Gloda, and **it indexes by itself, immediately, with no configuration**. Three messages
SMTPed into GreenMail were in `messages` within ten seconds of arriving, with bodies:

```
messages:      (32, folder 3, conv 1, 'gloda-probe-1@noctmalia.test')
               (33, folder 3, conv 1, 'gloda-probe-2@noctmalia.test')
conversations: (1, 'gloda probe')
messagesText_content:
  (32, 'probe body: capybara semaphore', 'gloda probe', '', 'alice@…', 'j@…')
```

Two consequences, both good, and one is large enough to delete a milestone.

**Drop maildir.** It was the answer to "where are the bodies", and the bodies are in Gloda as
indexed text. What a maildir mount would still uniquely buy is the *original MIME bytes* on disk —
which matters for exactly two things: `rg` over raw source, and sidestepping `getRaw`'s
base64-in-one-JSON-line for very large attachments. Neither is worth turning on offline download
for every account, doubling storage, and taking on a store format whose flags live somewhere else
anyway. Cut it from the plan. If a specific need appears later, the prefs that enable it
(`mail.server.default.offline_download`, `mail.serverDefaultStoreContractID`) have to be set
*before* the accounts exist, so note that and move on.

**"Mail as a directory of files" was really about two things**, and both survive without maildir:
trimming (which is a folder-and-filter problem, §4 in your notes' own terms, and Thunderbird
already does retention policies per folder) and greppability (which Gloda answers better than
`grep` ever did, because it has already parsed the MIME).

## 3. No database — and Thunderbird already has the one we'd have built

> *if we'd need some sort of state outside tbird for a feature, that's reason to question the feature*

The rule holds completely, and better than expected. Read/flagged/tags are Thunderbird's and
round-trip to IMAP. Rules are `msgFilterRules.dat`. Folders are folders. And the two things the
previous draft of this plan said we would have to build ourselves are already sitting in the
profile:

- **Threading is not ours.** Gloda assigns `conversationID`, and the two probe replies landed in
  one conversation with a canonical subject. **No JWZ implementation.** No in-memory per-folder
  header index. Ask Thunderbird which conversation a message is in.
- **Search is not ours either.** `messagesText_content` holds body, subject, author, recipients and
  attachment names as plain text per message. We do not need tantivy and we do not need our own
  index — we need a way to ask.

**How to ask: an Experiment over Gloda's JS API, not a direct SQLite read.** Both work; take the
first:

- The FTS table uses `mozporter`, a tokenizer Gecko registers at runtime, so stock SQLite
  (rusqlite included) can read `messagesText_content` but cannot run a ranked `MATCH` against
  `messagesText`. In-process JS gets the real query engine.
- Thunderbird holds the database open with WAL. Reading it live from another process is a
  concurrency question we would rather not own.
- The profile is a *named Docker volume*, which a host process cannot reach at all
  (`findings.md` §9) — a direct read would need a second bind mount beside the socket.
- It keeps one transport and one trust boundary. `findings.md` §8 item 5 already has "Gloda search"
  on the Experiment list; this is that, and it now also delivers threads.

Keep the direct SQLite read in your pocket for debugging and for fixtures — it is how the table
above was produced — but not in the product.

**So the "no db" rule costs nothing.** The features it was aimed at killing were snooze and
undo-send, and both are cut (§7).

## 4. Shape: one process, three surfaces in the titlebar

**Only one client may hold the bridge socket.** So mail cannot be a second binary beside the
rolodex — it has to be the same process. `crates/noctmalia/src/app.rs` is today a single
1881-line `Rolodex`; before mail lands it should become:

```
src/surfaces/mod.rs      the switcher, the shared notice bar, the keymap dispatch
src/surfaces/contacts.rs the rolodex as it is
src/surfaces/mail.rs     new
src/mail.rs              bridge calls by area, next to contacts.rs
src/mime.rs              parse, sanitize, downconvert
```

`src/contacts.rs` is the pattern for `src/mail.rs`: typed async functions over `Bridge`, errors
flattened to `String` because iced messages must be `Clone`. Follow it exactly.

### The switcher lives in the titlebar

Not a left rail. Thunderbird backs mail, calendar and contacts and that is the whole list — three
or four items never earns a permanent column of screen, and the rail each surface *does* want is
the one showing its own folders or address books. If the suite ever outgrows what Thunderbird can
answer for, revisit; until then this is a decision, not a placeholder.

The titlebar already says which surface you are in. Make that the switcher: the title text names
where you are, and the two surfaces you are not in sit beside it as glyphs.

```
┌──────────────────────────────────────────────────────────────────┐
│ Mail   ✉·[📅][👤]                                     ─  □  ✕   │
└──────────────────────────────────────────────────────────────────┘
   ▲       ▲                                              ▲
   here    the other two                          window controls
```

`chrome::capsule` is already exactly this button — 28×28, bare at rest, the hover role underneath —
so the switcher is in the design language before it is written. **Keep it on the left, next to the
title.** The right-hand cluster stays window controls and nothing else; a surface switch does not
want to live one pixel from Close.

This also answers the modal objection properly. `g m` / `g c` / `g k` still switch from the
keyboard — that is the mutt-ness and it stays — but a mode you cannot see is a mode that bites you,
and the titlebar is where you were already going to look. The keys become an accelerator for
something visible rather than hidden state.

**What it costs in the library.** `chrome::frame` takes `title: &str` and builds the titlebar as
`row![drag(Length::Fill), controls]`, mapping the whole thing through `Action`. Three changes,
all in `noctalia-iced`:

- Add `chrome::frame_with(chrome, title, leading, content, on_action)`, keeping `frame` as it is so
  nothing else in the ecosystem breaks.
- Build the titlebar row in `M` rather than `Action`: map the drag and control parts through
  `on_action`, push `leading` as-is. Leading controls emit app messages, so they cannot travel
  through `Action`.
- Split the drag region in two — one hugging the label, one filling after the leading buttons — so
  the empty titlebar still drags the window everywhere it does today.

Making `capsule` public falls out of the same change.

## 5. Vimmy, and what goes back into the library

> *we are modular · our juice composes*

The interaction model is **mutt's, not vim's**: three focus regions (rail, index, pager), one
modal keymap, no insert mode outside text widgets.

```
index   j/k  gg/G  n/N unread  x mark  ; on marked  e archive  d delete  s flag
        / filter   l limit     t tag   u undo       Enter open
pager   j/k scroll  space page  r reply  R reply-all  f forward
        h headers   \ raw source  v attachments  O open elsewhere  q back
global  g i inbox   g f flagged  ctrl+k palette
```

Two things mail forces us to build, and **both belong in `noctalia-iced`, not here**, because
contacts and calendar want them too:

- **`keymap`** — table-driven key sequences with counts and prefixes (`3j`, `gi`). The app's
  current `keys()` takes only what no widget wanted, which is the right rule and stays; what it
  lacks is *mode*, so `j` scrolls the pager and types in the composer. Exactly one iced-focused
  text widget at a time; every other focus is app state.
- **`list`** — a fixed-height windowed list. iced lays out all 50k rows of a `scrollable`, and a
  50k-message folder is the one place the rolodex's approach does not survive. Fixed row height is
  already a convention here (`widgets::ROW_HEIGHT`) and it is what mutt looks like anyway, so the
  windowing is arithmetic: render the visible slice, spacers above and below, `scroll_to` by
  multiplication. The selection-bar spring works over it unchanged.

On `bouncy<styled<Component>>`: iced has no opacity and no transform that survives clipping —
`app.rs` says so and works around it by animating padding, width, height and colour. So the
composable layer cannot be widget wrappers. It should be **builder wrappers over layout and colour
values**, which is what `motion::Replay` and `motion::mix` already are. `bouncy(row(...))` returns
a row whose numbers come from a curve. That composes, and it is the only version that composes in
this toolkit.

## 6. Driving Thunderbird into arbitrary mail states

> *we can abuse our bound tbird into representing various mailholding states for testing*

The lever is **`messages.import {folderId, base64, properties}`** — already on the bridge. It writes
an `.eml` straight into a folder with read/flagged/tags set, deterministically and synchronously,
with no SMTP, no IDLE, and no waiting. GreenMail stays for the one thing it uniquely tests: that a
message genuinely *arrives* and fires `onNewMailReceived`.

What exists: `tools/fixture.py` is the single source of truth for accounts and contacts;
`tools/seed.py` writes it idempotently; `tools/smoke.sh` proves SMTP → IMAP → event → `getFull`;
`dev.eval` is a privileged escape hatch for anything else.

What to add:

1. **`fixture.py: MESSAGES`** — one corpus, deliberately nasty: a five-deep threaded conversation,
   an HTML newsletter with tracking pixels and a `list-unsubscribe`, a message whose `Received:`
   chain doesn't add up, a spoofed `From` display name, a UTF-8 subject, an attachment, a 2 MB
   body, a `Re:` reply (which also settles the open query quirk in `findings.md` §5), and a
   message with no `Message-ID`.
2. **`fixture.py eml`** — emit the corpus as files, so `tools/fake-bridge.py` serves
   `messages.list`/`getFull` from the same set. Same mail on screen with or without Docker, exactly
   as the contacts fixture works today.
3. **Named states.** `just state empty`, `just state inbox-500`, `just state threaded`,
   `just state hostile`. Keep a profile volume per state and swap volumes rather than reseeding —
   a cold profile is about a minute.
4. **`msgFilterRules.dat` is writable**, so filter states seed the same way.
5. **Move `seq`/`wait` out of `tools/mailnd-stub.py` into `noctmalia-bridge`** as a test helper, so
   Rust integration tests can assert on real events from real Thunderbird.

## 7. Milestones

Each ends somewhere you can look at.

**M0 · The shared shell.** Split `app.rs` into surfaces; add the switcher; land `keymap` and the
windowed `list` in `noctalia-iced`; enable iced's `markdown` feature; add `frame_with` and make
`capsule` public in `chrome`.
*Demo: the rolodex, unchanged to look at, now driven by `j`/`k` through the new keymap and drawing
through the new list.*

**M1 · Read, text-first.** `src/mail.rs`, `src/mime.rs`, rail + index + pager. Every body through
the markdown pipeline. Flags, archive, delete, with the optimistic-row-out and undo toast. Fixture
`MESSAGES` and the `fake-bridge` mail methods. *Demo: triage a 500-message inbox from the keyboard
against a real Thunderbird, and read a sanitized newsletter with nothing loaded from the internet.*

**M2 · Threads and search, both out of Gloda.** One Experiment (`experiments/noctmalia`, which
already exists) growing two calls: `gloda.conversation {headerMessageId}` and `gloda.search {query}`.
Threads are `conversationID`; search is Gloda's own ranked FTS, which we cannot run from outside
the process. Collapsible thread rows; `/` filters the loaded index instantly, a second `/` runs the
real query. **No JWZ, no index of our own** — see §3. *Demo: a conversation collapses to one row,
and `/capybara` finds a message by a word in its body.*

**M3 · Compose and reply.** Add `compose`, `compose.send`, `compose.save` to
`tbd/bridge/manifest.json` — they are absent today. Reply and forward go through
`compose.beginReply`/`beginForward` → `setComposeDetails` → `compose.sendMessage`, which
`spike/boundary-probe` already proved carries correct `In-Reply-To`/`References`; windowless
`messages.send` does not thread and is for new mail only. Markdown draft → `multipart/alternative`.
*Demo: a reply that threads, sent and received through GreenMail.*

**M4 · Screening.** An Experiment over `nsIMsgFilterList`, and a UI that proposes a rule from the
message you are looking at — "everything from this list → Newsletters" — as one keystroke at the
moment you would want it. The rules are Thunderbird's file; we store nothing. *Demo: a newsletter
you never see in the inbox again, and the rule readable in `msgFilterRules.dat`.*

**M5 · The security surface.** Header-oddity badges from our own MIME parse (`Received:` chains,
`From` display-name spoofing, failed alignment), a remote-content panel (§10 — not an allow-list,
because nothing can load one), and `\` raw source over `messages.getRaw`.
*Demo: a phishing sample that announces itself.*

**Cut, deliberately:** the maildir mount (§2), a JWZ threader and a search index of our own (§3),
snooze, undo-send, muting, an HTML engine, swipe gestures, the calendar-in-mail invite cards,
OAuth/`TBD_MODE=gui` bootstrap, pushing rules to the mailhost (Sieve, Gmail), and the second
surface with its hub. The first three are cut because Thunderbird already does them; the rest are
milestones of their own later, and none of them is the thing that makes reading mail nice.

## 8. Ranked risks

1. **Markdown fidelity on real HTML mail** is the whole §1 bet. It is a week to find out rather
   than a month, and the raw-source and open-elsewhere hatches mean a bad downconvert is an
   annoyance rather than a dead end. Test it on genuinely hostile mail in M1, not M5.
2. **Headless compose** is verified once, in a spike, with no attachments and no reopened draft.
   `findings.md` §6 lists reopening drafts as unverified. M3 is where this bites.
3. **Big folders** — `messages.list` pages properly, but every `getFull` is a round trip through a
   Python shim. Cache by `headerMessageId`; prefetch the next row on selection.
4. **Gloda's schema is Thunderbird's private business.** M2 and M4 both ride Experiments over
   internals — Gloda's query API, `nsIMsgFilterList` — and internals move on version bumps. This is
   now load-bearing rather than optional, so `tools/smoke.sh` must cover conversations and search,
   and must run on every Thunderbird bump alongside the calendar Experiment's check.
5. **Gloda indexing is asynchronous and can be behind.** A message is listable before it is
   indexed, so a thread can briefly have no conversation and a search can briefly miss. Show the
   message either way; never block the index on the indexer.
6. ~~**The `msgFilterRules.dat` write path** may fight Thunderbird's in-memory copy.~~ Settled:
   rules are created through `nsIMsgFilterList` and saved with `saveToDefaultFile`, and the file is
   never written behind Thunderbird's back. Sweeping a new rule over mail already in the folder is
   `messages.query` + `messages.move` instead (§10).
7. **1 MiB toward Thunderbird** caps outgoing attachments. Chunking is designed in `design.md` and
   not built. It blocks nothing before M3.

## 9. On basalt

Nothing transplants. Basalt's editor is browser CodeMirror and its Rust core is a collaboration
server — automerge, sqlx, qdrant — with no markdown renderer in it. iced's own `markdown` widget
is the better answer here and it is already a dependency away.

What is worth stealing is shape: the vault's directory-of-markdown-files as the model for §2, and
`basalt-core/src/query`'s DQL as prior art for the `from: is:unread has:attachment before:` search
grammar in M2 — read it for the grammar and the plan/eval split, then write a far smaller one.

## 10. What the building changed

Five things turned out differently once there was code. None of them is a retreat; four are the
plan's own principles applied harder than the plan applied them.

**The HTML road is one pass, not two (§1).** `ammonia` sanitize → downconvert → markdown became
parse → write markdown from an allowlist, in `src/mime/html.rs`. A sanitizer subtracts what it
recognises as dangerous, so its failures survive; building the output from a list of what a letter
is made of means its failures are already gone. It also removed `html5ever` from the dependency
list, and `crates/noctmalia/tests/mail.rs` now asserts the claim where it belongs — against the
laid-out widget tree, with a message full of beacons open in the pager.

**A draft goes out as plain text, and the plain text is the Markdown (M3).** The plan said
`multipart/alternative` with an HTML rendering beside the source. Thunderbird settles it the other
way: `ComposeDetails.plainTextBody` is used *only* when `isPlainText` is true, so composing in HTML
means Thunderbird derives the plain part from our HTML rather than sending the text a person
actually typed. Sending Markdown as text is both lossless and the thing `notes.md` was asking for —
a Markdown document is a plain-text document, which is what the format is for.

**"Per-sender remote allow" is a feature with nothing to enable (M5).** Nothing in the program
fetches an image, so there is no blocked state to lift — and an allow-list would be exactly the
kind of state outside Thunderbird §3 says to question a feature over. What replaced it is the
honest version: the security surface names every URL the sender wanted loaded, says none of them
were, and offers to hand one to the desktop. Information about the sender rather than a decision
for the reader.

**Sweeping a new rule over the mail already in the folder is two documented calls, not an
Experiment (M4).** `messages.query` plus `messages.move` works the same on every build, gives an
exact count, and cannot fight Thunderbird's in-memory filter list — which was risk 6. Creating the
rule still rides `nsIMsgFilterList`, because nothing else can; running it does not have to.

**Threading collapses by default.** The plan said "collapsible thread rows" without saying which
way. A conversation you are not reading should cost one line, so a thread is one row and a number
until `<Space>` opens it — which is also what makes a folder of five-deep threads legible at all.

### What is not there

`compose.saveMessage` as a *reopened* draft is still unverified (risk 2): a draft can be saved, and
nothing yet opens one back into the composer. Search is Gloda's ranked full-text over everything
indexed, with no `from:`/`is:unread`/`before:` grammar over it yet — §9's DQL note is still the
right place to start when there is one. Tags round-trip through the bridge but have no key bound to
them. And `messages.list` still pages a hundred at a time through a Python shim, so the first
screenful of a very large folder arrives quickly and the last one does not; `LIST_CAP` is where
that stops.
