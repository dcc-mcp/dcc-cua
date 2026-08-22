import assert from "node:assert/strict";
import test from "node:test";

import {
  buildPairCommand,
  contentCommandFromNative,
  validateContentCommand,
} from "../content-protocol.ts";
import { PROTOCOL_SCHEMA, validateNativeRequest } from "../protocol.ts";

test("content commands use the same bounded contract as native requests", () => {
  const request = validateNativeRequest({
    schema: PROTOCOL_SCHEMA,
    request_id: "request-1",
    method: "snapshot",
    session_nonce: "session-1",
    tab_id: 42,
    max_nodes: 300,
  });
  if (request.method !== "snapshot") throw new Error("expected snapshot request");

  assert.deepEqual(contentCommandFromNative(request), {
    schema: PROTOCOL_SCHEMA,
    method: "snapshot",
    session_nonce: "session-1",
    max_nodes: 300,
  });
});

test("content parser rejects unknown fields and weaker bounds", () => {
  const pair = buildPairCommand("session-1");
  assert.deepEqual(validateContentCommand(pair), pair);
  assert.throws(() => validateContentCommand({ ...pair, ignored: true }));
  assert.throws(() =>
    validateContentCommand({
      schema: PROTOCOL_SCHEMA,
      method: "snapshot",
      session_nonce: "session-1",
      max_nodes: 301,
    }),
  );
  assert.throws(() =>
    validateContentCommand({
      schema: PROTOCOL_SCHEMA,
      method: "type",
      session_nonce: "session-1",
      snapshot_id: "snapshot-1",
      ref: "p1:1",
      text: "x".repeat(4097),
      replace: true,
    }),
  );
});

test("content translation strips native-only routing fields", () => {
  const request = validateNativeRequest({
    schema: PROTOCOL_SCHEMA,
    request_id: "request-2",
    method: "click",
    session_nonce: "session-1",
    tab_id: 42,
    snapshot_id: "snapshot-1",
    ref: "p1:1",
  });
  if (request.method !== "click") throw new Error("expected click request");

  const content = contentCommandFromNative(request);
  assert.equal("request_id" in content, false);
  assert.equal("tab_id" in content, false);
  assert.deepEqual(validateContentCommand(content), content);
});
