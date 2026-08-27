import unittest

from scripts.select_ci_source_sha import select_source_sha


class SelectCiSourceShaTests(unittest.TestCase):
    event_sha = "a" * 40
    pull_request_head_sha = "b" * 40

    def test_pull_request_uses_only_the_exact_head_not_the_merge_ref(self):
        self.assertEqual(
            select_source_sha(
                "pull_request", self.event_sha, self.pull_request_head_sha
            ),
            self.pull_request_head_sha,
        )

    def test_push_uses_the_exact_event_sha_and_ignores_pr_shaped_data(self):
        self.assertEqual(
            select_source_sha("push", self.event_sha, self.pull_request_head_sha),
            self.event_sha,
        )

    def test_workflow_dispatch_uses_the_exact_event_sha(self):
        self.assertEqual(
            select_source_sha("workflow_dispatch", self.event_sha, ""),
            self.event_sha,
        )

    def test_missing_or_invalid_event_identity_fails_closed(self):
        cases = (
            ("pull_request", self.event_sha, ""),
            ("push", "", ""),
            ("workflow_dispatch", "merge-ref", ""),
            ("schedule", self.event_sha, ""),
        )
        for event_name, event_sha, pull_request_head_sha in cases:
            with self.subTest(event_name=event_name), self.assertRaises(ValueError):
                select_source_sha(event_name, event_sha, pull_request_head_sha)


if __name__ == "__main__":
    unittest.main()
