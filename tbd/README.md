# tbd

Thunderbird 155.0.1 running headless as the mail and calendar backend for noctmalia. mailnd connects to it through a sideloaded MailExtension, the bridge.

```
thunderbird --headless
  └ bridge@noctmalia (MV2, persistent background)
      └ runtime.connectNative("noctmalia.bridge")
          └ nm-shim  ── NDJSON over unix socket ──►  mailnd  (/run/noctmalia/bridge.sock)
```

- **Thunderbird build.** Mozilla ships Linux builds for x86_64 only, so on arm64 hosts the image runs under emulation. Startup takes about a minute.
- **The bridge** maps method names to `messenger.*` calls and forwards events. It keeps no state.
- **The shim** relays bytes without reading them. It reconnects to mailnd forever, and Thunderbird restarts it along with the extension.
- **mailnd** owns the socket and all application logic. Until it exists, `tools/mailnd-stub.py` stands in for it.

## Run

```sh
docker compose up -d --build              # tbd + mailnd stub
docker compose exec mailnd python /tools/bridgectl.py status
docker compose exec mailnd python /tools/bridgectl.py call accounts.list
tools/smoke.sh                            # full end-to-end test against GreenMail (wipes volumes)
```

| Env | Default | Meaning |
|---|---|---|
| `TBD_MODE` | `headless` | `gui` runs Thunderbird with `MOZ_ENABLE_WAYLAND=1` for interactive bootstrap, e.g. OAuth sign-in. Needs a mounted Wayland socket. Not yet tested. |
| `TBD_EXTRA_PREFS` | empty | Raw `user_pref(...)` lines appended to the managed `user.js` |
| `NOCTMALIA_BRIDGE_SOCKET` | `/run/noctmalia/bridge.sock` | Where the shim connects |

| Volume | Contents |
|---|---|
| `/data/profile` | Thunderbird profile: accounts, mail store, `logins.json`/`key4.db`. Back this up. |
| `/run/noctmalia` | Unix sockets shared with mailnd |

`prefs/user.js` is rewritten on every start. Anything Thunderbird writes to `prefs.js` itself (accounts, folders) persists.

## Bridge protocol v1

On the socket, each message is one JSON object per line. The shim converts to and from native-messaging framing.

```jsonc
// mailnd → bridge
{"id": 7, "method": "messages.list", "params": {"folderId": "account1://INBOX"}}
// bridge → mailnd
{"id": 7, "result": {...}}
{"id": 7, "error": {"name": "Error", "message": "..."}}
{"event": "messages.onNewMailReceived", "data": {...}}
```

- **Handshake:** the bridge sends a `bridge.hello` event (same payload as `bridge.info`) each time mailnd attaches. mailnd should resync on every hello.
- **Size limit:** messages from mailnd to the bridge are capped at 1 MiB, a Gecko limit. The shim rejects oversized requests with a `ShimError` reply. Messages from the bridge have no practical cap.
- **Binary data** travels as `{"base64": "..."}`.
- **Message ids** (`MessageId`) are per session. Key durable state on `headerMessageId` plus folder.

### Methods

Parameters are named. Each method maps to the `messenger.*` call of the same name (see the [WebExtension API docs](https://webextension-api.thunderbird.net/en/mv2/)).

| Group | Methods |
|---|---|
| bridge | `ping`, `info` |
| accounts | `accounts.list {includeSubFolders}`, `accounts.get {accountId}`, `identities.list {accountId}` |
| folders | `query`, `get`, `getSubFolders`, `getFolderInfo`, `getFolderCapabilities`, `create {parentId,name}`, `rename`, `delete`, `markAsRead` — all take `{folderId}` |
| messages | `list {folderId,sortType,sortOrder}` → `{id,messages}`; `continueList {listId}`; `abortList`; `query`; `get`/`getFull`/`getRaw`/`listAttachments {messageId}`; `getAttachment {messageId,partName}`; `update {messageIds,properties}`; `move`/`copy {messageIds,folderId}`; `delete {messageIds,deletePermanently}`; `archive {messageIds}`; `import {folderId,base64}`; `send {details,mode}` |
| tags | `tags.list`, `tags.create {key,tag,color}`, `tags.update {key,updateProperties}`, `tags.delete {key}` |
| mail | `mail.checkNow {accountId?}`: fetch now instead of waiting for IDLE or biff |
| calendar | `calendar.calendars.{query,get,create,update,remove,synchronize}`, `calendar.items.{query,get,create,update,remove}`, `calendar.timezones.getDefinition {tzid,format}`. Items are raw iCal/jCal: `{calendarId, type, format, item}`. |
| dev | `dev.provisionAccount {email,name,fullName,imap{host,port,socketType,auth,username,password},smtp{...}}`; `dev.eval {code}` (only when pref `extensions.noctmalia.dev` is true) |

### Events

- `bridge.hello`
- `messages.onNewMailReceived {folder,messages,listId}`
- `messages.onUpdated`, `onMoved`, `onCopied`, `onDeleted`
- `folders.onCreated`, `onRenamed`, `onMoved`, `onDeleted`, `onUpdated`, `onFolderInfoChanged`
- `accounts.onCreated`, `onDeleted`, `onUpdated`
- `calendar.calendars.onCreated`, `onUpdated`, `onRemoved`
- `calendar.items.onCreated`, `onUpdated`, `onRemoved`, `onAlarm` (iCal payloads)

## Vendored code

`bridge/experiments/calendar` is an unmodified copy of the thunderbird/webext-experiments calendar Experiment; the pinned commit is in `UPSTREAM`. On every Thunderbird version bump, re-vendor it and rerun `tools/smoke.sh`.
