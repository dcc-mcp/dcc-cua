"""Real release CLI pixels against our own windows on a private Xvfb display.

Run only as: xvfb-run -a --server-args='-screen 0 640x480x24 -nolisten tcp'
    python -B scripts/test_linux_pixels_x11.py --binary target/release/dcc-cua
No window manager, compositor, AT-SPI, input synthesis, or real desktop required.
"""
import argparse
from contextlib import contextmanager
import ctypes as C
import ctypes.util
import json
import os
from pathlib import Path
import select
import struct
import subprocess
import tempfile
import time
import zlib


def fixture_reply(process, timeout=10):
    deadline = time.monotonic() + timeout
    line = bytearray()
    while len(line) < 4096:
        remaining = deadline - time.monotonic()
        assert remaining > 0, "native fixture response timed out"
        readable, _, _ = select.select([process.stdout], [], [], remaining)
        assert readable, "native fixture response timed out"
        byte = os.read(process.stdout.fileno(), 1)
        assert byte, "native fixture exited before its response"
        if byte == b"\n":
            return json.loads(line)
        line.extend(byte)
    raise AssertionError("native fixture response exceeded its bound")


def fixture_command(process, command):
    assert command in {"blue", "cover", "uncover", "hide", "destroy"}
    process.stdin.write(command + "\n")
    process.stdin.flush()
    assert fixture_reply(process) == {"ok": command}


@contextmanager
def native_fixture():
    with tempfile.TemporaryDirectory(prefix="dcc-cua-x11-native-") as directory:
        binary = Path(directory) / "dcc-cua-x11-fixture"
        source = Path(__file__).with_name("fixtures") / "linux_x11_window.c"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-O2",
                        str(source), "-lX11", "-o", str(binary)], check=True, timeout=30)
        process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, text=True)
        try:
            ready = fixture_reply(process)
            assert ready["pid"] == process.pid, "fixture must advertise its own PID"
            assert Path(f"/proc/{process.pid}/exe").resolve() == binary.resolve(), \
                "fixture must have a genuine native executable identity"
            assert ready["target"] > 0 and ready["sibling"] > 0
            assert ready["target"] != ready["sibling"]
            yield process, ready
        finally:
            try:
                process.stdin.close()
            except OSError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            process.stdout.close()
        assert process.returncode == 0, "native fixture failed"


def png_rgb(data):
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    offset, compressed = 8, bytearray()
    while offset < len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        kind = data[offset + 4:offset + 8]
        chunk = data[offset + 8:offset + 8 + length]
        assert zlib.crc32(kind + chunk) == struct.unpack_from(">I", data, offset + 8 + length)[0]
        if kind == b"IHDR":
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", chunk)
            assert (depth, color, compression, filtering, interlace) == (8, 2, 0, 0, 0)
        elif kind == b"IDAT":
            compressed.extend(chunk)
        offset += 12 + length
    raw = zlib.decompress(compressed)
    stride = width * 3
    assert len(raw) == height * (stride + 1)
    pixels, previous = bytearray(), bytearray(stride)
    for row in range(height):
        kind = raw[row * (stride + 1)]
        scan = bytearray(raw[row * (stride + 1) + 1:(row + 1) * (stride + 1)])
        for x in range(stride):
            left, above, upper_left = scan[x - 3] if x >= 3 else 0, previous[x], previous[x - 3] if x >= 3 else 0
            if kind == 0:
                predictor = 0
            elif kind == 1:
                predictor = left
            elif kind == 2:
                predictor = above
            elif kind == 3:
                predictor = (left + above) // 2
            elif kind == 4:
                estimate = left + above - upper_left
                distances = [abs(estimate - left), abs(estimate - above), abs(estimate - upper_left)]
                predictor = [left, above, upper_left][distances.index(min(distances))]
            else:
                raise AssertionError(f"invalid PNG filter {kind}")
            scan[x] = (scan[x] + predictor) & 255
        pixels.extend(scan)
        previous = scan
    return width, height, bytes(pixels)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    assert os.environ.get("DISPLAY"), "CI must supply private Xvfb; no silent skip"
    lib = C.CDLL(ctypes.util.find_library("X11"))
    def api(name, result, arguments):
        fn = getattr(lib, name)
        fn.restype, fn.argtypes = result, arguments
        return fn
    display = api("XOpenDisplay", C.c_void_p, [C.c_char_p])(None)
    assert display, "XOpenDisplay failed"
    root = api("XDefaultRootWindow", C.c_ulong, [C.c_void_p])(display)
    create = api("XCreateSimpleWindow", C.c_ulong, [C.c_void_p, C.c_ulong, C.c_int, C.c_int, C.c_uint, C.c_uint, C.c_uint, C.c_ulong, C.c_ulong])
    atom = api("XInternAtom", C.c_ulong, [C.c_void_p, C.c_char_p, C.c_int])
    prop = api("XChangeProperty", C.c_int, [C.c_void_p, C.c_ulong, C.c_ulong, C.c_ulong, C.c_int, C.c_int, C.c_void_p, C.c_int])
    map_window = api("XMapWindow", C.c_int, [C.c_void_p, C.c_ulong])
    sync = api("XSync", C.c_int, [C.c_void_p, C.c_int])
    def fixture(x, y, color):
        window = create(display, root, x, y, 80, 60, 0, 0, color)
        pid = C.c_ulong(os.getpid())
        prop(display, window, atom(display, b"_NET_WM_PID", 0), 6, 32, 0, C.byref(pid), 1)
        map_window(display, window)
        sync(display, False)
        return window
    results = []
    try:
        with native_fixture() as (native, identity), \
                tempfile.TemporaryDirectory(prefix="dcc-cua-x11-") as directory:
            target, fixture_pid = identity["target"], identity["pid"]
            python_target = fixture(400, 200, 0xff00ff)
            directory = Path(directory)
            env = dict(os.environ, HOME=str(directory), XDG_DATA_HOME=str(directory / "data"),
                       XDG_CONFIG_HOME=str(directory / "config"), XDG_CACHE_HOME=str(directory / "cache"))
            env.pop("WAYLAND_DISPLAY", None)
            env["XDG_SESSION_TYPE"] = "x11"
            def run(name, expected=True, pid=None, window=None, extra=(), overrides=None,
                    error_code=None):
                selected_pid = fixture_pid if pid is None else pid
                selected_window = target if window is None else window
                output = directory / f"{name}.png"
                result = subprocess.run([binary, "snapshot", "--pid", str(selected_pid),
                    "--window-id", str(selected_window), "--pixels-only", "--output", str(output), *extra],
                    env=env | (overrides or {}), capture_output=True, text=True, timeout=15, check=False)
                receipt = json.loads(result.stdout)
                assert (result.returncode == 0) == expected, (name, result.returncode, receipt, result.stderr)
                assert output.exists() == expected, (name, "unexpected screenshot publication")
                if expected:
                    observation = receipt["observation"]
                    assert (observation["process_id"], observation["window_handle"]) == (selected_pid, selected_window)
                    assert receipt["accessibility"]["accessibility_available"] is False
                    assert observation["capture_provenance"]["read_only"] is True
                else:
                    assert receipt["success"] is False
                    if error_code is not None:
                        assert receipt["error"]["code"] == error_code, receipt
                results.append({"case": name, "passed": True, "exit_code": result.returncode})
                return output
            first = run("exact_red_window")
            assert png_rgb(first.read_bytes()) == (80, 60, bytes([255, 0, 0]) * (80 * 60))
            fixture_command(native, "blue")
            second = run("fresh_blue_pixels")
            assert png_rgb(second.read_bytes()) == (80, 60, bytes([0, 0, 255]) * (80 * 60))
            run("python_interpreter_refused", False, pid=os.getpid(), window=python_target,
                error_code="invalid_target")
            with native_fixture() as (_, other_identity):
                assert other_identity["pid"] != fixture_pid
                run("wrong_pid", False, pid=other_identity["pid"], error_code="invalid_target")
            run("activation_refused", False, extra=("--activate",))
            run("wayland_refused", False, overrides={"WAYLAND_DISPLAY": "wayland-test"})
            run("remote_display_refused", False, overrides={"DISPLAY": "example.invalid:0"})
            run("invalid_screen_refused", False, overrides={"DISPLAY": os.environ["DISPLAY"].split(".")[0] + ".9999"})
            fixture_command(native, "cover")
            run("occluded_refused", False)
            fixture_command(native, "uncover")
            fixture_command(native, "hide")
            run("hidden_refused", False)
            fixture_command(native, "destroy")
            run("closed_xid_refused", False)
            assert all(path.suffix == ".png" and path.is_file() for path in directory.iterdir()), \
                "read-only CLI must not install themes or write desktop configuration"
        print(json.dumps({"fixture": "private synthetic X11 windows", "tests": results}, indent=2))
    finally:
        api("XCloseDisplay", C.c_int, [C.c_void_p])(display)


if __name__ == "__main__":
    main()
