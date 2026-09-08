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


class AcceptanceStagesTests(unittest.TestCase):
    def setUp(self):
        isolate_actions_environment(self)
        distribution.AcceptanceContractTests.setUp(self)

    write = distribution.AcceptanceContractTests.write

    def test_prepare_works_with_pending_acceptance_and_keeps_source_validation(self):
        for name in ("Cargo.toml", "Cargo.lock", "common/Cargo.toml", "finalcut/config/release-inputs.json",
                     "finalcut/Xcode/GyroflowFinalCut.xcodeproj/project.pbxproj"):
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        self.write("finalcut/validation/geometry-support.json", {"release_blocked": True})
        with mock.patch.dict(os.environ, {"GITHUB_EVENT_NAME": "workflow_dispatch",
                                         "GITHUB_REF": "refs/heads/main", "GITHUB_RUN_NUMBER": "52"}):
            self.assertEqual(release.prepare(self.root)["build_version"], "52")
        with self.assertRaisesRegex(ValueError, "pixel-verified"):
            release.require_acceptance(self.root)
        (self.root / "common/Cargo.toml").write_text('gyroflow-core = { path = "../core" }')
        with self.assertRaisesRegex(ValueError, "local path"):
            release.prepare(self.root)

    def test_pending_status_lists_all_blockers_without_changing_acceptance(self):
        self.write("finalcut/validation/geometry-support.json", {"release_blocked": True})
        self.write("finalcut/validation/performance-baseline.json", {"release_blocked": True})
        self.write("finalcut/validation/release-acceptance.json", {"release_blocked": True})
        files = list((self.root / "finalcut/validation").glob("*.json"))
        original = {path: path.read_bytes() for path in files}
        output, summary = self.root / "output", self.root / "summary"
        with mock.patch.dict(os.environ, {"GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(summary)}):
            result = release.check_acceptance(self.root / "diagnostics", self.root)
        self.assertFalse(result["ready"])
        self.assertEqual(len(result["blockers"]), 3)
        self.assertEqual(output.read_text(), "ready=false\npackage=false\ndelivery=blocked\n")
        self.assertIn("Release pending acceptance", summary.read_text())
        self.assertEqual(json.loads((self.root / "diagnostics/release-readiness.json").read_text()), result)
        self.assertEqual({path: path.read_bytes() for path in files}, original)
        with self.assertRaisesRegex(ValueError, "pixel-verified"):
            release.require_acceptance(self.root)

    def test_complete_acceptance_is_ready_but_corrupt_input_is_an_error(self):
        result = release.check_acceptance(self.root / "diagnostics", self.root)
        self.assertEqual(result, {"ready": True, "blockers": [], "delivery": "validated", "package": True})
        (self.root / "finalcut/validation/geometry-support.json").write_text("invalid json")
        with self.assertRaises(json.JSONDecodeError):
            release.check_acceptance(self.root / "invalid-diagnostics", self.root)
        self.assertFalse((self.root / "invalid-diagnostics").exists())

    def test_only_manual_runs_can_deliver_candidates_while_acceptance_is_pending(self):
        self.write("finalcut/validation/geometry-support.json", {"release_blocked": True})
        for event, expected in (("workflow_dispatch", "candidate"), ("push", "blocked"), ("pull_request", "blocked")):
            with self.subTest(event=event), mock.patch.dict(os.environ, {"GITHUB_EVENT_NAME": event}):
                result = release.check_acceptance(self.root / "diagnostics", self.root)
            self.assertEqual(result["delivery"], expected)
            self.assertEqual(result["package"], event == "workflow_dispatch")
            self.assertFalse(result["ready"])
            with self.assertRaises(ValueError):
                release.require_acceptance(self.root)


class CompilationStagesTests(unittest.TestCase):
    def setUp(self):
        isolate_actions_environment(self)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.args = argparse.Namespace(output_dir=self.root / "compiled", runtime_frameworks=self.root / "frameworks",
                                       diagnostics_dir=self.root / "diagnostics", compiled_app=None,
                                       candidate=False, signing_identity="fixture", unsigned_for_testing=False,
                                       allow_unvalidated_capacity_for_testing=False,
                                       allow_unvalidated_geometry_for_testing=False)

    def fake_compile(self, staging, frameworks, diagnostics=None):
        app = staging / builder.APP_NAME
        app.mkdir()
        (app / "binary").write_bytes(b"compiled fixture")
        return app, []

    def test_compilation_does_not_require_acceptance_or_create_a_distribution_zip(self):
        with mock.patch.object(builder, "compile_app", side_effect=self.fake_compile), \
             mock.patch.object(release, "require_acceptance", side_effect=AssertionError("must not check release")), \
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

    def test_production_packaging_is_blocked_even_with_a_compiled_app(self):
        for compiled_app in (None, self.root / builder.APP_NAME):
            self.args.compiled_app = compiled_app
            with self.subTest(compiled_app=compiled_app), \
                 mock.patch.object(builder, "compile_app", side_effect=AssertionError("must not compile")), \
                 mock.patch.object(builder, "sign_bundle", side_effect=AssertionError("must not sign")), \
                 self.assertRaisesRegex(ValueError, "pixel-verified"):
                builder.build(self.args)
        self.assertFalse(self.args.output_dir.exists())

    def package_fixture(self, candidate):
        app, _ = self.fake_compile(self.root, None)
        (self.root / "compile-result.json").write_text(json.dumps(builder.compilation_record(app)))
        self.args.compiled_app = app
        self.args.candidate = candidate
        def run(command, **kwargs):
            if command[0] == "ditto":
                if "-c" in command:
                    Path(command[-1]).write_bytes(b"zip fixture")
                else:
                    shutil.copytree(command[-2], command[-1])
            return ""
        def geometry_gate(*args):
            if candidate:
                if len(args) == 1:
                    raise AssertionError("candidate cannot require geometry acceptance")
                raise RuntimeError("unit-test pending geometry report")
        with mock.patch.object(release, "require_acceptance", side_effect=AssertionError("candidate cannot require acceptance") if candidate else None), \
             mock.patch.object(builder, "require_geometry_gate", side_effect=geometry_gate), \
             mock.patch.object(builder, "compile_app", side_effect=AssertionError("must reuse")), \
             mock.patch.object(builder, "sign_bundle") as signing, \
             mock.patch.object(builder, "run", side_effect=run):
            output_app, output_zip = builder.build(self.args)
        signing.assert_called_once()
        self.assertTrue(output_app.is_dir())
        self.assertTrue(output_zip.is_file())
        builder.validate_compiled_app(app)
        return json.loads((self.args.output_dir / "distribution-status.json").read_text())

    def test_ready_packaging_reuses_the_verified_app_without_recompilation(self):
        self.assertEqual(self.package_fixture(False)["channel"], "validated")

    def test_candidate_is_signed_and_retains_the_unfinished_acceptance_status(self):
        result = self.package_fixture(True)
        self.assertEqual(result["channel"], "candidate")
        self.assertFalse(result["acceptance_ready"])
        self.assertTrue(result["acceptance_blockers"])

    def test_candidate_summary_keeps_pending_acceptance_distinct_from_notarization(self):
        (self.root / release.ZIP_NAME).write_bytes(b"unit-test candidate ZIP")
        (self.root / "notary-result.json").write_text(json.dumps({"status": "Accepted", "id": "unit-test-only"}))
        (self.root / "distribution-status.json").write_text(json.dumps({"channel": "candidate", "acceptance_ready": False,
                                                                       "acceptance_blockers": ["unit-test pending fixture"]}))
        report = release.summarize(self.root)
        self.assertEqual(report["distribution"]["channel"], "candidate")
        self.assertFalse(report["distribution"]["acceptance_ready"])

    def test_candidate_rejects_tag_jobs_unsigned_output_and_unverified_input(self):
        self.args.candidate = True
        with self.assertRaisesRegex(ValueError, "verified compiled App"):
            builder.build(self.args)
        self.args.compiled_app = self.root / builder.APP_NAME
        with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": "push"}):
            with self.assertRaisesRegex(ValueError, "manual workflow_dispatch"):
                builder.build(self.args)
        self.args.unsigned_for_testing = True
        with self.assertRaisesRegex(ValueError, "Developer ID"):
            builder.build(self.args)
        self.args.unsigned_for_testing = False
        self.args.signing_identity = ""
        with self.assertRaisesRegex(RuntimeError, "signing-identity"):
            builder.build(self.args)
        self.assertFalse(self.args.output_dir.exists())

    def test_workflow_separates_candidate_uploads_from_validated_release_uploads(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        finalcut, tagged = workflow.split("  build_finalcut:\n", 1)[1].split("  check_finalcut_release:\n", 1)
        self.assertLess(finalcut.index("--compile-only"), finalcut.index("id: acceptance"))
        self.assertLess(finalcut.index("id: acceptance"), finalcut.index("secrets.MACOS_"))
        distribution = finalcut.split("      - name: Validate signing configuration", 1)[1]
        distribution = distribution.split("      - name: Preserve compilation", 1)[0]
        for step in distribution.split("      - name: "):
            if "Upload signed and notarized candidate" in step:
                self.assertIn("if: steps.acceptance.outputs.delivery == 'candidate'", step)
                self.assertIn("name: GyroflowNiyien-FinalCut-macos-candidate", step)
            elif "Upload drag-to-Applications installer" in step or "Upload directly consumable" in step:
                self.assertIn("if: steps.acceptance.outputs.ready == 'true'", step)
            else:
                self.assertIn("if: steps.acceptance.outputs.package == 'true'", step)
            self.assertNotIn("always()", step)
        self.assertLess(finalcut.index("--notarize"), finalcut.index("Upload signed and notarized candidate"))
        diagnostics = finalcut.split("      - name: Preserve compilation", 1)[1]
        self.assertIn("if: always()", diagnostics)
        self.assertNotIn("build-finalcut/", diagnostics)
        self.assertNotIn("release-finalcut/", diagnostics)
        self.assertNotIn(".app", diagnostics)
        self.assertIn("require-acceptance", tagged)
        self.assertIn("if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/')", tagged)
        self.assertIn("needs: [build, build_finalcut, check_finalcut_release]", workflow)


if __name__ == "__main__":
    unittest.main()
