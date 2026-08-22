import {
  PROTOCOL_SCHEMA,
  buildHello,
  errorResponse,
  successResponse,
  validateNativeHelloAck,
  validateNativeRequest,
  type NativeRequest,
  type Pairing,
} from "../protocol";
import { browser, type Browser } from "wxt/browser";
import { defineBackground } from "wxt/utils/define-background";

const NATIVE_HOST = "com.dcc_mcp.dcc_cua";
const PAIRING_STORAGE_KEY = "dcc_cua_pairing_v1";

type PairCommand = {
  schema: typeof PROTOCOL_SCHEMA;
  method: "pair";
  session_nonce: string;
};
type TabCommand = PairCommand | Exclude<NativeRequest, { method: "ping" | "unpair" }>;
type TabResponse =
  | { ok: true; result: unknown }
  | { ok: false; error: { code: string; message: string } };

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isPairing(value: unknown): value is Pairing {
  return (
    isObject(value) &&
    typeof value.session_nonce === "string" &&
    Number.isSafeInteger(value.tab_id) &&
    Number.isSafeInteger(value.window_id) &&
    typeof value.origin === "string" &&
    typeof value.document_id === "string"
  );
}

function errorCode(error: unknown, fallback: string): string {
  return isObject(error) && typeof error.code === "string" ? error.code : fallback;
}

function tabResponse(value: unknown): TabResponse {
  if (!isObject(value) || typeof value.ok !== "boolean") {
    throw Object.assign(new Error("paired tab did not return a valid response"), {
      code: "tab_command_failed",
    });
  }
  if (value.ok === true && "result" in value) {
    return { ok: true, result: value.result };
  }
  if (
    value.ok === false &&
    isObject(value.error) &&
    typeof value.error.code === "string" &&
    typeof value.error.message === "string"
  ) {
    return { ok: false, error: { code: value.error.code, message: value.error.message } };
  }
  throw Object.assign(new Error("paired tab returned a malformed response"), {
    code: "tab_command_failed",
  });
}

export default defineBackground(() => {
  let nativePort: Browser.runtime.Port | null = null;
  let nativeHandshakeComplete = false;
  let requestQueue: Promise<void> = Promise.resolve();

  async function setBadge(text: string, color: string): Promise<void> {
    await browser.action.setBadgeText({ text });
    if (text) {
      await browser.action.setBadgeBackgroundColor({ color });
    }
  }

  async function loadPairing(): Promise<Pairing | null> {
    const stored = await browser.storage.session.get(PAIRING_STORAGE_KEY);
    const value = stored[PAIRING_STORAGE_KEY];
    if (value == null) {
      return null;
    }
    if (!isPairing(value)) {
      await browser.storage.session.remove(PAIRING_STORAGE_KEY);
      throw new Error("stored browser pairing is malformed");
    }
    return value;
  }

  async function savePairing(pairing: Pairing | null): Promise<void> {
    if (pairing === null) {
      await browser.storage.session.remove(PAIRING_STORAGE_KEY);
    } else {
      await browser.storage.session.set({ [PAIRING_STORAGE_KEY]: pairing });
    }
  }

  async function ensureContentScript(tabId: number): Promise<void> {
    await browser.scripting.executeScript({
      target: { tabId },
      files: ["/tab-bridge.js"],
    });
  }

  async function sendTabCommand(tabId: number, command: TabCommand): Promise<unknown> {
    await ensureContentScript(tabId);
    const response = tabResponse(
      await browser.tabs.sendMessage(tabId, { type: "dcc_cua_command", command }),
    );
    if (!response.ok) {
      throw Object.assign(new Error(response.error.message), { code: response.error.code });
    }
    return response.result;
  }

  async function pairTab(tab: Browser.tabs.Tab): Promise<void> {
    if (!Number.isSafeInteger(tab.id) || !Number.isSafeInteger(tab.windowId)) {
      throw new Error("browser did not provide an exact tab and window identity");
    }
    const tabId = tab.id as number;
    const windowId = tab.windowId as number;
    const url = new URL(tab.url ?? "");
    if (url.protocol !== "https:" && url.protocol !== "http:") {
      throw new Error("only http and https tabs can be paired");
    }
    const sessionNonce = crypto.randomUUID();
    const result = await sendTabCommand(tabId, {
      schema: PROTOCOL_SCHEMA,
      method: "pair",
      session_nonce: sessionNonce,
    });
    if (!isObject(result) || !isObject(result.document)) {
      throw new Error("paired tab did not return a document identity");
    }
    const { origin, id } = result.document;
    if (typeof origin !== "string" || typeof id !== "string") {
      throw new Error("paired tab returned a malformed document identity");
    }
    const pairing: Pairing = {
      session_nonce: sessionNonce,
      tab_id: tabId,
      window_id: windowId,
      origin,
      document_id: id,
    };
    await savePairing(pairing);
    connectNative(pairing);
  }

  function connectNative(pairing: Pairing): void {
    nativePort?.disconnect();
    nativePort = browser.runtime.connectNative(NATIVE_HOST);
    nativeHandshakeComplete = false;
    requestQueue = Promise.resolve();
    nativePort.onMessage.addListener((message: unknown) => {
      requestQueue = requestQueue
        .then(async () => {
          if (!nativeHandshakeComplete) {
            validateNativeHelloAck(message);
            nativeHandshakeComplete = true;
            await setBadge("ON", "#1f883d");
            return;
          }
          await handleNativeMessage(message);
        })
        .catch(() => setBadge("!", "#cf222e"));
    });
    nativePort.onDisconnect.addListener(() => {
      void browser.runtime.lastError;
      nativePort = null;
      nativeHandshakeComplete = false;
      void setBadge("!", "#cf222e");
    });
    const manifest = browser.runtime.getManifest();
    nativePort.postMessage(
      buildHello(pairing, { id: browser.runtime.id, version: manifest.version }),
    );
  }

  async function handleNativeMessage(message: unknown): Promise<void> {
    let requestId: unknown = isObject(message) ? message.request_id : undefined;
    try {
      const request = validateNativeRequest(message);
      requestId = request.request_id;
      if (request.method === "ping") {
        nativePort?.postMessage(successResponse(request.request_id, { pong: true }));
        return;
      }
      const pairing = await loadPairing();
      if (
        pairing === null ||
        pairing.session_nonce !== request.session_nonce ||
        pairing.tab_id !== request.tab_id
      ) {
        throw Object.assign(new Error("request does not match the explicitly paired tab"), {
          code: "pairing_mismatch",
        });
      }
      if (request.method === "unpair") {
        await savePairing(null);
        await setBadge("", "#000000");
        nativePort?.postMessage(successResponse(request.request_id, { unpaired: true }));
        return;
      }
      const result = await sendTabCommand(pairing.tab_id, request);
      nativePort?.postMessage(successResponse(request.request_id, result));
    } catch (error) {
      nativePort?.postMessage(
        errorResponse(
          requestId,
          errorCode(error, "invalid_request"),
          error instanceof Error ? error.message : "native request failed",
        ),
      );
    }
  }

  browser.action.onClicked.addListener((tab) => {
    void pairTab(tab).catch(() => setBadge("!", "#cf222e"));
  });

  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    if (changeInfo.status !== "loading") return;
    void loadPairing().then(async (pairing) => {
      if (pairing?.tab_id === tabId) {
        await savePairing(null);
        await setBadge("", "#000000");
      }
    });
  });

  browser.tabs.onRemoved.addListener((tabId) => {
    void loadPairing().then(async (pairing) => {
      if (pairing?.tab_id === tabId) {
        await savePairing(null);
        await setBadge("", "#000000");
      }
    });
  });

  void loadPairing().then((pairing) => {
    if (pairing !== null) {
      connectNative(pairing);
    }
  });
});
