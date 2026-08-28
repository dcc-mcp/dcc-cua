import json
import socket
import subprocess
import sys
import tempfile
import threading
from pathlib import Path


def serve(listener: socket.socket) -> None:
    for tick in (2, 1):
        connection, _ = listener.accept()
        with connection:
            request = b""
            while b"\r\n\r\n" not in request:
                chunk = connection.recv(4096)
                if not chunk:
                    break
                request += chunk
            body = json.dumps({"schemaVersion": "2.2.0", "tickId": tick}).encode()
            response = (
                b"HTTP/1.1 200 OK\r\n"
                b"Content-Type: application/json\r\n"
                + f'ETag: "tick-{tick}"\r\n'.encode()
                + f"Content-Length: {len(body)}\r\n".encode()
                + b"Connection: close\r\n\r\n"
                + body
            )
            connection.sendall(response)
    listener.close()


def main() -> None:
    binary = Path(sys.argv[1]).resolve()
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    server = threading.Thread(target=serve, args=(listener,), daemon=True)
    server.start()

    with tempfile.TemporaryDirectory(prefix="dcc-cua-profile-watch-") as directory:
        profile_path = Path(directory) / "profile.json"
        profile_path.write_text(
            json.dumps(
                {
                    "schema_version": 3,
                    "id": "profile-watch-boundary",
                    "profile_version": "1.0.0",
                    "application": {"family": "fixture", "versions": []},
                    "display_name": "Profile Watch Boundary",
                    "selectors": [{"application_names": ["fixture.exe"]}],
                    "surfaces": [],
                    "state_sources": [
                        {
                            "id": "fixture-state",
                            "type": "loopback_http_json",
                            "mode": "read_only",
                            "url": f"http://127.0.0.1:{listener.getsockname()[1]}/state",
                            "expected_schema_version": "2.2.0",
                            "schema_version_pointer": "/schemaVersion",
                            "tick_pointer": "/tickId",
                            "use_etag": True,
                            "timeout_ms": 1000,
                            "max_response_bytes": 1048576,
                            "optional": False,
                        }
                    ],
                    "settings": {
                        "dialog_style": "application_rendered",
                        "preferred_route": "visual_fallback",
                    },
                }
            ),
            encoding="utf-8",
        )
        completed = subprocess.run(
            [
                str(binary),
                "profile-state",
                "--profile-file",
                str(profile_path),
                "--watch",
                "--poll-ms",
                "50",
            ],
            capture_output=True,
            timeout=10,
            check=False,
        )

    server.join(timeout=2)
    if server.is_alive():
        raise AssertionError("profile-state watch fixture server did not stop")
    stdout = completed.stdout.decode("utf-8")
    stderr = completed.stderr.decode("utf-8")
    lines = [json.loads(line) for line in stdout.splitlines() if line]
    result = {
        "exit_code": completed.returncode,
        "stderr": stderr,
        "line_count": len(lines),
        "lines": lines,
    }
    if completed.returncode != 1:
        raise AssertionError("profile-state watch failure must exit 1")
    if stderr:
        raise AssertionError("profile-state watch failure leaked stderr")
    if len(lines) != 1:
        raise AssertionError("profile-state watch failure appended a protocol trailer")
    event = lines[0]
    if (
        event.get("success") is not True
        or event.get("profile_id") != "profile-watch-boundary"
        or event.get("source", {}).get("status") != "changed"
        or event.get("source", {}).get("tick") != 2
        or "error" in event
    ):
        raise AssertionError("profile-state watch did not retain its native event schema")

    single_read = subprocess.run(
        [str(binary), "profile-state", "--profile-file", str(profile_path)],
        capture_output=True,
        timeout=10,
        check=False,
    )
    single_stderr = single_read.stderr.decode("utf-8")
    single_lines = [
        json.loads(line)
        for line in single_read.stdout.decode("utf-8").splitlines()
        if line
    ]
    result["single_read"] = {
        "exit_code": single_read.returncode,
        "stderr": single_stderr,
        "lines": single_lines,
    }
    print(json.dumps(result, sort_keys=True))
    if single_read.returncode != 1 or single_stderr or len(single_lines) != 1:
        raise AssertionError("profile-state single read lost its one-shot failure contract")
    envelope = single_lines[0]
    if (
        envelope.get("success") is not False
        or envelope.get("error", {}).get("code") != "command_failed"
    ):
        raise AssertionError("profile-state single read did not retain its one-shot envelope")


if __name__ == "__main__":
    main()
