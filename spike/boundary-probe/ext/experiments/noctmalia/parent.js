"use strict";

// Privileged helpers the MailExtension API does not cover.

var { MailServices } = ChromeUtils.importESModule("resource:///modules/MailServices.sys.mjs");
var { ExtensionUtils: { ExtensionError } } = ChromeUtils.importESModule("resource://gre/modules/ExtensionUtils.sys.mjs");
var { ExtensionPermissions } = ChromeUtils.importESModule("resource://gre/modules/ExtensionPermissions.sys.mjs");

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
