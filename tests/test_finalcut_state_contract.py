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
    appkit: bool = False,
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
    if appkit:
        command.extend(["-framework", "AppKit"])
    localized_sources = {
        EFFECT / "GFProjectStore.m",
        EFFECT / "GFSourceResolver.m",
    }
    if localized_sources.intersection(sources):
        sources = [*sources, EFFECT / "GFLocalization.m"]
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
    def test_readback_failure_clears_pending_preserves_previous_and_allows_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-readback-failure",
                [EFFECT / "GFProjectStore.m", HELPERS / "finalcut_project_store_main.m"],
                blocks=True,
            )
            first = root / "A.gyroflow"
            second = root / "B.gyroflow"
            first.write_text("project-A", encoding="utf-8")
            second.write_text("project-B-is-different", encoding="utf-8")

            result = subprocess.run(
                [str(executable), "--readback-failure", str(first), str(second)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["readbackFailureClearedPending"])
            self.assertTrue(state["readbackFailurePreservedPrevious"])
            self.assertTrue(state["retryAfterReadbackFailureSucceeded"])
            self.assertEqual(state["currentProjectName"], "B.gyroflow")

    def test_pending_timeout_is_generation_scoped_and_allows_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-pending-timeout",
                [EFFECT / "GFProjectStore.m", HELPERS / "finalcut_project_store_main.m"],
                blocks=True,
            )
            first = root / "A.gyroflow"
            second = root / "B.gyroflow"
            first.write_text("project-A", encoding="utf-8")
            second.write_text("project-B-is-different", encoding="utf-8")

            result = subprocess.run(
                [str(executable), "--pending-timeout", str(first), str(second)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["wrongGenerationTimeoutIgnored"])
            self.assertTrue(state["matchingGenerationTimeoutApplied"])
            self.assertTrue(state["readbackFailurePreservedPrevious"])
            self.assertTrue(state["retryAfterReadbackFailureSucceeded"])

    def test_late_old_host_readback_cannot_rollback_a_newer_pending_load(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-stale-host-readback",
                [EFFECT / "GFProjectStore.m", HELPERS / "finalcut_project_store_main.m"],
                blocks=True,
            )
            first = root / "A.gyroflow"
            second = root / "B.gyroflow"
            first.write_text("project-A", encoding="utf-8")
            second.write_text("project-B-is-different", encoding="utf-8")

            result = subprocess.run(
                [str(executable), "--stale-host-readback", str(first), str(second)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertTrue(state["generationReadbackAPIAvailable"])
            self.assertFalse(state["staleHostReadbackApplied"])
            self.assertTrue(state["staleHostReadbackPreservedCurrentProject"])
            self.assertTrue(state["freshHostReadbackReconciled"])
            self.assertEqual(state["pendingHostCommitCalls"], 2)
            effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text()
            plugin_state = effect.split("- (BOOL)pluginState:", 1)[1]
            self.assertLess(
                plugin_state.index("beginHostProjectReadback"),
                plugin_state.index("persistedProjectPayloadWithHash"),
            )
            self.assertIn("readbackGeneration:projectReadbackGeneration", plugin_state)

    def test_pending_load_blocks_a_second_commit_until_host_generation_reconciles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-pending-generation",
                [EFFECT / "GFProjectStore.m", HELPERS / "finalcut_project_store_main.m"],
                blocks=True,
            )
            first = root / "A.gyroflow"
            second = root / "B.gyroflow"
            first.write_text("project-A", encoding="utf-8")
            second.write_text("project-B-is-different", encoding="utf-8")

            result = subprocess.run(
                [str(executable), "--pending-load", str(first), str(second)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertFalse(state["secondPendingLoadAccepted"])
            self.assertEqual(state["pendingHostCommitCalls"], 1)
            self.assertTrue(state["pendingGenerationReconciled"])
            self.assertEqual(state["currentProjectName"], "A.gyroflow")

    def test_manual_reader_rejects_sparse_project_over_raw_cap_before_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-raw-cap",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )
            oversized = root / "oversized.gyroflow"
            with oversized.open("wb") as output:
                output.truncate(256 * 1024 * 1024 + 1)

            result = subprocess.run(
                [str(executable), str(oversized)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertEqual(state["builderCalls"], 0)
            self.assertEqual(state["currentPayload"], "")
            self.assertEqual(len(state["errors"]), 1)

    def test_import_candidate_keeps_only_filename_and_project_parameter_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-candidate",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )
            source = root / "private" / "camera-card" / "P1004783.gyroflow"
            source.parent.mkdir(parents=True)
            source.write_text("valid-project", encoding="utf-8")

            result = subprocess.run(
                [str(executable), str(source)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            state = json.loads(result.stdout)
            self.assertEqual(state["currentProjectName"], "P1004783.gyroflow")
            self.assertEqual(state["builderDisplayName"], "P1004783.gyroflow")
            self.assertNotIn(str(source.parent), state["status"])
            self.assertEqual(state["candidateFOV"], 1.25)
            self.assertEqual(state["candidateSmoothness"], 42.0)
            self.assertEqual(state["candidateLensCorrection"], 95.0)
            self.assertEqual(state["candidateHorizonLock"], 33.0)
            self.assertEqual(state["candidateHorizonRoll"], 4.0)
            self.assertEqual(state["candidateZoomMode"], 0)
            self.assertEqual(state["candidateOverview"], 1)


    def test_restore_uses_persisted_filename_and_legacy_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "project-store-display-name",
                [
                    EFFECT / "GFProjectStore.m",
                    HELPERS / "finalcut_project_store_main.m",
                ],
                blocks=True,
            )

            named = subprocess.run(
                [str(executable), "--restore-no-timing"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            fallback = subprocess.run(
                [str(executable), "--restore-no-name"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(named.returncode, 0, msg=named.stdout + named.stderr)
            self.assertEqual(fallback.returncode, 0, msg=fallback.stdout + fallback.stderr)
            named_state = json.loads(named.stdout)
            fallback_state = json.loads(fallback.stdout)
            self.assertEqual(named_state["currentProjectName"], "P1004783.gyroflow")
            self.assertEqual(named_state["currentProjectFilename"], "P1004783.gyroflow")
            self.assertIn("P1004783.gyroflow", named_state["status"])
            self.assertEqual(fallback_state["currentProjectName"], "Embedded project")
            self.assertEqual(fallback_state["currentProjectFilename"], "embedded-project")
            self.assertIn("Embedded project", fallback_state["status"])

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
            self.assertEqual(state["currentProjectName"], "P1004783.gyroflow")
            self.assertIn("P1004783.gyroflow", state["status"])

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
            self.assertEqual(state["currentProjectName"], "P1004783.gyroflow")
            self.assertIn("P1004783.gyroflow", state["status"])

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
    def test_explicit_load_atomically_replaces_static_parameters_and_keyframes(self):
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
            self.assertEqual(state["lastWriteID"], 1905)
            self.assertEqual(state["legacyWriteCount"], 0)
            self.assertEqual(state["nonEmptyChunksB"], 1)
            self.assertEqual(state["manifestWriteIDs"], [1905])
            self.assertEqual(state["manifestB"]["version"], 1)
            self.assertEqual(state["manifestB"]["generation"], 2)
            self.assertEqual(state["manifestB"]["chunk_count"], 1)
            self.assertEqual(
                state["manifestB"]["encoded_length"], len(state["payload"])
            )
            self.assertEqual(len(state["manifestB"]["payload_sha256"]), 64)
            self.assertEqual(state["displayName"], "P1004783.gyroflow")
            self.assertTrue(state["projectStaticWritesAtZero"])
            self.assertEqual(state["totalKeyframes"], 0)
            self.assertEqual(state["visibleState"]["2001"]["base"], 1.25)
            self.assertEqual(state["visibleState"]["2002"]["base"], 42.0)
            self.assertEqual(state["visibleState"]["2003"]["base"], 95.0)
            self.assertEqual(state["visibleState"]["2004"]["base"], 33.0)
            self.assertEqual(state["visibleState"]["2005"]["base"], 4.0)
            self.assertEqual(state["visibleState"]["2006"]["base"], 0)
            self.assertFalse(state["visibleState"]["2007"]["base"])
            self.assertTrue(
                all(not value["frames"] for value in state["visibleState"].values())
            )

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
            self.assertGreater(state["insideActionReads"], 13)
            self.assertEqual(state["insideActionReads"], state["readCount"])
            self.assertEqual(state["outsideActionWrites"], 0)

    def test_each_transaction_failure_restores_old_bank_values_and_keyframes(self):
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
            for mode in (
                "--discard-visible-write",
                "--discard-manifest-write",
                "--fail-keyframe-removal",
                "--discard-rollback-write",
            ):
                with self.subTest(mode=mode):
                    result = subprocess.run(
                        [str(executable), mode],
                        cwd=ROOT,
                        env=environment,
                        capture_output=True,
                        text=True,
                    )

                    self.assertEqual(
                        result.returncode, 1, msg=result.stdout + result.stderr
                    )
                    state = json.loads(result.stdout)
                    self.assertFalse(state["committed"])
                    self.assertEqual(state["recoveredPayload"], "b2xkLXByb2plY3Q=")
                    self.assertEqual(state["displayName"], "old.gyroflow")
                    self.assertEqual(state["nonEmptyChunksB"], 0)
                    self.assertEqual(state["totalKeyframes"], 14)
                    self.assertTrue(state["rollbackRestored"])
                    self.assertEqual(
                        state["visibleState"], state["oldVisibleState"]
                    )
                    self.assertEqual(state["startCount"], 1)
                    self.assertEqual(state["endCount"], 1)
                    self.assertEqual(state["outsideActionWrites"], 0)

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
    def test_render_diagnostics_rate_limit_and_counters_are_thread_safe_contracts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = compile_helper(
                root,
                "render-diagnostics",
                [
                    EFFECT / "GFRenderDiagnostics.m",
                    HELPERS / "finalcut_render_diagnostics_main.m",
                ],
                blocks=True,
            )
            result = subprocess.run(
                [str(executable)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            metrics = json.loads(result.stdout)
            self.assertEqual(metrics["passthrough_logs"], 4)
            self.assertEqual(metrics["captured_logs"], 4)
            self.assertTrue(metrics["first_log_has_reason"])
            self.assertTrue(metrics["last_log_has_new_reason"])
            self.assertEqual(metrics["device_enumerations"], 1)
            self.assertEqual(metrics["command_queue_creations"], 1)
            self.assertEqual(metrics["pipeline_creations"], 1)
            self.assertEqual(metrics["project_decodes"], 1)
            self.assertEqual(metrics["project_cache_hits"], 1)
            self.assertEqual(metrics["project_cache_misses"], 1)
            self.assertEqual(metrics["plugin_state_calls"], 2)
            self.assertEqual(metrics["plugin_state_bytes"], 300)
            self.assertEqual(metrics["cache_resident_bytes"], 0)
            self.assertEqual(metrics["cache_peak_bytes"], 6144)
            self.assertEqual(metrics["cache_evictions"], 1)
            self.assertEqual(metrics["cache_purges"], 1)
            self.assertEqual(metrics["memory_pressure_purges"], 1)
            self.assertEqual(metrics["gpu_time_samples"], 1)
            self.assertEqual(metrics["gpu_ns"], 4_000_000)
            self.assertEqual(metrics["processed_frames"], 1)
            self.assertEqual(metrics["passthrough_frames"], 1)

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
                    EFFECT / "GFRenderDiagnostics.m",
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
            self.assertTrue(state["hashConflictRejected"])
            self.assertTrue(state["keyframesReusePreparedProject"])
            self.assertEqual(state["keyframeProjectDecodes"], 1)
            self.assertTrue(state["evictionReconstructed"])
            self.assertTrue(state["xpcRestartReconstructed"])
            self.assertTrue(state["multipleInstancesIndependent"])
            self.assertTrue(state["largeProjectReady"])
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

    def test_render_state_persists_display_name_hash_and_mode_for_xpc_restore(self):
        state_header = (EFFECT / "GFRenderState.h").read_text(encoding="utf-8")
        state_source = (EFFECT / "GFRenderState.m").read_text(encoding="utf-8")
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")

        self.assertIn("projectDisplayName", state_header)
        self.assertIn("projectContentHash", state_header)
        self.assertIn("schemaVersion", state_header)
        self.assertIn("GFRenderMode", state_header)
        self.assertIn("kGFRenderStateSchema = 2", state_source)
        self.assertIn("schema != 1", state_source)
        self.assertIn("snapshot.state.projectDisplayName", effect)
        self.assertIn("state.schemaVersion", effect)
        self.assertNotIn('restoreValidatedRenderProjectPayloadIfEmpty:projectPayload\n                                                displayName:@""', effect)

    def test_project_view_has_manual_import_and_single_item_drop_contract(self):
        view = (EFFECT / "GFProjectDropView.m").read_text(encoding="utf-8")

        self.assertIn("@selector(importProject:)", view)
        self.assertNotIn('effect.action.open_gyroflow', view)
        self.assertIn("panel.allowedContentTypes = @[gyroflowType]", view)
        self.assertIn("panel.allowsMultipleSelection = NO", view)
        self.assertIn("urls.count == 1", view)
        self.assertNotIn("AVAsset", view)
        self.assertIn("NSMakeRect(0, 0, 280, 56)", view)
        self.assertIn("self.statusLabel.maximumNumberOfLines = 1", view)
        self.assertIn("self.statusLabel.accessibilityValue = detail", view)
        self.assertNotIn("openButton", view)
        self.assertIn("NSMakeRect(8, 2, 264, 26)", view)
        self.assertNotIn("GFEmbeddedProjectOpener", view)

    def test_empty_timing_commit_enters_direct_mode(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        create_view = effect.split("- (NSView *)createViewForParameterID:", 1)[1].split(
            "- (NSSet<Class> *)classesForCustomParameterID:", 1
        )[0]

        self.assertNotIn("recordDirectModeReady", create_view)
        self.assertNotIn("recordReprocessRequired", create_view)
        plugin_state = effect.split("- (BOOL)pluginState:", 1)[1].split(
            "- (BOOL)destinationImageRect:", 1
        )[0]
        self.assertLess(
            plugin_state.index("reconcileHostPersistedProjectPayload"),
            plugin_state.index("recordDirectModeReady"),
        )

    def test_project_view_manual_import_is_accessible_from_view_service(self):
        view = (EFFECT / "GFProjectDropView.m").read_text(encoding="utf-8")
        button = view.split("@implementation GFImportButton", 1)[1].split(
            "@end", 1
        )[0]

        self.assertIn("@interface GFImportButton : NSButton", view)
        self.assertIn("- (BOOL)accessibilityPerformPress", view)
        self.assertIn("- (BOOL)acceptsFirstMouse:", button)
        self.assertIn("[NSApp sendAction:self.action to:self.target from:self]", view)
        self.assertIn("[[GFImportButton alloc] initWithFrame:NSMakeRect(8, 2, 264, 26)]", view)
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
