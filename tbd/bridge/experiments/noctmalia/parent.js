"use strict";

// Privileged helpers the MailExtension API does not cover.

var { MailServices } = ChromeUtils.importESModule("resource:///modules/MailServices.sys.mjs");
var { ExtensionUtils: { ExtensionError } } = ChromeUtils.importESModule("resource://gre/modules/ExtensionUtils.sys.mjs");
var { ExtensionPermissions } = ChromeUtils.importESModule("resource://gre/modules/ExtensionPermissions.sys.mjs");

// Gloda is Thunderbird's own message index: it assigns every message a conversation and keeps a
// full-text index of every body. Both run unasked in a default profile (docs/findings.md), and
// both are reachable only from inside this process — the index declares the `mozporter`
// tokenizer, which Gecko registers at runtime and stock SQLite has never heard of.
var { Gloda } = ChromeUtils.importESModule("resource:///modules/gloda/GlodaPublic.sys.mjs");
// The NOUN_* constants moved out of Gloda itself in Thunderbird 102 and `Gloda.NOUN_MESSAGE` has
// been undefined ever since — which fails as "nounDef is undefined" three frames deep in
// newQuery, so it is worth naming here.
var { GlodaConstants } = ChromeUtils.importESModule("resource:///modules/gloda/GlodaConstants.sys.mjs");

// Gloda's query API is collection-and-listener rather than a promise.
function collect(query, limit) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const done = (items) => {
      if (!settled) {
        settled = true;
        resolve(items);
      }
    };
    const listener = {
      onItemsAdded() {},
      onItemsModified() {},
      onItemsRemoved() {},
      onQueryCompleted(collection) {
        done(collection.items.slice(0, limit ?? collection.items.length));
      },
    };
    try {
      query.getCollection(listener);
    } catch (error) {
      settled = true;
      reject(error);
    }
  });
}

// A search term's value is a boxed object whose `attrib` has to be set before its string is.
function searchValue(term, attrib, str) {
  const value = term.value;
  value.attrib = attrib;
  value.str = str;
  term.value = value;
}

const FILTER_ATTRIBUTES = {
  from: () => Ci.nsMsgSearchAttrib.Sender,
  to: () => Ci.nsMsgSearchAttrib.To,
  subject: () => Ci.nsMsgSearchAttrib.Subject,
  "list-id": () => Ci.nsMsgSearchAttrib.OtherHeader,
};

// A search term reads back as numbers. Naming them is the difference between a rule that says
// "from contains deals@shopfront.example" and one that says "1 contains deals@shopfront.example".
function termName(term) {
  if (term.attrib === Ci.nsMsgSearchAttrib.OtherHeader) {
    return term.arbitraryHeader || "header";
  }
  const named = {
    [Ci.nsMsgSearchAttrib.Sender]: "from",
    [Ci.nsMsgSearchAttrib.To]: "to",
    [Ci.nsMsgSearchAttrib.CC]: "cc",
    [Ci.nsMsgSearchAttrib.ToOrCC]: "to or cc",
    [Ci.nsMsgSearchAttrib.Subject]: "subject",
    [Ci.nsMsgSearchAttrib.Body]: "body",
    [Ci.nsMsgSearchAttrib.Date]: "date",
    [Ci.nsMsgSearchAttrib.Keywords]: "tag",
    [Ci.nsMsgSearchAttrib.MsgStatus]: "status",
    [Ci.nsMsgSearchAttrib.Size]: "size",
  };
  return named[term.attrib] ?? `attribute ${term.attrib}`;
}

function operatorName(op) {
  const named = {
    [Ci.nsMsgSearchOp.Contains]: "contains",
    [Ci.nsMsgSearchOp.DoesntContain]: "does not contain",
    [Ci.nsMsgSearchOp.Is]: "is",
    [Ci.nsMsgSearchOp.Isnt]: "is not",
    [Ci.nsMsgSearchOp.BeginsWith]: "begins with",
    [Ci.nsMsgSearchOp.EndsWith]: "ends with",
    [Ci.nsMsgSearchOp.IsBefore]: "is before",
    [Ci.nsMsgSearchOp.IsAfter]: "is after",
  };
  return named[op] ?? "matches";
}

function accountOf(accountId) {
  const account = MailServices.accounts.getAccount(accountId);
  if (!account) {
    throw new ExtensionError(`unknown account: ${accountId}`);
  }
  return account;
}

// A folder is named by whatever the caller has, and three things can be true of a Thunderbird:
// `folderManager.get` resolves a WebExtension id, or it does not; the id itself is `<account>://
// <path>`, which can be walked; or the caller passed the path separately. Try all three and say
// what was tried when none of them work.
function descend(root, path) {
  if (!path || path === "/") {
    return root;
  }
  let folder = root;
  for (const step of path.split("/").filter(Boolean)) {
    folder = folder.getChildNamed(step);
    if (!folder) {
      return null;
    }
  }
  return folder;
}

function folderOf(context, accountId, folderId, path) {
  const manager = context.extension.folderManager;
  if (folderId && manager?.get) {
    try {
      const found = manager.get(folderId);
      if (found) {
        return found;
      }
    } catch {
      // Fall through.
    }
  }
  const root = accountOf(accountId).incomingServer.rootFolder;
  const inside = folderId?.includes("://") ? folderId.slice(folderId.indexOf("://") + 3) : null;
  for (const candidate of [inside, path]) {
    if (candidate === null || candidate === undefined) {
      continue;
    }
    const found = descend(root, candidate);
    if (found) {
      return found;
    }
  }
  throw new ExtensionError(`no folder for ${folderId ?? "(no id)"} or ${path ?? "(no path)"}`);
}

const SOCKET_TYPES = {
  plain: Ci.nsMsgSocketType.plain,
  starttls: Ci.nsMsgSocketType.alwaysSTARTTLS,
  tls: Ci.nsMsgSocketType.SSL,
};

const AUTH_METHODS = {
  none: Ci.nsMsgAuthMethod.none,
  cleartext: Ci.nsMsgAuthMethod.passwordCleartext,
  encrypted: Ci.nsMsgAuthMethod.passwordEncrypted,
};

function pick(table, key, field) {
  if (!(key in table)) {
    throw new ExtensionError(`${field}: expected one of ${Object.keys(table).join(", ")}, got ${key}`);
  }
  return table[key];
}

// The localFoldersServer getter throws NS_ERROR_UNEXPECTED on a fresh profile instead of returning null.
function hasLocalFolders() {
  try {
    return Boolean(MailServices.accounts.localFoldersServer);
  } catch {
    return false;
  }
}

async function storeLogin(scheme, host, username, password) {
  const origin = `${scheme}://${host}`;
  const login = Cc["@mozilla.org/login-manager/loginInfo;1"].createInstance(Ci.nsILoginInfo);
  login.init(origin, null, origin, username, password, "", "");
  const existing = await Services.logins.searchLoginsAsync({ origin, httpRealm: origin });
  for (const old of existing.filter((l) => l.username === username)) {
    Services.logins.removeLogin(old);
  }
  await Services.logins.addLoginAsync(login);
}

// Gecko replaces anything but ExtensionError with "An unexpected error occurred";
// keep the real message and top of the stack so mailnd can report it.
function surfaceErrors(api) {
  const wrapped = {};
  for (const [name, fn] of Object.entries(api)) {
    wrapped[name] = async (...args) => {
      try {
        return await fn(...args);
      } catch (error) {
        if (error instanceof ExtensionError) {
          throw error;
        }
        const where = String(error?.stack ?? "").split("\n").slice(0, 3).join(" <- ");
        throw new ExtensionError(`${name}: ${error}${where ? ` @ ${where}` : ""}`);
      }
    };
  }
  return wrapped;
}

this.noctmalia = class extends ExtensionAPI {
  getAPI(context) {
    return {
      noctmalia: surfaceErrors({
        // Headless has nobody to answer permissions.request(); grant the manifest's
        // optional permissions directly. Returns the ones that were missing.
        async grantOptionalPermissions() {
          const { extension } = context;
          const missing = (extension.manifest.optional_permissions ?? []).filter(
            (permission) => !permission.includes("://") && !extension.hasPermission(permission)
          );
          if (missing.length) {
            await ExtensionPermissions.add(extension.id, { permissions: missing, origins: [] }, extension);
          }
          return missing;
        },

        async checkMail(accountId) {
          const accounts = accountId
            ? [MailServices.accounts.getAccount(accountId)]
            : MailServices.accounts.accounts;
          const checked = [];
          for (const account of accounts) {
            if (!account) {
              throw new ExtensionError(`unknown account: ${accountId}`);
            }
            const server = account.incomingServer;
            if (!server || server.type === "none") {
              continue;
            }
            const inbox = server.rootFolder.getFolderWithFlags(Ci.nsMsgFolderFlags.Inbox);
            if (inbox) {
              server.getNewMessages(inbox, null, null);
              checked.push(account.key);
            }
          }
          return checked;
        },

        async provisionAccount(config) {
          const { imap, smtp, email } = config;
          if (!imap?.host || !imap?.username || !smtp?.host || !email) {
            throw new ExtensionError("provisionAccount requires email, imap{host,username}, smtp{host}");
          }

          if (!hasLocalFolders()) {
            MailServices.accounts.createLocalMailAccount();
          }

          const found = MailServices.accounts.findServer(imap.username, imap.host, "imap");
          if (found) {
            return { accountId: MailServices.accounts.findAccountForServer(found).key, created: false };
          }

          const server = MailServices.accounts.createIncomingServer(imap.username, imap.host, "imap");
          server.port = imap.port ?? 993;
          server.socketType = pick(SOCKET_TYPES, imap.socketType ?? "tls", "imap.socketType");
          server.authMethod = pick(AUTH_METHODS, imap.auth ?? "cleartext", "imap.auth");
          server.prettyName = config.name ?? email;
          server.doBiff = true;
          server.biffMinutes = config.biffMinutes ?? 1;

          const outgoing = MailServices.outgoingServer.createServer("smtp");
          const smtpServer = outgoing.QueryInterface(Ci.nsISmtpServer);
          smtpServer.hostname = smtp.host;
          smtpServer.port = smtp.port ?? 465;
          outgoing.socketType = pick(SOCKET_TYPES, smtp.socketType ?? "tls", "smtp.socketType");
          outgoing.authMethod = pick(AUTH_METHODS, smtp.auth ?? "cleartext", "smtp.auth");
          outgoing.username = smtp.username ?? imap.username;

          const identity = MailServices.accounts.createIdentity();
          identity.fullName = config.fullName ?? "";
          identity.email = email;
          identity.smtpServerKey = outgoing.key;

          const account = MailServices.accounts.createAccount();
          account.incomingServer = server;
          account.addIdentity(identity);

          if (imap.password) {
            await storeLogin("imap", imap.host, imap.username, imap.password);
          }
          if (smtp.password ?? imap.password) {
            await storeLogin("smtp", smtp.host, outgoing.username, smtp.password ?? imap.password);
          }

          Services.prefs.savePrefFile(null);
          return { accountId: account.key, created: true };
        },

        // Which conversation Thunderbird has put each of these messages in.
        //
        // Not a threading algorithm: Thunderbird has been threading the whole time, and the gap
        // was in the WebExtension API rather than in Thunderbird. Gloda indexes asynchronously, so
        // a message that has just arrived may not be in a conversation yet — that is a message on
        // its own for a few seconds, not an error.
        async glodaConversations(headerMessageIds) {
          if (!headerMessageIds?.length) {
            return [];
          }
          const query = Gloda.newQuery(GlodaConstants.NOUN_MESSAGE);
          query.headerMessageID(...headerMessageIds);
          const messages = await collect(query);
          const conversations = new Map();
          for (const message of messages) {
            const conversation = message.conversation;
            if (!conversation) {
              continue;
            }
            if (!conversations.has(conversation.id)) {
              conversations.set(conversation.id, {
                id: conversation.id,
                subject: conversation.subject ?? "",
                messages: [],
              });
            }
            conversations.get(conversation.id).messages.push(message.headerMessageID);
          }
          return [...conversations.values()];
        },

        // Gloda's own ranked full-text search, over every folder it has indexed.
        async glodaSearch(queryString, limit) {
          const { GlodaMsgSearcher } = ChromeUtils.importESModule(
            "resource:///modules/gloda/GlodaMsgSearcher.sys.mjs"
          );
          const searcher = new GlodaMsgSearcher(null, queryString);
          const messages = await collect(searcher.buildFulltextQuery(), limit ?? 200);
          const found = [];
          for (const message of messages) {
            // A hit whose folder is gone, or whose message has been deleted since it was indexed.
            const header = message.folderMessage;
            if (!header) {
              continue;
            }
            found.push(context.extension.messageManager.convert(header));
          }
          return found;
        },

        // Thunderbird's own message filters, which live in msgFilterRules.dat and are the whole of
        // what this program knows about screening: we propose one and Thunderbird keeps it.
        async filtersList(accountId) {
          const list = accountOf(accountId).incomingServer.getFilterList(null);
          const rules = [];
          for (let index = 0; index < list.filterCount; index++) {
            const filter = list.getFilterAt(index);
            const terms = [];
            for (const term of filter.searchTerms) {
              terms.push(`${termName(term)} ${operatorName(term.op)} ${term.value?.str ?? ""}`);
            }
            rules.push({ name: filter.filterName, enabled: filter.enabled, summary: terms.join(", ") });
          }
          return rules;
        },

        async filtersCreate({ accountId, name, header, value, folderId, folderPath }) {
          const attribute = FILTER_ATTRIBUTES[header];
          if (!attribute) {
            throw new ExtensionError(`cannot match on ${header}`);
          }
          const attrib = attribute();
          const target = folderOf(context, accountId, folderId, folderPath);
          const list = accountOf(accountId).incomingServer.getFilterList(null);

          const filter = list.createFilter(name);
          filter.enabled = true;
          filter.filterType = Ci.nsMsgFilterType.InboxRule;

          const term = filter.createTerm();
          term.attrib = attrib;
          if (attrib === Ci.nsMsgSearchAttrib.OtherHeader) {
            term.arbitraryHeader = header;
          }
          term.op = Ci.nsMsgSearchOp.Contains;
          searchValue(term, attrib, value);
          term.booleanAnd = true;
          filter.appendTerm(term);

          const action = filter.createAction();
          action.type = Ci.nsMsgFilterAction.MoveToFolder;
          action.targetFolderUri = target.URI;
          filter.appendAction(action);

          list.insertFilterAt(0, filter);
          list.saveToDefaultFile();
          return { name: filter.filterName };
        },

        async devEval(code) {
          if (!Services.prefs.getBoolPref("extensions.noctmalia.dev", false)) {
            throw new ExtensionError("devEval is disabled (extensions.noctmalia.dev = false)");
          }
          const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
          const result = await new AsyncFunction("MailServices", code)(MailServices);
          return JSON.parse(JSON.stringify(result ?? null));
        },
      }),
    };
  }
};
