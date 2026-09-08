import AppKit
import Foundation

enum RouteDProcessor {
    static func processBatchFCPXML(
        _ input: Data,
        documentURL: URL
    ) throws -> [String: Data] {
        guard !input.isEmpty, documentURL.isFileURL else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let report = Data("""
        {"original_project_name":"Original","occurrence_count":2,"updated_project_count":1,"timing_only_count":0,"skipped_count":1,"targets":[{"occurrence":1,"clip_name":"Same Name","asset_ref":"a","media_url":"file:///Media/A.mov","expected_project_path":"/Media/A.gyroflow","project_display_name":"A.gyroflow","action":"updated_project","detail":"updated","geometry_reasons":[]},{"occurrence":2,"clip_name":"Same Name","asset_ref":"b","media_url":"file:///Media/B.mov","expected_project_path":"/Media/B.gyroflow","project_display_name":"B.gyroflow","action":"skipped","skip_reason":"missing_project","detail":"missing","geometry_reasons":[]}]}
        """.utf8)
        let output = Data(
            "<fcpxml version=\"1.14\"><project name=\"Original\"/></fcpxml>".utf8
        )
        return ["fcpxml": output, "report": report]
    }
}

@main
@MainActor
struct FinalCutAppModelContractRunner {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let source = root.appendingPathComponent("Original.fcpxml")
        try Data("<fcpxml version=\"1.14\"><project name=\"Original\"/></fcpxml>".utf8)
            .write(to: source)

        var opened: [URL] = []
        let model = FinalCutAppModel(
            openURL: { url in
                opened.append(url)
                return true
            },
            outputRoot: root.appendingPathComponent("Automatic", isDirectory: true)
        )
        model.selectFCPXML(source)
        let deadline = Date().addingTimeInterval(5)
        while model.routeDState != .saved && !model.routeDState.isFailed && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        let savedURL = model.savedProject?.destination

        var failedOpenCount = 0
        let openFailureModel = FinalCutAppModel(
            openURL: { _ in
                failedOpenCount += 1
                return false
            },
            outputRoot: root.appendingPathComponent("Open Failure", isDirectory: true)
        )
        openFailureModel.selectFCPXML(source)
        let failureDeadline = Date().addingTimeInterval(5)
        while openFailureModel.routeDState != .saved &&
                !openFailureModel.routeDState.isFailed &&
                Date() < failureDeadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        let failedOpenURL = openFailureModel.savedProject?.destination
        openFailureModel.retryOpenSavedProject()

        let slowStarted = DispatchSemaphore(value: 0)
        let slowRelease = DispatchSemaphore(value: 0)
        let cancelledModel = FinalCutAppModel(
            processBatch: { source in
                slowStarted.signal()
                _ = slowRelease.wait(timeout: .now() + 5)
                let result = try RouteDProcessor.processBatchFCPXML(
                    source.data,
                    documentURL: source.xmlURL
                )
                return RouteDBatchProcessorOutput(
                    fcpxml: result["fcpxml"]!,
                    report: result["report"]!
                )
            },
            openURL: { _ in true },
            outputRoot: root.appendingPathComponent("Cancelled", isDirectory: true)
        )
        cancelledModel.selectFCPXML(source)
        let started = slowStarted.wait(timeout: .now() + 2) == .success
        cancelledModel.cancelActiveWork()
        slowRelease.signal()
        let cancelDeadline = Date().addingTimeInterval(1)
        while Date() < cancelDeadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }

        let allSkippedReport = Data("""
        {"original_project_name":"Original","occurrence_count":1,"updated_project_count":0,"timing_only_count":0,"skipped_count":1,"targets":[{"occurrence":1,"clip_name":"Missing","asset_ref":"a","media_url":"file:///Media/Missing.mov","expected_project_path":"/Media/Missing.gyroflow","project_display_name":null,"action":"skipped","skip_reason":"missing_project","detail":"missing","geometry_reasons":[]}]}
        """.utf8)
        var allSkippedOpened = false
        let allSkippedModel = FinalCutAppModel(
            processBatch: { source in
                RouteDBatchProcessorOutput(fcpxml: source.data, report: allSkippedReport)
            },
            openURL: { _ in
                allSkippedOpened = true
                return true
            },
            outputRoot: root.appendingPathComponent("All Skipped", isDirectory: true)
        )
        allSkippedModel.selectFCPXML(source)
        let allSkippedDeadline = Date().addingTimeInterval(5)
        while !allSkippedModel.routeDState.isFailed && Date() < allSkippedDeadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }

        let boundedName = model.defaultReplacementFilename(
            for: String(repeating: "é", count: 300) + "/unsafe"
        )
        let result: [String: Any] = [
            "oneSelectionAutomaticallySaved": model.routeDState == .saved
                && savedURL.map { FileManager.default.fileExists(atPath: $0.path) } == true,
            "oneSelectionAutomaticallyOpened": opened == savedURL.map { [$0] } ?? [],
            "mixedReportIsPreserved": model.preparedProject?.report.occurrenceCount == 2
                && model.skippedTargets.count == 1,
            "outputUsesUniqueContainerDirectory": savedURL?.deletingLastPathComponent()
                .deletingLastPathComponent() == root.appendingPathComponent("Automatic"),
            "openFailurePreservesOutput": failedOpenURL.map {
                FileManager.default.fileExists(atPath: $0.path)
            } == true && openFailureModel.savedProject?.warning != nil,
            "openFailureCanRetry": failedOpenCount == 2,
            "explicitCancellationDiscardsLateResult": started
                && cancelledModel.routeDState == .inputReady
                && cancelledModel.preparedProject == nil
                && cancelledModel.savedProject == nil,
            "allSkippedEndsInFailure": allSkippedModel.routeDState.isFailed,
            "allSkippedKeepsReport": allSkippedModel.preparedProject?.report.skippedCount == 1,
            "allSkippedDoesNotSave": allSkippedModel.savedProject == nil,
            "allSkippedDoesNotOpen": !allSkippedOpened,
            "allSkippedKeepsNoTargetsMessage": allSkippedModel.workflowMessage
                == FinalCutStrings.text(
                    "app.error.route.no_targets",
                    fallback: "The batch did not update any replacement targets."
                ),
            "filenameWithinUTF8Budget": boundedName.utf8.count <= 255,
        ]
        let json = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        FileHandle.standardOutput.write(json)
    }
}
