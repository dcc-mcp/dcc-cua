import assert from "node:assert/strict";
import { test } from "node:test";
import { navigatePairedTab } from "../navigation.ts";
import type { NavigationOperations } from "../navigation.ts";
import type { Pairing } from "../protocol.ts";
import { validateNativeRequest } from "../protocol.ts";

const pairing: Pairing = { session_nonce: "nonce", tab_id: 7, window_id: 9,
  origin: "https://example.test", document_id: "doc" };
const request = { schema: "dcc-cua.browser-extension.v1", request_id: "request", method: "navigate",
  session_nonce: "nonce", tab_id: 7, snapshot_id: "fresh", url: "https://example.test/next",
  deadline_unix_ms: 1000 } as const;
test("native protocol accepts only a bounded navigation command with deadline and snapshot", () => {
  assert.equal(validateNativeRequest(request).method, "navigate");
  for (const change of [{ deadline_unix_ms: 0 }, { snapshot_id: "" }, { url: "file:///private" },
    { url: "https://user:secret@example.test/" }]) {
    assert.throws(() => validateNativeRequest({ ...request, ...change }));
  }
});
function fixture() {
  const events: string[] = [];
  let current = true;
  let now = 1;
  let paired = true;
  const ops: NavigationOperations = {
    isCurrent: () => current, now: () => now,
    async requirePairing() { if (!paired) throw new Error("pairing_mismatch"); },
    async foreground() { return true; },
    async invalidateDocument() { events.push("invalidate"); },
    async revokePairing() { paired = false; events.push("revoke"); },
    async updateTab(tab, url) { assert.equal(tab, 7); assert.equal(url, request.url); events.push("update"); },
  };
  return { ops, events, cancel() { current = false; }, expire() { now = 1000; } };
}
test("navigation revokes document and pairing before one exact-tab dispatch; replay fails", async () => {
  const f = fixture();
  const result = await navigatePairedTab(request, pairing, f.ops);
  assert.deepEqual(f.events, ["invalidate", "revoke", "update"]);
  assert.equal(result.phase, "dispatched"); assert.equal(result.completion_known, false);
  assert.equal(result.pairing_required, true);
  await assert.rejects(navigatePairedTab(request, pairing, f.ops), /pairing_mismatch/);
  assert.equal(f.events.filter(e => e === "update").length, 1);
});
test("cross-origin, credentials, another tab or nonce never dispatch", async () => {
  for (const change of [{ url: "https://other.test/" }, { url: "https://user:secret@example.test/" },
    { url: "javascript:alert(1)" }, { tab_id: 8 }, { session_nonce: "other" }]) {
    const f = fixture();
    await assert.rejects(navigatePairedTab({ ...request, ...change }, pairing, f.ops), /navigation_target_refused/);
    assert.deepEqual(f.events, []);
  }
});
test("expiration, cancellation and focus loss before admission never update a tab", async () => {
  for (const failure of ["expire", "cancel", "focus"] as const) {
    const f = fixture();
    if (failure === "expire") f.expire();
    if (failure === "cancel") f.cancel();
    if (failure === "focus") f.ops.foreground = async () => false;
    await assert.rejects(navigatePairedTab(request, pairing, f.ops));
    assert.deepEqual(f.events, []);
  }
});
test("cancellation during document invalidation cannot revoke a replacement pairing or navigate", async () => {
  const f = fixture(); f.ops.invalidateDocument = async () => { f.cancel(); };
  await assert.rejects(navigatePairedTab(request, pairing, f.ops), /navigation_cancelled/);
  assert.deepEqual(f.events, []);
});
test("a stale document refusal prevents navigation and preserves pairing", async () => {
  const f = fixture(); f.ops.invalidateDocument = async () => { throw new Error("stale_observation"); };
  await assert.rejects(navigatePairedTab(request, pairing, f.ops), /stale_observation/);
  assert.deepEqual(f.events, []);
});
test("browser error, cancellation or timeout after admission is unknown and never retried", async () => {
  for (const failure of ["error", "cancel", "expire"] as const) {
    const f = fixture();
    f.ops.updateTab = async () => {
      f.events.push("update");
      if (failure === "error") throw new Error("private browser error");
      if (failure === "cancel") f.cancel(); else f.expire();
    };
    await assert.rejects(navigatePairedTab(request, pairing, f.ops), /navigation_outcome_unknown/);
    assert.deepEqual(f.events, ["invalidate", "revoke", "update"]);
  }
});
