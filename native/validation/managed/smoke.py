#!/usr/bin/env python3
"""Exercise a managed native window and its real desktop MCP HTTP transport."""
import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=Path)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--paused-access", action="store_true", help="Prove paused MCP does not make a healthy GUI fail its health check")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
binary = str(args.binary.resolve())

def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]

api_port, port = free_port(), free_port()
assert port != api_port
secret = secrets.token_hex(32)
sequence = 0
session = None
base = f"http://127.0.0.1:{port}"

def request(method, path, data=None, *, authorization=True, origin=False):
    headers = {"Content-Type": "application/json"}
    if authorization:
        headers["Authorization"] = "Bearer " + secret
    if origin:
        headers["Origin"] = "https://example.invalid"
    if session:
        headers["Mcp-Session-Id"] = session
        headers["MCP-Protocol-Version"] = "2025-11-25"
    body = None if data is None else json.dumps(data).encode()
    req = urllib.request.Request(base + path, body, headers, method=method)
    try:
        response = urllib.request.urlopen(req, timeout=90)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        content = response.read()
        return response.status, response.headers, json.loads(content) if content else None


def rpc(method, params=None):
    global sequence, session
    sequence += 1
    code, headers, reply = request("POST", "/mcp", {
        "jsonrpc": "2.0", "id": sequence, "method": method, "params": params or {},
    })
    assert code == 200 and "error" not in reply, (code, reply)
    session = headers.get("Mcp-Session-Id", session)
    return reply["result"]


def tool(name, **arguments):
    deadline = time.monotonic() + 90
    while True:
        reply = rpc("tools/call", {"name": "pointcloud_" + name, "arguments": arguments})
        if not reply.get("isError"):
            break
        # Retry only read-only probes: a CPU renderer can exceed the desktop
        # command timeout while compiling its first screenshot's pipelines.
        assert name in {"stand", "screenshot"} and "HTTP 504" in str(reply) and time.monotonic() < deadline, reply
        time.sleep(.2)
    if "structuredContent" in reply:
        return reply["structuredContent"]
    return json.loads(next(item["text"] for item in reply["content"] if item["type"] == "text"))


def save_to(path, expected_points):
    submitted = tool("opslaan", path=str(path))
    reply = tool("wait_for_job", id=submitted["job_id"], timeout_seconds=60)
    assert not reply["timed_out"], reply
    job = reply["job"]
    assert job["state"] == "complete" and job["path"] == str(path), job
    assert job["points"] == expected_points, job
    return job["points"]


def xyz_coordinates(path):
    rows = [tuple(map(float, line.split())) for line in path.read_text().splitlines()]
    assert all(len(row) == 3 for row in rows)
    assert len(rows) == len(set(rows)), "duplicate coordinates in the synthetic grid"
    return set(rows)


def reopen_version(path, source, readback, expected):
    tool("open", path=str(path))
    idle = tool("wait_until_idle", timeout_seconds=60)
    assert idle["idle"], idle
    layers = tool("stand")["result"]["clouds"]
    assert len(layers) == 2 and {layer["path"] for layer in layers} == {str(source), str(path)}, layers
    layer = next(layer for layer in layers if layer["path"] == str(path))
    assert layer["points"] == layer["remaining"] == len(expected), layer
    tool("set_active", index=layer["index"])
    count = save_to(readback, len(expected))
    assert xyz_coordinates(readback) == expected
    tool("remove", index=layer["index"])
    assert [layer["path"] for layer in tool("stand")["result"]["clouds"]] == [str(source)]
    return count


with tempfile.TemporaryDirectory(prefix="ops-managed-") as directory:
    directory = Path(directory)
    work = directory / "files"
    work.mkdir()
    source = work / "sample.xyz"
    source.write_text("".join(f"{x/10} {y/10} {z/10}\n" for x in range(10) for y in range(10) for z in range(10)))
    source_bytes = source.read_bytes()
    env = {**os.environ, "XDG_CONFIG_HOME": str(directory / "config"),
           "XDG_CACHE_HOME": str(directory / "cache"), "OPENAEC_BEHEERD": "1",
           "OPENAEC_MAP": str(work), "OPENAEC_TAAL": "en", "OPENAEC_THEMA": "dark",
           "OPENAEC_MCP_POORT": str(port), "OPENAEC_MCP_GEHEIM": secret,
           "OPENAEC_MCP_ADRES": "127.0.0.1"}
    if args.paused_access:
        settings = directory / "config/open-pointcloud-studio-native/settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({"mcp_enabled": False}))
    window = bridge = None
    with (args.output / "window.log").open("w") as window_log, (args.output / "http.log").open("w") as http_log:
        try:
            window = subprocess.Popen([binary, "--api-port", str(api_port)], env=env, stdout=window_log, stderr=window_log)
            bridge = subprocess.Popen([binary, "--mcp-http"], env=env, stdout=http_log, stderr=http_log)
            deadline = time.monotonic() + 90
            while True:
                assert window.poll() is None and bridge.poll() is None, "test process exited"
                try:
                    if request("GET", "/gezond")[0] == 200:
                        break
                except OSError:
                    pass
                assert time.monotonic() < deadline, "managed window did not become healthy"
                time.sleep(.2)
            assert request("GET", "/gezond", authorization=False)[0] == 401
            assert request("GET", "/gezond", origin=True)[0] == 403
            initialized = rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "managed-smoke", "version": "1"}})
            assert request("POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"})[0] == 202
            definitions = rpc("tools/list")["tools"]
            names = {item["name"] for item in definitions}
            assert {"pointcloud_stand", "pointcloud_open", "pointcloud_opslaan", "pointcloud_registration_refine"} <= names
            assert all(name.startswith("pointcloud_") for name in names)
            idle_rss_kib = None
            open_save_points = 0
            staged_points = []
            reopened_points = []
            if args.paused_access:
                blocked = rpc("tools/call", {"name": "pointcloud_stand", "arguments": {}})
                assert blocked.get("isError") and "paused" in str(blocked), blocked
                assert request("GET", "/gezond")[0] == 200, "Pausing MCP must not make a healthy GUI look crashed"
            else:
                assert tool("stand")["result"]["clouds"] == []
                tool("screenshot", path=str((args.output / "managed-empty.png").resolve()), window=True)
                process_status = Path(f"/proc/{window.pid}/status")
                if process_status.is_file():
                    for line in process_status.read_text().splitlines():
                        if line.startswith("VmRSS:"):
                            idle_rss_kib = int(line.split()[1])
                tool("open", path=str(source))
                tool("wait_until_idle", timeout_seconds=60)
                status = tool("stand")["result"]
                assert status["managed"] == {"enabled": True, "directory": str(work)}
                assert status["language"] == "en"
                assert status["clouds"][0]["points"] == 1000
                destination = work / "saved.ply"
                open_save_points = save_to(destination, 1000)
                assert destination.is_file() and destination.stat().st_size > 1000
                # The host can collect each completed version from this working path.
                original = xyz_coordinates(source)
                staged = work / "next-version.xyz"
                tool("thin", percent=50)
                tool("wait_until_idle", timeout_seconds=60)
                staged_points.append(save_to(staged, 500))
                edited_bytes = staged.read_bytes()
                edited = xyz_coordinates(staged)
                assert len(edited) == 500 and edited < original
                tool("undo_delete")
                staged_points.append(save_to(staged, 1000))
                assert xyz_coordinates(staged) == original
                # Deliver successive versions at one working path while the
                # original layer with the same basename stays in the window.
                mounted = work / "docs" / source.name
                mounted.parent.mkdir()
                for version, expected in [(edited_bytes, edited), (staged.read_bytes(), original)]:
                    mounted.write_bytes(version)
                    reopened_points.append(reopen_version(mounted, source, work / "readback.xyz", expected))
                    assert mounted.read_bytes() == version
                assert source.read_bytes() == source_bytes
                tool("screenshot", path=str((args.output / "managed-window.png").resolve()), window=True)
                assert (args.output / "managed-window.png").stat().st_size > 1000
            assert request("DELETE", "/mcp")[0] == 200
            assert request("POST", "/mcp", {"jsonrpc": "2.0", "id": 999, "method": "ping"})[0] == 404
            window.send_signal(signal.SIGTERM)
            assert window.wait(timeout=30) == 0
            assert request("GET", "/gezond")[0] == 503
            report = {"ok": True, "server": initialized["serverInfo"], "tool_count": len(definitions),
                      "open_save_points": open_save_points, "staged_version_points": staged_points,
                      "reopened_version_points": reopened_points, "mcp_paused": args.paused_access, "idle_rss_kib": idle_rss_kib, "unauthenticated_status": 401, "origin_status": 403,
                      "health_after_window_exit": 503, "sigterm_exit": 0}
            (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))
        finally:
            for process in [window, bridge]:
                if process and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
