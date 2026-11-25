#!/usr/bin/env python3
"""Version synchronization must leave third-party package versions alone."""

import importlib.util
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "classic_version", Path(__file__).with_name("classic-version.py")
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class Versions(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        data = {
            "classic/package.json": {"version": "0.3.0", "dependencies": {"example": "0.3.0"}},
            "classic/package-lock.json": {
                "version": "0.3.0",
                "packages": {"": {"version": "0.3.0"}, "node_modules/example": {"version": "0.3.0"}},
            },
            "classic/src-tauri/tauri.conf.json": {"version": "0.3.0", "bundle": {}},
        }
        for name, value in data.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(value))
        for name in module.FILES:
            if name.endswith(".json"):
                continue
            header = "[[package]]" if name.endswith("Cargo.lock") else "[package]"
            text = header + '\nname = "open-pointcloud-studio"\nversion = "0.3.0"\n'
            if name.endswith("Cargo.lock"):
                text += '[[package]]\nname = "third-party"\nversion = "0.3.0"\n'
            (self.root / name).write_text(text)

    def test_synchronizes_product_only_and_maps_msi(self):
        result = module.contents(self.root, "2026.45.1")
        lock = json.loads(result["classic/package-lock.json"])
        self.assertEqual(lock["version"], "2026.45.1")
        self.assertEqual(lock["packages"][""]["version"], "2026.45.1")
        self.assertEqual(lock["packages"]["node_modules/example"]["version"], "0.3.0")
        self.assertIn('name = "third-party"\nversion = "0.3.0"', result["classic/src-tauri/Cargo.lock"])
        tauri = json.loads(result["classic/src-tauri/tauri.conf.json"])
        self.assertEqual(tauri["bundle"]["windows"]["wix"]["version"], "26.45.1")
        for name, text in result.items():
            (self.root / name).write_text(text)
        module.check(self.root, "2026.45.1")

    def test_check_rejects_stale_manifests_without_writing(self):
        before = (self.root / module.FILES[0]).read_bytes()
        with self.assertRaises(ValueError):
            module.check(self.root, "2026.45.1")
        self.assertEqual(before, (self.root / module.FILES[0]).read_bytes())

    def test_missing_manifest_is_an_error(self):
        (self.root / module.FILES[1]).unlink()
        with self.assertRaises(OSError):
            module.contents(self.root, "2026.45.1")

    def test_native_only_workspace_is_supported(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(module.contents(Path(directory), "2026.45.1"), {})

    def test_release_script_updates_native_and_classic_together(self):
        source = Path(__file__).resolve().parents[2]
        shutil.copytree(source / "native/packaging", self.root / "native/packaging")
        for name in ["Cargo.toml", "Cargo.lock", "core/Cargo.toml", "desktop/Cargo.toml", "ui/Cargo.toml", "server/Cargo.toml", "sdk/Cargo.toml"]:
            target = self.root / "native" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / "native" / name, target)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        subprocess.run(
            ["bash", str(self.root / "native/packaging/set-version.sh"), "set", "2026.46.0"],
            check=True, capture_output=True, text=True,
        )
        module.check(self.root, "2026.46.0")
        lock = (self.root / "native/Cargo.lock").read_text()
        self.assertIn('name = "openaec-ui"\nversion = "2026.46.0"', lock)


if __name__ == "__main__":
    unittest.main()
