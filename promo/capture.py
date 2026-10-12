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

from Xlib import display


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
        x11 = display.Display()
        x11.create_resource_object("window", int(window_id, 0)).warp_pointer(10, 850)
        x11.sync()
        x11.close()
        time.sleep(0.2)
        result = command("screenshot", path=str(path), window=True)
        print(f"{path.name}: {result.get('width')}×{result.get('height')}", flush=True)

    size(1584)
    command("set_ribbon_tab", tab="start")
    command("clear_measure")
    if command("status")["walk"] is not None:
        command("close_panorama")
    command("clear_section")
    command("clear_selection")
    command("show_tab", index=0)
    if command("status")["view_tabs"]["active"] != 0:
        raise RuntimeError("The 3D model tab did not open")
    command("set_point_size", size=3.5)
    command("set_camera", yaw=-0.8, pitch=0.6, zoom=0.15, pan=[-645.0, 150.0])
    capture(1, "overview")

    command("set_ribbon_tab", tab="view")
    command("set_color", mode="elevation")
    capture(11, "elevation")
    command("set_color", mode="rgb")
    command("set_point_size", size=3.5)
    command("set_budget", points=2_000_000)
    command("set_eye_dome", enabled=True)
    capture(12, "point-detail")
    command("set_eye_dome", enabled=False)
    command("set_point_size", size=2.0)
    command("set_budget", points=1_000_000)

    command("set_ribbon_tab", tab="start")
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
    command("set_scan_poses_visible", visible=False)
    command(
        "measure",
        mode="area",
        points=[
            [-6.088917913422734, -33.02902547028664, 2.872678799066441],
            [0.22886960311853932, -25.716804089341533, 3.749726043361721],
            [6.506600485445442, -35.990820942486764, 2.7760313235014396],
        ],
    )
    capture(13, "measure")
    command("clear_measure")
    command("set_scan_poses_visible", visible=True)
    command("set_ribbon_tab", tab="view")
    command("set_section", min=[-29.0, -118.0, -1.8], max=[0.0, 11.0, 5.3])
    capture(3, "section")
    command("clear_section")

    command("set_ribbon_tab", tab="start")
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

    command("show_tab", index=0)
    for _ in range(20):
        state = command("status")
        if state["view_tabs"]["active"] == 0 and not state["drawing_view"]["shown"]:
            break
        time.sleep(0.2)
    else:
        raise RuntimeError("The 3D model tab did not reopen after the drawings")
    command("open_panorama", index=0, station=0)
    for _ in range(120):
        walk = command("status")["walk"]
        if walk is not None and walk.get("station", {}).get("full_resolution"):
            break
        time.sleep(0.5)
    command("set_ribbon_tab", tab="view")
    capture(8, "panorama")
    command("close_panorama")

    size(920)
    command("set_camera", yaw=-0.8, pitch=0.6, zoom=0.1612183, pan=[-226.0, 106.0])
    capture(9, "responsive")
    size(1584)
    command("set_camera", yaw=-0.8, pitch=0.6, zoom=0.1612183, pan=[-500.0, 106.0])
    capture(10, "view")
    command("set_ribbon_tab", tab="start")
    size(1584)


if __name__ == "__main__":
    main()
