import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import Ajv2020 from "ajv/dist/2020.js";

function jsonFile(path: string): unknown {
  return JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
}

test("protocol fixtures and schema remain fail-closed", () => {
  const schema = jsonFile("../protocol-v1.schema.json") as Record<string, unknown>;
  const fixtures = jsonFile("../protocol-v1.fixtures.json") as {
    valid: unknown[];
    invalid: unknown[];
  };
  const validate = new Ajv2020({ allErrors: true, strict: true }).compile(schema);

  for (const fixture of fixtures.valid) {
    assert.equal(validate(fixture), true, JSON.stringify(validate.errors));
  }
  for (const fixture of fixtures.invalid) {
    assert.equal(validate(fixture), false, `unexpectedly accepted ${JSON.stringify(fixture)}`);
  }
});
