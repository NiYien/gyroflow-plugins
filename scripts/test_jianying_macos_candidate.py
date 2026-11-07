# SPDX-License-Identifier: GPL-3.0-or-later
import importlib.util
import hashlib
import json
from pathlib import Path
import plistlib
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("macos_candidate", Path(__file__).with_name("build_jianying_macos_candidate.py"))
candidate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(candidate)


class CandidateTests(unittest.TestCase):
    def fixture(self, directory, autoload=True):
        root = Path(directory)
        bundle = root / candidate.BUNDLE
        version = "2.1.2-macos-candidate"
        contents = bundle / "Contents"
        for relative, data in {
            "MacOS/" + candidate.WRAPPER: b"signed outer",
            "MacOS/" + candidate.CORE: b"byte-identical signed core",
            "Resources/LICENSE": b"GPL-3.0-or-later",
            "Resources/version.txt": (version + "\n").encode(),
            "_CodeSignature/CodeResources": b"generated signature seal",
        }.items():
            target = contents / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        info = {"CFBundleIdentifier": candidate.PLUGIN_ID, "CFBundleExecutable": candidate.WRAPPER,
                "CFBundleShortVersionString": "2.1.2", "NiYienPackageVersion": version}
        (contents / "Info.plist").write_bytes(plistlib.dumps(info))
        inner = candidate.candidate_metadata(version, autoload=autoload)
        inner["files"] = [item for item in candidate.file_inventory(bundle)
                          if not item["path"].startswith("Contents/_CodeSignature/")
                          and item["path"] != "Contents/MacOS/" + candidate.WRAPPER]
        candidate.write_json(contents / "Resources/package-manifest.json", inner)
        delivery = candidate.candidate_metadata(version, autoload=autoload)
        delivery["files"] = [{**item, "path": candidate.BUNDLE + "/" + item["path"]}
                             for item in candidate.file_inventory(bundle)]
        candidate.write_json(root / candidate.DELIVERY, delivery)
        return root, delivery

    def change_metadata(self, root, change):
        inner_path = root / candidate.BUNDLE / "Contents/Resources/package-manifest.json"
        inner = json.loads(inner_path.read_text())
        delivery = json.loads((root / candidate.DELIVERY).read_text())
        change(inner)
        change(delivery)
        inner_path.write_text(json.dumps(inner))
        delivery["files"] = [{**item, "path": candidate.BUNDLE + "/" + item["path"]}
                             for item in candidate.file_inventory(root / candidate.BUNDLE)]
        (root / candidate.DELIVERY).write_text(json.dumps(delivery))

    def archive(self, root, entries=None):
        package = root / candidate.PACKAGE
        with zipfile.ZipFile(package, "w") as archive:
            for path in sorted((root / candidate.BUNDLE).rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(root).as_posix())
            archive.write(root / candidate.DELIVERY, candidate.DELIVERY)
            for name, data in entries or []:
                archive.writestr(name, data)
        return package

    def test_delivery_inventory_covers_generated_signature_and_inner_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root, delivery = self.fixture(directory)
            verified = candidate.verify_delivery(self.archive(root))
            names = {item["path"] for item in verified["files"]}
            self.assertIn(candidate.BUNDLE + "/Contents/_CodeSignature/CodeResources", names)
            self.assertIn(candidate.BUNDLE + "/Contents/Resources/package-manifest.json", names)
            self.assertIn(candidate.BUNDLE + "/Contents/MacOS/" + candidate.WRAPPER, names)
            self.assertFalse(verified["host_verified"])
            self.assertFalse(verified["notarized"])
            self.assertEqual(verified["files"], delivery["files"])

    def test_enabled_autoload_declares_source_provider_and_binding_priority(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory)
            verified = candidate.verify_delivery(self.archive(root))
            self.assertTrue(verified["autoload"])
            self.assertEqual(verified["autoload_provider"], candidate.AUTOLOAD_PROVIDER)
            self.assertTrue(verified["manual_saved_binding_priority"])
            self.assertFalse(verified["host_verified"])
            self.assertFalse(verified["notarized"])

    def test_historical_disabled_autoload_archive_remains_verifiable(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory, autoload=False)
            verified = candidate.verify_delivery(self.archive(root))
            self.assertFalse(verified["autoload"])
            self.assertNotIn("autoload_provider", verified)
            self.assertNotIn("manual_saved_binding_priority", verified)

    def test_enabled_autoload_rejects_incomplete_or_unverified_provider(self):
        changes = {
            "missing-provider": lambda data: data.pop("autoload_provider"),
            "unknown-version": lambda data: data["autoload_provider"].update(version=candidate.AUTOLOAD_PROVIDER["version"] + 1),
            "unexpected-host-fingerprint": lambda data: data["autoload_provider"]["host"].update(sha256="0" * 64),
            "missing-host-module": lambda data: data["autoload_provider"]["host"].pop("module"),
            "wrong-host-bundle": lambda data: data["autoload_provider"]["host"].update(bundle_id="other.host"),
            "missing-priority": lambda data: data.pop("manual_saved_binding_priority"),
            "false-priority": lambda data: data.update(manual_saved_binding_priority=False),
        }
        for name, change in changes.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root, _ = self.fixture(directory)
                self.change_metadata(root, change)
                with self.assertRaisesRegex(ValueError, "Enabled autoload"):
                    candidate.verify_delivery(self.archive(root))

    def test_cold_autoload_archive_verification_never_reads_external_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory)
            package = self.archive(root)
            with patch.object(candidate, "ROOT", root / "missing-source-checkout"), \
                    patch.object(candidate, "run", side_effect=AssertionError("No external commands")), \
                    patch.object(candidate.Path, "read_bytes", side_effect=AssertionError("No external bytes")), \
                    patch.object(candidate.Path, "read_text", side_effect=AssertionError("No external text")):
                verified = candidate.verify_delivery(package)
            self.assertTrue(verified["autoload"])

    def test_autoload_build_requires_receipt_before_output_reservation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(candidate, "reserve_output") as reserve:
                with self.assertRaisesRegex(ValueError, "fresh --local-core-provenance"):
                    candidate.build(root / "old-signed-inner", root / "output", root, "2.1.2")
            reserve.assert_not_called()
            self.assertFalse((root / "output").exists())

    def exercise_autoload_build_preflight(self, root, recorded):
        core = root / "inner.dylib"
        core.write_bytes(b"Signed arm64 binary identity is checked by the provenance validator.")
        receipt = root / "receipt.json"
        receipt.write_text("{}")
        for name in candidate.NATIVE_SOURCES:
            path = root / "jianying/native/macos" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        current = {name: hashlib.sha256(name.encode()).hexdigest()
                   for name in candidate.AUTOLOAD_REQUIRED_RUST_SOURCES}
        original = {"rust_source_sha256": current, "shared_core_source_revision": "formal-base"}
        validated = {"local_core_provenance": {"plugin_source_sha256": recorded}}
        with patch.object(candidate, "ROOT", root), \
                patch.object(candidate, "require_arm64"), \
                patch.object(candidate, "run", return_value=SimpleNamespace(stderr="Signature=adhoc")), \
                patch.object(candidate, "source_provenance", return_value=original), \
                patch.object(candidate, "validate_local_core_provenance", return_value=validated), \
                patch.object(candidate, "reserve_output", side_effect=RuntimeError("preflight accepted")) as reserve:
            try:
                candidate.build(core, root / "output", root, "2.1.2", receipt)
            finally:
                self.assertFalse((root / "output").exists())
                self.preflight_reserve_calls = reserve.call_count

    def test_autoload_build_rejects_old_two_path_and_stale_source_receipts(self):
        current = {name: hashlib.sha256(name.encode()).hexdigest()
                   for name in candidate.AUTOLOAD_REQUIRED_RUST_SOURCES}
        old_two = {name: current[name] for name in ("openfx/src/gyroflow.rs", "openfx/src/jianying_macos.rs")}
        stale = {**current, "openfx/src/jianying_autoload.rs": "0" * 64}
        for name, recorded in (("old-two-path-receipt", old_two), ("stale-autoload-source", stale)):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                with self.assertRaisesRegex(ValueError, "must cover current Rust source"):
                    self.exercise_autoload_build_preflight(Path(directory), recorded)
                self.assertEqual(self.preflight_reserve_calls, 0)

    def test_fresh_autoload_source_receipt_reaches_output_reservation(self):
        current = {name: hashlib.sha256(name.encode()).hexdigest()
                   for name in candidate.AUTOLOAD_REQUIRED_RUST_SOURCES}
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "preflight accepted"):
                self.exercise_autoload_build_preflight(Path(directory), current)
            self.assertEqual(self.preflight_reserve_calls, 1)

    def test_autoload_preflight_rejects_old_inner_with_real_source_validator(self):
        from test_jianying_local_core_provenance import SourceFixture, provenance
        with tempfile.TemporaryDirectory() as directory:
            fixture = SourceFixture(directory)
            current = {}
            for name in candidate.AUTOLOAD_REQUIRED_RUST_SOURCES:
                for base in (fixture.formal, fixture.workspace):
                    path = base / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text("Current autoload source: " + name)
                current[name] = hashlib.sha256((fixture.formal / name).read_bytes()).hexdigest()
            fixture.data["plugin_source_sha256"] = current
            fixture.save()
            old_inner = Path(directory) / "old-inner.dylib"
            old_inner.write_bytes(b"Older signed inner must not inherit the new source receipt.")
            with patch.object(candidate, "ROOT", fixture.formal), \
                    patch.object(candidate, "local_validation_module", return_value=provenance), \
                    patch.object(provenance, "BASE_REVISION", fixture.base):
                with self.assertRaisesRegex(ValueError, "inner binary identity differs"):
                    candidate.validate_autoload_build_provenance(fixture.receipt, old_inner, current)
                valid = candidate.validate_autoload_build_provenance(fixture.receipt, fixture.binary, current)
            self.assertEqual(valid["local_core_provenance"]["plugin_source_sha256"], current)

    def test_unlisted_or_corrupted_payload_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory)
            package = self.archive(root, [(candidate.BUNDLE + "/Contents/unlisted", b"extra")])
            with self.assertRaisesRegex(ValueError, "inventory"):
                candidate.verify_delivery(package)
            (root / candidate.BUNDLE / "Contents/MacOS" / candidate.CORE).write_bytes(b"changed core")
            package = self.archive(root)
            with self.assertRaisesRegex(ValueError, "checksum"):
                candidate.verify_delivery(package)

    def test_acceptance_claim_and_unsafe_delivery_path_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root, delivery = self.fixture(directory)
            delivery["host_verified"] = True
            (root / candidate.DELIVERY).write_text(json.dumps(delivery))
            with self.assertRaisesRegex(ValueError, "production acceptance"):
                candidate.verify_delivery(self.archive(root))
            delivery["host_verified"] = False
            delivery["files"][0]["path"] = "../outside"
            (root / candidate.DELIVERY).write_text(json.dumps(delivery))
            with self.assertRaisesRegex(ValueError, "Invalid delivery path"):
                candidate.verify_delivery(self.archive(root))

    def test_existing_output_and_other_architectures_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / candidate.BUNDLE).mkdir()
            with self.assertRaises(FileExistsError):
                candidate.reserve_output(root)
            with self.assertRaisesRegex(ValueError, "installation directory"):
                candidate.reserve_output(Path("/Library/OFX/Plugins/local-candidate"))
            with patch.object(candidate, "architectures", return_value=["arm64", "x86_64"]):
                with self.assertRaisesRegex(ValueError, "exact arm64"):
                    candidate.require_arm64(root / "universal.dylib")


if __name__ == "__main__":
    unittest.main()
