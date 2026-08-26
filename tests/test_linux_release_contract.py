import importlib.util
import re
import stat
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERIFY_SCRIPT = ROOT / "scripts" / "verify_linux_openfx_package.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_linux_openfx_package", VERIFY_SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Unable to load {VERIFY_SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class LinuxWorkflowContractTests(unittest.TestCase):
    def test_linux_deploy_skips_adobe_but_keeps_openfx_and_frei0r(self):
        justfile = (ROOT / "Justfile").read_text(encoding="utf-8")

        self.assertIn('if os() == "linux"', justfile)
        self.assertIn('just -f openfx/Justfile deploy', justfile)
        self.assertIn('just -f frei0r/Justfile deploy', justfile)
        self.assertIn('Skipping Adobe deploy on Linux', justfile)

    def test_linux_workflow_never_uploads_adobe(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")

        match = re.search(
            r"- uses: actions/upload-artifact@v4\s+"
            r"if: \$\{\{ ([^}]+) \}\}\s+"
            r"with:\s+name: GyroflowNiyien-Adobe-\$\{\{ matrix\.targets\.type \}\}",
            workflow,
        )
        self.assertIsNotNone(match)
        self.assertEqual(match.group(1), "matrix.targets.type == 'windows'")
        self.assertNotIn("GyroflowNiyien-Adobe-linux", workflow)
        self.assertIn("name: GyroflowNiyien-OpenFX-${{ matrix.targets.type }}", workflow)


class LinuxZipContractTests(unittest.TestCase):
    def setUp(self):
        self.verifier = load_verifier()

    def write_zip(self, path: Path, missing: str | None = None):
        entries = {
            "GyroflowNiyien.ofx.bundle/Contents/Linux-x86-64/GyroflowNiyien.ofx": b"plugin",
            "GyroflowNiyien.ofx.bundle/Contents/Info.plist": b"plist",
            "GyroflowNiyien.ofx.bundle/Contents/LICENSE": b"license",
            "GyroflowNiyien.ofx.bundle/Contents/version.txt": b"2.1.2\n",
            "ResolveScripts/Gyroflow NiYien Auto Cut Current Clip.lua": b"clip",
            "ResolveScripts/Gyroflow NiYien Auto Cut Current Track.lua": b"track",
            "ResolveScripts/gyroflow_autocut_common.inc": b"common",
        }
        with zipfile.ZipFile(path, "w") as archive:
            for name, contents in entries.items():
                if name == missing:
                    continue
                info = zipfile.ZipInfo(name)
                info.create_system = 3
                mode = 0o755 if name.endswith("GyroflowNiyien.ofx") else 0o644
                info.external_attr = (stat.S_IFREG | mode) << 16
                archive.writestr(info, contents)

    def test_complete_linux_zip_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory) / "GyroflowNiyien-OpenFX-linux.zip"
            self.write_zip(package)

            metadata = self.verifier.verify_package(package)

        self.assertEqual(metadata["version"], "2.1.2")
        self.assertEqual(metadata["binary_mode"] & stat.S_IXUSR, stat.S_IXUSR)

    def test_missing_required_member_is_rejected(self):
        missing = "ResolveScripts/gyroflow_autocut_common.inc"
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory) / "GyroflowNiyien-OpenFX-linux.zip"
            self.write_zip(package, missing=missing)

            with self.assertRaisesRegex(ValueError, missing):
                self.verifier.verify_package(package)


if __name__ == "__main__":
    unittest.main()
