import {
  PROTOCOL_SCHEMA,
  buildHello,
  errorResponse,
  successResponse,
  validateNativeRequest,
} from "./protocol.js";

const NATIVE_HOST = "com.dcc_mcp.dcc_cua";
const PAIRING_STORAGE_KEY = "dcc_cua_pairing_v1";

let nativePort = null;
let requestQueue = Promise.resolve();

async function setBadge(text, color) {
  await chrome.action.setBadgeText({ text });
  if (text) {
    await chrome.action.setBadgeBackgroundColor({ color });
  }
}

async function loadPairing() {
  const stored = await chrome.storage.session.get(PAIRING_STORAGE_KEY);
  return stored[PAIRING_STORAGE_KEY] ?? null;
}

async function savePairing(pairing) {
  if (pairing === null) {
    await chrome.storage.session.remove(PAIRING_STORAGE_KEY);
  } else {
    await chrome.storage.session.set({ [PAIRING_STORAGE_KEY]: pairing });
  }
}

async function ensureContentScript(tabId) {
  await chrome.scripting.executeScript({
    target: { tabId },
    files: ["content-script.js"],
  });
}

async function sendTabCommand(tabId, command) {
  await ensureContentScript(tabId);
  const response = await chrome.tabs.sendMessage(tabId, {
    type: "dcc_cua_command",
    command,
  });
  if (!response || response.ok !== true) {
    const message = response?.error?.message ?? "paired tab did not return a valid response";
    const code = response?.error?.code ?? "tab_command_failed";
    const error = new Error(message);
    error.code = code;
    throw error;
  }
  return response.result;
}

async function pairTab(tab) {
  if (!Number.isSafeInteger(tab.id) || !Number.isSafeInteger(tab.windowId)) {
    throw new Error("Chrome did not provide an exact tab and window identity");
  }
  const url = new URL(tab.url ?? "");
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error("only http and https tabs can be paired");
  }
  const sessionNonce = crypto.randomUUID();
  const result = await sendTabCommand(tab.id, {
    schema: PROTOCOL_SCHEMA,
    method: "pair",
    session_nonce: sessionNonce,
  });
  const pairing = {
    session_nonce: sessionNonce,
    tab_id: tab.id,
    window_id: tab.windowId,
    origin: result.document.origin,
    document_id: result.document.id,
  };
  await savePairing(pairing);
  connectNative(pairing);
  await setBadge("ON", "#1f883d");
}

function connectNative(pairing) {
  if (nativePort !== null) {
    nativePort.disconnect();
  }
  nativePort = chrome.runtime.connectNative(NATIVE_HOST);
  nativePort.onMessage.addListener((message) => {
    requestQueue = requestQueue
      .then(() => handleNativeMessage(message))
      .catch(() => setBadge("!", "#cf222e"));
  });
  nativePort.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    nativePort = null;
    void setBadge("!", "#cf222e");
  });
  nativePort.postMessage(buildHello(pairing));
}

async function handleNativeMessage(message) {
  let requestId = message?.request_id;
  try {
    const request = validateNativeRequest(message);
    requestId = request.request_id;
    if (request.method === "ping") {
      nativePort?.postMessage(successResponse(requestId, { pong: true }));
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
      nativePort?.postMessage(successResponse(requestId, { unpaired: true }));
      return;
    }
    const result = await sendTabCommand(pairing.tab_id, request);
    nativePort?.postMessage(successResponse(requestId, result));
  } catch (error) {
    nativePort?.postMessage(
      errorResponse(
        requestId,
        typeof error?.code === "string" ? error.code : "invalid_request",
        error instanceof Error ? error.message : "native request failed",
      ),
    );
  }
}

chrome.action.onClicked.addListener((tab) => {
  void pairTab(tab).catch(async () => {
    await setBadge("!", "#cf222e");
  });
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status !== "loading") {
    return;
  }
  void loadPairing().then(async (pairing) => {
    if (pairing?.tab_id === tabId) {
      await savePairing(null);
      await setBadge("", "#000000");
    }
  });
});

chrome.tabs.onRemoved.addListener((tabId) => {
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
    void setBadge("ON", "#1f883d");
  }
});
