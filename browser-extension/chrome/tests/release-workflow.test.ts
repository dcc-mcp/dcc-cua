import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";

import { parseDocument, stringify } from "yaml";

const WORKFLOW_URL = new URL(
  "../../../.github/workflows/release-please.yml",
  import.meta.url,
);

const ACTION_PINS: Record<string, string> = {
  "actions/checkout": "3d3c42e5aac5ba805825da76410c181273ba90b1",
  "actions/download-artifact": "d3f86a106a0bac45b974a628896c90dbdf5c8093",
  "actions/setup-node": "820762786026740c76f36085b0efc47a31fe5020",
  "actions/upload-artifact": "ea165f8d65b6e75b540449e92b4886f43607fa02",
  "dtolnay/rust-toolchain": "4360b52568e2003a75bf9bc1d59f33a8e3fc893c",
  "google-github-actions/auth": "7c6bc770dae815cd3e89ee6cdf493a5fab2cc093",
  "googleapis/release-please-action":
    "45996ed1f6d02564a971a2fa1b5860e934307cf7",
};

type Step = {
  env?: Record<string, unknown>;
  id?: string;
  name?: string;
  run?: string;
  uses?: string;
  with?: Record<string, unknown>;
};

type Job = {
  needs?: string | string[];
  outputs?: Record<string, unknown>;
  permissions?: Record<string, string>;
  steps?: Step[];
  "timeout-minutes"?: number;
};

type Workflow = {
  jobs?: Record<string, Job>;
  on?: Record<string, unknown>;
  permissions?: Record<string, string>;
};

const STEP_ALLOWLIST: Record<string, string[]> = {
  validate: [
    "uses:actions/checkout",
    "name:Validate release configuration",
    "uses:dtolnay/rust-toolchain",
    "run:cargo metadata --locked --no-deps --format-version 1",
    "run:python -B -m unittest scripts.test_release_workflow scripts.test_release_integrity scripts.test_verify_release_assets",
  ],
  "release-please": [
    "uses:actions/checkout",
    "name:Refuse pre-existing release identities",
    "uses:googleapis/release-please-action",
    "name:Keep the native runtime release Latest",
    "name:Refresh independent release PRs from current main",
  ],
  build: [
    "uses:actions/checkout",
    "name:Verify native release source binding",
    "uses:./.github/actions/select-macos-toolchain",
    "uses:dtolnay/rust-toolchain",
    "run:sudo apt-get update && sudo apt-get install --no-install-recommends -y libx11-dev libxi-dev libxtst-dev",
    'run:swiftc_path="$(xcrun --find swiftc)"',
    "run:cargo build --release --locked -p dcc-cua-cli",
    "name:Package dcc-cua",
    "uses:actions/upload-artifact",
  ],
  "consolidate-native": [
    "uses:actions/checkout",
    "name:Verify native build artifact set",
    "name:Download and verify native build artifacts",
    "name:Write immutable native release provenance",
    "uses:actions/upload-artifact",
  ],
  "attach-assets": [
    "uses:actions/checkout",
    "name:Verify immutable native artifact identity",
    "name:Audit immutable native artifact download",
    "name:Download and extract verified native artifact",
    "name:Verify native release asset completeness and provenance",
    "name:Refuse existing native release assets",
    "name:Attach release archives",
    "name:Verify published native release immutability",
  ],
  "package-browser-extension": [
    "uses:actions/checkout",
    "name:Verify browser extension release source binding",
    "uses:actions/setup-node",
    "run:npm --prefix browser-extension/chrome ci",
    "run:npm --prefix browser-extension/chrome run check",
    "run:npm --prefix browser-extension/chrome test",
    "run:npm --prefix browser-extension/chrome run build",
    "run:python -B browser-extension/chrome/scripts/test_extension.py",
    "name:Package browser extensions",
    "name:Verify browser extension asset set",
    "uses:actions/upload-artifact",
  ],
  "attach-browser-extension-assets": [
    "uses:actions/checkout",
    "name:Verify immutable browser extension artifact identity",
    "name:Audit immutable browser extension artifact download",
    "name:Download and extract verified browser extension artifact",
    "name:Verify browser extension asset set",
    "name:Refuse existing browser extension release assets",
    "name:Attach browser extension review artifacts",
    "name:Verify published browser extension release immutability",
  ],
  "publish-chrome-web-store": [
    "uses:actions/checkout",
    "name:Verify immutable browser extension artifact identity",
    "name:Audit immutable browser extension artifact download",
    "name:Download and extract verified browser extension artifact",
    "name:Verify browser extension asset set",
    "uses:google-github-actions/auth",
    "name:Submit Chrome Web Store release",
  ],
  "publish-edge-addons": [
    "uses:actions/checkout",
    "name:Verify immutable browser extension artifact identity",
    "name:Audit immutable browser extension artifact download",
    "name:Download and extract verified browser extension artifact",
    "name:Verify browser extension asset set",
    "name:Submit Edge Add-ons release",
  ],
  "publish-firefox-addons": [
    "uses:actions/checkout",
    "uses:actions/setup-node",
    "run:npm --prefix browser-extension/chrome ci",
    "name:Verify immutable browser extension artifact identity",
    "name:Audit immutable browser extension artifact download",
    "name:Download and extract verified browser extension artifact",
    "name:Verify browser extension asset set",
    "name:Submit Firefox Add-ons release",
  ],
};

const SOURCE_BINDING_STATEMENTS = [
  "set -euo pipefail",
  'head_sha="$(git rev-parse HEAD)"',
  'tag_sha="$(git rev-parse "${TAG_NAME}^{commit}")"',
  'release_target="$(gh release view "$TAG_NAME" --repo "$GITHUB_REPOSITORY" --json targetCommitish --jq .targetCommitish)"',
  "python -B scripts/release_integrity.py verify-source \\",
  '--head-sha "$head_sha" \\',
  '--tag-sha "$tag_sha" \\',
  '--release-target "$release_target" \\',
  '--expected-sha "$EXPECTED_SHA"',
];

const NATIVE_ARTIFACT_IDENTITY_STATEMENTS = [
  "set -euo pipefail",
  'gh api "repos/$GITHUB_REPOSITORY/actions/artifacts/$ARTIFACT_ID" > "$RUNNER_TEMP/artifact.json"',
  "python -B scripts/release_integrity.py verify-artifact \\",
  '--metadata "$RUNNER_TEMP/artifact.json" \\',
  '--expected-id "$ARTIFACT_ID" \\',
  '--expected-digest "$EXPECTED_ARTIFACT_DIGEST" \\',
  "--expected-name dcc-cua-native-release \\",
  '--expected-run-id "$GITHUB_RUN_ID" \\',
  '--expected-head-sha "$EXPECTED_HEAD_SHA"',
];

const EXTENSION_ARTIFACT_IDENTITY_STATEMENTS = [
  "set -euo pipefail",
  'gh api "repos/$GITHUB_REPOSITORY/actions/artifacts/$ARTIFACT_ID" > "$RUNNER_TEMP/artifact.json"',
  "python -B scripts/release_integrity.py verify-artifact \\",
  '--metadata "$RUNNER_TEMP/artifact.json" \\',
  '--expected-id "$ARTIFACT_ID" \\',
  '--expected-digest "$EXPECTED_ARTIFACT_DIGEST" \\',
  "--expected-name dcc-cua-browser-extension \\",
  '--expected-run-id "$GITHUB_RUN_ID" \\',
  '--expected-head-sha "$EXPECTED_HEAD_SHA"',
];

const NATIVE_ARTIFACT_EXTRACTION_STATEMENTS = [
  "set -euo pipefail",
  'archive="$RUNNER_TEMP/native-release-artifact.zip"',
  "gh api \\",
  '"repos/$GITHUB_REPOSITORY/actions/artifacts/$ARTIFACT_ID/zip" \\',
  '> "$archive"',
  "python -B scripts/release_integrity.py verify-extract \\",
  '--archive "$archive" \\',
  '--expected-digest "$EXPECTED_ARTIFACT_DIGEST" \\',
  "--output dist",
];

const EXTENSION_ARTIFACT_EXTRACTION_STATEMENTS = [
  "set -euo pipefail",
  'archive="$RUNNER_TEMP/browser-extension-artifact.zip"',
  "gh api \\",
  '"repos/$GITHUB_REPOSITORY/actions/artifacts/$ARTIFACT_ID/zip" \\',
  '> "$archive"',
  "python -B scripts/release_integrity.py verify-extract \\",
  '--archive "$archive" \\',
  '--expected-digest "$EXPECTED_ARTIFACT_DIGEST" \\',
  "--output dist",
];

const EXTENSION_ASSET_SET_STATEMENTS = [
  "set -euo pipefail",
  "python -B scripts/release_integrity.py verify-extension-assets \\",
  "--directory dist \\",
  '--version "$EXTENSION_VERSION"',
];

const NATIVE_PUBLISHED_READBACK_STATEMENTS = [
  "set -euo pipefail",
  'gh release view "$TAG_NAME" \\',
  '--repo "$GITHUB_REPOSITORY" \\',
  "--json tagName,targetCommitish,assets \\",
  '> "$RUNNER_TEMP/published-native-release.json"',
  'latest_tag="$(' ,
  'gh release view --repo "$GITHUB_REPOSITORY" \\',
  "--json tagName --jq .tagName",
  ')"',
  "python -B scripts/release_integrity.py verify-published-native \\",
  '--metadata "$RUNNER_TEMP/published-native-release.json" \\',
  "--directory dist \\",
  '--version "${TAG_NAME#v}" \\',
  '--tag "$TAG_NAME" \\',
  '--source-sha "$SOURCE_SHA" \\',
  '--actual-latest-tag "$latest_tag"',
];

const EXTENSION_PUBLISHED_READBACK_STATEMENTS = [
  "set -euo pipefail",
  'gh release view "$TAG_NAME" \\',
  '--repo "$GITHUB_REPOSITORY" \\',
  "--json tagName,targetCommitish,assets \\",
  '> "$RUNNER_TEMP/published-extension-release.json"',
  'latest_tag="$(' ,
  'gh release view --repo "$GITHUB_REPOSITORY" \\',
  "--json tagName --jq .tagName",
  ')"',
  'native_tag="v$(tr -d \'\\r\\n\' < version.txt)"',
  "python -B scripts/release_integrity.py verify-published-extension \\",
  '--metadata "$RUNNER_TEMP/published-extension-release.json" \\',
  "--directory dist \\",
  '--version "$EXTENSION_VERSION" \\',
  '--tag "$TAG_NAME" \\',
  '--source-sha "$SOURCE_SHA" \\',
  '--expected-latest-tag "$native_tag" \\',
  '--actual-latest-tag "$latest_tag"',
];

const RUN_STATEMENT_DIGESTS: Record<string, string> = {
  "validate|Validate release configuration": "8d3f02794bf7a97330d98fa4d70ce5cc7f62e474df31c860bc71c2a29d178cbd",
  "validate|#3": "007b41ff968a7d8b2e96752351831db1ab9ff3248cf5215b5fa2e6b19ed1234e",
  "validate|#4": "fb14b56785075e6838de31dab4612824ae674ba0eac379a121de88784392065d",
  "release-please|Refuse pre-existing release identities": "0ce04695628ad6a72542b32d24e1b715f99c873707d5440660849620cba5b0fa",
  "release-please|Keep the native runtime release Latest": "3a51ac0c7bad4c6a9a83661752cb94a506d49a56350494c3a5a2c76faefecebb",
  "release-please|Refresh independent release PRs from current main": "2be98e30902d17319ab0347ce92c886e80573c2a50ce9d52f1aec56e213f8652",
  "build|Verify native release source binding": "c395d2eb87e998ccc88df9d8b441766c8ff3fdd3b2eed789b1eadcc0d20cf099",
  "build|#4": "0c9fd2bfc5153caf2f158fa8161b842aa52a3183cbb65d62b11be4f52b137204",
  "build|#5": "4e76b39a502964fdbcae32ff23ab3ad042dda3cd73ac0a7852d44dda0605b231",
  "build|#6": "b90711ea4af19f26b51ca5d7ca11dcbf2c9597d75e26d06658253999a73567e8",
  "build|Package dcc-cua": "6d805e9f9fb7e477a0402cb071542d6fafaea24fbde00cdc01d7dfa14fa7c4ce",
  "consolidate-native|Verify native build artifact set": "a22a2661b450fdd465e1cc5c05d91c7ea15069dab4d8d72f24d442280a3dd672",
  "consolidate-native|Download and verify native build artifacts": "a514a42fa4ee425b4e6f715caa2cfc449db4f7421d1ef50b291f04184454fc9a",
  "consolidate-native|Write immutable native release provenance": "fc8e085354a828c16431d7024a44127cb1c8890820e9b2ab61b98a40d249f1fc",
  "attach-assets|Verify immutable native artifact identity": "bc366f560c9a500468596b8d200ee5d2fd4fac7e957cb5aa2b29201a61099c6b",
  "attach-assets|Download and extract verified native artifact": "85f5327c24ec687979179e3c09b0b37070b07146ceea1c4fbc9f8fcbb5a118f4",
  "attach-assets|Verify native release asset completeness and provenance": "3c37f870acd06367d11001622d187388b775de5f5cc50e0a3b6794557b5d9bb3",
  "attach-assets|Refuse existing native release assets": "cdf8ddf1f49d77393c4e4721b06381c577e2bda26d0f0b44bc1ba6c289cf5f7e",
  "attach-assets|Attach release archives": "a12b9c4e3713e8000995688a2697f7cf9f7af2f4cf448f8f609f7455d30de5c2",
  "attach-assets|Verify published native release immutability": "8322831cb7b3c919995622489cca0f5f96bc442bae5903d6b3bf13d18f19b053",
  "package-browser-extension|Verify browser extension release source binding": "c395d2eb87e998ccc88df9d8b441766c8ff3fdd3b2eed789b1eadcc0d20cf099",
  "package-browser-extension|#3": "ea914f3855acfa08dce0988d9e918c5aee81b877d891d789a7ea6f668aedfd1a",
  "package-browser-extension|#4": "20e54c943b7b465505a2056e5b7e54e2df18e24d6c51c54692c56b99dc5df285",
  "package-browser-extension|#5": "ad3642d0e2d48d1a364750fae19b60f784cd61cba09d4307334190a6289097d6",
  "package-browser-extension|#6": "e15b990e0610e87d68ac5416283eeb310f1be29247f6ea1f707038c3454272a0",
  "package-browser-extension|#7": "fe87b566d1909517db3c2bac37c836fee08779f2e1b2b3d8fc73a9022c5e9347",
  "package-browser-extension|Package browser extensions": "4f361fe03fad1e3554746753016c0808aa35289a6e6fd056defeb9f73d309d43",
  "package-browser-extension|Verify browser extension asset set": "f75d416f0b832a307b0820123f4eb8a9318561734b4249f56e8d36be9d56246b",
  "attach-browser-extension-assets|Verify immutable browser extension artifact identity": "dcfc8adc9cfe1836b7290a3d2100001f84fa68af97c51b5969e176985eca33ca",
  "attach-browser-extension-assets|Download and extract verified browser extension artifact": "1dda8d720c3ad3fc6fdc88d28c6ebf0ae9e8cc8fa6be77d3b361b38a3c236cb8",
  "attach-browser-extension-assets|Verify browser extension asset set": "f75d416f0b832a307b0820123f4eb8a9318561734b4249f56e8d36be9d56246b",
  "attach-browser-extension-assets|Refuse existing browser extension release assets": "d280fe513ea5a8ac23f38cfa3a5c37a753ed3f7c1332bff40fa911c7d8a3278f",
  "attach-browser-extension-assets|Attach browser extension review artifacts": "a12b9c4e3713e8000995688a2697f7cf9f7af2f4cf448f8f609f7455d30de5c2",
  "attach-browser-extension-assets|Verify published browser extension release immutability": "1d32d5ed6ba7784aa9abde41b26875b6c827e076395283662c18da77a85c09a6",
  "publish-chrome-web-store|Verify immutable browser extension artifact identity": "dcfc8adc9cfe1836b7290a3d2100001f84fa68af97c51b5969e176985eca33ca",
  "publish-chrome-web-store|Download and extract verified browser extension artifact": "1dda8d720c3ad3fc6fdc88d28c6ebf0ae9e8cc8fa6be77d3b361b38a3c236cb8",
  "publish-chrome-web-store|Verify browser extension asset set": "f75d416f0b832a307b0820123f4eb8a9318561734b4249f56e8d36be9d56246b",
  "publish-chrome-web-store|Submit Chrome Web Store release": "5d7475ac62569c72517ea447a20b5601bf820af246f701619b612396d027758e",
  "publish-edge-addons|Verify immutable browser extension artifact identity": "dcfc8adc9cfe1836b7290a3d2100001f84fa68af97c51b5969e176985eca33ca",
  "publish-edge-addons|Download and extract verified browser extension artifact": "1dda8d720c3ad3fc6fdc88d28c6ebf0ae9e8cc8fa6be77d3b361b38a3c236cb8",
  "publish-edge-addons|Verify browser extension asset set": "f75d416f0b832a307b0820123f4eb8a9318561734b4249f56e8d36be9d56246b",
  "publish-edge-addons|Submit Edge Add-ons release": "29c4e7e2ac484e37f6e3cfe06ade616ca4db2fae9238b9ccd15c3a85f3fe1a88",
  "publish-firefox-addons|#2": "ea914f3855acfa08dce0988d9e918c5aee81b877d891d789a7ea6f668aedfd1a",
  "publish-firefox-addons|Verify immutable browser extension artifact identity": "dcfc8adc9cfe1836b7290a3d2100001f84fa68af97c51b5969e176985eca33ca",
  "publish-firefox-addons|Download and extract verified browser extension artifact": "1dda8d720c3ad3fc6fdc88d28c6ebf0ae9e8cc8fa6be77d3b361b38a3c236cb8",
  "publish-firefox-addons|Verify browser extension asset set": "f75d416f0b832a307b0820123f4eb8a9318561734b4249f56e8d36be9d56246b",
  "publish-firefox-addons|Submit Firefox Add-ons release": "7883a6e1f5db317d8f718fefe7dbc4b91cddb66ded2e184527ac2fc727f59a38",
};

function executableLines(run: string | undefined): string[] {
  return (run ?? "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));
}

function stepIdentity(step: Step): string {
  if (step.name !== undefined) return `name:${step.name}`;
  if (step.uses !== undefined) return `uses:${step.uses.split("@")[0]}`;
  const first = executableLines(step.run)[0];
  assert.ok(first, "anonymous run step must contain an executable statement");
  return `run:${first}`;
}

function assertExecutableInvocation(step: Step, command: string): void {
  assert.deepEqual(
    executableLines(step.run).filter((line) => line.includes(command)),
    [command],
    `${step.name ?? "run step"} must execute the exact reviewed command`,
  );
}

function assertExecutableStatements(step: Step, expected: string[]): void {
  assert.deepEqual(
    executableLines(step.run),
    expected,
    `${step.name ?? "run step"} executable statements changed`,
  );
}

function parseWorkflow(source: string): Workflow {
  const document = parseDocument(source, { uniqueKeys: true });
  assert.deepEqual(
    document.errors.map((error) => error.message),
    [],
    "release workflow must be valid YAML with unique mapping keys",
  );
  return document.toJS({ maxAliasCount: 0 }) as Workflow;
}

function mutateWorkflow(
  source: string,
  mutation: (workflow: Workflow) => void,
): string {
  const workflow = parseWorkflow(source);
  mutation(workflow);
  return stringify(workflow, { lineWidth: 0 });
}

function replaceRequired(
  source: string,
  search: string | RegExp,
  replacement: string,
  description: string,
): string {
  const mutated = source.replace(search, replacement);
  assert.notEqual(mutated, source, `${description} must change its fixture`);
  return mutated;
}

function steps(job: Job): Step[] {
  assert.ok(Array.isArray(job.steps), "job must define a steps list");
  return job.steps;
}

function namedStep(job: Job, name: string): Step {
  const matches = steps(job).filter((step) => step.name === name);
  assert.equal(matches.length, 1, `expected exactly one ${name} step`);
  return matches[0]!;
}

function actionStep(job: Job, ownerAndName: string): Step {
  const matches = steps(job).filter((step) =>
    step.uses?.startsWith(`${ownerAndName}@`),
  );
  assert.equal(
    matches.length,
    1,
    `expected exactly one ${ownerAndName} action step`,
  );
  return matches[0]!;
}

function requiredJob(jobs: Record<string, Job>, name: string): Job {
  const job = jobs[name];
  assert.ok(job, `missing ${name} job`);
  return job;
}

function assertNeeds(job: Job, expected: string[]): void {
  const actual = Array.isArray(job.needs)
    ? job.needs
    : job.needs === undefined
      ? []
      : [job.needs];
  assert.deepEqual(actual, expected);
}

function validateReleaseWorkflow(source: string): void {
  const workflow = parseWorkflow(source);
  assert.deepEqual(workflow.permissions, {});
  assert.deepEqual(Object.keys(workflow.on ?? {}), ["push"]);

  const jobs = workflow.jobs ?? {};
  assert.deepEqual(Object.keys(jobs).sort(), [
    "attach-assets",
    "attach-browser-extension-assets",
    "build",
    "consolidate-native",
    "package-browser-extension",
    "publish-chrome-web-store",
    "publish-edge-addons",
    "publish-firefox-addons",
    "release-please",
    "validate",
  ]);

  const expectedPermissions: Record<string, Record<string, string>> = {
    validate: { contents: "read" },
    "release-please": { contents: "write", "pull-requests": "write" },
    build: { contents: "read" },
    "consolidate-native": { actions: "read", contents: "read" },
    "attach-assets": { actions: "read", contents: "write" },
    "package-browser-extension": { contents: "read" },
    "attach-browser-extension-assets": {
      actions: "read",
      contents: "write",
    },
    "publish-chrome-web-store": {
      actions: "read",
      contents: "read",
      "id-token": "write",
    },
    "publish-edge-addons": { actions: "read", contents: "read" },
    "publish-firefox-addons": { actions: "read", contents: "read" },
  };
  for (const [jobName, job] of Object.entries(jobs)) {
    const permissions = expectedPermissions[jobName];
    assert.ok(permissions, `unexpected ${jobName} job`);
    assert.deepEqual(job.permissions, permissions);
    assert.ok(
      Number.isInteger(job["timeout-minutes"]) &&
        (job["timeout-minutes"] ?? 0) > 0 &&
        (job["timeout-minutes"] ?? 0) <= 45,
      `${jobName} must have a bounded timeout`,
    );
    assert.deepEqual(
      steps(job).map(stepIdentity),
      STEP_ALLOWLIST[jobName],
      `${jobName} steps must match the reviewed allowlist and order`,
    );
    for (const step of steps(job)) {
      if (step.uses === undefined || step.uses.startsWith("./")) continue;
      const reference = /^([^@]+)@([0-9a-f]{40})$/.exec(step.uses);
      assert.ok(reference, `${step.uses} has an invalid action ref`);
      const action = reference[1]!;
      const revision = reference[2]!;
      assert.equal(
        revision,
        ACTION_PINS[action],
        `${action} must use the reviewed immutable pin`,
      );
      assert.match(revision, /^[0-9a-f]{40}$/);
    }
  }

  const observedRunDigests: Record<string, string> = {};
  for (const [jobName, job] of Object.entries(jobs)) {
    steps(job).forEach((step, index) => {
      if (step.run === undefined) return;
      const key = `${jobName}|${step.name ?? `#${index}`}`;
      observedRunDigests[key] = createHash("sha256")
        .update(executableLines(step.run).join("\n"))
        .digest("hex");
    });
  }
  assert.deepEqual(
    observedRunDigests,
    RUN_STATEMENT_DIGESTS,
    "every executable release statement must match the reviewed allowlist",
  );

  const allowedReleaseMutations = new Set([
    'release-please|Keep the native runtime release Latest|gh release edit $nativeTag --repo "$env:GITHUB_REPOSITORY" --latest',
    'release-please|Keep the native runtime release Latest|gh release edit "$env:EXTENSION_TAG" --repo "$env:GITHUB_REPOSITORY" --latest=false',
    'attach-assets|Attach release archives|gh release upload "$TAG_NAME" dist/* --repo "$GITHUB_REPOSITORY"',
    'attach-browser-extension-assets|Attach browser extension review artifacts|gh release upload "$TAG_NAME" dist/* --repo "$GITHUB_REPOSITORY"',
  ]);
  for (const [jobName, job] of Object.entries(jobs)) {
    for (const step of steps(job)) {
      for (const line of executableLines(step.run)) {
        if (/\bgh\s+release\s+(create|delete|edit|upload)\b/i.test(line)) {
          assert.ok(
            allowedReleaseMutations.has(`${jobName}|${step.name ?? ""}|${line}`),
            `unreviewed release mutation in ${jobName}: ${line}`,
          );
        }
      }
    }
  }

  assert.ok(!source.includes("--clobber"));
  assert.ok(!source.includes("release_tag"));

  const release = requiredJob(jobs, "release-please");
  const releaseSteps = steps(release);
  const releaseCheckout = actionStep(release, "actions/checkout");
  const existingIdentity = namedStep(
    release,
    "Refuse pre-existing release identities",
  );
  const releaseAction = actionStep(
    release,
    "googleapis/release-please-action",
  );
  assert.ok(releaseSteps.indexOf(releaseCheckout) < releaseSteps.indexOf(existingIdentity));
  assert.ok(releaseSteps.indexOf(existingIdentity) < releaseSteps.indexOf(releaseAction));
  assert.equal(releaseCheckout.with?.ref, "${{ github.sha }}");
  assert.equal(releaseCheckout.with?.["fetch-depth"], 2);
  assert.equal(releaseCheckout.with?.["fetch-tags"], true);
  assert.match(existingIdentity.run ?? "", /release_integrity\.py changed-tags/);
  assert.match(existingIdentity.run ?? "", /git ls-remote --exit-code --tags/);
  assert.match(existingIdentity.run ?? "", /gh api --paginate/);
  assertExecutableInvocation(
    existingIdentity,
    "python -B scripts/release_integrity.py changed-tags \\",
  );
  assert.equal(
    release.outputs?.release_created,
    "${{ steps.release.outputs.release_created == 'true' }}",
  );
  assert.equal(release.outputs?.tag_name, "${{ steps.release.outputs.tag_name }}");
  assert.equal(release.outputs?.source_sha, "${{ steps.release.outputs.sha }}");
  assert.equal(
    release.outputs?.extension_source_sha,
    "${{ steps.release.outputs['browser-extension/chrome--sha'] }}",
  );

  const build = requiredJob(jobs, "build");
  assertNeeds(build, ["release-please"]);
  assert.equal(
    actionStep(build, "actions/checkout").with?.ref,
    "${{ needs.release-please.outputs.source_sha }}",
  );
  const nativeSource = namedStep(build, "Verify native release source binding");
  assert.match(nativeSource.run ?? "", /git rev-parse HEAD/);
  assert.match(nativeSource.run ?? "", /\$\{TAG_NAME\}\^\{commit\}/);
  assert.match(nativeSource.run ?? "", /targetCommitish/);
  assertExecutableInvocation(
    nativeSource,
    "python -B scripts/release_integrity.py verify-source \\",
  );
  assertExecutableStatements(nativeSource, SOURCE_BINDING_STATEMENTS);
  const nativeUpload = actionStep(build, "actions/upload-artifact");
  assert.equal(nativeUpload.id, "upload-native");
  assert.equal(nativeUpload.with?.name, "dcc-cua-native-${{ matrix.platform }}");

  const consolidate = requiredJob(jobs, "consolidate-native");
  assertNeeds(consolidate, ["release-please", "build"]);
  assertExecutableInvocation(
    namedStep(consolidate, "Verify native build artifact set"),
    "python -B scripts/release_integrity.py write-native-plan \\",
  );
  assertExecutableInvocation(
    namedStep(consolidate, "Download and verify native build artifacts"),
    "python -B scripts/release_integrity.py verify-extract \\",
  );
  namedStep(consolidate, "Write immutable native release provenance");
  const consolidatedUpload = actionStep(consolidate, "actions/upload-artifact");
  assert.equal(consolidatedUpload.id, "upload-release");
  assert.equal(consolidatedUpload.with?.name, "dcc-cua-native-release");
  assert.equal(
    consolidate.outputs?.artifact_id,
    "${{ steps.upload-release.outputs.artifact-id }}",
  );
  assert.equal(
    consolidate.outputs?.artifact_digest,
    "${{ steps.upload-release.outputs.artifact-digest }}",
  );

  const attach = requiredJob(jobs, "attach-assets");
  assertNeeds(attach, ["release-please", "consolidate-native"]);
  const nativeDownload = actionStep(attach, "actions/download-artifact");
  assert.equal(
    nativeDownload.with?.["artifact-ids"],
    "${{ needs.consolidate-native.outputs.artifact_id }}",
  );
  assert.equal(
    nativeDownload.with?.path,
    "${{ runner.temp }}/native-action-download-audit",
  );
  const nativeArtifact = namedStep(
    attach,
    "Verify immutable native artifact identity",
  );
  assert.equal(
    nativeArtifact.env?.EXPECTED_ARTIFACT_DIGEST,
    "${{ needs.consolidate-native.outputs.artifact_digest }}",
  );
  assertExecutableInvocation(
    nativeArtifact,
    "python -B scripts/release_integrity.py verify-artifact \\",
  );
  assertExecutableStatements(
    nativeArtifact,
    NATIVE_ARTIFACT_IDENTITY_STATEMENTS,
  );
  assertExecutableInvocation(
    namedStep(attach, "Download and extract verified native artifact"),
    "python -B scripts/release_integrity.py verify-extract \\",
  );
  assertExecutableStatements(
    namedStep(attach, "Download and extract verified native artifact"),
    NATIVE_ARTIFACT_EXTRACTION_STATEMENTS,
  );
  namedStep(attach, "Refuse existing native release assets");
  namedStep(attach, "Verify native release asset completeness and provenance");
  assert.equal(
    namedStep(attach, "Attach release archives").run,
    'gh release upload "$TAG_NAME" dist/* --repo "$GITHUB_REPOSITORY"',
  );
  assertExecutableInvocation(
    namedStep(attach, "Verify published native release immutability"),
    "python -B scripts/release_integrity.py verify-published-native \\",
  );
  assertExecutableStatements(
    namedStep(attach, "Verify published native release immutability"),
    NATIVE_PUBLISHED_READBACK_STATEMENTS,
  );

  const extension = requiredJob(jobs, "package-browser-extension");
  assertNeeds(extension, ["release-please"]);
  assert.equal(
    actionStep(extension, "actions/checkout").with?.ref,
    "${{ needs.release-please.outputs.extension_source_sha }}",
  );
  namedStep(extension, "Verify browser extension release source binding");
  assertExecutableInvocation(
    namedStep(extension, "Verify browser extension release source binding"),
    "python -B scripts/release_integrity.py verify-source \\",
  );
  assertExecutableStatements(
    namedStep(extension, "Verify browser extension release source binding"),
    SOURCE_BINDING_STATEMENTS,
  );
  assertExecutableStatements(
    namedStep(extension, "Verify browser extension asset set"),
    EXTENSION_ASSET_SET_STATEMENTS,
  );
  const extensionUpload = actionStep(extension, "actions/upload-artifact");
  assert.equal(extensionUpload.id, "upload-extension");
  assert.equal(
    extension.outputs?.artifact_id,
    "${{ steps.upload-extension.outputs.artifact-id }}",
  );
  assert.equal(
    extension.outputs?.artifact_digest,
    "${{ steps.upload-extension.outputs.artifact-digest }}",
  );

  const extensionConsumers = [
    "attach-browser-extension-assets",
    "publish-chrome-web-store",
    "publish-edge-addons",
    "publish-firefox-addons",
  ];
  assertNeeds(requiredJob(jobs, "attach-browser-extension-assets"), [
    "release-please",
    "package-browser-extension",
  ]);
  for (const jobName of extensionConsumers.slice(1)) {
    assertNeeds(requiredJob(jobs, jobName), [
      "release-please",
      "package-browser-extension",
      "attach-browser-extension-assets",
    ]);
  }
  for (const jobName of extensionConsumers) {
    const job = requiredJob(jobs, jobName);
    const download = actionStep(job, "actions/download-artifact");
    assert.equal(
      download.with?.["artifact-ids"],
      "${{ needs.package-browser-extension.outputs.artifact_id }}",
      `${jobName} must consume the exact build artifact ID`,
    );
    assert.equal(
      download.with?.path,
      "${{ runner.temp }}/extension-action-download-audit",
      `${jobName} must quarantine the action-managed extraction`,
    );
    assert.equal(
      download.with?.["merge-multiple"],
      true,
      `${jobName} must merge the exact artifact at the requested path`,
    );
    assert.equal(
      namedStep(job, "Verify immutable browser extension artifact identity").env
        ?.EXPECTED_ARTIFACT_DIGEST,
      "${{ needs.package-browser-extension.outputs.artifact_digest }}",
      `${jobName} must consume the exact build artifact digest`,
    );
    assertExecutableInvocation(
      namedStep(job, "Verify immutable browser extension artifact identity"),
      "python -B scripts/release_integrity.py verify-artifact \\",
    );
    assertExecutableStatements(
      namedStep(job, "Verify immutable browser extension artifact identity"),
      EXTENSION_ARTIFACT_IDENTITY_STATEMENTS,
    );
    assertExecutableInvocation(
      namedStep(
        job,
        "Download and extract verified browser extension artifact",
      ),
      "python -B scripts/release_integrity.py verify-extract \\",
    );
    assertExecutableStatements(
      namedStep(
        job,
        "Download and extract verified browser extension artifact",
      ),
      EXTENSION_ARTIFACT_EXTRACTION_STATEMENTS,
    );
    assertExecutableStatements(
      namedStep(job, "Verify browser extension asset set"),
      EXTENSION_ASSET_SET_STATEMENTS,
    );
  }
  const extensionAttach = requiredJob(jobs, "attach-browser-extension-assets");
  namedStep(
    extensionAttach,
    "Refuse existing browser extension release assets",
  );
  assert.equal(
    namedStep(extensionAttach, "Attach browser extension review artifacts").run,
    'gh release upload "$TAG_NAME" dist/* --repo "$GITHUB_REPOSITORY"',
  );
  assertExecutableInvocation(
    namedStep(
      extensionAttach,
      "Verify published browser extension release immutability",
    ),
    "python -B scripts/release_integrity.py verify-published-extension \\",
  );
  assertExecutableStatements(
    namedStep(
      extensionAttach,
      "Verify published browser extension release immutability",
    ),
    EXTENSION_PUBLISHED_READBACK_STATEMENTS,
  );

  assert.equal(nativeDownload.with?.["merge-multiple"], true);
}

test("release workflow is immutable, least-privilege, and source-bound", () => {
  validateReleaseWorkflow(readFileSync(WORKFLOW_URL, "utf8"));
});

function assertAdversarialMutations(source: string): void {
  validateReleaseWorkflow(source);
  const lineEnding = source.includes("\r\n") ? "\r\n" : "\n";

  const duplicate = replaceRequired(
    source,
    `  attach-assets:${lineEnding}`,
    `  attach-assets:${lineEnding}  attach-assets:${lineEnding}`,
    "duplicate job",
  );
  assert.throws(() => validateReleaseWorkflow(duplicate));

  const moved = replaceRequired(
    source,
    `  consolidate-native:${lineEnding}`,
    `  consolidate-native-decoy:${lineEnding}`,
    "moved job",
  );
  assert.throws(() => validateReleaseWorkflow(moved));

  const exactId =
    "artifact-ids: ${{ needs.consolidate-native.outputs.artifact_id }}";
  const decoy = `${replaceRequired(
    source,
    exactId,
    "name: dcc-cua-native-release",
    "artifact ID decoy",
  )}${lineEnding}# ${exactId}${lineEnding}`;
  assert.throws(() => validateReleaseWorkflow(decoy));

  const mergeLine = `          merge-multiple: true${lineEnding}`;
  assert.throws(() =>
    validateReleaseWorkflow(
      replaceRequired(source, mergeLine, "", "missing merge-multiple"),
    ),
  );
  assert.throws(() =>
    validateReleaseWorkflow(
      replaceRequired(
        source,
        mergeLine,
        `          merge-multiple: false${lineEnding}`,
        "false merge-multiple",
      ),
    ),
  );
  assert.throws(() =>
    validateReleaseWorkflow(
      replaceRequired(
        source,
        mergeLine,
        `          merge-multiple: "true"${lineEnding}`,
        "string merge-multiple",
      ),
    ),
  );
  const withoutMerge = replaceRequired(
    source,
    mergeLine,
    "",
    "wrong-job merge-multiple removal",
  );
  const checkoutFetchTags = `          fetch-tags: true${lineEnding}`;
  const wrongJob = replaceRequired(
    withoutMerge,
    checkoutFetchTags,
    `${checkoutFetchTags}${mergeLine}`,
    "wrong-job merge-multiple insertion",
  );
  assert.throws(() => validateReleaseWorkflow(wrongJob));

  const duplicateStep = mutateWorkflow(source, (workflow) => {
    const attachSteps = steps(requiredJob(workflow.jobs ?? {}, "attach-assets"));
    const original = attachSteps[2]!;
    attachSteps.splice(2, 0, { ...original, env: { ...original.env } });
  });
  assert.throws(() => validateReleaseWorkflow(duplicateStep));

  const reorderedSteps = mutateWorkflow(source, (workflow) => {
    const attachSteps = steps(requiredJob(workflow.jobs ?? {}, "attach-assets"));
    [attachSteps[1], attachSteps[2]] = [attachSteps[2]!, attachSteps[1]!];
  });
  assert.throws(() => validateReleaseWorkflow(reorderedSteps));

  const movedToWrongJob = mutateWorkflow(source, (workflow) => {
    const jobs = workflow.jobs ?? {};
    const attachSteps = steps(requiredJob(jobs, "attach-assets"));
    const index = attachSteps.findIndex(
      (step) => step.name === "Audit immutable native artifact download",
    );
    assert.notEqual(index, -1);
    const [movedStep] = attachSteps.splice(index, 1);
    steps(requiredJob(jobs, "release-please")).push(movedStep!);
  });
  assert.throws(() => validateReleaseWorkflow(movedToWrongJob));

  const missingExtensionVerifier = mutateWorkflow(source, (workflow) => {
    const attachSteps = steps(
      requiredJob(workflow.jobs ?? {}, "attach-browser-extension-assets"),
    );
    const index = attachSteps.findIndex(
      (step) => step.name === "Verify browser extension asset set",
    );
    assert.notEqual(index, -1);
    attachSteps.splice(index, 1);
  });
  assert.throws(() => validateReleaseWorkflow(missingExtensionVerifier));

  const movedExtensionVerifier = mutateWorkflow(source, (workflow) => {
    const jobs = workflow.jobs ?? {};
    const attachSteps = steps(
      requiredJob(jobs, "attach-browser-extension-assets"),
    );
    const index = attachSteps.findIndex(
      (step) => step.name === "Verify browser extension asset set",
    );
    assert.notEqual(index, -1);
    const [verifier] = attachSteps.splice(index, 1);
    steps(requiredJob(jobs, "release-please")).push(verifier!);
  });
  assert.throws(() => validateReleaseWorkflow(movedExtensionVerifier));

  const reorderedExtensionVerifier = mutateWorkflow(source, (workflow) => {
    const attachSteps = steps(
      requiredJob(workflow.jobs ?? {}, "attach-browser-extension-assets"),
    );
    const verifier = attachSteps.findIndex(
      (step) => step.name === "Verify browser extension asset set",
    );
    const upload = attachSteps.findIndex(
      (step) => step.name === "Attach browser extension review artifacts",
    );
    assert.notEqual(verifier, -1);
    assert.notEqual(upload, -1);
    [attachSteps[verifier], attachSteps[upload]] = [
      attachSteps[upload]!,
      attachSteps[verifier]!,
    ];
  });
  assert.throws(() => validateReleaseWorkflow(reorderedExtensionVerifier));

  const storeWithoutAttachReadback = mutateWorkflow(source, (workflow) => {
    requiredJob(workflow.jobs ?? {}, "publish-chrome-web-store").needs = [
      "release-please",
      "package-browser-extension",
    ];
  });
  assert.throws(() => validateReleaseWorkflow(storeWithoutAttachReadback));

  const splitClobber = mutateWorkflow(source, (workflow) => {
    const upload = namedStep(
      requiredJob(workflow.jobs ?? {}, "attach-assets"),
      "Attach release archives",
    );
    upload.run = [
      "clobber_prefix='--clo'",
      'gh release upload "$TAG_NAME" dist/* --repo "$GITHUB_REPOSITORY" "${clobber_prefix}bber"',
    ].join("\n");
  });
  assert.throws(() => validateReleaseWorkflow(splitClobber));

  for (const replacement of [
    "# python -B scripts/release_integrity.py verify-source \\",
    'echo "python -B scripts/release_integrity.py verify-source \\\"',
  ]) {
    const fakeSourceChecker = mutateWorkflow(source, (workflow) => {
      const sourceStep = namedStep(
        requiredJob(workflow.jobs ?? {}, "build"),
        "Verify native release source binding",
      );
      sourceStep.run = replaceRequired(
        sourceStep.run ?? "",
        "python -B scripts/release_integrity.py verify-source \\",
        replacement,
        "source checker decoy",
      );
    });
    assert.throws(() => validateReleaseWorkflow(fakeSourceChecker));
  }
}

test("parsed contract rejects adversarial mutations with LF and CRLF", () => {
  const canonical = readFileSync(WORKFLOW_URL, "utf8");
  for (const lineEnding of ["\n", "\r\n"]) {
    const source = canonical.replace(/\r?\n/g, lineEnding);
    assertAdversarialMutations(source);
  }
});
