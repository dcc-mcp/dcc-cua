import assert from "node:assert/strict";
import test from "node:test";

import { createContentScriptEnsurer } from "../entrypoints/background.ts";

test("injects once per document and coalesces concurrent commands", async () => {
  const calls: number[] = [];
  let release!: () => void;
  const injection = new Promise<void>((resolve) => {
    release = resolve;
  });
  const ensurer = createContentScriptEnsurer(async (tabId) => {
    calls.push(tabId);
    await injection;
  });

  const first = ensurer.ensure(42);
  const second = ensurer.ensure(42);
  release();
  await Promise.all([first, second]);
  await ensurer.ensure(42);

  assert.deepEqual(calls, [42]);
});

test("navigation invalidates the document injection", async () => {
  const calls: number[] = [];
  const ensurer = createContentScriptEnsurer(async (tabId) => {
    calls.push(tabId);
  });

  await ensurer.ensure(42);
  ensurer.invalidate(42);
  await ensurer.ensure(42);

  assert.deepEqual(calls, [42, 42]);
});

test("failed injection is retryable", async () => {
  let attempts = 0;
  const ensurer = createContentScriptEnsurer(async () => {
    attempts += 1;
    if (attempts === 1) throw new Error("injection failed");
  });

  await assert.rejects(ensurer.ensure(42), /injection failed/);
  await ensurer.ensure(42);
  assert.equal(attempts, 2);
});
