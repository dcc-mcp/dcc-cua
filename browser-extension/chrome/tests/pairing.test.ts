import assert from "node:assert/strict";
import test from "node:test";

import { PairingLifecycle, type PairingStore } from "../pairing.ts";
import type { Pairing } from "../protocol.ts";

const pairing: Pairing = {
  session_nonce: "session-1",
  tab_id: 42,
  window_id: 7,
  origin: "https://example.com",
  document_id: "document-1",
};

function memoryStore(initial: unknown = null): PairingStore & { value: unknown } {
  return {
    value: initial,
    async read() {
      return this.value;
    },
    async write(value) {
      this.value = value;
    },
  };
}

test("pairing survives an extension worker restart through session storage", async () => {
  const store = memoryStore();
  await new PairingLifecycle(store).save(pairing);

  const restored = await new PairingLifecycle(store).load();

  assert.deepEqual(restored, pairing);
});

test("malformed stored pairing fails closed and is removed", async () => {
  const store = memoryStore({ ...pairing, tab_id: "42" });

  await assert.rejects(() => new PairingLifecycle(store).load(), /malformed/);
  assert.equal(store.value, null);
});

test("native requests require the exact stored nonce and tab", async () => {
  const lifecycle = new PairingLifecycle(memoryStore(pairing));

  assert.deepEqual(
    await lifecycle.requireMatch({ session_nonce: "session-1", tab_id: 42 }),
    pairing,
  );
  await assert.rejects(
    () => lifecycle.requireMatch({ session_nonce: "session-2", tab_id: 42 }),
    (error: unknown) =>
      error instanceof Error &&
      "code" in error &&
      error.code === "pairing_mismatch",
  );
  await assert.rejects(
    () => lifecycle.requireMatch({ session_nonce: "session-1", tab_id: 43 }),
    (error: unknown) =>
      error instanceof Error &&
      "code" in error &&
      error.code === "pairing_mismatch",
  );
});

test("navigation and tab removal clear only the paired tab", async () => {
  const store = memoryStore(pairing);
  const lifecycle = new PairingLifecycle(store);

  assert.equal(await lifecycle.clearIfTab(41), false);
  assert.deepEqual(store.value, pairing);
  assert.equal(await lifecycle.clearIfTab(42), true);
  assert.equal(store.value, null);
});

test("explicit unpair clears the persisted pairing", async () => {
  const store = memoryStore(pairing);
  const lifecycle = new PairingLifecycle(store);

  await lifecycle.clear();

  assert.equal(await lifecycle.load(), null);
});
