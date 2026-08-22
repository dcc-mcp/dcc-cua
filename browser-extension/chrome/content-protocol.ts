import {
  MAX_SNAPSHOT_NODES,
  MAX_TEXT_CHARS,
  PROTOCOL_SCHEMA,
  boundedString,
  isObject,
  type NativeRequest,
} from "./protocol.ts";

export type Action = "click" | "type";
export type PairCommand = {
  schema: typeof PROTOCOL_SCHEMA;
  method: "pair";
  session_nonce: string;
};
export type SnapshotCommand = {
  schema: typeof PROTOCOL_SCHEMA;
  method: "snapshot";
  session_nonce: string;
  max_nodes: number;
};
export type EvidenceCommand = {
  schema: typeof PROTOCOL_SCHEMA;
  session_nonce: string;
  snapshot_id: string;
  ref: string;
};
export type ClickCommand = EvidenceCommand & { method: "click" };
export type TypeCommand = EvidenceCommand & { method: "type"; text: string; replace: true };
export type ContentCommand = PairCommand | SnapshotCommand | ClickCommand | TypeCommand;

type ForwardedNativeRequest = Exclude<NativeRequest, { method: "ping" | "unpair" }>;
type ContentMethod = ContentCommand["method"];

const METHODS = new Set<ContentMethod>(["pair", "snapshot", "click", "type"]);
const COMMON_KEYS = new Set(["schema", "method", "session_nonce"]);
const METHOD_KEYS: Record<ContentMethod, ReadonlySet<string>> = {
  pair: new Set(),
  snapshot: new Set(["max_nodes"]),
  click: new Set(["snapshot_id", "ref"]),
  type: new Set(["snapshot_id", "ref", "text", "replace"]),
};

function parseContentCommand(value: unknown): ContentCommand {
  if (!isObject(value) || value.schema !== PROTOCOL_SCHEMA) {
    throw new Error("content command schema is unsupported");
  }
  const method = boundedString(value.method, "method", 32) as ContentMethod;
  if (!METHODS.has(method)) {
    throw new Error(`unsupported content method: ${method}`);
  }
  const allowed = new Set([...COMMON_KEYS, ...METHOD_KEYS[method]]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`unexpected content command field: ${key}`);
    }
  }
  const sessionNonce = boundedString(value.session_nonce, "session_nonce", 64);
  if (method === "pair") {
    return { schema: PROTOCOL_SCHEMA, method, session_nonce: sessionNonce };
  }
  if (method === "snapshot") {
    if (
      !Number.isSafeInteger(value.max_nodes) ||
      (value.max_nodes as number) < 1 ||
      (value.max_nodes as number) > MAX_SNAPSHOT_NODES
    ) {
      throw new Error(`max_nodes must be between 1 and ${MAX_SNAPSHOT_NODES}`);
    }
    return {
      schema: PROTOCOL_SCHEMA,
      method,
      session_nonce: sessionNonce,
      max_nodes: value.max_nodes as number,
    };
  }
  const evidence: EvidenceCommand = {
    schema: PROTOCOL_SCHEMA,
    session_nonce: sessionNonce,
    snapshot_id: boundedString(value.snapshot_id, "snapshot_id", 128),
    ref: boundedString(value.ref, "ref", 128),
  };
  if (method === "click") {
    return { ...evidence, method };
  }
  if (typeof value.text !== "string" || value.text.length > MAX_TEXT_CHARS) {
    throw new Error(`text must be a string of at most ${MAX_TEXT_CHARS} characters`);
  }
  if (value.replace !== true) {
    throw new Error("type currently requires replace=true");
  }
  return { ...evidence, method, text: value.text, replace: true };
}

export function validateContentCommand(value: unknown): ContentCommand {
  try {
    return parseContentCommand(value);
  } catch (error) {
    const failure = error instanceof Error ? error : new Error("content command is malformed");
    throw Object.assign(failure, { code: "invalid_request" });
  }
}

export function buildPairCommand(sessionNonce: string): PairCommand {
  return validateContentCommand({
    schema: PROTOCOL_SCHEMA,
    method: "pair",
    session_nonce: sessionNonce,
  }) as PairCommand;
}

export function contentCommandFromNative(request: ForwardedNativeRequest): ContentCommand {
  if (request.method === "snapshot") {
    return validateContentCommand({
      schema: PROTOCOL_SCHEMA,
      method: request.method,
      session_nonce: request.session_nonce,
      max_nodes: request.max_nodes,
    });
  }
  const evidence = {
    schema: PROTOCOL_SCHEMA,
    method: request.method,
    session_nonce: request.session_nonce,
    snapshot_id: request.snapshot_id,
    ref: request.ref,
  };
  if (request.method === "click") {
    return validateContentCommand(evidence);
  }
  return validateContentCommand({
    ...evidence,
    text: request.text,
    replace: request.replace,
  });
}
