"""Admit only missing or exactly matching releases for an existing immutable tag.

The pinned release-please action tolerates createRef's existing-tag 422 and then
creates its draft. Only an authenticated release lookup's explicit 404 admits
that tag-only recovery; authorization/server/transport failures remain failures.
"""

import argparse
import json
import os
import re
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def existing_release(repository: str, tag: str, source_sha: str) -> str:
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise ValueError("invalid repository")
    if not re.fullmatch(r"(?:v|dcc-cua-browser-extension-v)\d+\.\d+\.\d+", tag):
        raise ValueError("invalid release tag")
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha):
        raise ValueError("release target must be a full commit SHA")
    request = Request(
        f"https://api.github.com/repos/{repository}/releases/tags/{tag}",
        headers={"Authorization": f"Bearer {os.environ['GH_TOKEN']}",
                 "Accept": "application/vnd.github+json", "Cache-Control": "no-cache"},
    )
    try:
        with urlopen(request, timeout=30) as response:
            document = json.load(response)
    except HTTPError as error:
        if error.code == 404:
            return "missing"
        raise
    if document.get("tag_name") != tag or document.get("target_commitish") != source_sha:
        raise ValueError("existing release does not match its immutable tag/source")
    return "present"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("repository", "tag", "source-sha"):
        parser.add_argument(f"--{name}", required=True)
    args = parser.parse_args()
    print(existing_release(args.repository, args.tag, args.source_sha))


if __name__ == "__main__":
    main()
