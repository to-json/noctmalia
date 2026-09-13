var { cal } = ChromeUtils.importESModule("resource:///modules/calendar/calUtils.sys.mjs");
this.dbg = class extends ExtensionAPI {
  getAPI(context) {
    return { dbg: { async run(code) {
      try {
        const fn = new (Object.getPrototypeOf(async function(){}).constructor)("cal", "Services", code);
        const r = await fn(cal, Services);
        return JSON.stringify(r);
      } catch (e) { return "THREW: " + String(e) + "\n" + (e.stack || ""); }
    } } };
  }
};
