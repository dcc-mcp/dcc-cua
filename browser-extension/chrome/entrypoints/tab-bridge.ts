import { browser } from "wxt/browser";
import { defineUnlistedScript } from "wxt/utils/define-unlisted-script";

declare global {
  var __dccCuaContentBridgeV1: boolean | undefined;
}

type Action = "click" | "type";
type PairCommand = { method: "pair"; session_nonce: string };
type SnapshotCommand = { method: "snapshot"; session_nonce: string; max_nodes: number };
type EvidenceCommand = {
  session_nonce: string;
  snapshot_id: string;
  ref: string;
};
type ClickCommand = EvidenceCommand & { method: "click" };
type TypeCommand = EvidenceCommand & { method: "type"; text: string; replace: true };
type ContentCommand = PairCommand | SnapshotCommand | ClickCommand | TypeCommand;
type RefEntry = { element: HTMLElement; actions: Action[] };

type BridgeState = {
  sessionNonce: string | null;
  documentId: string | null;
  documentUrl: string | null;
  generation: number;
  snapshotId: string | null;
  refs: Map<string, RefEntry>;
};

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function commandFrom(value: unknown): ContentCommand {
  if (!isObject(value) || typeof value.method !== "string" || typeof value.session_nonce !== "string") {
    throw Object.assign(new Error("content command is malformed"), { code: "invalid_request" });
  }
  if (value.method === "pair") {
    return { method: "pair", session_nonce: value.session_nonce };
  }
  if (value.method === "snapshot" && Number.isSafeInteger(value.max_nodes)) {
    return { method: "snapshot", session_nonce: value.session_nonce, max_nodes: value.max_nodes as number };
  }
  if (
    (value.method === "click" || value.method === "type") &&
    typeof value.snapshot_id === "string" &&
    typeof value.ref === "string"
  ) {
    if (value.method === "click") {
      return {
        method: "click",
        session_nonce: value.session_nonce,
        snapshot_id: value.snapshot_id,
        ref: value.ref,
      };
    }
    if (typeof value.text === "string" && value.replace === true) {
      return {
        method: "type",
        session_nonce: value.session_nonce,
        snapshot_id: value.snapshot_id,
        ref: value.ref,
        text: value.text,
        replace: true,
      };
    }
  }
  throw Object.assign(new Error("content command is unsupported or malformed"), {
    code: "invalid_request",
  });
}

function errorCode(error: unknown): string {
  return isObject(error) && typeof error.code === "string" ? error.code : "content_command_failed";
}

export default defineUnlistedScript(() => {
  if (globalThis.__dccCuaContentBridgeV1) return;

  const MAX_NAME_CHARS = 256;
  const state: BridgeState = {
    sessionNonce: null,
    documentId: null,
    documentUrl: null,
    generation: 0,
    snapshotId: null,
    refs: new Map(),
  };

  function boundedText(value: unknown, maximum = MAX_NAME_CHARS): string {
    return String(value ?? "").replace(/\s+/gu, " ").trim().slice(0, maximum);
  }

  function documentIdentity(): string {
    return `${location.origin}:${crypto.randomUUID()}`;
  }

  function elementRole(element: HTMLElement): string {
    const explicit = boundedText(element.getAttribute("role"), 64).toLowerCase();
    if (explicit) return explicit;
    const tag = element.tagName.toLowerCase();
    const roles: Record<string, string> = {
      a: "link",
      button: "button",
      input:
        element instanceof HTMLInputElement && element.type === "checkbox" ? "checkbox" : "textbox",
      select: "combobox",
      summary: "button",
      textarea: "textbox",
    };
    return roles[tag] ?? "generic";
  }

  function elementName(element: HTMLElement): string {
    const controlValue =
      element instanceof HTMLInputElement ||
      element instanceof HTMLTextAreaElement ||
      element instanceof HTMLSelectElement
        ? element.value
        : "";
    return boundedText(
      element.getAttribute("aria-label") ||
        element.getAttribute("alt") ||
        element.getAttribute("title") ||
        element.getAttribute("placeholder") ||
        element.innerText ||
        controlValue,
    );
  }

  function visibility(element: HTMLElement): "in_viewport" | "offscreen" | null {
    if (element.hidden || element.getAttribute("aria-hidden") === "true") return null;
    const style = getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) {
      return null;
    }
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return null;
    const inViewport =
      rect.bottom > 0 &&
      rect.right > 0 &&
      rect.top < window.innerHeight &&
      rect.left < window.innerWidth;
    return inViewport ? "in_viewport" : "offscreen";
  }

  function elementActions(element: HTMLElement): Action[] {
    if (element.matches(":disabled") || element.getAttribute("aria-disabled") === "true") {
      return [];
    }
    const actions: Action[] = [];
    if (
      element.matches("a[href],button,input[type=button],input[type=submit],summary") ||
      element.getAttribute("role") === "button" ||
      element.hasAttribute("onclick")
    ) {
      actions.push("click");
    }
    if (
      element.matches("input:not([type=button]):not([type=submit]),textarea") ||
      element.isContentEditable
    ) {
      actions.push("type");
    }
    return actions;
  }

  function semanticSnapshot(maxNodes: number) {
    state.generation += 1;
    state.snapshotId = `${state.generation}-${crypto.randomUUID()}`;
    state.refs.clear();
    state.documentUrl = location.href;
    const selectors = [
      "a[href]",
      "button",
      "input",
      "textarea",
      "select",
      "summary",
      "[contenteditable=true]",
      "[onclick]",
      "[role=button]",
      "[role=link]",
      "[role=menuitem]",
    ].join(",");
    const refs: Array<Record<string, unknown>> = [];
    for (const element of document.querySelectorAll<HTMLElement>(selectors)) {
      if (refs.length >= maxNodes) break;
      const visible = visibility(element);
      const actions = elementActions(element);
      if (visible === null || actions.length === 0) continue;
      const ref = `p${state.generation}:${refs.length + 1}`;
      state.refs.set(ref, { element, actions });
      refs.push({
        ref,
        role: elementRole(element),
        name: elementName(element),
        actions,
        states: { disabled: false, focusable: element.tabIndex >= 0 },
        visibility: visible,
        frame: "main",
      });
    }
    return {
      snapshot_id: state.snapshotId,
      generation: state.generation,
      complete: refs.length < maxNodes,
      document: {
        id: state.documentId,
        origin: location.origin,
        url: boundedText(location.href, 2048),
        title: boundedText(document.title),
      },
      refs,
    };
  }

  function resolveRef(command: ClickCommand | TypeCommand, action: Action): HTMLElement {
    if (state.documentUrl !== location.href || command.snapshot_id !== state.snapshotId) {
      throw Object.assign(new Error("snapshot is no longer current"), {
        code: "stale_observation",
      });
    }
    const entry = state.refs.get(command.ref);
    if (!entry || !entry.element.isConnected || !entry.actions.includes(action)) {
      throw Object.assign(new Error("semantic ref is stale or does not allow this action"), {
        code: "stale_observation",
      });
    }
    state.refs.clear();
    state.snapshotId = null;
    return entry.element;
  }

  function click(command: ClickCommand) {
    resolveRef(command, "click").click();
    return {
      action_attempted: true,
      dom_event_dispatched: true,
      physical_input_sent: false,
      effect: "unverified",
      fresh_observation_required: true,
    };
  }

  function replaceText(command: TypeCommand) {
    const element = resolveRef(command, "type");
    if (element instanceof HTMLInputElement && element.type === "password") {
      throw Object.assign(new Error("password fields remain a trusted human boundary"), {
        code: "trusted_human_required",
      });
    }
    if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
      const prototype =
        element instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
      if (!setter) {
        throw Object.assign(new Error("text control has no native value setter"), {
          code: "unsupported_control",
        });
      }
      setter.call(element, command.text);
    } else if (element.isContentEditable) {
      element.textContent = command.text;
    } else {
      throw Object.assign(new Error("semantic ref is not a supported text control"), {
        code: "unsupported_control",
      });
    }
    element.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText" }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
    return {
      action_attempted: true,
      dom_event_dispatched: true,
      physical_input_sent: false,
      effect: "unverified",
      fresh_observation_required: true,
    };
  }

  function handle(command: ContentCommand): unknown {
    if (command.method === "pair") {
      state.sessionNonce = command.session_nonce;
      state.documentId = documentIdentity();
      state.documentUrl = location.href;
      state.refs.clear();
      state.snapshotId = null;
      return {
        paired: true,
        document: {
          id: state.documentId,
          origin: location.origin,
          url: boundedText(location.href, 2048),
        },
      };
    }
    if (!state.sessionNonce || command.session_nonce !== state.sessionNonce) {
      throw Object.assign(new Error("command does not match the paired session"), {
        code: "pairing_mismatch",
      });
    }
    if (command.method === "snapshot") return semanticSnapshot(command.max_nodes);
    if (command.method === "click") return click(command);
    return replaceText(command);
  }

  browser.runtime.onMessage.addListener((message: unknown, _sender, sendResponse) => {
    if (!isObject(message) || message.type !== "dcc_cua_command") return false;
    try {
      sendResponse({ ok: true, result: handle(commandFrom(message.command)) });
    } catch (error) {
      sendResponse({
        ok: false,
        error: {
          code: errorCode(error),
          message: error instanceof Error ? error.message.slice(0, 256) : "content command failed",
        },
      });
    }
    return false;
  });

  globalThis.__dccCuaContentBridgeV1 = true;
});
