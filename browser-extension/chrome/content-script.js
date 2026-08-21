(() => {
  if (globalThis.__dccCuaContentBridgeV1) {
    return;
  }

  const MAX_NAME_CHARS = 256;
  const state = {
    sessionNonce: null,
    documentId: null,
    documentUrl: null,
    generation: 0,
    snapshotId: null,
    refs: new Map(),
  };

  function boundedText(value, maximum = MAX_NAME_CHARS) {
    return String(value ?? "")
      .replace(/\s+/gu, " ")
      .trim()
      .slice(0, maximum);
  }

  function documentIdentity() {
    return `${location.origin}:${crypto.randomUUID()}`;
  }

  function elementRole(element) {
    const explicit = boundedText(element.getAttribute("role"), 64).toLowerCase();
    if (explicit) {
      return explicit;
    }
    const tag = element.tagName.toLowerCase();
    return {
      a: "link",
      button: "button",
      input: element.type === "checkbox" ? "checkbox" : "textbox",
      select: "combobox",
      summary: "button",
      textarea: "textbox",
    }[tag] ?? "generic";
  }

  function elementName(element) {
    return boundedText(
      element.getAttribute("aria-label") ||
        element.getAttribute("alt") ||
        element.getAttribute("title") ||
        element.getAttribute("placeholder") ||
        element.innerText ||
        element.value,
    );
  }

  function visibility(element) {
    if (element.hidden || element.getAttribute("aria-hidden") === "true") {
      return null;
    }
    const style = getComputedStyle(element);
    if (
      style.display === "none" ||
      style.visibility === "hidden" ||
      Number(style.opacity) === 0
    ) {
      return null;
    }
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return null;
    }
    const inViewport =
      rect.bottom > 0 &&
      rect.right > 0 &&
      rect.top < window.innerHeight &&
      rect.left < window.innerWidth;
    return inViewport ? "in_viewport" : "offscreen";
  }

  function elementActions(element) {
    if (element.matches(":disabled") || element.getAttribute("aria-disabled") === "true") {
      return [];
    }
    const actions = [];
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

  function semanticSnapshot(maxNodes) {
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
    const refs = [];
    for (const element of document.querySelectorAll(selectors)) {
      if (refs.length >= maxNodes) {
        break;
      }
      const visible = visibility(element);
      const actions = elementActions(element);
      if (visible === null || actions.length === 0) {
        continue;
      }
      const ref = `p${state.generation}:${refs.length + 1}`;
      state.refs.set(ref, { element, actions });
      refs.push({
        ref,
        role: elementRole(element),
        name: elementName(element),
        actions,
        states: {
          disabled: false,
          focusable: element.tabIndex >= 0,
        },
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

  function resolveRef(command, action) {
    if (state.documentUrl !== location.href) {
      throw Object.assign(new Error("document URL changed after the snapshot"), {
        code: "stale_observation",
      });
    }
    if (command.snapshot_id !== state.snapshotId) {
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

  function click(command) {
    const element = resolveRef(command, "click");
    element.click();
    return {
      action_attempted: true,
      dom_event_dispatched: true,
      physical_input_sent: false,
      effect: "unverified",
      fresh_observation_required: true,
    };
  }

  function replaceText(command) {
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

  function handle(command) {
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
    if (command.method === "snapshot") {
      return semanticSnapshot(command.max_nodes);
    }
    if (command.method === "click") {
      return click(command);
    }
    if (command.method === "type") {
      return replaceText(command);
    }
    throw Object.assign(new Error("content command is unsupported"), {
      code: "unsupported_method",
    });
  }

  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (message?.type !== "dcc_cua_command") {
      return false;
    }
    try {
      sendResponse({ ok: true, result: handle(message.command) });
    } catch (error) {
      sendResponse({
        ok: false,
        error: {
          code: typeof error?.code === "string" ? error.code : "content_command_failed",
          message: error instanceof Error ? error.message.slice(0, 256) : "content command failed",
        },
      });
    }
    return false;
  });

  globalThis.__dccCuaContentBridgeV1 = true;
})();
