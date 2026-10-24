"""Exercise real pipe/EOF handling with an owned fake MCP peer, plus trace statistics."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from scripts import cua_latency as m
from scripts.cua_latency import SCHEMA, probe, summarize


PEER = '''
import json, sys, time
for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        result = {"serverInfo": {"version": "1.14.0"}}
    elif method == "ping":
        if MODE == "timeout":
            time.sleep(60)
        if MODE == "malformed":
            print("not json", flush=True)
            continue
        result = {}
    else:
        raise RuntimeError("probe attempted a task or UI operation")
    print(json.dumps({"jsonrpc":"2.0", "id":request["id"], "result":result}), flush=True)
if MODE == "ignore_eof":
    time.sleep(60)
'''


class ProtocolTests(unittest.TestCase):
    def run_peer(self, mode="normal", version="1.14.0", trials=1):
        with tempfile.TemporaryDirectory() as directory:
            peer = Path(directory) / "peer.py"
            peer.write_text(f"MODE = {mode!r}\n" + PEER, encoding="utf-8")
            return probe([sys.executable, "-u", str(peer)], trials, 2, .5, version)

    def test_real_transport_cold_warm_and_eof(self):
        report = self.run_peer(trials=2)
        self.assertEqual(len({child["pid"] for child in report["children"]}), 2)
        self.assertTrue(all(child["eof_exit"] for child in report["children"]))
        self.assertEqual(len(report["samples"]), 8)
        self.assertTrue(all(s["outcome"] == "success" for s in report["samples"]))
        result = summarize(report)
        self.assertIn("capture", result["unmeasured_stages"])
        self.assertIn("queue", result["unmeasured_stages"])

    def test_wrong_version_stops_before_warm_measurement(self):
        report = self.run_peer(version="other")
        self.assertEqual(report["samples"][0]["outcome"], "error")
        self.assertFalse(any(s["phase"] == "warm" for s in report["samples"]))
        self.assertTrue(report["children"][0]["eof_exit"])

    def test_timeout_records_failure_and_reaps_only_owned_child(self):
        report = self.run_peer("timeout")
        self.assertTrue(any(s["outcome"] == "timeout" for s in report["samples"]))
        self.assertFalse(report["children"][0]["eof_exit"])
        self.assertEqual(report["samples"][-1]["outcome"], "error")

    def test_malformed_response_is_not_a_fast_success(self):
        report = self.run_peer("malformed")
        warm = [s for s in report["samples"] if s["phase"] == "warm"]
        self.assertEqual(len(warm), 1)
        self.assertEqual(warm[0]["outcome"], "error")

    def test_exit_timeout_is_separate_from_successful_requests(self):
        report = self.run_peer("ignore_eof")
        self.assertTrue(all(s["outcome"] == "success" for s in report["samples"][:-1]))
        self.assertEqual(report["samples"][-1]["outcome"], "error")


class StatisticsTests(unittest.TestCase):
    def report(self, samples, boundary="mcp_protocol"):
        return {"schema": SCHEMA, "boundary": boundary, "samples": samples}

    def sample(self, duration, outcome="success", phase="warm", stage="end_to_end", **extra):
        return dict(duration_ms=duration, outcome=outcome, phase=phase, stage=stage, **extra)

    def test_nearest_rank_keeps_tails_and_failures_visible(self):
        samples = [self.sample(n) for n in range(1, 101)]
        samples += [self.sample(1000, "timeout"), self.sample(1, "error")]
        group = summarize(self.report(samples))["groups"][0]
        self.assertEqual((group["p50_ms"], group["p95_ms"], group["max_ms"]), (50, 95, 100))
        self.assertEqual((group["attempts"], group["errors"], group["timeouts"]), (102, 1, 1))
        self.assertFalse(group["tail_sample_warning"])

    def test_all_failed_group_has_no_percentiles(self):
        group = summarize(self.report([self.sample(5, "error")]))["groups"][0]
        self.assertIsNone(group["p95_ms"])
        self.assertEqual(group["successes"], 0)

    def test_cold_and_warm_are_not_pooled(self):
        groups = summarize(self.report([self.sample(100, phase="cold"), self.sample(1)]))["groups"]
        self.assertEqual([g["p50_ms"] for g in groups], [100, 1])

    def test_native_requires_binding_and_does_not_pool_dimensions(self):
        binding = dict(provider="dcc-cua", runtime="1.14.0", pid=42, process_birth="2026-10-24T00:00:00Z",
                       hwnd=123, observation_mode="pixels_only", width=1280, height=720,
                       source_revision="a" * 40, binary_sha256="b" * 64)
        samples = [self.sample(10, stage="capture", binding=binding, scenario="capture", trial_id="one"),
                   self.sample(20, stage="capture", binding=dict(binding, width=1920), scenario="capture", trial_id="two")]
        self.assertEqual(len(summarize(self.report(samples, "native_task"))["groups"]), 2)
        del samples[0]["binding"]["process_birth"]
        with self.assertRaises(ValueError):
            summarize(self.report(samples, "native_task"))

    def test_native_scenarios_are_not_pooled(self):
        binding = dict(provider="dcc-cua", runtime="1.14.0", pid=42, process_birth="birth",
                       hwnd=123, observation_mode="pixels_only", width=1280, height=720,
                       source_revision="a" * 40, binary_sha256="b" * 64)
        samples = [self.sample(10, binding=binding, trial_id="one", scenario="click"),
                   self.sample(20, binding=binding, trial_id="two", scenario="drag")]
        self.assertEqual(len(summarize(self.report(samples, "native_task"))["groups"]), 2)
        del samples[0]["scenario"]
        with self.assertRaises(ValueError):
            summarize(self.report(samples, "native_task"))

    def test_native_requires_exact_source_and_artifact_identity(self):
        binding = dict(provider="dcc-cua", runtime="1.14.0", pid=42, process_birth="birth",
                       hwnd=123, observation_mode="pixels_only", width=1280, height=720,
                       source_revision="a" * 40, binary_sha256="b" * 64)
        samples = [self.sample(10, binding=binding, trial_id="one", scenario="click"),
                   self.sample(20, binding=dict(binding, binary_sha256="c" * 64), trial_id="two", scenario="click")]
        self.assertEqual(len(summarize(self.report(samples, "native_task"))["groups"]), 2)
        for key in ("source_revision", "binary_sha256"):
            with self.subTest(key=key):
                invalid = dict(binding, **{key: "short"})
                with self.assertRaises(ValueError):
                    summarize(self.report([self.sample(1, binding=invalid, trial_id="one", scenario="click")], "native_task"))

    def test_invalid_measurements_are_rejected(self):
        for duration in (-1, float("nan"), float("inf"), True, "1"):
            with self.subTest(duration=duration), self.assertRaises(ValueError):
                summarize(self.report([self.sample(duration)]))
        with self.assertRaises(ValueError):
            summarize(self.report([self.sample(0, stage="capture")]))


class ReceiptTests(unittest.TestCase):
    def child(self, failure=None):
        child = Mock()
        child.process.pid = 123
        child.request.side_effect = failure or [{'serverInfo': {'version': '1.14.0'}}, {}]
        child.close.return_value = (True, 0.01, [], True)
        return child

    def run_main(self, factory):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'fixture-not-executed'
            binary.write_bytes(b'fixture')
            output = Path(directory) / 'report.json'
            args = ['probe', 'protocol', '--binary', str(binary), '--expected-sha256',
                    hashlib.sha256(b'fixture').hexdigest(), '--expected-version', '1.14.0',
                    '--trials', '3', '--warm-samples', '1', '--output', str(output)]
            with patch.object(m, 'ProtocolProcess', side_effect=factory) as constructor, \
                    patch('sys.argv', args), contextlib.redirect_stdout(io.StringIO()):
                status = m.main()
            report = json.loads(output.read_text())
            self.assertEqual(status, 1)
            self.assertTrue(report['samples'])
            return report, constructor.call_count

    def test_initialize_eof_preserves_prior_trial_and_cleanup(self):
        report, count = self.run_main([self.child(), self.child(EOFError()), self.child()])
        self.assertEqual(count, 3)
        self.assertEqual(len(report['children']), 3)
        self.assertEqual(sum(s['reason'] == 'EOFError' for s in report['samples'] if 'reason' in s), 1)
        self.assertEqual(sum(s['stage'] == 'cleanup' for s in report['samples']), 3)

    def test_ping_eof_preserves_startup_and_cleanup(self):
        bad = self.child([{'serverInfo': {'version': '1.14.0'}}, EOFError()])
        report, _ = self.run_main([bad, self.child(), self.child()])
        self.assertEqual(report['samples'][0]['outcome'], 'success')
        self.assertEqual(report['samples'][1]['reason'], 'EOFError')
        self.assertEqual(report['samples'][2]['stage'], 'cleanup')

    def test_spawn_failure_preserves_prior_trial_without_fake_child(self):
        report, count = self.run_main([self.child(), OSError(), self.child()])
        self.assertEqual(count, 2)
        self.assertEqual(len(report['children']), 1)
        self.assertEqual(report['samples'][-1]['reason'], 'OSError')
        self.assertTrue(report['run_aborted'])

    def test_reader_start_failure_keeps_child_and_cleanup(self):
        bad = self.child()
        bad.start_reader.side_effect = RuntimeError('thread unavailable')
        report, _ = self.run_main([bad, self.child(), self.child()])
        self.assertEqual(report['samples'][0]['reason'], 'RuntimeError')
        self.assertTrue(report['children'][0]['exit_confirmed'])
        bad.close.assert_called_once_with()

    def test_cleanup_exception_preserves_original_error_and_stops(self):
        for error in (OSError(), subprocess.TimeoutExpired('fixture', 1)):
            with self.subTest(error=type(error).__name__):
                bad = self.child(ValueError())
                bad.close.side_effect = error
                report, count = self.run_main([self.child(), bad, self.child()])
                self.assertEqual(count, 2)
                self.assertEqual(report['samples'][-2]['reason'], 'ValueError')
                self.assertEqual(report['samples'][-1]['errors'], [type(error).__name__])
                self.assertFalse(report['children'][-1]['exit_confirmed'])


class CleanupTests(unittest.TestCase):
    def owned(self):
        child = object.__new__(m.ProtocolProcess)
        child.timeout = 1
        child.process = Mock(returncode=0)
        child.reader = Mock()
        child.reader.is_alive.return_value = False
        return child

    def test_kill_failure_reports_unresolved_exit(self):
        child = self.owned()
        child.process.wait.side_effect = subprocess.TimeoutExpired('fixture', 1)
        child.process.kill.side_effect = OSError()
        clean, _, errors, exited = child.close()
        self.assertFalse(clean)
        self.assertFalse(exited)
        self.assertEqual(errors, ['OSError'])
        child.process.kill.assert_called_once_with()

    def test_second_wait_timeout_reports_unresolved_exit(self):
        child = self.owned()
        child.process.wait.side_effect = subprocess.TimeoutExpired('fixture', 1)
        clean, _, errors, exited = child.close()
        self.assertFalse(clean)
        self.assertFalse(exited)
        self.assertEqual(errors, ['TimeoutExpired'])
        self.assertEqual(child.process.wait.call_count, 2)

    def test_stdout_close_failure_does_not_erase_exit_evidence(self):
        child = self.owned()
        child.process.stdout.close.side_effect = OSError()
        clean, _, errors, exited = child.close()
        self.assertFalse(clean)
        self.assertTrue(exited)
        self.assertEqual(errors, ['OSError'])

    def test_unstarted_reader_is_not_joined(self):
        child = self.owned()
        child.reader.ident = None
        clean, _, errors, exited = child.close()
        self.assertTrue(clean)
        self.assertTrue(exited)
        self.assertEqual(errors, [])
        child.reader.join.assert_not_called()

    def test_live_reader_stream_is_not_closed(self):
        child = self.owned()
        child.reader.is_alive.return_value = True
        clean, _, errors, _ = child.close()
        self.assertFalse(clean)
        self.assertEqual(errors, ['ReaderUnresolved'])
        child.process.stdout.close.assert_not_called()



if __name__ == "__main__":
    unittest.main()
