import {
  PROTOCOL_SCHEMA,
  buildHello,
  errorResponse,
  successResponse,
  validateNativeHelloAck,
  validateNativeRequest,
  type NativeRequest,
  type Pairing,
} from "../protocol.ts";
import {
  buildPairCommand,
  contentCommandFromNative,
  type ContentCommand,
} from "../content-protocol.ts";
import { PairingLifecycle } from "../pairing.ts";
import { navigatePairedTab } from "../navigation.ts";
import { browser, type Browser } from "wxt/browser";
import { defineBackground } from "wxt/utils/define-background";

const NATIVE_HOST = "com.dcc_mcp.dcc_cua";
const PAIRING_STORAGE_KEY = "dcc_cua_pairing_v1";

const NATIVE_STATUS = {
  unpaired: ["", "Select the exact tab and click to pair."],
  native_connecting: ["...", "Waiting for the native host handshake."],
  native_compatible: ["LINK", "Native host compatible; task binding not verified."],
  native_incompatible: ["!", "Native host protocol or task-owned pairing capability is incompatible."],
  native_host_unavailable: ["!", "Browser cannot find the registered native host."],
  native_host_forbidden: ["!", "Browser refused native host access for this extension."],
  native_disconnected: ["!", "Native host disconnected; check the task and pair the exact tab again."],
  native_message_rejected: ["!", "Native message rejected; pair the exact tab again."],
  pairing_failed: ["!", "Exact-tab pairing failed; check site access and the selected tab."],
} as const;

type NativeStatus = keyof typeof NATIVE_STATUS;

type TabResponse =
  | { ok: true; result: unknown }
  | { ok: false; error: { code: string; message: string } };

type ContentScriptState = {
  generation: number;
  promise: Promise<void>;
};

export type ContentScriptEnsurer = {
  ensure(tabId: number): Promise<void>;
  invalidate(tabId: number): void;
  remove(tabId: number): void;
};

/**
 * Injects the bridge once per tab document while coalescing concurrent calls.
 * A navigation invalidates the generation before the next command is sent.
 */
export function createContentScriptEnsurer(
  inject: (tabId: number) => Promise<void>,
): ContentScriptEnsurer {
  const generations = new Map<number, number>();
  const states = new Map<number, ContentScriptState>();

  function generationFor(tabId: number): number {
    return generations.get(tabId) ?? 0;
  }

  function invalidate(tabId: number): void {
    generations.set(tabId, generationFor(tabId) + 1);
    states.delete(tabId);
  }

  function remove(tabId: number): void {
    generations.delete(tabId);
    states.delete(tabId);
  }

  async function ensure(tabId: number): Promise<void> {
    const generation = generationFor(tabId);
    const existing = states.get(tabId);
    if (existing?.generation === generation) {
      return existing.promise;
    }

    const promise = Promise.resolve()
      .then(() => inject(tabId))
      .catch((error: unknown) => {
        if (states.get(tabId)?.promise === promise) {
          states.delete(tabId);
        }
        throw error;
      });
    states.set(tabId, { generation, promise });
    return promise;
  }

  return { ensure, invalidate, remove };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
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
  let nativePortGeneration = 0;
  let nativeHandshakeComplete = false;
  let requestQueue: Promise<void> = Promise.resolve();
  let pairingUpdates: Promise<void> = Promise.resolve();
  const pairings = new PairingLifecycle({
    async read(): Promise<unknown> {
      const stored = await browser.storage.session.get(PAIRING_STORAGE_KEY);
      return stored[PAIRING_STORAGE_KEY];
    },
    async write(pairing: Pairing | null): Promise<void> {
      if (pairing === null) {
        await browser.storage.session.remove(PAIRING_STORAGE_KEY);
      } else {
        await browser.storage.session.set({ [PAIRING_STORAGE_KEY]: pairing });
      }
    },
  });
  const contentScripts = createContentScriptEnsurer(async (tabId) => {
    await browser.scripting.executeScript({
      target: { tabId },
      files: ["/tab-bridge.js"],
    });
  });

  async function setStatus(status: NativeStatus): Promise<void> {
    const [text, description] = NATIVE_STATUS[status];
    // Never reflect page content, browser error text or protocol payloads here.
    await browser.action.setTitle({ title: `DCC-CUA: ${status} — ${description}` });
    await browser.action.setBadgeText({ text });
    if (text) {
      await browser.action.setBadgeBackgroundColor({ color: text === "!" ? "#cf222e" : "#0969da" });
    }
  }

  function updatePairing(update: () => Promise<void>): Promise<void> {
    const next = pairingUpdates.then(update);
    pairingUpdates = next.catch(() => {});
    return next;
  }

  async function ensureContentScript(tabId: number): Promise<void> {
    await contentScripts.ensure(tabId);
  }

  async function sendTabCommand(tabId: number, command: ContentCommand): Promise<unknown> {
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
    const result = await sendTabCommand(tabId, buildPairCommand(sessionNonce));
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
    await updatePairing(async () => {
      await pairings.save(pairing);
      await setStatus("native_connecting");
      connectNative(pairing);
    });
  }

  function failCurrentPort(port: Browser.runtime.Port, reason: NativeStatus): void {
    if (nativePort !== port) return;
    const generation = nativePortGeneration;
    nativePort = null;
    nativeHandshakeComplete = false;
    void updatePairing(async () => {
      if (nativePortGeneration !== generation) return;
      try {
        await pairings.clear();
      } finally {
        if (nativePortGeneration === generation) await setStatus(reason);
      }
    }).catch(() => {});
  }

  function connectNative(pairing: Pairing): void {
    const previous = nativePort;
    const port = browser.runtime.connectNative(NATIVE_HOST);
    nativePort = port;
    nativePortGeneration += 1;
    previous?.disconnect();
    nativeHandshakeComplete = false;
    requestQueue = Promise.resolve();
    port.onMessage.addListener((message: unknown) => {
      if (nativePort !== port) return;
      requestQueue = requestQueue
        .then(async () => {
          if (nativePort !== port) return;
          if (!nativeHandshakeComplete) {
            const ack = validateNativeHelloAck(message);
            if (!ack.capabilities.includes("task_owned_pairing_v1")) {
              throw new Error("native host upgrade required for task-owned pairing");
            }
            nativeHandshakeComplete = true;
            await setStatus("native_compatible");
            return;
          }
          await handleNativeMessage(message, port);
        })
        .catch(() => {
          if (nativePort !== port) return;
          failCurrentPort(port, nativeHandshakeComplete ? "native_message_rejected" : "native_incompatible");
          port.disconnect();
        });
    });
    port.onDisconnect.addListener(() => {
      const portError = "error" in port && isObject(port.error) ? port.error.message : undefined;
      const message = typeof portError === "string" ? portError : browser.runtime.lastError?.message;
      const reason = message === "Specified native messaging host not found."
        ? "native_host_unavailable"
        : message === "Access to the specified native messaging host is forbidden."
          ? "native_host_forbidden"
          : "native_disconnected";
      failCurrentPort(port, reason);
    });
    const manifest = browser.runtime.getManifest();
    port.postMessage(
      buildHello(pairing, { id: browser.runtime.id, version: manifest.version }),
    );
  }

  async function handleNativeMessage(message: unknown, port: Browser.runtime.Port): Promise<void> {
    let requestId: unknown = isObject(message) ? message.request_id : undefined;
    try {
      const request = validateNativeRequest(message);
      requestId = request.request_id;
      if (request.method === "ping") {
        port.postMessage(successResponse(request.request_id, { pong: true }));
        return;
      }
      const pairing = await pairings.requireMatch(request);
      if (request.method === "navigate") {
        const result = await navigatePairedTab(request, pairing, {
          isCurrent: () => nativePort === port && nativeHandshakeComplete,
          now: () => Date.now(),
          async requirePairing() { await pairings.requireMatch(request); },
          async foreground() {
            const [window, tab] = await Promise.all([browser.windows.get(pairing.window_id), browser.tabs.get(pairing.tab_id)]);
            return window.focused === true && tab.active === true && tab.windowId === pairing.window_id;
          },
          async invalidateDocument() {
            const value = await sendTabCommand(pairing.tab_id, contentCommandFromNative(request));
            if (!isObject(value) || value.invalidated !== true) {
              throw Object.assign(new Error("document invalidation was not confirmed"), { code: "stale_observation" });
            }
          },
          async revokePairing() {
            await updatePairing(async () => {
              if (nativePort !== port) throw new Error("pairing replaced");
              await pairings.requireMatch(request);
              await pairings.clear();
              await setStatus("unpaired");
            });
          },
          async updateTab(tabId, url) { await browser.tabs.update(tabId, { url }); },
        });
        port.postMessage(successResponse(request.request_id, result));
        return;
      }
      if (request.method === "unpair") {
        await pairings.clear();
        await setStatus("unpaired");
        port.postMessage(successResponse(request.request_id, { unpaired: true }));
        return;
      }
      if (request.method === "click" || request.method === "type") {
        const [window, tab] = await Promise.all([browser.windows.get(pairing.window_id), browser.tabs.get(pairing.tab_id)]);
        if (!window.focused || !tab.active || tab.windowId !== pairing.window_id) {
          throw Object.assign(new Error("paired tab must be active in the focused browser window"), { code: "target_not_foreground" });
        }
      }
      if (nativePort !== port) throw new Error("pairing replaced before dispatch");
      const result = await sendTabCommand(pairing.tab_id, contentCommandFromNative(request));
      port.postMessage(successResponse(request.request_id, result));
    } catch (error) {
      port.postMessage(
        errorResponse(
          requestId,
          errorCode(error, "invalid_request"),
          error instanceof Error ? error.message : "native request failed",
        ),
      );
    }
  }

  browser.action.onClicked.addListener((tab) => {
    void pairTab(tab).catch(() => setStatus("pairing_failed"));
  });

  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    if (changeInfo.status !== "loading") return;
    contentScripts.invalidate(tabId);
    void pairings.clearIfTab(tabId).then(async (cleared) => {
      if (cleared) {
        await setStatus("unpaired");
      }
    });
  });

  browser.tabs.onRemoved.addListener((tabId) => {
    contentScripts.remove(tabId);
    void pairings.clearIfTab(tabId).then(async (cleared) => {
      if (cleared) {
        await setStatus("unpaired");
      }
    });
  });

  browser.storage.onChanged.addListener((changes, areaName) => {
    if (areaName !== "session") return;
    const change = changes[PAIRING_STORAGE_KEY];
    if (change !== undefined) {
      pairings.updateFromStorage(change.newValue);
    }
  });

  void updatePairing(async () => {
    await pairings.clear();
    await setStatus("unpaired");
  }).catch(() => setStatus("pairing_failed"));
});
