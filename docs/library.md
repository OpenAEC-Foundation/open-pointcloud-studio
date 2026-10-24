# The point-cloud core as a library

Open Pointcloud Studio is built on a point-cloud core, `pointcloud-core` in
`native/core`, that reads, indexes, exports and meshes point clouds without a
window. Programs outside this repository can use it in two ways:

- from Rust, as the crate `pointcloud-core`, through the module
  `pointcloud_core::ops`;
- from C, C++ or any language that calls C, as the library `ops_pointcloud`
  with the header `ops_pointcloud.h`, which the release carries for Windows,
  macOS and Linux.

Both run the same functions as the command line and the cloud API
(docs/cloud-api.md), so a result is the same whichever way it was asked for.

## Boundary

`pointcloud-core` is licensed LGPL-3.0-or-later (`LICENSE.md` at the root of
the repository). The desktop application in `native/desktop` is
GPL-3.0-only and is not a library.

The supported entry point for programs outside this repository is
`pointcloud_core::ops` and the types its functions name: `PointCloud`,
`LoadError`, `ExportFormat`, `OrientedBox`, `Bounds`, `ScanPose`,
`OctreeIndex`, `IndexProgress`, `MeshStats`, `SurfaceMeshConfig`, the drawing
types `DrawingView`, `DrawingUnits`, `DrawingRequest`, `DrawingStats` and
`DrawingFormat`, and the switch `set_disk_caches` / `disk_caches`. One
function each opens a source (`open_source`), reports what it holds
(`inspect`), converts or crops it (`convert`, `convert_file`), draws a section
(`section_drawing_file`), meshes it (`terrain_mesh_file`,
`surface_mesh_file`) and indexes it (`build_index`, `index_summary`). Each
long operation takes a `KeepGoing` check (`&ops::never_stop` never stops),
asked between its steps; an error from it stops the work before anything is
put in place.

Everything else the crate makes public is there for the application and may
change in any release without notice.

The core has one feature, `bag3d`, on by default: the client of the 3D BAG
building service and the HTTP and TLS stack it needs. A program that does not
fetch buildings turns it off, as the cloud API and the C library do, and
builds without any network code:

```toml
pointcloud-core = { git = "https://github.com/OpenAEC-Foundation/open-pointcloud-studio", tag = "vYYYY.W.N", default-features = false }
```

`ops` names nothing of `bag3d`, so it is complete without it.

## Compatibility

- The core is consumed by the git tag of a release, `vYYYY.W.N`; it is not
  published on crates.io.
- A change that breaks `pointcloud_core::ops` or the C interface is announced
  in CHANGELOG.md under a "Library" heading one release before the release
  that makes it. Additions are not announced in advance.
- Beyond that rule the Rust interface makes no promise of semantic
  versioning: the version of the crate is the calendar version of the
  application.
- The C interface has a version of its own and follows the rules of
  [ABI versioning](#abi-versioning).

## Errors in Rust

Every function of `ops` that can fail returns `LoadError`:

| Variant | When |
| --- | --- |
| `Io` | the file system refused: a missing file, no permission, a full disk, a destination folder that does not exist |
| `Las` | the LAS or LAZ reader or writer refused the file, such as a damaged header |
| `E57` | the E57 reader or writer refused the file |
| `UnsupportedFormat` | the extension names no format the operation reads or writes; it holds the extension |
| `InvalidData` | the file is not a readable point cloud, the source changed after it was opened, or an argument was refused. Two texts of `ops` are cases of it: `TOO_MANY_POINTS`, when a result would hold more than `Convert::max_points` (test with `ops::is_too_many_points`; no file is left), and `EMPTY_SLAB`, when the slab of a section drawing holds no points (`ops::hint_empty_slab` adds what to change for the view) |
| `Cancelled` | the `KeepGoing` check or a progress callback asked to stop; nothing was put in place |

`ops` formats no messages for people; the `Display` text of an error is a
short English sentence that names the cause.

## Licence

A program that uses the core, through Rust or through the C library, is a
"Combined Work" in the sense of the GNU Lesser General Public License,
version 3. Whoever conveys it has the duties of section 4 of that licence:

- **4(a)** say prominently, in each copy, that the program uses the
  point-cloud core of Open Pointcloud Studio and that the core is covered by
  the LGPL;
- **4(b)** give copies of the GPL and the LGPL texts with it. The C library
  archives carry both, as `LICENSE-GPL-3.0.txt` and `LICENSE-LGPL-3.0.md`;
- **4(c)** if the program shows copyright notices while it runs, show the
  notice of the core among them, with a reference to those two texts;
- **4(d)** let the user replace the core with a modified version:
  - **4(d)(1)** using the shared C library as it comes (the DLL, `.so` or
    `.dylib`, loaded at run time from where it lies, and working with any
    version of the same ABI) does this;
  - **4(d)(0)** a program that links the core statically, which every Rust
    program that depends on the crate does, must instead give the
    Minimal Corresponding Source of the core and its own code in a form that
    lets the user link it again with a modified core: its source code, or its
    object files;
- **4(e)** for a User Product, give the installation information that
  section 6 of the GPL asks for, as far as a user needs it to install and run
  the program with a modified core: beside the source with 4(d)(0), in the
  way section 6 of the GPL gives with 4(d)(1).

The source of the core of a release is the tag `vYYYY.W.N` of
https://github.com/OpenAEC-Foundation/open-pointcloud-studio, in
`native/core`; the C library adds `native/sdk`. The crates the core is built
from have their own licences, all of them permissive or file-level copyleft
(MPL-2.0); `THIRD-PARTY-NOTICES.txt` in each C library archive lists them with
their texts, as `native/tools/third-party-notices.py` writes it.

## The Rust consumer

`native/examples/rust-consumer` is a program with a workspace of its own that
uses the core as another repository would: it opens a generated file, runs
`ops::inspect` and `ops::convert`, and prints the result. The SDK workflow
builds it on every run without a lock file, so that Cargo takes the newest
compatible crates, and on the main branch also with the git form of the
dependency at the commit of the run.

```toml
[dependencies]
# From another repository, at the tag of a release:
pointcloud-core = { git = "https://github.com/OpenAEC-Foundation/open-pointcloud-studio", tag = "vYYYY.W.N", default-features = false }
# Or from a checkout beside it:
# pointcloud-core = { path = "../open-pointcloud-studio/native/core", default-features = false }

# The core reads and writes large files; a debug build of it is slow
# without this, and a dependency does not inherit it from the workspace of
# the core.
[profile.dev.package.pointcloud-core]
opt-level = 2
```

```rust
use pointcloud_core::ops::{self, Convert};
use pointcloud_core::ExportFormat;

let cloud = ops::open_source("scan.e57".as_ref(), &ops::never_stop)?;
println!("{} points", ops::inspect(&cloud).points);
let convert = Convert { format: ExportFormat::Laz, section: None, max_points: None };
let written = ops::convert(&cloud, "scan.laz".as_ref(), convert, &ops::never_stop)?;
```

## C library

The library is built from `native/sdk` (crate `pointcloud-sdk`, library
`ops_pointcloud`). The header `native/sdk/include/ops_pointcloud.h` is written
by hand; a test holds it to the library: the declared functions, the
`#define` values and the struct sizes and field offsets.

### Archives

Every release carries one archive per system, each with a `.sha256` beside it:

| File | System | Library |
| --- | --- | --- |
| `open-pointcloud-studio-sdk_<v>_windows-x64-dll.zip` | Windows 10 and 11, x64 | `bin/ops_pointcloud.dll`, import library `lib/ops_pointcloud.dll.lib` |
| `open-pointcloud-studio-sdk_<v>_macos-universal-dylib.tar.gz` | macOS 11 and later, arm64 and x86-64 in one file | `lib/libops_pointcloud.dylib`, install name `@rpath/libops_pointcloud.dylib` |
| `open-pointcloud-studio-sdk_<v>_linux-amd64-so.tar.gz` | Linux x86-64, C library 2.35 or newer | `lib/libops_pointcloud.so.1` (soname), link `lib/libops_pointcloud.so` |
| `open-pointcloud-studio-sdk_<v>_linux-arm64-so.tar.gz` | Linux 64-bit ARM, C library 2.35 or newer | the same |

Each also holds `include/ops_pointcloud.h`, `examples/inspect.c`,
`README.md`, `LICENSE-LGPL-3.0.md`, `LICENSE-GPL-3.0.txt` and
`THIRD-PARTY-NOTICES.txt`. The names end in `-dll`, `-dylib` and `-so` so that
no download button of the foundation's website takes them for the
application. A static archive is built and tested by the SDK workflow but is
not in the release.

### Building against it

Linux and macOS, from the unpacked folder:

```sh
cc -std=c11 -Wall -Iinclude examples/inspect.c -Llib -lops_pointcloud \
   -Wl,-rpath,"$PWD/lib" -o inspect
```

Windows, with the Windows C compiler (`cl.exe`) in a developer command
prompt for x64:

```bat
cl /nologo /W4 /std:c11 /Iinclude examples\inspect.c lib\ops_pointcloud.dll.lib
copy bin\ops_pointcloud.dll .
```

A program that links the static archive defines `OPS_STATIC` before it
includes the header (on Windows the header otherwise declares the functions
as imported from the DLL), and links the system libraries that
`cargo rustc --crate-type staticlib -- --print native-static-libs` lists.

### Functions

| Function | Does | Owns |
| --- | --- | --- |
| `ops_abi_version()` | `(OPS_ABI_MAJOR << 16) \| OPS_ABI_MINOR` of the loaded library | |
| `ops_version()` | the version of the release, such as `"2026.44.0"` | static string |
| `ops_last_error()` | the message of the last failed call on this thread, or NULL | the library; valid until the next call on this thread |
| `ops_string_free(text)` | frees a string the library returned through `char **`; NULL is ignored | |
| `ops_set_disk_caches(enabled)` | index and preview caches in the cache folder of the application on (1) or off (anything else), for the whole process | |
| `ops_cloud_open(path, &cloud)` | opens a file: LAS and LAZ from their header, other formats with one pass over their points (`ops::open_source`) | the caller, until `ops_cloud_free` |
| `ops_cloud_free(cloud)` | frees a cloud; NULL is ignored | |
| `ops_cloud_info(cloud, &info)` | count, bounds, attributes, stations and station photos (`ops::inspect`) | |
| `ops_cloud_info_json(cloud, &json)` | the same as JSON, the answer of the cloud API's `inspect` | the caller, until `ops_string_free` |
| `ops_cloud_read(cloud, box, batch, callback, user_data)` | every point of the source, or those in a turned box, in source order, in batches (65,536 by default, at most 1,048,576) | the batch belongs to the library during the call |
| `ops_cloud_export(cloud, destination, options, &written)` | writes all points, or those in `options->box`, as LAS, LAZ, E57, PLY, XYZ, PTS or CSV, by extension or as named; refuses with `OPS_ERROR_LIMIT` and no file when the result would hold more than `options->max_points` (`ops::convert`) | |
| `ops_index_build(cloud, options, &index)` | an octree index on disk, in `options->scratch_dir` or the system temp folder, or in the cache folder with `cached` = 1 and the caches on (`ops::build_index`); reports progress and may be stopped | the caller, until `ops_index_free`, which removes an uncached index |
| `ops_index_free(index)` | frees an index; NULL is ignored | |
| `ops_index_info(index, &info)` | points, nodes, leaves and depth (`ops::index_summary`) | |
| `ops_index_read(index, box, batch, callback, user_data)` | the points of the index, or those in a turned box, leaf by leaf; only the leaves that meet the box are read | the batch belongs to the library during the call |

An export cannot be stopped in ABI 1.0, because the export of the core has
no hook for it yet; a later minor version appends a progress callback to
`OpsExportOptions`.

### Errors

Every function that returns an `OpsStatus` clears the message of the calling
thread first and sets it when it fails; a success leaves no message. Out
pointers are NULL (counts 0) after a failure.

| Status | When |
| --- | --- |
| `OPS_OK` 0 | it worked |
| `OPS_ERROR_ARGUMENT` 1 | a NULL pointer, an unknown value (a format, `cached` other than 0 or 1, a batch above 1,048,576, a box with limits out of order or not finite), a `struct_size` below the size of ABI 1.0, a path that is not UTF-8 |
| `OPS_ERROR_IO` 2 | `LoadError::Io` |
| `OPS_ERROR_FORMAT` 3 | `LoadError::UnsupportedFormat` |
| `OPS_ERROR_DATA` 4 | `LoadError::Las`, `E57` and `InvalidData`: not a readable point cloud, or the source changed |
| `OPS_ERROR_CANCELLED` 5 | a callback returned something other than 0 |
| `OPS_ERROR_INTERNAL` 6 | an internal error was caught; the message names the function |
| `OPS_ERROR_LIMIT` 7 | the export would hold more than `max_points` points; no file was written |

A struct with a `struct_size` is read and written as far as both sides know
it: the caller sets `struct_size` to the size of the struct in its header,
and the library reads or writes no more than the size it knows. Missing
fields at the end take their defaults; `OpsPoint` and `OpsBox` never change
in ABI 1.x.

### Threads, callbacks and unloading

- A cloud or an index may be read from several threads at once, and must not
  be freed while one of them uses it. A compile-time check in the library
  holds the Rust types to this.
- Callbacks run on the thread that called the function. They may call the
  library, including `ops_last_error()`, and must not unwind: no C++
  exception and no `longjmp` may leave them.
- The library is never unloaded once loaded: on Windows its first call pins
  the DLL, on Linux it is linked with `-z nodelete`, and macOS does not unload
  a library with thread-local storage. Its worker threads stay alive between
  calls, and `fork()` after its first call is not supported.
- Calls inside the library never go through its exported names, and on
  Linux it is linked with `-Bsymbolic-functions`, so a program that defines a
  function with the same name as one of the library does not change it.

### Disk caches

The application keeps indexes and previews of large files in a cache folder:
`XDG_CACHE_HOME` when set, otherwise the local application data folder on
Windows and `~/.cache` elsewhere. The library writes nothing there unless the
program asks: its first call turns the caches of the core off, unless the
program called `ops_set_disk_caches` before; `ops_set_disk_caches(1)` turns
them on for the whole process. An index built without `cached` lies only in
`scratch_dir`, or the system temp folder, and goes when it is freed.

### Windows

The DLL is built with the static C runtime, so it needs no runtime that a
clean Windows lacks. It loads only libraries of the system; the SDK workflow
fails on any other. The libraries it may load are `KERNEL32.dll`, `ntdll.dll`,
`bcryptprimitives.dll`, `bcrypt.dll`, `ucrtbase.dll`, `ws2_32.dll`,
`advapi32.dll`, `userenv.dll`, `ole32.dll`, `shell32.dll`, `secur32.dll`,
`crypt32.dll`, `ncrypt.dll`, `user32.dll` and the `api-ms-win-*` sets; a build
of this version loads `KERNEL32.dll`, `ntdll.dll`, `bcryptprimitives.dll` and
`api-ms-win-core-synch-l1-2-0.dll`. Because the runtime is the library's own,
a string it returns is freed with `ops_string_free`, never with `free`. The
DLL carries version details (product "Open Pointcloud Studio library", file
and product version the release); a release gives it the same code
signature as the Windows programs of the application.

### ABI versioning

- `OPS_ABI_MAJOR` changes only when a function or a layout is removed or
  changed; `OPS_ABI_MINOR` grows with additions.
- On Linux the soname carries the major version, `libops_pointcloud.so.1`.
- On Windows and macOS, and as a check everywhere, a program tests at start
  that `ops_abi_version() >> 16 == OPS_ABI_MAJOR` and that
  `(ops_abi_version() & 0xffff)` is at least the minor version whose
  functions it uses.
- Structs with a `struct_size` only grow at the end; `OpsPoint` and `OpsBox`
  are fixed for 1.x.
- `ops_version()` is the calendar version of the release; the ABI version is
  independent of it.

### The example

`examples/inspect.c` checks the ABI version, writes a grid of 1000 points as
`grid.xyz`, opens it and prints what it holds as JSON, reads its points in
batches of 100 and checks their count and bounds, exports the half of the
grid in a box to `grid.laz` (and checks that `max_points` = 10 is refused),
opens that file, builds an index of it in a scratch folder with a progress
callback and reads the box through the index. It prints one line and returns
0, or prints the message of the library and returns 1. The SDK workflow
builds and runs it against each unpacked archive: on Linux also linked
statically, on Windows with `cl.exe /W4 /WX`, on macOS on both processor
types.

## One calculation path

The command line, the cloud API, the C library and the Rust consumer call the
same functions of `pointcloud_core::ops`, and none of them calculates
anything of its own. Tests hold this:

- the tests of `ops` in the core compare each operation with the core
  functions the command line called before;
- `native/desktop/src/cloud_parity.rs` holds the command-line functions and
  the window's section-drawing job to `ops::section_drawing_file` and the
  other operations, and the cloud API to the command line;
- `native/sdk/tests/abi.rs` holds the C library to `ops`: an export writes
  the bytes `ops::convert` writes, the JSON is `ops::inspect`, and a read
  through an index gives the points of a read of the source.

The window of the application exports an unedited layer through
`export_full`, the function `ops::convert` uses for a whole cloud. It differs
where it must: an export of a layer with deleted points, a moved or scaled
layer, or colours from photos goes through the edited export of the
application, which the library does not offer.
