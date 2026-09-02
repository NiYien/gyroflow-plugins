import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
APP_SOURCE = ROOT / "finalcut" / "Xcode" / "App"
HELPER = ROOT / "tests" / "helpers" / "finalcut_fcpxml_input_main.swift"


class FinalCutFCPXMLInputContractTests(unittest.TestCase):
    def test_file_package_workspace_and_atomic_output_contract(self):
        with tempfile.TemporaryDirectory(prefix="finalcut-fcpxml-contract-") as directory:
            root = Path(directory)
            executable = root / "finalcut-fcpxml-contract"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    str(APP_SOURCE / "FCPXMLDocumentInput.swift"),
                    str(APP_SOURCE / "ProcessedProjectStore.swift"),
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
