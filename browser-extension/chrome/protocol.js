export const PROTOCOL_SCHEMA = "dcc-cua.browser-extension.v1";
export const PROTOCOL_MIN = 1;
export const PROTOCOL_MAX = 1;
export const MAX_REQUEST_ID_CHARS = 128;
export const MAX_TEXT_CHARS = 4096;
export const MAX_SNAPSHOT_NODES = 300;

const METHODS = new Set(["ping", "snapshot", "click", "type", "unpair"]);
const COMMON_KEYS = new Set([
  "schema",
  "request_id",
  "method",
  "session_nonce",
  "tab_id",
]);
const METHOD_KEYS = {
  ping: new Set(),
  snapshot: new Set(["max_nodes"]),
  click: new Set(["snapshot_id", "ref"]),
  type: new Set(["snapshot_id", "ref", "text", "replace"]),
  unpair: new Set(),
};

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function boundedString(value, name, maximum) {
  if (typeof value !== "string" || value.length === 0 || value.length > maximum) {
    throw new Error(`${name} must be a non-empty string of at most ${maximum} characters`);
  }
  return value;
}

function positiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive safe integer`);
  }
  return value;
}

export function validateNativeRequest(message) {
  if (!isObject(message)) {
    throw new Error("native request must be an object");
  }
  if (message.schema !== PROTOCOL_SCHEMA) {
    throw new Error("native request schema is unsupported");
  }
  const requestId = boundedString(
    message.request_id,
    "request_id",
    MAX_REQUEST_ID_CHARS,
  );
  const method = boundedString(message.method, "method", 32);
  if (!METHODS.has(method)) {
    throw new Error(`unsupported native method: ${method}`);
  }
  const allowed = new Set([...COMMON_KEYS, ...METHOD_KEYS[method]]);
  for (const key of Object.keys(message)) {
    if (!allowed.has(key)) {
      throw new Error(`unexpected native request field: ${key}`);
    }
  }
  if (method === "ping") {
    return { schema: PROTOCOL_SCHEMA, request_id: requestId, method };
  }
  const sessionNonce = boundedString(message.session_nonce, "session_nonce", 64);
  const tabId = positiveInteger(message.tab_id, "tab_id");
  const request = {
    schema: PROTOCOL_SCHEMA,
    request_id: requestId,
    method,
    session_nonce: sessionNonce,
    tab_id: tabId,
  };
  if (method === "snapshot") {
    const maxNodes = message.max_nodes ?? MAX_SNAPSHOT_NODES;
    if (
      !Number.isSafeInteger(maxNodes) ||
      maxNodes < 1 ||
      maxNodes > MAX_SNAPSHOT_NODES
    ) {
      throw new Error(`max_nodes must be between 1 and ${MAX_SNAPSHOT_NODES}`);
    }
    request.max_nodes = maxNodes;
  }
  if (method === "click" || method === "type") {
    request.snapshot_id = boundedString(message.snapshot_id, "snapshot_id", 128);
    request.ref = boundedString(message.ref, "ref", 128);
  }
  if (method === "type") {
    if (typeof message.text !== "string" || message.text.length > MAX_TEXT_CHARS) {
      throw new Error(`text must be a string of at most ${MAX_TEXT_CHARS} characters`);
    }
    if (message.replace !== true) {
      throw new Error("type currently requires replace=true");
    }
    request.text = message.text;
    request.replace = true;
  }
  return request;
}

export function buildHello(pairing) {
  const manifest = chrome.runtime.getManifest();
  return {
    schema: PROTOCOL_SCHEMA,
    type: "hello",
    protocol: { min: PROTOCOL_MIN, max: PROTOCOL_MAX },
    extension: {
      id: chrome.runtime.id,
      version: manifest.version,
    },
    capabilities: [
      "explicit_tab_pairing_v1",
      "semantic_snapshot_v1",
      "dom_click_v1",
      "dom_replace_text_v1",
    ],
    pairing: pairing ?? null,
  };
}

export function successResponse(requestId, result) {
  return {
    schema: PROTOCOL_SCHEMA,
    type: "response",
    request_id: requestId,
    ok: true,
    result,
  };
}

export function errorResponse(requestId, code, message) {
  return {
    schema: PROTOCOL_SCHEMA,
    type: "response",
    request_id:
      typeof requestId === "string" && requestId.length <= MAX_REQUEST_ID_CHARS
        ? requestId
        : "invalid-request",
    ok: false,
    error: {
      code: boundedString(code, "error code", 64),
      message: boundedString(message, "error message", 256),
    },
  };
}
