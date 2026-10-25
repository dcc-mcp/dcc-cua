import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { runInNewContext } from "node:vm";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import * as protocol from "../protocol.ts";
import * as content from "../content-protocol.ts";
import { PairingLifecycle } from "../pairing.ts";
import { navigatePairedTab } from "../navigation.ts";

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
  const navigations: string[] = [];
  let navigationGate: Promise<void> | null = null;
  let badge = "";
  let nextRemove: Promise<void> | null = null;
  let nextSet: Promise<void> | null = null;
  let stored: unknown = { session_nonce: "stale", tab_id: 1, window_id: 2, origin: "https://example.com", document_id: "old" };
  function pauseStorage(kind: "remove" | "set") {
    let resume!: () => void;
    const gate = new Promise<void>(resolve => { resume = resolve; });
    if (kind === "remove") nextRemove = gate;
    else nextSet = gate;
    return resume;
  }
  const browser = {
    action: { onClicked: clicked, setBadgeText: async ({ text }: { text: string }) => { badge = text; }, setBadgeBackgroundColor: async () => {} },
    runtime: {
      id: "published-extension", lastError: null, getManifest: () => ({ version: "0.3.1" }),
      connectNative: () => {
        const port = {
          onDisconnect: event(), onMessage: event(), disconnected: false,
          postMessage(value: unknown) { if (this.disconnected) throw new Error("disconnected port"); responses.push(value); },
          // Chrome fires onDisconnect at the other end, never at this local caller.
          disconnect() { this.disconnected = true; },
        };
        ports.push(port); return port;
      },
    },
    storage: { session: {
      get: async () => ({ dcc_cua_pairing_v1: stored }),
      remove: async () => { const gate = nextRemove; nextRemove = null; if (gate) await gate; stored = undefined; },
      set: async (value: any) => { const gate = nextSet; nextSet = null; if (gate) await gate; stored = value.dcc_cua_pairing_v1; },
    }, onChanged: event() },
    scripting: { executeScript: async () => {} },
    windows: { get: async () => ({ focused }) },
    tabs: { get: async () => ({ active: true, windowId: 2 }), update: async (_tab: number, value: { url: string }) => { navigations.push(value.url); const gate = navigationGate; navigationGate = null; if (gate) await gate; }, onUpdated: event(), onRemoved: event(), sendMessage: async (_tab: number, value: any) => { commands += 1; return { ok: true, result: value.command.method === "prepare_navigation" ? { invalidated: true } : { document: { origin: "https://example.com", id: "doc" } } }; } },
  };
  const source = readFileSync(new URL("../entrypoints/background.ts", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?;\r?\n/gm, "").replace(/export default /g, "").replace(/export /g, "");
  runInNewContext(stripTypeScriptTypes(source), { ...protocol, ...content, PairingLifecycle, navigatePairedTab, browser, defineBackground: (run: () => void) => run(), crypto: { randomUUID }, URL });
  return { ports, responses, navigations, pauseNavigation() { let resume!: () => void; navigationGate = new Promise<void>(resolve => { resume = resolve; }); return resume; }, badge: () => badge, pauseClear: () => pauseStorage("remove"), pauseSave: () => pauseStorage("set"), loseFocus: () => { focused = false; }, commands: () => commands, stored: () => stored, async pair() { clicked.fire({ id: 1, windowId: 2, url: "https://example.com" }); await flush(); } };
}

test("background clears restored pairing and requires explicit pairing after disconnect", async () => {
  const f = fixture(); await flush();
  assert.equal(f.stored(), undefined); assert.equal(f.ports.length, 0);
  await f.pair(); assert.equal(f.ports.length, 1); assert.ok(f.stored());
  f.ports[0].onDisconnect.fire(); await flush();
  assert.equal(f.stored(), undefined); assert.equal(f.ports.length, 1);
});

test("old native port cannot clear a replacement pairing and old hosts fail closed", async () => {
  const f = fixture(); await flush(); await f.pair(); await f.pair();
  assert.equal(f.ports.length, 2); assert.ok(f.stored());
  f.ports[0].onDisconnect.fire(); await flush(); assert.ok(f.stored());
  f.ports[1].onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["native_host_bridge_v1"] });
  await flush(); assert.equal(f.stored(), undefined);
});

test("a locally rejected native host clears pairing and badge without an onDisconnect event", async () => {
  const f = fixture(); await flush(); await f.pair();
  const port = f.ports[0];
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["native_host_bridge_v1"] });
  await flush();
  assert.equal(port.disconnected, true);
  assert.equal(f.stored(), undefined);
  assert.equal(f.badge(), "!");
  const responses = f.responses.length;
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["task_owned_pairing_v1"] });
  await flush();
  assert.equal(f.badge(), "!");
  assert.equal(f.responses.length, responses);
});

test("a pending failure clear cannot remove a replacement pairing", async () => {
  const f = fixture(); await flush(); await f.pair();
  const previous = f.ports[0];
  const oldPairing = f.stored() as any;
  const resume = f.pauseClear();
  previous.onDisconnect.fire(); await flush();
  await f.pair();
  resume(); await flush();
  assert.equal(f.ports.length, 2);
  assert.ok(f.stored());
  assert.notEqual((f.stored() as any).session_nonce, oldPairing.session_nonce);
  const replacement = f.ports[1];
  replacement.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["task_owned_pairing_v1"] });
  await flush(); previous.onDisconnect.fire(); await flush();
  assert.ok(f.stored());
  assert.equal(f.badge(), "ON");
});

test("failure during a replacement save cannot clear the replacement generation", async () => {
  const f = fixture(); await flush(); await f.pair();
  const previous = f.ports[0];
  const oldPairing = f.stored() as any;
  const resume = f.pauseSave();
  await f.pair();
  previous.onDisconnect.fire(); await flush();
  resume(); await flush();
  assert.equal(f.ports.length, 2);
  assert.notEqual((f.stored() as any).session_nonce, oldPairing.session_nonce);
  f.ports[1].onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["task_owned_pairing_v1"] });
  await flush(); previous.onDisconnect.fire(); await flush();
  assert.ok(f.stored());
  assert.equal(f.badge(), "ON");
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

test("real background navigates only its paired tab and consumes pairing before replay", async () => {
  const f = fixture(); await flush(); await f.pair();
  const port = f.ports[0];
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["task_owned_pairing_v1"] });
  await flush();
  const pairing = f.stored() as any;
  const request = { schema: protocol.PROTOCOL_SCHEMA, request_id: "navigate-1", method: "navigate",
    session_nonce: pairing.session_nonce, tab_id: 1, snapshot_id: "fresh", url: "https://example.com/next", deadline_unix_ms: Date.now() + 10000 };
  port.onMessage.fire(request); await flush();
  assert.deepEqual(f.navigations, [request.url]);
  assert.equal(f.stored(), undefined);
  assert.equal(f.responses.at(-1).result.completion_known, false);
  port.onMessage.fire(request); await flush();
  assert.equal(f.responses.at(-1).ok, false);
  assert.equal(f.navigations.length, 1);
  await f.pair();
  const replacement = f.stored();
  port.onDisconnect.fire(); await flush();
  assert.equal(f.stored(), replacement);
});

test("late navigation completion cannot clear or report success for a replacement pairing", async () => {
  const f = fixture(); await flush(); await f.pair();
  const port = f.ports[0];
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, type: "hello_ack", protocol: 1, capabilities: ["task_owned_pairing_v1"] });
  await flush();
  const pairing = f.stored() as any;
  const resume = f.pauseNavigation();
  port.onMessage.fire({ schema: protocol.PROTOCOL_SCHEMA, request_id: "old-navigation", method: "navigate",
    session_nonce: pairing.session_nonce, tab_id: 1, snapshot_id: "fresh", url: "https://example.com/next", deadline_unix_ms: Date.now() + 10000 });
  await flush(); assert.equal(f.navigations.length, 1);
  await f.pair(); const replacement = f.stored();
  resume(); await flush();
  assert.equal(f.stored(), replacement);
  assert.equal(f.responses.some((value: any) => value.request_id === "old-navigation" && value.ok), false);
});
