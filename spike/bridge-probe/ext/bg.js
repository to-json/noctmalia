const post = (tag, data) => fetch("http://127.0.0.1:8765/" + tag, {method: "POST", body: JSON.stringify(data)});
(async () => {
  try {
    await post("hello", {ua: navigator.userAgent, info: await browser.runtime.getBrowserInfo()});
    const accts = await messenger.accounts.list(true);
    await post("accounts", accts.map(a => ({id: a.id, type: a.type, name: a.name, folders: (a.rootFolder?.subFolders || a.folders || []).map(f => f.path || f.name)})));
    const local = accts.find(a => a.type === "none");
    const root = local.rootFolder || local;
    const folders = await messenger.folders.getSubFolders(root.id ? root.id : root, false).catch(e => ({err: String(e)}));
    await post("folders", folders);
    let inbox = (Array.isArray(folders) ? folders : []).find(f => f.name === "Inbox");
    if (!inbox) { inbox = await messenger.folders.create(root.id ? root.id : root, "Inbox").catch(e => ({err: String(e)})); await post("created", inbox); }
    const eml = new File(["From: Alice <alice@example.com>\r\nTo: j@example.com\r\nSubject: probe message\r\nDate: Sat, 13 Sep 2026 10:00:00 +0000\r\nMessage-ID: <probe1@example.com>\r\n\r\nhello from inside the daemon\r\n"], "m.eml", {type: "message/rfc822"});
    const imported = await messenger.messages.import(eml, inbox.id || inbox).catch(e => ({err: String(e)}));
    await post("imported", imported);
    const q = await messenger.messages.query({subject: "probe"}).catch(e => ({err: String(e)}));
    await post("query", q);
    const full = q.messages?.[0] ? await messenger.messages.getFull(q.messages[0].id) : null;
    await post("full", full);
    messenger.messages.onNewMailReceived.addListener((f, l) => post("newmail", {f, l}));
    // long-poll style command channel test
    const ws = new WebSocket("ws://127.0.0.1:8766");
    ws.onopen = () => post("ws", {open: true});
    ws.onerror = e => post("ws", {error: String(e)});
    await post("done", {});
  } catch (e) { await post("fatal", {e: String(e), stack: e.stack}); }
})();
