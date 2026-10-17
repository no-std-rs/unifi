import { test } from "node:test";
import assert from "node:assert/strict";
import { DisplaySerial } from "../serial-client.js";

function fixture(handle = () => null) {
  let input, outgoing = "", closed = 0;
  const commands = [];
  const encoder = new TextEncoder();
  const response = data => input.enqueue(encoder.encode(`boot noise\nUNIFI_APP ${JSON.stringify(data)}\n`));
  const port = {
    readable: new ReadableStream({ start(c) { input = c; } }),
    writable: new WritableStream({ write(bytes) {
      outgoing += new TextDecoder().decode(bytes);
      const lines = outgoing.split("\n"); outgoing = lines.pop();
      for (const line of lines) {
        if (!line.startsWith("UNIFI_APP ")) continue;
        const request = JSON.parse(line.slice(10)); commands.push(request);
        let result;
        if (request.command.name === "hello") result = { nonce: request.command.nonce, protocol: 1, board: "ESP32-2424S012", capabilities: ["scan_wifi", "test_wifi", "save_config"] };
        else result = handle(request, response);
        if (result !== undefined) response({ v: 1, id: request.id, session: 123, ok: true, result });
      }
    } }),
    open: async () => {}, close: async () => { closed++; },
  };
  return { serial: { requestPort: async () => port }, commands, response, raw: text => input.enqueue(encoder.encode(text)), unplug: () => input.close(), get closed() { return closed; } };
}

test("handshake, noise, split lines, session binding and parallel heartbeat", async () => {
  const f = fixture((request, reply) => {
    if (request.command.name === "status") {
      reply({ v: 1, id: request.id, session: 999, ok: true, result: "stale" });
      setTimeout(() => reply({ v: 1, id: request.id, session: 123, ok: true, result: "current" }), 25);
      return undefined;
    }
    return null;
  });
  const client = new DisplaySerial(); await client.connect(f.serial);
  assert.equal(client.connected, true);
  f.raw("UNIFI_APP {bad json}\n" + "x".repeat(9000) + "\n");
  const pending = client.command({ name: "status" });
  assert.equal(await client.command({ name: "ping" }), null);
  assert.equal(await pending, "current");
  await client.disconnect();
  assert.equal(client.connected, false); assert.equal(f.closed, 1);
  assert.equal(f.commands.at(-1).command.name, "bye");
});

test("timeouts, device errors and physical unplug reject requests and release the port", async () => {
  let disconnects = 0;
  const f = fixture((request, reply) => {
    if (request.command.name === "test_wifi") reply({ v: 1, id: request.id, session: 123, ok: false, error: "Wrong password" });
    return undefined;
  });
  const client = new DisplaySerial(() => disconnects++); await client.connect(f.serial);
  await assert.rejects(client.command({ name: "test_wifi" }), /Wrong password/);
  await assert.rejects(client.command({ name: "status" }, 10), /respond in time/);
  const request = client.command({ name: "scan_wifi" });
  const rejected = assert.rejects(request, /disconnected/);
  f.unplug(); await rejected;
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(f.closed, 1); assert.equal(disconnects, 1);
});


test("an unplug between open and stream acquisition closes the port and permits retry", async () => {
  let closed=0;
  const client=new DisplaySerial();
  await assert.rejects(client.connect({requestPort:async()=>({open:async()=>{},writable:null,close:async()=>{closed++;}})}));
  assert.equal(closed,1);
  const f=fixture();
  await client.connect(f.serial);
  assert.equal(client.connected,true);
  await client.disconnect();
});
