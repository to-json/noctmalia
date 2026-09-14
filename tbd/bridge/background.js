"use strict";

// noctmalia bridge — thin RPC executor over native messaging. Protocol: tbd/README.md.
// All state, threading, search and policy live in mailnd; this file only maps
// method names onto messenger.* calls and forwards events.

const PROTOCOL = 1;
const HOST = "noctmalia.bridge";
const RECONNECT_MS = 2000;

let port = null;

function send(message) {
  if (port) {
    port.postMessage(message);
  }
}

function emit(event, data) {
  send({ event, data });
}

function errorInfo(error) {
  // Gecko replaces the message of anything thrown inside a WebExtension API with "An unexpected
  // error occurred", so the top of the stack is often the only thing that says where it happened.
  const where = String(error?.stack ?? "").split("\n")[0];
  const message = String(error?.message ?? error);
  return { name: error?.name ?? "Error", message: where ? `${message} @ ${where}` : message };
}

// Names the step that failed, which an API error otherwise will not.
async function attempt(what, action) {
  try {
    return await action();
  } catch (error) {
    throw new Error(`${what}: ${error?.message ?? error}`);
  }
}

// Binary payloads cross the bridge as base64.
async function fileToBase64(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

function base64ToFile(data, name, type) {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new File([bytes], name, { type });
}

// MV2 ContactNode carries the vCard under `properties`; MV3 exposes it at the top level.
// Normalise so clients always read `vCard`, and keep `properties` for everything else.
function contactNode(node) {
  if (!node) {
    return node;
  }
  return { ...node, vCard: node.vCard ?? node.properties?.vCard ?? null };
}

async function info() {
  return {
    protocol: PROTOCOL,
    bridgeVersion: messenger.runtime.getManifest().version,
    browser: await messenger.runtime.getBrowserInfo(),
    platform: await messenger.runtime.getPlatformInfo(),
    calendar: Boolean(messenger.calendar?.calendars),
  };
}

const { calendar } = messenger;

// Composing goes through a compose tab even though there is a windowless messages.send, because
// only compose.beginReply sets In-Reply-To and References — and a reply that does not thread shows
// up in the wrong place in everybody else's mail client. spike/boundary-probe is where that was
// established. The tab closes itself once the message is sent or saved.
//
// Two things the API does not say and a headless Thunderbird will not forgive.
//
// **`isPlainText` is decided when the window opens, not afterwards.** `setComposeDetails` accepts
// it and does nothing: the editor is already an HTML one, so `plainTextBody` is ignored and the
// message goes out as whatever `beginReply` quoted into it. So the details are passed to
// `beginReply`/`beginNew` as well, where the mode is still a choice.
//
// **The window is not ready when the promise resolves.** `getComposeDetails` starts answering
// before the editor inside it can be written to, and a `setComposeDetails` that lands in that gap
// is accepted and silently does nothing. Hence `apply`, which writes, reads back, and writes again
// until what it asked for is what is there.
const sleep = (ms) => new Promise((wake) => setTimeout(wake, ms));

// Writes the details into the compose window and checks they took.
//
// `getComposeDetails` starts answering before the editor inside the window is ready to be written
// to, and a `setComposeDetails` that lands in that gap is accepted and silently does nothing — the
// message goes out with whatever `beginReply` quoted into it instead of what was typed. So this
// writes, reads back, and writes again until the body it asked for is the body that is there.
async function apply(tab, details) {
  const wanted = (details.plainTextBody ?? details.body ?? "").slice(0, 40);
  let last = null;
  let why = "";
  for (let attempt = 0; attempt < 40; attempt++) {
    try {
      await messenger.compose.setComposeDetails(tab.id, details);
      last = await messenger.compose.getComposeDetails(tab.id);
      const got = `${last.plainTextBody ?? ""}${last.body ?? ""}`;
      if (!wanted || got.includes(wanted)) {
        return last;
      }
    } catch (error) {
      why = String(error?.message ?? error);
    }
    await sleep(250);
  }
  const got = `${last?.plainTextBody ?? ""}${last?.body ?? ""}`.slice(0, 120).replace(/\s+/g, " ");
  throw new Error(
    `the compose window would not take the details (isPlainText ${last?.isPlainText}, ` +
      `wanted ${JSON.stringify(wanted)}, got ${JSON.stringify(got)}${why ? `, last error ${why}` : ""})`
  );
}

async function deliver(tab, details, mode) {
  await attempt("setComposeDetails", () => apply(tab, details));
  if (mode === "draft" || mode === "template") {
    return attempt("saveMessage", () => messenger.compose.saveMessage(tab.id, { mode }));
  }
  return attempt("sendMessage", () => messenger.compose.sendMessage(tab.id, { mode }));
}

const REPLY_TYPES = {
  replyToSender: "reply",
  replyToAll: "reply",
  replyToList: "reply",
  forwardInline: "forward",
  forwardAsAttachment: "forward",
};

const methods = {
  "bridge.ping": async () => ({ pong: Date.now() }),
  "bridge.info": info,

  "accounts.list": ({ includeSubFolders = false } = {}) => messenger.accounts.list(includeSubFolders),
  "accounts.get": ({ accountId, includeSubFolders = false }) => messenger.accounts.get(accountId, includeSubFolders),
  "identities.list": ({ accountId } = {}) => messenger.identities.list(accountId),

  "folders.query": (queryInfo = {}) => messenger.folders.query(queryInfo),
  "folders.get": ({ folderId, includeSubFolders = false }) => messenger.folders.get(folderId, includeSubFolders),
  "folders.getSubFolders": ({ folderId, includeSubFolders = false }) =>
    messenger.folders.getSubFolders(folderId, includeSubFolders),
  "folders.getFolderInfo": ({ folderId }) => messenger.folders.getFolderInfo(folderId),
  "folders.getFolderCapabilities": ({ folderId }) => messenger.folders.getFolderCapabilities(folderId),
  "folders.create": ({ parentId, name }) => messenger.folders.create(parentId, name),
  "folders.rename": ({ folderId, name }) => messenger.folders.rename(folderId, name),
  "folders.delete": ({ folderId }) => messenger.folders.delete(folderId),
  "folders.markAsRead": ({ folderId }) => messenger.folders.markAsRead(folderId),

  "messages.list": ({ folderId, sortType, sortOrder }) => messenger.messages.list(folderId, { sortType, sortOrder }),
  "messages.continueList": ({ listId }) => messenger.messages.continueList(listId),
  "messages.abortList": ({ listId }) => messenger.messages.abortList(listId),
  "messages.query": (queryInfo = {}) => messenger.messages.query(queryInfo),
  "messages.get": ({ messageId }) => messenger.messages.get(messageId),
  "messages.getFull": ({ messageId, decrypt }) => messenger.messages.getFull(messageId, { decrypt }),
  "messages.getRaw": async ({ messageId, decrypt }) => ({
    base64: await fileToBase64(await messenger.messages.getRaw(messageId, { data_format: "File", decrypt })),
  }),
  "messages.listAttachments": ({ messageId }) => messenger.messages.listAttachments(messageId),
  "messages.getAttachment": async ({ messageId, partName }) => ({
    base64: await fileToBase64(await messenger.messages.getAttachmentFile(messageId, partName)),
  }),
  "messages.update": ({ messageIds, properties }) =>
    Promise.all(messageIds.map((id) => messenger.messages.update(id, properties))),
  "messages.move": ({ messageIds, folderId, isUserAction }) =>
    messenger.messages.move(messageIds, folderId, { isUserAction }),
  "messages.copy": ({ messageIds, folderId, isUserAction }) =>
    messenger.messages.copy(messageIds, folderId, { isUserAction }),
  "messages.delete": ({ messageIds, deletePermanently, isUserAction }) =>
    messenger.messages.delete(messageIds, { deletePermanently, isUserAction }),
  "messages.archive": ({ messageIds }) => messenger.messages.archive(messageIds),
  "messages.import": ({ folderId, base64, properties }) =>
    messenger.messages.import(base64ToFile(base64, "message.eml", "message/rfc822"), folderId, properties),
  "messages.send": ({ details, mode = "sendNow" }) => messenger.messages.sendMessage(details, { mode }),

  "compose.begin": async ({ details, mode = "sendNow" }) =>
    deliver(await attempt("beginNew", () => messenger.compose.beginNew(null, details)), details, mode),
  "compose.reply": async ({ messageId, type = "replyToSender", details, mode = "sendNow" }) => {
    const kind = REPLY_TYPES[type];
    if (!kind) {
      throw new Error(`unknown reply type: ${type}`);
    }
    const tab = await attempt(kind === "reply" ? "beginReply" : "beginForward", () =>
      kind === "reply"
        ? messenger.compose.beginReply(messageId, type, details)
        : messenger.compose.beginForward(messageId, type, details)
    );
    return deliver(tab, details, mode);
  },

  // Threading and search are Thunderbird's own index; see experiments/noctmalia/parent.js.
  "gloda.conversations": ({ headerMessageIds }) => messenger.noctmalia.glodaConversations(headerMessageIds),
  "gloda.search": ({ query, limit }) => messenger.noctmalia.glodaSearch(query, limit),

  "filters.list": ({ accountId }) => messenger.noctmalia.filtersList(accountId),
  "filters.create": (options) => messenger.noctmalia.filtersCreate(options),

  "tags.list": () => messenger.messages.tags.list(),
  "tags.create": ({ key, tag, color }) => messenger.messages.tags.create(key, tag, color),
  "tags.update": ({ key, updateProperties }) => messenger.messages.tags.update(key, updateProperties),
  "tags.delete": ({ key }) => messenger.messages.tags.delete(key),

  "addressBooks.list": ({ complete = false } = {}) => messenger.addressBooks.list(complete),
  "addressBooks.get": ({ addressBookId, complete = false }) => messenger.addressBooks.get(addressBookId, complete),
  "addressBooks.create": ({ name }) => messenger.addressBooks.create({ name }),
  "addressBooks.update": ({ addressBookId, name }) => messenger.addressBooks.update(addressBookId, { name }),
  "addressBooks.delete": ({ addressBookId }) => messenger.addressBooks.delete(addressBookId),

  "contacts.list": async ({ parentId }) => (await messenger.contacts.list(parentId)).map(contactNode),
  "contacts.quickSearch": async ({ parentId, searchString }) =>
    (await (parentId
      ? messenger.contacts.quickSearch(parentId, searchString)
      : messenger.contacts.quickSearch(searchString))
    ).map(contactNode),
  "contacts.get": async ({ contactId }) => contactNode(await messenger.contacts.get(contactId)),
  "contacts.create": ({ parentId, vCard }) => messenger.contacts.create(parentId, { vCard }),
  "contacts.update": ({ contactId, vCard }) => messenger.contacts.update(contactId, { vCard }),
  "contacts.delete": ({ contactId }) => messenger.contacts.delete(contactId),
  "contacts.getPhoto": async ({ contactId }) => {
    const file = await messenger.contacts.getPhoto(contactId);
    return file ? { base64: await fileToBase64(file), type: file.type } : null;
  },
  "contacts.setPhoto": ({ contactId, base64, type = "image/png" }) =>
    messenger.contacts.setPhoto(contactId, base64ToFile(base64, "photo", type)),

  "mailingLists.list": ({ parentId }) => messenger.mailingLists.list(parentId),
  "mailingLists.get": ({ mailingListId }) => messenger.mailingLists.get(mailingListId),
  "mailingLists.create": ({ parentId, ...properties }) => messenger.mailingLists.create(parentId, properties),
  "mailingLists.update": ({ mailingListId, ...properties }) => messenger.mailingLists.update(mailingListId, properties),
  "mailingLists.delete": ({ mailingListId }) => messenger.mailingLists.delete(mailingListId),
  "mailingLists.addMember": ({ mailingListId, contactId }) => messenger.mailingLists.addMember(mailingListId, contactId),
  "mailingLists.removeMember": ({ mailingListId, contactId }) =>
    messenger.mailingLists.removeMember(mailingListId, contactId),
  "mailingLists.listMembers": async ({ mailingListId }) =>
    (await messenger.mailingLists.listMembers(mailingListId)).map(contactNode),

  "mail.checkNow": ({ accountId } = {}) => messenger.noctmalia.checkMail(accountId),
  "dev.provisionAccount": (config) => messenger.noctmalia.provisionAccount(config),
  "dev.eval": ({ code }) => messenger.noctmalia.devEval(code),

  "calendar.calendars.query": (queryProps = {}) => calendar.calendars.query(queryProps),
  "calendar.calendars.get": ({ calendarId }) => calendar.calendars.get(calendarId),
  "calendar.calendars.create": (createProperties) => calendar.calendars.create(createProperties),
  "calendar.calendars.update": ({ calendarId, updateProperties }) =>
    calendar.calendars.update(calendarId, updateProperties),
  "calendar.calendars.remove": ({ calendarId }) => calendar.calendars.remove(calendarId),
  "calendar.calendars.synchronize": ({ calendarIds } = {}) => calendar.calendars.synchronize(calendarIds),
  "calendar.items.query": (queryOptions = {}) => calendar.items.query(queryOptions),
  "calendar.items.get": ({ calendarId, id, returnFormat }) => calendar.items.get(calendarId, id, { returnFormat }),
  "calendar.items.create": ({ calendarId, ...createProperties }) => calendar.items.create(calendarId, createProperties),
  "calendar.items.update": ({ calendarId, id, ...updateProperties }) =>
    calendar.items.update(calendarId, id, updateProperties),
  "calendar.items.remove": ({ calendarId, id }) => calendar.items.remove(calendarId, id),
  "calendar.timezones.getDefinition": ({ tzid, format = "ical" }) => calendar.timezones.getDefinition(tzid, format),
};

async function handleRequest({ id, method, params }) {
  const handler = methods[method];
  if (!handler) {
    send({ id, error: { name: "MethodNotFound", message: `unknown method: ${method}` } });
    return;
  }
  try {
    const result = await handler(params ?? {});
    send({ id, result: result ?? null });
  } catch (error) {
    send({ id, error: errorInfo(error) });
  }
}

async function onMessage(message) {
  if (message.shim === "connected") {
    emit("bridge.hello", await info());
  } else if (message.method) {
    handleRequest(message);
  }
}

function connect() {
  port = messenger.runtime.connectNative(HOST);
  port.onMessage.addListener(onMessage);
  port.onDisconnect.addListener((disconnected) => {
    console.warn("noctmalia bridge: native port closed", disconnected.error ?? "");
    if (port === disconnected) {
      port = null;
    }
    setTimeout(connect, RECONNECT_MS);
  });
}

function forward(apiEvent, name, shape = (...args) => args, extra = []) {
  apiEvent.addListener((...args) => emit(name, shape(...args)), ...extra);
}

forward(messenger.messages.onNewMailReceived, "messages.onNewMailReceived", (folder, list) => ({
  folder,
  messages: list.messages,
  listId: list.id,
}));
forward(messenger.messages.onUpdated, "messages.onUpdated", (message, changed) => ({ message, changed }));
forward(messenger.messages.onMoved, "messages.onMoved", (from, to) => ({ from, to }));
forward(messenger.messages.onCopied, "messages.onCopied", (from, to) => ({ from, to }));
forward(messenger.messages.onDeleted, "messages.onDeleted", (list) => ({ messages: list.messages, listId: list.id }));
forward(messenger.folders.onCreated, "folders.onCreated", (folder) => ({ folder }));
forward(messenger.folders.onRenamed, "folders.onRenamed", (from, to) => ({ from, to }));
forward(messenger.folders.onMoved, "folders.onMoved", (from, to) => ({ from, to }));
forward(messenger.folders.onDeleted, "folders.onDeleted", (folder) => ({ folder }));
forward(messenger.folders.onUpdated, "folders.onUpdated", (from, to) => ({ from, to }));
forward(messenger.folders.onFolderInfoChanged, "folders.onFolderInfoChanged", (folder, info) => ({ folder, info }));
forward(messenger.accounts.onCreated, "accounts.onCreated", (accountId, account) => ({ accountId, account }));
forward(messenger.accounts.onDeleted, "accounts.onDeleted", (accountId) => ({ accountId }));
forward(messenger.accounts.onUpdated, "accounts.onUpdated", (accountId, changed) => ({ accountId, changed }));

forward(messenger.addressBooks.onCreated, "addressBooks.onCreated", (node) => ({ addressBook: node }));
forward(messenger.addressBooks.onUpdated, "addressBooks.onUpdated", (node) => ({ addressBook: node }));
forward(messenger.addressBooks.onDeleted, "addressBooks.onDeleted", (addressBookId) => ({ addressBookId }));
forward(messenger.contacts.onCreated, "contacts.onCreated", (node) => ({ contact: contactNode(node) }));
forward(messenger.contacts.onUpdated, "contacts.onUpdated", (node, changed) => ({
  contact: contactNode(node),
  changed,
}));
forward(messenger.contacts.onDeleted, "contacts.onDeleted", (parentId, contactId) => ({ parentId, contactId }));
forward(messenger.mailingLists.onCreated, "mailingLists.onCreated", (node) => ({ mailingList: node }));
forward(messenger.mailingLists.onUpdated, "mailingLists.onUpdated", (node) => ({ mailingList: node }));
forward(messenger.mailingLists.onDeleted, "mailingLists.onDeleted", (parentId, mailingListId) => ({
  parentId,
  mailingListId,
}));
forward(messenger.mailingLists.onMemberAdded, "mailingLists.onMemberAdded", (node) => ({ contact: contactNode(node) }));
forward(messenger.mailingLists.onMemberRemoved, "mailingLists.onMemberRemoved", (parentId, contactId) => ({
  parentId,
  contactId,
}));

if (calendar?.calendars) {
  const ical = [{ returnFormat: "ical" }];
  forward(calendar.calendars.onCreated, "calendar.calendars.onCreated", (cal) => ({ calendar: cal }));
  forward(calendar.calendars.onUpdated, "calendar.calendars.onUpdated", (cal, changed) => ({ calendar: cal, changed }));
  forward(calendar.calendars.onRemoved, "calendar.calendars.onRemoved", (calendarId) => ({ calendarId }));
  forward(calendar.items.onCreated, "calendar.items.onCreated", (item) => ({ item }), ical);
  forward(calendar.items.onUpdated, "calendar.items.onUpdated", (item, changed) => ({ item, changed }), ical);
  forward(calendar.items.onRemoved, "calendar.items.onRemoved", (calendarId, id) => ({ calendarId, id }));
  forward(calendar.items.onAlarm, "calendar.items.onAlarm", (item, alarm) => ({ item, alarm }), ical);
}

// Permission-gated functions (messages.sendMessage) are injected when this page starts,
// so the first grant of optional permissions is followed by a single reload. Once the
// grant is persisted nothing is missing on the next start, which ends the cycle.
async function start() {
  const granted = await messenger.noctmalia.grantOptionalPermissions();
  if (granted.length && (await messenger.permissions.contains({ permissions: granted }))) {
    console.info("noctmalia bridge: granted", granted.join(", "), "- reloading once");
    messenger.runtime.reload();
    return;
  }
  connect();
}

start();
