## Downloads

- **Windows** 10 and 11, x64: `open-pointcloud-studio_@VERSION@_x64-setup.exe` installs for the current user without administrator rights, or for all users. `open-pointcloud-studio_@VERSION@_windows-x64.zip` is the same application without an installer. @WINDOWS_SIGNING@
- **macOS** 11 and later, arm64 and x86-64 processors in one file: open `open-pointcloud-studio_@VERSION@_macos-universal.dmg` and drag the application to Applications; read the note on the first start below. `open-pointcloud-studio_@VERSION@_macos-universal.tar.gz` holds only the command-line binary.
- **Linux** x86-64: `open-pointcloud-studio_@VERSION@_amd64.deb` for Debian, Ubuntu and their relatives, `open-pointcloud-studio_@VERSION@_amd64.AppImage` for any distribution, or `open-pointcloud-studio_@VERSION@_linux-amd64.tar.gz` with only the binary.
- **Linux** 64-bit ARM, experimental: `open-pointcloud-studio_@VERSION@_arm64.deb`, `open-pointcloud-studio_@VERSION@_arm64.AppImage` and `open-pointcloud-studio_@VERSION@_linux-arm64.tar.gz`. They are built and tested on build machines with software rendering, not yet on real boards or laptops.

Every file has a `.sha256` beside it. Check a download with `sha256sum -c FILE.sha256` on Linux, `shasum -a 256 -c FILE.sha256` on macOS, or compare the output of `Get-FileHash FILE` on Windows.

The Linux packages also carry a build attestation, kept by GitHub. With the GitHub CLI, signed in with `gh auth login`, `gh attestation verify FILE --repo OpenAEC-Foundation/open-pointcloud-studio --signer-workflow OpenAEC-Foundation/open-pointcloud-studio/.github/workflows/release.yml` checks that the release workflow of this project attested the file and that it has not been changed since; the output names the tag the workflow ran for.

### First start on macOS

The application is not signed with a paid developer certificate and not notarised, so macOS refuses to start it the first time. It has to be allowed once.

On macOS 15 and later: start the application, close the message, open System Settings > Privacy & Security, scroll to Security, choose **Open Anyway** and confirm. On macOS 11 to 14: Control-click the application, choose **Open** and confirm. On either, this command does the same:

```
xattr -dr com.apple.quarantine "/Applications/Open Pointcloud Studio.app"
```

### What Linux needs

A C library of version 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or later), X11 or Wayland, and a Vulkan driver or OpenGL ES 3 through EGL. File dialogs use the desktop portal, or `zenity` where there is none. The `.deb` installs what it needs.

A downloaded AppImage is not executable yet. Make it so once, with `chmod +x open-pointcloud-studio_@VERSION@_amd64.AppImage` or in the file manager under Properties > Permissions, and then start it. It needs FUSE to mount itself (`fusermount3` or `fusermount`); without it, start it as `./open-pointcloud-studio_@VERSION@_amd64.AppImage --appimage-extract-and-run`.

### Licence

The desktop application is GPL-3.0-only, the point-cloud core LGPL-3.0-or-later. Every package carries the licence texts.

### Nederlands

Kies het bestand voor je systeem uit de lijst hierboven: het bestand op `_x64-setup.exe` voor Windows, het `.dmg`-bestand voor macOS, en voor Linux het `.deb`-bestand (Debian, Ubuntu) of de `.AppImage` (elke distributie). De pakketten voor Linux op 64-bits ARM zijn experimenteel.

De Linux-pakketten hebben ook een build-attestatie, bewaard door GitHub. Met de GitHub CLI, aangemeld met `gh auth login`, controleert `gh attestation verify BESTAND --repo OpenAEC-Foundation/open-pointcloud-studio --signer-workflow OpenAEC-Foundation/open-pointcloud-studio/.github/workflows/release.yml` dat de release-workflow van dit project het bestand heeft geattesteerd en dat het daarna niet is veranderd; de uitvoer noemt de tag waarvoor de workflow liep.

Een gedownloade AppImage is nog niet uitvoerbaar. Maak het bestand één keer uitvoerbaar met `chmod +x open-pointcloud-studio_@VERSION@_amd64.AppImage`, of in de bestandsbeheerder onder Eigenschappen > Rechten, en start het daarna.

macOS weigert de eerste start, omdat de applicatie niet met een betaald ontwikkelaarscertificaat is ondertekend en niet genotariseerd is. macOS 15 en nieuwer: start de applicatie, sluit de melding, open Systeeminstellingen > Privacy en beveiliging, scrol naar Beveiliging, kies **Open toch** en bevestig. macOS 11 tot en met 14: Control-klik op de applicatie, kies **Open** en bevestig.
