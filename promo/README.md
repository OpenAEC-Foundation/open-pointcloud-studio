# Open Pointcloud Studio feature promo

The silent MP4 uses nine fresh desktop captures from the native Rust v0.9.2 build with a 46.6-million-point E57 scan. It shows the approved two-tab ribbon, point selection, section box, mesher settings, 2D plan and section, and an E57 station photo. The screenshots are unaltered inside a white promo frame.

Capture the scenes from a running build with the lobby scan open, using its process ID and X11 window ID:

```sh
python3 promo/capture.py PID WINDOW_ID
```

Render it with Python 3, Pillow and FFmpeg:

```sh
python3 promo/render.py --ffmpeg /path/to/ffmpeg
```

The result is `promo/open-pointcloud-studio-features.mp4` (1920×1080, H.264, silent). Temporary slides and clips go in `promo/.work/`.
