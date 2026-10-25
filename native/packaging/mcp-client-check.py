#!/usr/bin/env python3
"""Checks an MCP server of Open Pointcloud Studio from the outside, with the
reference Python SDK of the protocol (the package `mcp`), and writes what it
found as a record.

    mcp-client-check.py --stdio BINARY [--version NUMBER]
    mcp-client-check.py --http URL --key KEY [--version NUMBER]

--stdio starts `BINARY --mcp`, the server of the desktop application, as a
client configured with a command does. It needs a display, a virtual one
will do, because the server starts a window: settings and caches go to a
temporary folder (XDG_CONFIG_HOME, XDG_CACHE_HOME), and the window is ended
by its process id afterwards.

  1. initialize: the protocol, the version of the server, and that it offers
     tools and no resources;
  2. tools/list: the count, the annotations of status (read-only), thin
     (destructive) and bag3d (open world), and that every input schema
     refuses unknown arguments;
  3. a window with a grid of 1000 points (start_instance, then status);
  4. thin 50 keeps 500 points, undo_delete brings back 1000, redo_delete
     500, undo_delete 1000 again;
  5. open of a missing path is a tool error;
  6. the window's local API answers on 127.0.0.1 with the token of its
     discovery file, and refuses a command without it.

--http connects to the route /mcp of the cloud API at URL, such as
http://127.0.0.1:8080/mcp, over Streamable HTTP:

  1. without the key the server refuses the handshake with 401;
  2. with KEY: initialize, and tools/list gives the five tools, read-only;
  3. inspect of the grid gives 1000 points in structuredContent;
  4. convert to LAZ gives the file as an embedded resource, and inspect of
     that file gives 1000 points;
  5. a path argument is a tool error with error_code ARGUMENT_ONGELDIG.

The record is Markdown on standard output, and is appended to the summary of
the job when GITHUB_STEP_SUMMARY is set. The exit code is 1 when any check
fails, 2 when the arguments are wrong.

It is written for the version of the SDK that mcp-client-requirements.txt
pins, installed into a virtual environment of its own:

    python3 -m venv "$RUNNER_TEMP/mcp"
    "$RUNNER_TEMP/mcp/bin/pip" install --require-hashes -r native/packaging/mcp-client-requirements.txt
"""

import argparse
import base64
import importlib.metadata
import json
import os
import shutil
import signal
import sys
import tempfile
import urllib.error
import urllib.request

import anyio
from mcp import ClientSession, StdioServerParameters, stdio_client
from mcp.client.streamable_http import streamable_http_client
from mcp.shared._httpx_utils import create_mcp_http_client
from mcp.types import Implementation

GRID = 10
POINTS = GRID**3
CLOUD_TOOLS = ["inspect", "convert", "section_drawing", "terrain_mesh", "surface_mesh"]
CLIENT = Implementation(name="mcp-client-check", version="1")
STEP_SECONDS = 120


def grid_ply():
    """The grid of the package tests: 1000 coloured points, ASCII PLY."""
    lines = ["ply", "format ascii 1.0", f"element vertex {POINTS}"]
    lines += [f"property float {axis}" for axis in "xyz"]
    lines += [f"property uchar {channel}" for channel in ("red", "green", "blue")]
    lines.append("end_header")
    for x in range(GRID):
        for y in range(GRID):
            for z in range(GRID):
                lines.append("%.2f %.2f %.2f 255 0 255" % (x * 0.25, y * 0.25, z * 0.25))
    return ("\n".join(lines) + "\n").encode("ascii")


def wire(model):
    """A result of the SDK as the JSON on the wire, with camelCase keys."""
    return model.model_dump(by_alias=True, mode="json", exclude_none=True)


def text_json(result):
    """The JSON of the last text part of a tool result."""
    texts = [part["text"] for part in result.get("content", []) if part.get("type") == "text"]
    try:
        return json.loads(texts[-1])
    except (IndexError, ValueError):
        return None


class Record:
    """The facts of one run, and each check with its outcome."""

    def __init__(self, title):
        self.title = title
        self.facts = []
        self.checks = []

    def fact(self, name, value):
        self.facts.append((name, value))

    def check(self, passed, text):
        self.checks.append((bool(passed), text))
        print(("ok    " if passed else "WRONG ") + text, file=sys.stderr)
        return bool(passed)

    @property
    def failed(self):
        return [text for passed, text in self.checks if not passed]

    def markdown(self):
        cell = lambda value: str(value).replace("|", "\\|").replace("\n", " ")
        lines = [f"### {self.title}", "", "| | |", "| --- | --- |"]
        lines += [f"| {cell(name)} | {cell(value)} |" for name, value in self.facts]
        lines += ["", "| Check | Result |", "| --- | --- |"]
        lines += [f"| {cell(text)} | {'ok' if passed else 'wrong'} |" for passed, text in self.checks]
        passed = len(self.checks) - len(self.failed)
        lines += ["", f"{passed} of {len(self.checks)} checks passed.", ""]
        return "\n".join(lines)


def describe(error):
    """An exception as text; the errors inside a group of tasks one by one."""
    if isinstance(error, BaseExceptionGroup):
        return "; ".join(describe(inner) for inner in error.exceptions)
    return f"{type(error).__name__}: {error}"


def client_package():
    return f"mcp {importlib.metadata.version('mcp')} (the reference Python SDK of the protocol)"


async def call(session, name, **arguments):
    with anyio.fail_after(STEP_SECONDS):
        return wire(await session.call_tool(name, arguments))


# ---- --stdio ----------------------------------------------------------------


def local_api(config, pid, command, with_token=True):
    """Send a command to the window's local API: (status, answer)."""
    instance = os.path.join(config, "open-pointcloud-studio-native", "instances", f"instance-{pid}.json")
    with open(instance, encoding="utf-8") as file:
        discovery = json.load(file)
    headers = {"Content-Type": "application/json"}
    if with_token:
        headers["X-OPS-Token"] = discovery["token"]
    request = urllib.request.Request(
        f"http://127.0.0.1:{discovery['port']}/exec",
        data=json.dumps(command).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, None


async def wait_for_points(session, total, remaining, record):
    """Poll status until the window lists one layer with these counts."""
    clouds = None
    with anyio.move_on_after(60):
        while True:
            status = text_json(await call(session, "status")) or {}
            clouds = status.get("result", {}).get("clouds")
            if (
                clouds
                and len(clouds) == 1
                and clouds[0].get("points") == total
                and clouds[0].get("remaining", total) == remaining
            ):
                return True
            await anyio.sleep(0.5)
    record.fact("Layers last seen", json.dumps(clouds))
    return False


async def check_stdio(options, record, folder):
    grid = os.path.join(folder, "grid.ply")
    with open(grid, "wb") as file:
        file.write(grid_ply())
    config = os.path.join(folder, "config")
    environment = dict(os.environ)
    environment["XDG_CONFIG_HOME"] = config
    environment["XDG_CACHE_HOME"] = os.path.join(folder, "cache")
    server = StdioServerParameters(command=options.stdio, args=["--mcp"], env=environment)
    record.fact("Client", client_package())
    record.fact("Transport", f"stdio: `{options.stdio} --mcp`, JSON-RPC one message per line")
    window = None
    async with stdio_client(server) as (read, write):
        async with ClientSession(read, write, client_info=CLIENT) as session:
            try:
                with anyio.fail_after(STEP_SECONDS):
                    started = wire(await session.initialize())
                info = started.get("serverInfo", {})
                capabilities = started.get("capabilities", {})
                record.fact("Protocol", started.get("protocolVersion"))
                record.fact("Server", f"{info.get('name')} {info.get('version')}")
                record.fact(
                    "Capabilities",
                    ", ".join(sorted(capabilities)) + ("; resources: none" if "resources" not in capabilities else ""),
                )
                if options.version:
                    record.check(info.get("version") == options.version, f"the server reports version {options.version}")
                record.check(sorted(capabilities) == ["tools"], "the server offers tools and nothing else, no resources")

                with anyio.fail_after(STEP_SECONDS):
                    tools = [wire(tool) for tool in (await session.list_tools()).tools]
                by_name = {tool["name"]: tool for tool in tools}
                hints = lambda name: by_name.get(name, {}).get("annotations", {})
                record.fact("Tools", len(tools))
                record.check(hints("status").get("readOnlyHint") is True, "status is annotated read-only")
                record.check(hints("thin").get("destructiveHint") is True, "thin is annotated destructive")
                record.check(hints("bag3d").get("openWorldHint") is True, "bag3d is annotated open-world")
                open_schemas = [
                    tool["name"] for tool in tools if tool.get("inputSchema", {}).get("additionalProperties") is not False
                ]
                record.check(not open_schemas, f"every input schema refuses unknown arguments {open_schemas or ''}".strip())

                started = text_json(await call(session, "start_instance", files=[grid])) or {}
                window = started.get("started", {}).get("pid")
                record.fact("Window", f"process id {window}")
                record.check(
                    window and await wait_for_points(session, POINTS, POINTS, record),
                    "a window shows the grid as one layer of 1000 points",
                )

                steps = []
                for tool, arguments, remaining in (
                    ("thin", {"percent": 50}, 500),
                    ("undo_delete", {}, 1000),
                    ("redo_delete", {}, 500),
                    ("undo_delete", {}, 1000),
                ):
                    result = await call(session, tool, **arguments)
                    idle = text_json(await call(session, "wait_until_idle", timeout_seconds=60)) or {}
                    reached = not result.get("isError") and idle.get("idle") is not False
                    reached = reached and await wait_for_points(session, POINTS, remaining, record)
                    shown = tool + (f" {arguments['percent']}" if arguments else "")
                    steps.append(f"{shown}: {remaining}")
                    record.check(reached, f"{shown} leaves {remaining} of 1000 points")
                record.fact("Mutation and undo", "; ".join(steps))

                missing = os.path.join(folder, "missing", "nothing.las")
                failed = await call(session, "open", path=missing)
                record.check(failed.get("isError") is True, "open of a missing path is a tool error (isError)")

                status, answer = local_api(config, window, {"command": "status"})
                refused, _ = local_api(config, window, {"command": "status"}, with_token=False)
                record.fact(
                    "Permissions",
                    "tools drive the window through its local API on 127.0.0.1 with the token of the "
                    f"discovery file in the user's configuration folder (with it {status}, without it {refused}); "
                    "annotations readOnlyHint, destructiveHint, idempotentHint and openWorldHint",
                )
                record.check(status == 200 and answer and answer.get("ok"), "the local API answers a command with the token")
                record.check(refused == 403, "the local API refuses a command without the token")
            finally:
                if window:
                    try:
                        os.kill(window, signal.SIGTERM)
                    except OSError:
                        pass


# ---- --http -------------------------------------------------------------------


def raw_post(url, body, key=None, origin=None):
    """POST one JSON-RPC message without the SDK: (status, headers, answer)."""
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if key:
        headers["Authorization"] = f"Bearer {key}"
    if origin:
        headers["Origin"] = origin
    request = urllib.request.Request(url, data=json.dumps(body).encode("utf-8"), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, response.headers, json.loads(response.read())
    except urllib.error.HTTPError as error:
        try:
            answer = json.loads(error.read())
        except ValueError:
            answer = None
        return error.code, error.headers, answer


async def session_over_http(url, key, statuses):
    """The SDK's own HTTP client, with the key as a header and every status
    it receives noted in `statuses`."""
    client = create_mcp_http_client(headers={"Authorization": f"Bearer {key}"} if key else None)

    async def note(response):
        statuses.append(response.status_code)

    client.event_hooks = {"request": [], "response": [note]}
    return client


async def check_http(options, record, folder):
    record.fact("Client", client_package())
    record.fact("Transport", f"Streamable HTTP with JSON answers: `{options.http}`")

    # 1. Without the key.
    statuses = []
    refused = False
    client = await session_over_http(options.http, None, statuses)
    try:
        async with client:
            async with streamable_http_client(options.http, http_client=client) as streams:
                async with ClientSession(streams[0], streams[1], client_info=CLIENT) as session:
                    with anyio.fail_after(STEP_SECONDS):
                        await session.initialize()
    except Exception:  # noqa: BLE001 - any refusal counts; the status says which
        refused = True
    status, headers, answer = raw_post(
        options.http,
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "raw", "version": "1"}}},
    )
    code = (answer or {}).get("error_code")
    record.fact("Without the key", f"the SDK's handshake {'failed' if refused else 'succeeded'} after HTTP {statuses}; a plain POST got {status} {code}")
    record.check(refused and 401 in statuses, "without the key the handshake is refused with 401")
    record.check(
        status == 401 and code == "TOEGANG_GEWEIGERD" and (headers.get("WWW-Authenticate") or "").startswith("Bearer"),
        "the refusal is 401 TOEGANG_GEWEIGERD with WWW-Authenticate: Bearer",
    )

    # 2 to 5. With the key.
    statuses = []
    grid = {"format": "ply", "data": base64.b64encode(grid_ply()).decode("ascii"), "name": "grid.ply"}
    client = await session_over_http(options.http, options.key, statuses)
    async with client:
        async with streamable_http_client(options.http, http_client=client) as streams:
            async with ClientSession(streams[0], streams[1], client_info=CLIENT) as session:
                with anyio.fail_after(STEP_SECONDS):
                    started = wire(await session.initialize())
                info = started.get("serverInfo", {})
                capabilities = started.get("capabilities", {})
                record.fact("Protocol", started.get("protocolVersion"))
                record.fact("Server", f"{info.get('name')} {info.get('version')}")
                record.fact("Session", "none: the service is stateless and gives no session id")
                record.fact(
                    "Capabilities",
                    ", ".join(sorted(capabilities)) + ("; resources: none" if "resources" not in capabilities else ""),
                )
                status, _, answer = raw_post(
                    options.http, {"jsonrpc": "2.0", "id": 9, "method": "tools/list"},
                    key=options.key, origin="https://example.org",
                )
                foreign = f"{status} {(answer or {}).get('error_code')}"
                record.fact("Permissions", f"a bearer key on every request; a foreign Origin gets {foreign}; read-only tools")
                record.check(foreign == "403 HERKOMST_GEWEIGERD", "a request from a foreign Origin is refused with 403 HERKOMST_GEWEIGERD")
                if options.version:
                    record.check(info.get("version") == options.version, f"the server reports version {options.version}")
                record.check(sorted(capabilities) == ["tools"], "the server offers tools and nothing else, no resources")

                with anyio.fail_after(STEP_SECONDS):
                    tools = [wire(tool) for tool in (await session.list_tools()).tools]
                names = [tool["name"] for tool in tools]
                record.fact("Tools", ", ".join(names))
                record.check(names == CLOUD_TOOLS, "tools/list gives the five cloud tools in order")
                record.check(
                    all(tool.get("annotations", {}).get("readOnlyHint") is True for tool in tools)
                    and all(tool.get("annotations", {}).get("openWorldHint") is False for tool in tools),
                    "every tool is annotated read-only and not open-world",
                )

                inspected = await call(session, "inspect", input=grid)
                points = (inspected.get("structuredContent") or {}).get("report", {}).get("points")
                record.fact("inspect", f"{points} points")
                record.check(not inspected.get("isError") and points == POINTS, "inspect of the grid gives 1000 points in structuredContent")

                converted = await call(session, "convert", input=grid, output_format="laz")
                resources = [part["resource"] for part in converted.get("content", []) if part.get("type") == "resource"]
                output = (converted.get("structuredContent") or {}).get("output", {})
                blob = base64.b64decode(resources[0]["blob"]) if resources and "blob" in resources[0] else b""
                laz = {"format": "laz", "data": base64.b64encode(blob).decode("ascii")}
                again = await call(session, "inspect", input=laz) if blob else {}
                again_points = (again.get("structuredContent") or {}).get("report", {}).get("points")
                record.fact(
                    "convert",
                    f"{resources[0].get('uri') if resources else 'no resource'}, {len(blob)} bytes, {resources[0].get('mimeType') if resources else ''}; inspect of it {again_points} points",
                )
                record.check(
                    not converted.get("isError") and blob[:4] == b"LASF" and len(blob) == output.get("bytes") and "data" not in output,
                    "convert to LAZ gives the file once, as an embedded resource, and structuredContent without it",
                )
                record.check(again_points == POINTS, "inspect of the LAZ from the resource gives 1000 points")

                refused_path = await call(session, "inspect", input=grid, path="/etc/passwd")
                error_code = (refused_path.get("structuredContent") or {}).get("error_code")
                record.fact("A path argument", f"isError {refused_path.get('isError')}, {error_code}")
                record.check(
                    refused_path.get("isError") is True and error_code == "ARGUMENT_ONGELDIG",
                    "a path argument is a tool error with error_code ARGUMENT_ONGELDIG",
                )
    record.fact("HTTP statuses seen with the key", " ".join(str(status) for status in statuses))


def main():
    parser = argparse.ArgumentParser(description="Check an MCP server of Open Pointcloud Studio from the outside.")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--stdio", metavar="BINARY", help="the desktop application, started as BINARY --mcp")
    mode.add_argument("--http", metavar="URL", help="the /mcp route of the cloud API")
    parser.add_argument("--key", help="the bearer key of the cloud API (with --http)")
    parser.add_argument("--version", help="the version the server has to report")
    options = parser.parse_args()
    if options.http and not options.key:
        parser.error("--http needs --key")

    if options.stdio:
        options.stdio = os.path.abspath(options.stdio) if os.path.exists(options.stdio) else options.stdio
        record = Record("External MCP client: the desktop server over stdio")
        check = check_stdio
    else:
        record = Record("External MCP client: the cloud API over Streamable HTTP")
        check = check_http
    folder = tempfile.mkdtemp(prefix="ops-mcp-client-")
    try:
        anyio.run(check, options, record, folder)
    except Exception as error:  # noqa: BLE001 - recorded, and the run fails
        record.check(False, f"the run ended early: {describe(error)}")
    finally:
        shutil.rmtree(folder, ignore_errors=True)
    if not record.checks:
        record.check(False, "no check ran")

    text = record.markdown()
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as file:
            file.write(text + "\n")
    return 1 if record.failed else 0


if __name__ == "__main__":
    sys.exit(main())
