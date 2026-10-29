# Packaging

What turns the built binary into the files of a release, and the tests that
install and start each of them. The workflows in `.github/workflows` only
call the scripts here, so every step can be read and, where the tools exist,
run on a developer machine.

## What a release holds

`expected-assets.sh NUMBER` prints the list; each file has a `.sha256` beside
it. `YYYY.W.N` is the version, such as `2026.43.0` (see
[Releasing](#releasing)).

| File | For |
| --- | --- |
| `open-pointcloud-studio_YYYY.W.N_x64-setup.exe` | Windows 10 and 11: installer, built from [`../installer/windows.iss`](../installer/windows.iss) |
| `open-pointcloud-studio_YYYY.W.N_windows-x64.zip` | Windows: the application, Open CAD Studio and the licence texts |
| `open-pointcloud-studio_YYYY.W.N_macos-universal.dmg` | macOS 11 and later, arm64 and x86-64 in one application bundle |
| `open-pointcloud-studio_YYYY.W.N_macos-universal.tar.gz` | macOS: the universal binaries of the application and of Open CAD Studio, and the licence texts |
| `open-pointcloud-studio_YYYY.W.N_amd64.deb`, `_arm64.deb` | Debian, Ubuntu and their relatives |
| `open-pointcloud-studio_YYYY.W.N_amd64.AppImage`, `_arm64.AppImage` | any Linux distribution with a C library of version 2.35 or newer and the C++ runtime of GCC 12 or newer |
| `open-pointcloud-studio_YYYY.W.N_linux-amd64.tar.gz`, `_linux-arm64.tar.gz` | Linux: the binaries of the application and of Open CAD Studio, and the licence texts |
| `open-pointcloud-studio-sdk_YYYY.W.N_windows-x64-dll.zip`, `_macos-universal-dylib.tar.gz`, `_linux-amd64-so.tar.gz`, `_linux-arm64-so.tar.gz` | the point-cloud core as a C library, with its header and example ([`../sdk`](../sdk), [`../../docs/library.md`](../../docs/library.md)) |
| `open-cad-studio-source_SHORT.tar.gz` | the source of the Open CAD Studio that every package carries: every file of the pinned commit, `SHORT` being its first eight characters |
| `open-cad-studio-vendor_SHORT.tar.gz` | the crates that commit takes from git repositories, vendored |
| `SHA256SUMS` | the checksums of all the files above in one list; it has no `.sha256` of its own |
| `SHA256SUMS.asc`, `open-pointcloud-studio_YYYY.W.N_amd64.AppImage.asc`, `_arm64.AppImage.asc` | OpenPGP signatures, once the organisation has its key (see [Signatures](#signatures)) |
| `open-pointcloud-studio_YYYY.W.N_verification.md` | added after the release is public: the record of the checks of the published files ([Verification record](#verification-record)) |

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

The image of the cloud API is not a file of the release page: it is
`ghcr.io/openaec-foundation/open-pointcloud-studio-api:YYYY.W.N` in the
registry of the organisation, and the notes give its digest
([The image of the cloud API](#the-image-of-the-cloud-api)).

## Files

- `set-version.sh` works out the version of the next release (`next`),
  writes a version into `native/Cargo.toml` and the entries of
  `native/Cargo.lock`, and with `--changelog` the heading of its section in
  `CHANGELOG.md` (`set`), and checks that those agree (`check`), which the
  CI workflow does for every commit. It numbers only the members of the
  workspace with `version.workspace = true`: the `ui` crate, a copy of the
  shared OpenAEC crate, keeps the version of that crate.
- `version.sh` reads the version from `native/Cargo.toml`, the only place it
  is written, checks it with `set-version.sh check` and prints it in the
  forms the packages need.
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
- `signature-assets.sh` names the files of a release besides those of
  `expected-assets.sh`: `SHA256SUMS`, and the OpenPGP signatures when they
  are kept. `sign-linux.sh` writes those signatures with the organisation's
  key, and signs nothing while the key is not configured.
- `mcp-client-check.py` checks an MCP server from the outside with the
  reference Python SDK of the protocol, pinned with hashes in
  `mcp-client-requirements.txt`: `--stdio` the server of the installed
  application (the `verify-linux` job, under a virtual display), `--http`
  the route `/mcp` of the cloud API (the Container workflow). Each writes a
  record to the summary of its job.
- `test-scripts.sh` tests these scripts with stand-ins for the programs they
  call, and fetches from a small repository of its own in place of that of
  Open CAD Studio; it needs nothing installed besides git and runs first in
  the Packages workflow.
- `NOTICE.txt` travels with every package.

## What the application has to provide

The packages rely on five things in the desktop crate:

- `--version` prints `open-pointcloud-studio YYYY.W.N` and `--help` prints the
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
bash native/packaging/smoke-test.sh native/target/release/open-pointcloud-studio 2026.43.0
bash native/packaging/build-open-cad-studio.sh -j 4        # native/target/open-cad-studio/release
bash native/packaging/build-open-cad-studio.sh --target x86_64-pc-windows-msvc   # for a Windows package
bash native/packaging/archive-open-cad-studio-source.sh /tmp/packages
bash native/packaging/make-icons.sh /tmp/icons            # needs rsvg-convert
bash native/packaging/linux/build-deb.sh native/target/release/open-pointcloud-studio \
    native/target/open-cad-studio/release/OpenCADStudio \
    /tmp/icons 2026.43.0 2026-10-24 /tmp/packages         # needs dpkg-deb, desktop-file-validate
python3 native/packaging/gui-smoke.py native/target/release/open-pointcloud-studio
```

## Releasing

A version is `YYYY.W.N`: the ISO week-year and the ISO week of the release,
the week without a leading zero, and a sequence number from 0 within that
week, such as `2026.43.0`. A second release in that week is `2026.43.1`, the
first of the week after `2026.44.0`. The commit of a release sets its
version, so `main` has the version of the last release and never a week
ahead.

1. On the day of the release, run

   ```bash
   bash native/packaging/set-version.sh set next --changelog "$(date -u +%F)"
   ```

   It takes the week of that day in UTC and one more than the highest
   sequence number of that week among the tags and the releases on GitHub,
   drafts included, so `gh` has to be signed in (`--offline` counts the
   local tags only). It writes the version into `native/Cargo.toml` and
   `native/Cargo.lock`, and gives the *Unreleased* section of `CHANGELOG.md`
   its heading `## YYYY.W.N - YYYY-MM-DD` with a new empty *Unreleased*
   above it; it writes nothing while that section has no item.
2. Commit as `Release vYYYY.W.N`, run the **Packages** workflow on that
   commit and look at its screenshots.
3. Tag `vYYYY.W.N` and push the tag, or start the release workflow by hand;
   without a version it takes the next one of this week, which has to be the
   version of the commit. A draft that a failed run left behind counts as a
   release of that week, so give its tag to run again over it. The release
   workflow refuses a tag that differs from the version or has no changelog
   section, runs the tests on all three systems and the Packages workflow,
   and only then creates the release with all files.
4. Tell the website work when file names change: its download buttons and
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

The release workflow gives every Linux package and every Linux archive of
the C library a build attestation (`actions/attest`) before it creates the
release; the release notes and the README say how to check one with `gh
attestation verify` and `--signer-workflow`, which a bare `--repo` would
not check.

Besides the Packages workflow, the release workflow runs the SDK workflow
(`sdk.yml`, the four archives of the C library) and the Container workflow
(`container.yml`, the image of the cloud API, smoke-tested and kept as the
artifact `api-image`); a release is published only when all of them pass. A
draft run (*Leave the release as a draft*) does everything but push the
image and publish; it is the dry run of a release.

### Signatures

`SHA256SUMS` is published with every release: `sha256sum --ignore-missing -c
SHA256SUMS` checks every file downloaded beside it.

The Windows application, Open CAD Studio, the installer and the DLL of the C
library are signed when the secrets of the organisation's code-signing
account are configured; the step *Report the signatures* says what each
file carries, with its signer, issuer, thumbprint and timestamp, and the
release notes repeat it.
On a release run, the publish job refuses a release whose application is
signed while its library is not, on Windows or macOS; a draft run only
warns.

The Windows programs and the DLL are built with the static C runtime
(`-C target-feature=+crt-static`, set for the build steps only, so that it
reaches neither other builds under `native/` nor the cache of Open CAD
Studio unseen), and the step *Check the runtime dependencies* fails on a DLL
that a clean Windows does not have.

OpenPGP signatures of `SHA256SUMS` and of the two AppImages follow once the
organisation has its key. `sign-linux.sh` reads the secret signing subkey
from the secret `OPENAEC_GPG_PRIVATE_KEY`, its passphrase from
`OPENAEC_GPG_PASSPHRASE` and the fingerprint from the variable
`OPENAEC_GPG_FINGERPRINT`, refuses a key with another fingerprint, and
verifies each signature it writes. While any of them is missing it signs
nothing and the notes name no signature.

Updates through the OpenAEC Installer
([OpenAEC-Blueprint#33](https://github.com/OpenAEC-Foundation/OpenAEC-Blueprint/issues/33)):
after the Authenticode and OpenPGP signing, `sign-updates.sh` writes a
minisign `.sig` beside the Windows setup and each AppImage, with the updater
key in the secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; without them it signs nothing. The
`.sig` files go on the release (`signature-assets.sh`). Once the release is
public, the job *updater-manifest* makes `latest.json` with the
[release kit](https://github.com/OpenAEC-Foundation/openaec-installer/tree/main/release-kit)
of the installer: it checks every `.sig` against the variable
`UPDATER_PUBKEY` and the tag against `native/Cargo.toml`, and attaches
`latest.json` last. The installer then checks the signature before it
updates silently, with `/VERYSILENT` for this Inno Setup installer.

### The image of the cloud API

The Container workflow builds the image from
[`../../deploy/cloud-api/Dockerfile`](../../deploy/cloud-api/Dockerfile) on
every change to the service, checks it and never pushes it. In a tag run
the release workflow pushes the image of its run to
`ghcr.io/openaec-foundation/open-pointcloud-studio-api` by its digest and
without a tag, once every check has passed, and attests it; the notes give
the digest and how to verify the attestation. Only after the release is
public does the image get the version as its tag. A version whose image
exists already is refused.

### Verification record

After a tag run, the job *Verify the published release* downloads the
published files and the image, as a user would, on Windows, on macOS with
each processor and on Linux, and runs the checks a user can run: the
signatures with `Get-AuthenticodeSignature`, `codesign`, `spctl` and
`stapler`, the runtime dependencies, the application in a clean Windows
container and the packages in clean Linux containers, `sha256sum -c
SHA256SUMS`, `gh attestation verify` of the Linux files and of the image,
and `gpg --verify` once signed. The record of it, with the identities of the
signers, is attached to the release as
`open-pointcloud-studio_YYYY.W.N_verification.md`, with its `.sha256`.

Windows files are signed when the Azure Trusted Signing secrets are configured.
The macOS workflow can sign both universal executables and the app with a
Developer ID Application certificate, notarize the app and disk image with
Apple's `notarytool`, and staple both tickets. It uses the six secrets that the
OpenAEC organisation shares with its app repositories, under the names every
OpenAEC app uses: `APPLE_CERTIFICATE` (the Developer ID Application `.p12` in
base64), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (the full
Developer ID Application identity), `APPLE_API_KEY_CONTENT` (the App Store
Connect `.p8` key, as its text or in base64), `APPLE_API_KEY` (its key ID) and
`APPLE_API_ISSUER` (its issuer ID). The certificate and key are decoded into the
temporary macOS runner only; a partial set fails the job.
With no Apple secrets the workflow retains the ad hoc signature and the
release notes state that notarization was not performed. The signed path must
pass a real release run before its claim can be verified; local Linux script
tests cannot exercise Apple's signing and notary service.
