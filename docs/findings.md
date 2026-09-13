# noctmalia — findings & handoff

Snapshot: 2026-09-13. Development happened on macOS (arm64) with Docker Desktop and is moving to Linux hardware.
This file is the source of truth for what has been **verified**. `docs/design.md` covers UI and product direction; its mailnd sections are under review (see §2).

Legend: ✅ verified by running it · ⚠️ partially verified · ❓ untested · ❌ not possible with the official API

---

## 1. Goal

A mail, calendar, contacts and todo client that looks native on a Noctalia desktop, with Thunderbird as the backend. Thunderbird runs headless in Docker and keeps doing accounts, protocols, storage, sync and sending. We replace only the UI.

## 2. Architecture

```
┌─ container: tbd ─────────────────────────────────────────────┐
│ thunderbird 155.0.1 --headless                               │
│   └ bridge@noctmalia (MailExtension, MV2, persistent bg)     │
│       ├ official messenger.* APIs                            │
│       ├ experiments/calendar   (upstream, vendored)          │
│       ├ experiments/noctmalia  (ours, privileged)            │
│       └ runtime.connectNative("noctmalia.bridge")            │
│           └ nm-shim (stateless relay, spawned by TB)         │
└──────────────── NDJSON over unix socket /run/noctmalia/bridge.sock
                  │
          currently: tools/mailnd-stub.py (dev stand-in) + tools/bridgectl.py
```

### Open decision: mailnd or a hub
The original design put a Rust daemon, `mailnd`, between the bridge and the UI to handle threading, a search index, snooze and undo-send, and multiple clients. The user asked why we need it at all.

**Proposal, not yet decided:** drop mailnd.
- Turn `nm-shim` into a small **hub**. It listens on the socket, accepts several clients (main window, Noctalia bar/notification plugin), routes each reply back by id to the client that asked, and sends events to every client. It keeps no data.
- Take threads, search and undo from Thunderbird itself through our Experiment (§6). Drop snooze and undo-send for now.
- Tradeoff: depending on Thunderbird internals means pinning the version and running the tests on every Thunderbird bump.

## 3. Repo map

| Path | What |
|---|---|
| `compose.yaml` | `tbd`, `mailnd` (Python stub), `greenmail` (profile `dev`) |
| `tbd/Dockerfile` | Mozilla x86_64 tarball, SHA-512 pinned; non-root uid 1000; healthcheck = shim alive |
| `tbd/entrypoint.sh` | Rewrites `user.js` and the bridge XPI on each start, registers the native-messaging host, clears stale locks, `TBD_MODE=headless\|gui`, filters Gtk log noise, forwards signals |
| `tbd/prefs/user.js` | Managed prefs: sideloading, no first-run UI, no updates or telemetry, IDLE plus 1-minute biff, console to stdout, dev pref off |
| `tbd/shim/nm-shim.py` | Native messaging ⇄ NDJSON relay; reconnects; enforces the 1 MiB cap |
| `tbd/bridge/background.js` | RPC method table and event forwarding; grants optional permissions and reloads once |
| `tbd/bridge/experiments/noctmalia/` | Our privileged API: `grantOptionalPermissions`, `checkMail`, `provisionAccount`, `devEval` (pref-gated) |
| `tbd/bridge/experiments/calendar/` | Upstream calendar Experiment, unmodified; pinned commit in `UPSTREAM` |
| `tbd/README.md` | Bridge protocol v1: every method, event and env var |
| `tools/smoke.sh` | End-to-end test against GreenMail. Wipes volumes. Last result: PASS |
| `tools/mailnd-stub.py`, `tools/bridgectl.py` | Dev stand-in for mailnd, plus a CLI (`status`, `call`, `wait`) |
| `spike/bridge-probe` | First headless and MailExtension probe (TB 140) |
| `spike/native-messaging-probe` | Native messaging works headless, with host-initiated push |
| `spike/calendar-experiment-probe` | Calendar Experiment CRUD on TB 140 and 155 |
| `spike/boundary-probe` | Compose windows, contacts, tasks and reminders in an isolated TB container (§6) |
| `docs/design.md` | UI and product design; mailnd parts under review |

## 4. Running it

```sh
docker compose up -d --build                         # tbd + stub
docker compose exec mailnd python /tools/bridgectl.py status
docker compose exec mailnd python /tools/bridgectl.py call accounts.list
tools/smoke.sh                                       # full E2E, wipes volumes
docker compose --profile dev down                    # stop (add -v to wipe)
```

- **Dev eval:** enable with `TBD_EXTRA_PREFS='user_pref("extensions.noctmalia.dev", true);'`. `smoke.sh` sets it.
- **Screenshots of the hidden Thunderbird window,** for debugging or showing state. Run Marionette in the tbd container:
  - env `MOZ_MARIONETTE=1` **and** `MOZ_REMOTE_ALLOW_SYSTEM_ACCESS=1`; the second is required for chrome context in TB 155
  - connect to `127.0.0.1:2828` inside the container, `Marionette:SetContext chrome`, then `WebDriver:TakeScreenshot`
  - headless screenshots render correctly
  - never leave system access on in normal runs

## 5. Thunderbird facts learned

### Versions and builds
- As of 2026-09: Release 155.0.1, ESR 153.x (140.x ESR still patched), beta 156, nightly 157/158.
- **Mozilla ships Linux builds for x86_64 only.** On arm64 it runs emulated, which works but is slow to start. Debian's arm64 package is 140 ESR, which lacks `messages.sendMessage`.
- Tarball SHA-512s come from `archive.mozilla.org/pub/thunderbird/releases/<v>/SHA512SUMS`.

### Headless operation (all ✅ on 155)
- `--headless` builds a full profile. The main 3-pane window exists in memory, and so do compose windows.
- IMAP sync, `onNewMailReceived`, SMTP send, calendar storage and alarms all work with no display.
- **Sideloading:** put the XPI at `<profile>/extensions/<id>.xpi` and set `extensions.autoDisableScopes=0`, `extensions.enabledScopes=15`, `xpinstall.signatures.required=false`. There is no install prompt.
- **Pre-seeding accounts:** a Local Folders account can go in `user.js`. IMAP/SMTP accounts with passwords are created through our `provisionAccount` Experiment, which uses `MailServices.accounts` plus `Services.logins`.
- **Log noise:** Gtk icon-theme assertions (filtered in the entrypoint), D-Bus autolaunch warnings, and `CanCreateUserNamespace() EPERM` (content sandbox without user namespaces in Docker). None of them matter.

### Extension ↔ outside world
- **Native messaging works headless** in both directions, and the host can push without being polled. Only the extension ID in `allowed_extensions` can connect. The host manifest lives in `~/.mozilla/native-messaging-hosts/`.
- **The host is a child process of Thunderbird** and dies when TB restarts. That's why the shim is stateless and anything long-lived sits behind a socket.
- **Size limit:** messages from host to extension are capped at 1 MiB. Extension → host has no practical cap. Large payloads toward TB, such as outgoing attachments, need chunking. Not built.
- **Background page:** MV3 event pages are killed when idle, so we use MV2 with a persistent background.
- `fetch()` to `127.0.0.1` from the background works, given the host permission.

### API gotchas
- **`messages.send` is an OptionalOnlyPermission.** It is silently ignored in `permissions`. Declare it in `optional_permissions`, grant it through `ExtensionPermissions.add` in privileged code, then call `runtime.reload()` once: permission-gated functions are injected only when the background page starts. The grant persists in `extension-preferences.json`.
- **`MailServices.accounts.localFoldersServer` throws** `NS_ERROR_UNEXPECTED` on a fresh profile instead of returning null.
- **Gecko replaces any non-`ExtensionError` thrown by an Experiment** with "An unexpected error occurred". Our Experiment re-wraps errors with the message and the top of the stack.
- **There is no API to "get new mail now"**; that's our `checkMail`, via `server.getNewMessages(inbox)`.
- **`messages.query({fullText})` scans MIME message by message.** It is not the Gloda index and is slow on big mailboxes.
- **`MessageProperties` for `update`** covers only `flagged, junk, new, read, tags`. There is no answered or forwarded flag.
- **`messages.sendMessage` (windowless send)** lacks `relatedMessageId` and encryption, and `customHeaders` must start with `X-`. **Replies sent this way don't thread.** Use the compose-window API for replies and forwards.
- **Contacts in MV2:** `contacts.create(parentId, {vCard})` and `contacts.update(id, {vCard})`. The plain-vCard-string form is MV3-only. In MV2, `ContactNode` carries the vCard under `properties`.
- **Two add-ons in one profile, both vendoring the calendar Experiment:** the second add-on showed as active but its background script never ran. Cause not confirmed; suspect duplicate Experiment API names. Workaround: use a separate profile or container for test add-ons.
- **Not yet verified:** `messages.query({subject: "Re: …"})` didn't find a reply that should have been in the inbox. Probably Thunderbird stores the "Re:" prefix separately from the subject.

### Calendar
- **No core calendar extension API exists** in any build from 140.15 through 156.0b3 or at comm-central tip; `ext-mail.json` has no calendar entry. Tracking: meta bug 1627205 (NEW), depending on 2035321 (calendars/items) and 2042709 (timezones), both ASSIGNED with no milestone and no activity since 2026-07-03. The March 2026 digest aimed for "ahead of the next ESR"; nothing landed in 153.
- **Use the upstream Experiment**, `thunderbird/webext-experiments/calendar` v2.2.0. Experiments still load in release Thunderbird; Monthly-channel deprecation was postponed a year (June 2026 digest).
- **Declare `calendar_provider` even if unused.** Its startup hook registers the `resource://experiments-calendar-<uuid>` alias that calendars and items import. Without it every call fails with "unexpected error".
- **Items are raw iCal or jCal:** `{type, format, item}`. The upstream example `background.js` using `title`/`startDate` is stale.
- **`items.query({expand: true, rangeStart, rangeEnd})`** returns one entry per recurrence, each with `instance` set.
- **Snooze and dismiss need no new API.** Write Thunderbird's own iCal properties: `X-MOZ-SNOOZE-TIME:<utc>` snoozes, `X-MOZ-LASTACK:<utc>` dismisses.

### Test infrastructure quirks
- **GreenMail** (`greenmail/standalone:2.1.13`, multi-arch): users are `login:password@domain`, mail is in-memory, any recipient is accepted.
  - It does **not** add a `Message-ID` to injected mail; Thunderbird then synthesizes an `md5:…` id and replies carry no threading headers. Always set `Message-ID` when injecting.
- **Emulated Thunderbird** needed roughly 1–3 minutes to become healthy on the Mac.

## 6. API boundary for a full client

**Backing:**
- **official** = MailExtension API
- **upstream** = the calendar Experiment
- **ours** = `experiments/noctmalia`, privileged code we write and maintain against Thunderbird internals

| Area | Operations | Backing | Status |
|---|---|---|---|
| Transport | request/reply + events to an outside process | native messaging + shim | ✅ |
| Mail: read | folders, counts, quota, list/sort/page, getFull, raw, attachments, flags/tags, events | official | ✅ list/getFull/flags/new-mail/onUpdated · ❓ move/copy/delete/archive (API exists) |
| Mail: send new | plain/HTML, attachments, sendNow/sendLater | official `messages.sendMessage` | ✅ |
| Mail: reply / forward | threading headers, quoting | official compose API (`beginReply`/`beginForward` → `setComposeDetails` → `compose.sendMessage`) | ✅ correct `In-Reply-To`/`References` |
| Mail: drafts | save / edit | official `compose.saveMessage` | ✅ save · ❓ reopen existing draft |
| Mail: encrypt/sign | PGP, S/MIME | official compose (`selectedEncryptionTechnology`) | ❓ needs keys |
| Mail: threads | conversation tree | ours, over TB's thread view / msgDB | ❌ not built |
| Mail: search | fast full-text | official (slow scan) · Gloda via ours | ⚠️ slow path only |
| Mail: accounts | create/edit/delete, OAuth, check now | ours | ✅ password IMAP/SMTP, check now · ❌ OAuth, edit, delete |
| Mail: filters, saved searches, IMAP subscriptions, junk/retention settings, undo, remote-content allowlist, read receipts | — | ours | ❌ not built |
| Contacts | books, vCard CRUD, search/autocomplete, mailing lists | official | ✅ create/search/update/list-member/delete · ⚠️ field read-back unverified |
| Calendar | calendars, events, recurrence, timezones | upstream | ✅ · ❓ CalDAV subscription |
| Calendar: invites | accept/decline, iTIP replies | ours | ❌ not built |
| Todos | VTODO create/complete/query | upstream (`type: "task"`) | ✅ |
| Reminders | fire, snooze, dismiss | upstream `onAlarm` + iCal props | ✅ |

**Design rule:** the bridge today mirrors `messenger.*` names one-to-one. For the UI it should expose operations by area instead (`mail.reply`, `task.complete`, `reminder.snooze`, `contacts.search`), and choose the backing internally.

## 7. Noctalia (context for the UI)
- **Noctalia v5 is C++20, not QML.** It talks to Wayland directly and renders with OpenGL ES, cairo, pango and freetype, with no Qt or GTK. Repo `noctalia-dev/noctalia`, v5.1.0 (2026-09-10). v4 QML is frozen on `legacy-v4`.
- **Upstream toolkit shape, for reference.** The user has extracted the widget library separately (now finished), so check that library's actual API first:
  - widgets in `src/ui/controls/`
  - imperative setters, `std::function` callbacks, `Signal<>`
  - 16 palette `ColorRole`s (`primary`, `on_surface`, …)
  - `Style::` spacing, radius and font tokens
  - `AnimationManager`
  - controls include `VirtualListView`, `ScrollView`, `MarkdownView`, `Input`, `ContextMenu`, `CalendarView` and `CountdownRing`
- **Plugins:** Luau (`plugin.toml`). Entry types: bar widget, panel, desktop widget, service, launcher provider. Plugins have no toplevel window API; the codebase has `toplevel_surface`, used only by the settings window.
- **Prior art:** `noctalia-dev/community-plugins/thunderbird-companion`. A native-messaging Python host plus file polling for an unread badge, 50 recent headers and 7 commands. Opens Thunderbird's own windows. Reuse its native-messaging registration idea, not its polling design.
- **Toolkit pieces the client needs beyond stock controls:**
  - an app-window host (`xdg_toplevel`)
  - an HTML mail view (litehtml suggested)
  - a multi-line rich text editor for compose
  - swipe gestures
  - a toast host with actions
  - an async result → UI thread adaptor

## 8. Open questions and next steps
1. **Decide:** mailnd vs hub (§2).
2. **Real account.** A throwaway Gmail covers what GreenMail can't: OAuth, Gmail labels, SMTP with OAuth, Google CalDAV/CardDAV. Bootstrap via `TBD_MODE=gui` on the Wayland machine (written, untested), or sign in with any TB 155 and copy the profile into the `tbd-profile` volume. I can't create provider accounts; they need phone or CAPTCHA verification.
3. **Automatable real-ish server:** Stalwart in compose, for IMAP/SMTP/JMAP and probably CalDAV/CardDAV/OAuth. **Verify its feature list first.**
4. **Untested rows in §6:** encryption (needs keys), reopening drafts, move/delete/archive, contact field read-back, CalDAV subscription, reply-subject query quirk.
5. **Our Experiment work:** threads, Gloda search, invites, filters, OAuth account setup.
6. **Protocol:** redesign the bridge methods by area (§6 rule); chunking for payloads over 1 MiB toward TB.

## 9. Moving to Linux hardware
- **Check the CPU architecture first.** On x86_64 the image runs natively and startup is much faster. On arm64 it's still emulated (x86_64-only builds); install `qemu-user-static` / binfmt, or use Debian's 140 ESR and lose `messages.sendMessage`.
- **Docker Engine on Linux:** containers sharing the named volume `bridge-run` can reach each other's unix sockets exactly as on Docker Desktop. A **host-native UI** can't reach a named volume, so bind-mount a host dir for the socket instead, e.g. `${XDG_RUNTIME_DIR}/noctmalia:/run/noctmalia`. tbd runs as uid 1000, usually the desktop user; the stub currently chmods sockets to 0666, which should be tightened.
- **GUI bootstrap** (OAuth sign-in) needs a compose override, untested:
  ```yaml
  services:
    tbd:
      environment: { TBD_MODE: gui, WAYLAND_DISPLAY: wayland-0, XDG_RUNTIME_DIR: /run/user/1000 }
      volumes: ["${XDG_RUNTIME_DIR}/${WAYLAND_DISPLAY}:/run/user/1000/wayland-0"]
  ```
- **Timeouts:** `tools/smoke.sh` has generous timeouts sized for emulation. It uses `date -v` (BSD) with a GNU `date -d` fallback, so it works on both.
- **Migration:** the project directory isn't a git repo; `git init` before moving, or copy the whole tree. Docker volumes don't move: the profile is disposable in dev, and `smoke.sh` recreates state.
- **Non-portable context:** Claude memory for this project lives on the Mac (`~/.claude/projects/...`). This file replaces it.

## 10. Sources
- Thunderbird release notes: https://www.thunderbird.net/en-US/thunderbird/155.0/releasenotes/ (and 153.0, 154.0)
- WebExtension API docs: https://webextension-api.thunderbird.net/en/mv2/
- Calendar Experiment: https://github.com/thunderbird/webext-experiments/tree/main/calendar
- Bugs: https://bugzilla.mozilla.org/show_bug.cgi?id=1627205 · 2035321 · 2042709 · windowless send 1545930
- Dev digests: https://blog.thunderbird.net/2026/03/thunderbird-monthly-development-digest-march-2026/ · https://blog.thunderbird.net/2026/06/thunderbird-monthly-development-digest-june-2026/
- Noctalia: https://github.com/noctalia-dev/noctalia · plugins: https://docs.noctalia.dev/noctalia/plugins/development/
- Prior art: https://github.com/noctalia-dev/community-plugins/tree/main/thunderbird-companion · thunderbird-mcp projects (TKasperczyk, bb1, U-C4N, vitalio-sh/thunderbird-cli)
