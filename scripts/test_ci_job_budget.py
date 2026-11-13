from datetime import datetime, timedelta, timezone
import unittest

from scripts.ci_job_budget import remaining_minutes


class CiJobBudgetTests(unittest.TestCase):
    def test_setup_and_previous_steps_consume_both_diagnostic_budgets(self):
        start = datetime(2026, 11, 13, tzinfo=timezone.utc)
        for cap in (25, 35):
            with self.subTest(cap=cap):
                self.assertEqual(
                    remaining_minutes(
                        start.isoformat(), start + timedelta(minutes=30),
                        job_minutes=45, reserve_minutes=3, maximum_minutes=cap,
                    ),
                    12,
                )

    def test_rounds_down_and_never_consumes_the_upload_reserve(self):
        start = datetime(2026, 11, 13, tzinfo=timezone.utc)
        self.assertEqual(
            remaining_minutes(
                start.isoformat(), start + timedelta(minutes=40, seconds=59),
                job_minutes=45, reserve_minutes=3, maximum_minutes=25,
            ),
            1,
        )
        self.assertEqual(
            remaining_minutes(
                start.isoformat(), start, job_minutes=45,
                reserve_minutes=3, maximum_minutes=35,
            ),
            35,
        )
        for elapsed in (timedelta(minutes=42), timedelta(minutes=55), timedelta(seconds=-1)):
            with self.subTest(elapsed=elapsed), self.assertRaises(ValueError):
                remaining_minutes(
                    start.isoformat(), start + elapsed, job_minutes=45,
                    reserve_minutes=3, maximum_minutes=25,
                )

    def test_missing_timezone_or_invalid_metadata_cannot_create_a_fresh_budget(self):
        now = datetime(2026, 11, 13, tzinfo=timezone.utc)
        for started in ("", "not-a-time", "2026-11-13T00:00:00"):
            with self.subTest(started=started), self.assertRaises(ValueError):
                remaining_minutes(
                    started, now, job_minutes=45, reserve_minutes=3, maximum_minutes=25,
                )


if __name__ == "__main__":
    unittest.main()
