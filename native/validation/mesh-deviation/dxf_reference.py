"""Measure a PCD, LAS or LAZ scan against the triangular model-space surfaces in a Studio DXF."""

import argparse
import json
from pathlib import Path
import tempfile

import ezdxf
import numpy as np
import point_cloud_utils as pcu
from ezdxf.render import MeshBuilder

LAS_BATCH_POINTS = 250_000


def triangles(path):
    vertices = []
    faces = []
    for entity in ezdxf.readfile(path).modelspace():
        kind = entity.dxftype()
        if kind == "MESH" and entity.dxf.subdivision_levels == 0:
            mesh = MeshBuilder.from_mesh(entity)
        elif kind == "POLYLINE" and entity.is_poly_face_mesh:
            mesh = MeshBuilder.from_polyface(entity)
        elif kind == "LINE":
            # Studio draws cylinder axes alongside the measured surfaces.
            continue
        else:
            raise ValueError(f"Unsupported model-space entity {kind} at handle {entity.dxf.handle}")

        offset = len(vertices)
        vertices.extend(mesh.vertices)
        for face in mesh.faces:
            if len(face) != 3 or any(i < 0 or i >= len(mesh.vertices) for i in face):
                raise ValueError(f"Expected a triangle with three valid indices at handle {entity.dxf.handle}")
            faces.append([offset + i for i in face])

    if not faces:
        raise ValueError("The drawing contains no triangular mesh faces")
    positions = np.asarray(vertices, dtype=np.float64)
    if not np.isfinite(positions).all():
        raise ValueError("Non-finite coordinates in DXF mesh")
    return positions, np.asarray(faces, dtype=np.int32)


def point_batches(scan):
    if Path(scan).suffix.lower() in {".las", ".laz"}:
        import laspy

        with laspy.open(scan) as reader:
            count = 0
            for records in reader.chunk_iterator(LAS_BATCH_POINTS):
                count += len(records)
                # laspy applies each axis's scale and offset to these coordinates.
                yield np.column_stack((records.x, records.y, records.z))
            if count != reader.header.point_count:
                raise ValueError(f"LAS header declares {reader.header.point_count} points, read {count}")
    else:
        from pypcd4 import PointCloud

        yield PointCloud.from_path(scan).numpy(fields=["x", "y", "z"])


def measure(scan, drawing):
    vertices, faces = triangles(drawing)
    with tempfile.TemporaryFile() as spool:
        samples = 0
        for batch in point_batches(scan):
            points = np.ascontiguousarray(batch[np.isfinite(batch).all(axis=1)], dtype=np.float64)
            if not len(points):
                continue
            # point-cloud-utils 0.34 misreads the strides of a single query row.
            query = np.repeat(points, 2, axis=0) if len(points) == 1 else points
            distances, _, _ = pcu.closest_points_on_mesh(query, vertices, faces)
            distances[:len(points)].tofile(spool)
            samples += len(points)
        if not samples:
            raise ValueError("The scan contains no finite points")
        values = np.memmap(spool, dtype=np.float64, mode="r+", shape=(samples,))
        mean, maximum = float(values.mean()), float(values.max())
        # Exact nearest rank over all batches, using the temporary file for partitioning.
        rank = (95 * samples + 99) // 100 - 1
        values.partition(rank)
        p95 = float(values[rank])
        del values
    return {
        "triangles": len(faces),
        "samples": samples,
        "mean": mean,
        "p95": p95,
        "max": maximum,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scan", help="PCD, LAS or LAZ with x, y and z in the drawing's coordinates and units")
    parser.add_argument("drawing", help="Studio DXF mesh or detected-face export")
    args = parser.parse_args()
    print(json.dumps(measure(args.scan, args.drawing), indent=2, allow_nan=False))
