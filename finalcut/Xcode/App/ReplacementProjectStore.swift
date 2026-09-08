import Foundation

enum ReplacementProjectStoreError: LocalizedError {
    case sourceWouldBeModified
    case destinationExists
    case symbolicLink

    var errorDescription: String? {
        switch self {
        case .sourceWouldBeModified:
            return FinalCutStrings.text(
                "app.error.replacement.source_would_change",
                fallback: "The replacement project must be written separately from the selected input."
            )
        case .destinationExists:
            return FinalCutStrings.text(
                "app.error.replacement.destination_exists",
                fallback: "The replacement project destination already exists."
            )
        case .symbolicLink:
            return FinalCutStrings.text(
                "app.error.replacement.symbolic_link",
                fallback: "Symbolic-link replacement destinations are not accepted."
            )
        }
    }
}

struct ReplacementProjectStore {
    let fileManager: FileManager

    init(fileManager: FileManager = .default) {
        self.fileManager = fileManager
    }

    func write(
        _ data: Data,
        to requestedDestination: URL,
        source: ResolvedFCPXMLInput
    ) throws {
        let destination = requestedDestination.standardizedFileURL
        try validate(destination: destination, source: source)
        let parent = destination.deletingLastPathComponent()
        try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)
        let staging = parent.appendingPathComponent(
            ".gyroflow-staging-\(UUID().uuidString).tmp"
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

    private func validate(destination: URL, source: ResolvedFCPXMLInput) throws {
        let parent = destination.deletingLastPathComponent()
        guard parent.resolvingSymlinksInPath() == parent,
              destination.resolvingSymlinksInPath() == destination
        else {
            throw ReplacementProjectStoreError.symbolicLink
        }
        let selection = source.selectionURL.standardizedFileURL
        let xml = source.xmlURL.standardizedFileURL
        guard destination != selection, destination != xml else {
            throw ReplacementProjectStoreError.sourceWouldBeModified
        }
        if selection.pathExtension.lowercased() == "fcpxmld",
           isDescendant(destination, of: selection)
        {
            throw ReplacementProjectStoreError.sourceWouldBeModified
        }
        guard !fileManager.fileExists(atPath: destination.path) else {
            throw ReplacementProjectStoreError.destinationExists
        }
    }

    private func isDescendant(_ child: URL, of parent: URL) -> Bool {
        let childComponents = child.pathComponents
        let parentComponents = parent.pathComponents
        return childComponents.count > parentComponents.count
            && Array(childComponents.prefix(parentComponents.count)) == parentComponents
    }
}
