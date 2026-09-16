# noctmalia

A mail, calendar and contacts client that looks native on a [Noctalia](https://github.com/noctalia-dev/noctalia)
desktop, with **Thunderbird as the backend**. noctmalia runs its own headless Thunderbird — fetched
once, started with the window, stopped with it — which keeps doing accounts, protocols, storage,
sync and sending. We replace only the UI, and you never see the rest.

Mail, people and calendar are three surfaces of one window. `docs/mail-plan.md` is why the mail
surface is shaped the way it is; `docs/one-program-plan.md` is why there is one program.

```
noctmalia ──spawns──►  thunderbird --headless ── bridge (MailExtension) ── nm-shim ──►  noctmalia
                       in noctmalia-thunderbird.scope                   NDJSON over a unix socket
                                                                        (the app listens; the shim connects in)
```

| Path | What |
|---|---|
| `flake.nix`, `scripts/` | The devshell, and the cargo/run wrappers around it |
| `crates/noctmalia-bridge` | The protocol: binds the socket, matches replies by id, streams events |
| `crates/noctmalia` | The app: MIME and Markdown, mail, people and calendar calls, vCards and iCal, theme/font, the window, and `thunderbird/`, which fetches, provisions, spawns, watches and stops the Thunderbird behind it |
| `bridge/` | The MailExtension the app sideloads into its Thunderbird: the RPC method table and two Experiments. Protocol reference: `docs/bridge-protocol.md` |
| `tools/` | `fixture.py` (the test data, mail included), `seed.sh`/`seed.py` (put it into the running window's Thunderbird), `fake-bridge.py` (a Thunderbird stand-in), `noctmalia-ctl.py` (the control socket's CLI), `smoke.sh` |
| `docs/findings.md` | What has actually been verified, and the gotchas behind it |
| `docs/design.md` | Where this is going |
| `docs/mail-plan.md` | The mail surface: the decisions, the milestones, and what building them changed |
| `docs/want-sequence.md` | The lazyvim/fzf-feel wishlist (`want.md`), turned into six sequenced plans |
| `docs/one-program-plan.md` | The next step: noctmalia spawns and stops its own Thunderbird, and the container goes |

## Running it

```sh
just                  # the window. It starts its Thunderbird and stops it when closed.
just fake             # the window against a stand-in: no Thunderbird at all, the same mail
just dev              # the window with the bridge's dev methods on, for `just seed` and `just smoke`
just seed --reset     # put the development fixture — accounts, contacts and mail — into it
just status           # what is running, and what the bridge says about it
just shots            # draw every surface headlessly and write the PNGs
just --list           # the rest
```

`just` is the one to reach for. The first run downloads Thunderbird 155.0.1 from Mozilla (86 MB,
SHA-512 checked, into `$XDG_DATA_HOME/noctmalia`) and says so on the window; every run after that
starts it in a couple of seconds. The window opens at once, on a page that says what is happening,
and shows mail as soon as Thunderbird says hello. Closing the window asks Thunderbird to quit
through the front door — SIGTERM is an instant death for Gecko that leaves the profile locked —
and waits until it has.

Thunderbird runs in a transient systemd user scope, `noctmalia-thunderbird.scope`, so the whole
process tree is one unit: `systemctl --user status noctmalia-thunderbird.scope` shows it, and a
scope left behind by a noctmalia that was SIGKILLed is stopped by the next one before it starts.
Without a systemd user session it runs in a plain process group instead. Its output goes to
`$XDG_STATE_HOME/noctmalia/thunderbird.log`; its profile is `$XDG_DATA_HOME/noctmalia/profile`,
private to noctmalia, and `just reset` throws it away. `NOCTMALIA_THUNDERBIRD=/path/to/thunderbird`
uses a build you supply instead of the fetched one, which is how a package will work and the only
way on a machine that is not x86_64.

`just fake` is the fastest loop by a wide margin: no Thunderbird, no profile, and the same
deliberately nasty mail on screen as a real Thunderbird would serve — `tools/fixture.py` is the one
definition and both `fake-bridge.py` and `seed.py` read it. It runs the window with
`--backend external`, which only listens on the socket.

`just` comes from the devshell if it is not on your host (`nix develop -c just`).

## Build and run

Neither cargo nor rustc is installed here, so everything runs in this repository's nix devshell
(`flake.nix`), which also carries the Wayland and GL libraries iced needs.

```sh
scripts/run.sh                  # build release and run it
scripts/cargo.sh test --workspace
```

Use `scripts/run.sh`, not `cargo run`. Both the profile and the library path matter: a debug build
of iced is unusably slow, and a devshell without Mesa on it falls back to software rendering, which
looks like the application being broken rather than like a driver problem. `scripts/run.sh --debug`
switches profile when you want a backtrace.

`nix develop` reads the git tree, so `git add` a new file before the shell will see it.

When it looks stuck, ask it. `tools/noctmalia-ctl.py status` says which bridge connection is being
served, how many calls are in flight, and the slowest call so far; a call nobody answers fails
after thirty seconds rather than spinning forever. `NOCTMALIA_TRACE=1` prints every bridge call
with its latency on stderr, and connections are numbered there whether or not tracing is on.

`scripts/run.sh` forces software rendering, because this machine's Ivy Bridge GPU renders iced as
torn frames on both of Mesa's hardware paths (noctalia-iced's clock demo included — it is the stack,
not this app). Set `NOCTMALIA_GPU=1` to use the GPU on hardware that works. `NOCTMALIA_FPS=1`
reports redraws/s and `view()` time; `NOCTMALIA_FPS=drive` redraws continuously to show the ceiling.
`NOCTMALIA_REDUCE_MOTION=1` (or `NOCTALIA_REDUCE_MOTION`) collapses every animation to a millisecond,
and `NOCTALIA_MOTION_SCALE=8` runs them in slow motion, which is how the curves get looked at.

### Against a fake Thunderbird

```sh
scripts/run.sh --backend external      # binds $XDG_RUNTIME_DIR/noctmalia/bridge.sock and waits
tools/fake-bridge.py                   # connects to it and serves mail and contacts from memory
```

`tools/fake-bridge.py --empty` serves an empty store, for the empty states. The stand-in pages
`messages.list` five at a time on purpose: the windowed index and the progressive load are the two
things a fake that handed everything over at once would never exercise.

### Test data in the real thing

```sh
just dev                              # in one terminal: the window, dev methods on
tools/seed.sh                         # in another: seed accounts, contacts and mail
tools/seed.sh --reset                 # ...replacing what is already there
tools/seed.sh --flood 500             # ...plus 500 unremarkable messages
```

Seeding goes through the control socket's `call`, which passes raw bridge methods through only
when the window was started with `--dev`. The accounts, the contacts and the mail all come from
`tools/fixture.py`, which is also what `tools/fake-bridge.py` serves — so the same people and the
same mail are on screen either way. Seeding is idempotent and keeps the profile.

Mail is written with `messages.import`, straight into folders with read, flagged and junk already
set: no SMTP, no IDLE, nothing to wait for. GreenMail is kept for the one thing it uniquely tests,
which is that a message genuinely *arrives*: `tools/smoke.sh` runs it in the one container left,
on the loopback, and checks the whole round trip from a fresh profile — the window starting its
Thunderbird, mail arriving, a send, Gloda threading and search, a reply that threads, a filter that
reaches `msgFilterRules.dat`, a calendar event, and the window's exit taking Thunderbird down
cleanly. Docker is only needed for that; `scripts/with-docker.sh` sorts out the group.

## Mail

Three panes and three focus regions, which is mutt's shape: folders, the index, one letter. `h` and
`l` walk between them, `j` and `k` move, `Enter` opens, and `<Space>` opens a thread — a
conversation is one row and a number until you ask it to be more. Moving the cursor shows the
letter under it and marks nothing: you can walk down an inbox and leave it as unread as you found
it. Enter, `l`, a click or `n` is what reads. `e` archives, `d` deletes, `u`
puts it back, `r`/`R`/`f` reply and forward, `S` proposes a rule from the message in front of you,
`/` filters what is loaded and Enter searches everything. `g i`, `g s`, `g d`, `g a`, `g t` go to
the folders those letters name, and `g m` / `g c` switch surfaces — the titlebar says which one you
are in, offers the others as buttons beside the title, and shows a half-typed `g` while you are
typing it. A count works where you would expect: `3j`, `10gg`.

### One renderer, and it is Markdown

Every letter takes the same road. Plain text is already nearly Markdown, so it stays itself, with
`>` quoting and `format=flowed` reflow honoured. A Markdown part is what somebody meant. HTML is
**parsed and rewritten** as Markdown from an allowlist of the dozen elements a letter is made of —
which is not sanitizing, and the difference is the whole point: a sanitizer subtracts what it
recognises as dangerous, so its failures survive, while building the output from a list of what is
allowed means its failures are already gone.

So there is no `src` attribute anywhere in the output, no stylesheet, no script, no frame, and no
HTML engine to have a bug in. **Remote content cannot load — not "is blocked", cannot**: there is
no image fetch in the program. An image becomes the word "image" and its alt text; a tracking pixel
becomes nothing at all and is counted on the way past, so the reader can be told how many there
were. A link whose words name one site and whose `href` is another says so, which is a check the
client can make on its own without trusting anybody.

The loss is layout fidelity, which mail spends on making advertisements look like advertisements.
Where it matters there are three hatches, each one key: `o` lays the letter out with real CSS
(litehtml, which has no script engine and is handed the sender's HTML only after the same
allowlist has been written back as HTML — no `src`, no handlers, stylesheets with every `url()`
taken out), `\` shows the source exactly as it arrived, and `O` hands the whole message to
whatever the desktop opens `.eml` with.

### Nothing is stored outside Thunderbird

Read, flagged and tags are Thunderbird's and go back to IMAP. **Threading is Gloda's** — it assigns
every message a `conversationID` and has been doing it all along; the gap was in the WebExtension
API, not in Thunderbird, so there is no JWZ implementation here. **Search is Gloda's** ranked
full-text index, which cannot be run from outside Thunderbird's process at all: the index declares
the `mozporter` tokenizer, which Gecko registers at runtime and stock SQLite has never heard of.
Rules are `msgFilterRules.dat`. Even undo keeps no state worth the name — a numeric message id
belongs to wherever the message currently is, so undo remembers `headerMessageId` strings and asks
Thunderbird where they went.

Composing runs the model backwards: the draft is Markdown and goes out as plain text, exactly the
words that were typed. Thunderbird's compose API decides that one — `plainTextBody` is used only
when `isPlainText` is true, so sending HTML would mean the recipient gets a re-rendering rather than
the original. A Markdown document is a plain-text document; that is what the format is for.

## Looking like the rest of the desktop

The window follows the palette your Noctalia shell is running, re-read when you change theme, and
draws in the family fontconfig resolves `sans-serif` to. Neither is iced's default: iced's text
stack hardcodes `Open Sans` as its sans-serif and falls back to a serif when that is missing, and
noctalia-iced used to hardcode one palette. Both now come from the system.

Following the colours takes one command, once per machine:

```sh
noctmalia --install-palette-template   # or: just palette
```

Noctalia resolves its colours from one of four sources — a built-in palette, one generated from your
wallpaper, a community palette, or a custom one — and only the last two exist as files anyone else
can read. Built-ins are compiled into the shell, and "pure black" re-anchors the whole dark surface
ramp rather than darkening a single role, so reading `settings.toml` and the palette files it names
cannot tell you what is actually on screen.

What Noctalia does offer is templates: on every theme change it renders the palette it resolved
through whatever template files are configured. So noctmalia ships one. The command above writes it
to `$XDG_STATE_HOME/noctmalia/palette.tpl` and registers it in `~/.config/noctalia/noctmalia.toml`
— a file of our own beside Noctalia's settings, not an edit to them — and from then on the shell
writes the sixteen roles to `$XDG_STATE_HOME/noctmalia/palette.json` whenever they change. That
follows all four sources, and "pure black" along with them, because the shell has already done the
resolving. Deleting those two files undoes it. Without them the window keeps noctalia-iced's
built-in palette and says so on startup.

One role does not survive the trip: Noctalia 5 dropped `hover`, which noctalia-iced still uses as an
accent, so the template maps it to `primary` — which is what the 4.x palettes set it to anyway.

Every colour on screen is one of those sixteen roles. Nothing is a fraction of one: where two
things used to want different strengths of the same colour they now share the role and are told
apart some other way — hover and selection are both `surface_variant`, and what says a row is
selected is the accent bar and the accent in its avatar. The only alpha left is an animation on its
way between two roles, where the fraction is time rather than a shade. That is what makes a theme
change land properly instead of approximately.

It also moves the way the rest of the desktop does. The selection bar slides between rows on a soft
spring and the rows it passes cross-fade under it, a contact card rises into place when you pick
someone, the list arrives one row at a time, and the notice banner slides open. The durations are
noctalia-shell's own — `animFast` 100 ms, `animNormal` 200 ms, `animSlow` 400 ms — and the curves
come from `noctalia_iced::motion`. Nothing animates ambiently: the window subscribes to frames only
while something is actually moving, and an idle rolodex redraws on demand as it always did.

It also gives way as the window narrows. Three panes do not fit beside each other on half a laptop
screen, so when there is a card to show and not enough room for it, the address book rail trades its
labels for icons and slides down to a 60px strip, with a tooltip taking over the job of naming a
book. On a window wide enough for all three it stays as it is. Nothing spills out of its pane
either: the card wraps, breaking a word when a single one is wider than the pane — which is what an
address book full of unbroken strings needs — while list and rail rows are a fixed height and so
stay on one line, clipped.

The editor is a form rather than a wall of boxes. It opens with the cursor in the first field, each
input is under its own label instead of inside a placeholder that vanishes when you type, sections
carry a rule out to the edge and end in an `Add email` slot that also fills the empty ones, and the
avatar takes the initials and the accent the moment what is typed amounts to a name — at the size
the card shows it, so pressing Edit does not move it. Save is offered only when there is something
to save. On a narrow pane the buttons drop to a row of their own rather than squeezing the title
out.

## Status

- **Verified by running it:** the transport (reconnect, out-of-order replies, in-flight failure on
  disconnect, the 1 MiB cap), vCard round-tripping, contacts list/search/create/update/delete, and
  the MIME and Markdown pipeline against deliberately hostile mail.
- **Runs its own Thunderbird** (155.0.1 headless, native, in a systemd user scope): `tools/seed.sh`
  provisions two IMAP accounts and writes the fixture — contacts *and* mail — into its profile.
  `tools/smoke.sh` covers the whole round trip from a fresh profile, including Gloda threading and
  search, a reply that threads, a filter that reaches `msgFilterRules.dat`, and a clean exit. The
  Gloda and filter steps ride Thunderbird internals rather than the WebExtension API, so smoke is
  the thing to run on every Thunderbird bump.
- **Checked against the widget tree, not just the parser:** `crates/noctmalia/tests/mail.rs` lays
  the real mail surface out headlessly and asserts that a newsletter full of beacons puts no tracker
  URL on screen, and that a folder of five thousand messages builds a screenful of rows rather than
  five thousand.
- **Keyboard:** see [Mail](#mail). In People, `j`/`k` and the arrows move, `gg`/`G` jump, `/`
  searches, `n`/`e` start and edit, ctrl+S saves, Tab walks the editor's fields, Escape backs out.
  A focused text field keeps its own keys — iced reports a press a widget consumed, and the keymap
  never sees it.
- **Still missing:** reopening a saved draft, a `from:`/`is:unread`/`before:` grammar over Gloda
  search, a key for tags, and postal-address editing in the contact editor.
