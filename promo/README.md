# Open Pointcloud Studio feature promo

The silent MP4 shows fourteen captures of the whole application window, in the English interface and the theme Light, each inside a white frame with a title. The captures are unaltered apart from their scale. They come from the development build after 0.9.5, which still reads v0.9.5 in its title bar and status bar, and show nothing but the two generated scenes of the [demo scene generator](../native/TEST_DATA.md#generated-demo-scenes):

- **`building.laz`**, a house on its plot with a street, trees and a shed (24.2 million points): the window with its File and Home tabs, Project Browser and Properties and the tabs of a plan, a section and a saved view; elevation colours and the detail controls of the Display panel beneath the view cube; a selection of the roof; an area measured on the roof; the ribbon folding its groups in a narrow window; and the Export page of the File view.
- **`rooms.e57`**, two furnished rooms scanned from two stations, each with its panorama (6.5 million points): a plan with its filled cut and straight CAD lines, a section through both rooms, the section box with the ceiling taken off, the photo of Station 2 in its own tab and beside the 3D model, the four methods of Mesh Pointcloud, and a closed mesh in the colours of the scan.

| Still | Scene |
| --- | --- |
| `01-overview.png` | The house in the window, with the tabs Ground floor, Section A and Front garden; also the poster |
| `02-selection.png` | 2.6 million roof points selected |
| `03-section.png` | The rooms in a section box below the ceiling |
| `04-mesher.png` | Mesh Pointcloud on its Method step |
| `05-mesher-options.png` | The closed mesh of the rooms, coloured, without the points |
| `06-plan.png` | The drawing Plan with its fill and straight CAD lines |
| `07-section-2d.png` | The drawing Section A through both rooms |
| `08-panorama.png` | The photo of Station 2 in its own tab |
| `09-responsive.png` | The ribbon in a window 920 pixels wide |
| `10-view.png` | The Export page of the File view |
| `11-elevation.png` | Elevation colours and the Display panel |
| `12-point-detail.png` | Larger points and Eye-dome in the Display panel |
| `13-measure.png` | The area of the south face of the roof |
| `14-photo-compare.png` | The photo of Station 2 beside the 3D model |

## Making the scenes

Write the two scenes with the demo scene generator, from `native/`:

```sh
cargo run -p pointcloud-core --release --example demo_scenes -- SCENES
```

## Capturing the stills

`capture.py` drives a window of the application through its [local command API](../native/API.md), makes the drawings and the saved view it shows, and writes the stills to `promo/stills/` at 960 pixels high, the height at which the video shows them. It needs Python 3 and Pillow.

On Windows it starts a window of its own, off-screen, with a temporary `APPDATA` and `LOCALAPPDATA`, so the settings, views and recent files of the user stay as they are, and closes it again; `--data DIR` keeps those folders, and the index of the scans, for a second run:

```sh
python promo/capture.py SCENES --exe PATH\TO\open-pointcloud-studio.exe
```

On Linux, start a window without a scan open and pass its process ID and X11 window ID; the script sizes it with `wmctrl` and parks the pointer in it with `python-xlib`. On Windows `--pid` drives a window that is already open in the same way. Either way the window is switched to English and the theme Light, and keeps them:

```sh
python3 promo/capture.py SCENES --pid PID --window WINDOW_ID
```

## Rendering

Render the video and the poster with Python 3, Pillow and FFmpeg:

```sh
python3 promo/render.py --ffmpeg /path/to/ffmpeg
```

The result is `promo/open-pointcloud-studio-features.mp4` (1920×1080, H.264, 24 frames per second, silent, about 55 seconds) and `promo/poster.png`, the overview scene with its frame. Temporary slides and clips go in `promo/.work/`.
