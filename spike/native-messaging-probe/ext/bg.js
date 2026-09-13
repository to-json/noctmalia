const post = (t, d) => fetch("http://127.0.0.1:8765/" + t, {method: "POST", body: JSON.stringify(d)});
try {
  const port = messenger.runtime.connectNative("noctmalia.probe");
  let n = 0;
  port.onMessage.addListener(m => { n++; post("fromhost", m); if (m.pushed && n < 4) port.postMessage({ack: m.pushed}); });
  port.onDisconnect.addListener(p => post("disconnect", {err: String(p.error || messenger.runtime.lastError)}));
  port.postMessage({hello: 1, big: "x".repeat(2 * 1024 * 1024).length});
} catch (e) { post("fatal", {e: String(e)}); }
