# SPDX-License-Identifier: GPL-3.0-or-later
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
            attribution = (ROOT / "finalcut" / "template" / "UPSTREAM.txt").read_text(
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
                    1906,
                    *range(1910, 1920),
                    *range(1930, 1940),
                    2000,
                    2001,
                    10001,
                    10002,
                    10003,
                },
            )
            self.assertEqual(len(direct_parameters), 33)
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
            display_name = next(
                parameter
                for parameter in direct_parameters
                if parameter.attrib["id"] == "1906"
            )
            self.assertEqual(display_name.attrib["name"], "Project Display Name")
            self.assertEqual(display_name.attrib["flags"], "12889161760")
            self.assertEqual(bank_parameters[1910].attrib["name"], "Project Payload A 01")
            self.assertEqual(bank_parameters[1919].attrib["name"], "Project Payload A 10")
            self.assertEqual(bank_parameters[1930].attrib["name"], "Project Payload B 01")
            self.assertEqual(bank_parameters[1939].attrib["name"], "Project Payload B 10")
            self.assertNotIn("Project Path", text)
            self.assertNotIn("Bookmark", text)
            self.assertNotIn("Workflow", text)

    def test_project_parameter_defaults_and_english_labels_match_manual_import(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.generate(Path(directory))
            root = ET.parse(output)
            stabilization = next(
                parameter
                for parameter in root.findall(".//filter/parameter")
                if parameter.attrib["id"] == "2000"
            )
            parameters = {
                int(parameter.attrib["id"]): parameter
                for parameter in stabilization.findall("./parameter")
            }
            fov = next(
                parameter
                for parameter in root.findall(".//filter/parameter")
                if parameter.attrib["id"] == "2001"
            )

            self.assertEqual(set(parameters), set(range(2002, 2008)))
            self.assertEqual(fov.attrib["default"], "1")
            self.assertEqual(fov.attrib["value"], "1")
            self.assertEqual(parameters[2002].attrib["default"], "15")
            self.assertEqual(parameters[2002].attrib["value"], "15")
            self.assertEqual(parameters[2003].attrib["default"], "100")
            self.assertEqual(parameters[2004].attrib["default"], "0")
            self.assertEqual(parameters[2005].attrib["default"], "0")
            self.assertEqual(parameters[2006].attrib["default"], "1")
            self.assertEqual(parameters[2007].attrib["default"], "0")
            self.assertEqual(parameters[2007].attrib["name"], "Stabilization Overview")

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
            large_header = (output / "large.png").read_bytes()[:24]
            small_header = (output / "small.png").read_bytes()[:24]
            self.assertEqual(large_header[:8], b"\x89PNG\r\n\x1a\n")
            self.assertEqual(small_header[:8], b"\x89PNG\r\n\x1a\n")
            self.assertEqual(
                (
                    int.from_bytes(large_header[16:20], "big"),
                    int.from_bytes(large_header[20:24], "big"),
                ),
                (640, 360),
            )
            self.assertEqual(
                (
                    int.from_bytes(small_header[16:20], "big"),
                    int.from_bytes(small_header[20:24], "big"),
                ),
                (192, 108),
            )
            self.assertIn(
                "MIT License",
                (output / "UPSTREAM.txt").read_text(encoding="utf-8"),
            )
            self.assertEqual(self.verify(output / "Gyroflow NiYien.moef").returncode, 0)


class FinalCutTemplateInstallerTests(unittest.TestCase):
    def test_install_repair_rollback_remove_and_scope_boundaries(self):
        with tempfile.TemporaryDirectory(
            prefix=".finalcut-template-installer-",
            dir=ROOT,
        ) as directory:
            root = Path(directory)
            executable = root / "template-installer"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    "-parse-as-library",
                    "-module-cache-path",
                    str(root / "module-cache"),
                    str(ROOT / "finalcut" / "xcode" / "App" / "FinalCutStrings.swift"),
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

            fixture = root / "fixture"
            fixture.mkdir()
            run = subprocess.run(
                [str(executable), str(fixture.resolve())],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, msg=run.stdout + run.stderr)
            result = json.loads(run.stdout)
            self.assertTrue(result["preflightPassedWithoutMutation"])
            self.assertTrue(result["automaticPreparationIsIdempotent"])
            self.assertTrue(result["automaticForeignRejected"])
            self.assertEqual(result["before"], "notInstalled")
            self.assertEqual(result["installed"], "installed")
            self.assertEqual(result["bundledDrift"], "repairRequired")
            self.assertEqual(result["bundledDriftRepaired"], "installed")
            self.assertEqual(result["damaged"], "repairRequired")
            self.assertEqual(result["repaired"], "installed")
            self.assertEqual(result["previewDamaged"], "repairRequired")
            self.assertTrue(result["transactionSnapshotContained"])
            self.assertTrue(result["rollbackFailed"])
            self.assertTrue(result["rollbackPreserved"])
            self.assertTrue(result["afterCommitRollbackFailed"])
            self.assertTrue(result["afterCommitRollbackPreserved"])
            self.assertTrue(result["removed"])
            self.assertTrue(result["foreignRejected"])
            self.assertTrue(result["foreignPreserved"])
            self.assertTrue(result["escapedInstallationsRejected"])
            self.assertTrue(result["escapedInstallsCreatedNothing"])
            self.assertTrue(result["escapedRepairsRejected"])
            self.assertTrue(result["escapedRemovalsRejected"])
            self.assertTrue(result["escapedTargetsPreserved"])
            self.assertTrue(result["exactDestinationSymlinkRejectedByGuard"])
            self.assertTrue(result["ancestorSymlinkRejectedByGuard"])
            self.assertTrue(result["mutationTargetsCanonicalAndContained"])
            self.assertTrue(result["allMutationKindsObserved"])

    def test_wrapper_prepares_template_without_a_maintenance_footer(self):
        app_main = (
            ROOT / "finalcut" / "xcode" / "App" / "AppMain.swift"
        ).read_text(encoding="utf-8")
        batch_view_path = ROOT / "finalcut" / "xcode" / "App" / "BatchProcessView.swift"
        self.assertTrue(batch_view_path.is_file())
        batch_view = batch_view_path.read_text(encoding="utf-8")
        installer = (
            ROOT / "finalcut" / "xcode" / "App" / "TemplateInstaller.swift"
        ).read_text(encoding="utf-8")

        self.assertNotIn("installationArea", batch_view)
        self.assertNotIn('FinalCutStrings.text("app.title")', batch_view)
        self.assertNotIn('FinalCutStrings.text("app.subtitle")', batch_view)
        self.assertNotIn("LabeledContent", batch_view)
        self.assertIn("model.prepareTemplateIfNeeded()", batch_view)
        self.assertIn("model.errorMessage ?? model.templatePreparationError", batch_view)
        self.assertIn("--install-template-and-quit", app_main)
        self.assertIn("--preflight-template-and-quit", app_main)
        self.assertIn("--remove-template-and-quit", app_main)
        self.assertIn("static let success: Int32 = 0", installer)
        self.assertIn("static let installFailed: Int32 = 20", installer)
        self.assertIn("static let verificationFailed: Int32 = 21", installer)
        self.assertNotIn("Workflow Extension", app_main + batch_view)


if __name__ == "__main__":
    unittest.main()
