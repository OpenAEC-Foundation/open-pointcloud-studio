#!/usr/bin/env python3
"""Reopen an exported OBJ in trimesh's independent OpenGL viewer.

Requires trimesh, NumPy, Pillow, pyglet<2 and an X11/OpenGL display.
The hidden viewer saves its rendered frame without taking keyboard focus.
"""
import argparse
from pathlib import Path
import trimesh

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('obj', type=Path)
parser.add_argument('png', type=Path)
args = parser.parse_args()
scene = trimesh.load(str(args.obj.resolve()), force='scene', process=False)
assert scene.geometry
assert all(mesh.visual.kind == 'texture' for mesh in scene.geometry.values())
args.png.write_bytes(scene.save_image(resolution=[800, 600], visible=False))
print('Rendered exported UV/materials with trimesh', trimesh.__version__)
