import hashlib
import json
import plistlib
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GENERATOR = ROOT / "scripts" / "generate_finalcut_template.py"
VERIFIER = ROOT / "scripts" / "verify_finalcut_template.py"
ASSEMBLER = ROOT / "scripts" / "assemble_finalcut_template_resources.py"
XPC_INFO = ROOT / "finalcut" / "xcode" / "Effect" / "Info.plist"
IDENTITY = ROOT / "finalcut" / "config" / "identity.json"
BASELINE = (
    ROOT
    / "probes"
    / "finalcut_phase0"
    / "template"
    / "upstream"
    / "Gyroflow Toolbox.moef"
)


class FinalCutProductionTemplateTests(unittest.TestCase):
    def generate(self, directory: Path) -> Path:
        output = directory / "Gyroflow NiYien.moef"
        result = subprocess.run(
            [sys.executable, str(GENERATOR), "--output", str(output)],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        return output

    def verify(self, template: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(VERIFIER), str(template), str(XPC_INFO)],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def test_generator_preserves_pinned_mit_baseline_and_owned_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.generate(Path(directory))
            verified = self.verify(output)
            identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
            root = ET.parse(output)
            effect = root.find(".//filter")

            self.assertEqual(verified.returncode, 0, msg=verified.stderr)
            self.assertEqual(
                hashlib.sha256(BASELINE.read_bytes()).hexdigest(),
                "42dcf66155aecfc9010af748071976f0868c4d403c5fb2f6495fbda380f3a70e",
            )
            self.assertEqual(effect.attrib["pluginUUID"], identity["effect_uuid"])
            self.assertEqual(effect.attrib["pluginVersion"], identity["marketing_version"])
            attribution = (ROOT / "finalcut" / "template" / "UPSTREAM.md").read_text(
                encoding="utf-8"
            )
            self.assertIn("MIT License", attribution)
            self.assertIn("cda60919e53f600daf87e2f17ba55d995f134253", attribution)

    def test_mapping_has_one_effect_source_filter_and_no_path_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.generate(Path(directory))
            root = ET.parse(output)
            filters = root.findall(".//filter")
            published = {
                (target.attrib["channel"], target.attrib["name"])
                for target in root.findall(".//publishSettings/target")
            }
            direct_parameters = filters[0].findall("./parameter")
            direct_ids = {int(parameter.attrib["id"]) for parameter in direct_parameters}
            bank_parameters = {
                int(parameter.attrib["id"]): parameter
                for parameter in direct_parameters
                if int(parameter.attrib["id"])
                in {1904, 1905, *range(1910, 1920), *range(1930, 1940)}
            }
            text = output.read_text(encoding="utf-8")

            self.assertEqual(len(filters), 1)
            self.assertEqual(
                root.findall(".//scenenode[@name='Effect Source']/filter"),
                filters,
            )
            self.assertEqual(
                published,
                {
                    ("./1000", "Gyroflow Project"),
                    ("./2000", "Stabilization"),
                },
            )
            self.assertEqual(
                direct_ids,
                {
                    1,
                    1000,
                    1901,
                    1902,
                    1903,
                    1904,
                    1905,
                    *range(1910, 1920),
                    *range(1930, 1940),
                    2000,
                    10001,
                    10002,
                    10003,
                },
            )
            self.assertEqual(len(direct_parameters), 31)
            self.assertEqual(
                next(
                    parameter
                    for parameter in direct_parameters
                    if parameter.attrib["id"] == "1901"
                ).attrib["name"],
                "Instance Identity",
            )
            self.assertEqual(len(bank_parameters), 22)
            self.assertTrue(
                all(
                    parameter.attrib["flags"] == "12889161760"
                    for parameter in bank_parameters.values()
                )
            )
            self.assertEqual(
                bank_parameters[1904].attrib["name"], "Project Payload Manifest A"
            )
            self.assertEqual(
                bank_parameters[1905].attrib["name"], "Project Payload Manifest B"
            )
            self.assertEqual(bank_parameters[1910].attrib["name"], "Project Payload A 01")
            self.assertEqual(bank_parameters[1919].attrib["name"], "Project Payload A 10")
            self.assertEqual(bank_parameters[1930].attrib["name"], "Project Payload B 01")
            self.assertEqual(bank_parameters[1939].attrib["name"], "Project Payload B 10")
            self.assertNotIn("Project Path", text)
            self.assertNotIn("Bookmark", text)
            self.assertNotIn("Workflow", text)

    def test_opaque_or_version_drift_blocks_template(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = self.generate(root)
            output.write_text(
                output.read_text(encoding="utf-8").replace(
                    "</defaultVal>", "X</defaultVal>", 1
                ),
                encoding="utf-8",
            )
            opaque = self.verify(output)
            self.assertNotEqual(opaque.returncode, 0)
            self.assertIn("opaque", opaque.stderr)

            output = self.generate(root)
            output.write_text(
                output.read_text(encoding="utf-8").replace(
                    'pluginVersion="2.1.2"', 'pluginVersion="999"', 1
                ),
                encoding="utf-8",
            )
            version = self.verify(output)
            self.assertNotEqual(version.returncode, 0)
            self.assertIn("version", version.stderr)

    def test_generation_requires_no_motion_installation(self):
        self.assertFalse(Path("/Applications/Motion.app").exists())
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(self.verify(self.generate(Path(directory))).returncode, 0)

    def test_assembler_creates_wrapper_resource_layout_with_owned_previews(self):
        with tempfile.TemporaryDirectory() as directory:
            output = (
                Path(directory)
                / "Resources"
                / "Motion Templates"
                / "Effects.localized"
                / "NiYien"
                / "Gyroflow"
            )
            result = subprocess.run(
                [sys.executable, str(ASSEMBLER), "--output", str(output)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            self.assertTrue((output / "Gyroflow NiYien.moef").is_file())
            self.assertTrue((output / "large.png").is_file())
            self.assertTrue((output / "small.png").is_file())
            self.assertIn(
                "MIT License",
                (output / "UPSTREAM.md").read_text(encoding="utf-8"),
            )
            self.assertEqual(self.verify(output / "Gyroflow NiYien.moef").returncode, 0)


class FinalCutTemplateInstallerTests(unittest.TestCase):
    def test_install_repair_rollback_remove_and_scope_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "template-installer"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    "-parse-as-library",
                    "-module-cache-path",
                    str(root / "module-cache"),
                    str(ROOT / "finalcut" / "xcode" / "App" / "TemplateInstaller.swift"),
                    str(
                        ROOT
                        / "tests"
                        / "helpers"
                        / "finalcut_template_installer_main.swift"
                    ),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)

            run = subprocess.run(
                [str(executable), str(root / "fixture")],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, msg=run.stdout + run.stderr)
            result = json.loads(run.stdout)
            self.assertEqual(result["before"], "notInstalled")
            self.assertEqual(result["installed"], "installed")
            self.assertEqual(result["damaged"], "repairRequired")
            self.assertEqual(result["repaired"], "installed")
            self.assertEqual(result["previewDamaged"], "repairRequired")
            self.assertTrue(result["rollbackFailed"])
            self.assertTrue(result["rollbackPreserved"])
            self.assertTrue(result["afterCommitRollbackFailed"])
            self.assertTrue(result["afterCommitRollbackPreserved"])
            self.assertTrue(result["removed"])
            self.assertTrue(result["foreignRejected"])
            self.assertTrue(result["foreignPreserved"])

    def test_wrapper_ui_exposes_versions_route_d_behavior_and_stable_command_mode(self):
        app = (ROOT / "finalcut" / "xcode" / "App" / "AppMain.swift").read_text(
            encoding="utf-8"
        )
        installer = (
            ROOT / "finalcut" / "xcode" / "App" / "TemplateInstaller.swift"
        ).read_text(encoding="utf-8")

        for label in ("App", "FxPlug XPC", "Template", "Status"):
            self.assertIn(f'Text("{label}")', app)
        self.assertIn("Install / Repair", app)
        self.assertIn("Remove Template", app)
        self.assertIn("original project is preserved", app)
        self.assertIn("assigns the imported project a new UID", app)
        self.assertIn("--install-template-and-quit", app)
        self.assertIn("static let success: Int32 = 0", installer)
        self.assertIn("static let installFailed: Int32 = 20", installer)
        self.assertIn("static let verificationFailed: Int32 = 21", installer)
        self.assertNotIn("Workflow Extension", app)

    def test_wrapper_ui_exposes_one_click_preview_and_permanent_manual_fallback(self):
        app = (ROOT / "finalcut" / "xcode" / "App" / "AppMain.swift").read_text(
            encoding="utf-8"
        )
        for text in (
            "Process Current Final Cut Project",
            "Choose FCPXML or FCPXMLD",
            "Batch Preview",
            "Inserted",
            "Updated",
            "Skipped",
            "Failed",
            "Confirm Import as New Project",
            "The original Final Cut project remains unchanged",
        ):
            self.assertIn(text, app)
        self.assertIn('UTType(filenameExtension: "fcpxmld")', app)
        self.assertIn("ProgressView", app)
        self.assertIn("accessibilityHint", app)
        self.assertIn("minHeight: 44", app)
        self.assertNotIn("processAndImportFCPXML", app)


if __name__ == "__main__":
    unittest.main()
