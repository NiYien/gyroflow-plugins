import Foundation

enum ReplacementProjectStoreError: LocalizedError {
    case invalidSource
    case sourceChanged
    case symbolicLink

    var errorDescription: String? {
        switch self {
        case .invalidSource:
            return FinalCutStrings.text(
                "app.error.replacement.invalid_source",
                fallback: "The selected Final Cut XML cannot be replaced safely."
            )
        case .sourceChanged:
            return FinalCutStrings.text(
                "app.error.replacement.source_changed",
                fallback: "The selected Final Cut XML changed during processing. Select it again."
            )
        case .symbolicLink:
            return FinalCutStrings.text(
                "app.error.replacement.symbolic_link",
                fallback: "Symbolic-link Final Cut XML inputs cannot be replaced."
            )
        }
    }
}

struct StagedSourceReplacement {
    let destination: URL
    let staging: URL
}

struct ReplacementProjectStore {
    let fileManager: FileManager

    init(fileManager: FileManager = .default) {
        self.fileManager = fileManager
    }

    func stage(
        _ data: Data,
        replacing source: ResolvedFCPXMLInput
    ) throws -> StagedSourceReplacement {
        let destination = try validateCurrentSource(source)
        let parent = destination.deletingLastPathComponent()
        let staging = parent.appendingPathComponent(
            ".gyroflow-staging-\(UUID().uuidString).fcpxml"
        )
        do {
            try data.write(to: staging, options: [.withoutOverwriting])
            let handle = try FileHandle(forWritingTo: staging)
            do {
                try handle.synchronize()
                try handle.close()
            } catch {
                try? handle.close()
                throw error
            }
        } catch {
            try? fileManager.removeItem(at: staging)
            throw error
        }
        return StagedSourceReplacement(
            destination: destination,
            staging: staging
        )
    }

    func commit(
        _ staged: StagedSourceReplacement,
        replacing source: ResolvedFCPXMLInput
    ) throws {
        let destination = try validateCurrentSource(source)
        guard destination == staged.destination else {
            throw ReplacementProjectStoreError.invalidSource
        }
        _ = try fileManager.replaceItemAt(
            destination,
            withItemAt: staged.staging,
            backupItemName: nil,
            options: []
        )
    }

    func discard(_ staged: StagedSourceReplacement) {
        try? fileManager.removeItem(at: staged.staging)
    }

    private func validateCurrentSource(
        _ source: ResolvedFCPXMLInput
    ) throws -> URL {
        let selection = source.selectionURL.standardizedFileURL
        let destination = source.xmlURL.standardizedFileURL
        switch selection.pathExtension.lowercased() {
        case "fcpxml":
            guard destination == selection else {
                throw ReplacementProjectStoreError.invalidSource
            }
        case "fcpxmld":
            let expected = selection
                .appendingPathComponent("Info.fcpxml", isDirectory: false)
                .standardizedFileURL
            guard destination == expected else {
                throw ReplacementProjectStoreError.invalidSource
            }
        default:
            throw ReplacementProjectStoreError.invalidSource
        }

        guard destination.resolvingSymlinksInPath() == destination else {
            throw ReplacementProjectStoreError.symbolicLink
        }
        let attributes = try fileManager.attributesOfItem(atPath: destination.path)
        guard attributes[.type] as? FileAttributeType == .typeRegular else {
            throw ReplacementProjectStoreError.invalidSource
        }
        let current = try Data(contentsOf: destination, options: [.mappedIfSafe])
        guard current == source.data else {
            throw ReplacementProjectStoreError.sourceChanged
        }
        return destination
    }
}
