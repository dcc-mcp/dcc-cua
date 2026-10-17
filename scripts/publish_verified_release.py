#!/usr/bin/env python3
"""Publish immutable release assets before advertising an installable Latest.

Draft -> public prerelease -> public byte verification -> stable/Latest.
No operation retries, overwrites assets, or unpublishes an existing stable release.
GitHub's release:published event also fires for the prerelease staging transition;
consumers must filter prereleases or use Latest to require install readiness.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.error import HTTPError
from urllib.request import Request, urlopen

try:
    from scripts import release_integrity as integrity
except ModuleNotFoundError:
    import release_integrity as integrity


def normalize_release(document: dict) -> dict:
    return {
        "id": document["id"],
        "tagName": document["tag_name"],
        "targetCommitish": document["target_commitish"],
        "isDraft": document["draft"],
        "isPrerelease": document["prerelease"],
        "assets": document["assets"],
    }


class GitHub:
    """Authenticated metadata/mutations; unauthenticated public download proof."""

    def __init__(self, repository: str):
        if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
            raise ValueError("invalid repository")
        self.repository = repository

    def api(self, path: str, payload: dict | None = None):
        args = ["gh", "api", f"repos/{self.repository}/{path}"]
        if payload is not None:
            args += ["--method", "PATCH", "--input", "-"]
        # stderr and exit status remain available to the CI caller; never retry.
        output = subprocess.check_output(
            args, input=None if payload is None else json.dumps(payload).encode(),
        )
        return json.loads(output)

    def release(self, tag: str) -> dict:
        return normalize_release(self.api(f"releases/tags/{tag}"))

    def tag_sha(self, tag: str) -> str:
        obj = self.api(f"git/ref/tags/{tag}")["object"]
        for _ in range(5):
            if obj["type"] == "commit":
                return obj["sha"]
            if obj["type"] != "tag":
                break
            obj = self.api(f"git/tags/{obj['sha']}")["object"]
        raise ValueError("release tag does not resolve to a bounded commit identity")

    def latest(self) -> dict | None:
        request = Request(
            f"https://api.github.com/repos/{self.repository}/releases/latest",
            headers={"Cache-Control": "no-cache", "Accept": "application/vnd.github+json"},
        )
        try:
            with urlopen(request, timeout=30) as response:
                return normalize_release(json.load(response))
        except HTTPError as error:
            if error.code == 404:
                return None  # Bootstrap: no stable release exists yet.
            raise

    def upload(self, tag: str, paths: list[Path]) -> None:
        subprocess.run(
            ["gh", "release", "upload", tag, "--repo", self.repository,
             *[str(path) for path in paths]], check=True,
        )

    def patch(self, release_id: int, payload: dict) -> None:
        self.api(f"releases/{release_id}", payload)

    def public_digest(self, url: str, expected_size: int) -> tuple[int, str]:
        digest = hashlib.sha256()
        size = 0
        request = Request(url, headers={"Cache-Control": "no-cache"})
        with urlopen(request, timeout=60) as response:
            if response.status != 200:
                raise ValueError("public release asset did not return HTTP 200")
            while chunk := response.read(min(1024 * 1024, expected_size + 1 - size)):
                size += len(chunk)
                if size > expected_size:
                    raise ValueError("public release asset exceeds its verified size")
                digest.update(chunk)
        return size, digest.hexdigest()


def _version(tag: str) -> tuple[int, int, int]:
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("Latest must identify a stable native runtime version")
    return tuple(map(int, tag[1:].split(".")))


def publish(
    gateway, directory: Path, *, component: str, version: str, tag: str,
    source_sha: str, workflow_head_sha: str, native_tag: str,
    record=lambda event: None,
) -> dict:
    if component not in ("native", "extension"):
        raise ValueError("unknown release component")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("release must use stable semver")
    expected_tag = f"v{version}" if component == "native" else f"dcc-cua-browser-extension-v{version}"
    if tag != expected_tag:
        raise ValueError("release tag differs from the component version")
    for value in (source_sha, workflow_head_sha):
        if not re.fullmatch(r"[0-9a-f]{40}", value):
            raise ValueError("release sources must be full commit SHAs")
    _version(native_tag)
    if component == "native" and native_tag != tag:
        raise ValueError("native Latest anchor differs from its release version")

    def content(metadata):
        if component == "native":
            integrity.verify_uploaded_native_release(
                metadata, directory, version=version, tag=tag, source_sha=source_sha,
                workflow_head_sha=workflow_head_sha,
            )
        else:
            integrity.verify_uploaded_extension_release(
                metadata, directory, version=version, tag=tag, source_sha=source_sha,
            )

    # Validate the complete local immutable artifact before any network mutation.
    paths = sorted(directory.iterdir())
    local_assets = [dict(
        name=path.name, size=path.stat().st_size, state="uploaded",
        digest="sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
    ) for path in paths if integrity.is_regular_unlinked_file(path)]
    content(dict(tagName=tag, targetCommitish=source_sha, assets=local_assets))
    initial = gateway.release(tag)
    release_id = initial.get("id")
    if type(release_id) is not int or release_id <= 0:
        raise ValueError("release must have an exact positive numeric identity")
    frozen_assets = None

    def asset_identities(metadata):
        identities = []
        for asset in metadata["assets"]:
            asset_id = asset.get("id")
            if type(asset_id) is not int or asset_id <= 0:
                raise ValueError("uploaded asset must have a positive numeric identity")
            url = (f"https://github.com/{gateway.repository}/releases/download/"
                   f"{tag}/{asset['name']}")
            if asset.get("browser_download_url") != url:
                raise ValueError("release public asset URL does not match its immutable identity")
            identities.append((asset_id, asset["name"], asset["size"], asset["digest"], url))
        if len({entry[0] for entry in identities}) != len(identities):
            raise ValueError("uploaded asset IDs must be unique")
        return sorted(identities)

    def fresh(*, complete: bool) -> dict:
        metadata = gateway.release(tag)
        if (type(metadata.get("id")) is not int or metadata.get("id") != release_id
                or metadata.get("tagName") != tag
                or metadata.get("targetCommitish") != source_sha
                or gateway.tag_sha(tag) != source_sha):
            raise ValueError("fresh release/tag/source identity mismatch")
        if (type(metadata.get("isDraft")) is not bool
                or type(metadata.get("isPrerelease")) is not bool):
            raise ValueError("release visibility flags must be explicit booleans")
        if complete:
            content(metadata)
            if frozen_assets is not None and asset_identities(metadata) != frozen_assets:
                raise ValueError("uploaded asset identities changed during publication")
        return metadata

    def latest_guard():
        latest = gateway.latest()
        if latest is None:
            if component == "extension":
                raise ValueError("extension publication requires a stable native Latest")
            return None
        if latest.get("isDraft") is not False or latest.get("isPrerelease") is not False:
            raise ValueError("Latest must be a stable public native runtime")
        latest_tag = latest.get("tagName", "")
        if component == "extension":
            if latest_tag != native_tag:
                raise ValueError("browser extension must preserve native Latest")
        elif _version(latest_tag) > _version(tag):
            raise ValueError("refusing to replace a newer native Latest")
        if latest_tag == tag and (latest.get("id") != release_id
                                 or latest.get("targetCommitish") != source_sha):
            raise ValueError("Latest does not identify this exact release/source")
        return latest

    metadata = fresh(complete=False)
    latest_guard()
    if metadata.get("assets") == []:
        record(dict(phase="upload_intent", release_id=release_id, tag=tag))
        gateway.upload(tag, paths)
        metadata = fresh(complete=True)
    else:
        content(metadata)  # Complete exact set is reusable; partial sets fail closed.
    frozen_assets = asset_identities(metadata)

    if metadata["isDraft"]:
        metadata = fresh(complete=True)
        latest_guard()
        record(dict(phase="public_prerelease_intent", release_id=release_id))
        gateway.patch(release_id, dict(draft=False, prerelease=True, make_latest="false"))
        metadata = fresh(complete=True)
        if metadata["isDraft"] or not metadata["isPrerelease"]:
            raise ValueError("release did not enter public prerelease staging")

    metadata = fresh(complete=True)
    if metadata["isDraft"]:
        raise ValueError("public byte verification requires a public release")
    for asset in sorted(metadata["assets"], key=lambda asset: asset["name"]):
        url = asset["browser_download_url"]
        actual = gateway.public_digest(url, asset["size"])
        if actual != (asset["size"], asset["digest"].removeprefix("sha256:")):
            raise ValueError("public release asset bytes differ from verified artifact")
        record(dict(phase="public_asset_verified", name=asset["name"],
                    asset_id=asset["id"], url=url, size=actual[0], sha256=actual[1]))

    metadata = fresh(complete=True)
    latest = latest_guard()
    already_stable = metadata["isDraft"] is False and metadata["isPrerelease"] is False
    if not already_stable or (component == "native" and (latest or {}).get("tagName") != tag):
        record(dict(phase="stable_promotion_intent", release_id=release_id))
        gateway.patch(release_id, dict(
            draft=False, prerelease=False,
            make_latest="true" if component == "native" else "false",
        ))
    metadata = fresh(complete=True)
    if metadata["isDraft"] or metadata["isPrerelease"]:
        raise ValueError("release is not stable after promotion")
    latest = latest_guard()
    actual_latest = (latest or {}).get("tagName")
    if component == "native":
        integrity.verify_published_native_release(
            metadata, directory, version=version, tag=tag, source_sha=source_sha,
            workflow_head_sha=workflow_head_sha, actual_latest_tag=actual_latest,
        )
    else:
        integrity.verify_published_extension_release(
            metadata, directory, version=version, tag=tag, source_sha=source_sha,
            expected_latest_tag=native_tag, actual_latest_tag=actual_latest,
        )
    result = dict(phase="published_and_verified", release_id=release_id, tag=tag,
                  source_sha=source_sha, asset_count=len(paths), latest=actual_latest)
    record(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("component", "directory", "version", "tag", "source-sha",
                 "workflow-head-sha", "native-tag", "repository", "evidence"):
        parser.add_argument(f"--{name}", required=True)
    args = parser.parse_args()
    with Path(args.evidence).open("x", encoding="utf-8") as evidence:
        def record(event):
            evidence.write(json.dumps(
                dict(at=datetime.now(timezone.utc).isoformat(), **event), sort_keys=True,
            ) + "\n")
            evidence.flush()
        try:
            publish(
                GitHub(args.repository), Path(args.directory), component=args.component,
                version=args.version, tag=args.tag, source_sha=args.source_sha,
                workflow_head_sha=args.workflow_head_sha, native_tag=args.native_tag,
                record=record,
            )
        except Exception as error:
            record(dict(phase="failed", error_type=type(error).__name__, message=str(error)))
            raise


if __name__ == "__main__":
    main()
