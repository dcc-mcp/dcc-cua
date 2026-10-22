import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { runInNewContext } from "node:vm";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { validateContentCommand } from "../content-protocol.ts";
import { isObject } from "../protocol.ts";
import { snapshotElementName } from "../snapshot-name.ts";

function fixture() {
  let listener: (message: unknown, sender: unknown, response: (value: any) => void) => boolean;
  class Element {
    tagName = "BUTTON"; type = "button"; hidden = false; isConnected = true; tabIndex = 0;
    textContent = "Continue"; innerText = "Continue"; isContentEditable = false;
    disabled = false; clicks = 0;
    getAttribute(name: string) { return name === "role" ? "button" : null; }
    getBoundingClientRect() { return { top: 1, left: 1, bottom: 20, right: 30, width: 29, height: 19 }; }
    matches(selector: string) { return selector === ":disabled" ? this.disabled : true; }
    hasAttribute() { return false; }
    click() { this.clicks += 1; }
  }
  const element = new Element();
  let source = readFileSync(new URL("../entrypoints/tab-bridge.ts", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?;\r?\n/gm, "")
    .replace("export default defineUnlistedScript", "defineUnlistedScript");
  runInNewContext(stripTypeScriptTypes(source), {
    browser: { runtime: { onMessage: { addListener(value: typeof listener) { listener = value; } } } },
    defineUnlistedScript: (run: () => void) => run(),
    validateContentCommand, isObject, snapshotElementName,
    document: { title: "fixture", querySelectorAll: () => [element] },
    location: { origin: "https://example.com", href: "https://example.com/account" },
    window: { innerWidth: 800, innerHeight: 600 }, crypto: { randomUUID },
    HTMLElement: Element, HTMLInputElement: Element, HTMLTextAreaElement: class {},
    getComputedStyle: () => ({ display: "block", visibility: "visible", opacity: "1" }),
  });
  function command(data: Record<string, unknown>) {
    let result: any;
    listener({ type: "dcc_cua_command", command: { schema: "dcc-cua.browser-extension.v1", session_nonce: "pair-1", ...data } }, {}, value => { result = value; });
    return result;
  }
  assert.equal(command({ method: "pair" }).ok, true);
  return { element, command };
}

test("real tab bridge consumes one observed click and rejects changed controls", () => {
  for (const change of ["disabled", "name", "replay"] as const) {
    const { element, command } = fixture();
    const snapshot = command({ method: "snapshot", max_nodes: 10 }).result;
    const action = { method: "click", snapshot_id: snapshot.snapshot_id, ref: snapshot.refs[0].ref };
    if (change === "disabled") element.disabled = true;
    if (change === "name") element.textContent = "Delete account";
    if (change === "replay") assert.equal(command(action).ok, true);
    assert.equal(command(action).error.code, "stale_observation");
    assert.equal(element.clicks, change === "replay" ? 1 : 0);
  }
});

test("tab bridge declares password controls sensitive without reading their value", () => {
  const { element, command } = fixture();
  element.type = "password";
  Object.defineProperty(element, "value", { get() { throw new Error("must never read credentials"); } });
  const snapshot = command({ method: "snapshot", max_nodes: 10 });
  assert.equal(snapshot.ok, true);
  assert.equal(snapshot.result.refs[0].states.sensitive, true);
});
