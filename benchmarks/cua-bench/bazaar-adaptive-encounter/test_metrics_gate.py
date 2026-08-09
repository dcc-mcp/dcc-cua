import unittest

from metrics_gate import evaluate_live_observation, evaluate_metrics


class MetricsGateTests(unittest.TestCase):
    def test_accepts_completed_low_cost_run(self):
        result = evaluate_metrics(
            {
                "schema": "dcc-cua.host-jsonl.metrics.v2",
                "run_status": "succeeded",
                "transport_success": True,
                "errors_total": 0,
                "action_requests_total": 4,
                "action_kinds": {"click": 4},
                "standalone_snapshot_requests_total": 3,
                "json_input_bytes": 1000,
                "json_output_bytes": 2000,
            },
            max_actions=4,
            max_moves=0,
            max_snapshots=4,
            max_json_bytes=4096,
        )
        self.assertTrue(result["passed"])

    def test_rejects_running_or_over_budget_run(self):
        result = evaluate_metrics(
            {
                "schema": "dcc-cua.host-jsonl.metrics.v2",
                "run_status": "running",
                "transport_success": None,
                "errors_total": 0,
                "action_requests_total": 5,
                "action_kinds": {"move": 2, "click": 3},
                "standalone_snapshot_requests_total": 1,
                "json_input_bytes": 100,
                "json_output_bytes": 100,
            },
            max_actions=4,
            max_moves=0,
            max_snapshots=4,
            max_json_bytes=4096,
        )
        self.assertFalse(result["passed"])
        self.assertFalse(result["checks"]["completed"])
        self.assertFalse(result["checks"]["action_budget"])
        self.assertFalse(result["checks"]["movement_budget"])

    def test_accepts_persistent_low_latency_live_observation(self):
        result = evaluate_live_observation(
            {
                "type": "live_observation_state",
                "result": {
                    "active": True,
                    "capture_mode": "persistent_wgc",
                    "capture_failures": 0,
                    "recent_effective_fps": 10.4,
                    "last_capture_duration_ms": 84,
                    "latest_frame_age_ms": 92,
                },
            },
            min_recent_fps=8.0,
            max_capture_ms=125,
            max_frame_age_ms=250,
        )
        self.assertTrue(result["passed"])

    def test_rejects_old_or_degraded_live_observation(self):
        result = evaluate_live_observation(
            {
                "active": True,
                "capture_mode": "one_shot_wgc_fallback",
                "capture_failures": 0,
                "recent_effective_fps": 3.37,
                "last_capture_duration_ms": 294,
                "latest_frame_age_ms": 293,
            },
            min_recent_fps=8.0,
            max_capture_ms=125,
            max_frame_age_ms=250,
        )
        self.assertFalse(result["passed"])
        self.assertFalse(result["checks"]["persistent_capture"])
        self.assertFalse(result["checks"]["recent_fps"])
        self.assertFalse(result["checks"]["capture_latency"])


if __name__ == "__main__":
    unittest.main()
