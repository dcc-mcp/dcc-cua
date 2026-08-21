import assert from "node:assert/strict";
import test from "node:test";

import {
  PROTOCOL_SCHEMA,
  errorResponse,
  successResponse,
  validateNativeRequest,
} from "../protocol.js";

function base(method) {
  return {
    schema: PROTOCOL_SCHEMA,
    request_id: "request-1",
    method,
    session_nonce: "session-1",
    tab_id: 42,
  };
}

test("ping is the only request that does not require a pairing", () => {
  assert.deepEqual(
    validateNativeRequest({
      schema: PROTOCOL_SCHEMA,
      request_id: "ping-1",
      method: "ping",
    }),
    {
      schema: PROTOCOL_SCHEMA,
      request_id: "ping-1",
      method: "ping",
    },
  );
  assert.throws(() =>
    validateNativeRequest({
      schema: PROTOCOL_SCHEMA,
      request_id: "snapshot-1",
      method: "snapshot",
    }),
  );
});

test("snapshot bounds are fail closed", () => {
  assert.equal(validateNativeRequest({ ...base("snapshot"), max_nodes: 300 }).max_nodes, 300);
  for (const max_nodes of [0, 301, 1.5, "10"]) {
    assert.throws(() => validateNativeRequest({ ...base("snapshot"), max_nodes }));
  }
});

test("click and type require exact snapshot evidence", () => {
  const click = validateNativeRequest({
    ...base("click"),
    snapshot_id: "snapshot-1",
    ref: "p1:1",
  });
  assert.equal(click.ref, "p1:1");

  const type = validateNativeRequest({
    ...base("type"),
    snapshot_id: "snapshot-1",
    ref: "p1:2",
    text: "dcc-mcp-core",
    replace: true,
  });
  assert.equal(type.text, "dcc-mcp-core");
  assert.throws(() =>
    validateNativeRequest({ ...type, replace: false }),
  );
  assert.throws(() =>
    validateNativeRequest({ ...type, text: "x".repeat(4097) }),
  );
});

test("unknown methods and fields cannot widen the protocol", () => {
  assert.throws(() => validateNativeRequest({ ...base("execute_script") }));
  assert.throws(() =>
    validateNativeRequest({ ...base("snapshot"), arbitrary_code: "alert(1)" }),
  );
});

test("response envelopes preserve correlation and bounded errors", () => {
  assert.deepEqual(successResponse("request-1", { pong: true }), {
    schema: PROTOCOL_SCHEMA,
    type: "response",
    request_id: "request-1",
    ok: true,
    result: { pong: true },
  });
  const error = errorResponse("request-2", "pairing_mismatch", "pairing changed");
  assert.equal(error.request_id, "request-2");
  assert.equal(error.ok, false);
  assert.equal(error.error.code, "pairing_mismatch");
});
