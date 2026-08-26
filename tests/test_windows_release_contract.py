import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(os.name == "nt", "Windows release contract requires PowerShell")
class WindowsOpenFxDeployContractTests(unittest.TestCase):
    def test_windows_deploy_recipe_executes_under_powershell(self):
        just = shutil.which("just")
        if just is None:
            self.skipTest("just is not installed")

        with tempfile.TemporaryDirectory() as directory:
            temporary_root = Path(directory)
            bin_dir = temporary_root / "bin"
            target_dir = temporary_root / "target"
            deploy_dir = target_dir / "deploy"
            bin_dir.mkdir()
            deploy_dir.mkdir(parents=True)

            (bin_dir / "cargo.cmd").write_text("@echo off\r\nexit /b 0\r\n", encoding="utf-8")
            (bin_dir / "7z.cmd").write_text("@echo off\r\nexit /b 0\r\n", encoding="utf-8")
            (deploy_dir / "gyroflow_ofx.dll").write_bytes(b"plugin")

            environment = os.environ.copy()
            environment["CARGO_TARGET_DIR"] = str(target_dir)
            environment["PATH"] = str(bin_dir) + os.pathsep + environment["PATH"]

            result = subprocess.run(
                [just, "--justfile", str(ROOT / "openfx" / "Justfile"), "deploy"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            packaged_plugin = (
                target_dir
                / "gyroflow-niyien-ofx-windows"
                / "GyroflowNiyien.ofx.bundle"
                / "Contents"
                / "Win64"
                / "GyroflowNiyien.ofx"
            )
            self.assertEqual(packaged_plugin.read_bytes(), b"plugin")


if __name__ == "__main__":
    unittest.main()
