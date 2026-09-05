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

function memoryStore(initial: unknown = null): PairingStore & { value: unknown; reads: number } {
  return {
    value: initial,
    reads: 0,
    async read() {
      this.reads += 1;
      return this.value;
    },
    async write(value) {
      this.value = value;
    },
  };
}

test("pairing reads storage once and serves cache hits", async () => {
  const store = memoryStore(pairing);
  const lifecycle = new PairingLifecycle(store);

  await lifecycle.requireMatch({ session_nonce: "session-1", tab_id: 42 });
  await lifecycle.requireMatch({ session_nonce: "session-1", tab_id: 42 });

  assert.equal(store.reads, 1);
});

test("pair and unpair update the cache without another storage read", async () => {
  const store = memoryStore();
  const lifecycle = new PairingLifecycle(store);

  await lifecycle.save(pairing);
  assert.deepEqual(await lifecycle.load(), pairing);
  await lifecycle.clear();
  assert.equal(await lifecycle.load(), null);
  assert.equal(store.reads, 0);
});

test("storage changes replace or invalidate the cached pairing", async () => {
  const store = memoryStore(pairing);
  const lifecycle = new PairingLifecycle(store);
  await lifecycle.load();

  const changed = { ...pairing, session_nonce: "session-2" };
  store.value = changed;
  lifecycle.updateFromStorage(changed);
  assert.deepEqual(
    await lifecycle.requireMatch({ session_nonce: "session-2", tab_id: 42 }),
    changed,
  );

  store.value = null;
  lifecycle.updateFromStorage(undefined);
  await assert.rejects(
    () => lifecycle.requireMatch({ session_nonce: "session-2", tab_id: 42 }),
    /explicitly paired tab/,
  );
  assert.equal(store.reads, 1);
});

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
