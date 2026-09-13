"""Resolve empty published releases without replacing their immutable identities."""

import json
import os
from pathlib import Path
import re
import subprocess


SHA = re.compile(r"[0-9a-f]{40}")
VERSION = re.compile(r"\d+\.\d+\.\d+")


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def validate_release(metadata, tag, source_sha):
    if not SHA.fullmatch(source_sha):
        raise ValueError("release source must be a full commit SHA")
    if (
        metadata.get("tagName") != tag
        or metadata.get("targetCommitish") != source_sha
        or metadata.get("isDraft") is not False
        or metadata.get("isPrerelease") is not False
    ):
        raise ValueError("published release identity does not match the immutable tag")
    if metadata.get("assets") != []:
        raise ValueError(
            "recovery only accepts empty releases; existing assets cannot be replaced"
        )


def resolve(component, repository, workflow_sha, manifest):
    if component not in ("native", "browser-extension", "both"):
        raise ValueError("unknown recovery component")
    if not SHA.fullmatch(workflow_sha):
        raise ValueError("workflow source must be a full commit SHA")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise ValueError("invalid repository")
    outputs = {"release_created": "false", "extension_release_created": "false"}
    components = (
        ("native", "browser-extension") if component == "both" else (component,)
    )
    for selected in components:
        native = selected == "native"
        version = manifest["." if native else "browser-extension/chrome"]
        if not isinstance(version, str) or not VERSION.fullmatch(version):
            raise ValueError("release version must use stable semver")
        tag = f"v{version}" if native else f"dcc-cua-browser-extension-v{version}"
        metadata = json.loads(
            command(
                "gh",
                "release",
                "view",
                tag,
                "--repo",
                repository,
                "--json",
                "tagName,targetCommitish,assets,isDraft,isPrerelease",
            )
        )
        command("git", "fetch", "--no-tags", "origin", f"refs/tags/{tag}")
        source_sha = command("git", "rev-parse", "FETCH_HEAD^{commit}")
        validate_release(metadata, tag, source_sha)
        command("git", "merge-base", "--is-ancestor", source_sha, workflow_sha)
        anchor = "version.txt" if native else "browser-extension/chrome/package.json"
        value = command("git", "show", f"{source_sha}:{anchor}")
        source_version = value if native else json.loads(value)["version"]
        if source_version != version:
            raise ValueError(
                "tagged source version does not match the requested release"
            )
        if native:
            outputs.update(release_created="true", tag_name=tag, source_sha=source_sha)
        else:
            outputs.update(
                extension_release_created="true",
                extension_tag_name=tag,
                extension_source_sha=source_sha,
                extension_version=version,
            )
    return outputs


def main():
    if os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("release recovery must run from the reviewed main workflow")
    outputs = resolve(
        os.environ["RECOVER_COMPONENT"],
        os.environ["GITHUB_REPOSITORY"],
        os.environ["GITHUB_SHA"],
        json.loads(Path(".release-please-manifest.json").read_text()),
    )
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in outputs.items():
            output.write(f"{key}={value}\n")


if __name__ == "__main__":
    main()
