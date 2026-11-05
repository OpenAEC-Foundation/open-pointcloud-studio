# E57 photographs and raw field validation

Install Python 3 with `numpy`, `scipy`, `pye57` and `Pillow`. pye57 uses the C++
libE57 implementation, independently of the application's Rust E57 library.
Use a dedicated working directory: the scripts generate fixtures, photo-edit
sidecars and exports there. Original vendor downloads are never committed.

## Public vendor fixtures

- [Scanner2GO BLK2GO sample](https://support.scanner2go.de/hc/de/articles/18239199476369-Download-Beispiele-BLK2GO-f%C3%BCr-Import-CAD):
  [direct E57](https://demo.scanner2go.de/2024/LeicaBLK2GO/Sanierungsobjekt_Demo02/BLK2GO_Demo02_Sanner2GO.e57).
  Save as `BLK2GO_Demo02.e57`. It contains 199,186,802 points and six pinhole
  station images. These are real SLAM data; the additional panorama in the
  editing probe is deliberately synthetic and is reported separately.
- [libE57 example data](https://e57-3d-imgfmt.sourceforge.net/data.html),
  Calabi Yau Systems ManitouNoInvalidPoints: five Riegl Z420i static scans with
  five spherical images. Copyright Calabi Yau Systems LLC; download separately.

Create the small field-test section with the application:

```sh
open-pointcloud-studio --section BLK2GO_Demo02.e57 -5.1,-30,-1.5,-5.06,-20,-0.01 section.e57
python3 prepare.py /path/to/working-directory
```

`prepare.py` independently streams the entire original source in 500,000-record
chunks and saves the expected selection's original XYZ, intensity and RGB. It
also hashes the original Images2D blobs. This validates the section against the
actual vendor data, rather than merely comparing an export to itself.

Start a fresh test window with isolated XDG_CONFIG_HOME and XDG_CACHE_HOME, then
pass that window's instance JSON from its configuration `instances` directory.
Never commit or print that JSON: it contains the local command API token.

```sh
python3 validate.py /path/to/working-directory /path/to/open-pointcloud-studio /path/to/instance-PID.json --static /path/to/ManitouNoInvalidPoints.e57
```

The probe refuses a window that already has loaded clouds. It checks the 471
selected records against independent source values, the six original images,
an added and edited panorama, photo colour export, the seven images and their
scan associations, registration of image positions and orientations, and raw
non-colour attributes. The optional static test checks five scan identities,
five image payloads and every non-colour raw point field after recolouring.
Generated JSON records the assertions that passed; failed assertions abort.
