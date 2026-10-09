#!/usr/bin/env python3
"""Capture the feature promo from a running native Rust desktop build.

Usage: python3 promo/capture.py PID WINDOW_ID
The instance must have the Matterport lobby E57 open and indexed.
"""

import json
import pathlib
import subprocess
import sys
import time
import urllib.request


ROOT = pathlib.Path(__file__).resolve().parent


def main() -> None:
    pid, window_id = sys.argv[1:3]
    instance = pathlib.Path.home() / ".config/open-pointcloud-studio-native/instances" / f"instance-{pid}.json"
    config = json.loads(instance.read_text())

    def command(action: str, **fields):
        request = urllib.request.Request(
            f"http://127.0.0.1:{config['port']}/exec",
            data=json.dumps({"command": action, **fields}).encode(),
            headers={"Content-Type": "application/json", "X-OPS-Token": config["token"]},
        )
        answer = json.loads(urllib.request.urlopen(request, timeout=120).read())
        if not answer.get("ok"):
            raise RuntimeError(f"{action}: {answer}")
        return answer.get("result", answer)

    def size(width: int):
        subprocess.run(["wmctrl", "-ir", window_id, "-e", f"0,20,80,{width},900"], check=True)
        time.sleep(0.5)

    def capture(number: int, name: str):
        path = ROOT / "stills" / f"{number:02d}-{name}.png"
        result = command("screenshot", path=str(path), window=True)
        print(f"{path.name}: {result.get('width')}×{result.get('height')}", flush=True)

    size(1440)
    if command("status")["walk"] is not None:
        command("close_panorama")
    command("clear_section")
    command("clear_selection")
    command("show_drawing", name="3D model")
    command("set_camera", yaw=-0.8, pitch=0.6, zoom=0.1612183, pan=[-500.0, 106.0])
    capture(1, "overview")

    selected = command(
        "select_world", min=[4.0, -26.0, -1.9], max=[7.0, -20.0, 5.4]
    )
    for _ in range(120):
        state = command("status")
        if not state["selection_pending"]:
            break
        time.sleep(0.5)
    print(f"Selection: {state['selected_points']:,} points; job: {selected}", flush=True)
    capture(2, "selection")

    command("clear_selection")
    command("set_section", min=[-29.0, -117.5, -1.8], max=[19.0, 10.5, 5.3])
    capture(3, "section")
    command("clear_section")

    command("mesh_wizard", open=True, step="method", method="closed")
    capture(4, "mesher")
    command("mesh_wizard", open=True, step="options", method="closed")
    capture(5, "mesher-options")
    command("mesh_wizard", open=False)

    for number, slug, name in [
        (6, "plan", "Matterport lobby - issue 11 fill"),
        (7, "section-2d", "Matterport lobby - issue 9 section"),
    ]:
        command("show_drawing", name=name)
        for _ in range(120):
            state = command("status")["drawing_view"]
            if state["shown"] and not state["reading"] and not state["remaking"]:
                break
            time.sleep(0.5)
        capture(number, slug)

    command("show_drawing", name="3D model")
    for _ in range(20):
        if not command("status")["drawing_view"]["shown"]:
            break
        time.sleep(0.2)
    command("open_panorama", index=0, station=0)
    for _ in range(120):
        walk = command("status")["walk"]
        if walk is not None and walk.get("station", {}).get("full_resolution"):
            break
        time.sleep(0.5)
    capture(8, "panorama")
    command("close_panorama")

    size(920)
    command("set_camera", yaw=-0.8, pitch=0.6, zoom=0.1612183, pan=[-226.0, 106.0])
    capture(9, "responsive")
    size(1584)


if __name__ == "__main__":
    main()
