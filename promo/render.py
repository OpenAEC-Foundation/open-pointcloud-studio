#!/usr/bin/env python3
"""Render the native-app feature promo from verified desktop captures."""

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
SCREEN_SIZE = (1536, 960)
INK = "#27272A"
MUTED = "#57534E"
AMBER = "#D97706"
PAPER = "#FAFAF9"


# Every image is an unaltered native-app capture; only the surrounding promo
# frame, title and fade are rendered here.
SCENES = [
    ("intro", None, 2.4, "Open Pointcloud Studio", "Van grote scan naar bruikbare 3D en 2D"),
    ("overview", "01-overview.png", 3.6, "Grote scans, direct bruikbaar", "E57 · 46,6 miljoen punten · adaptieve detailniveaus"),
    ("selection", "02-selection.png", 3.6, "Selecteer punten op volle resolutie", "2,96 miljoen punten in één selectie"),
    ("section", "03-section.png", 3.6, "Snijd de scan met een 3D-section box", "Direct zicht op het gebied dat telt"),
    ("mesher", "04-mesher.png", 3.3, "Van punten naar mesh", "Gesloten mesh, terrein, 3D-oppervlak en vlakken"),
    ("mesher-options", "05-mesher-options.png", 3.3, "Bepaal de reconstructie", "Instelbare meshgrootte, buren en randfactor"),
    ("plan", "06-plan.png", 3.6, "Maak een 2D-plattegrond", "Lijnen, vullingen en DXF/DWG-lagen"),
    ("section-2d", "07-section-2d.png", 3.3, "Bekijk een verticale doorsnede", "Uit dezelfde E57-scan"),
    ("panorama", "08-panorama.png", 4.0, "Stap de scan in", "Stationfoto's rechtstreeks uit de E57"),
    ("responsive", "09-responsive.png", 3.6, "Een ribbon die meeschakelt", "Groepen klappen in tot compacte iconen en dropdowns"),
    ("outro", None, 2.6, "Open Pointcloud Studio", "Native Rust · E57, LAS en LAZ · 3D en 2D"),
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
    if screenshot.height != 900 or screenshot.width not in (920, 1440):
        raise ValueError(f"Expected a 920×900 or 1440×900 desktop capture: {source}")
    image = Image.new("RGB", SIZE, PAPER)
    scale = min(SCREEN_SIZE[0] / screenshot.width, SCREEN_SIZE[1] / screenshot.height)
    shown_size = (round(screenshot.width * scale), round(screenshot.height * scale))
    shown_pos = ((SIZE[0] - shown_size[0]) // 2, SCREEN_POS[1] + (SCREEN_SIZE[1] - shown_size[1]) // 2)
    image.paste(screenshot.resize(shown_size, Image.Resampling.LANCZOS), shown_pos)
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
