// Native CDC USB protocol shared with the no_std Rust firmware's app module.
const prefix = "UNIFI_APP ";
const encoder = new TextEncoder();
export class DisplaySerial {
  constructor(onDisconnect = () => {}) {
    this.onDisconnect = onDisconnect;
    this.pending = new Map();
    this.id = crypto.getRandomValues(new Uint32Array(1))[0];
    this.session = 0;
    this.tail = Promise.resolve();
  }
  get connected() { return !!this.port && this.session !== 0; }
  async connect(serial = navigator.serial) {
    if (this.port) throw new Error("Disconnect the display first.");
    if (!serial) throw new Error("USB setup needs desktop Chrome or Edge over HTTPS.");
    const port = await serial.requestPort();
    await port.open({ baudRate: 115200, bufferSize: 16384 });
    this.port = port;
    this.session = 0;
    this.writer = port.writable.getWriter();
    this.reader = port.readable.getReader();
    this.tail = Promise.resolve();
    this.reading = this.readLoop(this.reader).catch(() => {}).finally(() => {
      if (this.port === port) void this.disconnect(false);
    });
    try {
      const nonce = [...crypto.getRandomValues(new Uint8Array(16))].map(n => n.toString(16).padStart(2, "0")).join("");
      const hello = await this.request({ name: "hello", nonce }, 8000, false);
      if (hello.result?.nonce !== nonce || hello.result?.protocol !== 1 || hello.result?.board !== "ESP32-2424S012"
          || !["scan_wifi", "test_wifi", "save_config"].every(c => hello.result.capabilities?.includes(c))
          || !Number.isInteger(hello.session) || hello.session <= 0 || hello.session > 0xffffffff) throw new Error("This firmware does not support USB setup. Install the latest firmware first.");
      this.session = hello.session;
      this.heartbeat = setInterval(() => { this.request({ name: "ping" }, 4000).catch(() => { void this.disconnect(false); }); }, 2000);
      return hello.result;
    } catch (error) { await this.disconnect(false); throw error; }
  }
  async readLoop(reader) {
    const decoder = new TextDecoder();
    let line = "", dropping = false;
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) return;
        for (const char of decoder.decode(value, { stream: true })) {
          if (char === "\n") {
            if (!dropping && line.startsWith(prefix)) {
              let response;
              try { response = JSON.parse(line.slice(prefix.length)); } catch { /* Boot/panic noise is not a protocol reply. */ }
              const pending = response && this.pending.get(response.id);
              if (pending && response.v === 1 && (!pending.session || response.session === pending.session)) {
                this.pending.delete(response.id); clearTimeout(pending.timer);
                if (response.ok === true) pending.resolve(response);
                else pending.reject(new Error(typeof response.error === "string" ? response.error : "Device request failed."));
              }
            }
            line = ""; dropping = false;
          } else if (!dropping) {
            if (line.length >= 8192) { line = ""; dropping = true; }
            else line += char;
          }
        }
      }
    } finally { reader.releaseLock(); }
  }
  request(command, timeout = 5000, needsSession = true) {
    if (!this.port || needsSession && !this.session) return Promise.reject(new Error("Connect your display first."));
    const id = this.id = (this.id + 1) >>> 0;
    const session = this.session;
    const frame = encoder.encode(`\n${prefix}${JSON.stringify({ v: 1, id, session, command })}\n`);
    if (frame.length > 4097) return Promise.reject(new Error("USB settings are too large."));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(command.name === "hello" ? "No setup response. Install the latest firmware, close the flasher, and reconnect." : "The display did not respond in time. Reconnect and try again."));
      }, timeout);
      this.pending.set(id, { resolve, reject, timer, session: needsSession ? session : 0 });
      const writer = this.writer;
      this.tail = this.tail.then(() => {
        if (!this.pending.has(id)) return;
        return writer.write(frame);
      }).catch(() => {
        const pending = this.pending.get(id);
        if (pending) { clearTimeout(pending.timer); this.pending.delete(id); pending.reject(new Error("USB write failed. Reconnect the display.")); }
      });
    });
  }
  async command(command, timeout = 5000) { return (await this.request(command, timeout)).result; }
  async disconnect(sendBye = true) {
    if (this.closing) return this.closing;
    if (!this.port) return;
    this.closing = this.close(sendBye);
    try { await this.closing; } finally { this.closing = undefined; }
  }
  async close(sendBye) {
    clearInterval(this.heartbeat);
    if (sendBye && this.connected) { try { await this.request({ name: "bye" }, 500); } catch { /* Session also expires without a heartbeat. */ } }
    const port = this.port;
    this.port = undefined; this.session = 0;
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(new Error("Display disconnected.")); }
    this.pending.clear();
    try { await this.reader?.cancel(); } catch { /* Physical unplug. */ }
    // readLoop releases its own reader, including when a cable is removed.
    // Do not await reading here: its finally callback can itself initiate close.
    try { await this.writer?.abort(); } catch { /* Physical unplug. */ }
    try { this.writer?.releaseLock(); } catch { /* Already closed. */ }
    try { await port.close(); } catch { /* Already disconnected. */ }
    this.onDisconnect();
  }
}
