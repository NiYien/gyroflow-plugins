import os
import re
import shutil
import subprocess
import tempfile
import textwrap
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


@unittest.skipUnless(os.name == "nt", "Windows release contract requires PowerShell")
class WindowsDeployFailureTests(unittest.TestCase):
    def setUp(self):
        self.just = shutil.which("just")
        if self.just is None:
            self.skipTest("just is not installed")
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def run_deploy(self, module, cargo_exit=0, archive_exit=0):
        root = self.root / module
        bin_dir = root / "bin"
        target = root / "target"
        bin_dir.mkdir(parents=True)
        (target / "deploy").mkdir(parents=True)
        binaries = {"openfx": "gyroflow_ofx.dll", "adobe": "gyroflow_adobe.dll",
                    "frei0r": "gyroflow_frei0r.dll"}
        packages = {"openfx": "gyroflow-niyien-ofx-windows/GyroflowNiyien.ofx.bundle/Contents/Win64/GyroflowNiyien.ofx",
                    "adobe": "GyroflowNiyien-Adobe-windows.aex", "frei0r": "libgyroflow_niyien_frei0r.dll"}
        (target / "deploy" / binaries[module]).write_bytes(b"previous build")
        archive_marker = root / "archive-called"
        (bin_dir / "cargo.cmd").write_text(f"@echo off\r\nexit /b {cargo_exit}\r\n")
        (bin_dir / "7z.cmd").write_text(
            f'@echo off\r\necho archive>"{archive_marker}"\r\nexit /b {archive_exit}\r\n')
        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(target)
        environment["PATH"] = str(bin_dir) + os.pathsep + environment["PATH"]
        result = subprocess.run(
            [self.just, "--justfile", str(ROOT / module / "Justfile"), "deploy"],
            cwd=ROOT, env=environment, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=30)
        return result, target / packages[module], archive_marker

    def test_compiler_failure_preserves_the_cached_binary_without_publishing_it(self):
        for module in ("openfx", "adobe", "frei0r"):
            with self.subTest(module=module):
                result, package, archive_marker = self.run_deploy(module, cargo_exit=73)
                # Just's Windows script launcher reports a failed recipe as exit 1.
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertFalse(package.exists())
                self.assertFalse(archive_marker.exists())
                self.assertEqual(next((self.root / module / "target/deploy").iterdir()).read_bytes(),
                                 b"previous build")

    def test_successful_adobe_and_frei0r_builds_are_copied(self):
        for module in ("adobe", "frei0r"):
            with self.subTest(module=module):
                result, package, _ = self.run_deploy(module)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(package.read_bytes(), b"previous build")

    def test_openfx_archive_failure_is_reported(self):
        result, _, archive_marker = self.run_deploy("openfx", archive_exit=74)
        self.assertTrue(archive_marker.exists())
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_workflow_stops_before_deploy_when_dependency_installation_fails(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        match = re.search(r"- name: Build plugins \(Windows\).*?run: \|\n((?:          [^\n]+\n)+)",
                          workflow, re.DOTALL)
        self.assertIsNotNone(match)
        script = self.root / "workflow.ps1"
        script.write_text(textwrap.dedent(match[1]))
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        deploy_marker = self.root / "deploy-called"
        (bin_dir / "just.cmd").write_text(
            '@echo off\r\nif "%1"=="install-deps" exit /b 73\r\n'
            f'echo deploy>"{deploy_marker}"\r\nexit /b 0\r\n')
        environment = os.environ.copy()
        environment["PATH"] = str(bin_dir) + os.pathsep + environment["PATH"]
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(script)],
            env=environment, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30)
        self.assertEqual(result.returncode, 73, result.stdout + result.stderr)
        self.assertFalse(deploy_marker.exists())


if __name__ == "__main__":
    unittest.main()
