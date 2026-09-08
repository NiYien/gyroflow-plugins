import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EFFECT = ROOT / "finalcut" / "Xcode" / "Effect"
VALIDATION = ROOT / "finalcut" / "validation"


class FinalCutPerformanceGeometryContractTests(unittest.TestCase):
    def test_geometry_manifest_is_six_locale_machine_readable_and_fail_closed(self):
        schema = json.loads(
            (VALIDATION / "geometry-fixture.schema.json").read_text(encoding="utf-8")
        )
        manifest = json.loads(
            (VALIDATION / "geometry-support.json").read_text(encoding="utf-8")
        )

        locales = {entry["locale"] for entry in manifest["entries"]}
        self.assertEqual(locales, {"en", "zh-CN", "zh-TW", "ja", "ko", "ru"})
        self.assertEqual(
            set(manifest["parser_display_name_fallbacks"]), locales
        )
        self.assertEqual(manifest["verified_supported_entry_ids"], [])
        self.assertTrue(manifest["release_blocked"])
        self.assertTrue(
            all(entry["support_result"] == "unverified" for entry in manifest["entries"])
        )
        required = set(schema["required"])
        for entry in manifest["entries"]:
            self.assertTrue(required.issubset(entry))
            self.assertEqual(len(entry["expected_transform"]), 9)
            self.assertIn("maximum_channel_error", entry["tolerance"])
            self.assertIn("different_pixel_ratio", entry["tolerance"])

    def test_performance_matrix_has_required_surfaces_and_unfrozen_absolute_budget(self):
        baseline = json.loads(
            (VALIDATION / "performance-baseline.json").read_text(encoding="utf-8")
        )

        self.assertEqual(baseline["capture_status"], "not_captured")
        self.assertTrue(baseline["release_blocked"])
        self.assertIsNone(baseline["absolute_budgets"])
        self.assertEqual(
            baseline["relative_gates"],
            {
                "maximum_p95_regression_ratio": 0.05,
                "maximum_peak_memory_regression_ratio": 0.1,
            },
        )
        entries = baseline["matrix"]
        self.assertEqual({entry["resolution"] for entry in entries}, {"4k", "8k"})
        self.assertEqual(
            {entry["proxy"] for entry in entries}, {"none", "half", "quarter"}
        )
        self.assertEqual(
            {entry["parameters"] for entry in entries},
            {"static", "keyframed", "passthrough"},
        )
        self.assertEqual(
            {entry["surface"] for entry in entries},
            {"thumbnail", "preview", "export"},
        )
        self.assertIn(2, {entry["instances"] for entry in entries})
        self.assertTrue(all(entry["measurements"] is None for entry in entries))

    def test_project_import_prepares_off_main_and_commits_only_on_main(self):
        store_header = (EFFECT / "GFProjectStore.h").read_text(encoding="utf-8")
        view = (EFFECT / "GFProjectDropView.m").read_text(encoding="utf-8")

        self.assertIn("prepareImportProjectURL", store_header)
        self.assertIn("commitPreparedImportCandidate", store_header)
        self.assertIn("QOS_CLASS_USER_INITIATED", view)
        background = view.split(
            "dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0)", 1
        )[1]
        prepare_index = background.index("prepareImportProjectURL")
        main_index = background.index("dispatch_async(dispatch_get_main_queue()")
        commit_index = background.index("commitPreparedImportCandidate")
        self.assertLess(prepare_index, main_index)
        self.assertLess(main_index, commit_index)
        self.assertIn("importGeneration != generation", view)
        self.assertIn("startAccessingSecurityScopedResource", (EFFECT / "GFProjectStore.m").read_text())
        self.assertIn("stopAccessingSecurityScopedResource", (EFFECT / "GFProjectStore.m").read_text())


if __name__ == "__main__":
    unittest.main()
