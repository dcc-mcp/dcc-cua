import argparse
import re

SHA_PATTERN = re.compile(r"[0-9a-f]{40}")


def select_source_sha(
    event_name: str, event_sha: str, pull_request_head_sha: str
) -> str:
    if event_name == "pull_request":
        selected = pull_request_head_sha
    elif event_name in {"push", "workflow_dispatch"}:
        selected = event_sha
    else:
        raise ValueError(f"unsupported CI event: {event_name}")
    if SHA_PATTERN.fullmatch(selected) is None:
        raise ValueError(f"{event_name} does not provide an exact source commit SHA")
    return selected


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event-name", required=True)
    parser.add_argument("--event-sha", required=True)
    parser.add_argument("--pull-request-head-sha", default="")
    args = parser.parse_args()
    print(
        select_source_sha(args.event_name, args.event_sha, args.pull_request_head_sha)
    )


if __name__ == "__main__":
    main()
