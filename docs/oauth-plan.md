# OAuth2 account provisioning — Gmail, IMAP, headless-first

Date: 2026-09-15
Depends on: None

---

## Context

Mail, Contacts, and Calendar are all built and read+write against real headless Thunderbird, and
the last piece of standing infrastructure — `test.secret` → `seed.sh` → `seed.py` →
`dev.provisionAccount` — makes a real test account provision itself automatically on every stack
start, no manual credential handling, ever. That pipeline works end to end for password auth. It
does not work for Gmail: `tools/test_account.py` confirms Gmail's IMAP server rejects a plain
password outright (`AUTHENTICATIONFAILED`), because Gmail requires either an App Password (needs
2FA) or OAuth2. `docs/mail-plan.md` deliberately cut "OAuth/`TBD_MODE=gui` bootstrap" from the mail
milestones as a later increment; this plan is that increment. It's also more than dev tooling: the
user wants real OAuth support regardless ("we'll need that anyway"), so the consent mechanism this
plan builds should be shaped like a feature, not just a test-account hack.

Today, `tbd/bridge/experiments/noctmalia/parent.js`'s `AUTH_METHODS` maps only `none` /
`cleartext` / `encrypted` to `Ci.nsMsgAuthMethod`; `Ci.nsMsgAuthMethod.OAuth2` is unused, and
passing `"oauth2"` to `provisionAccount` throws immediately. Passwords are handed to Thunderbird
via the Login Manager (`storeLogin`, keyed by `scheme://host` + username) — there is no token
storage path today.

**A genuine open question drives the sequencing of this plan**: Thunderbird ships with its own
baked-in OAuth2 client credentials for known providers, including Gmail — it's how an ordinary
Thunderbird user clicks "sign in with Google" without ever touching Google Cloud Console. If the
account wizard uses that baked-in client when OAuth2 is selected for a `gmail.com` host, we may
never need our own Google Cloud project at all — and since Mozilla's own client is presumably in
verified/production status rather than "Testing," the 7-day refresh-token expiry that would apply
to *our own* unverified client might not apply either. That would delete most of the renewal
machinery this plan would otherwise need. This is cheap to find out and expensive to assume, so
the plan probes it first, before any Google Cloud setup happens.

**The consent surface is allowed to be Thunderbird's own native UI.** Popping up a real Thunderbird
account-settings/OAuth window — Google's actual consent screen inside it — is an acceptable,
even preferred, way to do this, as long as triggering it from noctmalia's own app feels seamless
(one action in our UI kicks it off; the window itself can look and behave like Thunderbird, not
be reimplemented). That reframes what "headless" needs to mean here: the steady-state (adding
mail, browsing calendar, etc.) stays headless as it is today, but *account setup* was always going
to need a real window somewhere — Google will never hand over a token to a program with no UI at
all — so the goal is a clean, on-demand trigger for that window, not eliminating it.

---

## Stream 1: Spike — does Thunderbird need our own OAuth client, and how does it store what it gets

**Problem**: Two unknowns gate everything else, and both are cheap to answer empirically and
expensive to guess wrong: (a) whether Google Cloud setup (Stream 0) is even necessary, and (b)
what a working OAuth2 login actually looks like once stored, so Streams 2-3 build toward a real
target instead of a guess.

**File(s)**: none durable — throwaway inspection, like `real_calendar_scratch.rs` was for Calendar.

### 1.1 Get a real Thunderbird window on screen

Write the `TBD_MODE=gui` compose override that `docs/findings.md` sketches but that doesn't exist
yet (`compose.yaml` currently hardcodes `TBD_MODE: headless`; `compose.ui.yaml` doesn't set it
either) — a mounted Wayland socket, uid-1000 alignment per `compose.ui.yaml`'s existing caveat.
Confirm Thunderbird actually renders and is interactable at all; this container installs no
Wayland/GL packages explicitly today, so this alone may take real debugging, not be a formality.

### 1.2 Try OAuth2 with no custom client first

In that session, add the Gmail test account through Thunderbird's own account wizard, choosing
OAuth2 as the auth method, **without** any Google Cloud project of our own. See whether Thunderbird
silently uses a baked-in client and completes Google's consent flow on its own. This determines
whether Stream 0 (below) happens at all.

- If it works: note it, skip Stream 0, and re-check whether the 7-day "Testing" expiry premise
  even applies (it may not, if Thunderbird's client isn't ours) before carrying that assumption
  into Stream 3.
- If Thunderbird has no way to accept OAuth2 for a host without a registered client (no field for
  it, or it errors) or the built-in flow fails for another reason: proceed to Stream 0, then repeat
  this step with the new client.

### 1.3 Inspect what landed in the profile

Diff `logins.json`, `key4.db`, and `prefs.js` (account-keyed OAuth state may live there too, not
just in the login manager) and note — without exposing secrets — the origin/username/shape of
whatever appeared, and whether IMAP actually authenticates and syncs. Kill the container, restart
headless against the *same* profile volume, and confirm IMAP still authenticates without the GUI
attached — this decides whether "restart without `-v`" is already solved for free by volume
persistence.

### 1.4 Determine whether a captured token can be replayed into a fresh profile

Copy the relevant login-manager/`key4.db`/`prefs.js` state into a brand-new, never-consented
profile and see whether Thunderbird accepts it without re-running consent. This answers whether
"survive `docker compose down -v`" is buildable at all, or structurally requires re-running the
GUI flow each time the volume is wiped.

### 1.5 Confirm the trigger is seamless, not just possible

However consent ends up working, confirm it can be *invoked* — from a bridge call, ideally — rather
than requiring someone to navigate Thunderbird's full menu tree by hand. This is what "seamless
from our app" cashes out to for both the dev-bootstrap case and, eventually, a real in-app "connect
your Gmail" action.

**Exit criteria**: a short written note (appended to this plan after execution) stating: whether
Stream 0 is needed at all, what origin/shape Thunderbird stores the token under, whether it
survives an ordinary restart, whether it can be captured and replayed into a fresh profile, and
whether/how the window can be triggered on demand. Streams 0, 2, and 3 all branch on these answers.

---

## Stream 0: Google Cloud OAuth client (manual, human — only if Stream 1.2 shows it's needed)

**Problem**: If Thunderbird's baked-in Gmail client doesn't cover this, we need our own — and that
setup can only be done by a human (browser + possibly phone/CAPTCHA verification; not something
this agent can do).

**File(s)**: none — a checklist. Record the outcome (client ID + a label, never the secret).

### 0.1 Create the project and client

- Google Cloud Console → new (or reuse an existing throwaway) project.
- Enable the Gmail API (gates the `https://mail.google.com/` OAuth scope even for raw IMAP).
- OAuth consent screen: External user type, Testing publishing status, add
  `obstinatesystemstest@gmail.com` (confirm this matches `test.secret`) as a test user.
- Credentials → Create OAuth client ID → Application type **Desktop app**.

### 0.2 Hand off the client ID/secret

Into the single cached-credential file Stream 3.1 defines — not a separate file. Typed once,
read only by our own tooling, never printed, never committed.

---

## Stream 2: Bridge support (`tbd/bridge/experiments/noctmalia/parent.js`)

**Problem**: The Experiment can't provision an OAuth2 account today, can't trigger a native
account/OAuth window on demand, and has an existing-account dedup that will actively fight this
feature once it exists.

**File(s)**: `tbd/bridge/experiments/noctmalia/parent.js`

### 2.1 Add the `oauth2` auth method

`AUTH_METHODS.oauth2 = Ci.nsMsgAuthMethod.OAuth2`. `provisionAccount` accepts it for `imap.auth`
the same way it accepts `cleartext` today (IMAP only — SMTP/OAuth is out of scope this pass).

### 2.2 Fix the dedup-vs-auth-method gap

`provisionAccount`'s existing-account lookup matches by username+host only, not auth method. Since
`seed_test_account()` already runs today with `auth: "cleartext"` for the Gmail test account, a
broken cleartext server is likely already registered in the profile — re-running with `auth:
"oauth2"` would find that stale match and silently skip fixing it. Detect an auth-method mismatch
on the found server and delete-and-recreate it, rather than treat "found" as "done."

### 2.3 SMTP: explicit known-limitation, not silent gap

IMAP-only OAuth means SMTP has no working auth this pass — Gmail rejects cleartext SMTP too, so
the provisioned account can sync but not send, and today's failure mode is silent until someone
tries to send. Document this plainly (code comment + docs update in Stream 4), rather than leaving
it implicit.

### 2.4 A way to trigger the account/OAuth window on demand

Shaped by Stream 1.5's findings — likely a `dev.*` (or eventually non-dev) bridge method that opens
Thunderbird's native account-setup UI pre-filled for the target account, so it can be invoked from
a script (dev bootstrap) or eventually from noctmalia's own UI (the real feature), rather than
requiring manual menu navigation each time.

### 2.5 Token injection, if Stream 1.4 shows it's possible

If a captured token can be replayed: a narrow `dev.injectOAuthToken`-style method that writes
whatever Thunderbird expects, in the shape Stream 1 discovered, so a fresh profile can be primed
without re-opening the window. If replay doesn't work cleanly: skip this, and document that OAuth
provisioning on a fresh profile always requires the on-demand window from 2.4.

---

## Stream 3: Host tooling

**Problem**: Getting a token, caching it, deciding when it's stale, and wiring it into
`seed.py`/`seed.sh` the way `test.secret` already works for passwords.

**File(s)**: `tools/oauth_bootstrap.sh` (new), `tools/seed.py`, `tools/seed.sh`, `.gitignore`,
one new gitignored secret file (name TBD, e.g. `oauth.secret`)

### 3.1 The cached-credential file (single artifact)

One gitignored file holding whatever's actually needed per Stream 1's findings — at minimum the
account email and refresh token, plus a Google client ID/secret *only if* Stream 0 happened — and
an expiry timestamp we set ourselves (issued-at + 7 days) as a best-effort heuristic, since Google
doesn't hand back a human-readable one. Same handling rules as `test.secret`: typed/captured once,
read only by our own tooling, never printed, never committed.

### 3.2 `tools/oauth_bootstrap.sh`

The one deliberate command a person runs to (re-)establish consent: brings up the account/OAuth
window (Stream 2.4), walks through it, extracts the resulting token (Stream 1/2 findings), and
writes/updates the cached-credential file.

### 3.3 `seed.py`/`seed.sh` integration — expiry as a heuristic, not a promise

`seed_test_account()` (or a sibling) checks the cached-credential file's expiry as a first-pass
filter (missing/past-guessed-expiry → print `OAuth token missing/expired — run
tools/oauth_bootstrap.sh` and skip, loudly, unlike today's silent test.secret no-op). But since
that 7-day figure is our own guess, not Google's — a real `AUTHENTICATIONFAILED` at provisioning
time must be treated as the same signal regardless of what the guessed clock says, so a wrong
guess degrades to "one confusing run" rather than a silent false pass.

---

## Stream 4: Verification

**Problem**: Confirming the thing actually does what it's for, under the conditions it's for,
including the failure modes Streams 2-3 chose to accept rather than solve.

### 4.1 Fresh-profile round trip

`docker compose down -v`, then `scripts/with-docker.sh tools/seed.sh` with a valid cached token:
confirm the account provisions, IMAP authenticates, and real Gmail mail is visible — zero manual
steps, assuming the token isn't expired.

### 4.2 Ordinary restart

Restart the stack without `-v`; confirm the account still authenticates (expected to already work
from profile persistence per Stream 1.3, but verify explicitly).

### 4.3 Expired/wrong-guess token path

Force the cached expiry into the past; confirm `seed.py` prints the clear message and the rest of
seeding still completes. Separately, confirm a real auth failure (guessed expiry not yet reached,
but Google actually rejects it) is reported clearly rather than crashing seeding.

### 4.4 Confirm send is documented as out of scope

Not "fixed" — just confirm the account being unable to send is visible/documented, not a silent
trap for whoever tests mail compose against this account next.

---

## Sequence integration

Standalone — no other active plan depends on this or blocks it. It picks back up the "OAuth/
`TBD_MODE=gui` bootstrap" item `docs/mail-plan.md` explicitly deferred. Once done, `docs/design.md`
and `docs/findings.md`'s OAuth rows should be updated to reflect what Stream 1 actually found —
including "Stream 0 wasn't needed" if that's how it goes, since that's as useful a finding as the
alternative.

## Risks

- **Reordered specifically to de-risk this**: Stream 1 answers "do we even need our own OAuth
  client" and "what does a working login look like" before Stream 0's human-only setup cost is
  spent, and before Streams 2-3 are built on an assumption.
- **Stream 1 stacks three never-before-run layers at once** (Wayland socket mount, uid alignment,
  GTK/Mesa completeness in a container that installs no Wayland packages explicitly) — expect
  several debug cycles, not one session.
- **7-day refresh token expiry is our own guess**, only relevant if Stream 0 happens at all;
  mitigated by treating a real auth failure as equally authoritative as the guessed clock (3.3).
- **SMTP-over-OAuth is explicitly out of this pass** — the account can read but not send; call this
  out wherever the account shows up in future testing so it isn't mistaken for a real bug later.
- **Scope discipline**: non-Gmail providers stay out; resist folding them in even though the
  bridge's auth-method mapping would make it tempting once `oauth2` exists at all.

---

## Stream 1 findings (2026-09-15, partial — 1.1–1.3 done, 1.4–1.5 open)

**The big question is answered: Stream 0 is not needed.** Adding the Gmail test account through
Thunderbird's own account wizard, choosing OAuth2, with no Google Cloud project of our own,
worked — a real Google login/consent screen appeared and completing it produced a working IMAP
login. Thunderbird's baked-in Gmail OAuth client covers this. Skip Stream 0 entirely unless
something downstream forces our own client (nothing has so far).

**Real, reusable gotchas surfaced along the way:**

- **`compose.gui.yaml`** (new, uncommitted) needs to bind-mount the *whole* host
  `$XDG_RUNTIME_DIR`, not just the Wayland socket file. A single-file bind mount leaves
  `/run/user/1000` root-owned inside the container (Docker synthesizes the parent directory), which
  broke Gecko's Wayland proxy (`bind(): Permission denied`) — it needs to create its own extra
  socket in that directory, not just read the one we handed it.
- **`mailnews.oauth.useExternalBrowser` had to be forced to `false`.** Modern Thunderbird's default
  is to hand OAuth consent off to the system's default browser, which this container doesn't have.
  Set via `TBD_EXTRA_PREFS`, not through the GUI (about:config isn't reachable by typing a URL in
  Thunderbird's UI — no address bar — it's Settings → General → Config Editor, but setting it via
  the env var before the window even opens is far more reliable than navigating there live).
  Whatever provisions this going forward should set this pref by default.
- **The dedup-vs-auth-method gap (Stream 2.2) is real, not hypothetical.** The wizard refused to
  add the account at all ("incoming server already exists") because of an already-registered,
  broken cleartext `test-{email}` server from earlier `seed_test_account()` runs. Had to remove it
  by hand in Account Settings before the wizard would proceed. Confirms the general review's
  finding — this needs fixing in Stream 2, not just noting.
- **Restart-persistence (Stream 1.3's exit test) is confirmed.** After completing OAuth consent,
  force-recreating the `tbd` container from scratch in headless mode (no GUI, no Wayland) against
  the *same* profile volume reattached cleanly and synced real Gmail mail through noctmalia's own
  UI. Ordinary restarts need nothing extra.

**Still open (not yet attempted):** 1.4 (does a captured token replay into a *fresh*, never-
consented profile — the thing that decides whether surviving `docker compose down -v` is
buildable), 1.5 (triggering the account/OAuth window from a bridge call instead of the manual
wizard), and inspecting exactly what `logins.json`/`key4.db`/`prefs.js` hold (only observed that
the login worked, not yet *what* row makes it work).

**Unrelated but found along the way, fixed separately**: real Gmail mail exposed a genuine bug in
the already-shipped mail surface — `message/rfc822` parts (forwards, or the original message under
a bounce) weren't recognized as containers, so the letter rendered empty and the embedded message
showed up as an unfetchable "attachment" labelled by its raw content type. Fixed in
`crates/noctmalia/src/mime/mod.rs` (`Part::is_container`, used by both `pick()` and `collect()`).
Not part of this plan's scope, but exactly the kind of thing `docs/mail-plan.md`'s "risk 1" warned
real mail would surface that no synthetic fixture would.
