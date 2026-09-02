import json
import html
import os
import plistlib
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PROBE_ROOT = ROOT / "probes" / "finalcut_phase0"
GENERATOR = PROBE_ROOT / "scripts" / "generate_template.py"
BUILDER = PROBE_ROOT / "scripts" / "build_probe.py"
VERIFIER = PROBE_ROOT / "scripts" / "verify_template.py"
ROUTE_D_PATCHER = PROBE_ROOT / "scripts" / "patch_route_d.py"
IDENTITY = PROBE_ROOT / "config" / "identity.json"
XPC_INFO = PROBE_ROOT / "xpc" / "Info.plist"
XPC_ENTITLEMENTS = PROBE_ROOT / "xpc" / "SandboxEntitlements.entitlements"
MEDIA_RESOLVER = PROBE_ROOT / "xpc" / "GFPhase0MediaResolver.m"
MEDIA_RESOLVER_HELPER = ROOT / "tests" / "helpers" / "finalcut_phase0_resolver_main.m"
EFFECT_HEADER = PROBE_ROOT / "xpc" / "GFPhase0Effect.h"
EFFECT_SOURCE = PROBE_ROOT / "xpc" / "GFPhase0Effect.m"
DROP_ZONE_HEADER = PROBE_ROOT / "xpc" / "GFPhase0DropZoneView.h"
PROJECT_STORE = PROBE_ROOT / "xpc" / "GFPhase0ProjectStore.m"
PROJECT_STORE_HELPER = ROOT / "tests" / "helpers" / "finalcut_phase0_project_store_main.m"
PARAMETER_COMMITTER = PROBE_ROOT / "xpc" / "GFPhase0ParameterCommitter.m"
PARAMETER_COMMITTER_HELPER = ROOT / "tests" / "helpers" / "finalcut_phase0_parameter_committer_main.m"
WORKFLOW_INFO = PROBE_ROOT / "workflow" / "Info.plist"
WORKFLOW_SOURCE = PROBE_ROOT / "workflow" / "GFPhase0WorkflowViewController.m"
WORKFLOW_ENTITLEMENTS = PROBE_ROOT / "workflow" / "SandboxEntitlements.entitlements"
WORKFLOW_SDK = Path("/Library/Developer/SDKs/WorkflowExtensionSDK.sdk")
WORKFLOW_HEADER = (
    WORKFLOW_SDK
    / "Library"
    / "Frameworks"
    / "ProExtensionHost.framework"
    / "Headers"
    / "FCPXHost.h"
)


class FinalCutTemplateContractTests(unittest.TestCase):
    def generate_template(self, directory: str) -> Path:
        output = Path(directory) / "Gyroflow NiYien Phase 0.moef"
        result = subprocess.run(
            [sys.executable, str(GENERATOR), "--output", str(output)],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        return output

    def test_generator_emits_owned_identity_and_parseable_template(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.generate_template(directory)
            identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
            with XPC_INFO.open("rb") as plist_file:
                xpc_info = plistlib.load(plist_file)
            tree = ET.parse(output)
            filters = tree.findall(".//filter")

            self.assertEqual(len(filters), 1)
            self.assertEqual(filters[0].attrib["pluginUUID"], identity["effect_uuid"])
            self.assertEqual(filters[0].attrib["pluginName"], identity["effect_name"])
            self.assertEqual(
                xpc_info["ProPlugPlugInList"][0]["uuid"],
                identity["effect_uuid"],
            )
            self.assertEqual(
                xpc_info["ProPlugPlugInGroupList"][0]["uuid"],
                identity["group_uuid"],
            )
            self.assertNotIn("92ADB2F9-C649-48C2-B2D4-441CFC0633CB", output.read_text(encoding="utf-8"))
            self.assertNotIn("Gyroflow Toolbox", output.read_text(encoding="utf-8"))

    def test_generated_template_uses_only_phase0_parameter_mapping(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.generate_template(directory)
            tree = ET.parse(output)
            filter_element = tree.find(".//filter")
            self.assertIsNotNone(filter_element)

            direct_parameter_ids = {
                int(parameter.attrib["id"])
                for parameter in filter_element.findall("./parameter")
            }
            published_targets = {
                (target.attrib["channel"], target.attrib["name"])
                for target in tree.findall(".//publishSettings/target")
            }

            self.assertEqual(
                direct_parameter_ids,
                {1, 1100, 1901, 1902, 1903, 10001, 10002, 10003},
            )
            self.assertEqual(published_targets, {("./1100", "Phase 0 Probe")})
            self.assertIsNotNone(filter_element.find("./parameter[@id='1100']/parameter[@id='1101']/defaultVal"))
            self.assertIsNotNone(filter_element.find("./parameter[@id='1100']/parameter[@id='1101']/dataValue"))

    def test_verifier_rejects_opaque_template_drift(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.generate_template(directory)
            accepted = subprocess.run(
                [sys.executable, str(VERIFIER), str(output), str(XPC_INFO)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(accepted.returncode, 0, msg=accepted.stdout + accepted.stderr)

            rendered = output.read_text(encoding="utf-8")
            output.write_text(rendered.replace("</defaultVal>", "X</defaultVal>", 1), encoding="utf-8")
            rejected = subprocess.run(
                [sys.executable, str(VERIFIER), str(output), str(XPC_INFO)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("opaque", rejected.stderr)


class FinalCutProbeSourceContractTests(unittest.TestCase):
    def test_effect_advertises_custom_parameter_view_protocol(self):
        header = EFFECT_HEADER.read_text(encoding="utf-8")
        self.assertIn(
            "<FxTileableEffect, FxCustomParameterViewHost_v2>",
            header,
        )
        self.assertIn(
            "createViewForParameterID:(UInt32)parameterID NS_RETURNS_RETAINED",
            header,
        )

    def test_project_picker_is_gyroflow_filtered_and_has_no_media_api(self):
        drop_zone = (PROBE_ROOT / "xpc" / "GFPhase0DropZoneView.m").read_text(encoding="utf-8")
        self.assertIn('typeWithFilenameExtension:@"gyroflow"', drop_zone)
        self.assertIn("panel.allowedContentTypes", drop_zone)
        self.assertIn("panel.treatsFilePackagesAsDirectories = YES", drop_zone)
        self.assertIn("[panel beginWithCompletionHandler:completion]", drop_zone)
        self.assertNotIn("beginSheetModalForWindow", drop_zone)
        for forbidden in ("AVAsset", "get_video_metadata", "importMedia"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, drop_zone)

    def test_hidden_identity_and_project_payload_use_fxplug_string_parameters(self):
        effect = EFFECT_SOURCE.read_text(encoding="utf-8")
        drop_zone_header = DROP_ZONE_HEADER.read_text(encoding="utf-8")
        for required in (
            "FxParameterRetrievalAPI_v6",
            "GFPhase0ParameterCommitter",
            "getStringParameterValue",
        ):
            with self.subTest(required=required):
                self.assertIn(required, effect)
        self.assertIn("GFPhase0ProjectCommitHandler", drop_zone_header)
        self.assertIn("commitHandler", drop_zone_header)

    def test_plugin_state_reads_parameters_without_mutating_host_state(self):
        effect = EFFECT_SOURCE.read_text(encoding="utf-8")
        plugin_state = effect.split("- (BOOL)pluginState:", 1)[1].split(
            "- (BOOL)destinationImageRect:", 1
        )[0]
        self.assertIn("stringValueForParameter", plugin_state)
        self.assertNotIn("ensureInstanceIdentity", plugin_state)
        self.assertNotIn("setStringParameterValue", plugin_state)

    def test_plugin_state_probes_host_timing_without_render_thread_api_calls(self):
        effect = EFFECT_SOURCE.read_text(encoding="utf-8")
        plugin_state = effect.split("- (BOOL)pluginState:", 1)[1].split(
            "- (BOOL)destinationImageRect:", 1
        )[0]
        render = effect.split("- (BOOL)renderDestinationImage:", 1)[1]

        for required in (
            "FxTimingAPI_v4",
            "inputTime:&inputTime",
            "fromTimelineTime:renderTime",
            "timelineTime:&roundTripTime",
            "fromInputTime:inputTime",
            "startTimeForEffect:&effectStartTime",
            "startTimeOfInputToFilter:&inputStartTime",
            "inPointTimeOfTimelineForEffect:&timelineInTime",
            "phase0 timing probe",
        ):
            with self.subTest(required=required):
                self.assertIn(required, plugin_state)

        for forbidden in ("FxTimingAPI_v4", "inputTime:", "timelineTime:"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, render)

    def test_timing_probe_normalizes_native_render_time_before_input_conversion(self):
        effect = EFFECT_SOURCE.read_text(encoding="utf-8")
        plugin_state = effect.split("- (BOOL)pluginState:", 1)[1].split(
            "- (BOOL)destinationImageRect:", 1
        )[0]

        for required in (
            "CMTimeSubtract(renderTime, effectStartTime)",
            "CMTimeAdd(timelineInTime, effectLocalTime)",
            "inputTime:&mappedInputTime",
            "fromTimelineTime:timelineProbeTime",
            "timelineTime:&mappedRoundTripTime",
            "fromInputTime:mappedInputTime",
            '"mappedInputTimeValue"',
            '"mappedInputTimeScale"',
        ):
            with self.subTest(required=required):
                self.assertIn(required, plugin_state)


class FinalCutParameterCommitterTests(unittest.TestCase):
    def test_project_commit_is_an_action_scoped_base64_transaction(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "finalcut_phase0_parameter_committer"
            build = subprocess.run(
                [
                    "xcrun",
                    "clang",
                    "-fobjc-arc",
                    "-fblocks",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-F/Library/Developer/SDKs/FxPlug.sdk/Library/Frameworks",
                    "-framework",
                    "Foundation",
                    "-framework",
                    "FxPlug",
                    "-framework",
                    "PluginManager",
                    "-I",
                    str(PROBE_ROOT / "xpc"),
                    str(PARAMETER_COMMITTER),
                    str(PARAMETER_COMMITTER_HELPER),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)

            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            run = subprocess.run(
                [str(executable)],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, msg=run.stdout + run.stderr)
            result = json.loads(run.stdout)

            self.assertTrue(result["first"])
            self.assertTrue(result["second"])
            self.assertEqual(result["startCount"], 2)
            self.assertEqual(result["endCount"], 2)
            self.assertEqual(result["outsideActionWrites"], 0)
            self.assertEqual(result["identityLength"], 36)
            self.assertTrue(result["sameIdentity"])
            self.assertEqual(result["projectPayload"], "c2Vjb25k")
            self.assertIn("payload_base64=8", result["evidence"])
            self.assertEqual(
                result["events"],
                [
                    "start",
                    "set-1901",
                    "set-1902",
                    "set-1103",
                    "end",
                    "start",
                    "set-1902",
                    "set-1103",
                    "end",
                ],
            )


class FinalCutRouteDPatcherTests(unittest.TestCase):
    def test_patch_creates_new_project_identity_and_only_changes_timing_params(self):
        fixture = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.14">
  <resources>
    <format id="r1" frameDuration="100/3000s"/>
    <asset id="r2" start="3600s" duration="30s"/>
    <effect id="r8" name="Gyroflow NiYien Phase 0" uid="niyien-effect"/>
    <effect id="r9" name="Magnetic Mask" uid="other-effect"/>
  </resources>
  <project name="Original" uid="11111111-1111-4111-8111-111111111111">
    <sequence format="r1" duration="30s">
      <spine>
        <asset-clip ref="r2" offset="0s" start="3600s" duration="30s">
          <object-tracker><tracking-shape id="tr1"/></object-tracker>
          <filter-video ref="r8" name="Gyroflow NiYien Phase 0">
            <param name="Instance Identity" key="instance" value="first-instance"/>
            <param name="Project Payload" key="project" value="first-project"/>
            <param name="Timing Payload" key="timing" value=""/>
          </filter-video>
          <filter-video ref="r8" name="Gyroflow NiYien Phase 0">
            <param name="Instance Identity" key="instance" value="second-instance"/>
            <param name="Project Payload" key="project" value="second-project"/>
            <param name="Timing Payload" key="timing" value="old-timing"/>
          </filter-video>
          <filter-video ref="r9" name="Magnetic Mask">
            <param name="Mask Data" key="mask" value="opaque-mask"/>
          </filter-video>
        </asset-clip>
      </spine>
    </sequence>
  </project>
</fcpxml>"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.fcpxml"
            output = root / "patched.fcpxml"
            source.write_text(fixture, encoding="utf-8")
            source_before = source.read_bytes()

            result = subprocess.run(
                [
                    sys.executable,
                    str(ROUTE_D_PATCHER),
                    "--input",
                    str(source),
                    "--output",
                    str(output),
                    "--target-project-name",
                    "Original - Route D Patched",
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            self.assertEqual(source.read_bytes(), source_before)
            self.assertTrue(output.read_bytes().startswith(b'<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE fcpxml>\n'))

            summary = json.loads(result.stdout)
            tree = ET.parse(output)
            project = tree.find("./project")
            self.assertIsNotNone(project)
            self.assertEqual(project.attrib["name"], "Original - Route D Patched")
            self.assertNotEqual(project.attrib["uid"], "11111111-1111-4111-8111-111111111111")
            self.assertEqual(summary["sourceProjectUID"], "11111111-1111-4111-8111-111111111111")
            self.assertEqual(summary["targetProjectUID"], project.attrib["uid"])
            self.assertEqual(summary["filtersPatched"], 2)

            asset_clip = tree.find(".//asset-clip")
            self.assertEqual(
                asset_clip.attrib,
                {"ref": "r2", "offset": "0s", "start": "3600s", "duration": "30s"},
            )
            self.assertIsNotNone(asset_clip.find("./object-tracker/tracking-shape[@id='tr1']"))
            self.assertEqual(
                asset_clip.find("./filter-video[@name='Magnetic Mask']/param").attrib,
                {"name": "Mask Data", "key": "mask", "value": "opaque-mask"},
            )

            niyien_filters = asset_clip.findall("./filter-video[@name='Gyroflow NiYien Phase 0']")
            self.assertEqual(len(niyien_filters), 2)
            self.assertEqual(
                [item.find("./param[@name='Instance Identity']").attrib["value"] for item in niyien_filters],
                ["first-instance", "second-instance"],
            )
            self.assertEqual(
                [item.find("./param[@name='Project Payload']").attrib["value"] for item in niyien_filters],
                ["first-project", "second-project"],
            )
            timing_values = [
                item.find("./param[@name='Timing Payload']").attrib["value"]
                for item in niyien_filters
            ]
            self.assertEqual(len(set(timing_values)), 2)
            self.assertIn("occurrence=0001", timing_values[0])
            self.assertIn("occurrence=0002", timing_values[1])
            for value in timing_values:
                self.assertIn(f"project={project.attrib['uid']}", value)
                self.assertIn("assetStart=3600s", value)
                self.assertIn("clipStart=3600s", value)
                self.assertIn("offset=0s", value)


class FinalCutWorkflowExtensionContractTests(unittest.TestCase):
    def test_workflow_extension_declares_official_extension_point(self):
        with WORKFLOW_INFO.open("rb") as plist_file:
            info = plistlib.load(plist_file)
        extension = info["NSExtension"]
        self.assertEqual(
            extension["NSExtensionPointIdentifier"],
            "com.apple.FinalCut.WorkflowExtension",
        )
        self.assertEqual(
            extension["ProExtensionPrincipalViewControllerClass"],
            "GFPhase0WorkflowViewController",
        )
        self.assertEqual(info["CFBundleIdentifier"], "com.niyien.gyroflow.finalcut.workflow")

    def test_workflow_probe_observes_only_public_timeline_proxy_and_accepts_fcpxml(self):
        source = WORKFLOW_SOURCE.read_text(encoding="utf-8")
        for required in (
            "ProExtensionHostSingleton",
            "addTimelineObserver",
            "activeSequence",
            "playheadTime",
            "sequenceTimeRange",
            "com.apple.finalcutpro.xml.v1-%ld",
            "version = 14",
            "NSXMLDocument",
            "last-drop.fcpxml",
            "writeToURL",
        ):
            with self.subTest(required=required):
                self.assertIn(required, source)
        for forbidden in ("AVAsset", "importMedia", "mediaBytes", "enumerateClips"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, source)

    def test_workflow_entitlements_scope_library_validation_exception_to_appex(self):
        with WORKFLOW_ENTITLEMENTS.open("rb") as plist_file:
            entitlements = plistlib.load(plist_file)
        self.assertEqual(
            entitlements,
            {
                "com.apple.security.app-sandbox": True,
                "com.apple.security.cs.disable-library-validation": True,
            },
        )
        with XPC_ENTITLEMENTS.open("rb") as plist_file:
            xpc_entitlements = plistlib.load(plist_file)
        self.assertNotIn(
            "com.apple.security.cs.disable-library-validation",
            xpc_entitlements,
        )

    @unittest.skipUnless(WORKFLOW_HEADER.is_file(), "Workflow Extension SDK is required")
    def test_installed_sdk_has_sequence_playhead_but_no_clip_enumeration(self):
        header = WORKFLOW_HEADER.read_text(encoding="utf-8")
        for public_api in (
            "activeSequence",
            "playheadTime",
            "sequenceTimeRange",
            "addTimelineObserver",
        ):
            with self.subTest(public_api=public_api):
                self.assertIn(public_api, header)
        for absent_api in ("clip", "assetClip", "selectedClip"):
            with self.subTest(absent_api=absent_api):
                self.assertNotIn(absent_api, header)


@unittest.skipUnless(sys.platform == "darwin", "Final Cut drop resolver requires Foundation")
class FinalCutDropResolverTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._temporary_directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls._temporary_directory.cleanup)
        cls.resolver = Path(cls._temporary_directory.name) / "finalcut_phase0_resolver"
        result = subprocess.run(
            [
                "xcrun",
                "clang",
                "-fobjc-arc",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-framework",
                "Foundation",
                "-I",
                str(PROBE_ROOT / "xpc"),
                str(MEDIA_RESOLVER),
                str(MEDIA_RESOLVER_HELPER),
                "-o",
                str(cls.resolver),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise AssertionError(result.stdout + result.stderr)

    def run_resolver(self, mode: str, *inputs: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.resolver), mode, *(str(path) for path in inputs)],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def write_fcpxml(self, directory: Path, assets: str, clips: str) -> Path:
        fcpxml = directory / "drop.fcpxml"
        fcpxml.write_text(
            f'<fcpxml version="1.11"><resources>{assets}</resources>'
            f'<library><event>{clips}</event></library></fcpxml>',
            encoding="utf-8",
        )
        return fcpxml

    def asset(self, asset_id: str, *representations: tuple[str, str]) -> str:
        media_representations = "".join(
            f'<media-rep kind="{html.escape(kind)}" src="{html.escape(url)}"/>'
            for kind, url in representations
        )
        return f'<asset id="{html.escape(asset_id)}">{media_representations}</asset>'

    def test_finder_requires_exactly_one_file_and_derives_only_sibling_project(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            media = root / "Camera 01.mov"
            media.touch()
            project = root / "Camera 01.gyroflow"
            project.touch()

            accepted = self.run_resolver("finder", media)
            self.assertEqual(accepted.returncode, 0, msg=accepted.stdout + accepted.stderr)
            resolution = json.loads(accepted.stdout)
            self.assertEqual(resolution["source"], "finder")
            self.assertEqual(resolution["mediaURL"], media.as_uri())
            self.assertEqual(resolution["projectURL"], project.as_uri())
            self.assertTrue(resolution["projectExists"])

            rejected = self.run_resolver("finder", media, root / "Camera 02.mov")
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("exactly one Finder file", rejected.stderr)

    def test_fcpxml_accepts_one_clip_asset_and_unique_original_media(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            media = root / "Original Clip.mov"
            project = root / "Original Clip.gyroflow"
            project.touch()
            asset = self.asset(
                "r2",
                ("original-media", media.as_uri()),
                ("proxy-media", (root / "Proxy Clip.mov").as_uri()),
            )
            fcpxml = self.write_fcpxml(root, asset, '<asset-clip ref="r2"/>')

            accepted = self.run_resolver("fcpxml", fcpxml)
            self.assertEqual(accepted.returncode, 0, msg=accepted.stdout + accepted.stderr)
            resolution = json.loads(accepted.stdout)
            self.assertEqual(resolution["source"], "fcpxml")
            self.assertEqual(resolution["mediaURL"], media.as_uri())
            self.assertEqual(resolution["projectURL"], project.as_uri())
            self.assertTrue(resolution["projectExists"])

    def test_fcpxml_rejects_multiple_clips_or_assets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = (root / "Original.mov").as_uri()
            one_asset = self.asset("r2", ("original-media", original))
            two_clips = self.write_fcpxml(
                root,
                one_asset,
                '<asset-clip ref="r2"/><asset-clip ref="r2"/>',
            )
            rejected_clips = self.run_resolver("fcpxml", two_clips)
            self.assertNotEqual(rejected_clips.returncode, 0)
            self.assertIn("exactly one clip reference", rejected_clips.stderr)

            two_assets = self.write_fcpxml(
                root,
                one_asset + self.asset("r3", ("original-media", (root / "Other.mov").as_uri())),
                '<asset-clip ref="r2"/>',
            )
            rejected_assets = self.run_resolver("fcpxml", two_assets)
            self.assertNotEqual(rejected_assets.returncode, 0)
            self.assertIn("exactly one asset", rejected_assets.stderr)

    def test_fcpxml_rejects_proxy_optimized_ambiguous_and_remote_originals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cases = [
                self.asset("r2", ("proxy-media", (root / "Proxy.mov").as_uri())),
                self.asset("r2", ("optimized-media", (root / "Optimized.mov").as_uri())),
                self.asset(
                    "r2",
                    ("original-media", (root / "One.mov").as_uri()),
                    ("original-media", (root / "Two.mov").as_uri()),
                ),
            ]
            for asset in cases:
                with self.subTest(asset=asset):
                    fcpxml = self.write_fcpxml(root, asset, '<asset-clip ref="r2"/>')
                    rejected = self.run_resolver("fcpxml", fcpxml)
                    self.assertNotEqual(rejected.returncode, 0)
                    self.assertIn("exactly one original-media", rejected.stderr)

            remote_asset = self.asset("r2", ("original-media", "https://example.com/Original.mov"))
            remote_fcpxml = self.write_fcpxml(root, remote_asset, '<asset-clip ref="r2"/>')
            rejected_remote = self.run_resolver("fcpxml", remote_fcpxml)
            self.assertNotEqual(rejected_remote.returncode, 0)
            self.assertIn("local file URL", rejected_remote.stderr)

    def test_resolver_source_has_no_media_content_reading_api(self):
        source = MEDIA_RESOLVER.read_text(encoding="utf-8")
        forbidden_media_readers = (
            "AVAsset",
            "NSFileHandle",
            "readDataToEndOfFile",
            "dataWithContentsOfURL",
        )
        for forbidden in forbidden_media_readers:
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, source)


@unittest.skipUnless(sys.platform == "darwin", "Final Cut project store requires Foundation")
class FinalCutProjectStoreTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._temporary_directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls._temporary_directory.cleanup)
        cls.store = Path(cls._temporary_directory.name) / "finalcut_phase0_project_store"
        result = subprocess.run(
            [
                "xcrun",
                "clang",
                "-fobjc-arc",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-framework",
                "Foundation",
                "-I",
                str(PROBE_ROOT / "xpc"),
                str(PROJECT_STORE),
                str(PROJECT_STORE_HELPER),
                "-o",
                str(cls.store),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise AssertionError(result.stdout + result.stderr)

    def run_store(self, *arguments: str) -> dict:
        result = subprocess.run(
            [str(self.store), *arguments],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        return json.loads(result.stdout)

    def test_valid_project_is_committed_only_after_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            valid = root / "valid.gyroflow"
            valid.write_text(
                json.dumps({"version": 3, "videofile": "", "gyro_source": {}}),
                encoding="utf-8",
            )

            result = self.run_store(str(valid))
            self.assertEqual(result["currentBytes"], valid.stat().st_size)
            self.assertEqual(result["currentVersion"], 3)
            self.assertEqual(result["errors"], [])
            self.assertIn("Loaded valid.gyroflow", result["status"])

    def test_corrupt_wrong_extension_and_cancel_preserve_valid_project(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            valid = root / "valid.gyroflow"
            valid.write_text(
                json.dumps({"version": 3, "videofile": "", "gyro_source": {}}),
                encoding="utf-8",
            )
            corrupt = root / "corrupt.gyroflow"
            corrupt.write_text("{not-json", encoding="utf-8")
            video = root / "not-a-project.mp4"
            video.write_bytes(b"must-not-be-read-as-a-project")

            corrupt_result = self.run_store(str(valid), str(corrupt))
            self.assertEqual(corrupt_result["currentBytes"], valid.stat().st_size)
            self.assertEqual(corrupt_result["currentVersion"], 3)
            self.assertIn("invalid JSON", corrupt_result["errors"][0])
            self.assertIn("kept previous project", corrupt_result["status"])

            extension_result = self.run_store(str(valid), str(video))
            self.assertEqual(extension_result["currentBytes"], valid.stat().st_size)
            self.assertIn("only .gyroflow", extension_result["errors"][0])

            cancelled_result = self.run_store(str(valid), "--cancel")
            self.assertEqual(cancelled_result["currentBytes"], valid.stat().st_size)
            self.assertEqual(cancelled_result["currentVersion"], 3)
            self.assertIn("cancelled; kept previous project", cancelled_result["status"])


@unittest.skipUnless(sys.platform == "darwin", "FxPlug probe build requires macOS")
class FinalCutProbeBuildTests(unittest.TestCase):
    def test_build_script_produces_signed_universal_probe_app(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "GyroflowNiYien Final Cut Phase 0.app"
            result = subprocess.run(
                [
                    sys.executable,
                    str(BUILDER),
                    "--output",
                    str(app),
                    "--framework-root",
                    "/Applications/Final Cut Pro Creator Studio.app/Contents/Frameworks",
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)

            xpc = app / "Contents" / "PlugIns" / "GyroflowNiYienPhase0Renderer.pluginkit"
            xpc_executable = xpc / "Contents" / "MacOS" / "GyroflowNiYienPhase0Renderer"
            workflow = app / "Contents" / "PlugIns" / "GyroflowNiYienPhase0Workflow.appex"
            workflow_executable = workflow / "Contents" / "MacOS" / "GyroflowNiYienPhase0Workflow"
            template = (
                app
                / "Contents"
                / "Resources"
                / "Motion Templates"
                / "Effects.localized"
                / "NiYien"
                / "Gyroflow"
                / "Gyroflow NiYien Phase 0.moef"
            )

            self.assertTrue(xpc_executable.is_file())
            self.assertTrue(workflow_executable.is_file())
            self.assertTrue(template.is_file())
            self.assertTrue(template.with_name("large.png").is_file())
            self.assertTrue(template.with_name("small.png").is_file())

            architectures = subprocess.run(
                ["lipo", "-archs", str(xpc_executable)],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.split()
            self.assertEqual(set(architectures), {"arm64", "x86_64"})

            workflow_architectures = subprocess.run(
                ["lipo", "-archs", str(workflow_executable)],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.split()
            self.assertEqual(set(workflow_architectures), {"arm64", "x86_64"})

            with (workflow / "Contents" / "Info.plist").open("rb") as plist_file:
                workflow_info = plistlib.load(plist_file)
            self.assertEqual(
                workflow_info["NSExtension"]["NSExtensionPointIdentifier"],
                "com.apple.FinalCut.WorkflowExtension",
            )

            verification = subprocess.run(
                ["codesign", "--verify", "--strict", str(app)],
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                verification.returncode,
                0,
                msg=verification.stdout + verification.stderr,
            )

            xpc_signature = subprocess.run(
                ["codesign", "-d", "--verbose=4", str(xpc)],
                capture_output=True,
                text=True,
            )
            xpc_signature_details = xpc_signature.stdout + xpc_signature.stderr
            self.assertEqual(
                xpc_signature.returncode,
                0,
                msg=xpc_signature_details,
            )
            self.assertIn("Identifier=com.niyien.gyroflow.finalcut.effect", xpc_signature_details)
            self.assertIn("runtime", xpc_signature_details)
            self.assertIn("adhoc", xpc_signature_details)

            embedded_fxplug = xpc / "Contents" / "Frameworks" / "FxPlug.framework"
            fxplug_signature = subprocess.run(
                ["codesign", "-d", "--verbose=4", str(embedded_fxplug)],
                capture_output=True,
                text=True,
            )
            signature_details = fxplug_signature.stdout + fxplug_signature.stderr
            self.assertEqual(fxplug_signature.returncode, 0, msg=signature_details)
            self.assertIn("TeamIdentifier=PTN9T2S29T", signature_details)
            self.assertNotIn("Signature=adhoc", signature_details)

    def test_build_script_supports_explicit_processing_color_variants(self):
        with tempfile.TemporaryDirectory() as directory:
            for color_info in ("linear", "gamma"):
                with self.subTest(color_info=color_info):
                    app = Path(directory) / f"GyroflowNiYien-{color_info}.app"
                    result = subprocess.run(
                        [
                            sys.executable,
                            str(BUILDER),
                            "--output",
                            str(app),
                            "--framework-root",
                            "/Applications/Final Cut Pro Creator Studio.app/Contents/Frameworks",
                            "--processing-color-info",
                            color_info,
                        ],
                        cwd=ROOT,
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
                    executable = (
                        app
                        / "Contents"
                        / "PlugIns"
                        / "GyroflowNiYienPhase0Renderer.pluginkit"
                        / "Contents"
                        / "MacOS"
                        / "GyroflowNiYienPhase0Renderer"
                    )
                    strings = subprocess.run(
                        ["strings", str(executable)],
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(strings.returncode, 0, msg=strings.stderr)
                    self.assertIn("DesiredProcessingColorInfo", strings.stdout)

    @unittest.skipUnless(
        os.environ.get("FINALCUT_PROBE_SIGNING_IDENTITY"),
        "Developer ID integration test requires FINALCUT_PROBE_SIGNING_IDENTITY",
    )
    def test_build_script_signs_every_runtime_component_with_explicit_identity(self):
        signing_identity = os.environ["FINALCUT_PROBE_SIGNING_IDENTITY"]
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "GyroflowNiYien Final Cut Phase 0.app"
            result = subprocess.run(
                [
                    sys.executable,
                    str(BUILDER),
                    "--output",
                    str(app),
                    "--framework-root",
                    "/Applications/Final Cut Pro Creator Studio.app/Contents/Frameworks",
                    "--signing-identity",
                    signing_identity,
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)

            xpc = app / "Contents" / "PlugIns" / "GyroflowNiYienPhase0Renderer.pluginkit"
            signed_components = [
                xpc / "Contents" / "Frameworks" / "FxPlug.framework",
                xpc / "Contents" / "Frameworks" / "PluginManager.framework",
                xpc,
                app / "Contents" / "PlugIns" / "GyroflowNiYienPhase0Workflow.appex",
                app,
            ]
            for component in signed_components:
                signature = subprocess.run(
                    ["codesign", "-d", "--verbose=4", str(component)],
                    capture_output=True,
                    text=True,
                )
                details = signature.stdout + signature.stderr
                self.assertEqual(signature.returncode, 0, msg=details)
                self.assertIn("TeamIdentifier=H59FJRN2AM", details)
                self.assertNotIn("Signature=adhoc", details)

            verification = subprocess.run(
                ["codesign", "--verify", "--deep", "--strict", str(app)],
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                verification.returncode,
                0,
                msg=verification.stdout + verification.stderr,
            )


if __name__ == "__main__":
    unittest.main()
