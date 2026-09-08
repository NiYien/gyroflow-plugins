import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "finalcut" / "Xcode" / "App"
EFFECT = ROOT / "finalcut" / "Xcode" / "Effect"


class FinalCutUIContractTests(unittest.TestCase):
    def test_wrapper_has_one_selection_and_no_media_generate_or_save_controls(self):
        view = (APP / "BatchProcessView.swift").read_text(encoding="utf-8")
        model = (APP / "FinalCutAppModel.swift").read_text(encoding="utf-8")
        app_main = (APP / "AppMain.swift").read_text(encoding="utf-8")

        self.assertEqual(view.count("stageContainer(title:"), 1)
        self.assertIn('FinalCutStrings.text("app.step.choose_fcpxml")', view)
        self.assertNotIn('FinalCutStrings.text("app.step.save_open")', view)
        self.assertIn(".defaultSize(width: 580, height: 240)", app_main)
        for removed in (
            "authorizationStage",
            "chooseMediaRoots",
            "addMediaRoots",
            "removeMediaRoot",
            "NSSavePanel",
            'FinalCutStrings.text("app.action.generate")',
            'FinalCutStrings.text("app.action.save_replacement")',
        ):
            self.assertNotIn(removed, view + model)
        self.assertIn("self.process()", model)
        self.assertIn("self.saveReplacement(to: self.automaticDestination", model)

    def test_report_is_one_collapsed_processed_video_list(self):
        view = (APP / "BatchProcessView.swift").read_text(encoding="utf-8")
        workflow = (APP / "SandboxedRouteDWorkflow.swift").read_text(encoding="utf-8")

        self.assertIn("processedVideosExpanded = false", view)
        self.assertIn('"app.label.processed_videos"', view)
        self.assertIn("LazyVStack", view)
        self.assertIn("DisclosureGroup", view)
        self.assertIn("ForEach(report.targets", view)
        self.assertIn("expandedByDefault: false", view)
        for removed in (
            "app.label.report",
            "app.label.report_total",
            "app.label.report_updated",
            "app.label.report_timing",
            "app.label.report_skipped",
            "app.filter.issues",
            "app.filter.all",
            "app.placeholder.search",
            "BatchReportFilter",
            "reportFilter",
            "searchText",
            "successesExpanded",
            "reportMetric",
        ):
            self.assertNotIn(removed, view)
        self.assertIn("matchingTargets(issuesOnly: Bool, query: String)", workflow)

    def test_processing_can_cancel_and_saved_output_can_retry_or_reveal(self):
        view = (APP / "BatchProcessView.swift").read_text(encoding="utf-8")
        model = (APP / "FinalCutAppModel.swift").read_text(encoding="utf-8")

        self.assertIn("ProgressView()", view)
        self.assertIn("model.cancelActiveWork()", view)
        self.assertNotIn("saved.destination.path", view)
        self.assertIn("model.retryOpenSavedProject", view)
        self.assertIn("model.revealSavedProject", view)
        self.assertIn("let warning = saved.warning", view)
        self.assertIn("activateFileViewerSelecting", model)
        self.assertIn("workflow.reopenSavedProject", model)
        self.assertNotIn('FinalCutStrings.text("app.replace.unconfirmed")', view)
        self.assertNotIn("outputStage", view)

    def test_inspector_has_one_load_action_and_no_temporary_open_path(self):
        view = (EFFECT / "GFProjectDropView.m").read_text(encoding="utf-8")
        store = (EFFECT / "GFProjectStore.h").read_text(encoding="utf-8")
        project = (ROOT / "finalcut" / "Xcode" / "GyroflowFinalCut.xcodeproj" /
                   "project.pbxproj").read_text(encoding="utf-8")

        self.assertIn("GFProjectModeStatus", store)
        self.assertIn("self.nameLabel", view)
        self.assertIn("self.modeLabel", view)
        self.assertIn("self.statusLabel.maximumNumberOfLines = 2", view)
        self.assertIn('GFLocalized(@"effect.action.load_project"', view)
        for removed in (
            "openButton",
            "openProject:",
            "GFEmbeddedProjectOpener",
            "effect.action.open_gyroflow",
            "effect.warning.temporary_copy",
        ):
            self.assertNotIn(removed, view + store + project)

    def test_fov_remains_a_saved_parameter_but_has_no_user_facing_control(self):
        effect = (EFFECT / "GyroflowFinalCutEffect.m").read_text(encoding="utf-8")
        fov_block = effect.split(
            'addFloatSliderWithName:GFLocalized(@"effect.param.fov"', 1
        )[1].split(
            "ok = ok && [parameters addFloatSliderWithName:", 1
        )[0]

        self.assertIn("parameterID:kGFFOV", fov_block)
        self.assertIn("kFxParameterFlag_HIDDEN", fov_block)
        self.assertIn("kFxParameterFlag_DONT_DISPLAY_IN_DASHBOARD", fov_block)
        self.assertNotIn("kFxParameterFlag_NOT_ANIMATABLE", fov_block)
        adjustment_group = effect.split(
            '@"effect.group.adjust"', 1
        )[1].split("[parameters endParameterSubGroup]", 1)[0]
        self.assertNotIn("kGFFOV", adjustment_group)


if __name__ == "__main__":
    unittest.main()
