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
| `open-pointcloud-studio_X.Y.Z_windows-x64.zip` | Windows: the application, Open CAD Studio and the licence texts |
| `open-pointcloud-studio_X.Y.Z_macos-universal.dmg` | macOS 11 and later, arm64 and x86-64 in one application bundle |
| `open-pointcloud-studio_X.Y.Z_macos-universal.tar.gz` | macOS: the universal binaries of the application and of Open CAD Studio, and the licence texts |
| `open-pointcloud-studio_X.Y.Z_amd64.deb`, `_arm64.deb` | Debian, Ubuntu and their relatives |
| `open-pointcloud-studio_X.Y.Z_amd64.AppImage`, `_arm64.AppImage` | any Linux distribution with a C library of version 2.35 or newer and the C++ runtime of GCC 12 or newer |
| `open-pointcloud-studio_X.Y.Z_linux-amd64.tar.gz`, `_linux-arm64.tar.gz` | Linux: the binaries of the application and of Open CAD Studio, and the licence texts |
| `open-cad-studio-source_SHORT.tar.gz` | the source of the Open CAD Studio that every package carries: every file of the pinned commit, `SHORT` being its first eight characters |

Every package carries Open CAD Studio, the CAD program that shows exported
drawings: `OpenCADStudio` or `OpenCADStudio.exe` beside the application in
the installer, the archives and the macOS bundle, and in
`/usr/lib/open-pointcloud-studio` in the `.deb` and the AppImage. Its source
is not part of this repository. [`open-cad-studio.pin`](open-cad-studio.pin)
names the one commit of its repository that is built: the URL, the full hash
of the commit, the hash of its tree and its date, and nothing else. Every
package carries its licence text (`OpenCADStudio-LICENSE.txt`, the GPL-3.0
text) and a notice (`OpenCADStudio-NOTICE.txt`) that names the commit and
says that its source is on the release page of that version, as
`open-cad-studio-source_SHORT.tar.gz`; the GPL asks that whoever gets the
program can get its source from the same place. The `.deb` is the exception
for the licence text, as for those of the application: its notice points at
`/usr/share/common-licenses/GPL-3`, which every Debian-based system has.

The endings `_x64-setup.exe`, `.dmg`, `.AppImage` and `_amd64.deb` are what
the download buttons of the foundation's website look for. That script takes
the first match in name order, which is why the processor types are written
`amd64` and `arm64`: the x86-64 AppImage has to sort first. The name of the
source archive ends in none of these endings, so no button offers it.

The 64-bit ARM packages for Linux are experimental: they are built and
tested on build machines with software rendering only.

## Files

- `version.sh` reads the version from `native/Cargo.toml`, the only place it
  is written, and prints it in the forms the packages need.
- `make-icons.sh` draws the tile icon from `../assets/icons/logo.svg` and
  renders the sizes for Linux and macOS.
- `build-open-cad-studio.sh` fetches the commit that `open-cad-studio.pin`
  names into `native/target/open-cad-studio/source`, refuses to go on unless
  its tree is the pinned tree, its `Cargo.lock` names one commit for every git
  dependency and its licence is the GPL-3.0 text, and builds it unchanged with
  `cargo build --locked` into `native/target/open-cad-studio`; other options
  go to `cargo build`. `--pin` prints the pin, `--fetch` only fetches and
  checks. Reading the pin and fetching and checking the source is in
  `open-cad-studio-source.sh`, which `common.sh` sources; a change there or in
  the build script builds the program again in the workflow, a change to the
  other scripts does not.
- `archive-open-cad-studio-source.sh` writes the two source archives of the
  release from the same checkout: the commit, and the crates from git
  repositories that `cargo vendor` writes into
  `native/target/open-cad-studio/vendor`, which a later run for the same
  commit uses again. It refuses to write them while the notices do not name
  those crates, or a crate under a copyleft licence, and checks that the two
  unpacked together resolve without the git repositories.
- `check-open-cad-studio.sh` checks in an installed or unpacked package that
  Open CAD Studio lies where the application looks for it, starts, and
  converts a small DXF file without a window. `OpenCADStudio-NOTICE.txt.in` is
  its notice.
- `build-archive.sh` packs the binary with Open CAD Studio and the licence
  texts; the Windows installer is built from the folder it leaves behind.
  For Windows it takes Open CAD Studio built with the MSVC toolchain, as the
  workflow builds it: a build with the GNU toolchain loads the C++ runtime of
  MinGW, which no package carries, and is refused here and by
  `check-open-cad-studio.sh`.
- `linux/` has the desktop entry, the file types for the shared MIME
  database, the AppStream metadata and the control and copyright files of the
  `.deb`. `stage-tree.sh` lays them out under `usr/`, and `build-deb.sh` and
  `build-appimage.sh` both pack that tree. `check-binary.sh` holds a binary
  to the C library baseline, and Open CAD Studio also to that of the C++
  runtime; `check-file-types.sh` asks an installed system whether it
  recognises the files.
- `macos/` has the `Info.plist` of the bundle with its document types, the
  note about the first start that goes into the disk image, and
  `build-app.sh`, which assembles, signs ad hoc and packs the bundle.
- `smoke-test.sh` tests a binary without a window; `gui-smoke.py` starts a
  window through the MCP server and looks at a screenshot. Both write their
  own test data: a grid of 1000 points.
- In the Packages workflow, the job `open-cad-studio-pin` reads the pin file
  with `build-open-cad-studio.sh --pin`, fetches and checks the pinned commit
  with `--fetch` and writes the two source archives, which the release takes
  as the artifact `package-open-cad-studio-source`. It keeps the checkout and
  the vendored crates in the cache under the pinned commit, so that a later
  run fetches nothing from the repository of Open CAD Studio or those of its
  git dependencies, and hands the commit on with `--export` as the artifact
  `open-cad-studio-commit`. The job `open-cad-studio` builds the program for
  each system and processor from that artifact (`OCS_FETCH_FROM`) and from
  the crates of the vendor archive (`OCS_VENDOR_DIR`), so the programs are
  built from exactly what the release page carries, and keeps each in the
  cache under all that decides it: the target, the pinned commit, the Rust
  toolchain, the system image with its C and C++ compiler (and SDK on macOS),
  the oldest macOS it is built for, and the hash of
  `open-cad-studio-source.sh` and `build-open-cad-studio.sh`. It is built
  again only when one of them changes; a build with an empty cache takes the
  longest part of a run. GitHub removes a cache that has not been used for
  seven days, so a run after a quiet week or after a new pin needs those
  repositories again; `OCS_FETCH_FROM` can then name a mirror. The macOS job
  joins the two halves with `lipo`, as it does for the application, and
  every package is checked with `check-open-cad-studio.sh` after it is
  installed or unpacked.
- `release-notes.sh` cuts the section of a version out of
  [`../../CHANGELOG.md`](../../CHANGELOG.md) and appends
  `release-notes-footer.md`; `draft-release.sh` creates the draft release
  those notes go into, at the commit the packages were built from, or takes
  over the draft of an earlier run. After the upload,
  `prune-release-assets.sh` removes from the draft every file that
  `expected-assets.sh` does not list, so that a file of an earlier run whose
  name has changed since, such as a source archive of Open CAD Studio for a
  commit pinned before, is not published.
- `test-scripts.sh` tests these scripts with stand-ins for the programs they
  call, and fetches from a small repository of its own in place of that of
  Open CAD Studio; it needs nothing installed besides git and runs first in
  the Packages workflow.
- `NOTICE.txt` travels with every package.

## What the application has to provide

The packages rely on five things in the desktop crate:

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
- The application looks for Open CAD Studio beside its executable and in
  `../lib/open-pointcloud-studio` from the folder of the executable, and
  starts it from a copy in the cache folder when it lies in a mounted
  AppImage (`cad_viewer.rs`).

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
bash native/packaging/build-open-cad-studio.sh -j 4        # native/target/open-cad-studio/release
bash native/packaging/build-open-cad-studio.sh --target x86_64-pc-windows-msvc   # for a Windows package
bash native/packaging/archive-open-cad-studio-source.sh /tmp/packages
bash native/packaging/make-icons.sh /tmp/icons            # needs rsvg-convert
bash native/packaging/linux/build-deb.sh native/target/release/open-pointcloud-studio \
    native/target/open-cad-studio/release/OpenCADStudio \
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

To move to another commit of Open CAD Studio, write its URL, full hash, tree
hash (`git rev-parse COMMIT^{tree}`) and date (`git log -1 --format=%cs
COMMIT`) into `open-cad-studio.pin`, build it with
`build-open-cad-studio.sh`, open an exported DXF from a development build of
the application, read the changes of its licence and of its git
dependencies, and add a line to `CHANGELOG.md`. The script refuses a pin
whose hashes or date do not fit what it fetched. Then run
`archive-open-cad-studio-source.sh`: it names the git repositories, commits
and crates that `OpenCADStudio-NOTICE.txt.in` has to list for the new
commit, and the crates under a copyleft licence that `NOTICE.txt` has to
name, and writes nothing until both do.

The release workflow gives every Linux package a build attestation
(`actions/attest`) before it creates the release; the release notes and the
README say how to check one with `gh attestation verify` and
`--signer-workflow`, which a bare `--repo` would not check.

Not done here, and said in the release notes: the macOS bundle is signed ad
hoc and not notarised, so the first start has to be allowed by hand, and the
Windows files are signed only when the signing service is configured.
