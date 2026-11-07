# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise archive identity, payload integrity and formal/local separation."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import plistlib
import sys
import struct
import subprocess
import zlib
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import build_jianying_macos_package as package


def png_fixture(color=6):
    def chunk(name, payload):
        return struct.pack(">I", len(payload)) + name + payload + struct.pack(">I", zlib.crc32(name + payload))
    header = struct.pack(">IIBBBBB", 128, 128, 8, color, 0, 0, 0)
    channels = 4 if color == 6 else 3
    pixels = (b"\x00" + b"\xff" * 128 * channels) * 128
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b"")


class PackageTests(unittest.TestCase):
    def test_failed_command_reports_output_and_preserves_the_failure(self):
        diagnostics = io.StringIO()
        with contextlib.redirect_stderr(diagnostics):
            with self.assertRaises(subprocess.CalledProcessError) as raised:
                package.run(sys.executable, "-c",
                            "import sys; print('command output'); print('command error', file=sys.stderr); sys.exit(7)")
        self.assertEqual(raised.exception.returncode, 7)
        self.assertEqual(raised.exception.stdout, "command output\n")
        self.assertEqual(raised.exception.stderr, "command error\n")
        self.assertEqual(diagnostics.getvalue(), "command output\ncommand error\n")

    @unittest.skipUnless(sys.platform == "darwin", "macOS codesign is required")
    def test_system_binary_is_rejected_by_the_developer_id_requirement(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(subprocess.CalledProcessError) as raised:
                package.verify_developer_id(Path("/usr/bin/true"))
        self.assertIn("code failed to satisfy specified code requirement(s)", raised.exception.stderr)

    def fixture(self, directory, local=False, mutate=None):
        meta = package.metadata("2.1.2-local" if local else "2.1.2", local)
        meta.update(core_sha256=hashlib.sha256(b"core").hexdigest(),
                    formal_dependency_revision="e" * 40, rust_source_sha256={"common/src/sampling_policy.rs": "a" * 64})
        files = {"Contents/MacOS/" + package.WRAPPER: b"wrapper",
                 "Contents/MacOS/" + package.CORE: b"core",
                 "Contents/Info.plist": plistlib.dumps({
                     "CFBundleIdentifier": package.candidate.PLUGIN_ID,
                     "CFBundleExecutable": package.WRAPPER,
                     "CFBundleShortVersionString": meta["version"].split("-")[0],
                     "CFBundleVersion": meta["version"].split("-")[0],
                     "NiYienPackageVersion": meta["version"]}),
                 "Contents/Resources/version.txt": (meta["version"] + "\n").encode(),
                 "Contents/Resources/LICENSE": b"GPL-3.0-or-later",
                 "Contents/Resources/" + package.ICON_SVG: b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                 "Contents/Resources/" + package.ICON_PNG: png_fixture(),
                 "Contents/_CodeSignature/CodeResources": b"seal"}
        inventory = lambda values: [{"path": name, "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest()} for name, data in values.items()]
        inner = {**meta, "files": inventory({k: v for k, v in files.items()
                    if not k.startswith("Contents/_CodeSignature/")
                    and k != "Contents/MacOS/" + package.WRAPPER})}
        if mutate:
            mutate(meta, inner, files)
        files["Contents/Resources/package-manifest.json"] = json.dumps(inner).encode()
        delivery = {**meta, "files": [{**item, "path": package.BUNDLE + "/" + item["path"]}
                                     for item in inventory(files)]}
        path = Path(directory) / "jianying.zip"
        with zipfile.ZipFile(path, "w") as archive:
            for name, data in files.items():
                archive.writestr(package.BUNDLE + "/" + name, data)
            archive.writestr("delivery-manifest.json", json.dumps(delivery))
        return path

    def test_formal_archive_keeps_the_shared_ofx_identity(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root)
            result = package.verify(path)
            self.assertEqual(result["plugin_id"], "xyz.niyien.gyroflow.jianying.batch11")
            self.assertEqual(result["fragment_sampling_policy"], "core-default")

    def test_local_archive_cannot_pass_formal_verification(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, local=True)
            with self.assertRaises(ValueError):
                package.verify(path)
            self.assertTrue(package.verify(path, allow_local=True)["candidate"])

    def test_integer_sampling_cannot_silently_enter_the_formal_archive(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, mutate=lambda meta, inner, files: (
                meta.update(fragment_sampling_policy="integer-coordinates"),
                inner.update(fragment_sampling_policy="integer-coordinates")))
            with self.assertRaises(ValueError):
                package.verify(path)

    def test_inner_manifest_cannot_omit_a_required_payload(self):
        with tempfile.TemporaryDirectory() as root:
            def omit(meta, inner, files):
                inner["files"] = [item for item in inner["files"]
                                  if item["path"] != "Contents/MacOS/" + package.CORE]
            with self.assertRaises(ValueError):
                package.verify(self.fixture(root, mutate=omit))

    def test_declared_core_identity_is_checked_against_actual_bytes(self):
        with tempfile.TemporaryDirectory() as root:
            def wrong(meta, inner, files):
                meta["core_sha256"] = inner["core_sha256"] = "0" * 64
            with self.assertRaises(ValueError):
                package.verify(self.fixture(root, mutate=wrong))

    def test_icon_resources_are_required_and_covered_by_the_inventory(self):
        with tempfile.TemporaryDirectory() as root:
            def missing(meta, inner, files):
                files.pop("Contents/Resources/" + package.ICON_PNG)
                inner["files"] = [item for item in inner["files"]
                                  if item["path"] != "Contents/Resources/" + package.ICON_PNG]
            with self.assertRaises(ValueError):
                package.verify(self.fixture(root, mutate=missing))

    def test_browser_icon_is_named_after_the_exported_plugin_identifier(self):
        self.assertEqual(package.ICON_PNG, "xyz.niyien.gyroflow.jianying.batch11.png")
        self.assertEqual(package.ICON_SVG, "xyz.niyien.gyroflow.jianying.batch11.svg")

    def test_browser_icon_rejects_rgb_without_an_alpha_channel(self):
        package.validate_browser_png(png_fixture())
        with self.assertRaises(ValueError):
            package.validate_browser_png(png_fixture(color=2))

    def test_generic_core_must_match_byte_for_byte(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root)
            generic = Path(root) / "ofx.zip"
            with zipfile.ZipFile(generic, "w") as archive:
                archive.writestr(package.GENERIC_CORE, b"different core")
            with self.assertRaises(ValueError):
                package.verify(path, generic)

    def test_payload_tampering_is_rejected_even_with_valid_outer_zip(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root)
            with zipfile.ZipFile(path) as archive:
                contents = {name: archive.read(name) for name in archive.namelist()}
            contents[package.BUNDLE + "/Contents/MacOS/" + package.CORE] = b"tampered"
            bad = Path(root) / "tampered.zip"
            with zipfile.ZipFile(bad, "w") as archive:
                for name, data in contents.items():
                    archive.writestr(name, data)
            with self.assertRaises(ValueError):
                package.verify(bad)


if __name__ == "__main__":
    unittest.main()
