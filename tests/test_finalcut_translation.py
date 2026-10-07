# SPDX-License-Identifier: GPL-3.0-or-later
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EFFECT = ROOT / "finalcut/Xcode/Effect"


class TranslationSettingsTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "requires macOS FxPlug")
    def test_action_readback_rollback_reopen_reload_and_undo(self):
        from test_finalcut_state_contract import compile_helper
        import json
        with tempfile.TemporaryDirectory() as directory:
            executable = compile_helper(Path(directory), "translation-committer", [
                EFFECT / "GFParameterCommitter.m",
                EFFECT / "GFTranslationSettings.m",
                ROOT / "tests/helpers/finalcut_parameter_committer_main.m",
            ], fxplug=True)
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            run = subprocess.run([str(executable), "--translation"], capture_output=True, text=True, env=environment)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            result = json.loads(run.stdout)
            for key in ("saved", "rolledBack", "restartRestored", "reloadReset", "staleEditRejected", "undoRestored", "balancedActions"):
                self.assertTrue(result[key], key)
            self.assertEqual(result["outsideActionWrites"], 0)

    @unittest.skipUnless(sys.platform == "darwin", "requires macOS Foundation")
    def test_saved_values_legacy_state_and_project_switch(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "translation-settings"
            build = subprocess.run([
                "xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                "-framework", "Foundation", "-I", str(EFFECT), "-I", str(ROOT / "finalcut/include"),
                str(EFFECT / "GFTranslationSettings.m"),
                str(ROOT / "tests/helpers/finalcut_translation_settings_main.m"), "-o", str(executable),
            ], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(executable)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == "__main__":
    unittest.main()
