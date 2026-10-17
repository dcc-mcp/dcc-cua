"""Exercise the production visibility transition with immutable artifact fixtures."""

import copy
import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts import publish_verified_release as publisher
from scripts.test_release_integrity import (
    MODULE, RUN_ID, SOURCE_SHA, TAG, VERSION, _build_artifacts,
    _published_release, _write_complete_release, _write_extension_assets,
)


class FakeGitHub:
    repository = "dcc-mcp/dcc-cua"

    def __init__(self, directory, tag):
        self.directory = directory
        self.tag = tag
        self.document = dict(id=42, tagName=tag, targetCommitish=SOURCE_SHA,
                             isDraft=True, isPrerelease=False, assets=[])
        self.current_latest = dict(tagName="v1.5.0", isDraft=False, isPrerelease=False)
        self.events = []
        self.source = SOURCE_SHA
        self.bad_public = None
        self.patch_error = None

    def release(self, tag):
        self.events.append("metadata")
        return copy.deepcopy(self.document)

    def tag_sha(self, tag):
        return self.source

    def latest(self):
        self.events.append("latest")
        return copy.deepcopy(self.current_latest)

    def upload(self, tag, paths):
        self.events.append("upload")
        self.document["assets"] = _published_release(self.directory, tag)["assets"]
        for asset in self.document["assets"]:
            asset["browser_download_url"] = (
                f"https://github.com/{self.repository}/releases/download/{tag}/{asset['name']}"
            )

    def patch(self, release_id, payload):
        assert release_id == 42
        self.events.append(("patch", payload.copy()))
        if self.patch_error:
            raise self.patch_error
        self.document.update(isDraft=payload["draft"], isPrerelease=payload["prerelease"])
        if payload["make_latest"] == "true":
            self.current_latest = copy.deepcopy(self.document)

    def public_digest(self, url, size):
        # This assertion catches any producer regression that exposes Latest early.
        assert self.current_latest is None or self.current_latest["tagName"] != self.tag or not self.document["isPrerelease"]
        self.events.append("public_get")
        if isinstance(self.bad_public, Exception):
            raise self.bad_public
        body = (self.directory / url.rsplit("/", 1)[1]).read_bytes()
        return self.bad_public or (len(body), hashlib.sha256(body).hexdigest())


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        _write_complete_release(self.directory)
        MODULE.write_release_provenance(
            self.directory / MODULE.PROVENANCE_NAME,
            MODULE.build_release_provenance(
                self.directory, version=VERSION, tag=TAG, source_sha=SOURCE_SHA,
                release_target_sha=SOURCE_SHA, workflow_run_id=RUN_ID,
                workflow_artifacts=_build_artifacts(),
            ),
        )
        self.gateway = FakeGitHub(self.directory, TAG)
        self.kwargs = dict(component="native", version=VERSION, tag=TAG,
                           source_sha=SOURCE_SHA, workflow_head_sha=SOURCE_SHA, native_tag=TAG)

    def publish(self):
        return publisher.publish(self.gateway, self.directory, **self.kwargs)

    def patches(self):
        return [entry[1] for entry in self.gateway.events if isinstance(entry, tuple)]

    def test_latest_changes_only_after_every_public_body_matches(self):
        result = self.publish()
        self.assertEqual(result["asset_count"], 13)
        self.assertEqual(result["latest"], TAG)
        events = self.gateway.events
        staging, stable = [i for i, event in enumerate(events) if isinstance(event, tuple)]
        gets = [i for i, event in enumerate(events) if event == "public_get"]
        self.assertLess(events.index("upload"), staging)
        self.assertEqual(len(gets), 13)
        self.assertLess(staging, min(gets))
        self.assertLess(max(gets), stable)
        self.assertEqual(self.patches(), [
            dict(draft=False, prerelease=True, make_latest="false"),
            dict(draft=False, prerelease=False, make_latest="true"),
        ])

    def test_bootstrap_has_no_latest_until_verified(self):
        self.gateway.current_latest = None
        self.assertEqual(self.publish()["latest"], TAG)

    def test_unknown_patch_outcome_is_not_retried(self):
        self.gateway.patch_error = subprocess.CalledProcessError(1, "gh patch")
        with self.assertRaises(subprocess.CalledProcessError):
            self.publish()
        self.assertEqual(len(self.patches()), 1)
        self.assertNotIn("public_get", self.gateway.events)

    def test_http_wrong_size_and_wrong_digest_never_promote_stable(self):
        for failure in (RuntimeError("HTTP 404"), (0, "0" * 64), (1, "0" * 64)):
            with self.subTest(failure=failure):
                self.gateway = FakeGitHub(self.directory, TAG)
                self.gateway.bad_public = failure
                with self.assertRaises((ValueError, RuntimeError)):
                    self.publish()
                self.assertTrue(self.gateway.document["isPrerelease"])
                self.assertEqual(len(self.patches()), 1)
                self.assertEqual(self.gateway.current_latest["tagName"], "v1.5.0")

    def test_complete_public_prerelease_resumes_without_upload_or_restage(self):
        self.gateway.upload(TAG, [])
        self.gateway.document.update(isDraft=False, isPrerelease=True)
        self.gateway.events.clear()
        self.publish()
        self.assertNotIn("upload", self.gateway.events)
        self.assertEqual(len(self.patches()), 1)
        self.assertFalse(self.patches()[0]["prerelease"])

    def test_existing_stable_is_never_unpublished_and_idempotent(self):
        self.publish()
        self.gateway.events.clear()
        self.publish()
        self.assertNotIn("upload", self.gateway.events)
        self.assertEqual(self.patches(), [])
        self.assertEqual(self.gateway.events.count("public_get"), 13)

    def test_bad_inventory_never_mutates_visibility_or_overwrites(self):
        self.gateway.upload(TAG, [])
        good = copy.deepcopy(self.gateway.document)
        asset = good["assets"][0]
        changes = [
            {"assets": good["assets"][:-1]},
            {"assets": [*good["assets"], asset]},
            {"assets": [*good["assets"], {**asset, "name": "extra"}]},
            *[{"assets": [{**asset, key: value}, *good["assets"][1:]]}
              for key, value in (("state", "new"), ("size", 0), ("digest", "sha256:" + "0" * 64))],
        ]
        for change in changes:
            with self.subTest(change=change):
                self.gateway.document = {**copy.deepcopy(good), **change}
                self.gateway.events.clear()
                with self.assertRaises(ValueError):
                    self.publish()
                self.assertEqual(self.patches(), [])
                self.assertNotIn("upload", self.gateway.events)

    def test_changed_identity_and_newer_latest_fail_before_upload(self):
        for key, value in (("targetCommitish", "b" * 40), ("tagName", "v0.0.0"),
                           ("isDraft", None)):
            with self.subTest(key=key):
                self.gateway = FakeGitHub(self.directory, TAG)
                self.gateway.document[key] = value
                with self.assertRaises(ValueError):
                    self.publish()
                self.assertNotIn("upload", self.gateway.events)
        for change in ("tag", "newer", "extension_latest"):
            self.gateway = FakeGitHub(self.directory, TAG)
            if change == "tag":
                self.gateway.source = "b" * 40
            else:
                self.gateway.current_latest["tagName"] = "v9.0.0" if change == "newer" else "dcc-cua-browser-extension-v0.3.0"
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.publish()
            self.assertNotIn("upload", self.gateway.events)

    def test_release_id_change_between_reads_is_rejected(self):
        original = self.gateway.release
        calls = 0
        def changing(tag):
            nonlocal calls
            calls += 1
            metadata = original(tag)
            if calls > 1:
                metadata["id"] = 99
            return metadata
        self.gateway.release = changing
        with self.assertRaises(ValueError):
            self.publish()
        self.assertNotIn("upload", self.gateway.events)

    def test_unexpected_public_url_never_promotes_stable(self):
        self.gateway.upload(TAG, [])
        self.gateway.document["assets"][0]["browser_download_url"] = "https://other.invalid/asset"
        with self.assertRaises(ValueError):
            self.publish()
        self.assertEqual(len(self.patches()), 1)

    def test_extension_preserves_native_latest_and_requires_it(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            version = "0.3.0"
            tag = f"dcc-cua-browser-extension-v{version}"
            _write_extension_assets(directory, version)
            gateway = FakeGitHub(directory, tag)
            gateway.current_latest["tagName"] = TAG
            kwargs = dict(component="extension", version=version, tag=tag,
                          source_sha=SOURCE_SHA, workflow_head_sha=SOURCE_SHA, native_tag=TAG)
            result = publisher.publish(gateway, directory, **kwargs)
            self.assertEqual(result["latest"], TAG)
            self.assertEqual(result["asset_count"], 4)
            self.assertTrue(all(event[1]["make_latest"] == "false"
                                for event in gateway.events if isinstance(event, tuple)))
            for latest in (None, dict(tagName="v9.0.0", isDraft=False, isPrerelease=False)):
                gateway = FakeGitHub(directory, tag)
                gateway.current_latest = latest
                with self.assertRaises(ValueError):
                    publisher.publish(gateway, directory, **kwargs)
                self.assertNotIn("upload", gateway.events)

    def test_patch_uses_json_stdin_and_retains_failure(self):
        with patch.object(publisher.subprocess, "check_output", return_value=b"{}") as command:
            publisher.GitHub("dcc-mcp/dcc-cua").patch(42, dict(prerelease=False, make_latest="true"))
        args, kwargs = command.call_args
        self.assertEqual(args[0][-4:], ["--method", "PATCH", "--input", "-"])
        self.assertEqual(kwargs["input"], b'{"prerelease": false, "make_latest": "true"}')


if __name__ == "__main__":
    unittest.main()
