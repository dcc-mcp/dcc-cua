import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { runInNewContext } from "node:vm";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import * as protocol from "../protocol.ts";
import * as content from "../content-protocol.ts";
import { PairingLifecycle } from "../pairing.ts";

const flush = () => new Promise(resolve => setImmediate(resolve));
function event() {
  let listener: (...args: any[]) => void = () => {};
  return { addListener(value: typeof listener) { listener = value; }, fire(...args: any[]) { listener(...args); } };
}
function fixture() {
  const clicked = event();
  const ports: any[] = [];
  const responses: any[] = [];
  let focused = true;
  let commands = 0;
  let stored: unknown = { session_nonce: "stale", tab_id: 1, window_id: 2, origin: "https://example.com", document_id: "old" };
  const browser = {
    action: { onClicked: clicked, setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {} },
    runtime: {
      id: "published-extension", lastError: null, getManifest: () => ({ version: "0.3.1" }),
      connectNative: () => {
        const port = { onDisconnect: event(), onMessage: event(), postMessage: (value: unknown) => { responses.push(value); }, disconnect() { this.onDisconnect.fire(); } };
        ports.push(port); return port;
      },
    },
    storage: { session: { get: async () => ({ dcc_cua_pairing_v1: stored }), remove: async () => { stored = undefined; }, set: async (value: any) => { stored = value.dcc_cua_pairing_v1; } }, onChanged: event() },
    scripting: { executeScript: async () => {} },
    windows: { get: async () => ({ focused }) },
    tabs: { get: async () => ({ active: true, windowId: 2 }), onUpdated: event(), onRemoved: event(), sendMessage: async () => { commands += 1; return { ok: true, result: { document: { origin: "https://example.com", id: "doc" } } }; } },
  };
  const source = readFileSync(new URL("../entrypoints/background.ts", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?;\r?\n/gm, "").replace(/export default /g, "").replace(/export /g, "");
  runInNewContext(stripTypeScriptTypes(source), { ...protocol, ...content, PairingLifecycle, browser, defineBackground: (run: () => void) => run(), crypto: { randomUUID }, URL });
  return { ports, responses, loseFocus: () => { focused = false; }, commands: () => commands, stored: () => stored, async pair() { clicked.fire({ id: 1, windowId: 2, url: "https://example.com" }); await flush(); } };
}

test("background clears restored pairing and requires explicit pairing after disconnect", async () => {
  const f = fixture(); await flush();
  assert.equal(f.stored(), undefined); assert.equal(f.ports.length, 0);
  await f.pair(); assert.equal(f.ports.length, 1); assert.ok(f.stored());
  f.ports[0].disconnect(); await flush();
  assert.equal(f.stored(), undefined); assert.equal(f.ports.length, 1);
});

test("old native port cannot clear a replacement pairing and old hosts fail closed", async () => {
  const f = fixture(); await flush(); await f.pair(); await f.pair();
  assert.equal(f.ports.length, 2); assert.ok(f.stored());
  f.ports[0].disconnect(); await flush(); assert.ok(f.stored());
  f.ports[1].onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["native_host_bridge_v1"] });
  await flush(); assert.equal(f.stored(), undefined);
});


test("background denies extension input after the paired window loses focus", async () => {
  const f = fixture(); await flush(); await f.pair();
  const port = f.ports[0];
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["task_owned_pairing_v1"] });
  await flush(); f.loseFocus();
  const pairing = f.stored() as any;
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, request_id: "input", method: "click", session_nonce: pairing.session_nonce, tab_id: 1, snapshot_id: "fresh", ref: "control" });
  await flush();
  assert.equal(f.responses.at(-1).error.code, "target_not_foreground");
  assert.equal(f.commands(), 1);
});
