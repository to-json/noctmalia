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
  return { name: error?.name ?? "Error", message: String(error?.message ?? error) };
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

  "tags.list": () => messenger.messages.tags.list(),
  "tags.create": ({ key, tag, color }) => messenger.messages.tags.create(key, tag, color),
  "tags.update": ({ key, updateProperties }) => messenger.messages.tags.update(key, updateProperties),
  "tags.delete": ({ key }) => messenger.messages.tags.delete(key),

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
