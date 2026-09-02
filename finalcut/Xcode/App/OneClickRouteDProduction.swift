import AppKit
import Foundation

enum OneClickRouteDProduction {
    static func dependencies(
        accessibilityDriver: FinalCutAccessibilityDriver = FinalCutAccessibilityDriver()
    ) -> OneClickRouteDDependencies {
        OneClickRouteDDependencies(
            makeWorkspace: {
                try UniqueExportWorkspace.create()
            },
            exportCurrentProject: { workspace in
                _ = try accessibilityDriver.exportCurrentProject(
                    to: workspace.directoryURL,
                    fileName: "Current Project.fcpxml",
                    timeout: 20
                )
                return try workspace.waitForStableInput(
                    timeout: 30,
                    pollInterval: 0.1
                )
            },
            resolveManualInput: { selection in
                try FCPXMLDocumentInput.resolve(selection)
            },
            process: { input, processedName in
                let result = try RouteDProcessor.processBatchFCPXML(
                    input,
                    processedName: processedName
                )
                guard let fcpxml = result["fcpxml"],
                      let report = result["report"]
                else {
                    throw CocoaError(.fileReadCorruptFile)
                }
                return RouteDBatchProcessorOutput(fcpxml: fcpxml, report: report)
            },
            write: { data, destination, source in
                try ProcessedProjectStore.write(data, to: destination, source: source)
            },
            open: { destination in
                NSWorkspace.shared.open(destination)
            }
        )
    }
}
