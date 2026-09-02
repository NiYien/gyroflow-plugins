import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EFFECT = ROOT / "finalcut" / "xcode" / "Effect"
PUBLIC_INCLUDE = ROOT / "finalcut" / "include"
POLICY_HELPER = ROOT / "tests" / "helpers" / "finalcut_render_policy_main.c"
GEOMETRY_HELPER = ROOT / "tests" / "helpers" / "finalcut_frame_geometry_adapter_main.m"
FXPLUG_FRAMEWORKS = "/Library/Developer/SDKs/FxPlug.sdk/Library/Frameworks"


class FinalCutRenderPolicyTests(unittest.TestCase):
    def test_live_frame_geometry_adapter_normalizes_current_tiles_and_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "finalcut-frame-geometry-adapter"
            build = subprocess.run(
                [
                    "xcrun",
                    "clang",
                    "-fobjc-arc",
                    "-fmodules",
                    f"-fmodules-cache-path={Path(directory) / 'module-cache'}",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-F",
                    FXPLUG_FRAMEWORKS,
                    "-framework",
                    "FxPlug",
                    "-framework",
                    "Foundation",
                    "-I",
                    str(EFFECT),
                    "-I",
                    str(PUBLIC_INCLUDE),
                    str(EFFECT / "GFFrameGeometryAdapter.m"),
                    str(GEOMETRY_HELPER),
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

    def test_fail_closed_statuses_choose_safe_passthrough(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "finalcut_render_policy"
            build = subprocess.run(
                [
                    "xcrun",
                    "clang",
                    "-std=c11",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-I",
                    str(EFFECT),
                    "-I",
                    str(PUBLIC_INCLUDE),
                    str(EFFECT / "GFRenderPolicy.c"),
                    str(POLICY_HELPER),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, msg=build.stdout + build.stderr)
            run = subprocess.run(
                [str(executable)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, msg=run.stdout + run.stderr)

    def test_effect_requests_linear_full_buffer_and_uses_effect_local_time(self):
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")

        self.assertIn("kFxPropertyKey_NeedsFullBuffer : @YES", source)
        self.assertIn(
            "kFxPropertyKey_DesiredProcessingColorInfo : @(kFxImageColorInfo_RGB_LINEAR)",
            source,
        )
        self.assertIn(".command_queue = NULL", source)
        self.assertIn(".effect_local_time", source)
        self.assertNotIn("sourceImage.mediaTime", source)

    def test_effect_builds_v1_geometry_from_every_render_callback(self):
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        render = source.split("- (BOOL)renderDestinationImage:", 1)[1]

        self.assertIn('#import "GFFrameGeometryAdapter.h"', source)
        self.assertIn("gf_finalcut_instance_get_project_geometry(", render)
        self.assertIn("GFFrameGeometryBuild(", render)
        self.assertIn("sourceImage.pixelTransform", render)
        self.assertIn("sourceImage.inversePixelTransform", render)
        self.assertIn("destinationImage.pixelTransform", render)
        self.assertIn("destinationImage.inversePixelTransform", render)
        self.assertIn("sourceImage.imageOrigin", render)
        self.assertIn("destinationImage.imageOrigin", render)
        self.assertIn("sourceImage.imagePixelBounds", render)
        self.assertIn("sourceImage.tilePixelBounds", render)
        self.assertIn("destinationImage.imagePixelBounds", render)
        self.assertIn("destinationImage.tilePixelBounds", render)
        self.assertIn(".geometry = frameGeometry", render)
        self.assertNotIn("GFFrameGeometry){0}", render)

    def test_effect_requests_the_current_destination_tile_from_the_source(self):
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        destination_rect = source.split("- (BOOL)destinationImageRect:", 1)[1].split(
            "- (BOOL)sourceTileRect:", 1
        )[0]
        source_rect = source.split("- (BOOL)sourceTileRect:", 1)[1].split(
            "- (nullable id<MTLDevice>)deviceForRegistryID:", 1
        )[0]

        self.assertIn("destinationImage.imagePixelBounds", destination_rect)
        self.assertNotIn("sourceImages.firstObject.imagePixelBounds", destination_rect)
        self.assertIn("*sourceTileRect = destinationTileRect", source_rect)
        self.assertNotIn("sourceImages[sourceImageIndex].imagePixelBounds", source_rect)

    def test_effect_declares_pixel_transform_support_and_records_each_render(self):
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        project = (
            ROOT
            / "finalcut"
            / "Xcode"
            / "GyroflowFinalCut.xcodeproj"
            / "project.pbxproj"
        ).read_text(encoding="utf-8")

        self.assertIn(
            "kFxPropertyKey_PixelTransformSupport : "
            "@(kFxPixelTransform_ScaleTranslate)",
            source,
        )
        self.assertIn('#import "GFGeometryProbe.h"', source)
        self.assertIn("GFGeometryProbeRecordFrame(", source)
        self.assertIn("GFGeometryProbe.m", project)
        self.assertIn("GFGeometryProbe.h", project)

    def test_geometry_probe_is_bounded_opt_in_metadata_only(self):
        header_path = EFFECT / "GFGeometryProbe.h"
        source_path = EFFECT / "GFGeometryProbe.m"
        self.assertTrue(header_path.is_file(), msg=f"missing {header_path}")
        self.assertTrue(source_path.is_file(), msg=f"missing {source_path}")
        header = header_path.read_text(encoding="utf-8")
        source = source_path.read_text(encoding="utf-8")

        self.assertIn("GFGeometryProbeRecordFrame", header)
        self.assertIn("GYROFLOW_FINALCUT_GEOMETRY_PROBE", source)
        self.assertIn("kGFGeometryProbeMaxRecordCount", source)
        self.assertIn("atomic_fetch_add_explicit", source)
        for required in (
            "sourceImage.pixelTransform",
            "sourceImage.inversePixelTransform",
            "destinationImage.pixelTransform",
            "destinationImage.inversePixelTransform",
            "sourceImage.imagePixelBounds",
            "sourceImage.tilePixelBounds",
            "destinationImage.imagePixelBounds",
            "destinationImage.tilePixelBounds",
            "sourceImage.imageOrigin",
            "destinationImage.imageOrigin",
            "sourceTexture.width",
            "sourceTexture.height",
            "destinationTexture.width",
            "destinationTexture.height",
        ):
            with self.subTest(required=required):
                self.assertIn(required, source)
        for forbidden in (
            "AVAsset",
            "AVFoundation",
            "ioSurface",
            "getBytes",
            "fileURL",
            "path",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, header + source)

    def test_production_effect_has_no_media_decoding_or_analysis_api(self):
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")

        for forbidden in ("AVAsset", "AVFoundation", "importMedia", "get_video_metadata"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, source)

    def test_validated_render_snapshot_restores_restart_inspector_status(self):
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")

        self.assertIn(
            "snapshot.preparationStatus != GF_STATUS_OK",
            source,
        )
        self.assertIn(
            "restoreValidatedRenderProjectPayloadIfEmpty:projectPayload",
            source,
        )
        self.assertIn(
            "[self restoreProjectStoreFromRenderSnapshotIfNeeded:snapshot]",
            source,
        )

    def test_banked_payload_geometry_fits_final_cut_template_budget(self):
        parameter_ids = (EFFECT / "GFParameterIDs.h").read_text(encoding="utf-8")

        self.assertIn("kGFProjectPayloadChunksPerBank = 10", parameter_ids)
        self.assertIn("kGFProjectPayloadChunkBytes = 416 * 1024", parameter_ids)
        self.assertIn("kGFProjectPayloadChunksA[10]", parameter_ids)
        self.assertIn("kGFProjectPayloadChunksB[10]", parameter_ids)
        source = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        self.assertIn('addStringParameterWithName:@"Instance Identity"', source)



if __name__ == "__main__":
    unittest.main()
