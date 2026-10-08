# Open Pointcloud Studio feature promo

The silent MP4 uses nine fresh desktop captures from the native Rust build with a 46.6-million-point E57 scan. It shows the responsive ribbon, a 4.12-million-point selection, section box, mesher settings, 2D plan and section, and an E57 station photo. The screenshots are unaltered inside a white promo frame.

Render it with Python 3, Pillow and FFmpeg:

```sh
python3 promo/render.py --ffmpeg /path/to/ffmpeg
```

The result is `promo/open-pointcloud-studio-features.mp4` (1920×1080, H.264, silent). Temporary slides and clips go in `promo/.work/`.
