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


with tempfile.TemporaryDirectory(prefix="ops-managed-") as directory:
    directory = Path(directory)
    work = directory / "files"
    work.mkdir()
    source = work / "sample.xyz"
    source.write_text("".join(f"{x/10} {y/10} {z/10}\n" for x in range(10) for y in range(10) for z in range(10)))
    env = {**os.environ, "XDG_CONFIG_HOME": str(directory / "config"),
           "XDG_CACHE_HOME": str(directory / "cache"), "OPENAEC_BEHEERD": "1",
           "OPENAEC_MAP": str(work), "OPENAEC_TAAL": "en", "OPENAEC_THEMA": "dark",
           "OPENAEC_MCP_POORT": str(port), "OPENAEC_MCP_GEHEIM": secret,
           "OPENAEC_MCP_ADRES": "127.0.0.1"}
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
            assert tool("stand")["result"]["clouds"] == []
            tool("screenshot", path=str((args.output / "managed-empty.png").resolve()), window=True)
            idle_rss_kib = None
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
            exported = tool("opslaan", path=str(destination))
            tool("wait_for_job", id=exported["job_id"], timeout_seconds=60)
            assert destination.is_file() and destination.stat().st_size > 1000
            tool("screenshot", path=str((args.output / "managed-window.png").resolve()), window=True)
            assert (args.output / "managed-window.png").stat().st_size > 1000
            assert request("DELETE", "/mcp")[0] == 200
            assert request("POST", "/mcp", {"jsonrpc": "2.0", "id": 999, "method": "ping"})[0] == 404
            window.send_signal(signal.SIGTERM)
            assert window.wait(timeout=30) == 0
            assert request("GET", "/gezond")[0] == 503
            report = {"ok": True, "server": initialized["serverInfo"], "tool_count": len(definitions),
                      "open_save_points": 1000, "idle_rss_kib": idle_rss_kib, "unauthenticated_status": 401, "origin_status": 403,
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
