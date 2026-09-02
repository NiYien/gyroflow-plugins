import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "scripts"
BUILDER = SCRIPTS / "build_finalcut_package.py"
NOTARIZER = SCRIPTS / "notarize_finalcut_package.py"
VERIFIER = SCRIPTS / "verify_finalcut_package.py"
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"


def load_script(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class FinalCutPackageBuildContractTests(unittest.TestCase):
    def test_production_build_accepts_validated_capacity_gate(self):
        module = load_script("build_finalcut_package_capacity_gate", BUILDER)
        module.require_capacity_gate(False)

    def test_signing_order_is_frameworks_then_xpc_then_app(self):
        module = load_script("build_finalcut_package", BUILDER)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / module.APP_NAME
            frameworks = [root / "FxPlug.framework", root / "PluginManager.framework"]
            commands = []
            with mock.patch.object(
                module,
                "run",
                side_effect=lambda command, environment=None: commands.append(command) or "",
            ):
                module.sign_bundle(app, frameworks, "FINGERPRINT")

            signed_targets = [Path(command[-1]).name for command in commands]
            self.assertEqual(
                signed_targets,
                [
                    "FxPlug.framework",
                    "PluginManager.framework",
                    module.XPC_NAME,
                    module.APP_NAME,
                ],
            )
            self.assertTrue(all("--options" in command for command in commands))
            self.assertTrue(all("runtime" in command for command in commands))

    def test_builder_uses_fixed_zip_name_and_forbids_workflow_payload(self):
        builder = BUILDER.read_text(encoding="utf-8")
        verifier = VERIFIER.read_text(encoding="utf-8")

        self.assertIn('ZIP_NAME = "GyroflowNiyien-FinalCut-macos.zip"', builder)
        self.assertIn("FxPlug.framework", builder)
        self.assertIn("PluginManager.framework", builder)
        self.assertNotIn("WorkflowExtensionSDK", builder)
        self.assertIn('rglob("*.appex")', verifier)
        self.assertIn('"--deep", "--strict"', verifier)
        self.assertIn('"spctl"', verifier)
        self.assertIn('"stapler", "validate"', verifier)

    def test_wrapper_and_xpc_signed_entitlement_policy_is_fail_closed(self):
        with mock.patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            module = load_script("verify_finalcut_package_entitlements", VERIFIER)
        effect = {
            "com.apple.security.app-sandbox": True,
            "com.apple.security.files.bookmarks.app-scope": True,
            "com.apple.security.files.user-selected.read-only": True,
        }

        module.verify_entitlement_policy({}, effect)

        forbidden = (
            "com.apple.security.app-sandbox",
            "com.apple.security.application-groups",
            "com.apple.security.cs.disable-library-validation",
            "com.apple.security.get-task-allow",
            "com.apple.security.network.client",
            "com.apple.security.network.server",
            "com.apple.security.files.user-selected.read-write",
            "com.apple.security.assets.movies.read-write",
        )
        for entitlement in forbidden:
            with self.subTest(entitlement=entitlement):
                with self.assertRaises(ValueError):
                    module.verify_entitlement_policy({entitlement: True}, effect)

        with self.assertRaises(ValueError):
            module.verify_entitlement_policy({}, {})
        with self.assertRaises(ValueError):
            module.verify_entitlement_policy(
                {},
                {**effect, "com.apple.security.network.client": True},
            )


class FinalCutNotaryAndWorkflowContractTests(unittest.TestCase):
    def test_notary_staples_app_then_recreates_and_verifies_same_zip(self):
        source = NOTARIZER.read_text(encoding="utf-8")
        submit = source.index('"notarytool"')
        staple = source.index('"stapler"')
        repackage = source.index('"--keepParent"', staple)
        notarized_verify = source.index('"--expect-notarized"', repackage)

        self.assertLess(submit, staple)
        self.assertLess(staple, repackage)
        self.assertLess(repackage, notarized_verify)

    def test_nightly_and_tag_use_signed_fxplug_runner_and_fixed_artifact(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("tags:", workflow)
        self.assertIn("schedule:", workflow)
        self.assertIn("runs-on: [self-hosted, macOS, fxplug-sdk]", workflow)
        self.assertIn("scripts/build_finalcut_package.py", workflow)
        self.assertIn("scripts/notarize_finalcut_package.py", workflow)
        self.assertIn("--output-dir release-finalcut", workflow)
        self.assertIn("release-finalcut/GyroflowNiyien-FinalCut-macos.zip", workflow)
        self.assertNotIn("--output-dir target", workflow)
        self.assertNotIn("--allow-unvalidated-capacity-for-testing", workflow)
        self.assertNotIn("--unsigned-for-testing", workflow)


if __name__ == "__main__":
    unittest.main()
