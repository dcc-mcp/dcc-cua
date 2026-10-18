"""Read draft metadata in the existing privileged release job.

Builders consume validated scalar bindings without receiving a write token.
No mutation, fallback, or retry is permitted on a metadata failure.
"""
import json
import os
from pathlib import Path
import re
import subprocess


def read_release(repository, tag):
    return json.loads(subprocess.check_output([
        "gh", "release", "view", tag, "--repo", repository, "--json",
        "tagName,targetCommitish,isDraft,isPrerelease",
    ]))


def resolve(environment, read=read_release):
    repository = environment["GITHUB_REPOSITORY"]
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise ValueError("invalid repository")
    outputs = {}
    for component, prefix in (("native", "v"), ("extension", "dcc-cua-browser-extension-v")):
        key = component.upper()
        active = environment[key + "_CREATED"]
        if active not in ("true", "false"):
            raise ValueError("release-created must be an explicit boolean")
        if active == "false":
            continue
        tag = environment[key + "_TAG"]
        source = environment[key + "_SOURCE"]
        if not re.fullmatch(re.escape(prefix) + r"\d+\.\d+\.\d+", tag):
            raise ValueError("invalid component tag")
        if not re.fullmatch(r"[0-9a-f]{40}", source):
            raise ValueError("release source must be a full commit SHA")
        metadata = read(repository, tag)
        if (metadata.get("tagName") != tag or metadata.get("targetCommitish") != source
                or type(metadata.get("isDraft")) is not bool
                or metadata.get("isPrerelease") is not False):
            raise ValueError("release metadata differs from the expected source/visibility")
        outputs[component + "_release_target"] = source
    return outputs


def main():
    # Resolve every active component before emitting any consumable outputs.
    outputs = resolve(os.environ)
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        for key, value in outputs.items():
            output.write(f"{key}={value}\n")


if __name__ == "__main__":
    main()
