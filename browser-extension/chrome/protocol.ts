export const PROTOCOL_SCHEMA = "dcc-cua.browser-extension.v1";
export const PROTOCOL_MIN = 1;
export const PROTOCOL_MAX = 1;
export const MAX_REQUEST_ID_CHARS = 128;
export const MAX_TEXT_CHARS = 4096;
export const MAX_SNAPSHOT_NODES = 300;

export type NativeMethod = "ping" | "snapshot" | "click" | "type" | "unpair";

type PairedRequestBase = {
  schema: typeof PROTOCOL_SCHEMA;
  request_id: string;
  session_nonce: string;
  tab_id: number;
};

export type NativeRequest =
  | { schema: typeof PROTOCOL_SCHEMA; request_id: string; method: "ping" }
  | (PairedRequestBase & { method: "snapshot"; max_nodes: number })
  | (PairedRequestBase & { method: "click"; snapshot_id: string; ref: string })
  | (PairedRequestBase & {
      method: "type";
      snapshot_id: string;
      ref: string;
      text: string;
      replace: true;
    })
  | (PairedRequestBase & { method: "unpair" });

export type Pairing = {
  session_nonce: string;
  tab_id: number;
  window_id: number;
  origin: string;
  document_id: string;
};

const METHODS = new Set<NativeMethod>(["ping", "snapshot", "click", "type", "unpair"]);
const COMMON_KEYS = new Set(["schema", "request_id", "method", "session_nonce", "tab_id"]);
const METHOD_KEYS: Record<NativeMethod, ReadonlySet<string>> = {
  ping: new Set(),
  snapshot: new Set(["max_nodes"]),
  click: new Set(["snapshot_id", "ref"]),
  type: new Set(["snapshot_id", "ref", "text", "replace"]),
  unpair: new Set(),
};

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function boundedString(value: unknown, name: string, maximum: number): string {
  if (typeof value !== "string" || value.length === 0 || value.length > maximum) {
    throw new Error(`${name} must be a non-empty string of at most ${maximum} characters`);
  }
  return value;
}

function positiveInteger(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) <= 0) {
    throw new Error(`${name} must be a positive safe integer`);
  }
  return value as number;
}

function nativeMethod(value: unknown): NativeMethod {
  const method = boundedString(value, "method", 32) as NativeMethod;
  if (!METHODS.has(method)) {
    throw new Error(`unsupported native method: ${method}`);
  }
  return method;
}

export function validateNativeRequest(message: unknown): NativeRequest {
  if (!isObject(message)) {
    throw new Error("native request must be an object");
  }
  if (message.schema !== PROTOCOL_SCHEMA) {
    throw new Error("native request schema is unsupported");
  }
  const requestId = boundedString(message.request_id, "request_id", MAX_REQUEST_ID_CHARS);
  const method = nativeMethod(message.method);
  const allowed = new Set([...COMMON_KEYS, ...METHOD_KEYS[method]]);
  for (const key of Object.keys(message)) {
    if (!allowed.has(key)) {
      throw new Error(`unexpected native request field: ${key}`);
    }
  }
  if (method === "ping") {
    return { schema: PROTOCOL_SCHEMA, request_id: requestId, method };
  }

  const base: PairedRequestBase = {
    schema: PROTOCOL_SCHEMA,
    request_id: requestId,
    session_nonce: boundedString(message.session_nonce, "session_nonce", 64),
    tab_id: positiveInteger(message.tab_id, "tab_id"),
  };
  if (method === "snapshot") {
    const maxNodes = message.max_nodes ?? MAX_SNAPSHOT_NODES;
    if (!Number.isSafeInteger(maxNodes) || (maxNodes as number) < 1 || (maxNodes as number) > MAX_SNAPSHOT_NODES) {
      throw new Error(`max_nodes must be between 1 and ${MAX_SNAPSHOT_NODES}`);
    }
    return { ...base, method, max_nodes: maxNodes as number };
  }

  if (method === "unpair") {
    return { ...base, method };
  }
  const evidence = {
    snapshot_id: boundedString(message.snapshot_id, "snapshot_id", 128),
    ref: boundedString(message.ref, "ref", 128),
  };
  if (method === "click") {
    return { ...base, method, ...evidence };
  }
  if (typeof message.text !== "string" || message.text.length > MAX_TEXT_CHARS) {
    throw new Error(`text must be a string of at most ${MAX_TEXT_CHARS} characters`);
  }
  if (message.replace !== true) {
    throw new Error("type currently requires replace=true");
  }
  return { ...base, method, ...evidence, text: message.text, replace: true };
}

export function buildHello(pairing: Pairing | null, extension: { id: string; version: string }) {
  return {
    schema: PROTOCOL_SCHEMA,
    type: "hello",
    protocol: { min: PROTOCOL_MIN, max: PROTOCOL_MAX },
    extension: {
      id: boundedString(extension.id, "extension id", 128),
      version: boundedString(extension.version, "extension version", 64),
    },
    capabilities: [
      "explicit_tab_pairing_v1",
      "semantic_snapshot_v1",
      "dom_click_v1",
      "dom_replace_text_v1",
    ],
    pairing,
  };
}

export function successResponse(requestId: string, result: unknown) {
  return { schema: PROTOCOL_SCHEMA, type: "response", request_id: requestId, ok: true, result };
}

export function errorResponse(requestId: unknown, code: string, message: string) {
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
