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

        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: video.path)
        let output = try JSONSerialization.data(withJSONObject: results, options: [.sortedKeys])
        FileHandle.standardOutput.write(output)
    }
}
