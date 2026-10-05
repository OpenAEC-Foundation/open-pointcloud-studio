# Packaging

What turns the built binary into the files of a release, and the tests that
install and start each of them. The workflows in `.github/workflows` only
call the scripts here, so every step can be read and, where the tools exist,
run on a developer machine.

## What a release holds

`expected-assets.sh NUMBER` prints the list; each file has a `.sha256` beside
it.

| File | For |
| --- | --- |
| `open-pointcloud-studio_X.Y.Z_x64-setup.exe` | Windows 10 and 11: installer, built from [`../installer/windows.iss`](../installer/windows.iss) |
| `open-pointcloud-studio_X.Y.Z_windows-x64.zip` | Windows: the application and the licence texts |
| `open-pointcloud-studio_X.Y.Z_macos-universal.dmg` | macOS 11 and later, arm64 and x86-64 in one application bundle |
| `open-pointcloud-studio_X.Y.Z_macos-universal.tar.gz` | macOS: the universal binary and the licence texts |
| `open-pointcloud-studio_X.Y.Z_amd64.deb`, `_arm64.deb` | Debian, Ubuntu and their relatives |
| `open-pointcloud-studio_X.Y.Z_amd64.AppImage`, `_arm64.AppImage` | any Linux distribution with a C library of version 2.35 or newer |
| `open-pointcloud-studio_X.Y.Z_linux-amd64.tar.gz`, `_linux-arm64.tar.gz` | Linux: the binary and the licence texts |

The endings `_x64-setup.exe`, `.dmg`, `.AppImage` and `_amd64.deb` are what
the download buttons of the foundation's website look for. That script takes
the first match in name order, which is why the processor types are written
`amd64` and `arm64`: the x86-64 AppImage has to sort first.

The 64-bit ARM packages for Linux are experimental: they are built and
tested on build machines with software rendering only.

## Files

- `version.sh` reads the version from `native/Cargo.toml`, the only place it
  is written, and prints it in the forms the packages need.
- `make-icons.sh` draws the tile icon from `../assets/icons/logo.svg` and
  renders the sizes for Linux and macOS.
- `build-archive.sh` packs the binary with the licence texts; the Windows
  installer is built from the folder it leaves behind.
- `linux/` has the desktop entry, the file types for the shared MIME
  database, the AppStream metadata and the control and copyright files of the
  `.deb`. `stage-tree.sh` lays them out under `usr/`, and `build-deb.sh` and
  `build-appimage.sh` both pack that tree. `check-binary.sh` holds the binary
  to the C library baseline; `check-file-types.sh` asks an installed system
  whether it recognises the files.
- `macos/` has the `Info.plist` of the bundle with its document types, the
  note about the first start that goes into the disk image, and
  `build-app.sh`, which assembles, signs ad hoc and packs the bundle.
- `smoke-test.sh` tests a binary without a window; `gui-smoke.py` starts a
  window through the MCP server and looks at a screenshot. Both write their
  own test data: a grid of 1000 points.
- `release-notes.sh` cuts the section of a version out of
  [`../../CHANGELOG.md`](../../CHANGELOG.md) and appends
  `release-notes-footer.md`; `draft-release.sh` creates the draft release
  those notes go into, at the commit the packages were built from.
- `test-scripts.sh` tests these scripts with stand-ins for the programs they
  call; it needs nothing installed and runs first in the Packages workflow.
- `NOTICE.txt` travels with every package.

## What the application has to provide

The packages rely on four things in the desktop crate:

- `--version` prints `open-pointcloud-studio X.Y.Z` and `--help` prints the
  command-line modes, naming at least `--export` and `--mcp`; both exit with
  success without opening a window. `smoke-test.sh` checks them.
- On Linux the window announces the application id
  `org.openaec.OpenPointcloudStudio`, the name of the desktop entry. A
  desktop shell uses it to give the window its icon and to tie it to the
  launcher.
- On macOS the application opens the files and folders that the system hands
  over when a document is opened with it; they do not arrive on the command
  line. The bundle declares E57, LAS, LAZ, PLY, PCD, PTX, PTS and scan
  project files, and folders.
- The MCP server started from an AppImage starts windows through the file
  named by `$APPIMAGE`, not through its own path inside the mounted image,
  which is gone when the server ends.

## Trying the packaging

The workflow **Packages** (`.github/workflows/packages.yml`) builds and tests
everything and publishes nothing; start it by hand from the Actions page for
any branch. Its packages and the screenshots of the window tests are kept as
artifacts of the run. For a commit from before `--version` and `--help`
existed, tick *Skip the --version and --help checks*.

On a developer machine, with a built binary:

```bash
bash native/packaging/test-scripts.sh                      # needs no binary
bash native/packaging/smoke-test.sh native/target/release/open-pointcloud-studio 0.8.0
bash native/packaging/make-icons.sh /tmp/icons            # needs rsvg-convert
bash native/packaging/linux/build-deb.sh native/target/release/open-pointcloud-studio \
    /tmp/icons 0.8.0 0.8.0 2026-10-10 /tmp/packages       # needs dpkg-deb, desktop-file-validate
python3 native/packaging/gui-smoke.py native/target/release/open-pointcloud-studio
```

## Releasing

1. Set the version in `native/Cargo.toml` (`[workspace.package] version`) and
   run `cargo check` in `native/` so that `Cargo.lock` follows.
2. In `CHANGELOG.md`, give the *Unreleased* section its heading
   `## X.Y.Z - YYYY-MM-DD` and start a new empty *Unreleased* above it.
3. Run the **Packages** workflow on that commit and look at its screenshots.
4. Commit, tag `vX.Y.Z` and push the tag. The release workflow refuses a tag
   that differs from the version or has no changelog section, runs the tests
   on all three systems and the Packages workflow, and only then creates the
   release with all files.
5. Tell the website work when file names change: its download buttons and
   its release notes read the release.

The release workflow gives every Linux package a build attestation
(`actions/attest`) before it creates the release; the release notes and the
README say how to check one with `gh attestation verify` and
`--signer-workflow`, which a bare `--repo` would not check.

Not done here, and said in the release notes: the macOS bundle is signed ad
hoc and not notarised, so the first start has to be allowed by hand, and the
Windows files are signed only when the signing service is configured.
