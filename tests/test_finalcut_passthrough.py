# SPDX-License-Identifier: GPL-3.0-or-later
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EFFECT = ROOT / "finalcut/Xcode/Effect"

class FinalCutPassthroughTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "requires real macOS Metal")
    def test_real_metal_preserves_pixels_across_preview_sizes_origins_and_padding(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "passthrough"
            build = subprocess.run([
                "xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                "-framework", "Foundation", "-framework", "Metal",
                "-I", str(EFFECT), "-I", str(ROOT / "finalcut/include"),
                str(EFFECT / "GFMetalPassthrough.m"),
                str(ROOT / "tests/helpers/finalcut_metal_passthrough_main.m"),
                "-o", str(executable),
            ], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(executable)], capture_output=True, text=True)
            if run.returncode == 77:
                self.skipTest("no Metal device is available")
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertIn("16 real Metal passthrough image cases passed", run.stdout)

if __name__ == "__main__":
    unittest.main()
