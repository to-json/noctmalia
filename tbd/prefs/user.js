// Managed by tbd/entrypoint.sh — rewritten on every container start.

// Sideloaded bridge extension
user_pref("extensions.autoDisableScopes", 0);
user_pref("extensions.enabledScopes", 15);
user_pref("xpinstall.signatures.required", false);

// No first-run UI
user_pref("mail.shell.checkDefaultClient", false);
user_pref("mail.provider.suppress_dialog_on_startup", true);
user_pref("mailnews.start_page.enabled", false);
user_pref("mail.rights.override", true);

// The image is the update channel
user_pref("app.update.enabled", false);
user_pref("app.update.auto", false);
user_pref("extensions.update.enabled", false);

// No telemetry
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("toolkit.telemetry.enabled", false);

// Sync behaviour: IDLE where available, 1-minute biff otherwise, no local alerts
user_pref("mail.server.default.check_new_mail", true);
user_pref("mail.server.default.check_time", 1);
user_pref("mail.server.default.use_idle", true);
user_pref("mail.biff.show_alert", false);
user_pref("mail.biff.play_sound", false);

// Console output to container logs
user_pref("devtools.console.stdout.chrome", true);
user_pref("browser.dom.window.dump.enabled", true);

// Bridge dev escape hatch (noctmalia.devEval); enable via TBD_EXTRA_PREFS only in dev
user_pref("extensions.noctmalia.dev", false);
