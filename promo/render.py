#!/usr/bin/env python3
"""Render the feature promo and its poster from the stills of capture.py.

The stills are the whole application window, captured from the generated
demo scenes and kept at 960 pixels high, the height at which they are shown
here. This script draws the white frame with the title of each scene around
them, renders the title cards and joins every scene into one silent MP4 with
FFmpeg; the overview becomes poster.png.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parent
ASSETS = ROOT.parent / "native" / "assets" / "fonts"
WORK = ROOT / ".work"
OUTPUT = ROOT / "open-pointcloud-studio-features.mp4"
SIZE = (1920, 1080)
SCREEN_POS = (192, 106)
SCREEN_SIZE = (1800, 960)
INK = "#27272A"
MUTED = "#57534E"
AMBER = "#D97706"
PAPER = "#FAFAF9"


# Every image is an unaltered capture of the application; only the frame
# around it, the title and the fade are rendered here.
SCENES = [
    ("intro", None, 2.4, "Open Pointcloud Studio", "From a large scan to usable 3D and 2D"),
    ("overview", "01-overview.png", 4.0, "The shared look of the OpenAEC tools", "File and Home tabs, Project Browser and Properties around a 24-million-point scan"),
    ("elevation", "11-elevation.png", 3.3, "Choose how the scan looks", "Switch from scan colour to elevation in the Display panel"),
    ("point-detail", "12-point-detail.png", 3.4, "Control the detail in the viewport", "Point size, points in view and Eye-dome beneath the view cube"),
    ("selection", "02-selection.png", 3.6, "Select points at full resolution", "2.6 million points of a roof in one selection"),
    ("measure", "13-measure.png", 3.6, "Measure in the point cloud", "Area, plan area and perimeter of a roof face"),
    ("section", "03-section.png", 3.6, "Cut the scan with a 3D section box", "Take the ceiling off and look into the rooms"),
    ("mesher", "04-mesher.png", 3.3, "From points to a mesh", "Closed mesh, terrain mesh, 3D surface and flat faces"),
    ("mesher-options", "05-mesher-options.png", 3.3, "A closed mesh in the colours of the scan", "Export it as OBJ, PLY, STL, DXF, DWG or IFC, or as a textured OBJ"),
    ("plan", "06-plan.png", 3.6, "Make a 2D plan", "Filled cut, points and straight CAD lines on DXF/DWG layers"),
    ("section-2d", "07-section-2d.png", 3.3, "Draw a vertical section", "Through both rooms of the same E57 scan"),
    ("panorama", "08-panorama.png", 4.0, "Open a station photo in its own tab", "Panoramas from the E57 file, laid over the points"),
    ("photo-compare", "14-photo-compare.png", 4.0, "Compare the photo with the 3D model", "Side by side, each pane with its own camera"),
    ("responsive", "09-responsive.png", 3.6, "A ribbon that adapts", "Groups fold into menus in a narrow window"),
    ("view", "10-view.png", 3.0, "Everything in the File view", "Open, import and export the scan, drawings, meshes and faces"),
    ("outro", None, 2.6, "Open Pointcloud Studio", "Native Rust · E57, LAS and LAZ · 3D and 2D"),
]


def font(name: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(ASSETS / name, size)


def logo(draw: ImageDraw.ImageDraw, left: int, top: int, radius: int) -> None:
    for row in range(3):
        for col in range(3):
            x = left + col * radius * 3
            y = top + row * radius * 3
            draw.ellipse((x, y, x + radius * 2, y + radius * 2), fill=AMBER)


def centered(draw: ImageDraw.ImageDraw, y: int, text: str, face: ImageFont.FreeTypeFont, color: str) -> None:
    box = draw.textbbox((0, 0), text, font=face)
    draw.text(((SIZE[0] - (box[2] - box[0])) // 2, y), text, font=face, fill=color)


def render_card(title: str, subtitle: str) -> Image.Image:
    image = Image.new("RGB", SIZE, PAPER)
    draw = ImageDraw.Draw(image)
    logo(draw, 884, 255, 23)
    centered(draw, 500, title, font("SpaceGrotesk-Medium.ttf", 74), INK)
    draw.rounded_rectangle((870, 625, 1050, 631), 3, fill=AMBER)
    centered(draw, 675, subtitle, font("Inter-Regular.ttf", 29), MUTED)
    return image


def render_scene(source: Path, title: str, subtitle: str) -> Image.Image:
    screenshot = Image.open(source).convert("RGB")
    if screenshot.height != SCREEN_SIZE[1] or not 0 < screenshot.width <= SCREEN_SIZE[0]:
        raise ValueError(f"Expected a capture {SCREEN_SIZE[1]} pixels high and at most {SCREEN_SIZE[0]} wide, as capture.py writes it: {source}")
    image = Image.new("RGB", SIZE, PAPER)
    shown_size = screenshot.size
    shown_pos = ((SIZE[0] - shown_size[0]) // 2, SCREEN_POS[1])
    image.paste(screenshot, shown_pos)
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 0, SIZE[0], 100), fill="#FFFFFF")
    draw.rectangle((0, 99, SIZE[0], 100), fill="#E7E5E4")
    draw.rounded_rectangle((160, 24, 166, 83), 3, fill=AMBER)
    draw.text((192, 19), title, font=font("SpaceGrotesk-Medium.ttf", 35), fill=INK)
    draw.text((193, 62), subtitle, font=font("Inter-Regular.ttf", 20), fill=MUTED)
    logo(draw, 1774, 31, 5)
    draw.rectangle((shown_pos[0] - 1, shown_pos[1] - 1, shown_pos[0] + shown_size[0], shown_pos[1] + shown_size[1]), outline="#D6D3D1", width=2)
    return image


def run(command: list[str]) -> None:
    subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg", default=shutil.which("ffmpeg"), help="Path to ffmpeg binary")
    args = parser.parse_args()
    if not args.ffmpeg:
        parser.error("ffmpeg is required; pass --ffmpeg /path/to/ffmpeg")
    WORK.mkdir(exist_ok=True)
    clips: list[Path] = []
    for number, (slug, source, duration, title, subtitle) in enumerate(SCENES, 1):
        image = render_card(title, subtitle) if source is None else render_scene(ROOT / "stills" / source, title, subtitle)
        if slug == "overview":
            image.save(ROOT / "poster.png", optimize=True)
        slide = WORK / f"{number:02d}-{slug}.png"
        clip = WORK / f"{number:02d}-{slug}.mp4"
        image.save(slide, optimize=True)
        fade_out = duration - 0.3
        run([
            args.ffmpeg, "-hide_banner", "-loglevel", "error", "-y",
            "-loop", "1", "-framerate", "24", "-i", str(slide), "-t", str(duration),
            "-vf", f"fade=t=in:st=0:d=0.3:color=white,fade=t=out:st={fade_out}:d=0.3:color=white,format=yuv420p",
            "-c:v", "libx264", "-preset", "medium", "-crf", "19", "-r", "24",
            "-movflags", "+faststart", str(clip),
        ])
        clips.append(clip)
        print(f"Rendered {number}/{len(SCENES)}: {title}", flush=True)
    playlist = WORK / "clips.txt"
    playlist.write_text("".join(f"file '{clip}'\n" for clip in clips))
    run([args.ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(playlist), "-c", "copy", "-movflags", "+faststart", str(OUTPUT)])
    print(f"Created {OUTPUT}")


if __name__ == "__main__":
    main()
