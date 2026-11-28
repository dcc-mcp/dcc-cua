"""Check recovery after a delivered action loses its post-action observation."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "skills/cua-cli/scripts"))
from action_sequence import window_sequence


class ObservationRecoveryTests(unittest.TestCase):
    def test_failed_recovery_preserves_delivery_and_stops_before_next_action(self):
        observation = {"observation_id": "before", "observation": {"process_id": 1, "window_handle": 2}}
        calls, journal = [], []

        def call(method, params):
            calls.append(method)
            if method == "snapshot":
                raise RuntimeError("target unavailable")
            self.assertEqual(params["observation_id"], "before")
            return {"status": "succeeded", "observation_required": True,
                    "post_snapshot": {"success": False, "action_was_executed": True}}

        with self.assertRaisesRegex(RuntimeError, "target unavailable"):
            window_sequence(call, (1, 2), observation, [{"action": "click"}, {"action": "drag"}], journal)
        self.assertEqual(calls, ["execute_action", "snapshot"])
        self.assertEqual(len(journal), 1)
        self.assertEqual(journal[0]["delivery_status"], "succeeded")
        self.assertEqual(journal[0]["status"], "observation_failed")
