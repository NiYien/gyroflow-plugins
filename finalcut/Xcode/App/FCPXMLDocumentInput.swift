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

    var errorDescription: String? {
        switch self {
        case .unsupportedSelection:
            return FinalCutStrings.text(
                "app.error.fcpxml.unsupported",
                fallback: "Choose one .fcpxml file or .fcpxmld package."
            )
        case .missingOrAmbiguousPackageInfo:
            return FinalCutStrings.text(
                "app.error.fcpxml.ambiguous_package",
                fallback: "The .fcpxmld package must contain exactly one root Info.fcpxml."
            )
        case .symbolicLink:
            return FinalCutStrings.text(
                "app.error.fcpxml.symbolic_link",
                fallback: "Symbolic-link FCPXML inputs are not accepted."
            )
        case .invalidFile:
            return FinalCutStrings.text(
                "app.error.fcpxml.invalid_file",
                fallback: "The selected FCPXML is not a regular readable file."
            )
        case .tooLarge:
            return FinalCutStrings.text(
                "app.error.fcpxml.too_large",
                fallback: "The selected FCPXML exceeds the supported size limit."
            )
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
