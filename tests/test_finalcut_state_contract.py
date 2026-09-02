import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EFFECT = ROOT / "finalcut" / "xcode" / "Effect"
PUBLIC_INCLUDE = ROOT / "finalcut" / "include"
HELPERS = ROOT / "tests" / "helpers"
FXPLUG_FRAMEWORKS = "/Library/Developer/SDKs/FxPlug.sdk/Library/Frameworks"


def compile_helper(
    directory: Path,
    name: str,
    sources: list[Path],
    *,
    fxplug: bool = False,
    blocks: bool = False,
) -> Path:
    executable = directory / name
    command = [
        "xcrun",
        "clang",
        "-fobjc-arc",
        "-fmodules",
        f"-fmodules-cache-path={directory / 'module-cache'}",
        "-Wall",
        "-Wextra",
        "-Werror",
    ]
    if blocks:
        command.append("-fblocks")
    if fxplug:
        command.extend(
            [
                "-F",
                FXPLUG_FRAMEWORKS,
                "-framework",
                "FxPlug",
                "-framework",
                "PluginManager",
            ]
        )
    command.extend(
        [
            "-framework",
            "Foundation",
            "-I",
            str(EFFECT),
            "-I",
            str(PUBLIC_INCLUDE),
            *(str(source) for source in sources),
            "-o",
            str(executable),
        ]
    )
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise AssertionError(result.stdout + result.stderr)
    return executable


class FinalCutSourceResolverTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.executable = compile_helper(
            self.root,
            "source-resolver",
            [
                EFFECT / "GFSourceResolver.m",
                HELPERS / "finalcut_source_resolver_main.m",
            ],
        )

    def tearDown(self):
        self.temporary.cleanup()

    def run_resolver(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.executable), *arguments],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def test_finder_uses_only_exact_sibling_basename(self):
        media = self.root / "A001.camera.MP4"
        media.write_bytes(b"media bytes must never be read")
        project = self.root / "A001.camera.gyroflow"
        project.write_text("{}", encoding="utf-8")

        result = self.run_resolver("finder", str(media))

        self.assertEqual(result.returncode, 0, msg=result.stderr)
        resolved = json.loads(result.stdout)
        self.assertEqual(Path(resolved["projectURL"].removeprefix("file://")), project)
        self.assertTrue(resolved["projectExists"])

    def test_multiple_finder_items_are_rejected(self):
        result = self.run_resolver(
            "finder",
            str(self.root / "one.mov"),
            str(self.root / "two.mov"),
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly one", result.stderr)

    def test_fcpxml_requires_one_clip_asset_and_original_media(self):
        project = self.root / "B002.gyroflow"
        project.write_text("{}", encoding="utf-8")
        valid = self.root / "valid.fcpxml"
        valid.write_text(
            f"""<?xml version="1.0"?>
<fcpxml><resources><asset id="r1">
<media-rep kind="proxy-media" src="file:///proxy/B002.mov"/>
<media-rep kind="original-media" src="{(self.root / 'B002.mov').as_uri()}"/>
</asset></resources><library><event><project><sequence><spine>
<asset-clip ref="r1"/>
</spine></sequence></project></event></library></fcpxml>""",
            encoding="utf-8",
        )

        result = self.run_resolver("fcpxml", str(valid))

        self.assertEqual(result.returncode, 0, msg=result.stderr)
        self.assertEqual(
            Path(json.loads(result.stdout)["projectURL"].removeprefix("file://")),
            project,
        )

        ambiguous = self.root / "ambiguous.fcpxml"
        ambiguous.write_text(
            valid.read_text(encoding="utf-8").replace(
                "</asset>",
                f'<media-rep kind="original-media" src="{(self.root / "other.mov").as_uri()}"/>'
                "</asset>",
            ),
            encoding="utf-8",
        )
        rejected = self.run_resolver("fcpxml", str(ambiguous))
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("exactly one original-media", rejected.stderr)


class FinalCutProjectStoreTests(unittest.TestCase):
    def test_validated_render_state_restores_an_empty_inspector_only_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-render-restore",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )

            result = subprocess.run(
                [str(executable), "--restore-validated-render-state"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["renderRestoreSucceeded"])
            self.assertFalse(state["renderRestoreReplaced"])
            self.assertEqual(state["currentPayload"], "persisted-payload")
            self.assertEqual(
                state["status"],
                "Direct mode ready (untrimmed forward 1×). Complex edits: use Process Current Final Cut Project.",
            )

    def test_plugin_state_host_rejection_rolls_back_pending_import(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-host-reconcile",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )
            previous = root / "previous.gyroflow"
            previous.write_text("previous-project", encoding="utf-8")
            replacement = root / "replacement.gyroflow"
            replacement.write_text("replacement-project", encoding="utf-8")

            result = subprocess.run(
                [str(executable), "--host-reject", str(previous), str(replacement)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["hostReconciled"])
            self.assertEqual(state["currentBytes"], len(b"previous-project"))
            self.assertEqual(state["currentPayload"], "payload-16")
            self.assertIn("Final Cut did not persist", state["status"])

    def test_document_restore_without_timing_enters_direct_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-document-restore",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )

            result = subprocess.run(
                [str(executable), "--restore-no-timing"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["restoreSucceeded"])
            self.assertEqual(state["currentPayload"], "persisted-payload")
            self.assertEqual(
                state["status"],
                "Direct mode ready (untrimmed forward 1×). Complex edits: use Process Current Final Cut Project.",
            )

    def test_invalid_import_and_cancel_preserve_the_previous_project(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )
            valid = root / "valid.gyroflow"
            valid.write_text("valid-project", encoding="utf-8")
            invalid = root / "invalid.gyroflow"
            invalid.write_text("invalid-project", encoding="utf-8")
            video = root / "replacement.mov"
            video.write_text("not a project", encoding="utf-8")

            result = subprocess.run(
                [str(executable), str(valid), str(invalid), str(video), "--cancel"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertEqual(state["currentBytes"], len(b"valid-project"))
            self.assertEqual(state["currentPayload"], "payload-13")
            self.assertEqual(len(state["errors"]), 2)
            self.assertIn("kept previous project", state["status"])

    def test_action_commit_failure_preserves_the_previous_project(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-commit-failure",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )
            previous = root / "previous.gyroflow"
            previous.write_text("previous-project", encoding="utf-8")
            replacement = root / "replacement.gyroflow"
            replacement.write_text("replacement-project-is-longer", encoding="utf-8")

            result = subprocess.run(
                [str(executable), str(previous), "--reject-commit", str(replacement)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertEqual(state["currentBytes"], len(b"previous-project"))
            self.assertEqual(state["currentPayload"], "payload-16")
            self.assertIn("did not persist", state["errors"][0])
            self.assertIn("kept previous project", state["status"])


class FinalCutParameterCommitterTests(unittest.TestCase):
    def test_payload_write_occurs_only_inside_one_custom_parameter_action(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "parameter-committer",
                [
                    EFFECT / "GFParameterCommitter.m",
                    HELPERS / "finalcut_parameter_committer_main.m",
                ],
                fxplug=True,
            )
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            result = subprocess.run(
                [str(executable)],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["committed"])
            self.assertEqual(state["startCount"], 1)
            self.assertEqual(state["endCount"], 1)
            self.assertEqual(state["outsideActionWrites"], 0)
            self.assertEqual(
                state["payload"], "dmVyc2lvbmVkLWJhc2U2NC1lbnZlbG9wZQ=="
            )
            self.assertEqual(state["recoveredPayload"], state["payload"])
            self.assertEqual(state["lastWriteID"], 1904)
            self.assertEqual(state["writeCount"], 11)
            self.assertEqual(state["legacyWriteCount"], 0)
            self.assertEqual(state["nonEmptyChunksA"], 1)
            self.assertEqual(state["manifestA"]["version"], 1)
            self.assertEqual(state["manifestA"]["generation"], 1)
            self.assertEqual(state["manifestA"]["chunk_count"], 1)
            self.assertEqual(
                state["manifestA"]["encoded_length"], len(state["payload"])
            )
            self.assertEqual(len(state["manifestA"]["payload_sha256"]), 64)

    def test_parameter_apis_are_requested_only_after_the_action_starts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "parameter-committer-action-gated-apis",
                [
                    EFFECT / "GFParameterCommitter.m",
                    HELPERS / "finalcut_parameter_committer_main.m",
                ],
                fxplug=True,
            )
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            result = subprocess.run(
                [str(executable), "--action-gated-apis"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["committed"])
            self.assertEqual(state["startCount"], 1)
            self.assertEqual(state["endCount"], 1)
            self.assertEqual(state["insideActionReads"], 13)
            self.assertEqual(state["outsideActionWrites"], 0)

    def test_silent_host_rejection_fails_when_exact_readback_did_not_change(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "parameter-committer-readback",
                [
                    EFFECT / "GFParameterCommitter.m",
                    HELPERS / "finalcut_parameter_committer_main.m",
                ],
                fxplug=True,
            )
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            result = subprocess.run(
                [str(executable), "--discard-write"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 1, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertFalse(state["committed"])
            self.assertEqual(
                state["payload"], "dmVyc2lvbmVkLWJhc2U2NC1lbnZlbG9wZQ=="
            )
            self.assertEqual(state["recoveredPayload"], "previous-payload")
            self.assertGreaterEqual(state["readCount"], 11)
            self.assertEqual(state["insideActionReads"], 13)

    def test_banks_rotate_and_corrupt_newest_falls_back_to_previous_generation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "parameter-committer-cycle",
                [
                    EFFECT / "GFParameterCommitter.m",
                    HELPERS / "finalcut_parameter_committer_main.m",
                ],
                fxplug=True,
            )
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            result = subprocess.run(
                [str(executable), "--cycle"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertEqual(state["manifestWriteIDs"], [1904, 1905, 1904])
            self.assertEqual(state["generationA"], 3)
            self.assertEqual(state["generationB"], 2)
            self.assertTrue(state["latestRecovered"])
            self.assertTrue(state["fallbackRecovered"])
            self.assertEqual(state["legacyWriteCount"], 0)

    def test_equal_generation_with_different_payloads_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "parameter-committer-conflict",
                [
                    EFFECT / "GFParameterCommitter.m",
                    HELPERS / "finalcut_parameter_committer_main.m",
                ],
                fxplug=True,
            )
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            result = subprocess.run(
                [str(executable), "--conflict"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertFalse(state["recovered"])
            self.assertIn("same generation", state["error"])

    def test_four_mebibyte_limit_rejects_before_starting_an_action(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "parameter-committer-capacity",
                [
                    EFFECT / "GFParameterCommitter.m",
                    HELPERS / "finalcut_parameter_committer_main.m",
                ],
                fxplug=True,
            )
            environment = os.environ.copy()
            environment["DYLD_FRAMEWORK_PATH"] = "/Library/Developer/Frameworks"
            result = subprocess.run(
                [str(executable), "--capacity"],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["maximumCommitted"])
            self.assertFalse(state["oversizedCommitted"])
            self.assertEqual(state["startCountAfterMaximum"], 1)
            self.assertEqual(state["startCountAfterOversized"], 1)
            self.assertEqual(state["maximumUsedChunks"], 10)


class FinalCutImmutableRenderStateTests(unittest.TestCase):
    def test_live_geometry_is_render_local_and_never_archived_or_cached_as_output(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        state_header = (EFFECT / "GFRenderState.h").read_text(encoding="utf-8")
        state_source = (EFFECT / "GFRenderState.m").read_text(encoding="utf-8")
        cache_header = (EFFECT / "GFRenderCache.h").read_text(encoding="utf-8")
        cache_source = (EFFECT / "GFRenderCache.m").read_text(encoding="utf-8")
        plugin_state = effect.split("- (BOOL)pluginState:", 1)[1].split(
            "- (BOOL)destinationImageRect:", 1
        )[0]
        render = effect.split("- (BOOL)renderDestinationImage:", 1)[1]

        self.assertIn("GFFrameGeometryBuild(", render)
        self.assertNotIn("GFFrameGeometry", plugin_state)
        self.assertNotIn("geometry", (state_header + state_source).lower())
        self.assertNotIn("GFFrameGeometry", cache_header + cache_source)
        self.assertNotIn("output_texture", cache_header + cache_source)

    def test_secure_state_round_trips_and_cache_rebuilds_concurrently(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "render-state",
                [
                    EFFECT / "GFRenderState.m",
                    EFFECT / "GFRenderCache.m",
                    HELPERS / "finalcut_render_state_main.m",
                ],
            )
            result = subprocess.run(
                [str(executable)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["secureRoundTrip"])
            self.assertTrue(state["sameSnapshot"])
            self.assertTrue(state["directSnapshotReady"])
            self.assertTrue(state["directSkippedTimingLoader"])
            self.assertTrue(state["invalidTimingRejected"])
            self.assertEqual(state["concurrentFailures"], 0)
            self.assertGreaterEqual(state["createCount"], 2)
            self.assertTrue(state["corruptRejected"])

    def test_plugin_state_is_read_only_and_render_does_not_query_host_parameters(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        plugin_state = effect.split("- (BOOL)pluginState:", 1)[1].split(
            "- (BOOL)destinationImageRect:", 1
        )[0]
        render = effect.split("- (BOOL)renderDestinationImage:", 1)[1]

        self.assertIn("NSKeyedArchiver", plugin_state)
        self.assertIn("requiringSecureCoding:YES", plugin_state)
        self.assertIn("getFloatValue", plugin_state)
        self.assertIn("FxTimingAPI_v4", plugin_state)
        for forbidden in (
            "setStringParameterValue",
            "FxCustomParameterActionAPI",
            "startAction:",
            "endAction:",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, plugin_state)
        self.assertNotIn("parameterRetrievalAPI", render)
        self.assertNotIn("apiForProtocol", render)

    def test_project_view_has_manual_import_and_single_item_drop_contract(self):
        view = (EFFECT / "GFProjectDropView.m").read_text(encoding="utf-8")

        self.assertIn('self.importButton.title = @"Import Gyroflow Project"', view)
        self.assertIn("panel.allowedContentTypes = @[gyroflowType]", view)
        self.assertIn("panel.allowsMultipleSelection = NO", view)
        self.assertIn("urls.count == 1", view)
        self.assertNotIn("AVAsset", view)
        self.assertIn("NSMakeRect(0, 0, 280, 104)", view)
        self.assertIn("NSMakeRect(8, 68, 264, 28)", view)
        self.assertIn("NSMakeRect(8, 7, 264, 56)", view)
        self.assertIn("maximumNumberOfLines = 3", view)

    def test_empty_timing_commit_enters_direct_mode(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        create_view = effect.split("- (NSView *)createViewForParameterID:", 1)[1].split(
            "- (NSSet<Class> *)classesForCustomParameterID:", 1
        )[0]

        self.assertIn("recordDirectModeReady", create_view)
        self.assertNotIn("recordReprocessRequired", create_view)

    def test_project_view_manual_import_is_accessible_from_view_service(self):
        view = (EFFECT / "GFProjectDropView.m").read_text(encoding="utf-8")
        button = view.split("@implementation GFImportButton", 1)[1].split(
            "@end", 1
        )[0]

        self.assertIn("@interface GFImportButton : NSButton", view)
        self.assertIn("- (BOOL)accessibilityPerformPress", view)
        self.assertIn("- (BOOL)acceptsFirstMouse:", button)
        self.assertIn("[NSApp sendAction:self.action to:self.target from:self]", view)
        self.assertIn("[[GFImportButton alloc] initWithFrame:NSZeroRect]", view)
        self.assertIn("NSApplicationActivationPolicy previousActivationPolicy", view)
        self.assertIn(
            "[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory]",
            view,
        )
        self.assertIn("[NSApp activateIgnoringOtherApps:YES]", view)
        self.assertIn("[NSApp setActivationPolicy:previousActivationPolicy]", view)

    def test_document_restore_does_not_touch_the_weak_view_from_host_callback(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        restore = effect.split(
            "- (void)restoreProjectStoreFromHostParameters", 1
        )[1].split("- (void)pluginInstanceAddedToDocument", 1)[0]
        create_view = effect.split("- (NSView *)createViewForParameterID:", 1)[1].split(
            "- (NSSet<Class> *)classesForCustomParameterID:", 1
        )[0]

        self.assertNotIn("projectView", restore)
        self.assertIn("[view refreshStatus]", create_view)

    def test_document_added_callback_does_not_query_unavailable_parameter_apis(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        callback = effect.split("- (void)pluginInstanceAddedToDocument", 1)[1].split(
            "- (NSView *)createViewForParameterID:", 1
        )[0]

        self.assertNotIn("restoreProjectStoreFromHostParameters", callback)
        self.assertNotIn("parameterRetrievalAPI", callback)
        self.assertNotIn("apiForProtocol", callback)


class FinalCutPayloadCapacityGateTests(unittest.TestCase):
    def test_release_gate_records_passing_host_round_trip(self):
        gate = json.loads(
            (ROOT / "finalcut" / "config" / "capacity-gate.json").read_text(
                encoding="utf-8"
            )
        )

        self.assertGreaterEqual(gate["minimum_representative_project_bytes"], 1024 * 1024)
        self.assertTrue(gate["host_parameter_round_trip_validated"])
        self.assertFalse(gate["release_blocked"])
        self.assertFalse(gate["fallback_path_allowed"])
        self.assertEqual(gate["representative_fixture"]["project_bytes"], 1860294)
        self.assertEqual(
            gate["representative_fixture"]["encoded_payload_bytes"], 2035832
        )
        self.assertEqual(gate["representative_fixture"]["used_chunks"], 5)
        self.assertEqual(gate["bank_layout"]["chunk_bytes"], 416 * 1024)
        self.assertEqual(gate["bank_layout"]["chunks_per_bank"], 10)
        self.assertEqual(
            gate["bank_layout"]["chunk_parameter_ranges"],
            [[1910, 1919], [1930, 1939]],
        )


if __name__ == "__main__":
    unittest.main()
