import Foundation

@main
struct FinalCutFCPXMLInputContractRunner {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let fileManager = FileManager.default
        var results: [String: Bool] = [:]
        let video = root.appendingPathComponent("A001.mov")
        try Data("video-sentinel-must-not-be-read".utf8).write(to: video)
        try fileManager.setAttributes([.posixPermissions: 0], ofItemAtPath: video.path)
        let xml = root.appendingPathComponent("Project.fcpxml")
        try Data(
            "<fcpxml version=\"1.14\"><resources><asset id=\"a\"><media-rep kind=\"original-media\" src=\"\(video.absoluteString)\"/></asset></resources><project name=\"P\"/></fcpxml>".utf8
        ).write(to: xml)

        let resolvedFile = try FCPXMLDocumentInput.resolve(xml)
        results["singleFile"] = resolvedFile.selectionURL == xml.standardizedFileURL
            && resolvedFile.xmlURL == xml.standardizedFileURL
            && resolvedFile.data.starts(with: Data("<fcpxml".utf8))

        let package = root.appendingPathComponent("Project.fcpxmld", isDirectory: true)
        try fileManager.createDirectory(at: package, withIntermediateDirectories: true)
        let packageInfo = package.appendingPathComponent("Info.fcpxml")
        try Data("<fcpxml version=\"1.14\"><project name=\"Package\"/></fcpxml>".utf8)
            .write(to: packageInfo)
        let resolvedPackage = try FCPXMLDocumentInput.resolve(package)
        results["packageRootInfo"] = resolvedPackage.selectionURL == package.standardizedFileURL
            && resolvedPackage.xmlURL == packageInfo.standardizedFileURL

        let nestedPackage = root.appendingPathComponent("Nested.fcpxmld", isDirectory: true)
        let nested = nestedPackage.appendingPathComponent("Nested", isDirectory: true)
        try fileManager.createDirectory(at: nested, withIntermediateDirectories: true)
        try Data("<fcpxml/>".utf8).write(to: nested.appendingPathComponent("Info.fcpxml"))
        results["nestedInfoRejected"] = (try? FCPXMLDocumentInput.resolve(nestedPackage)) == nil

        let outside = root.appendingPathComponent("Outside.fcpxml")
        try Data("<fcpxml/>".utf8).write(to: outside)
        let linkedPackage = root.appendingPathComponent("Linked.fcpxmld", isDirectory: true)
        try fileManager.createDirectory(at: linkedPackage, withIntermediateDirectories: true)
        try fileManager.createSymbolicLink(
            at: linkedPackage.appendingPathComponent("Info.fcpxml"),
            withDestinationURL: outside
        )
        results["packageSymlinkEscapeRejected"] =
            (try? FCPXMLDocumentInput.resolve(linkedPackage)) == nil

        let exportBase = root.appendingPathComponent("Exports", isDirectory: true)
        try fileManager.createDirectory(at: exportBase, withIntermediateDirectories: true)
        let firstWorkspace = try UniqueExportWorkspace.create(baseDirectory: exportBase)
        let secondWorkspace = try UniqueExportWorkspace.create(baseDirectory: exportBase)
        results["uniqueWorkspaces"] = firstWorkspace.directoryURL != secondWorkspace.directoryURL
            && firstWorkspace.directoryURL.deletingLastPathComponent() == exportBase
            && secondWorkspace.directoryURL.deletingLastPathComponent() == exportBase

        let exported = firstWorkspace.directoryURL.appendingPathComponent("Current.fcpxml")
        try Data("<fcpxml version=\"1.14\"/>".utf8).write(to: exported)
        var clock: TimeInterval = 0
        var waits = 0
        let observer = UniqueExportWorkspace(
            directoryURL: firstWorkspace.directoryURL,
            monotonicTime: { clock },
            wait: { interval in
                waits += 1
                clock += interval
                if waits == 1 {
                    try? Data("<fcpxml version=\"1.14\"><project/></fcpxml>".utf8)
                        .write(to: exported)
                }
            }
        )
        let stable = try observer.waitForStableInput(timeout: 1, pollInterval: 0.01)
        results["stableUniqueOutput"] = stable.xmlURL == exported.standardizedFileURL
            && waits >= 2

        let ambiguousDirectory = secondWorkspace.directoryURL
        try Data("<fcpxml/>".utf8).write(
            to: ambiguousDirectory.appendingPathComponent("One.fcpxml")
        )
        try Data("<fcpxml/>".utf8).write(
            to: ambiguousDirectory.appendingPathComponent("Two.fcpxml")
        )
        var ambiguousClock: TimeInterval = 0
        let ambiguousObserver = UniqueExportWorkspace(
            directoryURL: ambiguousDirectory,
            monotonicTime: { ambiguousClock },
            wait: { interval in ambiguousClock += interval }
        )
        results["ambiguousOutputRejected"] = (try? ambiguousObserver.waitForStableInput(
            timeout: 0.02,
            pollInterval: 0.01
        )) == nil

        let destination = root.appendingPathComponent("Processed.fcpxml")
        try ProcessedProjectStore.write(
            Data("processed".utf8),
            to: destination,
            source: resolvedFile
        )
        results["atomicSeparateWrite"] = try Data(contentsOf: destination)
            == Data("processed".utf8)
        results["sourceOverwriteRejected"] = (try? ProcessedProjectStore.write(
            Data("bad".utf8),
            to: xml,
            source: resolvedFile
        )) == nil
        results["packageInteriorRejected"] = (try? ProcessedProjectStore.write(
            Data("bad".utf8),
            to: package.appendingPathComponent("Processed.fcpxml"),
            source: resolvedPackage
        )) == nil

        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: video.path)
        let output = try JSONSerialization.data(withJSONObject: results, options: [.sortedKeys])
        FileHandle.standardOutput.write(output)
    }
}
