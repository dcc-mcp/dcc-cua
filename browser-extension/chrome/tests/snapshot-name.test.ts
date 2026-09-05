import assert from "node:assert/strict";
import test from "node:test";

import { snapshotElementName } from "../snapshot-name.ts";

test("snapshot names never fall back to a form control's current value", () => {
  const sources = {
    ariaLabel: "",
    alt: "",
    title: "",
    placeholder: "",
    innerText: "",
    value: "model-must-not-see-this",
  };
  assert.equal(
    snapshotElementName(sources),
    "",
  );
});

test("snapshot names retain bounded public labeling metadata", () => {
  const sources = {
    ariaLabel: "  API   key  ",
    alt: "",
    title: "",
    placeholder: "",
    innerText: "",
    value: "model-must-not-see-this",
  };
  assert.equal(
    snapshotElementName(sources),
    "API key",
  );
});

test("snapshot names fall back to text only when metadata is absent", () => {
  assert.equal(
    snapshotElementName({
      ariaLabel: null,
      alt: null,
      title: null,
      placeholder: null,
      innerText: "  Visible   button  ",
    }),
    "Visible button",
  );
  assert.equal(
    snapshotElementName({
      ariaLabel: "  ",
      title: "Tooltip",
      innerText: "Visible button",
    }),
    "",
  );
});
