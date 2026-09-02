import Foundation

enum ProcessedProjectStoreError: LocalizedError {
    case sourceWouldBeModified
    case destinationExists

    var errorDescription: String? {
        switch self {
        case .sourceWouldBeModified:
            return "The processed project must be written separately from the original input."
        case .destinationExists:
            return "The processed project destination already exists."
        }
    }
}

enum ProcessedProjectStore {
    static func write(
        _ data: Data,
        to destination: URL,
        source: ResolvedFCPXMLInput,
        fileManager: FileManager = .default
    ) throws {
        let destination = resolvedDestination(destination)
        let selection = source.selectionURL.resolvingSymlinksInPath()
        let xml = source.xmlURL.resolvingSymlinksInPath()
        if destination == selection || destination == xml {
            throw ProcessedProjectStoreError.sourceWouldBeModified
        }
        if selection.pathExtension.lowercased() == "fcpxmld",
           isDescendant(destination, of: selection)
        {
            throw ProcessedProjectStoreError.sourceWouldBeModified
        }
        guard !fileManager.fileExists(atPath: destination.path) else {
            throw ProcessedProjectStoreError.destinationExists
        }
        let parent = destination.deletingLastPathComponent()
        try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)
        let staging = parent.appendingPathComponent(
            ".\(destination.lastPathComponent).staging.\(UUID().uuidString)"
        )
        defer {
            try? fileManager.removeItem(at: staging)
        }
        try data.write(to: staging, options: [.withoutOverwriting])
        let handle = try FileHandle(forWritingTo: staging)
        try handle.synchronize()
        try handle.close()
        try fileManager.moveItem(at: staging, to: destination)
    }

    private static func resolvedDestination(_ destination: URL) -> URL {
        let standardized = destination.standardizedFileURL
        let parent = standardized.deletingLastPathComponent().resolvingSymlinksInPath()
        return parent.appendingPathComponent(standardized.lastPathComponent)
    }

    private static func isDescendant(_ child: URL, of parent: URL) -> Bool {
        let childComponents = child.standardizedFileURL.pathComponents
        let parentComponents = parent.standardizedFileURL.pathComponents
        return childComponents.count > parentComponents.count
            && Array(childComponents.prefix(parentComponents.count)) == parentComponents
    }
}
