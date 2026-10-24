# The C library of Open Pointcloud Studio

`ops_pointcloud` gives C, C++ and any language that calls C the point-cloud
core of Open Pointcloud Studio: open LAS, LAZ, E57, PLY, PCD, PTX, PTS, XYZ
and CSV files, read their points, export them or a turned box of them to LAS,
LAZ, E57, PLY, XYZ, PTS or CSV, and build an octree index on disk to read
regions quickly. It is the same code the application and its cloud API run.

The interface is ABI 1.0, declared in `include/ops_pointcloud.h`. The version
of the library is in the name of this folder and is what `ops_version()`
returns; the library was built from the tag `v` + that version of
https://github.com/OpenAEC-Foundation/open-pointcloud-studio, where
`native/sdk` holds its source and `docs/library.md` the full reference.

## Contents

| Path | What |
| --- | --- |
| `include/ops_pointcloud.h` | the header |
| `lib/libops_pointcloud.so.1`, `lib/libops_pointcloud.so` | Linux: the library and the link to it |
| `lib/libops_pointcloud.dylib` | macOS: the library for arm64 and x86-64, macOS 11 and later |
| `bin/ops_pointcloud.dll`, `lib/ops_pointcloud.dll.lib` | Windows x64: the library and its import library |
| `examples/inspect.c` | a program that uses every function |
| `LICENSE-LGPL-3.0.md`, `LICENSE-GPL-3.0.txt` | the licence of the library and the GPL it builds on |
| `THIRD-PARTY-NOTICES.txt` | the Rust crates in the library and their licences |

## Building the example

Linux and macOS, from this folder:

```sh
cc -std=c11 -Wall -Iinclude examples/inspect.c -Llib -lops_pointcloud \
   -Wl,-rpath,"$PWD/lib" -o inspect
./inspect .
```

Windows, with the Windows C compiler (`cl.exe`) in a developer command
prompt for x64, from this folder:

```bat
cl /nologo /W4 /std:c11 /Iinclude examples\inspect.c lib\ops_pointcloud.dll.lib
copy bin\ops_pointcloud.dll .
inspect.exe .
```

The program writes `grid.xyz` and `grid.laz` into the folder it is given,
prints what the grid holds as JSON and then one line, and returns 0.

## Rules in short

- Every function that returns an `OpsStatus` clears the message of the
  calling thread and sets it when it fails: `ops_last_error()`.
- Free a cloud with `ops_cloud_free`, an index with `ops_index_free`, and a
  string the library returned with `ops_string_free`, never with `free`.
- Paths are UTF-8. Callbacks run on the calling thread and must not unwind.
- A cloud or an index may be read from several threads at once.
- The library keeps no caches on disk unless `ops_set_disk_caches(1)` is
  called; it is never unloaded once loaded, and `fork()` after its first use
  is not supported.
- Check the ABI at start: `ops_abi_version() >> 16 == OPS_ABI_MAJOR` and
  `(ops_abi_version() & 0xffff) >= OPS_ABI_MINOR`.

## Licence

The library is licensed LGPL-3.0-or-later. A program that uses it is a
"Combined Work" in the sense of the LGPL: ship this licence and the GPL text
with it, say that it uses this library, and let its users replace the library
with another version. Linking the shared library as it comes does the last.
docs/library.md in the repository explains each duty.
