const SERVER = "http://127.0.0.1:8765/";
const post = (t, d) => fetch(SERVER + t, { method: "POST", body: JSON.stringify(d ?? null) }).catch(() => {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const step = async (name, fn) => {
  try { const r = await fn(); await post("ok/" + name, r); return r; }
  catch (e) { await post("FAIL/" + name, String(e?.message ?? e)); }
};
const header = (raw, name) => (new RegExp(`^${name}:\\s*(.*)$`, "mi").exec(raw) || [])[1]?.trim() ?? null;
const ical = (d) => d.toISOString().replace(/\.\d+Z$/, "Z").replace(/[:-]/g, "");

async function setup() {
  await step("setup: provision GreenMail account", () => messenger.noctmalia.provisionAccount({
    name: "greenmail", email: "j@noctmalia.test", fullName: "J",
    imap: { host: "greenmail", port: 3143, socketType: "plain", username: "j", password: "secret" },
    smtp: { host: "greenmail", port: 3025, socketType: "plain", auth: "none" },
  }));
  return step("setup: inbox synced", async () => {
    for (let i = 0; i < 45; i++) {
      await messenger.noctmalia.checkMail();
      const hit = (await messenger.messages.query({ subject: "threading probe" })).messages[0];
      if (hit) return hit.id;
      await sleep(4000);
    }
    throw new Error("'threading probe' never synced from GreenMail");
  });
}

async function composeTests() {
  const orig = (await messenger.messages.query({ subject: "threading probe" })).messages[0];
  await post("info/original", { id: orig.id, headerMessageId: orig.headerMessageId });

  const reply = await step("compose.beginReply", async () => (await messenger.compose.beginReply(orig.id, "replyToSender")).id);
  if (reply) {
    await sleep(3000);
    await step("reply: getComposeDetails", async () => {
      const d = await messenger.compose.getComposeDetails(reply);
      return { type: d.type, relatedMessageId: d.relatedMessageId, subject: d.subject, to: d.to, quotesOriginal: (d.body || d.plainTextBody || "").includes("reply to me") };
    });
    await step("reply: setComposeDetails(body)", () => messenger.compose.setComposeDetails(reply, { body: "<p>reply from headless compose</p>" }));
    const sent = await step("reply: compose.sendMessage", async () => {
      const r = await messenger.compose.sendMessage(reply, { mode: "sendNow" });
      return { mode: r.mode, headerMessageId: r.headerMessageId, copies: r.messages.map((m) => ({ id: m.id, folder: m.folder?.path })) };
    });
    if (sent?.copies?.length) {
      await step("reply: threading headers on sent copy", async () => {
        const raw = await (await messenger.messages.getRaw(sent.copies[0].id, { data_format: "File" })).text();
        return { subject: header(raw, "Subject"), inReplyTo: header(raw, "In-Reply-To"), references: header(raw, "References") };
      });
      await step("reply: arrived in inbox over SMTP/IMAP", async () => {
        for (let i = 0; i < 25; i++) {
          await messenger.noctmalia.checkMail();
          const hit = (await messenger.messages.query({ subject: "Re: threading probe" })).messages.find((m) => m.folder?.specialUse?.includes("inbox"));
          if (hit) return { id: hit.id, folder: hit.folder.path };
          await sleep(4000);
        }
        throw new Error("not in inbox after 100s");
      });
    }
  }

  const fwd = await step("compose.beginForward", async () => (await messenger.compose.beginForward(orig.id, "forwardInline")).id);
  if (fwd) {
    await sleep(3000);
    await step("forward: getComposeDetails", async () => {
      const d = await messenger.compose.getComposeDetails(fwd);
      return { type: d.type, relatedMessageId: d.relatedMessageId, subject: d.subject, includesOriginal: (d.body || "").includes("reply to me") };
    });
    await messenger.compose.setComposeDetails(fwd, { to: ["bob@example.com"] });
    await step("forward: compose.saveMessage(draft)", async () => {
      const r = await messenger.compose.saveMessage(fwd, { mode: "draft" });
      return { mode: r.mode, copies: r.messages.map((m) => ({ folder: m.folder?.path, subject: m.subject })) };
    });
    await step("forward: close compose tab", () => messenger.tabs.remove(fwd));
  }

  await step("new: encryption fields exposed", async () => {
    const tab = await messenger.compose.beginNew();
    await sleep(2000);
    const d = await messenger.compose.getComposeDetails(tab.id);
    await messenger.tabs.remove(tab.id);
    return { selectedEncryptionTechnology: d.selectedEncryptionTechnology ?? null, attachPublicPGPKey: d.attachPublicPGPKey ?? null };
  });
}

async function contactTests() {
  const books = await step("addressBooks.list", async () => (await messenger.addressBooks.list()).map((b) => ({ id: b.id, name: b.name, readOnly: b.readOnly })));
  const book = books?.find((b) => !b.readOnly);
  if (!book) return;
  const vcard = (fn) => `BEGIN:VCARD\r\nVERSION:4.0\r\nFN:${fn}\r\nN:Builder;Bob;;;\r\nEMAIL;PREF=1:bob@example.com\r\nTEL;TYPE=cell:+1 555 0100\r\nEND:VCARD\r\n`;
  const fn = (c) => /FN:(.*)/.exec(c.vCard)?.[1]?.trim();
  const id = await step("contacts.create(vCard)", () => messenger.contacts.create(book.id, { vCard: vcard("Bob Builder") }));
  if (!id) return;
  await step("contacts.quickSearch('bob')", async () => (await messenger.contacts.quickSearch("bob")).map(fn));
  await step("contacts.update(vCard)", () => messenger.contacts.update(id, { vCard: vcard("Robert Builder") }));
  await step("contacts.get", async () => fn(await messenger.contacts.get(id)));
  const list = await step("mailingLists.create", () => messenger.mailingLists.create(book.id, { name: "builders" }));
  if (list) {
    await step("mailingLists.addMember + listMembers", async () => {
      await messenger.mailingLists.addMember(list, id);
      return (await messenger.mailingLists.listMembers(list)).map(fn);
    });
  }
  await step("contacts.delete", () => messenger.contacts.delete(id));
}

async function calendarTests() {
  const L = messenger.calendar;
  const cal = await step("calendars.create", async () => (await L.calendars.create({ type: "storage", url: "moz-storage-calendar://", name: "spike" })).id);
  if (!cal) return;
  const now = new Date();

  const vtodo = (extra) => `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//noctmalia//spike//EN\r\nBEGIN:VTODO\r\nUID:task-1\r\nSUMMARY:buy milk\r\nDUE:${ical(new Date(+now + 864e5))}\r\n${extra}END:VTODO\r\nEND:VCALENDAR\r\n`;
  await step("task: create", async () => (await L.items.create(cal, { id: "task-1", type: "task", format: "ical", item: vtodo("STATUS:NEEDS-ACTION\r\n") })).id);
  await step("task: complete", async () => {
    await L.items.update(cal, "task-1", { format: "ical", item: vtodo(`STATUS:COMPLETED\r\nPERCENT-COMPLETE:100\r\nCOMPLETED:${ical(new Date())}\r\n`) });
    return (await L.items.query({ calendarId: cal, type: "task", returnFormat: "ical" })).map((i) => ({ id: i.id, status: /STATUS:(.*)/.exec(i.item)?.[1]?.trim() }));
  });

  const alarms = [];
  L.items.onAlarm.addListener((item, alarm) => {
    alarms.push({ at: new Date().toISOString(), id: item.id, alarm });
    post("event/onAlarm", alarms.at(-1));
  }, { returnFormat: "ical" });
  const start = new Date(+now + 75e3);
  const vevent = (extra) => `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//noctmalia//spike//EN\r\nBEGIN:VEVENT\r\nUID:alarm-1\r\nSUMMARY:reminder probe\r\nDTSTART:${ical(start)}\r\nDTEND:${ical(new Date(+start + 18e5))}\r\n${extra}BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:reminder probe\r\nTRIGGER:-PT1M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;
  await step("reminder: event with VALARM (-1m, fires ~15s)", async () => (await L.items.create(cal, { id: "alarm-1", type: "event", format: "ical", item: vevent("") })).id);
  await step("reminder: onAlarm fired", async () => {
    for (let i = 0; i < 90 && !alarms.length; i++) await sleep(1000);
    if (!alarms.length) throw new Error("no onAlarm within 90s");
    return alarms[0].at;
  });
  if (!alarms.length) return;

  await step("reminder: snooze 25s via X-MOZ-LASTACK + X-MOZ-SNOOZE-TIME", async () => {
    await L.items.update(cal, "alarm-1", { format: "ical", item: vevent(`X-MOZ-LASTACK:${ical(new Date())}\r\nX-MOZ-SNOOZE-TIME:${ical(new Date(Date.now() + 25e3))}\r\n`) });
    return "updated";
  });
  await step("reminder: re-fired after snooze", async () => {
    for (let i = 0; i < 60 && alarms.length < 2; i++) await sleep(1000);
    if (alarms.length < 2) throw new Error("no second onAlarm within 60s");
    return alarms[1].at;
  });
  await step("reminder: dismiss via X-MOZ-LASTACK", async () => {
    await L.items.update(cal, "alarm-1", { format: "ical", item: vevent(`X-MOZ-LASTACK:${ical(new Date())}\r\n`) });
    const n = alarms.length;
    await sleep(20000);
    if (alarms.length > n) throw new Error("alarm fired again after dismiss");
    return "no re-fire in 20s";
  });
}

(async () => {
  for (;;) {
    try { await fetch(SERVER + "go"); break; } catch { await sleep(2000); }
  }
  await post("start", await messenger.runtime.getBrowserInfo());
  const mail = setup().then((synced) => (synced ? composeTests() : post("SKIP/compose", "inbox never synced")));
  await Promise.all([mail, contactTests(), calendarTests()].map((p) => p.catch((e) => post("FAIL/suite", String(e)))));
  await post("done");
})();
