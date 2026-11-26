#!/usr/bin/env python3
"""Exercise cancellation and reopening of a large scan in a Linux or Windows desktop session."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from urllib.request import Request, urlopen

from PIL import Image, ImageChops


def check(binary, scan, expected_points, folder, timeout, budget):
    profile = folder / "config"
    environment = dict(os.environ, XDG_CONFIG_HOME=str(profile),
                       XDG_CACHE_HOME=str(folder / "cache"), OPS_INDEX_DIR=str(folder / "indexes"),
                       OPENAEC_BEHEERD="1")
    version = subprocess.check_output([binary, "--version"], text=True).strip()
    desktop = subprocess.Popen([binary, "--api-port", "0"], env=environment, stdout=sys.stderr)
    try:
        discovery = profile / "open-pointcloud-studio-native/instances" / f"instance-{desktop.pid}.json"
        deadline = time.monotonic() + timeout
        while not discovery.exists():
            if desktop.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("Desktop did not publish its API endpoint")
            time.sleep(0.1)
        instance = json.loads(discovery.read_text())
        endpoint = f"http://127.0.0.1:{instance['port']}/exec"
        headers = {"Content-Type": "application/json", "X-OPS-Token": instance["token"]}

        def api(command, **arguments):
            body = json.dumps({"command": command, **arguments}).encode()
            with urlopen(Request(endpoint, body, headers), timeout=timeout) as response:
                answer = json.load(response)
            if not answer.get("ok"):
                raise RuntimeError(f"{command} failed: {answer}")
            return answer

        def wait_for(description, predicate):
            deadline = time.monotonic() + timeout
            while True:
                state = api("status")["result"]
                if predicate(state):
                    return state
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"Timed out waiting for {description}: {state['status']}")
                time.sleep(0.1)

        def peak_rss_kib():
            if os.name == "nt":
                query = f"(Get-Process -Id {desktop.pid} -ErrorAction Stop).PeakWorkingSet64"
                return int(subprocess.check_output(
                    ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", query],
                    text=True,
                )) // 1024
            lines = Path(f"/proc/{desktop.pid}/status").read_text().splitlines()
            return int(next(line for line in lines if line.startswith("VmHWM:")).split()[1])

        api("set_auto_index", enabled=False)
        api("set_budget", points=budget)
        start = time.monotonic()
        api("open", path=str(scan))
        preview = wait_for("a preview during import", lambda s: s["imports"] and s["clouds"]
                           and s["clouds"][0]["view_sample"] > 0)
        result = {"version": version, "scan": scan.name, "bytes": scan.stat().st_size,
                  "requested_point_budget": budget,
                  "initial_preview_seconds": time.monotonic() - start,
                  "preview_points_before_cancel": preview["clouds"][0]["view_sample"]}
        start = time.monotonic()
        api("cancel_import", id=preview["imports"][0]["id"])
        wait_for("empty workspace after cancellation", lambda s: not s["imports"] and not s["clouds"])
        result["cancel_to_empty_seconds"] = time.monotonic() - start

        start = time.monotonic()
        api("open", path=str(scan))
        final = wait_for("reopened scan", lambda s: not s["imports"] and s["clouds"])
        result["reopen_seconds"] = time.monotonic() - start
        result["peak_rss_through_reopen_kib"] = peak_rss_kib()
        if len(final["clouds"]) != 1:
            raise RuntimeError(f"Expected one layer, found {len(final['clouds'])}")
        layer = final["clouds"][0]
        if (Path(layer["path"]) != scan or layer["points"] != expected_points
                or layer["remaining"] != expected_points or layer["view_sample"] <= 0 or layer["indexed"]):
            raise RuntimeError(f"Unexpected reopened layer: {layer}")
        result.update(source_points=layer["points"], view_sample_points=layer["view_sample"])

        camera = final["camera"]
        before, after = folder / "initial.png", folder / "rotated.png"
        api("screenshot", path=str(before), base64=False, max_edge=1024)
        api("set_camera", yaw=camera["yaw"] + 0.3, pitch=camera["pitch"],
            zoom=camera["zoom"], pan=camera["pan"])
        api("screenshot", path=str(after), base64=False, max_edge=1024)
        with Image.open(before) as first, Image.open(after) as second:
            if first.size != second.size:
                raise RuntimeError("Viewport dimensions changed")
            delta = ImageChops.difference(first.convert("RGB"), second.convert("RGB"))
            changes = sum(count for count, rgb in delta.getcolors(delta.width * delta.height)
                          if max(rgb) > 10)
        if changes < 1000:
            raise RuntimeError(f"Only {changes} viewport pixels changed after rotation")
        result.update(changed_pixels=changes, peak_rss_through_render_kib=peak_rss_kib())
        return result
    finally:
        desktop.terminate()
        try:
            desktop.wait(timeout=15)
        except subprocess.TimeoutExpired:
            desktop.kill()
            desktop.wait()
            raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("scan", type=Path)
    parser.add_argument("expected_points", type=int, help="known source count, independently of Studio")
    parser.add_argument("--timeout", type=float, default=180, help="seconds per stage")
    parser.add_argument("--budget", type=int, default=1_000_000, help="requested viewport point budget")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="ops-large-import-") as scratch:
        print(json.dumps(check(args.binary.resolve(strict=True), args.scan.resolve(strict=True),
                               args.expected_points, Path(scratch), args.timeout, args.budget), indent=2))
