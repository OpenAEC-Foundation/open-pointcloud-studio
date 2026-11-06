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

## External reader checks, 6 November 2026

`reader_roundtrip.py` verifies an identity section export with pye57/libE57,
PDAL and an E57-enabled CloudCompare. It operates on isolated copies so unrelated
photo-edit sidecars do not affect file equivalence. The original vendor files
remain unchanged. The external in-memory readers are limited to two million
points per fixture by default; the separate large-source probes cover the
199-million-point input.

```sh
python3 reader_roundtrip.py /absolute/path/to/open-pointcloud-studio \
  --pdal /absolute/path/to/pdal \
  --cloudcompare /absolute/path/to/CloudCompare \
  --cloudcompare-version 'the installed E57-enabled build' \
  --source /path/to/section.e57 /path/to/manitou.e57 \
  --output /new/validation/directory
```

Use Python with the dependencies listed above. The successful Linux run used
PDAL 2.10.2 and the upstream CloudComPy 2.13.90 experimental wheel's E57-enabled
CloudCompare executable (advertised as 2.14 beta). The Ubuntu 2.11.3 package
available on this machine had no E57 plugin. Both Qt platform and image-format
plugins must be available; the probe configures their paths for the wheel's
adjacent `plugins` directory. The [official CloudCompare CLI reference](https://cloudcompare.org/doc/wiki/index.php/Command_line_mode),
[PDAL E57 reader contract](https://pdal.io/en/stable/stages/readers.e57.html) and
[upstream wheel download](https://www.simulation.openfields.fr/index.php/cloudcompy-downloads/3-cloudcompy-binaries/13-experimental-pypi-wheels-install-with-pip/14-linux-wheel-files/142-cloudcompy-2-13-90-cp312-cp312-linux-x86-64-whl)
describe the tools. `pdal-linux64-explicit.txt` pins the public conda-forge
packages, and `readers-result-2026-11-06.json` records the executable/plugin/wheel
hashes and Python package versions. No reader binaries or vendor data are
redistributed.

The additional checks exposed two interoperability problems:

- The Riegl intensity maximum is a scalar `ScaledInteger`: raw value 32,767
  with scale 1/32,767 represents **1.0**. e57 0.11.13 discarded the scalar's
  scale/offset when writing limits. Raw Float32 point intensities stayed exact,
  but the exported maximum became 32,767; PDAL read the first normalised
  intensity as 0 instead of 8,190. Reading now resolves scaled or mixed scalar
  pairs to physical doubles, and writing retains the correct physical range.
  Original point records and their prototype encoding remain unchanged.
- Replacing the original sensor index bounds with derived smaller bounds
  changed CloudCompare's interpretation of 873 positions in the last Riegl
  scan, by up to 19.11 m. Raw values, poses and PDAL coordinates still matched.
  Filtered and scan-preserving scene exports now retain the original sensor
  grid frame, including the absence of explicit bounds. The scene path also
  uses the existing complete-packet iterator for valid alignment packets.

The corrected files pass all three external applications:

| Fixture | Point records | Scans | Original photos |
| --- | ---: | ---: | ---: |
| BLK2GO section | 471 | 1 | 6 |
| Riegl Manitou | 1,095,702 | 5 | 5 |

libE57 checks every supported point field and its prototype exactly, physical
intensity/colour limits, index bounds, station poses, image bytes, calibration
and remapped scan associations. PDAL's complete normalised point text and
CloudCompare's complete point text are each byte-identical before and after
export, with equal counts. The recorded pre-fix failures retain the evidence
for both corrections.

The integrations have distinct limits. PDAL exposes common Cartesian point
dimensions and quantises intensity/RGB to uint16; CloudCompare uses float32
coordinates and uint8 RGB. CloudCompare decodes all six pinhole images, and
loads the five Riegl spherical images as ordinary images with an explicit
unsupported-camera message. Their calibration is checked through libE57,
not claimed as a CloudCompare spherical-camera test. All three external
applications share the C++ libE57 family, independently of the native Rust
decoder. This strengthens cross-application validation and does not establish
three independently implemented external decoders or arbitrary vendor
extension/grouping preservation; the broader requirements in #32 remain open.

Existing affected exports need to be regenerated from the original source:
the discarded scale is not safely inferable from the bad export. Cached
normalised points are rebuilt only for sources with corrected scalar limits;
plain-limit E57 files retain their old cache keys, including the full BLK2GO
fixture. Invalid or changed source metadata is a cache miss, leaving error
reporting to the ordinary reader.

Final validation: 1,486 workspace tests passed, none failed and 12 were ignored.
Workspace/all-target Clippy and minimal-core Clippy pass with warnings denied;
Rustfmt, validation-script syntax and diff checks pass.
