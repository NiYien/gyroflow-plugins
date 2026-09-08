import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "finalcut" / "Xcode" / "App"
HELPER = ROOT / "tests" / "helpers" / "finalcut_app_model_main.swift"


class FinalCutAppModelContractTests(unittest.TestCase):
    def test_one_selection_automatically_processes_saves_and_opens(self):
        with tempfile.TemporaryDirectory(prefix="finalcut-app-model-") as directory:
            root = Path(directory)
            executable = root / "finalcut-app-model-contract"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    "-module-cache-path",
                    str(root / "module-cache"),
                    str(APP / "FinalCutStrings.swift"),
                    str(APP / "FCPXMLDocumentInput.swift"),
                    str(APP / "SecurityScopedAccess.swift"),
                    str(APP / "ReplacementProjectStore.swift"),
                    str(APP / "SandboxedRouteDWorkflow.swift"),
                    str(APP / "TemplateInstaller.swift"),
                    str(APP / "FinalCutAppModel.swift"),
                    str(HELPER),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)
            fixture = root / "fixture"
            fixture.mkdir()
            run = subprocess.run(
                [str(executable), str(fixture)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, msg=run.stdout + run.stderr)
            results = json.loads(run.stdout)
            self.assertTrue(results)
            self.assertTrue(all(results.values()), msg=results)


if __name__ == "__main__":
    unittest.main()
