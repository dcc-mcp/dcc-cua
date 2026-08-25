import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { parseDocument } from "yaml";

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

function parseWorkflow(source: string): Workflow {
  const document = parseDocument(source, { uniqueKeys: true });
  assert.deepEqual(
    document.errors.map((error) => error.message),
    [],
    "release workflow must be valid YAML with unique mapping keys",
  );
  return document.toJS({ maxAliasCount: 0 }) as Workflow;
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
  const nativeUpload = actionStep(build, "actions/upload-artifact");
  assert.equal(nativeUpload.id, "upload-native");
  assert.equal(nativeUpload.with?.name, "dcc-cua-native-${{ matrix.platform }}");

  const consolidate = requiredJob(jobs, "consolidate-native");
  assertNeeds(consolidate, ["release-please", "build"]);
  assert.equal(
    actionStep(consolidate, "actions/download-artifact").with?.pattern,
    "dcc-cua-native-*",
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
  const nativeArtifact = namedStep(attach, "Verify immutable workflow artifact");
  assert.equal(
    nativeArtifact.env?.EXPECTED_ARTIFACT_DIGEST,
    "${{ needs.consolidate-native.outputs.artifact_digest }}",
  );
  namedStep(attach, "Refuse existing native release assets");
  namedStep(attach, "Verify native release asset completeness and provenance");
  assert.ok(!(namedStep(attach, "Attach release archives").run ?? "").includes("--clobber"));

  const extension = requiredJob(jobs, "package-browser-extension");
  assert.equal(
    actionStep(extension, "actions/checkout").with?.ref,
    "${{ needs.release-please.outputs.extension_source_sha }}",
  );
  namedStep(extension, "Verify browser extension release source binding");
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
  for (const jobName of extensionConsumers) {
    const job = requiredJob(jobs, jobName);
    const download = actionStep(job, "actions/download-artifact");
    assert.equal(
      download.with?.["artifact-ids"],
      "${{ needs.package-browser-extension.outputs.artifact_id }}",
      `${jobName} must consume the exact build artifact ID`,
    );
    assert.equal(
      namedStep(job, "Verify immutable workflow artifact").env
        ?.EXPECTED_ARTIFACT_DIGEST,
      "${{ needs.package-browser-extension.outputs.artifact_digest }}",
      `${jobName} must consume the exact build artifact digest`,
    );
  }
  namedStep(
    requiredJob(jobs, "attach-browser-extension-assets"),
    "Refuse existing browser extension release assets",
  );
}

test("release workflow is immutable, least-privilege, and source-bound", () => {
  validateReleaseWorkflow(readFileSync(WORKFLOW_URL, "utf8"));
});

test("parsed contract rejects duplicate, moved, and textual-decoy mutations", () => {
  const source = readFileSync(WORKFLOW_URL, "utf8");
  validateReleaseWorkflow(source);

  const duplicate = source.replace(
    "  attach-assets:\n",
    "  attach-assets:\n  attach-assets:\n",
  );
  assert.throws(() => validateReleaseWorkflow(duplicate));

  const moved = source.replace(
    "  consolidate-native:\n",
    "  consolidate-native-decoy:\n",
  );
  assert.throws(() => validateReleaseWorkflow(moved));

  const exactId =
    "artifact-ids: ${{ needs.consolidate-native.outputs.artifact_id }}";
  const decoy = `${source.replace(exactId, "name: dcc-cua-native-release")}\n# ${exactId}\n`;
  assert.throws(() => validateReleaseWorkflow(decoy));
});
