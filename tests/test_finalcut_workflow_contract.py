import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
APP_SOURCE = ROOT / "finalcut" / "Xcode" / "App"
HELPER = ROOT / "tests" / "helpers" / "finalcut_workflow_main.swift"


class FinalCutWorkflowContractTests(unittest.TestCase):
    def test_one_click_manual_preview_and_confirmation_contract(self):
        with tempfile.TemporaryDirectory(prefix="finalcut-workflow-contract-") as directory:
            root = Path(directory)
            executable = root / "finalcut-workflow-contract"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    str(APP_SOURCE / "FinalCutAXModel.swift"),
                    str(APP_SOURCE / "FinalCutAXLocator.swift"),
                    str(APP_SOURCE / "FCPXMLDocumentInput.swift"),
                    str(APP_SOURCE / "ProcessedProjectStore.swift"),
                    str(APP_SOURCE / "OneClickRouteDWorkflow.swift"),
                    str(HELPER),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)
            fixture_root = root / "fixture"
            fixture_root.mkdir()
            run = subprocess.run(
                [str(executable), str(fixture_root)],
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
