## Downloads

- **Windows** 10 and 11, x64: `open-pointcloud-studio_@VERSION@_x64-setup.exe` installs for the current user without administrator rights, or for all users. `open-pointcloud-studio_@VERSION@_windows-x64.zip` is the same application without an installer. @WINDOWS_SIGNING@
- **macOS** 11 and later, arm64 and x86-64 processors in one file: open `open-pointcloud-studio_@VERSION@_macos-universal.dmg` and drag the application to Applications. @MACOS_SIGNING@ `open-pointcloud-studio_@VERSION@_macos-universal.tar.gz` holds the command-line binary without the application bundle, with Open CAD Studio beside it.
- **Linux** x86-64: `open-pointcloud-studio_@VERSION@_amd64.deb` for Debian, Ubuntu and their relatives, `open-pointcloud-studio_@VERSION@_amd64.AppImage` for any distribution, or `open-pointcloud-studio_@VERSION@_linux-amd64.tar.gz` with the bare binary and Open CAD Studio beside it.
- **Linux** 64-bit ARM, experimental: `open-pointcloud-studio_@VERSION@_arm64.deb`, `open-pointcloud-studio_@VERSION@_arm64.AppImage` and `open-pointcloud-studio_@VERSION@_linux-arm64.tar.gz`. They are built and tested on build machines with software rendering, not yet on real boards or laptops.
- **Library** for programs of your own: the point-cloud core as a C library (ABI 1.0), with its header, an example and the notices of the crates it is built from. `open-pointcloud-studio-sdk_@VERSION@_windows-x64-dll.zip` for Windows x64, `open-pointcloud-studio-sdk_@VERSION@_macos-universal-dylib.tar.gz` for macOS 11 and later on both processor types, `open-pointcloud-studio-sdk_@VERSION@_linux-amd64-so.tar.gz` and `open-pointcloud-studio-sdk_@VERSION@_linux-arm64-so.tar.gz` for Linux with a C library of version 2.35 or newer. It is licensed LGPL-3.0-or-later; `docs/library.md` in the repository describes it.
- **Source of Open CAD Studio**: `@CAD_SOURCE@` holds the source that the Open CAD Studio in every package was built from, and `@CAD_VENDOR@` the crates it was built with from git repositories; see Licence below.

Every file has a `.sha256` beside it. Check a download with `sha256sum -c FILE.sha256` on Linux, `shasum -a 256 -c FILE.sha256` on macOS, or compare the output of `Get-FileHash FILE` on Windows.

The Linux packages also carry a build attestation, kept by GitHub. With the GitHub CLI, signed in with `gh auth login`, `gh attestation verify FILE --repo OpenAEC-Foundation/open-pointcloud-studio --signer-workflow OpenAEC-Foundation/open-pointcloud-studio/.github/workflows/release.yml` checks that the release workflow of this project attested the file and that it has not been changed since; the output names the tag the workflow ran for.

### First start on macOS

@MACOS_FIRST_START@

### What Linux needs

A C library of version 2.35 or newer and the C++ runtime of GCC 12 or newer, which Open CAD Studio uses (Ubuntu 22.04, Debian 12, Fedora 36 or later have both), X11 or Wayland, and a Vulkan driver or OpenGL ES 3 through EGL. File dialogs use the desktop portal, or `zenity` where there is none. The `.deb` installs what it needs.

A downloaded AppImage is not executable yet. Make it so once, with `chmod +x open-pointcloud-studio_@VERSION@_amd64.AppImage` or in the file manager under Properties > Permissions, and then start it. It needs FUSE to mount itself (`fusermount3` or `fusermount`); without it, start it as `./open-pointcloud-studio_@VERSION@_amd64.AppImage --appimage-extract-and-run`.

### Licence

The desktop application is GPL-3.0-only, the point-cloud core LGPL-3.0-or-later. Every package carries the licence texts.

Every package also carries Open CAD Studio, the open-source CAD application by Hakan Seven and its contributors, which shows exported DXF and DWG drawings; it does not have to be installed separately. It is licensed GPL-3.0 and was built without changes from commit `@CAD_COMMIT@` (@CAD_DATE@) of @CAD_URL@. `@CAD_SOURCE@` on this page holds the source of that commit, and `@CAD_VENDOR@` the crates it takes from git repositories rather than from crates.io, each under its own licence; unpacked in one folder, the two build it. `OpenCADStudio-NOTICE.txt` in the packages names those crates with their licences and says the same.

### Nederlands

Kies het bestand voor je systeem uit de lijst hierboven: het bestand op `_x64-setup.exe` voor Windows, het `.dmg`-bestand voor macOS, en voor Linux het `.deb`-bestand (Debian, Ubuntu) of de `.AppImage` (elke distributie). De pakketten voor Linux op 64-bits ARM zijn experimenteel.

Voor eigen programma's is de puntenwolk-kern er ook als C-bibliotheek, met header, voorbeeld en de vermeldingen van de crates waaruit hij is gebouwd: `open-pointcloud-studio-sdk_@VERSION@_windows-x64-dll.zip` voor Windows, `open-pointcloud-studio-sdk_@VERSION@_macos-universal-dylib.tar.gz` voor macOS, en `open-pointcloud-studio-sdk_@VERSION@_linux-amd64-so.tar.gz` en `open-pointcloud-studio-sdk_@VERSION@_linux-arm64-so.tar.gz` voor Linux. De bibliotheek valt onder de LGPL-3.0-or-later; `docs/library.md` in de repository beschrijft hem.

Elk pakket bevat ook Open CAD Studio, dat geëxporteerde DXF- en DWG-tekeningen toont; het hoeft niet apart geïnstalleerd te worden. Open CAD Studio valt onder de GPL-3.0 en is zonder wijzigingen gebouwd uit commit `@CAD_COMMIT@` (@CAD_DATE@) van @CAD_URL@. De broncode van die commit staat op deze pagina als `@CAD_SOURCE@`, en de crates uit git-repository's waarmee het is gebouwd, elk onder een eigen licentie, als `@CAD_VENDOR@`.

De Linux-pakketten hebben ook een build-attestatie, bewaard door GitHub. Met de GitHub CLI, aangemeld met `gh auth login`, controleert `gh attestation verify BESTAND --repo OpenAEC-Foundation/open-pointcloud-studio --signer-workflow OpenAEC-Foundation/open-pointcloud-studio/.github/workflows/release.yml` dat de release-workflow van dit project het bestand heeft geattesteerd en dat het daarna niet is veranderd; de uitvoer noemt de tag waarvoor de workflow liep.

Een gedownloade AppImage is nog niet uitvoerbaar. Maak het bestand één keer uitvoerbaar met `chmod +x open-pointcloud-studio_@VERSION@_amd64.AppImage`, of in de bestandsbeheerder onder Eigenschappen > Rechten, en start het daarna.

@MACOS_FIRST_START_NL@
