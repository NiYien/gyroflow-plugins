# SPDX-License-Identifier: GPL-3.0-or-later
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EFFECT = ROOT / "finalcut/Xcode/Effect"

class UniformSamplingTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "requires macOS Foundation and AVFoundation")
    def test_uniform_sampling_and_legacy_persistence(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "uniform-sampling"
            build = subprocess.run([
                "xcrun", "clang", "-fobjc-arc", "-Wno-deprecated-declarations",
                "-I", str(EFFECT), "-I", str(ROOT / "finalcut/include"),
                str(EFFECT / "GFUniformSourceTiming.m"), str(EFFECT / "GFRenderState.m"),
                str(ROOT / "tests/helpers/finalcut_uniform_sampling_main.m"),
                "-framework", "Foundation", "-framework", "AVFoundation",
                "-framework", "CoreMedia", "-lz", "-o", str(binary)
            ], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertIn("1265 checks passed", run.stdout)

if __name__ == "__main__":
    unittest.main()
