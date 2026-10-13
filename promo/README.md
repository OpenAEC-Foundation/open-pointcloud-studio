# Open Pointcloud Studio feature promo

The silent MP4 uses thirteen desktop captures from the native Rust v0.9.5 build with a 46.6-million-point E57 scan. It opens on the single Start ribbon, demonstrates the controls beneath the view cube, then shows point selection, measuring, the section box, mesher settings, 2D plan and section, an E57 station photo, and responsive groups. The screenshots are unaltered inside a white promo frame.

Capture the scenes from a running build with the lobby scan open, using its process ID and X11 window ID:

```sh
python3 promo/capture.py PID WINDOW_ID
```

Render it with Python 3, Pillow and FFmpeg:

```sh
python3 promo/render.py --ffmpeg /path/to/ffmpeg
```

The result is `promo/open-pointcloud-studio-features.mp4` (1920×1080, H.264, silent). Temporary slides and clips go in `promo/.work/`.
