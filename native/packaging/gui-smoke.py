#!/usr/bin/env python3
"""Window test of a packaged application, driven through its MCP server.

    gui-smoke.py BINARY [--version NUMBER] [--screenshot FILE.png]
                 [--open-with APPLICATION] [--timeout SECONDS]

Starts `BINARY --mcp`, lets it start a window with a generated grid of 1000
magenta points, waits until the window is idle and then checks that

  * the window lists one layer with 1000 points,
  * a screenshot of the 3D view shows points in the colour of the grid,
  * the window reports the expected version (with --version).

The window is ended afterwards. With --open-with the window is not started by
the server but by the macOS `open` command, which hands the file over the way
a double click in the file manager does; that is the only test of that path.

Needs a display (a virtual one will do) and a graphics driver, and nothing
but the Python standard library. Settings and caches go to a temporary folder.
"""

import argparse
import json
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

GRID = 10
POINTS = GRID**3
# A colour that neither the interface nor its themes use, so pixels in it can
# only be the points.
COLOUR = (255, 0, 255)


class Failure(Exception):
    pass


def write_grid(path):
    with open(path, "w", newline="\n") as ply:
        ply.write("ply\nformat ascii 1.0\n")
        ply.write(f"element vertex {POINTS}\n")
        for axis in "xyz":
            ply.write(f"property float {axis}\n")
        for channel in ("red", "green", "blue"):
            ply.write(f"property uchar {channel}\n")
        ply.write("end_header\n")
        for x in range(GRID):
            for y in range(GRID):
                for z in range(GRID):
                    ply.write("%.2f %.2f %.2f %d %d %d\n" % (x * 0.25, y * 0.25, z * 0.25, *COLOUR))


def read_png(path):
    """Width, height and the RGB pixels of an 8-bit RGB or RGBA PNG."""
    with open(path, "rb") as file:
        data = file.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise Failure(f"{path} is not a PNG file")
    position = 8
    header = None
    packed = b""
    while position < len(data):
        length, kind = struct.unpack(">I4s", data[position : position + 8])
        body = data[position + 8 : position + 8 + length]
        position += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            packed += body
        elif kind == b"IEND":
            break
    if header is None:
        raise Failure(f"{path} has no image header")
    width, height, depth, colour_type, _, _, interlace = header
    if depth != 8 or colour_type not in (2, 6) or interlace != 0:
        raise Failure(f"{path}: only 8-bit RGB and RGBA images without interlacing are read")
    channels = 3 if colour_type == 2 else 4
    stride = width * channels
    raw = zlib.decompress(packed)
    if len(raw) != height * (stride + 1):
        raise Failure(f"{path}: the image data has the wrong length")
    pixels = []
    previous = bytearray(stride)
    for row in range(height):
        start = row * (stride + 1)
        method = raw[start]
        line = bytearray(raw[start + 1 : start + 1 + stride])
        # Undo the per-line prediction of the PNG format.
        if method == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 255
        elif method == 2:
            for i in range(stride):
                line[i] = (line[i] + previous[i]) & 255
        elif method == 3:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + (left + previous[i]) // 2) & 255
        elif method == 4:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                above = previous[i]
                corner = previous[i - channels] if i >= channels else 0
                estimate = left + above - corner
                nearest = min((abs(estimate - left), 0, left), (abs(estimate - above), 1, above), (abs(estimate - corner), 2, corner))
                line[i] = (line[i] + nearest[2]) & 255
        elif method != 0:
            raise Failure(f"{path}: unknown line filter {method}")
        for i in range(0, stride, channels):
            pixels.append((line[i], line[i + 1], line[i + 2]))
        previous = line
    return width, height, pixels


def check_screenshot(path):
    if not os.path.isfile(path):
        raise Failure(f"the screenshot {path} was not written")
    width, height, pixels = read_png(path)
    colours = {}
    for pixel in pixels:
        colours[pixel] = colours.get(pixel, 0) + 1
    if len(colours) < 2:
        raise Failure(f"the screenshot is one flat colour {next(iter(colours))}")
    # Shading may darken the points, so near the colour of the grid counts.
    grid = sum(count for (red, green, blue), count in colours.items() if red > 150 and green < 110 and blue > 150)
    print(f"gui-smoke: screenshot {width} x {height}, {len(colours)} colours, {grid} pixels in the colour of the grid")
    if grid < 50:
        common = sorted(colours.items(), key=lambda item: -item[1])[:5]
        raise Failure(f"the screenshot shows {grid} pixels in the colour of the grid, expected at least 50; most frequent colours: {common}")


class Server:
    """A `BINARY --mcp` child process and the calls made to it."""

    def __init__(self, binary, environment):
        self.process = subprocess.Popen(
            [binary, "--mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            env=environment,
            encoding="utf-8",
        )
        self.next_id = 0

    def request(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            message["params"] = params
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise Failure(f"the MCP server ended while {method} was waiting for its answer")
            answer = json.loads(line)
            if answer.get("id") == self.next_id:
                break
        if "error" in answer:
            raise Failure(f"{method}: {answer['error']}")
        return answer["result"]

    def notify(self, method):
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.process.stdin.flush()

    def tool(self, name, **arguments):
        """Call a tool and return the JSON of its text part."""
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        texts = [part["text"] for part in result["content"] if part["type"] == "text"]
        if result.get("isError"):
            raise Failure(f"{name} failed: {' '.join(texts)}")
        try:
            return json.loads(texts[-1])
        except (IndexError, ValueError):
            raise Failure(f"{name} answered without JSON: {texts}") from None

    def close(self):
        try:
            self.process.stdin.close()
            self.process.wait(timeout=10)
        except (OSError, subprocess.TimeoutExpired):
            self.process.kill()


def end_process(pid):
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass


def run(options, folder):
    grid = os.path.join(folder, "grid.ply")
    write_grid(grid)
    environment = dict(os.environ)
    environment["XDG_CONFIG_HOME"] = os.path.join(folder, "config")
    environment["XDG_CACHE_HOME"] = os.path.join(folder, "cache")
    screenshot = os.path.abspath(options.screenshot or os.path.join(folder, "screenshot.png"))
    if os.path.exists(screenshot):
        os.remove(screenshot)

    server = Server(options.binary, environment)
    window = None
    # The MCP calls block, so the time limit ends the server from a second
    # thread; the call that is waiting then fails and the test with it.
    watchdog = threading.Timer(options.timeout, server.process.kill)
    watchdog.daemon = True
    watchdog.start()
    try:
        started = server.request(
            "initialize",
            {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "gui-smoke", "version": "1"}},
        )
        reported = started["serverInfo"]["version"]
        if options.version and reported != options.version:
            raise Failure(f"the MCP server reports version {reported}, expected {options.version}")
        server.notify("notifications/initialized")

        if options.open_with:
            # `open` does not pass the environment on, so name the folders.
            subprocess.run(
                ["open", "-n", "-a", options.open_with]
                + ["--env", f"XDG_CONFIG_HOME={environment['XDG_CONFIG_HOME']}"]
                + ["--env", f"XDG_CACHE_HOME={environment['XDG_CACHE_HOME']}"]
                + [grid],
                check=True,
            )
            deadline = time.monotonic() + 60
            while window is None:
                instances = server.tool("list_instances")["instances"]
                if instances:
                    window = instances[0]["pid"]
                elif time.monotonic() > deadline:
                    raise Failure("no window appeared within a minute of `open`")
                else:
                    time.sleep(0.5)
            server.tool("select_instance", pid=window)
        else:
            window = server.tool("start_instance", files=[grid])["started"]["pid"]
        print(f"gui-smoke: window with process id {window}")

        # The import may not have begun when the window first answers, so
        # wait for the layer itself before asking whether work is under way.
        deadline = time.monotonic() + 60
        while True:
            clouds = server.tool("status")["result"]["clouds"]
            if len(clouds) == 1 and clouds[0]["points"] == POINTS:
                break
            if time.monotonic() > deadline:
                raise Failure(f"expected one layer with {POINTS} points, the window lists {clouds}")
            time.sleep(0.5)

        # The same picture on every system: stored colours, no shading, points
        # large enough to count, the whole grid in view.
        server.tool("set_color", mode="rgb")
        server.tool("set_eye_dome", enabled=False)
        server.tool("set_point_size", size=4)
        server.tool("zoom_all")
        idle = server.tool("wait_until_idle", timeout_seconds=60)
        if not idle.get("idle"):
            raise Failure(f"the window is still busy with {idle.get('busy')}")

        server.tool("screenshot", path=screenshot, max_edge=1024)
        check_screenshot(screenshot)

        listed = server.tool("list_instances")["instances"]
        ours = [instance for instance in listed if instance["pid"] == window]
        if not ours:
            raise Failure(f"the window is missing from list_instances: {listed}")
        if options.version and ours[0]["version"] != options.version:
            raise Failure(f"the window reports version {ours[0]['version']}, expected {options.version}")
    finally:
        watchdog.cancel()
        if window is not None:
            end_process(window)
        server.close()


def main():
    parser = argparse.ArgumentParser(description="Window test of a packaged Open Pointcloud Studio.")
    parser.add_argument("binary", help="the application binary, or an AppImage file")
    parser.add_argument("--version", help="the version the server and the window have to report")
    parser.add_argument("--screenshot", help="where the screenshot is kept (default: a temporary file)")
    parser.add_argument("--open-with", metavar="APPLICATION", help="macOS: start the window with `open -a APPLICATION FILE`")
    parser.add_argument("--timeout", type=float, default=120, help="seconds before the test gives up (default 120)")
    options = parser.parse_args()
    options.binary = os.path.abspath(options.binary)

    folder = tempfile.mkdtemp(prefix="ops-gui-smoke-")
    try:
        run(options, folder)
    except Failure as failure:
        print(f"gui-smoke: FAILED: {failure}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(folder, ignore_errors=True)
    print("gui-smoke: passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
