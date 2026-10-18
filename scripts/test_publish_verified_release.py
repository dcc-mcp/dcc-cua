"""Exercise the production visibility transition with immutable artifact fixtures."""

import copy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError, URLError

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
        for asset_id, asset in enumerate(self.document["assets"], start=101):
            asset["id"] = asset_id
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


class GitHubReleaseLookupTests(unittest.TestCase):
    repository = "dcc-mcp/dcc-cua"
    api_url = "https://api.github.com/repos/dcc-mcp/dcc-cua/releases/42"

    def document(self):
        return dict(id=42, tag_name=TAG, target_commitish=SOURCE_SHA,
                    draft=True, prerelease=False, assets=[])

    def lookup(self, **changes):
        value = dict(apiUrl=self.api_url, tagName=TAG)
        value.update(changes)
        return value

    def test_draft_and_public_metadata_are_read_by_numeric_identity(self):
        for draft, prerelease in ((True, False), (False, True), (False, False)):
            with self.subTest(draft=draft, prerelease=prerelease):
                document = self.document()
                document.update(draft=draft, prerelease=prerelease)
                with patch.object(publisher.subprocess, "check_output", side_effect=[
                    json.dumps(self.lookup()).encode(), json.dumps(document).encode(),
                ]) as command:
                    result = publisher.GitHub(self.repository).release(TAG)
                self.assertEqual(result, publisher.normalize_release(document))
                self.assertEqual(command.call_args_list[0].args[0], [
                    "gh", "release", "view", TAG, "--repo", self.repository,
                    "--json", "apiUrl,tagName",
                ])
                self.assertEqual(command.call_args_list[1].args[0], [
                    "gh", "api", f"repos/{self.repository}/releases/42",
                ])

    def test_invalid_lookup_does_not_request_an_arbitrary_api_resource(self):
        invalid = [None, 42, self.api_url + "?other=1", self.api_url + "/assets",
                   self.api_url.replace("/42", "/0"), self.api_url.replace("/42", "/042"),
                   self.api_url.replace("api.github.com", "example.com"),
                   self.api_url.replace("dcc-mcp/dcc-cua", "other/repository")]
        for api_url in invalid:
            with self.subTest(api_url=api_url):
                with patch.object(publisher.subprocess, "check_output", return_value=
                                  json.dumps(self.lookup(apiUrl=api_url)).encode()) as command:
                    with self.assertRaises(ValueError):
                        publisher.GitHub(self.repository).release(TAG)
                self.assertEqual(command.call_count, 1)

    def test_wrong_lookup_tag_is_rejected_before_api_read(self):
        with patch.object(publisher.subprocess, "check_output", return_value=
                          json.dumps(self.lookup(tagName="v9.9.9")).encode()) as command:
            with self.assertRaises(ValueError):
                publisher.GitHub(self.repository).release(TAG)
        self.assertEqual(command.call_count, 1)

    def test_rest_identity_must_match_the_authenticated_lookup(self):
        for changes in (dict(id=43), dict(id=True), dict(tag_name="v9.9.9")):
            with self.subTest(changes=changes):
                document = self.document()
                document.update(changes)
                with patch.object(publisher.subprocess, "check_output", side_effect=[
                    json.dumps(self.lookup()).encode(), json.dumps(document).encode(),
                ]) as command:
                    with self.assertRaises(ValueError):
                        publisher.GitHub(self.repository).release(TAG)
                self.assertEqual(command.call_count, 2)

    def test_lookup_and_id_read_failures_are_not_retried(self):
        error = subprocess.CalledProcessError(1, "gh", stderr=b"HTTP 404")
        for replies in ([error], [json.dumps(self.lookup()).encode(), error]):
            with self.subTest(request_count=len(replies)):
                with patch.object(publisher.subprocess, "check_output", side_effect=replies) as command:
                    with self.assertRaises(subprocess.CalledProcessError) as caught:
                        publisher.GitHub(self.repository).release(TAG)
                self.assertIs(caught.exception, error)
                self.assertEqual(command.call_count, len(replies))


class GitHubLatestTransportTests(unittest.TestCase):
    def test_latest_uses_the_existing_job_token_for_metadata(self):
        document = dict(id=42, tag_name=TAG, target_commitish=SOURCE_SHA,
                        draft=False, prerelease=False, assets=[])
        with patch.dict(os.environ, {"GH_TOKEN": "test-job-token"}), \
                patch.object(publisher, "urlopen", return_value=
                             io.BytesIO(json.dumps(document).encode())) as request:
            result = publisher.GitHub("dcc-mcp/dcc-cua").latest()
        self.assertEqual(result, publisher.normalize_release(document))
        argument = request.call_args.args[0]
        self.assertEqual(argument.full_url,
                         "https://api.github.com/repos/dcc-mcp/dcc-cua/releases/latest")
        self.assertEqual(argument.get_header("Authorization"), "Bearer test-job-token")
        self.assertEqual(argument.get_header("Cache-control"), "no-cache")
        self.assertEqual(request.call_count, 1)

    def test_only_explicit_latest_404_allows_bootstrap(self):
        for status in (401, 403, 404, 429, 500):
            with self.subTest(status=status):
                error = HTTPError("https://api.github.com", status, "metadata failure", {}, None)
                with patch.dict(os.environ, {"GH_TOKEN": "test-job-token"}), \
                        patch.object(publisher, "urlopen", side_effect=error) as request:
                    if status == 404:
                        self.assertIsNone(publisher.GitHub("dcc-mcp/dcc-cua").latest())
                    else:
                        with self.assertRaises(HTTPError) as caught:
                            publisher.GitHub("dcc-mcp/dcc-cua").latest()
                        self.assertIs(caught.exception, error)
                self.assertEqual(request.call_count, 1)

    def test_latest_transport_failure_is_preserved_without_retry(self):
        error = URLError("connection failed")
        with patch.dict(os.environ, {"GH_TOKEN": "test-job-token"}), \
                patch.object(publisher, "urlopen", side_effect=error) as request:
            with self.assertRaises(URLError) as caught:
                publisher.GitHub("dcc-mcp/dcc-cua").latest()
        self.assertIs(caught.exception, error)
        self.assertEqual(request.call_count, 1)

    def test_missing_job_token_fails_before_network(self):
        with patch.dict(os.environ, {}, clear=True), \
                patch.object(publisher, "urlopen") as request:
            with self.assertRaises(KeyError):
                publisher.GitHub("dcc-mcp/dcc-cua").latest()
        request.assert_not_called()

    def test_public_asset_body_remains_anonymous_with_job_token_present(self):
        body = io.BytesIO(b"public bytes")
        body.status = 200
        url = f"https://github.com/dcc-mcp/dcc-cua/releases/download/{TAG}/asset.zip"
        with patch.dict(os.environ, {"GH_TOKEN": "test-job-token"}), \
                patch.object(publisher, "urlopen", return_value=body) as request:
            result = publisher.GitHub("dcc-mcp/dcc-cua").public_digest(url, 12)
        self.assertEqual(result, (12, hashlib.sha256(b"public bytes").hexdigest()))
        self.assertEqual(request.call_args.args[0].full_url, url)
        self.assertIsNone(request.call_args.args[0].get_header("Authorization"))
        self.assertEqual(request.call_count, 1)


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

    def test_actual_metadata_gateway_supports_the_complete_draft_transition(self):
        self.gateway.release = publisher.GitHub(self.gateway.repository).release
        self.gateway.latest = publisher.GitHub(self.gateway.repository).latest
        requests = []

        def transport(args, input=None):
            requests.append(args)
            if args == ["gh", "release", "view", TAG, "--repo", self.gateway.repository,
                        "--json", "apiUrl,tagName"]:
                return json.dumps(dict(tagName=TAG, apiUrl=
                    "https://api.github.com/repos/dcc-mcp/dcc-cua/releases/42")).encode()
            if args == ["gh", "api", "repos/dcc-mcp/dcc-cua/releases/42"]:
                value = self.gateway.document
                return json.dumps(dict(id=value["id"], tag_name=value["tagName"],
                    target_commitish=value["targetCommitish"], draft=value["isDraft"],
                    prerelease=value["isPrerelease"], assets=value["assets"])).encode()
            raise subprocess.CalledProcessError(1, args, stderr=b"HTTP 404")

        def latest_transport(request, timeout):
            self.assertEqual(request.get_header("Authorization"), "Bearer test-job-token")
            self.assertEqual(request.full_url,
                             "https://api.github.com/repos/dcc-mcp/dcc-cua/releases/latest")
            value = self.gateway.current_latest
            return io.BytesIO(json.dumps(dict(id=value.get("id", 43), tag_name=value["tagName"],
                target_commitish=SOURCE_SHA, draft=value["isDraft"],
                prerelease=value["isPrerelease"], assets=[])).encode())

        with patch.dict(os.environ, {"GH_TOKEN": "test-job-token"}), \
                patch.object(publisher.subprocess, "check_output", side_effect=transport), \
                patch.object(publisher, "urlopen", side_effect=latest_transport) as latest:
            result = self.publish()
        self.assertGreaterEqual(latest.call_count, 2)
        self.assertEqual(result["asset_count"], 13)
        self.assertEqual(result["latest"], TAG)
        self.assertFalse(any("releases/tags/" in argument for args in requests for argument in args))
        self.assertEqual(self.patches(), [
            dict(draft=False, prerelease=True, make_latest="false"),
            dict(draft=False, prerelease=False, make_latest="true"),
        ])

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

    def test_latest_rate_limit_stops_publication_before_upload(self):
        self.gateway.latest = publisher.GitHub(self.gateway.repository).latest
        error = HTTPError("https://api.github.com", 403, "rate limit exceeded", {}, None)
        with patch.dict(os.environ, {"GH_TOKEN": "test-job-token"}), \
                patch.object(publisher, "urlopen", side_effect=error) as request:
            with self.assertRaises(HTTPError) as caught:
                self.publish()
        self.assertIs(caught.exception, error)
        self.assertEqual(request.call_count, 1)
        self.assertNotIn("upload", self.gateway.events)
        self.assertEqual(self.patches(), [])

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
        self.assertEqual(len(self.patches()), 0)

    def test_asset_replacement_after_public_readback_never_promotes(self):
        original = self.gateway.public_digest
        def replaced(url, size):
            result = original(url, size)
            self.gateway.document["assets"][0]["id"] += 1000
            return result
        self.gateway.public_digest = replaced
        with self.assertRaisesRegex(ValueError, "asset identities changed"):
            self.publish()
        self.assertEqual(len(self.patches()), 1)
        self.assertTrue(self.gateway.document["isPrerelease"])

    def test_recovery_keeps_workflow_source_distinct_and_rejects_wrong_workflow(self):
        provenance_path = self.directory / MODULE.PROVENANCE_NAME
        import json
        provenance = json.loads(provenance_path.read_text())
        workflow = "b" * 40
        provenance["workflow_commit"] = workflow
        for artifact in provenance["build_artifacts"]:
            artifact["head_sha"] = workflow
        MODULE.write_release_provenance(provenance_path, provenance)
        with self.assertRaisesRegex(ValueError, "workflow commit"):
            self.publish()
        self.assertNotIn("upload", self.gateway.events)
        self.kwargs["workflow_head_sha"] = workflow
        self.assertEqual(self.publish()["latest"], TAG)

    def test_applied_patch_unknown_outcome_resumes_explicitly_without_repeating(self):
        for failed_patch in (1, 2):
            with self.subTest(failed_patch=failed_patch):
                self.gateway = FakeGitHub(self.directory, TAG)
                original = self.gateway.patch
                count = 0
                def applied_then_failed(release_id, payload):
                    nonlocal count
                    original(release_id, payload)
                    count += 1
                    if count == failed_patch:
                        raise TimeoutError("server applied PATCH; response lost")
                self.gateway.patch = applied_then_failed
                with self.assertRaises(TimeoutError):
                    self.publish()
                self.assertEqual(len(self.patches()), failed_patch)
                self.gateway.patch = original
                self.gateway.events.clear()
                self.publish()
                self.assertNotIn("upload", self.gateway.events)
                self.assertEqual(len(self.patches()), 2 - failed_patch)

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
