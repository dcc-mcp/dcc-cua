from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "publish_browser_extension", ROOT / "scripts" / "publish_browser_extension.py"
)
assert SPEC and SPEC.loader
PUBLISH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLISH)


class FakeTransport:
    def __init__(self, responses):
        self.responses = list(responses)
        self.requests = []

    def __call__(self, method, url, headers, body):
        self.requests.append((method, url, dict(headers), body))
        if not self.responses:
            raise AssertionError("unexpected HTTP request")
        return self.responses.pop(0)


def response(status: int, body: dict[str, object], **headers: str):
    return PUBLISH.HttpResponse(
        status=status,
        headers=headers,
        body=json.dumps(body).encode("utf-8"),
    )


class BrowserStorePublishTests(unittest.TestCase):
    def make_artifact(self, directory: str, version: str = "1.2.3") -> Path:
        artifact = Path(directory) / "extension.zip"
        with zipfile.ZipFile(artifact, "w") as archive:
            archive.writestr(
                "manifest.json",
                json.dumps({"manifest_version": 3, "version": version}),
            )
        return artifact

    def test_artifact_version_is_verified_before_network_access(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifact = self.make_artifact(directory)
            self.assertEqual("1.2.3", PUBLISH.validate_artifact(artifact, "1.2.3"))
            with self.assertRaisesRegex(PUBLISH.PublishError, "version"):
                PUBLISH.validate_artifact(artifact, "9.9.9")

    def test_chrome_v2_upload_waits_then_submits_with_warnings_blocked(self) -> None:
        transport = FakeTransport(
            [
                response(200, {"uploadState": "IN_PROGRESS"}),
                response(200, {"lastAsyncUploadState": "SUCCEEDED"}),
                response(200, {"state": "PENDING_REVIEW"}),
            ]
        )
        with tempfile.TemporaryDirectory() as directory:
            result = PUBLISH.publish_chrome(
                self.make_artifact(directory),
                access_token="short-lived-token",
                publisher_id="publisher-id",
                extension_id="extension-id",
                transport=transport,
                sleeper=lambda _: None,
                poll_limit=2,
            )

        self.assertEqual(
            {"store": "chrome", "upload": "SUCCEEDED", "submission": "PENDING_REVIEW"},
            result,
        )
        upload, status, publish = transport.requests
        self.assertEqual("POST", upload[0])
        self.assertIn("/upload/v2/publishers/publisher-id/items/extension-id:upload", upload[1])
        self.assertEqual("Bearer short-lived-token", upload[2]["Authorization"])
        self.assertEqual("GET", status[0])
        self.assertTrue(status[1].endswith(":fetchStatus"))
        self.assertEqual({"blockOnWarnings": True}, json.loads(publish[3]))

    def test_edge_v1_upload_and_publish_wait_for_terminal_success(self) -> None:
        transport = FakeTransport(
            [
                response(202, {}, Location="upload-operation"),
                response(200, {"status": "InProgress"}),
                response(200, {"status": "Succeeded"}),
                response(202, {}, Location="publish-operation"),
                response(200, {"status": "Succeeded"}),
            ]
        )
        with tempfile.TemporaryDirectory() as directory:
            result = PUBLISH.publish_edge(
                self.make_artifact(directory),
                api_key="edge-api-key",
                client_id="edge-client-id",
                product_id="edge-product-id",
                transport=transport,
                sleeper=lambda _: None,
                poll_limit=2,
            )

        self.assertEqual(
            {"store": "edge", "upload": "Succeeded", "submission": "Succeeded"},
            result,
        )
        upload = transport.requests[0]
        self.assertEqual("ApiKey edge-api-key", upload[2]["Authorization"])
        self.assertEqual("edge-client-id", upload[2]["X-ClientID"])
        self.assertIn("/v1/products/edge-product-id/submissions/draft/package", upload[1])
        publish = transport.requests[3]
        self.assertEqual("text/plain", publish[2]["Content-Type"])
        self.assertEqual(
            b"DCC-CUA browser extension automated release",
            publish[3],
        )

    def test_http_errors_never_echo_provider_response_or_credentials(self) -> None:
        secret = "ultra-secret-provider-value"
        transport = FakeTransport([response(403, {"error": {"message": secret}})])
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(PUBLISH.PublishError) as raised:
                PUBLISH.publish_chrome(
                    self.make_artifact(directory),
                    access_token=secret,
                    publisher_id="publisher-id",
                    extension_id="extension-id",
                    transport=transport,
                )
        message = str(raised.exception)
        self.assertNotIn(secret, message)
        self.assertNotIn("message", message)
        self.assertIn("HTTP 403", message)

    def test_provider_states_are_allowlisted_before_logging_or_output(self) -> None:
        secret = "provider-secret-shaped-state"
        transport = FakeTransport([response(200, {"uploadState": secret})])
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(PUBLISH.PublishError) as raised:
                PUBLISH.publish_chrome(
                    self.make_artifact(directory),
                    access_token="short-lived-token",
                    publisher_id="publisher-id",
                    extension_id="extension-id",
                    transport=transport,
                )
        self.assertNotIn(secret, str(raised.exception))
        self.assertIn("UNKNOWN", str(raised.exception))

    def test_missing_environment_reports_names_without_values(self) -> None:
        with self.assertRaises(PUBLISH.PublishError) as raised:
            PUBLISH.read_required_environment(
                ["FIRST_SECRET", "SECOND_SECRET"],
                {"FIRST_SECRET": "configured-value"},
            )
        self.assertEqual("missing required environment: SECOND_SECRET", str(raised.exception))
        self.assertNotIn("configured-value", str(raised.exception))


if __name__ == "__main__":
    unittest.main(verbosity=2)
