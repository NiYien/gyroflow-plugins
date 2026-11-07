# SPDX-License-Identifier: GPL-3.0-or-later
import importlib.util
import json
import plistlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("build_jianying_package", ROOT / "scripts/build_jianying_package.py")
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


class JianyingPackageTests(unittest.TestCase):
    def test_version_uses_release_or_ci_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            (workspace / "Cargo.toml").write_text('[workspace.package]\nversion = "8.2.4"\n', encoding="utf-8")
            with patch.object(builder, "ROOT", workspace):
                self.assertEqual(builder.package_version({"GITHUB_REF": "refs/tags/v8.2.4"}), "v8.2.4")
                self.assertEqual(builder.package_version({"GITHUB_RUN_NUMBER": "84"}), "8.2.4.84")
                self.assertEqual(builder.package_version({}), "8.2.4-dev")

    def test_package_uses_separate_bundle_and_checks_file_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wrapper = root / "wrapper.ofx"
            core = root / "core.dll"
            wrapper.write_bytes(b"wrapper sentinel")
            core.write_bytes(b"shared core sentinel")
            package, manifest = builder.create_package(wrapper, core, root / "package", "2.1.2-dev")
            self.assertEqual(builder.verify_package(package), manifest)
            files = {entry["path"] for entry in manifest["files"]}
            self.assertIn("Contents/Win64/gyroflow_ofx.dll", files)
            self.assertIn("Contents/Win64/GyroflowJianyingBatch11.ofx", files)
            self.assertIn("Contents/Resources/xyz.niyien.jianying.gyroflow.png", files)
            self.assertEqual(manifest["plugin_id"], "xyz.niyien.gyroflow.jianying.batch11")
            self.assertEqual(manifest["effect_id"], "xyz.niyien.jianying.gyroflow")
            self.assertFalse(any("ResolveScripts" in path for path in files))
            with zipfile.ZipFile(package) as source:
                contents = {name: source.read(name) for name in source.namelist()}
            info = plistlib.loads(contents[builder.BUNDLE + "/Contents/Info.plist"])
            self.assertEqual(info["CFBundleIdentifier"], builder.EFFECT_ID)
            self.assertEqual(info["CFBundleExecutable"], builder.WRAPPER)
            self.assertEqual(info["CFBundleName"], "Gyroflow")
            self.assertEqual(info["CFBundleVersion"], "1.11")
            self.assertEqual(contents[builder.BUNDLE + "/Contents/Resources/" + builder.ICON_PNG],
                             (ROOT / "jianying/resources/icon.png").read_bytes())
            filename = builder.BUNDLE + "/Contents/Win64/gyroflow_ofx.dll"
            contents[filename] = b"corrupted shared core"
            with zipfile.ZipFile(package, "w") as target:
                for name, value in contents.items():
                    target.writestr(name, value)
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                builder.verify_package(package)

    def test_manifest_does_not_accept_escaping_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "invalid.zip"
            manifest = {"schema_version": 1, "plugin_id": builder.PLUGIN_ID, "effect_id": builder.EFFECT_ID, "version": "dev",
                        "files": [{"path": "../outside", "sha256": ""}]}
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr(builder.BUNDLE + "/Contents/package-manifest.json", json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "manifest path"):
                builder.verify_package(path)

    def test_packages_share_the_actual_openfx_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            jianying = directory / "jianying.zip"
            openfx = directory / "openfx.zip"
            with zipfile.ZipFile(jianying, "w") as archive:
                archive.writestr(builder.BUNDLE + "/Contents/Win64/gyroflow_ofx.dll", b"same build")
            with zipfile.ZipFile(openfx, "w") as archive:
                archive.writestr("GyroflowNiyien.ofx.bundle/Contents/Win64/GyroflowNiyien.ofx", b"same build")
            builder.verify_shared_core(jianying, openfx)
            with zipfile.ZipFile(openfx, "w") as archive:
                archive.writestr("GyroflowNiyien.ofx.bundle/Contents/Win64/GyroflowNiyien.ofx", b"another build")
            with self.assertRaisesRegex(ValueError, "different core binaries"):
                builder.verify_shared_core(jianying, openfx)

    def test_package_requires_the_browser_icon_even_if_inventory_omits_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wrapper = root / "wrapper.ofx"
            core = root / "core.dll"
            wrapper.write_bytes(b"wrapper sentinel")
            core.write_bytes(b"shared core sentinel")
            package, manifest = builder.create_package(wrapper, core, root / "package", "3.0.0-dev")
            icon = "Contents/Resources/" + builder.ICON_PNG
            with zipfile.ZipFile(package) as source:
                contents = {name: source.read(name) for name in source.namelist()
                            if name != builder.BUNDLE + "/" + icon}
            manifest["files"] = [entry for entry in manifest["files"] if entry["path"] != icon]
            contents[builder.BUNDLE + "/Contents/package-manifest.json"] = json.dumps(manifest).encode()
            with zipfile.ZipFile(package, "w") as target:
                for name, value in contents.items():
                    target.writestr(name, value)
            with self.assertRaisesRegex(ValueError, "package contents"):
                builder.verify_package(package)


if __name__ == "__main__":
    unittest.main()
