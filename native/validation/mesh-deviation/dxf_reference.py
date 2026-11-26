"""Measure a PCD scan against the triangular model-space surfaces in a Studio DXF."""

import argparse
import json

import ezdxf
import numpy as np
import point_cloud_utils as pcu
from ezdxf.render import MeshBuilder
from pypcd4 import PointCloud


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


def measure(scan, drawing):
    vertices, faces = triangles(drawing)
    points = PointCloud.from_path(scan).numpy(fields=["x", "y", "z"])
    points = np.ascontiguousarray(points[np.isfinite(points).all(axis=1)], dtype=np.float64)
    if not len(points):
        raise ValueError("The scan contains no finite points")
    distances, _, _ = pcu.closest_points_on_mesh(points, vertices, faces)
    return {
        "triangles": len(faces),
        "samples": len(points),
        "mean": float(distances.mean()),
        "p95": float(np.percentile(distances, 95, method="inverted_cdf")),
        "max": float(distances.max()),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scan", help="PCD with x, y and z in the drawing's coordinates and units")
    parser.add_argument("drawing", help="Studio DXF mesh or detected-face export")
    args = parser.parse_args()
    print(json.dumps(measure(args.scan, args.drawing), indent=2, allow_nan=False))
