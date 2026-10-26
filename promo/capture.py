#!/usr/bin/env python3
"""Capture the stills of the feature promo from the generated demo scenes.

The stills show nothing but the two scenes that the demo_scenes example of
pointcloud-core writes (see native/TEST_DATA.md, "Generated demo scenes"):

    cargo run -p pointcloud-core --release --example demo_scenes -- SCENES

Windows, in a window of its own (the usual way):

    python promo/capture.py SCENES --exe PATH\\TO\\open-pointcloud-studio.exe

starts the application with a temporary APPDATA and LOCALAPPDATA, so the
settings, views and recent files of the user stay as they are, keeps its
window off-screen, captures every still and closes the window again.
--data DIR keeps those two folders in DIR instead, and the index of the
scans with them, which makes a second run start faster.

Windows or Linux, in a window that is already open and has no scan open:

    python3 promo/capture.py SCENES --pid PID [--window X11_WINDOW_ID]

On Linux the window is sized with wmctrl and the pointer is parked inside it
with python-xlib, both through the X11 window ID; on Windows the window is
sized where it stands. The window is switched to English and the theme Light,
and those choices stay.

The window is driven through its local command API (native/API.md). Every
still is the whole window as the application draws it, at 1584 x 900 logical
pixels (920 x 900 for the narrow ribbon), scaled to 960 pixels high, the
height at which render.py shows it, and written to promo/stills/NN-name.png.
Pillow is needed for that.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from PIL import Image


ROOT = pathlib.Path(__file__).resolve().parent
STILLS = ROOT / "stills"
HEIGHT = 960
WIDE = (1584, 900)
NARROW = (920, 900)
OFF_SCREEN = (-4400, 40)

# The two scenes of the demo_scenes example. The house stands at X 1025-1036,
# Y 2022-2030.5 in a local grid; the rooms span X 0-10, Y 0-4.2, Z 0-2.7.
HOUSE_CENTRE = [1030.5, 2025.0, 12.5]
ROOF_SOUTH = [
    [1024.75, 2021.55, 16.25],
    [1036.25, 2021.55, 16.25],
    [1036.25, 2026.25, 20.19],
    [1024.75, 2026.25, 20.19],
]
ROOMS_CENTRE = [5.0, 2.1, 1.0]
# The rooms without their ceiling; set_section takes boxes inside the model.
ROOMS_OPEN = ([-0.013, -0.174, -0.007], [10.015, 4.208, 2.25])


class Api:
    """The local command API of one window."""

    def __init__(self, discovery: pathlib.Path, timeout: float = 120.0):
        deadline = time.time() + timeout
        while not discovery.exists():
            if time.time() > deadline:
                raise RuntimeError(f"No discovery file at {discovery}")
            time.sleep(0.2)
        time.sleep(0.2)
        config = json.loads(discovery.read_text())
        self.url = f"http://127.0.0.1:{config['port']}/exec"
        self.token = config["token"]

    def __call__(self, action: str, **fields):
        request = urllib.request.Request(
            self.url,
            data=json.dumps({"command": action, **fields}).encode(),
            headers={"Content-Type": "application/json", "X-OPS-Token": self.token},
        )
        try:
            answer = json.loads(urllib.request.urlopen(request, timeout=120).read())
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"{action}: HTTP {error.code} {error.read().decode(errors='replace')}") from None
        if not answer.get("ok"):
            raise RuntimeError(f"{action}: {answer}")
        return answer.get("result", answer)

    def status(self) -> dict:
        return self("status")

    def busy(self) -> list[str]:
        """What the window is still doing, as the MCP server's wait_until_idle
        sees it, plus a large scan whose index is not built yet."""
        state = self.status()
        busy = []
        if state.get("imports"):
            busy.append("imports")
        for key in ("index_progress", "scale", "mesh", "merge", "bag3d", "texture_export"):
            if isinstance(state.get(key), dict):
                busy.append(key)
        for key in ("selection_pending", "selection_bounds_pending", "section_align_pending",
                    "thin_pending", "mesh_export_pending", "detail_pending"):
            if state.get(key) is True:
                busy.append(key)
        if (state.get("photos_loading") or 0) > 0:
            busy.append("photos")
        photos = state.get("photos") or {}
        if (photos.get("listing") or 0) > 0 or (photos.get("decoding") or 0) > 0:
            busy.append("file_photos")
        if ((state.get("views") or {}).get("snapshots_pending") or 0) > 0:
            busy.append("snapshots")
        for key in ("drawing", "closed_mesh", "faces", "colour_from_photos"):
            if isinstance((state.get(key) or {}).get("job"), dict):
                busy.append(key)
        drawing_view = state.get("drawing_view") or {}
        if drawing_view.get("reading") is not None or drawing_view.get("remaking"):
            busy.append("drawing_view")
        if (state.get("section_fill") or {}).get("pending") is True:
            busy.append("section_caps")
        index = state.get("index") or {}
        if index.get("builds") or index.get("waiting"):
            busy.append("index")
        for cloud in state.get("clouds", []):
            if (cloud.get("points") or 0) >= 1_000_000 and not cloud.get("indexed") and not cloud.get("mesh"):
                busy.append("unindexed")
                break
        return busy

    def wait_idle(self, timeout: float = 1800.0) -> None:
        """Wait until three looks in a row find nothing under way."""
        time.sleep(0.5)
        deadline = time.time() + timeout
        quiet = 0
        while quiet < 3:
            if time.time() > deadline:
                raise RuntimeError(f"Still busy after {timeout:.0f} s: {self.busy()}")
            quiet = 0 if self.busy() else quiet + 1
            time.sleep(0.25)

    def open(self, path: pathlib.Path) -> None:
        """Open a scan and wait until it is read and indexed."""
        self("open", path=str(path))
        deadline = time.time() + 1800
        if path.suffix.lower() in (".las", ".laz"):
            # A LAS or LAZ file opens from its header and reads its points
            # afterwards, which no import reports. Once they are read the
            # status line says "Ready: N points from FILE", and a scan without
            # an index from an earlier run starts to build one.
            while True:
                state = self.status()
                if str(state.get("status", "")).startswith("Ready:"):
                    break
                index = state.get("index") or {}
                if index.get("builds") or index.get("waiting"):
                    break
                if time.time() > deadline:
                    raise RuntimeError(f"The points of {path.name} were not read")
                time.sleep(0.1)
        while not any(pathlib.Path(cloud["path"]).name == path.name for cloud in self.status()["clouds"]):
            if time.time() > deadline:
                raise RuntimeError(f"{path.name} did not open")
            time.sleep(0.5)
        self.wait_idle()

    def wait_job(self, answer: dict, timeout: float = 1800.0) -> dict:
        deadline = time.time() + timeout
        while time.time() < deadline:
            job = self("job", id=answer["job_id"])
            job = job.get("job", job)
            if job.get("state") not in ("running", "queued"):
                if job.get("state") != "complete":
                    raise RuntimeError(f"Job failed: {job}")
                return job
            time.sleep(0.5)
        raise RuntimeError(f"Job {answer['job_id']} did not finish")

    def show_drawing_tab(self, name: str) -> None:
        self("show_tab", name=name)
        for _ in range(600):
            state = self.status()
            drawing = state["drawing_view"]
            tabs = state["view_tabs"]
            active = next((tab for tab in tabs["tabs"] if tab["index"] == tabs["active"]), None)
            if (
                state["drawing"]["job"] is None
                and drawing["shown"]
                and drawing["reading"] is None
                and not drawing["remaking"]
                and active is not None
                and active["name"] == name
            ):
                return
            time.sleep(0.5)
        raise RuntimeError(f"Drawing {name} did not appear in its own tab")

    def show_model(self) -> None:
        """The 3D model tab, letting go of an active view."""
        self("show_drawing", name="3D model")
        for _ in range(40):
            state = self.status()
            if state["view_tabs"]["active"] == 0 and not state["drawing_view"]["shown"]:
                return
            time.sleep(0.25)
        raise RuntimeError("The 3D model tab did not open")


class WindowsWindow:
    """Size a window through user32, in physical pixels."""

    def __init__(self, pid: int, off_screen: bool):
        import ctypes
        import ctypes.wintypes as wt

        self.ctypes, self.wt = ctypes, wt
        self.user32 = ctypes.WinDLL("user32", use_last_error=True)
        self.user32.MoveWindow.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.BOOL]
        self.user32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
        self.user32.GetClientRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
        self.user32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
        self.user32.GetWindowTextW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
        self.user32.ShowWindow.argtypes = [wt.HWND, ctypes.c_int]
        self.user32.PostMessageW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
        self.pid = pid
        self.off_screen = off_screen
        self.hwnd = None
        deadline = time.time() + 120
        while self.hwnd is None:
            if time.time() > deadline:
                raise RuntimeError(f"No window of process {pid}")
            self.hwnd = self.find()
            time.sleep(0.01)
        if off_screen:
            # Move it away at once, and again while the window settles.
            for _ in range(40):
                self.place()
                time.sleep(0.05)

    def find(self):
        ctypes, wt = self.ctypes, self.wt
        found = []
        callback_type = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

        def callback(hwnd, _):
            owner = wt.DWORD()
            self.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
            if owner.value == self.pid:
                title = ctypes.create_unicode_buffer(256)
                self.user32.GetWindowTextW(hwnd, title, 256)
                if "Pointcloud" in title.value:
                    found.append(hwnd)
            return True

        self.user32.EnumWindows(callback_type(callback), 0)
        return found[0] if found else None

    def place(self, width: int | None = None, height: int | None = None) -> None:
        window, client = self.wt.RECT(), self.wt.RECT()
        self.user32.GetWindowRect(self.hwnd, self.ctypes.byref(window))
        self.user32.GetClientRect(self.hwnd, self.ctypes.byref(client))
        outer_w, outer_h = window.right - window.left, window.bottom - window.top
        if width is not None:
            outer_w = width + outer_w - (client.right - client.left)
            outer_h = height + outer_h - (client.bottom - client.top)
        x, y = OFF_SCREEN if self.off_screen else (window.left, window.top)
        self.user32.MoveWindow(self.hwnd, x, y, outer_w, outer_h, True)
        if self.off_screen:
            self.user32.ShowWindow(self.hwnd, 4)  # SW_SHOWNOACTIVATE

    def size(self, width: int, height: int, scale: float) -> None:
        self.place(round(width * scale), round(height * scale))

    def park_pointer(self) -> None:
        pass

    def close(self) -> None:
        if self.find():
            self.user32.PostMessageW(self.hwnd, 0x0010, 0, 0)  # WM_CLOSE


class X11Window:
    """Size a window with wmctrl and park the pointer in it, in logical pixels."""

    def __init__(self, window_id: str):
        self.window_id = window_id

    def size(self, width: int, height: int, scale: float) -> None:
        subprocess.run(["wmctrl", "-ir", self.window_id, "-e", f"0,20,80,{width},{height}"], check=True)

    def park_pointer(self) -> None:
        from Xlib import display

        x11 = display.Display()
        x11.create_resource_object("window", int(self.window_id, 0)).warp_pointer(10, 850)
        x11.sync()
        x11.close()
        time.sleep(0.2)


class Studio:
    """One window with its API, sized and captured."""

    def __init__(self, api: Api, window):
        self.api = api
        self.window = window
        probe = api("screenshot", window=True, max_edge=16)
        self.scale = float(probe.get("scale_factor") or 1.0)
        self.logical = None

    def size(self, logical: tuple[int, int]) -> None:
        self.logical = logical
        self.window.size(*logical, self.scale)
        time.sleep(0.8)

    def capture(self, number: int, name: str) -> None:
        self.window.park_pointer()
        self.api.wait_idle()
        path = STILLS / f"{number:02d}-{name}.png"
        raw = path.with_suffix(".capture.png")
        for _ in range(3):
            result = self.api("screenshot", window=True, path=str(raw), max_edge=8192)
            size = tuple(round(value) for value in result.get("viewport_size", [0, 0]))
            if size == self.logical:
                break
            # The window has not taken its new size yet.
            self.window.size(*self.logical, self.scale)
            time.sleep(1.0)
            self.api.wait_idle()
        else:
            raise RuntimeError(f"{path.name}: the window is {size}, not {self.logical}")
        image = Image.open(raw).convert("RGB")
        width = round(image.width * HEIGHT / image.height)
        image.resize((width, HEIGHT), Image.Resampling.LANCZOS).save(path, optimize=True)
        raw.unlink()
        print(f"{path.name}: {result['width']}×{result['height']} captured, {width}×{HEIGHT} kept", flush=True)


def building(studio: Studio, scenes: pathlib.Path) -> None:
    """The house: overview, colours, detail, selection, measuring, the narrow
    ribbon and the File view."""
    app = studio.api
    app.open(scenes / "building.laz")
    app("set_budget", points=3_000_000)
    app("set_point_size", size=2.0)
    app("set_color", mode="rgb")
    app("set_eye_dome", enabled=False)
    # A plan, a section and a saved view, so that their tabs show.
    app.wait_job(app("create_drawing", kind="plan", name="Ground floor", straight_lines=True))
    app.wait_job(app("create_drawing", kind="section", name="Section A", side="left",
                     position=1030.5, straight_lines=True))
    app.show_model()
    app("set_section", min=[1018.0, 2010.0, 9.661], max=[1046.0, 2042.0, 23.9])
    app("set_camera", yaw=-0.62, pitch=0.42, zoom=0.30, pan=[0, 0], orbit_point=HOUSE_CENTRE)
    app.wait_idle()
    app("save_camera_view", name="Front garden")
    app.wait_idle()
    app.show_model()
    app("clear_section")

    app("set_camera", yaw=-0.62, pitch=0.40, zoom=0.31, pan=[0, 0], orbit_point=HOUSE_CENTRE)
    studio.capture(1, "overview")

    # The Display panel beneath the view cube.
    app("set_ribbon_tab", tab="view")
    app("set_camera", yaw=-0.62, pitch=0.42, zoom=0.34, pan=[-170, 0], orbit_point=HOUSE_CENTRE)
    app("set_color", mode="elevation")
    studio.capture(11, "elevation")
    app("set_color", mode="rgb")
    app("set_point_size", size=3.0)
    app("set_eye_dome", enabled=True)
    studio.capture(12, "point-detail")
    app("set_eye_dome", enabled=False)
    app("set_point_size", size=2.0)
    app("set_ribbon_tab", tab="start")

    # A selection is marked by a sample of 8,000 of its points, which lie
    # closer together in a view from further away.
    app("set_camera", yaw=-0.62, pitch=0.46, zoom=0.36, pan=[0, 0], orbit_point=HOUSE_CENTRE)
    app("select_world", min=[1024.6, 2021.4, 15.9], max=[1036.4, 2031.1, 20.4])
    app.wait_idle()
    print(f"Selection: {app.status()['selected_points']:,} points", flush=True)
    studio.capture(2, "selection")
    app("clear_selection")

    app("set_camera", yaw=-0.62, pitch=0.46, zoom=0.27, pan=[0, 0], orbit_point=HOUSE_CENTRE)
    app("measure", mode="area", points=ROOF_SOUTH)
    studio.capture(13, "measure")
    app("clear_measure")

    studio.size(NARROW)
    app("set_camera", yaw=-0.62, pitch=0.42, zoom=0.25, pan=[0, 0], orbit_point=HOUSE_CENTRE)
    studio.capture(9, "responsive")
    studio.size(WIDE)

    app("file_view", open=True, page="export")
    studio.capture(10, "view")
    app("file_view", open=False)

    for name in ("Front garden", "Section A", "Ground floor"):
        app("close_tab", name=name)
    app("remove", index=0)
    app.wait_idle()


def rooms(studio: Studio, scenes: pathlib.Path) -> None:
    """The two rooms: drawings, the section box, the station photo beside the
    3D model, and the closed mesh."""
    app = studio.api
    app.open(scenes / "rooms.e57")
    # The labels of a station and of its photo lie on top of each other.
    app("set_scan_poses_visible", visible=False)

    app.wait_job(app("create_drawing", kind="plan", name="Plan", straight_lines=True))
    app.wait_job(app("create_drawing", kind="section", name="Section A", side="front",
                     position=1.7, thickness=1.5, straight_lines=True))
    app.show_drawing_tab("Plan")
    studio.capture(6, "plan")
    app.show_drawing_tab("Section A")
    studio.capture(7, "section-2d")

    app.show_model()
    app("set_section", min=ROOMS_OPEN[0], max=ROOMS_OPEN[1])
    app("set_camera", yaw=-0.95, pitch=0.80, zoom=0.70, pan=[0, 0], orbit_point=ROOMS_CENTRE)
    studio.capture(3, "section")

    # Station 2 looks through the open door into the first room. The pane of
    # the 3D model beside it keeps the camera of the 3D model tab.
    app("set_camera", yaw=-0.95, pitch=0.80, zoom=0.86, pan=[0, 0], orbit_point=ROOMS_CENTRE)
    app("enter_photo", index=1)
    for _ in range(240):
        view = app.status()["photos"]["view"]
        if view and view.get("shown"):
            break
        time.sleep(0.5)
    else:
        raise RuntimeError("The photo of Station 2 was not shown")
    app("set_panorama", yaw=2.75, pitch=-0.15, field_of_view=1.5)
    studio.capture(8, "panorama")
    app("set_photo_compare", enabled=True)
    app("set_panorama", yaw=2.75, pitch=-0.15, field_of_view=1.3)
    studio.capture(14, "photo-compare")
    app("set_photo_compare", enabled=False)
    app("close_panorama")
    app("close_tab", name="Station 2")
    app.show_model()

    app("clear_section")
    app("mesh_wizard", open=True, step="method", method="closed")
    studio.capture(4, "mesher")
    app("mesh_wizard", open=False)
    # The closed mesh of what lies below the ceiling, shown without the points.
    app("set_section", min=ROOMS_OPEN[0], max=ROOMS_OPEN[1])
    app("set_camera", yaw=-0.95, pitch=0.80, zoom=0.70, pan=[0, 0], orbit_point=ROOMS_CENTRE)
    app("mesh", mode="closed")
    app.wait_idle()
    last = app.status()["closed_mesh"]["last"]
    if not last or last.get("state") != "complete":
        raise RuntimeError(f"The closed mesh did not finish: {last}")
    print(f"Closed mesh: {last['vertices']:,} vertices, {last['triangles']:,} triangles", flush=True)
    app("set_visible", index=0, visible=False)
    studio.capture(5, "mesher-options")


def start(exe: pathlib.Path, data: pathlib.Path) -> subprocess.Popen:
    roaming, local = data / "r", data / "l"
    roaming.mkdir(parents=True, exist_ok=True)
    local.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, APPDATA=str(roaming), LOCALAPPDATA=str(local))
    info = subprocess.STARTUPINFO()
    info.dwFlags = subprocess.STARTF_USESHOWWINDOW
    info.wShowWindow = 0  # SW_HIDE
    flags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
    return subprocess.Popen([str(exe)], env=env, startupinfo=info, creationflags=flags,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=str(data))


def discovery_dir() -> pathlib.Path:
    if sys.platform == "win32":
        return pathlib.Path(os.environ["APPDATA"]) / "open-pointcloud-studio-native" / "instances"
    config = os.environ.get("XDG_CONFIG_HOME") or str(pathlib.Path.home() / ".config")
    return pathlib.Path(config) / "open-pointcloud-studio-native" / "instances"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("scenes", type=pathlib.Path, help="Folder with building.laz and rooms.e57")
    parser.add_argument("--exe", type=pathlib.Path, help="Windows: start this application off-screen")
    parser.add_argument("--data", type=pathlib.Path, help="Windows with --exe: keep its APPDATA and LOCALAPPDATA here")
    parser.add_argument("--pid", type=int, help="Drive the running window of this process")
    parser.add_argument("--window", help="Linux with --pid: the X11 window ID, for wmctrl and the pointer")
    args = parser.parse_args()
    scenes = args.scenes.resolve()
    for name in ("building.laz", "rooms.e57"):
        if not (scenes / name).exists():
            parser.error(f"{scenes / name} is missing; write the scenes with the demo_scenes example first")
    if bool(args.exe) == bool(args.pid):
        parser.error("give either --exe (Windows) or --pid")
    if sys.platform == "win32":
        import ctypes

        try:
            ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        except (AttributeError, OSError):
            pass
    elif args.exe:
        parser.error("--exe is for Windows; start the window yourself and pass --pid and --window")
    elif not args.window:
        parser.error("on Linux, pass the X11 window ID with --window")

    process = None
    data = None
    window = None
    try:
        if args.exe:
            data = args.data.resolve() if args.data else pathlib.Path(tempfile.mkdtemp(prefix="ops-promo-"))
            # Views and drawings of an earlier run would take the names of this one.
            shutil.rmtree(data / "r", ignore_errors=True)
            process = start(args.exe.resolve(), data)
            window = WindowsWindow(process.pid, off_screen=True)
            discovery = data / "r" / "open-pointcloud-studio-native" / "instances" / f"instance-{process.pid}.json"
        else:
            window = WindowsWindow(args.pid, off_screen=False) if sys.platform == "win32" else X11Window(args.window)
            discovery = discovery_dir() / f"instance-{args.pid}.json"
        api = Api(discovery)
        if api.status().get("clouds"):
            raise RuntimeError("The window has scans open; start from a window without them")
        api("set_language", language="en")
        api("set_theme", theme="light")
        studio = Studio(api, window)
        studio.size(WIDE)
        building(studio, scenes)
        rooms(studio, scenes)
    finally:
        if process is not None:
            if window is not None:
                window.close()
            try:
                process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            if args.data is None:
                shutil.rmtree(data, ignore_errors=True)


if __name__ == "__main__":
    main()
