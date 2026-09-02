import Foundation

private enum FakeError: Error {
    case failed
}

@main
struct FinalCutWorkflowContractRunner {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let fileManager = FileManager.default
        var results: [String: Bool] = [:]
        let manualFile = root.appendingPathComponent("Manual.fcpxml")
        try Data("<fcpxml version=\"1.14\"/>".utf8).write(to: manualFile)
        let package = root.appendingPathComponent("Manual.fcpxmld", isDirectory: true)
        try fileManager.createDirectory(at: package, withIntermediateDirectories: true)
        try Data("<fcpxml version=\"1.14\"/>".utf8)
            .write(to: package.appendingPathComponent("Info.fcpxml"))
        let reportData = try JSONSerialization.data(withJSONObject: [
            "original_project_name": "Original",
            "processed_project_name": "Processed",
            "import_token": "token",
            "occurrence_count": 2,
            "inserted_count": 1,
            "updated_count": 1,
            "skipped_count": 1,
            "failed_count": 0,
            "targets": [
                ["occurrence": 1, "clip_name": "Existing", "asset_ref": "a", "media_url": "file:///A.mov", "action": "updated", "detail": "updated", "geometry_status": "runtime_live", "geometry_reasons": [], "geometry_detail": "asset_format=1920x1080 pasp=1/1"],
                ["occurrence": 2, "clip_name": "Inserted", "asset_ref": "b", "media_url": "file:///B.mov", "action": "inserted", "detail": "inserted", "geometry_status": "runtime_live", "geometry_reasons": [], "geometry_detail": "asset_format=1080x1920 pasp=1/1"],
                ["clip_name": "Compound", "asset_ref": "c", "action": "skipped", "detail": "unsupported"],
            ],
        ])
        let decodedReport = try JSONDecoder().decode(RouteDBatchReport.self, from: reportData)
        var processCalls = 0
        var writeCalls = 0
        var openCalls = 0
        var denyExport = false
        var failProcess = false
        let dependencies = OneClickRouteDDependencies(
            makeWorkspace: {
                try UniqueExportWorkspace.create(baseDirectory: root)
            },
            exportCurrentProject: { _ in
                if denyExport {
                    throw FinalCutAutomationError.permissionRequired
                }
                return try FCPXMLDocumentInput.resolve(manualFile)
            },
            resolveManualInput: { try FCPXMLDocumentInput.resolve($0) },
            process: { input, _ in
                processCalls += 1
                if failProcess {
                    throw FakeError.failed
                }
                return RouteDBatchProcessorOutput(
                    fcpxml: Data("processed:\(input.count)".utf8),
                    report: reportData
                )
            },
            write: { data, destination, source in
                writeCalls += 1
                try ProcessedProjectStore.write(data, to: destination, source: source)
            },
            open: { _ in
                openCalls += 1
                return true
            }
        )
        let workflow = OneClickRouteDWorkflow(dependencies: dependencies)
        var states: [OneClickRouteDState] = []
        workflow.onStateChange = { states.append($0) }

        denyExport = true
        workflow.processCurrentProject(processedName: "Denied")
        results["permissionDenialStopsBeforeProcessing"] = processCalls == 0
            && openCalls == 0
            && workflow.preparedProject == nil
            && workflow.failedTargets.count == 1

        denyExport = false
        workflow.processCurrentProject(processedName: "Processed")
        let automaticPreview = workflow.preparedProject
        results["automaticStopsAtPreview"] = automaticPreview?.report.insertedCount == 1
            && automaticPreview?.report.updatedCount == 1
            && automaticPreview?.report.skippedCount == 1
            && automaticPreview?.report.targets.count == 3
            && automaticPreview?.report.targets[0].geometryStatus == "runtime_live"
            && automaticPreview?.report.targets[0].geometryReasons == []
            && automaticPreview?.report.targets[0].geometryDetail == "asset_format=1920x1080 pasp=1/1"
            && openCalls == 0
            && states.contains(.exporting)
            && states.contains(.processing)

        workflow.processManual(selection: package, processedName: "Package Processed")
        results["manualPackageUsesSameBatchPath"] = workflow.preparedProject?.source.selectionURL
            == package.standardizedFileURL && processCalls == 2

        failProcess = true
        workflow.processManual(selection: manualFile, processedName: "Failed")
        results["failedBatchRetainsNoOutput"] = workflow.preparedProject == nil
            && workflow.failedTargets.count == 1
            && openCalls == 0
        failProcess = false

        workflow.processManual(selection: manualFile, processedName: "Confirmed")
        let destination = root.appendingPathComponent("Confirmed.fcpxml")
        workflow.confirmImport(to: destination)
        results["confirmationWritesAndOpensOnce"] = writeCalls == 1
            && openCalls == 1
            && fileManager.fileExists(atPath: destination.path)
            && workflow.state == .completed(destination.standardizedFileURL)
        workflow.confirmImport(to: root.appendingPathComponent("Second.fcpxml"))
        results["secondConfirmationIsRejected"] = writeCalls == 1 && openCalls == 1

        let cancelWorkflow = OneClickRouteDWorkflow(dependencies: dependencies)
        cancelWorkflow.processManual(selection: manualFile, processedName: "Cancelled")
        cancelWorkflow.cancelPreview()
        results["cancelOpensNothing"] = cancelWorkflow.preparedProject == nil
            && cancelWorkflow.state == .idle
            && openCalls == 1

        let overwriteWorkflow = OneClickRouteDWorkflow(dependencies: dependencies)
        overwriteWorkflow.processManual(selection: manualFile, processedName: "Overwrite")
        overwriteWorkflow.confirmImport(to: manualFile)
        results["originalDestinationRejected"] = overwriteWorkflow.state.isFailure
            && openCalls == 1

        let output = try JSONSerialization.data(withJSONObject: results, options: [.sortedKeys])
        FileHandle.standardOutput.write(output)
    }
}
