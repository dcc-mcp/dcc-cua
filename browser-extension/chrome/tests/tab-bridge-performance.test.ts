import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const bridgeSource = await readFile(new URL("../entrypoints/tab-bridge.ts", import.meta.url), "utf8");

test("semantic snapshots filter disabled candidates before layout work", () => {
  const actionCheck = bridgeSource.indexOf("const actions = elementActions(element);");
  const visibilityCheck = bridgeSource.indexOf("const visible = visibility(element);");

  assert.ok(actionCheck >= 0);
  assert.ok(visibilityCheck > actionCheck);
  assert.match(bridgeSource, /if \(actions\.length === 0\) continue;/);
});

test("semantic snapshots use a bounded candidate safety margin", () => {
  assert.match(bridgeSource, /const candidateLimit = Math\.min\(maxNodes \* 4, 1_200\);/);
  assert.match(bridgeSource, /const scannedCandidates = Math\.min\(candidates\.length, candidateLimit\);/);
  assert.match(bridgeSource, /complete: refs\.length < maxNodes && candidates\.length <= candidateLimit/);
});

test("semantic names prefer cheap textContent before innerText", () => {
  const textContentRead = bridgeSource.indexOf("const textContent = element.textContent");
  const innerTextRead = bridgeSource.indexOf("innerText: element.innerText");

  assert.ok(textContentRead >= 0);
  assert.ok(innerTextRead > textContentRead);
});
