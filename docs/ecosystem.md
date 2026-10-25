# Ecosystem readiness

This is the record of issue [#33](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/33): what Open Pointcloud Studio delivers for each system, the library and the service it offers besides the desktop application, how its MCP servers, its interface, its signatures and its cloud deployment stand, and for each requirement of the issue which check proves it and what is still open.

It was written on 25 October 2026 for the build 2026.43.0, which is not released yet. The newest published release is 0.9.5.

## Contents

- [How to read it](#how-to-read-it)
- [Capability matrix](#capability-matrix)
- [Clean machines](#clean-machines)
- [Library boundary](#library-boundary)
- [Cloud API](#cloud-api)
- [MCP verification record](#mcp-verification-record)
- [AI assistant clients](#ai-assistant-clients)
- [UI and the OpenAEC Style Repo](#ui-and-the-openaec-style-repo)
- [Signing per file](#signing-per-file)
- [Deployment on AWS](#deployment-on-aws)
- [The requirements of issue #33](#the-requirements-of-issue-33)
- [Open items and deferred work](#open-items-and-deferred-work)

## How to read it

Every piece of evidence below is one of these kinds:

- **Local**: a check that ran on a development machine (Windows 11, Rust 1.99.0 with the GNU target) on 25 October 2026. Unless a line says otherwise, it ran on commit `386c863`, which joins the shared operations, the cloud API, the C library, the container, the deployment definition and the release wiring. The branch that carries this document adds to it the generated demo scenes (an example of the core, with a dependency for its tests only), the pictures taken from them, and more attempts for the notary steps of the macOS packages; the sources of the core, the desktop application, the service and the library, `deploy/` and the CI, SDK, Container and Release workflows are those of `386c863`.
- **Local, on this branch**: with this document, on 25 October 2026, `cargo test --workspace --locked` passed (core 555 with 3 ignored, core `disk_caches` 1, the demo scenes 3, desktop 722 with 9 ignored, ui 21, server 42, sdk `abi` 19 and `caches` 1), `bash native/packaging/test-scripts.sh` gave 86 ok, and `release-notes.sh --check 2026.43.0` and `set-version.sh check` passed.
- **CI**: a workflow and its job, and the step where it matters. None of these workflows has run on this work yet, so a CI line says what will prove a requirement, not a result.
- **Owner**: what waits for a decision, an account or a hand of the owner of the project.

The evidence of record comes from the published tag run of a release: its jobs, its `verify-release` job and the verification record `open-pointcloud-studio_VERSION_verification.md` it attaches to the release. A draft run (*Leave the release as a draft*) is a dry run: it pushes no image and publishes nothing, so it proves the build and the checks, not the published files. A requirement of the issue is ticked only when its evidence is in, and this document then names the release, the run and the date.

## Capability matrix

All releases so far are stable; there is no prerelease channel. "2026.43.0" in the table is the build this record was written for. The first release with the new parts takes the number of the week it is made in ([Releasing](../native/packaging/README.md#releasing)), so the "Since" column of those parts is filled in from the first tag run.

| Delivered as | System or runtime | Processors | Status | Version | Since | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Installer `_x64-setup.exe` and zip `_windows-x64.zip` | Windows 10 and 11 | x64 | stable | 0.9.5 published, 2026.43.0 built | installer 0.6.0, these names 0.8.0 | Packages `windows`; Release `verify-release` (windows) |
| Disk image `.dmg` and archive `_macos-universal.tar.gz` | macOS 11 and later | arm64 and x86-64 in one file | stable | as above | 0.8.0 | Packages `macos`, `verify-macos`; Release `verify-release` (macos-arm64, macos-x86_64) |
| `.deb`, AppImage and `_linux-amd64.tar.gz` | Linux with a C library of version 2.35 or newer, the C++ runtime of GCC 12 or newer, X11 or Wayland | amd64 | stable | as above | 0.8.0 | Packages `verify-linux`; Release `verify-release` (linux) |
| `_arm64.deb`, `_arm64.AppImage` and `_linux-arm64.tar.gz` | as above | arm64 | experimental: built and tested on build machines with software rendering only | as above | 0.8.0 | Packages `verify-linux` (arm64) |
| Command-line modes (`--export`, `--section`, `--drawing` and the others) | in every desktop package, without a window | as the package | stable | as above | 0.4.0 | `cli_help.rs`; `smoke-test.sh` in the Packages workflow |
| Local command API | a running window, on 127.0.0.1 with a token; `native-rust-v1` | as the package | stable; changes are additive, and a release that adds a command or a field raises the revision | revision 11 in 2026.43.0 | 0.4.0 | `native_api.rs` and `mcp/tests.rs` hold [API.md](../native/API.md) to the code |
| MCP over standard input and output (`--mcp`) | in every desktop package | as the package | stable | as above | 0.7.0 | Packages `verify-linux`, step *External MCP client* |
| C library `ops_pointcloud`, ABI 1.0: `ops_pointcloud.dll` | Windows 10 and 11 | x64 | new; ABI 1.x is stable | 2026.43.0, not released | first release with this work | SDK `windows` |
| `libops_pointcloud.dylib` | macOS 11 and later | arm64 and x86-64 in one file | new | as above | as above | SDK `macos`, `macos-x86_64`; Release `verify-release` (macos) |
| `libops_pointcloud.so.1` | Linux with a C library of version 2.35 or newer | amd64, arm64 | new | as above | as above | SDK `linux` |
| Rust crate `pointcloud-core`, entry point `pointcloud_core::ops` | by the git tag `vYYYY.W.N`; tested on Linux, Windows and macOS | any the core builds for | new; no semantic versioning beyond the one-release notice of [Compatibility](library.md#compatibility) | as above | as above | CI `native`; SDK `linux`, step *Build and run the Rust consumer* |
| Container image of the cloud API, `ghcr.io/openaec-foundation/open-pointcloud-studio-api` | Linux containers | amd64 | preview, until it has passed its smoke test in the cloud deployment | as above | as above | Container `image`; Release `image`, `verify-release` (linux) |
| MCP over Streamable HTTP (`POST /mcp` of the cloud API) | in the image | amd64 | preview, with the image | as above | as above | Container `image`, step *External MCP client* |

What is not delivered or does not apply, and why:

| Requirement | Here | Why |
| --- | --- | --- |
| .NET assembly | not delivered | The C library can be called from .NET through P/Invoke; a wrapper of its own is deferred |
| WebAssembly or a browser edition | not delivered | A desktop tool with a GPU view of scans of many gigabytes on local disks. The frozen `classic/` prototype is no supported edition |
| Android or iOS | not delivered | The same reason |
| Web view runtime | not applicable | The window is drawn by the application itself and uses no web view, so no web view runtime is installed or carried |
| Updater signatures | not applicable | There is no updater; a new version is downloaded from the release page |
| Image for arm64 | not delivered | Deferred; the image and the prepared deployment are for Linux on x86-64 |
| Installable MCP bundle | not delivered | Deferred; a client starts the desktop server from its configuration ([Starting it from a client](../native/MCP.md#starting-it-from-a-client)) |
| Interactive MCP app or connector | not delivered | Tracked under [style book #9](https://github.com/OpenAEC-Foundation/OpenAEC-style-book/issues/9) |

## Clean machines

The checks of the workflows, once they have run for one release, go in this table with the URL of the run and the result:

| Job | Machine | What it does | Result |
| --- | --- | --- | --- |
| Packages `windows` | windows-latest build machine | installs the setup silently, smoke-tests the installed application and uninstalls it silently; *Check the runtime dependencies* fails on a DLL that a clean Windows lacks | not run yet |
| Packages `verify-linux` | ubuntu-24.04 (amd64), ubuntu-24.04-arm (arm64) | installs the `.deb`, starts the window under a virtual display, starts the AppImage mounted and unpacked and the archive binary | not run yet |
| Packages `verify-macos` | macos-15-intel (x86-64), macos-26 (arm64) | mounts the disk image, checks the notarization when the files are signed, installs by copying and starts the window on arm64 | not run yet |
| Release `verify-release` (windows) | windows-2025 | runs the published zip in a Windows Server Core container (ltsc2025) with nothing installed: `--version` and `--export` of a small file | not run yet |
| Release `verify-release` (linux) | ubuntu-24.04 | installs the published `.deb` with `apt-get` in a clean `ubuntu:24.04` container and starts the published AppImage with `--appimage-extract-and-run --version` in a clean `ubuntu:22.04` container | not run yet |
| Release `verify-release` (macos-arm64, macos-x86_64) | macos-15, macos-15-intel | marks the published disk image as downloaded and assesses it as the system assesses a download | not run yet |

What these checks do not show: the build machines have compilers and runtimes installed, so their install tests show that a package installs and starts, not that a clean system has all it needs; the containers are clean but have no display, so they start the application without a window. A Server Core container may lack a graphics library that the application loads at start; that check runs after the release is public, so a failure shows in the verification record only. A Windows machine without a container engine records that instead of failing.

So each system also gets one check by hand, with a release as a user downloads it. On Windows that is Windows Sandbox or a clean virtual machine; Windows Sandbox is a system setting (Windows Features, *Windows Sandbox*, then a restart), which the owner switches on. On macOS and Linux it is a clean virtual machine, or the record that none was available.

```text
System and build:
Installed before:     nothing
Release and file:     open-pointcloud-studio_VERSION_...
Downloaded with:      the browser, so that the system treats it as a download
Warnings shown:
First start:
Sample scan opened:   (a public data set of native/TEST_DATA.md)
Result:
Date:
```

None has been done yet.

## Library boundary

The point-cloud core, `pointcloud-core` in `native/core`, is licensed LGPL-3.0-or-later. Its supported entry point for other programs is `pointcloud_core::ops` and the types it names: one function for each whole operation (open, inspect, convert or crop, section drawing, terrain and surface mesh, index), each long one with a check that can stop it. The command line, the cloud API and the C library call these functions and calculate nothing of their own. A program that does not fetch 3D BAG buildings turns the default feature `bag3d` off and builds without network code. The core is used by the git tag of a release; a change that breaks `ops` or the C interface is announced in CHANGELOG.md under a "Library" heading one release ahead.

[docs/library.md](library.md) has the [boundary](library.md#boundary), the [compatibility rule](library.md#compatibility), the [errors](library.md#errors-in-rust), the duties of the [licence](library.md#licence), the [Rust consumer](library.md#the-rust-consumer) and the [C library](library.md#c-library).

The C library `ops_pointcloud` has 15 functions declared in the hand-written header `ops_pointcloud.h`, ABI 1.0: status codes 0 to 7 with a message per thread (`ops_last_error`), structs that grow at the end by their `struct_size`, strings freed with `ops_string_free`, callbacks on the calling thread, a library that is never unloaded, and no disk caches unless the program asks. The DLL has the static C runtime and loads only libraries of the system; the Linux library has the soname `libops_pointcloud.so.1`; the macOS library is universal with the install name `@rpath/libops_pointcloud.dylib` and runs from macOS 11.

Evidence:

- Local: `cargo test --workspace --locked` passed: core 555 tests (3 ignored), among them the tests of `ops` against the calls the command line made before; core `disk_caches` 1; desktop 722 (9 ignored), among them `cloud_parity.rs`, which holds the command line, the drawing job of the window and the cloud API to `ops`; sdk `abi` 19, which holds the header to the library, and sdk `caches` 1.
- Local: `check-exports.sh` on the DLL built with the GNU target: "15 functions, as the header declares". `examples/inspect.c` built with `gcc` 15.2 (MinGW-w64) and `-std=c11 -Wall -Wextra -Wpedantic -Werror` gave no warning, and ran with exit code 0: `ops_pointcloud 2026.43.0, ABI 1.0: 1000 points read, 500 exported, 500 read through an index of 1 nodes`. The header passed `clang` 19 with `-Wconversion -Wsign-conversion -Werror`, and as C++17. `native/sdk/packaging/test-sdk-packaging.sh`: 32 ok.
- Local, earlier on 25 October 2026 in the review of the library, on the same library source: the DLL for the MSVC target with the static runtime, the example built with `cl /W4 /WX` and run against a locally packed archive, and the Rust consumer built without `--locked` and run.
- CI: CI `native`, steps *Test Rust workspace* (Linux, Windows, macOS) and *Check the core without default features* (Linux). SDK `linux` (amd64 and arm64): the soname, the exports, the C library baseline, the archive, the example against the unpacked archive and linked statically, *Build and run the Rust consumer*, and on `main` also *Build the Rust consumer from the repository by its git address*. SDK `windows`: the static runtime, *Check the runtime dependencies*, the exports, the version details, the example with `cl.exe`, *Sign the DLL* and *Report the signatures*. SDK `macos` and `macos-x86_64`: universal, the oldest macOS, *Sign and notarize the library*, the exports, the example on both processor types. Release `verify-release`: the example against the published macOS archive on both processor types, and the attestations of the published Linux archives.
- Open: the list of DLLs that "a build of this version loads" in [docs/library.md](library.md#windows) is checked against the first run of SDK `windows`.

## Cloud API

`open-pointcloud-studio-api` takes a point-cloud file in the request and runs one of five read-only tools on it: `inspect`, `convert` (also only the points in a turned box), `section_drawing` (DXF or DWG), `terrain_mesh` and `surface_mesh` (OBJ). REST and MCP answer on one port, the OpenAPI 3.1 document is at `/v1/openapi.json`, the keys are bearer keys checked against their SHA-256, and every answer that is not a success is the JSON of `error_code`, `melding` and `remedie`. Nothing is kept between calls.

[docs/cloud-api.md](cloud-api.md) has the [endpoints](cloud-api.md#endpoints), the [errors](cloud-api.md#errors), the [keys](cloud-api.md#keys), the [limits](cloud-api.md#limits), [what is not in the cloud](cloud-api.md#what-is-not-in-the-cloud) and the [extensions to the shared contract](cloud-api.md#extensions-to-the-shared-contract). A test of the server holds its list of tools and its table of errors to the code.

The smoke test runs against any address of the service:

```bash
bash deploy/cloud-api/smoke.sh BASE_URL KEY --version VERSION [--limit-mib N] [--load]
```

Evidence:

- Local: the server tests, 42 passed.
- Local: the image built from `deploy/cloud-api/Dockerfile` with version 2026.43.0 and revision `386c863`. `check-image.py config` gave 17 ok: user `10001:10001`, the health check, the entry point, `/work` as a volume, the environment, no `OPENAEC_API_TOEGANG`, and the labels with that version and revision. The service refused to start in four cases, each with exit code 2 and the variable named: no keys, open access together with keys, open access with `ECS_CONTAINER_METADATA_URI_V4` and open access with `AWS_EXECUTION_ENV`.
- Local: the container ran with a read-only root, `/work` as its only writable folder, user 10001, 4 GiB of memory, a random key and a body limit of 8 MiB. It was healthy after 3 s and logged a worst case of 3194 MiB. `smoke.sh` as the Container workflow runs it passed 22 of 22 checks; with `--load` (two section drawings at once, then health) 24 of 24. With a wrong key and a wrong version it failed as it should, with 7 of 21 checks passed. Afterwards `/work` held only the empty `tmp`, the process ran as `uid=10001`, and the 85 lines of the log held neither the key, nor the whole hash of the key, nor base64 of the test files. The notices in the image are those of the service's crates: they name `acadrust`, under the MPL-2.0, and otherwise permissive licences only, and they do not name `reqwest`, the HTTP client of the 3D BAG feature, which the service leaves out.
- CI: Container `image`: the build, the checks of the image and of the refusals, *Smoke-test the container*, *External MCP client*, the empty work folder, the log and the notices; with a version, the artifact `api-image`. Release `image` pushes it by its digest and attests it in a tag run only; `publish` gives it the version as its tag after the release is public; `verify-release` (linux) pulls it by version, compares the digest, verifies the attestation and runs `--version`.
- Owner: confirm English `melding` and `remedie` beside the shared Dutch codes, and give the go to propose the extensions on [style book #12](https://github.com/OpenAEC-Foundation/OpenAEC-style-book/issues/12).

## MCP verification record

`native/packaging/mcp-client-check.py` checks an MCP server from the outside with the reference Python SDK of the protocol (the package `mcp`), pinned with hashes in `mcp-client-requirements.txt`. It writes a record in Markdown, which CI adds to the summary of its job.

### The desktop server, over standard input and output

| | |
| --- | --- |
| Transport | `open-pointcloud-studio --mcp` as a child process of the client; JSON-RPC 2.0, one message per line; batches are accepted |
| Protocol versions | `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05` |
| Tools | 152 in this build, listed in [native/MCP.md](../native/MCP.md#tools); a test holds that list to the table of tools, in order |
| Resources | none: the server offers tools only |
| Permissions | the tools drive a window through its command API on 127.0.0.1, with the token of its discovery file in the configuration folder of the user; `tools/list` marks each tool with `readOnlyHint`, `destructiveHint`, `idempotentHint` and `openWorldHint`; the token of a run of an extension accepts only the commands the extension declared |
| Changes and undo | `undo_delete` and `redo_delete` over `delete_selection`, `thin`, `colour_from_photos` and `clear_photo_colours`, at most eight edits; what else is not taken back is in [Changes and undo](../native/MCP.md#changes-and-undo) |

`--stdio BINARY` starts `BINARY --mcp` with its own configuration and cache folders and checks:

1. `initialize`: the protocol, the version of the server, tools and no resources;
2. `tools/list`: the count, `status` read-only, `thin` destructive, `bag3d` open-world, and that every input schema refuses unknown arguments;
3. a window with a grid of 1000 points (`start_instance`, then `status`);
4. `thin` 50 keeps 500 points, `undo_delete` brings back 1000, `redo_delete` 500, `undo_delete` 1000 again;
5. `open` of a missing path is a tool error;
6. the command API of the window answers on 127.0.0.1 with the token of its discovery file, and refuses a command without it.

- Local: against the desktop server of a debug build of this work, the handshake and the tool list only: protocol `2025-11-25`, 152 tools, every input schema closed, `status`, `thin` and `bag3d` annotated as above. The whole check against a stand-in server: 14 of 14, which proves the check, not the server. Steps 3 to 6 have not run against the real server, because they start a window, and windows are started by CI only.
- CI: Packages `verify-linux`, step *External MCP client*, on amd64 and arm64, under a virtual display, against the installed `.deb`. This is the first run of thinning, undo and redo through an external client.

### The cloud server, over HTTP

| | |
| --- | --- |
| Transport | Streamable HTTP, `POST /mcp`, answered as JSON; stateless, without a session id |
| Protocol versions | `2025-11-25`, `2025-06-18`, `2025-03-26` |
| Tools | five: `inspect`, `convert`, `section_drawing`, `terrain_mesh`, `surface_mesh`; all read-only and not open-world |
| Resources | none; a result file comes as an embedded resource in the tool result |
| Permissions | a bearer key on every request (401 `TOEGANG_GEWEIGERD` without it); a foreign `Origin` gets 403 `HERKOMST_GEWEIGERD`; no argument is a path (400 `ARGUMENT_ONGELDIG`) |
| Changes and undo | none: nothing persists between calls, so there is nothing to take back |

`--http URL --key KEY` checks that the handshake without the key is refused with 401; with the key `initialize` and the five read-only tools; `inspect` of the grid gives 1000 points in `structuredContent`; `convert` to LAZ gives the file as an embedded resource, whose `inspect` gives 1000 points; a `path` argument is a tool error with `ARGUMENT_ONGELDIG`.

- Local: against the container of the [Cloud API](#cloud-api) evidence, with `mcp` 2.3.0: 11 of 11 checks passed. Protocol `2025-11-25`, server `open-pointcloud-studio-api 2026.43.0`, no session, tools and no resources, `result:grid.laz` of 1382 bytes. With a wrong key the check failed, as it should.
- CI: Container `image`, step *External MCP client*.

## AI assistant clients

What is delivered is configuration only: for the desktop server the configuration in the [README](../README.md#mcp-server) and in [Starting it from a client](../native/MCP.md#starting-it-from-a-client), for the cloud server the one in [MCP client](cloud-api.md#mcp-client). There is no installable bundle and no interactive connector.

The connection is checked once with a desktop AI assistant client and once with a command-line AI assistant client, with the owner, in this order: the configuration file used, the transport, the count of the first `tools/list`, one read-only call (`status`), one destructive call (`thin`) confirmed in the client, `undo_delete`, the result and the date. The names of the clients go in the comment on the issue, not here. The client of CI is the reference SDK, not an assistant client, so CI does not prove this.

| Client | Configuration | Transport | `tools/list` | Read-only call | Destructive call | `undo_delete` | Result | Date |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| a desktop AI assistant client | | | | | | | not done | |
| a command-line AI assistant client | | | | | | | not done | |

## UI and the OpenAEC Style Repo

The conventions are in the [OpenAEC style book](https://github.com/OpenAEC-Foundation/OpenAEC-style-book); the packages are in the [OpenAEC Style Repo](https://github.com/OpenAEC-Foundation/OpenAEC-Style-Repo), among them the crates `openaec-ui` and `openaec-icons` and the fonts. `native/ui` is a copy of `crates/openaec-ui` of the Style Repo at commit `214c01a` (22 October 2026), on iced 0.13.1; for that crate it is the same as `234cb10`, its last state on iced 0.13. The window has the five themes of the style book, Light (the default), Forge (dark), OpenAEC (dark), Blueprint and High contrast, and makes its buttons, fields, lists and other controls through the shared styles of `ui_style.rs`, which a test holds. [native/ui/UPSTREAM.md](../native/ui/UPSTREAM.md) lists what the copy holds and its local changes.

The gap, checked on 25 October 2026 by comparing `214c01a` with `main` of the Style Repo (then `287c1ba`): `main` is 12 commits ahead and none behind. Eight of them change `crates/openaec-ui`: `7f9f3c7` moves it to iced 0.14, and `3a4f8ad`, `97f52fb`, `edbdd39`, `19df5a0`, `2c3a86b`, `b54eb8d` and `287c1ba` build on that. Three are the B2 icon set and its review (`111d67f`, `31b556d`, `85a6aaf`), and `234cb10` adds research and evidence.

| Package or token set | Here | Reason |
| --- | --- | --- |
| `openaec-ui` at `214c01a`: the frame (`FrameSlots`), the shell metrics (title 32 px, ribbon tabs 28 px, ribbon 94 px, document strip 30 px, status 22 px), title actions, the settings dialog with its tabs, theme picker and reset confirmation, ribbon groups, rows of the property grid, the document strip and its tabs, the status band, tooltips and the dialog frame | adopted | The shell of the window |
| The five themes as 77 shared token fields each | adopted | |
| Inter and Space Grotesk | adopted, as the copy carries them | |
| Semantic icons (`Icon`) and domain icons (`DomainIcon`) | in part | The general tool and explorer icons that were reviewed for this application come from the crate; older command icons are still local vectors |
| The 0.4 design module (`src/design/`) | in the copy, not used by the window | Taken up step by step |
| `openaec-ui` on iced 0.14 (`7f9f3c7`) | not adopted | The desktop application and its 3D view are on iced 0.13.1; moving to 0.14 is a migration of the whole desktop crate. Deferred, and the owner's decision |
| Keyboard focus route, focus groups and tree navigation with the arrow keys (`3a4f8ad`, `97f52fb`, `edbdd39`) | not adopted | Need the crate on iced 0.14 |
| One command model for ribbon, menus and shortcuts, and shortcuts set by the user (`19df5a0`, `2c3a86b`) | not adopted | Need iced 0.14; the application keeps its own commands and its adaptive ribbon |
| Strict number input (`b54eb8d`) | not adopted | Needs iced 0.14 |
| One font package for Rust and the web (`287c1ba`) | not adopted | The copy carries the fonts it was taken with |
| `openaec-icons`, the B2 icon set | not adopted | A crate added after the copy was taken |
| The colours of the 3D scene, of drawings and of sheets; the adaptive ribbon; the panels of the application | local | The domain of this application |

Evidence:

- Local: the tests of `native/ui`, 21 passed. The review baselines are the software renders [native/design/pointcloud-shell-light.png](../native/design/pointcloud-shell-light.png) and [native/design/pointcloud-shell-forge.png](../native/design/pointcloud-shell-forge.png).
- Not run for this record: `scripts/check_portable_ui.py` of the Style Repo against `214c01a` and against `main`. It needs a clone of the Style Repo, which the development machine does not have; its output and date go here once it has run.
- CI: none. A check of the copy in CI is deferred.
- The requirement stays open on the gap: the current packages of the Style Repo are not adopted.

## Signing per file

Signing depends on the secrets of the organisation. Without the secrets of its code-signing account the Windows files are not signed; without its six `APPLE_*` secrets the macOS files are signed ad hoc and not notarized. The release notes say which a release is. *Check that the libraries are signed as the applications are* refuses a release whose application is signed while its library is not; a draft run only warns. The release workflow checks the published files with the commands of this table, in `verify-release`.

The last column is filled in from the first signed tag run; the verification record lists the same identities.

| Files | Signature | Check, as a user | Expected | CI | Identity |
| --- | --- | --- | --- | --- | --- |
| `open-pointcloud-studio.exe`, `OpenCADStudio.exe`, `_x64-setup.exe`, `ops_pointcloud.dll` | Authenticode through the organisation's code-signing account, SHA-256, RFC 3161 timestamp | `Get-AuthenticodeSignature FILE \| Format-List *` | `Status` `Valid`, `TimeStamperCertificate` set | Packages `windows` and SDK `windows`, *Report the signatures*; Release `verify-release` (windows) | subject and thumbprint: not recorded yet |
| `Open Pointcloud Studio.app` and the `.dmg` | Developer ID Application of the organisation, hardened runtime, timestamp; notarized, ticket stapled | `codesign -dvv --verbose=4 FILE`; `spctl --assess --type execute -vv "Open Pointcloud Studio.app"`; `spctl -a -t open --context context:primary-signature -v FILE.dmg`; `xcrun stapler validate FILE.dmg` | `Authority=Developer ID Application`; accepted, `source=Notarized Developer ID`; the ticket validates | Packages `macos`, *Report the signatures*; `verify-macos`; Release `verify-release` (macos) | Team ID: not recorded yet |
| The binaries in `_macos-universal.tar.gz` | Developer ID Application, hardened runtime, timestamp; not notarized on their own | `codesign -dvv --verbose=4 FILE` | `Authority=Developer ID Application` | Packages `macos`; Release `verify-release` (macos) | as above |
| `libops_pointcloud.dylib` | Developer ID Application, hardened runtime, timestamp; notarized (a library cannot be stapled; the notary service keeps the ticket) | `codesign -dvv --verbose=4 lib/libops_pointcloud.dylib` | `Authority=Developer ID Application`; the notarytool record says `Accepted` | SDK `macos`, *Sign and notarize the library*; Release `verify-release` (macos) | as above |
| `.deb`, `.AppImage`, `_linux-*.tar.gz` and the two Linux archives of the C library | build attestation through Sigstore, kept by GitHub | `gh attestation verify FILE --repo OpenAEC-Foundation/open-pointcloud-studio --signer-workflow OpenAEC-Foundation/open-pointcloud-studio/.github/workflows/release.yml` | verified; the output names the tag | Release `publish`, *Attest the Linux packages*; `verify-release` (linux) | signer workflow: not recorded yet |
| `SHA256SUMS` | the checksums of every file of the release, always | `sha256sum --ignore-missing -c SHA256SUMS` | `OK` for each file downloaded | Release `publish`, *Write SHA256SUMS*; `verify-release` (linux) | none |
| `SHA256SUMS.asc` and an `.asc` for each AppImage | OpenPGP, detached and armoured, by the organisation's key; **off until that key exists** | `gpg --verify SHA256SUMS.asc SHA256SUMS` | a good signature by the published fingerprint | Release `publish`, *Sign SHA256SUMS and the AppImages*; `verify-release` (linux) | no key yet |
| Every file | a `.sha256` beside it | `sha256sum -c FILE.sha256` | `OK` | Release `publish`, *Check that the release is complete* | none |
| The image | pushed by its digest; build attestation, stored with the image in the registry | `gh attestation verify oci://ghcr.io/openaec-foundation/open-pointcloud-studio-api@sha256:DIGEST --repo OpenAEC-Foundation/open-pointcloud-studio` | verified; `docker pull` by the version gives that digest | Release `image`, *Attest the image*; `verify-release` (linux) | digest: not recorded yet |
| An updater | not applicable: there is none | | | | |

Evidence:

- Local: `bash native/packaging/test-scripts.sh` passed, 86 ok: the release notes with and without the digest of the image, an OpenPGP signature and signed libraries; pruning a draft while keeping the signatures; `signature-assets.sh`; `sign-linux.sh` without a key (it signs nothing and says so), with a throwaway key (it signs and verifies) and with a wrong fingerprint (refused). In the review of the release wiring, a key exported as a signing subkey only signed and verified with either fingerprint, and `actionlint` 1.7.12 found nothing in the six workflows of `386c863`.
- Earlier releases: the notes of 0.9.5 say that its installer and application carry a valid code signature, as the run that built them found. This record has not checked the published 0.9.5 files itself. The macOS files of 0.9.5 are signed ad hoc and not notarized.
- Not run: no file of this work has been signed with the organisation's certificates or keys yet. The Developer ID signing and notarization of the macOS files, the library included, is first proven by a release run with the organisation's secrets.
- CI: the release run (Packages, SDK) and the tag run (`publish`, `image`, `verify-release`, `verification-record`).
- Owner: confirm that the Windows signing secrets and the six `APPLE_*` secrets are set for the organisation. For the OpenPGP signatures ([style book #10](https://github.com/OpenAEC-Foundation/OpenAEC-style-book/issues/10)): an organisation key with an ed25519 signing subkey and the primary key kept offline; the secrets `OPENAEC_GPG_PRIVATE_KEY` (the armoured secret subkey) and `OPENAEC_GPG_PASSPHRASE` and the variable `OPENAEC_GPG_FINGERPRINT` of the organisation; the public key on keys.openpgp.org and on the website; and who holds the primary key, where its offline copy is kept and when it expires. Until then `sign-linux.sh` signs nothing and the notes name no signature.

## Deployment on AWS

Status: **prepared, not deployed.** The definition is [deploy/aws](../deploy/aws/README.md): a CloudFormation template, `cloud-api.yaml`, and `deploy.sh`, which makes a change set to look at and executes it only with `--execute`.

**Runtime: Amazon ECS on AWS Fargate, behind an Application Load Balancer.** Not AWS Lambda, for these reasons ([Why ECS on Fargate](../deploy/aws/README.md#why-ecs-on-fargate)):

- Size: a call carries the point-cloud file in its body, up to 64 MiB. A synchronous Lambda invocation takes at most 6 MB and an API Gateway REST API 10 MB; the load balancer sets no limit on the body.
- Time: a call may compute for up to 300 s; the idle timeout of the load balancer is set to 330 s.
- Disk: every call works in a folder of its own, which can take gigabytes; the ephemeral storage of a Fargate task goes up to 200 GiB.
- One roll-out: the shared HTTP contract of the OpenAEC SuperCloud names containers on ECS with Fargate for its services.

What the template creates ([the full table](../deploy/aws/README.md#what-the-template-creates)):

- Network: a VPC of its own with two public subnets in two availability zones and no NAT gateway; the tasks are reached only from the load balancer.
- Load balancer: HTTPS on 443 with TLS 1.2 and 1.3 and the certificate of the host name, a redirect from 80; `/v1/*` and `/mcp` go to the service, any other path gets a fixed JSON 404 as the service answers itself.
- Health: the target group asks `GET /v1/health` every 30 s with a timeout of 10 s, and the container runs `open-pointcloud-studio-api --health-check`; the deployment circuit breaker rolls a failed deployment back.
- Logs: one JSON object per line in the log group `/openaec/open-pointcloud-studio-api`, kept 30 days; requests name the first eight digits of the hash of their key, never the key or a body.
- Configuration and secrets: the image by its digest; the limits as parameters; the hashes of the keys from a Secrets Manager secret into `OPENAEC_API_SLEUTELS`. The template has no setting that starts the service without keys, and the service refuses open access when it sees the variables of ECS.
- IAM: an execution role with an inline policy only, to write to the log group and read the named secrets; no managed policy, and no task role, because the service calls no AWS service.
- Limits and storage: 4 vCPU and 8 GiB by default, 50 GiB of ephemeral storage, a read-only root, `/work` for the work folders, user 10001; the service checks at start that its worst case fits the memory ([Limits](cloud-api.md#limits)).

Evidence:

- Local: the reproducible headless build is the image of the [Cloud API](#cloud-api) evidence, and the container ran as the task definition runs it: read-only root, `/work`, user 10001, a key. `bash deploy/aws/test-deploy.sh`: 7 of 7, with a stand-in for the AWS command line, which was checked to come first on the path, so no real AWS command ran. In the work on the deployment definition, `cfn-lint` 1.58.0 found nothing in the template and caught a broken copy of it (E3002); the template is unchanged since.
- CI: Container `aws-template`: `cfn-lint` pinned with hashes, the check that the template never starts the service open, and `test-deploy.sh`. No workflow has AWS credentials or deploys.
- Owner, in this order ([What the owner provides](../deploy/aws/README.md#what-the-owner-provides)): an AWS account and a region with the AWS command line signed in; a certificate of AWS Certificate Manager for the host name in that region; the secret with the hashes of the keys (its full ARN, which the execution role matches with its random suffix); the image published by a tag run and the package made public, or a registry secret; the go for the deployment, about USD 170 to 180 a month. Then `deploy.sh` without and with `--execute`, the DNS record of the host name pointed at `LoadBalancerDnsName`, and `smoke.sh` against the host name.

Record of the deployment, to fill in:

```text
Date:
Region and stack:
Image:             ghcr.io/openaec-foundation/open-pointcloud-studio-api@sha256:
deploy.sh:         change set looked at; executed
smoke.sh:          checks passed, with --load
Volume ownership:  /work and /work/tmp writable by user 10001 (yes, or the init-container fallback)
```

## The requirements of issue #33

Every requirement is open. "Now" is what exists and what ran; "CI" is what will prove it; "Open" is what waits.

**1. Capability matrix with runtimes, architectures and stable or prerelease versions; reasons for what does not apply.**

- Now: the [Capability matrix](#capability-matrix) with its reasons.
- CI: the "Since" column of the new parts, from the first tag run.
- Open: the go for the tag run.

**2. The artifacts verified on clean machines, recorded for one release.**

- Now: the jobs and the template of [Clean machines](#clean-machines). Nothing ran: release builds and windows are not made on the development machine.
- CI: Packages `windows`, `verify-linux` and `verify-macos` in the release run; Release `verify-release` and `verification-record` in the tag run.
- Open: Windows Sandbox or a clean virtual machine, switched on by the owner, and the check by hand on each system.

**3. The library boundary, entry points, version and compatibility policy, licence, and an example of an outside program.**

- Now: [docs/library.md](library.md), `native/examples/rust-consumer` and the local results of [Library boundary](#library-boundary).
- CI: CI `native`; SDK `linux`, *Build and run the Rust consumer*, and on `main` by its git address.
- Open: the owner confirms LGPL-3.0-or-later for the service and the library.

**4. The native library: a DLL with documented exports, ABI, buffer and error rules, and its Linux equivalent.**

- Now: `native/sdk` with the header, the 15 functions of ABI 1.0, the four archives of the release in `expected-assets.sh`, the notes, `SHA256SUMS` and the attestations, and the local results of [Library boundary](#library-boundary).
- CI: SDK `linux`, `windows`, `macos` and `macos-x86_64`; Release `publish` and `verify-release`.
- Open: the owner confirms the names of the four archives and the signing secrets.

**5. A headless API with its interface contract, authentication and errors, and a reproducible smoke test.**

- Now: `native/server`, [docs/cloud-api.md](cloud-api.md), `deploy/cloud-api` and the local results of [Cloud API](#cloud-api).
- CI: Container `image`.
- Open: the owner confirms English `melding` and `remedie`, and gives the go to propose the extensions on style book #12.

**6. The product MCP server verified with an external client: transport, tools and resources, permissions, changes and undo.**

- Now: [MCP verification record](#mcp-verification-record): the cloud server 11 of 11 locally; the desktop server only its handshake and tool list locally.
- CI: Packages `verify-linux`, *External MCP client* (the first run of thinning, undo and redo from outside); Container `image`, *External MCP client*.
- Open: the go for the comment on the issue.

**7. The connection of a desktop and a command-line AI assistant client; an installable bundle tracked apart from an interactive connector.**

- Now: configuration only, see [AI assistant clients](#ai-assistant-clients); no bundle, no connector.
- CI: nothing; the client of CI is no assistant client.
- Open: the two client checks with the owner, recorded in the table.

**8. The UI and its relation to the current packages and tokens of the OpenAEC Style Repo, themes and component conventions.**

- Now: [UI and the OpenAEC Style Repo](#ui-and-the-openaec-style-repo), the gap of 12 commits checked on 25 October 2026, and the ui tests.
- CI: nothing.
- Open: the decision on iced 0.14 and the current packages, and `check_portable_ui.py` against both commits. This requirement stays open on the gap.

**9. Signing verified on the files of a release, updater signatures apart, with signer or public key, command and result.**

- Now: [Signing per file](#signing-per-file) and the local script tests; there is no updater.
- CI: the release run and the tag run, with `verify-release` and the verification record.
- Open: the signing secrets confirmed, and the organisation's OpenPGP key, without which the Linux files carry attestations and checksums but no OpenPGP signature.

**10. The cloud deployment: runtime, headless build, deployment definition, configuration and secrets, access, health and logging, and a smoke test in the deployment.**

- Now: [Deployment on AWS](#deployment-on-aws), prepared and linted, not deployed.
- CI: Container `aws-template`. No workflow deploys.
- Open: the account, the certificate, the secret and the go of the owner, then `deploy.sh --execute` and `smoke.sh` against the service with the volume ownership recorded. This requirement stays open until then.

**11. Versioned evidence for each requirement before it is ticked.**

- Now: this document, and the verification record that the tag run attaches to its release.
- CI: the tag run.
- Open: the tag run, the links to it and to the record in this document, and the comment on the issue with the owner's go. After the tag run, 2, 7, 8 and 10 are expected to stay open.

## Open items and deferred work

Before a release:

- The draft run of the release workflow on this branch: every job green, no image pushed, the notes saying the image comes with the release, the four archives of the C library and `SHA256SUMS` on the draft, and none of the archive names matching a download button of the foundation's website (checked locally against the expected list on 25 October 2026; to repeat on the real list of the draft).
- The version: 25 October 2026 is the last day of ISO week 43; a tag from 26 October on belongs to week 44, so the release then needs `set-version.sh set next` and its section in CHANGELOG.md.
- The owner: the signing secrets confirmed, the creation of packages by the workflows allowed for the organisation, and the go for the tag run.

After the tag run: the identities in [Signing per file](#signing-per-file), the results in [Clean machines](#clean-machines), the package of the image made public, the client checks, and the deployment.

Known gaps of this work:

- The SDK workflow does not run on a pull request that changes only `native/core`; the CI workflow tests the core on all three systems then, and the SDK workflow runs on every push to `main`.
- The image is pushed before the signing gate of the release, so a failing gate leaves an image without a tag; a failing tag step after publication needs `crane tag IMAGE@DIGEST VERSION` by hand.
- A failing `hdiutil` or `cc` in the macOS part of `verify-release` ends that part without a line in the record.

Deferred:

- Cloud API: closed mesh, face detection and mesh conversion, once their one-file path is in `ops`; one shared thinning function; merging; photos and texture meshes; raw bodies, an object store and asynchronous jobs, which need a change of the shared contract first; stopping a conversion; sign-in with the OpenAEC account; an arm64 image; autoscaling, a web application firewall and access logs.
- C library: section drawings and meshes; progress and stopping of an export (a minor version); static archives in the release; a .NET wrapper; pkg-config and CMake files.
- MCP: tool names in the form of style book #8; the error format of the desktop server as the shared triple, which would break clients of API revision 11; an installable bundle; an interactive connector (style book #9).
- UI: the move to iced 0.14 and the current packages of the Style Repo; a CI check of the copy.
