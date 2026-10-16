"""Native-fixture protocol and cleanup regressions without an X11 display."""
from contextlib import contextmanager
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from scripts import test_linux_pixels_x11 as x11


class NativeFixtureTests(unittest.TestCase):
    @contextmanager
    def reply_pipe(self, value):
        reader, writer = os.pipe()
        with os.fdopen(reader, "rb") as output, os.fdopen(writer, "wb") as input_:
            input_.write(value)
            input_.flush()
            yield SimpleNamespace(stdout=output)

    def test_reply_accepts_one_bounded_json_line(self):
        with self.reply_pipe(b'{"pid":42,"target":77,"sibling":78}\n') as process:
            self.assertEqual(x11.fixture_reply(process),
                             {"pid": 42, "target": 77, "sibling": 78})

    def test_partial_response_cannot_escape_the_deadline(self):
        with self.reply_pipe(b'{"pid":') as process:
            with self.assertRaisesRegex(AssertionError, "timed out"):
                x11.fixture_reply(process, timeout=0.01)

    def test_expired_deadline_rejects_even_an_available_reply(self):
        with self.reply_pipe(b'{"ok":"blue"}\n') as process:
            with self.assertRaisesRegex(AssertionError, "timed out"):
                x11.fixture_reply(process, timeout=0)

    def test_oversized_reply_is_rejected(self):
        with self.reply_pipe(b"x" * 4096) as process:
            with self.assertRaisesRegex(AssertionError, "exceeded its bound"):
                x11.fixture_reply(process)

    def test_eof_before_newline_is_rejected(self):
        reader, writer = os.pipe()
        os.write(writer, b'{"pid":')
        os.close(writer)
        with os.fdopen(reader, "rb") as output:
            with self.assertRaisesRegex(AssertionError, "exited before"):
                x11.fixture_reply(SimpleNamespace(stdout=output))

    def test_command_requires_matching_acknowledgement(self):
        process = Mock()
        with patch.object(x11, "fixture_reply", return_value={"ok": "blue"}):
            x11.fixture_command(process, "blue")
        process.stdin.write.assert_called_once_with("blue\n")
        process.stdin.flush.assert_called_once_with()
        with patch.object(x11, "fixture_reply", return_value={"ok": "hide"}):
            with self.assertRaises(AssertionError):
                x11.fixture_command(process, "blue")

    def test_arbitrary_commands_are_never_sent(self):
        process = Mock()
        with self.assertRaises(AssertionError):
            x11.fixture_command(process, "blue\ndestroy")
        process.stdin.write.assert_not_called()

    @contextmanager
    def mocked_fixture(self, ready=None):
        process = Mock(pid=42, returncode=0)
        if ready is None:
            ready = {"pid": 42, "target": 77, "sibling": 78}
        with patch.object(x11.subprocess, "run"), \
                patch.object(x11.subprocess, "Popen", return_value=process), \
                patch.object(x11, "fixture_reply", return_value=ready), \
                patch.object(Path, "resolve", return_value=Path("/native-fixture")):
            yield process

    def test_real_process_pid_is_required_and_mismatch_still_cleans_up(self):
        with self.mocked_fixture({"pid": 999, "target": 77, "sibling": 78}) as process:
            with self.assertRaisesRegex(AssertionError, "own PID"):
                with x11.native_fixture():
                    self.fail("must not accept a different process identity")
            process.stdin.close.assert_called_once_with()
            process.wait.assert_called_once_with(timeout=5)
            process.stdout.close.assert_called_once_with()

    def test_native_executable_identity_is_required(self):
        with self.mocked_fixture() as process:
            with patch.object(Path, "resolve", side_effect=[Path("/python"), Path("/native")]):
                with self.assertRaisesRegex(AssertionError, "native executable identity"):
                    with x11.native_fixture():
                        self.fail("must not accept a substituted executable identity")
            process.stdin.close.assert_called_once_with()
            process.wait.assert_called_once_with(timeout=5)
            process.stdout.close.assert_called_once_with()

    def test_successful_body_closes_stdin_and_reaps_the_fixture(self):
        with self.mocked_fixture() as process:
            with x11.native_fixture() as (native, identity):
                self.assertIs(native, process)
                self.assertEqual(identity, {"pid": 42, "target": 77, "sibling": 78})
            process.stdin.close.assert_called_once_with()
            process.wait.assert_called_once_with(timeout=5)
            process.stdout.close.assert_called_once_with()
            process.kill.assert_not_called()

    def test_failed_test_body_still_reaps_the_fixture(self):
        with self.mocked_fixture() as process:
            with self.assertRaisesRegex(RuntimeError, "test failed"):
                with x11.native_fixture():
                    raise RuntimeError("test failed")
            process.stdin.close.assert_called_once_with()
            process.wait.assert_called_once_with(timeout=5)
            process.stdout.close.assert_called_once_with()

    def test_stalled_fixture_is_killed_reaped_and_fails_the_test(self):
        with self.mocked_fixture() as process:
            process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 5), -9]
            process.returncode = -9
            with self.assertRaisesRegex(AssertionError, "fixture failed"):
                with x11.native_fixture():
                    pass
            process.kill.assert_called_once_with()
            self.assertEqual(process.wait.call_count, 2)
            process.stdout.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
