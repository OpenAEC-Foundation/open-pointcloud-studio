import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import ezdxf
import laspy
import numpy as np

import dxf_reference


class ScanDistanceTest(unittest.TestCase):
    def test_global_percentile_with_single_point_last_batch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            doc = ezdxf.new()
            with doc.modelspace().add_mesh().edit_data() as mesh:
                mesh.vertices = [(207000, 475000, 100), (207003, 475000, 100), (207000, 475003, 100)]
                mesh.faces = [(0, 1, 2)]
            drawing = root / "terrain.dxf"
            doc.saveas(drawing)

            header = laspy.LasHeader(point_format=3, version="1.2")
            header.scales = np.array([0.002, 0.005, 0.001])
            header.offsets = np.array([207000.0, 475000.0, 100.0])
            scan = laspy.LasData(header)
            scan.X = np.full(21, 125)
            scan.Y = np.full(21, 50)
            scan.Z = np.array([1000] * 19 + [-9000] * 2)
            for extension in ("las", "laz"):
                with self.subTest(extension=extension):
                    path = root / f"survey.{extension}"
                    scan.write(path)
                    with patch.object(dxf_reference, "LAS_BATCH_POINTS", 20):
                        result = dxf_reference.measure(path, drawing)
                    self.assertEqual(result["samples"], 21)
                    self.assertEqual(result["p95"], 9)
                    self.assertEqual(result["max"], 9)
                    self.assertAlmostEqual(result["mean"], 37 / 21)
