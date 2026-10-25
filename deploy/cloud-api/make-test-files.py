#!/usr/bin/env python3
"""Write the test files and the request bodies of the smoke test of the
cloud API (smoke.sh), with nothing but the standard library.

    python3 -I make-test-files.py OUT_DIR
    python3 -I make-test-files.py --inspect-answer ANSWER.json REQUEST.json
    python3 -I make-test-files.py --large-body MIB REQUEST.json

The files in OUT_DIR:

  grid.ply      1000 coloured points on a 10 x 10 x 10 grid with a spacing
                of 0.25, as ASCII PLY (the grid of the package tests)
  grid.pcd      the same points as binary_compressed PCD, whose reader
                writes a scratch file to the system temp folder
  room.xyz      the walls of a room of 4 x 3 m and 2.5 m high
  terrain.xyz   a gently sloping terrain of 40 x 40 points
  hall.xyz      the walls of a hall of 20 x 15 m, the largest test file

and one request body per call, NAME.json, which smoke.sh sends.

--inspect-answer writes the body of an inspect call of the file that the
answer of a tool carries. --large-body writes a body of MIB MiB plus one
byte, which a service with a body limit of MIB MiB refuses.
"""

import base64
import json
import os
import struct
import sys

GRID = 10
SPACING = 0.25
COLOUR = (255, 0, 255)


def grid_points():
    for x in range(GRID):
        for y in range(GRID):
            for z in range(GRID):
                yield (x * SPACING, y * SPACING, z * SPACING)


def grid_ply():
    lines = ["ply", "format ascii 1.0", f"element vertex {GRID**3}"]
    lines += [f"property float {axis}" for axis in "xyz"]
    lines += [f"property uchar {channel}" for channel in ("red", "green", "blue")]
    lines.append("end_header")
    for point in grid_points():
        lines.append("%.2f %.2f %.2f %d %d %d" % (*point, *COLOUR))
    return ("\n".join(lines) + "\n").encode("ascii")


def lzf_literals(data):
    """LZF as literal runs only: a control byte below 32 says that the next
    control + 1 bytes are copied as they are. Any LZF reader takes it."""
    out = bytearray()
    for start in range(0, len(data), 32):
        chunk = data[start:start + 32]
        out.append(len(chunk) - 1)
        out += chunk
    return bytes(out)


def grid_pcd():
    points = list(grid_points())
    # binary_compressed holds the fields one after the other: every x, then
    # every y, then every z.
    payload = b"".join(
        struct.pack("<%df" % len(points), *(point[axis] for point in points))
        for axis in range(3)
    )
    compressed = lzf_literals(payload)
    header = (
        "# .PCD v0.7 - Point Cloud Data file format\n"
        "VERSION 0.7\n"
        "FIELDS x y z\n"
        "SIZE 4 4 4\n"
        "TYPE F F F\n"
        "COUNT 1 1 1\n"
        f"WIDTH {len(points)}\n"
        "HEIGHT 1\n"
        "VIEWPOINT 0 0 0 1 0 0 0\n"
        f"POINTS {len(points)}\n"
        "DATA binary_compressed\n"
    ).encode("ascii")
    return header + struct.pack("<II", len(compressed), len(payload)) + compressed


def walls(length, width, height, step):
    """The four walls of a room from the origin, a point every `step`."""
    rows = []
    levels = int(round(height / step)) + 1
    along = int(round(length / step))
    across = int(round(width / step))
    for level in range(levels):
        z = level * step
        for i in range(along + 1):
            x = i * step
            rows.append("%.3f %.3f %.3f" % (x, 0.0, z))
            rows.append("%.3f %.3f %.3f" % (x, width, z))
        for i in range(1, across):
            y = i * step
            rows.append("%.3f %.3f %.3f" % (0.0, y, z))
            rows.append("%.3f %.3f %.3f" % (length, y, z))
    return ("\n".join(rows) + "\n").encode("ascii")


def terrain():
    rows = []
    for i in range(40):
        for j in range(40):
            x, y = i * 0.5, j * 0.5
            rows.append("%.3f %.3f %.3f" % (x, y, 0.05 * x + 0.02 * y + 0.1 * ((i * j) % 3)))
    return ("\n".join(rows) + "\n").encode("ascii")


def b64(data):
    return base64.b64encode(data).decode("ascii")


def write_json(path, value):
    with open(path, "w", encoding="utf-8", newline="\n") as file:
        json.dump(value, file)


def write_all(out):
    os.makedirs(out, exist_ok=True)
    files = {
        "grid.ply": grid_ply(),
        "grid.pcd": grid_pcd(),
        "room.xyz": walls(4.0, 3.0, 2.5, 0.1),
        "terrain.xyz": terrain(),
        "hall.xyz": walls(20.0, 15.0, 3.0, 0.05),
    }
    for name, data in files.items():
        with open(os.path.join(out, name), "wb") as file:
            file.write(data)

    def upload(name):
        extension = name.rsplit(".", 1)[1]
        return {"format": extension, "data": b64(files[name]), "name": name}

    grid = upload("grid.ply")
    room = upload("room.xyz")
    bodies = {
        "inspect": {"input": grid},
        "convert-laz": {"input": grid, "output_format": "laz"},
        # x up to 1.1 keeps 5 of the 10 grid planes across x.
        "convert-box": {"input": grid, "output_format": "laz",
                        "box": {"min": [-1, -1, -1], "max": [1.1, 10, 10]}},
        "inspect-pcd": {"input": upload("grid.pcd")},
        "drawing": {"input": room, "box": {"min": [-0.5, -0.5, 0], "max": [4.5, 3.5, 1.2]},
                    "view": "plan", "output_format": "dxf"},
        "terrain": {"input": upload("terrain.xyz")},
        "surface": {"input": room, "max_vertices": 2000, "neighbors": 10},
        "path": {"input": grid, "path": "/etc/passwd"},
        "load": {"input": upload("hall.xyz"), "box": {"min": [-1, -1, 0], "max": [21, 16, 1.5]},
                 "view": "plan", "output_format": "dxf"},
        "mcp-initialize": {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                           "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                                      "clientInfo": {"name": "smoke", "version": "1"}}},
        "mcp-initialized": {"jsonrpc": "2.0", "method": "notifications/initialized"},
        "mcp-list": {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    }
    for name, body in bodies.items():
        write_json(os.path.join(out, name + ".json"), body)


def inspect_answer(answer, request):
    with open(answer, encoding="utf-8") as file:
        output = json.load(file)["output"]
    write_json(request, {"input": {"format": output["format"], "data": output["data"]}})


def large_body(mib, request):
    size = int(mib) * 1024 * 1024 + 1
    prefix = b'{"input": {"format": "xyz", "data": "'
    suffix = b'"}}'
    with open(request, "wb") as file:
        file.write(prefix)
        file.write(b"A" * (size - len(prefix) - len(suffix)))
        file.write(suffix)


def main(arguments):
    if len(arguments) == 1 and not arguments[0].startswith("--"):
        write_all(arguments[0])
    elif len(arguments) == 3 and arguments[0] == "--inspect-answer":
        inspect_answer(arguments[1], arguments[2])
    elif len(arguments) == 3 and arguments[0] == "--large-body":
        large_body(arguments[1], arguments[2])
    else:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
