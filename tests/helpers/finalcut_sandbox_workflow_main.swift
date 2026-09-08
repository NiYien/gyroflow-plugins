import Foundation

private final class URLLog {
    var values: [URL] = []
}

@main
struct FinalCutSandboxWorkflowContractRunner {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let fileManager = FileManager.default
        let sourceURL = root.appendingPathComponent("Original.fcpxml")
        let sourceData = Data(
            "<fcpxml version=\"1.14\"><project name=\"Original\"/></fcpxml>".utf8
        )
        try sourceData.write(to: sourceURL)
        let packageURL = root.appendingPathComponent("Other.fcpxmld", isDirectory: true)
        try fileManager.createDirectory(at: packageURL, withIntermediateDirectories: true)
        try sourceData.write(to: packageURL.appendingPathComponent("Info.fcpxml"))

        let target = RouteDBatchTarget(
            occurrence: 1,
            clipName: "Updated",
            assetRef: "a",
            mediaURL: "file:///A.mov",
            expectedProjectPath: "/Projects/A.gyroflow",
            projectDisplayName: "A.gyroflow",
            action: .updatedProject,
            skipReason: nil,
            detail: "updated",
            geometryStatus: nil,
            geometryReasons: [],
            geometryDetail: nil
        )
        let skipped = RouteDBatchTarget(
            occurrence: 2,
            clipName: "Skipped",
            assetRef: "b",
            mediaURL: "file:///B.mov",
            expectedProjectPath: "/Projects/B.gyroflow",
            projectDisplayName: "B.gyroflow",
            action: .skipped,
            skipReason: .missingProject,
            detail: "missing",
            geometryStatus: nil,
            geometryReasons: [],
            geometryDetail: nil
        )
        let reportData = Data("""
        {"original_project_name":"Original","occurrence_count":2,"updated_project_count":1,"timing_only_count":0,"skipped_count":1,"targets":[{"occurrence":1,"clip_name":"Updated","asset_ref":"a","media_url":"file:///A.mov","expected_project_path":"/Projects/A.gyroflow","project_display_name":"A.gyroflow","action":"updated_project","detail":"updated","geometry_reasons":[]},{"occurrence":2,"clip_name":"Skipped","asset_ref":"b","media_url":"file:///B.mov","expected_project_path":"/Projects/B.gyroflow","project_display_name":"B.gyroflow","action":"skipped","skip_reason":"missing_project","detail":"missing","geometry_reasons":[]}]}
        """.utf8)
        let emptyReportData = Data("""
        {"original_project_name":"Original","occurrence_count":1,"updated_project_count":0,"timing_only_count":0,"skipped_count":1,"targets":[{"occurrence":2,"clip_name":"Skipped","asset_ref":"b","media_url":"file:///B.mov","expected_project_path":"/Projects/B.gyroflow","project_display_name":"B.gyroflow","action":"skipped","skip_reason":"missing_project","detail":"missing","geometry_reasons":[]}]}
        """.utf8)
        let outputData = Data(
            "<fcpxml version=\"1.14\"><project name=\"Replaced\"/></fcpxml>".utf8
        )
        var processCalls = 0
        var useEmptyReport = false
        var processedSources: [URL] = []
        let starts = URLLog()
        let stops = URLLog()
        let access = SecurityScopedAccess(
            start: { url in starts.values.append(url); return true },
            stop: { url in stops.values.append(url) }
        )
        let opened = URLLog()
        let workflow = SandboxedRouteDWorkflow(
            access: access,
            process: { source in
                processCalls += 1
                processedSources.append(source.selectionURL)
                return RouteDBatchProcessorOutput(
                    fcpxml: outputData,
                    report: useEmptyReport ? emptyReportData : reportData
                )
            },
            open: { url in opened.values.append(url); return false }
        )

        var results: [String: Bool] = [:]
        workflow.process()
        results["inputRequiredBeforeProcess"] = processCalls == 0 && workflow.state.isFailed

        workflow.selectInput(sourceURL)
        workflow.process()
        results["oneInputIsEnoughToPreview"] = workflow.state == .preview
            && workflow.preparedProject?.report.skippedCount == 1
            && processedSources == [sourceURL.standardizedFileURL]
        results["onlySelectedDocumentScopeIsBalanced"] = starts.values == stops.values
            && starts.values.filter { $0 == sourceURL.standardizedFileURL }.count == 2

        workflow.saveReplacingSource()
        let replacedSourceData = try Data(contentsOf: sourceURL)
        results["openFailurePreservesReplacedSource"] = workflow.state.isSaved
            && workflow.savedProject?.warning != nil
            && replacedSourceData == outputData
            && opened.values == [sourceURL.standardizedFileURL]
        _ = workflow.reopenSavedProject()
        results["openFailureCanRetry"] = opened.values.count == 2

        workflow.selectInput(sourceURL)
        useEmptyReport = true
        workflow.process()
        results["allSkippedProducesNoOutputButKeepsReport"] = workflow.state.isFailed
            && workflow.preparedProject?.report.skippedCount == 1
            && workflow.savedProject == nil
        useEmptyReport = false

        workflow.selectInput(sourceURL)
        let stalePreparation = workflow.makePreparationJob()
        let newSelection = workflow.makeInputSelectionJob(packageURL)
        _ = workflow.acceptInputSelection(newSelection.run(), from: newSelection)
        if let stalePreparation {
            results["stalePreparationCannotReplaceNewSelection"] = !workflow.acceptPreparation(
                stalePreparation.run(),
                from: stalePreparation
            ) && workflow.source?.selectionURL == packageURL.standardizedFileURL
        }

        workflow.process()
        let packageXML = packageURL.appendingPathComponent("Info.fcpxml")
        let packageBeforeStaleSave = try Data(contentsOf: packageXML)
        let staleSave = workflow.makeSaveJob()
        workflow.selectInput(sourceURL)
        if let staleSave {
            let saveResult = staleSave.run()
            let accepted = workflow.acceptSave(saveResult, from: staleSave)
            let stagingFiles = try fileManager.contentsOfDirectory(atPath: packageURL.path)
                .filter { $0.hasPrefix(".gyroflow-staging-") }
            let packageAfterStaleSave = try Data(contentsOf: packageXML)
            results["staleSaveCannotReplacePreviousSelection"] = !accepted
                && packageAfterStaleSave == packageBeforeStaleSave
                && stagingFiles.isEmpty
        }

        workflow.selectInput(packageURL)
        workflow.process()
        workflow.saveReplacingSource()
        let replacedPackageData = try Data(contentsOf: packageXML)
        results["packageReplacesInfoAndOpensPackage"] = workflow.state.isSaved
            && replacedPackageData == outputData
            && opened.values.last == packageURL.standardizedFileURL

        let changedURL = root.appendingPathComponent("Changed.fcpxml")
        try sourceData.write(to: changedURL)
        workflow.selectInput(changedURL)
        workflow.process()
        let externallyChanged = Data(
            "<fcpxml version=\"1.14\"><project name=\"External\"/></fcpxml>".utf8
        )
        try externallyChanged.write(to: changedURL)
        let openedBeforeChangedSave = opened.values.count
        workflow.saveReplacingSource()
        let preservedExternalData = try Data(contentsOf: changedURL)
        results["externallyChangedSourceIsPreserved"] = workflow.state.isFailed
            && preservedExternalData == externallyChanged
            && opened.values.count == openedBeforeChangedSave

        let report = RouteDBatchReport(
            originalProjectName: "Original",
            occurrenceCount: 2,
            updatedProjectCount: 1,
            timingOnlyCount: 0,
            skippedCount: 1,
            targets: [target, skipped]
        )
        results["reportFilteringRemainsAccurate"] = report
            .matchingTargets(issuesOnly: true, query: "B.gyroflow") == [skipped]
            && target.structuralIdentity != skipped.structuralIdentity

        let output = try JSONSerialization.data(withJSONObject: results, options: [.sortedKeys])
        FileHandle.standardOutput.write(output)
    }
}
