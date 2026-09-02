import Foundation

struct ResolvedFCPXMLInput: Equatable {
    let selectionURL: URL
    let xmlURL: URL
    let data: Data
}

enum FCPXMLDocumentInputError: LocalizedError {
    case unsupportedSelection
    case missingOrAmbiguousPackageInfo
    case symbolicLink
    case invalidFile
    case tooLarge
    case outputMissingOrAmbiguous
    case outputDidNotStabilize

    var errorDescription: String? {
        switch self {
        case .unsupportedSelection:
            return "Choose one .fcpxml file or .fcpxmld package."
        case .missingOrAmbiguousPackageInfo:
            return "The .fcpxmld package must contain exactly one root Info.fcpxml."
        case .symbolicLink:
            return "Symbolic-link FCPXML inputs are not accepted."
        case .invalidFile:
            return "The selected FCPXML is not a regular readable file."
        case .tooLarge:
            return "The selected FCPXML exceeds the supported size limit."
        case .outputMissingOrAmbiguous:
            return "Final Cut did not create exactly one FCPXML output in the export directory."
        case .outputDidNotStabilize:
            return "Final Cut export did not finish writing before the timeout."
        }
    }
}

enum FCPXMLDocumentInput {
    static let maximumXMLBytes = 64 * 1024 * 1024

    static func resolve(
        _ selection: URL,
        fileManager: FileManager = .default
    ) throws -> ResolvedFCPXMLInput {
        let selection = selection.standardizedFileURL
        let extensionName = selection.pathExtension.lowercased()
        let xmlURL: URL
        if extensionName == "fcpxml" {
            xmlURL = selection
        } else if extensionName == "fcpxmld" {
            var isDirectory: ObjCBool = false
            guard fileManager.fileExists(
                atPath: selection.path,
                isDirectory: &isDirectory
            ), isDirectory.boolValue else {
                throw FCPXMLDocumentInputError.unsupportedSelection
            }
            let rootEntries = try fileManager.contentsOfDirectory(
                at: selection,
                includingPropertiesForKeys: nil,
                options: []
            )
            let candidates = rootEntries.filter {
                $0.lastPathComponent.lowercased() == "info.fcpxml"
            }
            guard candidates.count == 1,
                  candidates[0].lastPathComponent == "Info.fcpxml"
            else {
                throw FCPXMLDocumentInputError.missingOrAmbiguousPackageInfo
            }
            xmlURL = candidates[0].standardizedFileURL
        } else {
            throw FCPXMLDocumentInputError.unsupportedSelection
        }

        guard xmlURL.resolvingSymlinksInPath() == xmlURL else {
            throw FCPXMLDocumentInputError.symbolicLink
        }
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: xmlURL.path, isDirectory: &isDirectory),
              !isDirectory.boolValue
        else {
            throw FCPXMLDocumentInputError.invalidFile
        }
        let attributes = try fileManager.attributesOfItem(atPath: xmlURL.path)
        guard let size = attributes[.size] as? NSNumber,
              size.intValue > 0
        else {
            throw FCPXMLDocumentInputError.invalidFile
        }
        guard size.intValue <= maximumXMLBytes else {
            throw FCPXMLDocumentInputError.tooLarge
        }
        let data = try Data(contentsOf: xmlURL, options: [.mappedIfSafe])
        guard !data.isEmpty else {
            throw FCPXMLDocumentInputError.invalidFile
        }
        return ResolvedFCPXMLInput(
            selectionURL: selection,
            xmlURL: xmlURL,
            data: data
        )
    }
}

final class UniqueExportWorkspace {
    let directoryURL: URL

    private let fileManager: FileManager
    private let monotonicTime: () -> TimeInterval
    private let wait: (TimeInterval) -> Void

    init(
        directoryURL: URL,
        fileManager: FileManager = .default,
        monotonicTime: @escaping () -> TimeInterval = {
            ProcessInfo.processInfo.systemUptime
        },
        wait: @escaping (TimeInterval) -> Void = {
            Thread.sleep(forTimeInterval: $0)
        }
    ) {
        self.directoryURL = directoryURL.standardizedFileURL
        self.fileManager = fileManager
        self.monotonicTime = monotonicTime
        self.wait = wait
    }

    static func create(
        baseDirectory: URL = FileManager.default.temporaryDirectory,
        fileManager: FileManager = .default
    ) throws -> UniqueExportWorkspace {
        let directory = baseDirectory.standardizedFileURL.appendingPathComponent(
            "com.niyien.gyroflow.finalcut-route-d-\(UUID().uuidString)",
            isDirectory: true
        )
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: false
        )
        return UniqueExportWorkspace(directoryURL: directory, fileManager: fileManager)
    }

    func waitForStableInput(
        timeout: TimeInterval,
        pollInterval: TimeInterval = 0.1
    ) throws -> ResolvedFCPXMLInput {
        let deadline = monotonicTime() + timeout
        var previous: ResolvedFCPXMLInput?
        var sawUniqueCandidate = false
        while monotonicTime() <= deadline {
            let entries = try fileManager.contentsOfDirectory(
                at: directoryURL,
                includingPropertiesForKeys: nil,
                options: [.skipsHiddenFiles]
            )
            let candidates = entries.filter {
                ["fcpxml", "fcpxmld"].contains($0.pathExtension.lowercased())
            }
            if candidates.count == 1,
               let current = try? FCPXMLDocumentInput.resolve(
                   candidates[0],
                   fileManager: fileManager
               )
            {
                sawUniqueCandidate = true
                if previous == current {
                    return current
                }
                previous = current
            } else {
                previous = nil
            }
            wait(pollInterval)
        }
        if sawUniqueCandidate {
            throw FCPXMLDocumentInputError.outputDidNotStabilize
        }
        throw FCPXMLDocumentInputError.outputMissingOrAmbiguous
    }
}
