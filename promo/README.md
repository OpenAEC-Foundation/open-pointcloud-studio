# Open Pointcloud Studio feature promo

The silent MP4 uses fourteen desktop captures from the native Rust v0.9.5 build. Most scenes use a public 46.6-million-point E57 sample with nine scan stations. It opens on the single Start ribbon, demonstrates the controls beneath the view cube, then shows point selection, measuring, the section box, mesher settings, 2D plan and section, an E57 station photo, side-by-side photo comparison, and responsive groups. The screenshots are unaltered inside a white promo frame.

The comparison scene uses `manitouNoInvalidPoints.e57` from the [official E57 example data](https://e57-3d-imgfmt.sourceforge.net/data.html). The photographed Manitou scene is © 2012 Calabi Yau Systems LLC; the credit is shown in the video.

Capture the scenes from a running build with that sample open and indexed, and with its two drawings `Lobby plan - fill` and `Lobby section` saved, using its process ID and X11 window ID:

```sh
python3 promo/capture.py PID WINDOW_ID
```

Render it with Python 3, Pillow and FFmpeg:

```sh
python3 promo/render.py --ffmpeg /path/to/ffmpeg
```

The result is `promo/open-pointcloud-studio-features.mp4` (1920×1080, H.264, silent). Temporary slides and clips go in `promo/.work/`.
