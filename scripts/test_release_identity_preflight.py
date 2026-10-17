import io
import json
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from scripts import release_identity_preflight as preflight


class PreflightTests(unittest.TestCase):
    def call(self):
        with patch.dict(preflight.os.environ, GH_TOKEN="private-test-token"):
            return preflight.existing_release("dcc-mcp/dcc-cua", "v1.11.0", "a" * 40)

    def test_only_explicit_release_404_admits_tag_only_recovery(self):
        for code in (404, 403, 429, 500, 502):
            error = HTTPError("https://api.github.com/release", code, "failure", {}, None)
            with self.subTest(code=code), patch.object(preflight, "urlopen", side_effect=error) as get:
                if code == 404:
                    self.assertEqual(self.call(), "missing")
                else:
                    with self.assertRaises(HTTPError):
                        self.call()
                self.assertEqual(get.call_count, 1)
                self.assertEqual(get.call_args.args[0].get_header("Authorization"), "Bearer private-test-token")

    def test_existing_draft_and_stable_identities_are_reused(self):
        for draft in (True, False):
            response = io.BytesIO(json.dumps(dict(tag_name="v1.11.0", target_commitish="a" * 40, draft=draft)).encode())
            with patch.object(preflight, "urlopen", return_value=response):
                self.assertEqual(self.call(), "present")

    def test_wrong_source_wrong_tag_and_unknown_outcomes_fail(self):
        for document in (
            dict(tag_name="v1.11.0", target_commitish="b" * 40),
            dict(tag_name="v1.10.0", target_commitish="a" * 40),
        ):
            with patch.object(preflight, "urlopen", return_value=io.BytesIO(json.dumps(document).encode())):
                with self.assertRaises(ValueError):
                    self.call()
        for error in (TimeoutError("unknown result"), RuntimeError("transport failed")):
            with patch.object(preflight, "urlopen", side_effect=error) as get:
                with self.assertRaises(type(error)):
                    self.call()
                self.assertEqual(get.call_count, 1)


if __name__ == "__main__":
    unittest.main()
