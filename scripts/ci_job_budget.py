"""Bound a diagnostic step within its existing cumulative Actions job deadline."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import math


def remaining_minutes(
    started_at: str,
    now: datetime,
    *,
    job_minutes: int,
    reserve_minutes: int,
    maximum_minutes: int,
) -> int:
    started = datetime.fromisoformat(started_at.replace("Z", "+00:00"))
    if started.utcoffset() is None or now.utcoffset() is None:
        raise ValueError("job timestamps must have explicit time zones")
    if not 0 < reserve_minutes < job_minutes or maximum_minutes <= 0:
        raise ValueError("invalid job or diagnostic budget")
    elapsed = (now - started).total_seconds()
    if elapsed < 0:
        raise ValueError("job start is in the future")
    remaining = math.floor((job_minutes - reserve_minutes) - elapsed / 60)
    if remaining < 1:
        raise ValueError("job budget is exhausted before diagnostic upload reserve")
    return min(maximum_minutes, remaining)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--started-at", required=True)
    parser.add_argument("--job-minutes", type=int, required=True)
    parser.add_argument("--reserve-minutes", type=int, required=True)
    parser.add_argument("--maximum-minutes", type=int, required=True)
    args = parser.parse_args()
    try:
        minutes = remaining_minutes(
            args.started_at,
            datetime.now(timezone.utc),
            job_minutes=args.job_minutes,
            reserve_minutes=args.reserve_minutes,
            maximum_minutes=args.maximum_minutes,
        )
    except ValueError as error:
        parser.exit(1, f"CI diagnostic budget unavailable: {error}\n")
    print(f"minutes={minutes}")


if __name__ == "__main__":
    main()
