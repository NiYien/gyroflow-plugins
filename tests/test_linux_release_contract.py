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

    def test_linux_openfx_deploy_derives_a_nonempty_package_version(self):
        justfile = (ROOT / "openfx" / "Justfile").read_text(encoding="utf-8")
        linux_deploy = justfile.split("[linux]", 1)[1]

        self.assertIn("version=$(cargo pkgid -p gyroflow-ofx", linux_deploy)
        self.assertLess(linux_deploy.index("version=$(cargo pkgid"), linux_deploy.index('cd "{{TargetDir}}"'))

    def test_linux_openfx_returns_to_target_root_before_package_verification(self):
        justfile = (ROOT / "openfx" / "Justfile").read_text(encoding="utf-8")
        linux_deploy = justfile.split("[linux]", 1)[1]

        self.assertIn("pushd gyroflow-niyien-ofx-linux", linux_deploy)
        self.assertIn("popd", linux_deploy)
        self.assertNotIn("cd gyroflow-niyien-ofx-linux && zip", linux_deploy)

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

    def test_linux_discovers_the_distribution_libclang_instead_of_pin_to_13(self):
        justfile = (ROOT / "Justfile").read_text(encoding="utf-8")

        self.assertNotIn("libclang-13-dev", justfile)
        self.assertNotIn("/usr/lib/llvm-13/lib/", justfile)
        self.assertGreaterEqual(justfile.count("find /usr/lib/llvm-*/lib"), 2)


class LinuxZipContractTests(unittest.TestCase):
    def setUp(self):
        self.verifier = load_verifier()

    def test_verifier_defers_type_annotations_for_bullseye_python(self):
        script = VERIFY_SCRIPT.read_text(encoding="utf-8")

        self.assertTrue(script.startswith("from __future__ import annotations\n"))

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
