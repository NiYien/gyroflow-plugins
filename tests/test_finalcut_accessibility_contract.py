import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
APP_SOURCE = ROOT / "finalcut" / "Xcode" / "App"
HELPER = ROOT / "tests" / "helpers" / "finalcut_accessibility_main.swift"


class FinalCutAccessibilityContractTests(unittest.TestCase):
    def test_locator_and_state_machine_fail_closed(self):
        with tempfile.TemporaryDirectory(prefix="finalcut-ax-contract-") as directory:
            executable = Path(directory) / "finalcut-accessibility-contract"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    str(APP_SOURCE / "FinalCutAXModel.swift"),
                    str(APP_SOURCE / "FinalCutAXLocator.swift"),
                    str(APP_SOURCE / "FinalCutAccessibilityDriver.swift"),
                    str(HELPER),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)
            run = subprocess.run(
                [str(executable)],
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
