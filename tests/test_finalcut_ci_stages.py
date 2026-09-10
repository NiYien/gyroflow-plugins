# SPDX-License-Identifier: GPL-3.0-or-later
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import build_finalcut_package as builder
import finalcut_release as release
from tests import test_finalcut_distribution_contract as distribution


def isolate_actions_environment(test):
    patcher = mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "GITHUB_OUTPUT": "",
                                         "GITHUB_STEP_SUMMARY": "", "GITHUB_EVENT_NAME": "", "GITHUB_REF": ""})
    patcher.start()
    test.addCleanup(patcher.stop)


class ReleasePreparationTests(unittest.TestCase):
    def setUp(self):
        isolate_actions_environment(self)
        distribution.SourceDigestTests.setUp(self)

    write = distribution.SourceDigestTests.write

    def test_prepare_ignores_host_acceptance_records_and_keeps_source_validation(self):
        for name in ("Cargo.toml", "Cargo.lock", "common/Cargo.toml", "finalcut/config/release-inputs.json",
                     "finalcut/Xcode/GyroflowFinalCut.xcodeproj/project.pbxproj"):
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        self.write("finalcut/validation/geometry-support.json", {"release_blocked": True})
        with mock.patch.dict(os.environ, {"GITHUB_EVENT_NAME": "workflow_dispatch",
                                         "GITHUB_REF": "refs/heads/main", "GITHUB_RUN_NUMBER": "52"}):
            self.assertEqual(release.prepare(self.root)["build_version"], "52")
        (self.root / "common/Cargo.toml").write_text('gyroflow-core = { path = "../core" }')
        with self.assertRaisesRegex(ValueError, "local path"):
            release.prepare(self.root)


class CompilationStagesTests(unittest.TestCase):
    def setUp(self):
        isolate_actions_environment(self)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.args = argparse.Namespace(output_dir=self.root / "compiled", runtime_frameworks=self.root / "frameworks",
                                       diagnostics_dir=self.root / "diagnostics", compiled_app=None,
                                       signing_identity="fixture", unsigned_for_testing=False)

    def fake_compile(self, staging, frameworks, diagnostics=None):
        app = staging / builder.APP_NAME
        app.mkdir()
        (app / "binary").write_bytes(b"compiled fixture")
        return app, []

    def test_compilation_does_not_sign_or_create_a_distribution_zip(self):
        with mock.patch.object(builder, "compile_app", side_effect=self.fake_compile), \
             mock.patch.object(builder, "sign_bundle", side_effect=AssertionError("must not sign")):
            app = builder.compile_only(self.args)
        self.assertTrue(app.is_dir())
        builder.validate_compiled_app(app)
        self.assertFalse(list(self.root.rglob("*.zip")))
        self.assertEqual({p.name for p in self.args.diagnostics_dir.iterdir()}, {"compile-result.json"})

    def test_failed_compilation_keeps_logs_without_a_success_record(self):
        def fail(*args):
            self.args.diagnostics_dir.mkdir()
            (self.args.diagnostics_dir / "rust.log").write_text("real compiler error fixture")
            raise RuntimeError("Rust compilation failed")
        with mock.patch.object(builder, "compile_app", side_effect=fail), self.assertRaises(RuntimeError):
            builder.compile_only(self.args)
        self.assertFalse((self.args.output_dir / builder.APP_NAME).exists())
        self.assertFalse(list(self.root.rglob("compile-result.json")))
        self.assertTrue((self.args.diagnostics_dir / "rust.log").is_file())
        command_log = self.root / "xcode.log"
        with mock.patch.object(builder.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "stdout", "stderr")):
            with self.assertRaises(RuntimeError):
                builder.run(["xcodebuild"], log_path=command_log)
        self.assertIn("Exit code: 1", command_log.read_text())
        self.assertTrue(command_log.read_text().endswith("stdoutstderr"))

    def test_changed_bundle_or_stale_compilation_record_is_rejected(self):
        app, _ = self.fake_compile(self.root, None)
        record = builder.compilation_record(app)
        receipt = self.root / "compile-result.json"
        receipt.write_text(json.dumps(record))
        builder.validate_compiled_app(app)
        for key in ("source_sha256", "sdk_sha256", "build_version"):
            receipt.write_text(json.dumps({**record, key: "stale"}))
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "does not match"):
                builder.validate_compiled_app(app)
        receipt.write_text(json.dumps(record))
        (app / "binary").write_bytes(b"modified fixture")
        with self.assertRaisesRegex(ValueError, "bundle digest"):
            builder.validate_compiled_app(app)

    def package_fixture(self):
        app, _ = self.fake_compile(self.root, None)
        (self.root / "compile-result.json").write_text(json.dumps(builder.compilation_record(app)))
        self.args.compiled_app = app

        def run(command, **kwargs):
            if command[0] == "ditto":
                if "-c" in command:
                    Path(command[-1]).write_bytes(b"zip fixture")
                else:
                    shutil.copytree(command[-2], command[-1])
            return ""

        with mock.patch.object(builder, "compile_app", side_effect=AssertionError("must reuse")), \
             mock.patch.object(builder, "sign_bundle") as signing, \
             mock.patch.object(builder, "run", side_effect=run):
            output_app, output_zip = builder.build(self.args)
        signing.assert_called_once()
        self.assertTrue(output_app.is_dir())
        self.assertTrue(output_zip.is_file())
        builder.validate_compiled_app(app)
        return json.loads((self.args.output_dir / "distribution-status.json").read_text())

    def test_manual_and_tag_packaging_succeed_with_pending_host_acceptance_records(self):
        temporary_root = self.root
        for event in ("workflow_dispatch", "push"):
            with self.subTest(event=event), mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": event}):
                self.root = temporary_root / event
                self.root.mkdir()
                self.args.output_dir = self.root / "compiled"
                self.assertEqual(self.package_fixture(), {"channel": "release"})

    def test_release_summary_records_the_package_without_host_acceptance_status(self):
        (self.root / release.ZIP_NAME).write_bytes(b"unit-test ZIP")
        (self.root / "notary-result.json").write_text(json.dumps({"status": "Accepted", "id": "unit-test-only"}))
        (self.root / "distribution-status.json").write_text(json.dumps({"channel": "release"}))
        report = release.summarize(self.root)
        self.assertEqual(report["distribution"], {"channel": "release"})

    def test_reused_app_still_requires_signing(self):
        self.args.compiled_app = self.root / builder.APP_NAME
        self.args.unsigned_for_testing = True
        with self.assertRaisesRegex(ValueError, "Developer ID"):
            builder.build(self.args)
        self.args.unsigned_for_testing = False
        self.args.signing_identity = ""
        with self.assertRaisesRegex(RuntimeError, "signing-identity"):
            builder.build(self.args)
        self.assertFalse(self.args.output_dir.exists())

    def test_workflow_uploads_one_fcp_zip_and_dmg_after_notarization(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        finalcut = workflow.split("  build_finalcut:\n", 1)[1]
        self.assertLess(finalcut.index("--compile-only"), finalcut.index("secrets.MACOS_"))
        self.assertNotIn("acceptance.outputs", workflow)
        self.assertNotIn("check-acceptance", workflow)
        self.assertNotIn("require-acceptance", workflow)
        self.assertNotIn("--candidate", workflow)
        self.assertNotIn("name: GyroflowNiyien-FinalCut-macos", workflow)
        for name in ("GyroflowNiyien-FCP-macos-zip", "GyroflowNiyien-FCP-macos"):
            self.assertEqual(finalcut.count(f"name: {name}\n"), 1)
        self.assertLess(finalcut.index("--notarize"), finalcut.index("Upload signed and notarized"))
        for step in finalcut.split("      - name: "):
            if step.startswith("Upload "):
                self.assertNotIn("if:", step)
        diagnostics = finalcut.split("      - name: Preserve compilation", 1)[1]
        self.assertIn("if: always()", diagnostics)
        self.assertNotIn(".app", diagnostics)
        self.assertIn("needs: [build, build_finalcut]", workflow)


if __name__ == "__main__":
    unittest.main()
