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

Run the desktop probe in a disposable window. `run.py` creates isolated
configuration/cache directories, waits for the GUI command path, supplies its
discovery token privately to the validator and closes the window even on failure.

```sh
python3 run.py /path/to/open-pointcloud-studio /path/to/working-directory --static /path/to/ManitouNoInvalidPoints.e57
```

For an already isolated empty test window, the lower-level `validate.py` takes
its instance JSON as the third argument after directory and binary. Never commit
or print that JSON: it contains the local command API token.

The probe refuses a window that already has loaded clouds. It checks the 471
selected records against independent source values, the six original images,
an added and edited panorama, photo colour export, the seven images and their
scan associations, registration of image positions and orientations, and raw
non-colour attributes. The optional static test checks five scan identities,
five image payloads and every non-colour raw point field after recolouring.
Generated JSON records the assertions that passed; failed assertions abort.

## Result, 5 November 2026

`result-2026-11-05.json` records a successful run against the fixed development
binary. The BLK2GO source SHA-256 is
`e01da70d9cc3a2c8a0cf9e3e00f76b2d96895590d12285240b8148c2eb6a5040`.
Its 471-point selection was compared against an independent pass over all
199,186,802 source records. All six original pinhole payloads survive. Adding,
editing and recolouring from a synthetic panorama retains seven images and the
original non-colour point values, including Single-precision floating intensity.
An additional independent prototype comparison verifies unchanged non-colour
field types, precision, limits and scaled-integer scale/offset in all six tested
scans (one BLK2GO section and five static scans).
A rigid registration retains the image bytes and transforms camera positions
and orientations correctly. Rotations are compared as matrices: opposite-sign
quaternions represent the same orientation.

The static fixture matches the [published SourceForge checksum](https://sourceforge.net/projects/e57-3d-imgfmt/files/E57Example-data/manitouNoInvalidPoints.e57/download). Its SHA-256 is
`d4f06e2a80e3e54be7391130b911c82af0c6a7857cec133177431f5f578f23cf`.
After photo colouring, libE57 verified five scans, five unchanged image payloads,
1,095,702 points, unchanged scan poses and exact original non-colour fields.
894,556 points received photo colours; unobserved points are retained. The
Python convenience reader omits the standard `isColorInvalid` field, so the
validator reads that field through libE57's SourceDestBuffer API, checks its
validity domain and refuses any other unsupported field instead of skipping it.

The complete BLK2GO section command took 9m31.31s with peak RSS 66,572 KiB on a
shared Ryzen 7 3700U Linux workstation. This is a full-source **section export**
measurement; recolouring was tested on the extracted selection and the static
fixture. It is not a 199-million-point GUI colour benchmark. Original vendor
pinhole images and the deliberately added test panorama are reported separately;
this does not establish a vendor route-panorama trajectory calibration test.

All 568 core unit tests passed (3 ignored), including all 28 export tests. The
raw override applies to positive uniform scales/translations. Nonuniform or
reflected transforms use the existing generic path, which rejects E57 photo
poses that cannot be represented; this result does not claim arbitrary affine
camera support or preservation of every vendor extension/grouping structure.
