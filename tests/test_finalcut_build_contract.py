import json
import os
import plistlib
import re
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SDK_PROBE = ROOT / "scripts" / "check_finalcut_sdk.py"


class FinalCutSdkProbeTests(unittest.TestCase):
    def make_sdk(self, root: Path) -> Path:
        sdk = root / "FxPlug.sdk"
        frameworks = sdk / "Library" / "Frameworks"
        fxplug_headers = frameworks / "FxPlug.framework" / "Headers"
        plugin_manager_headers = frameworks / "PluginManager.framework" / "Headers"
        fxplug_headers.mkdir(parents=True)
        plugin_manager_headers.mkdir(parents=True)
        (fxplug_headers / "FxPlugSDK.h").write_text("// fixture\n", encoding="utf-8")
        (plugin_manager_headers / "PluginManager.h").write_text(
            "// fixture\n", encoding="utf-8"
        )
        target_info = {
            "main_library": {
                "target_info": [
                    {"min_deployment": "13.0", "target": "x86_64-macos"},
                    {"min_deployment": "13.0", "target": "arm64-macos"},
                ]
            }
        }
        (frameworks / "FxPlug.framework" / "FxPlug.tbd").write_text(
            json.dumps(target_info), encoding="utf-8"
        )
        (frameworks / "PluginManager.framework" / "PluginManager.tbd").write_text(
            json.dumps(target_info), encoding="utf-8"
        )
        with (sdk / "SDKSettings.plist").open("wb") as settings_file:
            plistlib.dump(
                {
                    "CanonicalName": "FxPlugSDK",
                    "MinimumSupportedToolsVersion": "15.0",
                    "Version": "1.0",
                },
                settings_file,
            )
        return sdk

    def run_probe(
        self,
        sdk: Path,
        stamp: Path,
        xcode_version: str = "15.0",
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SDK_PROBE),
                "--sdk-root",
                str(sdk),
                "--xcode-version",
                xcode_version,
                "--stamp",
                str(stamp),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def test_missing_sdk_fails_before_writing_release_stamp(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stamp = root / "sdk-ready.json"
            result = self.run_probe(root / "missing.sdk", stamp)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FxPlug SDK not found", result.stderr)
            self.assertFalse(stamp.exists())

    def test_incomplete_sdk_names_the_missing_runtime_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.make_sdk(root)
            missing = sdk / "Library" / "Frameworks" / "PluginManager.framework"
            missing.rename(root / "PluginManager.framework.disabled")
            stamp = root / "sdk-ready.json"
            result = self.run_probe(sdk, stamp)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("PluginManager.framework", result.stderr)
            self.assertFalse(stamp.exists())

    def test_xcode_older_than_sdk_minimum_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.make_sdk(root)
            stamp = root / "sdk-ready.json"
            result = self.run_probe(sdk, stamp, xcode_version="14.3")

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("requires Xcode 15.0 or newer", result.stderr)
            self.assertFalse(stamp.exists())

    def test_complete_sdk_writes_stamp_only_after_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.make_sdk(root)
            stamp = root / "sdk-ready.json"
            result = self.run_probe(sdk, stamp, xcode_version="26.6")

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            self.assertTrue(stamp.is_file())
            self.assertIn('"xcode_version": "26.6"', stamp.read_text(encoding="utf-8"))
            self.assertIn(
                '"minimum_macos_deployment": "13.0"',
                stamp.read_text(encoding="utf-8"),
            )
            self.assertIn(str(sdk), result.stdout)

    def test_deployment_target_older_than_framework_floor_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.make_sdk(root)
            result = subprocess.run(
                [
                    sys.executable,
                    str(SDK_PROBE),
                    "--sdk-root",
                    str(sdk),
                    "--xcode-version",
                    "26.6",
                    "--deployment-target",
                    "12.6",
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("require macOS 13.0", result.stderr)


class FinalCutWorkspaceSkeletonTests(unittest.TestCase):
    def test_cargo_workspace_exposes_finalcut_static_library(self):
        result = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        metadata = json.loads(result.stdout)
        finalcut_packages = [
            package
            for package in metadata["packages"]
            if package["name"] == "gyroflow-finalcut"
        ]
        self.assertEqual(len(finalcut_packages), 1)
        self.assertIn(
            "staticlib",
            finalcut_packages[0]["targets"][0]["crate_types"],
        )

    def test_xcode_project_exposes_only_app_and_fxplug_targets(self):
        project = (
            ROOT
            / "finalcut"
            / "Xcode"
            / "GyroflowFinalCut.xcodeproj"
        )
        result = subprocess.run(
            [
                "xcodebuild",
                "-project",
                str(project),
                "-list",
                "-json",
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        project_info = json.loads(result.stdout)["project"]
        self.assertEqual(
            sorted(project_info["targets"]),
            ["GyroflowFinalCutApp", "GyroflowFinalCutEffect"],
        )
        self.assertNotIn("Workflow", result.stdout)

    def test_app_target_compiles_accessibility_components(self):
        project = (
            ROOT
            / "finalcut"
            / "Xcode"
            / "GyroflowFinalCut.xcodeproj"
            / "project.pbxproj"
        ).read_text(encoding="utf-8")
        app_sources = re.search(
            r"700000000000000000000001 = \{isa = PBXSourcesBuildPhase;.*?files = \((.*?)\);",
            project,
            re.DOTALL,
        )
        self.assertIsNotNone(app_sources)
        for source in (
            "FinalCutAXModel.swift",
            "FinalCutAXLocator.swift",
            "FinalCutAccessibilityDriver.swift",
        ):
            file_reference = re.search(
                rf"([A-F0-9]+) = \{{isa = PBXFileReference;[^\n]+path = {re.escape(source)};",
                project,
            )
            self.assertIsNotNone(file_reference, msg=source)
            build_reference = re.search(
                rf"([A-F0-9]+) = \{{isa = PBXBuildFile; fileRef = {file_reference.group(1)};",
                project,
            )
            self.assertIsNotNone(build_reference, msg=source)
            self.assertIn(build_reference.group(1), app_sources.group(1))

    def test_app_target_compiles_fcpxml_input_components(self):
        project = (
            ROOT
            / "finalcut"
            / "Xcode"
            / "GyroflowFinalCut.xcodeproj"
            / "project.pbxproj"
        ).read_text(encoding="utf-8")
        app_sources = re.search(
            r"700000000000000000000001 = \{isa = PBXSourcesBuildPhase;.*?files = \((.*?)\);",
            project,
            re.DOTALL,
        )
        self.assertIsNotNone(app_sources)
        for source in ("FCPXMLDocumentInput.swift", "ProcessedProjectStore.swift"):
            file_reference = re.search(
                rf"([A-F0-9]+) = \{{isa = PBXFileReference;[^\n]+path = {re.escape(source)};",
                project,
            )
            self.assertIsNotNone(file_reference, msg=source)
            build_reference = re.search(
                rf"([A-F0-9]+) = \{{isa = PBXBuildFile; fileRef = {file_reference.group(1)};",
                project,
            )
            self.assertIsNotNone(build_reference, msg=source)
            self.assertIn(build_reference.group(1), app_sources.group(1))

    def test_app_target_compiles_one_click_workflow_components(self):
        project = (
            ROOT
            / "finalcut"
            / "Xcode"
            / "GyroflowFinalCut.xcodeproj"
            / "project.pbxproj"
        ).read_text(encoding="utf-8")
        app_sources = re.search(
            r"700000000000000000000001 = \{isa = PBXSourcesBuildPhase;.*?files = \((.*?)\);",
            project,
            re.DOTALL,
        )
        self.assertIsNotNone(app_sources)
        for source in (
            "OneClickRouteDWorkflow.swift",
            "OneClickRouteDProduction.swift",
        ):
            file_reference = re.search(
                rf"([A-F0-9]+) = \{{isa = PBXFileReference;[^\n]+path = {re.escape(source)};",
                project,
            )
            self.assertIsNotNone(file_reference, msg=source)
            build_reference = re.search(
                rf"([A-F0-9]+) = \{{isa = PBXBuildFile; fileRef = {file_reference.group(1)};",
                project,
            )
            self.assertIsNotNone(build_reference, msg=source)
            self.assertIn(build_reference.group(1), app_sources.group(1))

    def test_app_omits_temporary_route_d_diagnostic_harness(self):
        app_root = ROOT / "finalcut" / "Xcode" / "App"
        source = (app_root / "AppMain.swift").read_text(
            encoding="utf-8"
        )
        for temporary_hook in (
            "--diagnose-route-d-export-and-quit",
            "diagnostic-run-current-project-once",
            "autoProcessCurrentProjectOnAppear",
            "restoreRegularActivationAfterDiagnosticPreview",
            "NSApplication.shared.setActivationPolicy(.accessory)",
            '.keyboardShortcut("p", modifiers: [.command, .shift])',
            "LocalFinalCutAutomationDiagnostics",
            "LocalRouteDWorkflowDiagnostics",
        ):
            with self.subTest(temporary_hook=temporary_hook):
                self.assertNotIn(temporary_hook, source)
        all_sources = "\n".join(
            path.read_text(encoding="utf-8") for path in app_root.glob("*.swift")
        )
        for temporary_diagnostic in (
            "finalcut-route-d-diagnostic.json",
            "finalcut-route-d-workflow-diagnostic.json",
            "FinalCutAutomationDiagnosticRecord",
            "RouteDWorkflowDiagnosticRecord",
        ):
            with self.subTest(temporary_diagnostic=temporary_diagnostic):
                self.assertNotIn(temporary_diagnostic, all_sources)
        production = (
            ROOT / "finalcut" / "Xcode" / "App" / "OneClickRouteDProduction.swift"
        ).read_text(encoding="utf-8")
        self.assertNotIn("DispatchQueue.main.sync", production)
        driver = (
            ROOT / "finalcut" / "Xcode" / "App" / "FinalCutAccessibilityDriver.swift"
        ).read_text(encoding="utf-8")
        self.assertIn("Thread.isMainThread", driver)
        self.assertIn("DispatchQueue.main.sync", driver)

    def test_production_workspace_has_shared_build_schemes(self):
        workspace = ROOT / "finalcut" / "GyroflowFinalCut.xcworkspace"
        result = subprocess.run(
            ["xcodebuild", "-workspace", str(workspace), "-list", "-json"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        schemes = json.loads(result.stdout)["workspace"]["schemes"]
        self.assertEqual(
            sorted(schemes),
            ["GyroflowFinalCutApp", "GyroflowFinalCutEffect"],
        )


class FinalCutIdentityAndEntitlementTests(unittest.TestCase):
    PROJECT = (
        ROOT
        / "finalcut"
        / "Xcode"
        / "GyroflowFinalCut.xcodeproj"
    )
    IDENTITY = ROOT / "finalcut" / "config" / "identity.json"

    def target_settings(self, target: str) -> dict[str, str]:
        result = subprocess.run(
            [
                "xcodebuild",
                "-project",
                str(self.PROJECT),
                "-target",
                target,
                "-configuration",
                "Release",
                "-showBuildSettings",
                "-json",
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        return json.loads(result.stdout)[0]["buildSettings"]

    def test_bundle_identifiers_uuid_and_versions_share_owned_config(self):
        self.assertTrue(self.IDENTITY.is_file())
        identity = json.loads(self.IDENTITY.read_text(encoding="utf-8"))
        with (ROOT / "finalcut" / "Xcode" / "Effect" / "Info.plist").open("rb") as source:
            effect_info = plistlib.load(source)
        cargo_metadata = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        finalcut_package = next(
            package
            for package in json.loads(cargo_metadata.stdout)["packages"]
            if package["name"] == "gyroflow-finalcut"
        )
        app_settings = self.target_settings("GyroflowFinalCutApp")
        effect_settings = self.target_settings("GyroflowFinalCutEffect")

        self.assertEqual(identity["app_bundle_id"], "com.niyien.gyroflow.finalcut")
        self.assertEqual(
            identity["effect_bundle_id"],
            "com.niyien.gyroflow.finalcut.effect",
        )
        self.assertEqual(app_settings["PRODUCT_BUNDLE_IDENTIFIER"], identity["app_bundle_id"])
        self.assertEqual(
            effect_settings["PRODUCT_BUNDLE_IDENTIFIER"],
            identity["effect_bundle_id"],
        )
        self.assertEqual(
            effect_info["ProPlugPlugInGroupList"][0]["uuid"],
            identity["group_uuid"],
        )
        self.assertEqual(
            effect_info["ProPlugPlugInList"][0]["uuid"],
            identity["effect_uuid"],
        )
        self.assertEqual(app_settings["MARKETING_VERSION"], finalcut_package["version"])
        self.assertEqual(effect_settings["MARKETING_VERSION"], finalcut_package["version"])
        self.assertEqual(app_settings["CURRENT_PROJECT_VERSION"], identity["build_version"])
        self.assertEqual(effect_settings["CURRENT_PROJECT_VERSION"], identity["build_version"])
        self.assertNotEqual(identity["effect_uuid"], identity["effect_bundle_id"])

    def test_release_targets_use_same_team_hardened_runtime_and_entitlements(self):
        app_settings = self.target_settings("GyroflowFinalCutApp")
        effect_settings = self.target_settings("GyroflowFinalCutEffect")

        for settings in (app_settings, effect_settings):
            self.assertEqual(settings.get("DEVELOPMENT_TEAM"), "H59FJRN2AM")
            self.assertEqual(settings.get("ENABLE_HARDENED_RUNTIME"), "YES")
        self.assertTrue(
            app_settings.get("CODE_SIGN_ENTITLEMENTS", "").endswith(
                "App/App.entitlements"
            )
        )
        self.assertTrue(
            effect_settings.get("CODE_SIGN_ENTITLEMENTS", "").endswith(
                "Effect/Effect.entitlements"
            )
        )

    def test_entitlements_are_minimal_and_have_no_app_group(self):
        app_path = ROOT / "finalcut" / "Xcode" / "App" / "App.entitlements"
        effect_path = (
            ROOT / "finalcut" / "Xcode" / "Effect" / "Effect.entitlements"
        )
        self.assertTrue(app_path.is_file())
        self.assertTrue(effect_path.is_file())
        with app_path.open("rb") as source:
            app_entitlements = plistlib.load(source)
        with effect_path.open("rb") as source:
            effect_entitlements = plistlib.load(source)

        self.assertEqual(
            app_entitlements,
            {},
        )
        self.assertEqual(
            effect_entitlements,
            {
                "com.apple.security.app-sandbox": True,
                "com.apple.security.files.bookmarks.app-scope": True,
                "com.apple.security.files.user-selected.read-only": True,
            },
        )
        self.assertNotIn("com.apple.security.application-groups", app_entitlements)
        self.assertNotIn("com.apple.security.application-groups", effect_entitlements)


class FinalCutGpuFeatureIsolationTests(unittest.TestCase):
    def workspace_packages(self) -> dict[str, dict]:
        result = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        return {
            package["name"]: package
            for package in json.loads(result.stdout)["packages"]
        }

    def test_plugin_base_defaults_preserve_opencl_for_existing_hosts(self):
        packages = self.workspace_packages()
        plugin_base = packages["gyroflow-plugin-base"]

        self.assertEqual(plugin_base["features"].get("default"), ["opencl"])
        self.assertEqual(
            plugin_base["features"].get("opencl"),
            ["gyroflow-core/use-opencl"],
        )
        self.assertEqual(plugin_base["features"].get("metal"), [])
        for package_name in ("gyroflow_adobe", "gyroflow-ofx", "gyroflow-frei0r"):
            dependency = next(
                dependency
                for dependency in packages[package_name]["dependencies"]
                if dependency["name"] == "gyroflow-plugin-base"
            )
            self.assertTrue(dependency["uses_default_features"])
            self.assertEqual(dependency["features"], [])

    def test_finalcut_selects_metal_marker_without_opencl_default(self):
        packages = self.workspace_packages()
        dependencies = [
            dependency
            for dependency in packages["gyroflow-finalcut"]["dependencies"]
            if dependency["name"] == "gyroflow-plugin-base"
        ]
        self.assertEqual(len(dependencies), 1)
        dependency = dependencies[0]

        self.assertFalse(dependency["uses_default_features"])
        self.assertEqual(dependency["features"], ["metal"])
        self.assertNotIn("opencl", dependency["features"])


class FinalCutMotionlessBuildTests(unittest.TestCase):
    def test_workspace_and_text_template_build_without_motion(self):
        self.assertFalse(Path("/Applications/Motion.app").exists())
        workspace = ROOT / "finalcut" / "GyroflowFinalCut.xcworkspace"
        sdk_probe = ROOT / "scripts" / "check_finalcut_sdk.py"
        template_generator = (
            ROOT
            / "probes"
            / "finalcut_phase0"
            / "scripts"
            / "generate_template.py"
        )

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            preflight = subprocess.run(
                [sys.executable, str(sdk_probe)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                preflight.returncode,
                0,
                msg=preflight.stdout + preflight.stderr,
            )

            build = subprocess.run(
                [
                    "xcodebuild",
                    "-workspace",
                    str(workspace),
                    "-scheme",
                    "GyroflowFinalCutApp",
                    "-configuration",
                    "Debug",
                    "-derivedDataPath",
                    str(output / "DerivedData"),
                    "CODE_SIGNING_ALLOWED=NO",
                    "ONLY_ACTIVE_ARCH=YES",
                    "ARCHS=arm64",
                    "build",
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)
            products = output / "DerivedData" / "Build" / "Products" / "Debug"
            self.assertTrue((products / "GyroflowNiYien Final Cut.app").is_dir())
            self.assertTrue(
                (
                    products
                    / "GyroflowNiYien Final Cut.app"
                    / "Contents"
                    / "PlugIns"
                    / "GyroflowNiYienFinalCutEffect.pluginkit"
                ).is_dir()
            )

            generated_template = output / "Gyroflow NiYien Phase 0.moef"
            generation = subprocess.run(
                [
                    sys.executable,
                    str(template_generator),
                    "--output",
                    str(generated_template),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                generation.returncode,
                0,
                msg=generation.stdout + generation.stderr,
            )
            self.assertEqual(
                len(ET.parse(generated_template).findall(".//filter")),
                1,
            )


class FinalCutCAbiContractTests(unittest.TestCase):
    def test_public_header_is_valid_c11_with_fixed_pod_layout(self):
        header_directory = ROOT / "finalcut" / "include"
        helper = ROOT / "tests" / "helpers" / "finalcut_abi_header_main.c"
        result = subprocess.run(
            [
                "xcrun",
                "clang",
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
                str(header_directory),
                "-fsyntax-only",
                str(helper),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)

    def test_geometry_pod_has_a_fixed_versioned_c11_layout(self):
        header_directory = ROOT / "finalcut" / "include"
        source = r'''
#include "GyroflowFinalCut.h"
#include <stddef.h>
#include <stdint.h>

_Static_assert(sizeof(GFDimensionsU32) == 8, "dimensions must use uint32_t");
_Static_assert(sizeof(GFRectI32) == 16, "rectangles must use int32_t");
_Static_assert(sizeof(GFAffineTransform) == 72, "affines must contain nine doubles");
_Static_assert(sizeof(GFFrameGeometry) == 256, "geometry ABI size changed");
_Static_assert(_Alignof(GFFrameGeometry) == 8, "geometry ABI alignment changed");
_Static_assert(offsetof(GFFrameGeometry, source_dimensions) == 16, "source dimensions offset changed");
_Static_assert(offsetof(GFFrameGeometry, source_rect) == 48, "source rect offset changed");
_Static_assert(offsetof(GFFrameGeometry, forward_transform) == 96, "forward affine offset changed");
_Static_assert(offsetof(GFFrameGeometry, inverse_transform) == 168, "inverse affine offset changed");
_Static_assert(offsetof(GFFrameGeometry, reserved) == 240, "reserved offset changed");
_Static_assert(sizeof(((GFFrameGeometry *)0)->version) == sizeof(uint32_t), "version width changed");
_Static_assert(sizeof(((GFFrameGeometry *)0)->source_origin) == sizeof(uint32_t), "origin width changed");
_Static_assert(sizeof(((GFFrameGeometry *)0)->input_rotation) == sizeof(uint32_t), "rotation width changed");
_Static_assert(offsetof(GFMetalRenderRequest, geometry) == 136, "request geometry offset changed");
_Static_assert(sizeof(GFMetalRenderRequest) == 392, "request ABI size changed");

int main(void) {
    GFFrameGeometry geometry = {0};
    return geometry.version == GF_FRAME_GEOMETRY_VERSION_LEGACY &&
           geometry.validity == GF_GEOMETRY_VALIDITY_LEGACY_UNKNOWN &&
           geometry.support == GF_GEOMETRY_SUPPORT_UNKNOWN ? 0 : 1;
}
'''
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "geometry-layout"
            compile_result = subprocess.run(
                [
                    "xcrun",
                    "clang",
                    "-std=c11",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-I",
                    str(header_directory),
                    "-x",
                    "c",
                    "-",
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                input=source,
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                compile_result.returncode,
                0,
                msg=compile_result.stdout + compile_result.stderr,
            )
            run_result = subprocess.run(
                [str(executable)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                run_result.returncode,
                0,
                msg=run_result.stdout + run_result.stderr,
            )


class FinalCutUniversalRustBuildTests(unittest.TestCase):
    BUILDER = ROOT / "scripts" / "build_finalcut_rust.py"

    def write_fake_tools(self, directory: Path) -> tuple[Path, Path]:
        cargo = directory / "fake-cargo"
        cargo.write_text(
            """#!/usr/bin/env python3
import os
import sys
from pathlib import Path

if os.environ.get("FAKE_CARGO_FAIL") == "1":
    raise SystemExit(9)
if os.environ.get("MACOSX_DEPLOYMENT_TARGET") != "13.0":
    raise SystemExit(10)
target = sys.argv[sys.argv.index("--target") + 1]
root = Path(os.environ["CARGO_TARGET_DIR"])
library = root / target / "release" / "libgyroflow_finalcut.a"
library.parent.mkdir(parents=True, exist_ok=True)
library.write_bytes(target.encode("ascii"))
""",
            encoding="utf-8",
        )
        cargo.chmod(0o755)

        lipo = directory / "fake-lipo"
        lipo.write_text(
            """#!/usr/bin/env python3
import sys
from pathlib import Path

arguments = sys.argv[1:]
if arguments[0] == "-create":
    output = Path(arguments[arguments.index("-output") + 1])
    inputs = arguments[1:arguments.index("-output")]
    output.write_bytes(b"|".join(Path(item).read_bytes() for item in inputs))
elif arguments[0] == "-archs":
    print("arm64 x86_64")
else:
    raise SystemExit(2)
""",
            encoding="utf-8",
        )
        lipo.chmod(0o755)
        return cargo, lipo

    def test_builder_compiles_both_targets_and_atomically_creates_fat_library(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cargo, lipo = self.write_fake_tools(root)
            output = root / "build" / "libgyroflow_finalcut.a"
            result = subprocess.run(
                [
                    sys.executable,
                    str(self.BUILDER),
                    "--cargo",
                    str(cargo),
                    "--lipo",
                    str(lipo),
                    "--target-dir",
                    str(root / "target"),
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            self.assertEqual(
                output.read_bytes(),
                b"aarch64-apple-darwin|x86_64-apple-darwin",
            )
            self.assertIn("arm64 x86_64", result.stdout)

    def test_failed_architecture_build_preserves_previous_fat_library(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cargo, lipo = self.write_fake_tools(root)
            output = root / "build" / "libgyroflow_finalcut.a"
            output.parent.mkdir(parents=True)
            output.write_bytes(b"previous-good-library")
            environment = os.environ.copy()
            environment["FAKE_CARGO_FAIL"] = "1"
            result = subprocess.run(
                [
                    sys.executable,
                    str(self.BUILDER),
                    "--cargo",
                    str(cargo),
                    "--lipo",
                    str(lipo),
                    "--target-dir",
                    str(root / "target"),
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(output.read_bytes(), b"previous-good-library")

    def test_effect_target_links_the_generated_static_library(self):
        project = ROOT / "finalcut" / "Xcode" / "GyroflowFinalCut.xcodeproj"
        result = subprocess.run(
            [
                "xcodebuild",
                "-project",
                str(project),
                "-target",
                "GyroflowFinalCutEffect",
                "-showBuildSettings",
                "-json",
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        settings = json.loads(result.stdout)[0]["buildSettings"]
        self.assertIn("libgyroflow_finalcut.a", settings["OTHER_LDFLAGS"])

    @unittest.skipUnless(
        os.environ.get("FINALCUT_RUN_UNIVERSAL_BUILD_TEST") == "1",
        "set FINALCUT_RUN_UNIVERSAL_BUILD_TEST=1 on a macOS release builder",
    )
    def test_real_builder_outputs_arm64_and_x86_64(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "libgyroflow_finalcut.a"
            result = subprocess.run(
                [
                    sys.executable,
                    str(self.BUILDER),
                    "--target-dir",
                    str(root / "target"),
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            archs = subprocess.run(
                ["xcrun", "lipo", "-archs", str(output)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(archs.returncode, 0, msg=archs.stdout + archs.stderr)
            self.assertEqual(set(archs.stdout.split()), {"arm64", "x86_64"})


if __name__ == "__main__":
    unittest.main()
