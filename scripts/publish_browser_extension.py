#!/usr/bin/env python3
"""Publish a verified extension archive without accepting secrets as CLI arguments."""

from __future__ import annotations

import argparse
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any


CHROME_API = "https://chromewebstore.googleapis.com"
EDGE_API = "https://api.addons.microsoftedge.microsoft.com"
POLL_SECONDS = 5
CHROME_UPLOAD_STATES = {"SUCCEEDED", "IN_PROGRESS", "FAILED", "NOT_FOUND"}
CHROME_ITEM_STATES = {
    "PENDING_REVIEW",
    "STAGED",
    "PUBLISHED",
    "PUBLISHED_TO_TESTERS",
    "REJECTED",
    "CANCELLED",
}
EDGE_OPERATION_STATES = {"InProgress", "Succeeded", "Failed"}


class PublishError(RuntimeError):
    """A redacted browser-store publishing failure."""


class HttpResponse:
    def __init__(self, status: int, headers: Mapping[str, str], body: bytes):
        self.status = status
        self.headers = headers
        self.body = body


Transport = Callable[[str, str, Mapping[str, str], bytes | None], HttpResponse]


def _default_transport(
    method: str,
    url: str,
    headers: Mapping[str, str],
    body: bytes | None,
) -> HttpResponse:
    request = urllib.request.Request(url, data=body, headers=dict(headers), method=method)
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return HttpResponse(response.status, dict(response.headers.items()), response.read())
    except urllib.error.HTTPError as error:
        return HttpResponse(error.code, dict(error.headers.items()), error.read())
    except urllib.error.URLError as error:
        raise PublishError("browser store transport failed") from error


def _json(response: HttpResponse, store: str, expected: Sequence[int]) -> dict[str, Any]:
    if response.status not in expected:
        raise PublishError(f"{store} API request failed with HTTP {response.status}")
    if not response.body:
        return {}
    try:
        value = json.loads(response.body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PublishError(f"{store} API returned invalid JSON") from error
    if not isinstance(value, dict):
        raise PublishError(f"{store} API returned an invalid response shape")
    return value


def _header(response: HttpResponse, name: str, store: str) -> str:
    value = next(
        (value for key, value in response.headers.items() if key.lower() == name.lower()),
        "",
    ).strip()
    if not value:
        raise PublishError(f"{store} API response omitted {name}")
    return value


def read_required_environment(
    names: Sequence[str], environ: Mapping[str, str] | None = None
) -> dict[str, str]:
    source = os.environ if environ is None else environ
    missing = [name for name in names if not source.get(name, "").strip()]
    if missing:
        raise PublishError(f"missing required environment: {', '.join(missing)}")
    return {name: source[name].strip() for name in names}


def validate_artifact(path: Path, expected_version: str) -> str:
    if not path.is_file():
        raise PublishError("extension artifact is missing")
    try:
        with zipfile.ZipFile(path) as archive:
            manifest = json.loads(archive.read("manifest.json"))
    except (KeyError, OSError, UnicodeDecodeError, json.JSONDecodeError, zipfile.BadZipFile) as error:
        raise PublishError("extension artifact is not a valid store package") from error
    if not isinstance(manifest, dict) or manifest.get("manifest_version") != 3:
        raise PublishError("extension artifact must contain a Manifest V3 manifest")
    version = manifest.get("version")
    if version != expected_version:
        raise PublishError(
            f"extension artifact version does not match expected version {expected_version}"
        )
    return str(version)


def _chrome_upload_state(
    data: Mapping[str, Any],
    *,
    status_response: bool,
) -> str:
    key = "lastAsyncUploadState" if status_response else "uploadState"
    state = data.get(key)
    aliases = {
        "UPLOAD_SUCCEEDED": "SUCCEEDED",
        "UPLOAD_IN_PROGRESS": "IN_PROGRESS",
        "UPLOAD_FAILED": "FAILED",
    }
    normalized = aliases.get(str(state), str(state))
    return normalized if normalized in CHROME_UPLOAD_STATES else "UNKNOWN"


def publish_chrome(
    artifact: Path,
    *,
    access_token: str,
    publisher_id: str,
    extension_id: str,
    transport: Transport = _default_transport,
    sleeper: Callable[[float], None] = time.sleep,
    poll_limit: int = 60,
) -> dict[str, str]:
    if poll_limit < 1:
        raise PublishError("poll limit must be positive")
    name = (
        f"publishers/{urllib.parse.quote(publisher_id, safe='')}/items/"
        f"{urllib.parse.quote(extension_id, safe='')}"
    )
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/zip",
    }
    upload = transport(
        "POST",
        f"{CHROME_API}/upload/v2/{name}:upload",
        headers,
        artifact.read_bytes(),
    )
    state = _chrome_upload_state(_json(upload, "chrome", (200,)), status_response=False)
    if state == "IN_PROGRESS":
        status_headers = {"Authorization": f"Bearer {access_token}"}
        for attempt in range(poll_limit):
            status = transport(
                "GET", f"{CHROME_API}/v2/{name}:fetchStatus", status_headers, None
            )
            state = _chrome_upload_state(
                _json(status, "chrome", (200,)), status_response=True
            )
            if state != "IN_PROGRESS":
                break
            if attempt + 1 < poll_limit:
                sleeper(POLL_SECONDS)
    if state != "SUCCEEDED":
        raise PublishError(f"chrome upload did not succeed: {state}")

    publish_headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
    }
    publish = transport(
        "POST",
        f"{CHROME_API}/v2/{name}:publish",
        publish_headers,
        json.dumps({"blockOnWarnings": True}, separators=(",", ":")).encode(),
    )
    raw_submission = str(_json(publish, "chrome", (200,)).get("state", "UNKNOWN"))
    submission = raw_submission if raw_submission in CHROME_ITEM_STATES else "UNKNOWN"
    if submission == "UNKNOWN":
        raise PublishError("chrome publish response returned an unknown submission state")
    if submission in {"REJECTED", "CANCELLED"}:
        raise PublishError(f"chrome submission did not succeed: {submission}")
    return {"store": "chrome", "upload": state, "submission": submission}


def _edge_operation_id(response: HttpResponse) -> str:
    location = _header(response, "Location", "edge")
    operation_id = location.rstrip("/").rsplit("/", 1)[-1]
    if not operation_id:
        raise PublishError("edge API returned an invalid operation identifier")
    return urllib.parse.quote(operation_id, safe="")


def _wait_edge(
    url: str,
    headers: Mapping[str, str],
    transport: Transport,
    sleeper: Callable[[float], None],
    poll_limit: int,
) -> str:
    if poll_limit < 1:
        raise PublishError("poll limit must be positive")
    state = "InProgress"
    for attempt in range(poll_limit):
        response = transport("GET", url, headers, None)
        raw_state = str(_json(response, "edge", (200,)).get("status", "Unknown"))
        state = raw_state if raw_state in EDGE_OPERATION_STATES else "Unknown"
        if state != "InProgress":
            break
        if attempt + 1 < poll_limit:
            sleeper(POLL_SECONDS)
    if state != "Succeeded":
        raise PublishError(f"edge operation did not succeed: {state}")
    return state


def publish_edge(
    artifact: Path,
    *,
    api_key: str,
    client_id: str,
    product_id: str,
    transport: Transport = _default_transport,
    sleeper: Callable[[float], None] = time.sleep,
    poll_limit: int = 60,
) -> dict[str, str]:
    product = urllib.parse.quote(product_id, safe="")
    base = f"{EDGE_API}/v1/products/{product}/submissions"
    headers = {
        "Authorization": f"ApiKey {api_key}",
        "X-ClientID": client_id,
        "Content-Type": "application/zip",
    }
    upload = transport(
        "POST", f"{base}/draft/package", headers, artifact.read_bytes()
    )
    _json(upload, "edge", (202,))
    upload_operation = _edge_operation_id(upload)
    upload_state = _wait_edge(
        f"{base}/draft/package/operations/{upload_operation}",
        headers,
        transport,
        sleeper,
        poll_limit,
    )

    publish_headers = {
        "Authorization": f"ApiKey {api_key}",
        "X-ClientID": client_id,
        "Content-Type": "text/plain",
    }
    publish = transport(
        "POST",
        base,
        publish_headers,
        b"DCC-CUA browser extension automated release",
    )
    _json(publish, "edge", (202,))
    publish_operation = _edge_operation_id(publish)
    publish_state = _wait_edge(
        f"{base}/operations/{publish_operation}",
        publish_headers,
        transport,
        sleeper,
        poll_limit,
    )
    return {"store": "edge", "upload": upload_state, "submission": publish_state}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", choices=("chrome", "edge"), required=True)
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--poll-limit", type=int, default=60)
    args = parser.parse_args()
    artifact = args.artifact.resolve()
    validate_artifact(artifact, args.expected_version)

    if args.store == "chrome":
        values = read_required_environment(
            (
                "CHROME_WEBSTORE_ACCESS_TOKEN",
                "CHROME_WEBSTORE_PUBLISHER_ID",
                "CHROME_WEBSTORE_EXTENSION_ID",
            )
        )
        result = publish_chrome(
            artifact,
            access_token=values["CHROME_WEBSTORE_ACCESS_TOKEN"],
            publisher_id=values["CHROME_WEBSTORE_PUBLISHER_ID"],
            extension_id=values["CHROME_WEBSTORE_EXTENSION_ID"],
            poll_limit=args.poll_limit,
        )
    else:
        values = read_required_environment(
            ("EDGE_ADDONS_API_KEY", "EDGE_ADDONS_CLIENT_ID", "EDGE_ADDONS_PRODUCT_ID")
        )
        result = publish_edge(
            artifact,
            api_key=values["EDGE_ADDONS_API_KEY"],
            client_id=values["EDGE_ADDONS_CLIENT_ID"],
            product_id=values["EDGE_ADDONS_PRODUCT_ID"],
            poll_limit=args.poll_limit,
        )
    print(json.dumps(result, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
