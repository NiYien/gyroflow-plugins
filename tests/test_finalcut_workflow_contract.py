import plistlib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "finalcut" / "Xcode" / "App"


class FinalCutWorkflowContractTests(unittest.TestCase):
    def test_one_selection_automatically_processes_saves_and_opens(self):
        model = (APP / "FinalCutAppModel.swift").read_text(encoding="utf-8")
        workflow = (APP / "SandboxedRouteDWorkflow.swift").read_text(encoding="utf-8")

        self.assertIn('UTType(filenameExtension: "fcpxml")', model)
        self.assertIn('importedAs: "com.apple.finalcutpro.xmld"', model)
        self.assertIn("conformingTo: .package", model)
        self.assertIn("allowsMultipleSelection = false", model)
        self.assertIn("canChooseDirectories = true", model)
        self.assertIn("treatsFilePackagesAsDirectories = false", model)
        self.assertIn("documentURL: source.xmlURL", model)
        self.assertNotIn("documentURL: source.selectionURL", model)
        self.assertIn("self.process()", model)
        self.assertIn("automaticDestination", model)
        self.assertIn("acceptSave", model)
        self.assertIn("let warning = open(destination)", workflow)
        for removed in (
            "mediaRoots",
            "setMediaRoots",
            "chooseMediaRoots",
            "NSSavePanel",
        ):
            self.assertNotIn(removed, model + workflow)

    def test_non_sandboxed_bridge_reads_only_exact_sibling_project_snapshots(self):
        processor = (APP / "RouteDProcessor.m").read_text(encoding="utf-8")
        header = (ROOT / "finalcut" / "include" / "GyroflowFinalCut.h").read_text(
            encoding="utf-8"
        )
        info = plistlib.loads((APP / "Info.plist").read_bytes())
        entitlements = plistlib.loads((APP / "App.entitlements").read_bytes())

        self.assertNotIn("URLByResolvingBookmarkData", processor)
        self.assertNotIn("NSFileCoordinator", processor)
        self.assertNotIn("primaryPresentedItemURL", processor)
        self.assertIn("GFReadProject(projectURL, expectedPath)", processor)
        self.assertIn("GF_ROUTE_D_PROJECT_INPUT_MISSING", processor)
        self.assertIn("gf_finalcut_route_d_batch_patch_with_project_inputs", processor)
        self.assertIn("GFRouteDProjectInput", header)
        self.assertEqual(entitlements, {})
        self.assertNotIn("CFBundleDocumentTypes", info)
        self.assertNotIn("UTImportedTypeDeclarations", info)
        for key in (
            "NSDesktopFolderUsageDescription",
            "NSDocumentsFolderUsageDescription",
            "NSDownloadsFolderUsageDescription",
            "NSNetworkVolumesUsageDescription",
            "NSRemovableVolumesUsageDescription",
        ):
            self.assertTrue(info[key])
        for forbidden in ("AVAsset", "Data(contentsOf: mediaURL", "contentsOfDirectory"):
            self.assertNotIn(forbidden, processor)

    def test_manual_flow_has_no_accessibility_automation(self):
        sources = "\n".join(
            path.read_text(encoding="utf-8")
            for path in APP.iterdir()
            if path.suffix in {".swift", ".h", ".m"}
        )
        for symbol in (
            "AXUIElement",
            "AXIsProcessTrusted",
            "CGEvent",
            "ApplicationServices",
            "exportCurrentProject",
            "OneClickRouteD",
        ):
            self.assertNotIn(symbol, sources)


if __name__ == "__main__":
    unittest.main()
