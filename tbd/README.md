# tbd

Thunderbird 155.0.1 running headless as the mail, calendar and contacts backend for noctmalia. The client connects to it through a sideloaded MailExtension, the bridge.

```
thunderbird --headless
  └ bridge@noctmalia (MV2, persistent background)
      └ runtime.connectNative("noctmalia.bridge")
          └ nm-shim  ── NDJSON over unix socket ──►  the client  (/run/noctmalia/bridge.sock)
```

- **Thunderbird build.** Mozilla ships Linux builds for x86_64 only, so on arm64 hosts the image runs under emulation. Startup takes about a minute.
- **The bridge** maps method names to `messenger.*` calls and forwards events. It keeps no state.
- **The shim** relays bytes without reading them. It reconnects forever, and Thunderbird restarts it along with the extension.
- **The client owns the socket:** it listens, the shim connects in. Today that is `noctmalia` itself (`crates/noctmalia-bridge`); `tools/mailnd-stub.py` is the CLI stand-in, and `tools/fake-bridge.py` is the reverse, a Thunderbird stand-in for developing the UI.

## Run

```sh
docker compose up -d --build              # tbd + the CLI stub
docker compose exec mailnd python /tools/bridgectl.py status
docker compose exec mailnd python /tools/bridgectl.py call accounts.list
tools/seed.sh                             # stack up + the development fixture in the profile
tools/smoke.sh                            # full end-to-end test against GreenMail (wipes volumes)
```

`tools/seed.sh` is the one to reach for: it starts the stack with the dev pref on and GreenMail
carrying the fixture's users, waits for `bridge.hello`, then runs `tools/seed.py` to provision the
mail accounts and write the address books and contacts. It is idempotent and keeps the profile, so
re-run it freely; `--reset` replaces the fixture's own contacts. The fixture lives in
`tools/fixture.py`, and is the same data `tools/fake-bridge.py` serves without a container.

| Env | Default | Meaning |
|---|---|---|
| `TBD_MODE` | `headless` | `gui` runs Thunderbird with `MOZ_ENABLE_WAYLAND=1` for interactive bootstrap, e.g. OAuth sign-in. Needs a mounted Wayland socket. Not yet tested. |
| `TBD_EXTRA_PREFS` | empty | Raw `user_pref(...)` lines appended to the managed `user.js` |
| `NOCTMALIA_BRIDGE_SOCKET` | `/run/noctmalia/bridge.sock` | Where the shim connects |

| Volume | Contents |
|---|---|
| `/data/profile` | Thunderbird profile: accounts, mail store, `logins.json`/`key4.db`. Back this up. |
| `/run/noctmalia` | Unix sockets shared with the client. Bind-mount this from the host for a host-native UI — see `compose.ui.yaml`. |

`prefs/user.js` is rewritten on every start. Anything Thunderbird writes to `prefs.js` itself (accounts, folders) persists.

## Bridge protocol v1

On the socket, each message is one JSON object per line. The shim converts to and from native-messaging framing.

```jsonc
// client → bridge
{"id": 7, "method": "messages.list", "params": {"folderId": "account1://INBOX"}}
// bridge → client
{"id": 7, "result": {...}}
{"id": 7, "error": {"name": "Error", "message": "..."}}
{"event": "messages.onNewMailReceived", "data": {...}}
```

- **Handshake:** the bridge sends a `bridge.hello` event (same payload as `bridge.info`) each time a client attaches, and again whenever Thunderbird restarts under a live socket. Resync on every hello.
- **Size limit:** messages from the client to the bridge are capped at 1 MiB, a Gecko limit. The shim rejects oversized requests with a `ShimError` reply. Messages from the bridge have no practical cap.
- **Binary data** travels as `{"base64": "..."}`.
- **Contacts are vCards.** MV2 puts the vCard under `properties`; the bridge lifts it to `vCard` on
  every contact it returns, so clients read one field either way. Anything a client does not
  understand (`UID`, `REV`, `X-`) must be written back unchanged or Thunderbird loses it.
- **Message ids** (`MessageId`) are per session, and belong to wherever the message currently is —
  moving a message does not carry its id along. Key durable state on `headerMessageId` plus folder.
- **A `MessageHeader.subject` has no `Re:` on it.** Thunderbird's database strips the prefix and
  keeps it as a flag, so a reply is indexed under the subject it is replying to.
- **`messages.query` wants `autoPaginationTimeout: 0`** unless you are prepared to page it.

### Methods

Parameters are named. Each method maps to the `messenger.*` call of the same name (see the [WebExtension API docs](https://webextension-api.thunderbird.net/en/mv2/)).

| Group | Methods |
|---|---|
| bridge | `ping`, `info` |
| accounts | `accounts.list {includeSubFolders}`, `accounts.get {accountId}`, `identities.list {accountId}` |
| folders | `query`, `get`, `getSubFolders`, `getFolderInfo`, `getFolderCapabilities`, `create {parentId,name}`, `rename`, `delete`, `markAsRead` — all take `{folderId}` |
| messages | `list {folderId,sortType,sortOrder}` → `{id,messages}`; `continueList {listId}`; `abortList`; `query`; `get`/`getFull`/`getRaw`/`listAttachments {messageId}`; `getAttachment {messageId,partName}`; `update {messageIds,properties}`; `move`/`copy {messageIds,folderId}`; `delete {messageIds,deletePermanently}`; `archive {messageIds}`; `import {folderId,base64}`; `send {details,mode}` |
| compose | `compose.begin {details,mode}`, `compose.reply {messageId,type,details,mode}` — `type` is `replyToSender`/`replyToAll`/`replyToList`/`forwardInline`/`forwardAsAttachment`, `mode` is `sendNow`/`draft`/`template`. Only these set `In-Reply-To`/`References`; windowless `messages.send` does not thread and is for new mail only. |
| gloda | `gloda.conversations {headerMessageIds}` → `[{id,subject,messages}]`; `gloda.search {query,limit}` → `MessageHeader[]`. Thunderbird's own threading and ranked full-text index — neither is reachable from outside this process. |
| filters | `filters.list {accountId}` → `[{name,enabled,summary}]`; `filters.create {accountId,name,header,value,folderId,folderPath}` — `header` is `from`/`to`/`subject`/`list-id`. Writes `msgFilterRules.dat`. |
| tags | `tags.list`, `tags.create {key,tag,color}`, `tags.update {key,updateProperties}`, `tags.delete {key}` |
| addressBooks | `addressBooks.list {complete}`, `get {addressBookId,complete}`, `create {name}`, `update {addressBookId,name}`, `delete {addressBookId}` |
| contacts | `contacts.list {parentId}`, `quickSearch {searchString,parentId?}`, `get {contactId}`, `create {parentId,vCard}` → id, `update {contactId,vCard}`, `delete {contactId}`, `getPhoto {contactId}` → `{base64,type}`, `setPhoto {contactId,base64,type}` |
| mailingLists | `mailingLists.list {parentId}`, `get`/`delete` `{mailingListId}`, `create {parentId,name,nickName?,description?}`, `update {mailingListId,...}`, `addMember`/`removeMember {mailingListId,contactId}`, `listMembers {mailingListId}` |
| mail | `mail.checkNow {accountId?}`: fetch now instead of waiting for IDLE or biff |
| calendar | `calendar.calendars.{query,get,create,update,remove,synchronize}`, `calendar.items.{query,get,create,update,remove}`, `calendar.timezones.getDefinition {tzid,format}`. Items are raw iCal/jCal: `{calendarId, type, format, item}`. |
| dev | `dev.provisionAccount {email,name,fullName,imap{host,port,socketType,auth,username,password},smtp{...}}`; `dev.eval {code}` (only when pref `extensions.noctmalia.dev` is true) |

### Events

- `bridge.hello`
- `messages.onNewMailReceived {folder,messages,listId}`
- `messages.onUpdated`, `onMoved`, `onCopied`, `onDeleted`
- `folders.onCreated`, `onRenamed`, `onMoved`, `onDeleted`, `onUpdated`, `onFolderInfoChanged`
- `accounts.onCreated`, `onDeleted`, `onUpdated`
- `addressBooks.onCreated`, `onUpdated`, `onDeleted`
- `contacts.onCreated`, `onUpdated`, `onDeleted`
- `mailingLists.onCreated`, `onUpdated`, `onDeleted`, `onMemberAdded`, `onMemberRemoved`
- `calendar.calendars.onCreated`, `onUpdated`, `onRemoved`
- `calendar.items.onCreated`, `onUpdated`, `onRemoved`, `onAlarm` (iCal payloads)

## Vendored code

`bridge/experiments/calendar` is an unmodified copy of the thunderbird/webext-experiments calendar Experiment; the pinned commit is in `UPSTREAM`. On every Thunderbird version bump, re-vendor it and rerun `tools/smoke.sh`.

`bridge/experiments/noctmalia` is ours, and rides Thunderbird internals that have no API:
`nsIMsgAccountManager` for provisioning, Gloda for threading and search, `nsIMsgFilterList` for
rules. Internals move on version bumps, which is why `tools/smoke.sh` exercises all three — it is
the thing to run before believing a new Thunderbird.
