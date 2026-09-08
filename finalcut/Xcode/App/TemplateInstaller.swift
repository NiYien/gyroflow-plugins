import CryptoKit
import Foundation

enum TemplateInstallExitCode {
    static let success: Int32 = 0
    static let installFailed: Int32 = 20
    static let verificationFailed: Int32 = 21
}

enum TemplateInstallationState: String, Codable {
    case notInstalled
    case repairRequired
    case installed
}

struct TemplateInstallStatus: Codable {
    let state: TemplateInstallationState
    let installedVersion: String?
    let message: String
}

struct TemplateInstallMarker: Codable, Equatable {
    let schemaVersion: Int
    let templateVersion: String
    let effectBundleIdentifier: String
    let effectUUID: String
    let templateSHA256: String
}

enum TemplateMutationKind: String, CaseIterable, Hashable {
    case createDirectory
    case copy
    case write
    case backup
    case move
    case remove
}

struct TemplateMutation {
    let kind: TemplateMutationKind
    let targets: [URL]
}

enum TemplateInstallerError: LocalizedError {
    case missingResource(String)
    case invalidResource(String)
    case notOwned
    case unsafeDestination
    case symbolicLink
    case transaction(String)

    var errorDescription: String? {
        switch self {
        case let .missingResource(name):
            return String(format: FinalCutStrings.text(
                "app.error.template.missing_resource",
                fallback: "Bundled template resource is missing: %@"
            ), name)
        case let .invalidResource(message):
            return String(format: FinalCutStrings.text(
                "app.error.template.invalid_resource",
                fallback: "Bundled template resource is invalid: %@"
            ), message)
        case .notOwned:
            return FinalCutStrings.text(
                "app.error.template.not_owned",
                fallback: "Refusing to remove a template directory not owned by Gyroflow NiYien"
            )
        case .unsafeDestination:
            return FinalCutStrings.text(
                "app.error.template.unsafe_destination",
                fallback: "Template operations must stay inside the Gyroflow Motion Templates directory"
            )
        case .symbolicLink:
            return FinalCutStrings.text(
                "app.error.template.symbolic_link",
                fallback: "Template operations do not accept symbolic-link destinations"
            )
        case let .transaction(message):
            return String(format: FinalCutStrings.text(
                "app.error.template.transaction",
                fallback: "Template installation transaction failed: %@"
            ), message)
        }
    }
}

final class TemplateInstaller {
    static let markerName = ".gyroflow-install.json"
    static let templateName = "Gyroflow NiYien.moef"
    static let requiredPreviewNames = ["large.png", "small.png"]

    let sourceURL: URL
    let destinationURL: URL
    let productRootURL: URL
    let templateVersion: String
    let effectBundleIdentifier: String
    let effectUUID: String

    private let fileManager: FileManager
    private let beforeCommit: (() throws -> Void)?
    private let afterCommit: (() throws -> Void)?
    private let mutationRecorder: ((TemplateMutation) -> Void)?

    init(
        sourceURL: URL,
        destinationURL: URL,
        productRootURL: URL,
        templateVersion: String,
        effectBundleIdentifier: String,
        effectUUID: String,
        fileManager: FileManager = .default,
        beforeCommit: (() throws -> Void)? = nil,
        afterCommit: (() throws -> Void)? = nil,
        mutationRecorder: ((TemplateMutation) -> Void)? = nil
    ) {
        self.sourceURL = sourceURL.standardizedFileURL
        self.destinationURL = destinationURL.standardizedFileURL
        self.productRootURL = productRootURL.standardizedFileURL
        self.templateVersion = templateVersion
        self.effectBundleIdentifier = effectBundleIdentifier
        self.effectUUID = effectUUID
        self.fileManager = fileManager
        self.beforeCommit = beforeCommit
        self.afterCommit = afterCommit
        self.mutationRecorder = mutationRecorder
    }

    static func production(bundle: Bundle = .main) throws -> TemplateInstaller {
        guard let resources = bundle.resourceURL else {
            throw TemplateInstallerError.missingResource(FinalCutStrings.text(
                "app.resource.app_resources",
                fallback: "App Resources",
                bundle: bundle
            ))
        }
        let source = resources
            .appendingPathComponent("Motion Templates", isDirectory: true)
            .appendingPathComponent("Effects.localized", isDirectory: true)
            .appendingPathComponent("NiYien", isDirectory: true)
            .appendingPathComponent("Gyroflow", isDirectory: true)
        guard let movies = FileManager.default.urls(
            for: .moviesDirectory,
            in: .userDomainMask
        ).first else {
            throw TemplateInstallerError.transaction(FinalCutStrings.text(
                "app.error.template.movies_unavailable",
                fallback: "Movies directory is unavailable",
                bundle: bundle
            ))
        }
        // Resolve the system Movies entry, but keep descendant symlinks guarded.
        let productRoot = movies.resolvingSymlinksInPath()
            .appendingPathComponent("Motion Templates.localized", isDirectory: true)
            .appendingPathComponent("Effects.localized", isDirectory: true)
            .appendingPathComponent("NiYien", isDirectory: true)
            .appendingPathComponent("Gyroflow", isDirectory: true)
        let destination = productRoot
        let version = bundle.object(
            forInfoDictionaryKey: "CFBundleShortVersionString"
        ) as? String ?? "unknown"
        return TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            productRootURL: productRoot,
            templateVersion: version,
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: "ABAD71F5-23F5-46F6-AB08-C11603168AA4"
        )
    }

    func status() -> TemplateInstallStatus {
        do {
            try validateOperationTargets([destinationURL])
        } catch {
            return TemplateInstallStatus(
                state: .repairRequired,
                installedVersion: nil,
                message: error.localizedDescription
            )
        }
        guard fileManager.fileExists(atPath: destinationURL.path) else {
            return TemplateInstallStatus(
                state: .notInstalled,
                installedVersion: nil,
                message: FinalCutStrings.text(
                    "app.status.template.not_installed",
                    fallback: "Final Cut template is not installed"
                )
            )
        }
        do {
            try validateOperationTargets([
                markerURL(at: destinationURL),
                destinationURL.appendingPathComponent(Self.templateName),
            ] + Self.requiredPreviewNames.map {
                destinationURL.appendingPathComponent($0)
            })
            let marker = try readMarker(at: destinationURL)
            let template = destinationURL.appendingPathComponent(Self.templateName)
            let templateHash = try sha256(template)
            let bundledTemplate = sourceURL.appendingPathComponent(Self.templateName)
            try validateSource()
            let bundledTemplateHash = try sha256(bundledTemplate)
            let previewsPresent = Self.requiredPreviewNames.allSatisfy { name in
                fileManager.fileExists(
                    atPath: destinationURL.appendingPathComponent(name).path
                )
            }
            guard fileManager.fileExists(atPath: template.path),
                  previewsPresent,
                  marker.schemaVersion == 1,
                  marker.templateVersion == templateVersion,
                  marker.effectBundleIdentifier == effectBundleIdentifier,
                  marker.effectUUID == effectUUID,
                  marker.templateSHA256 == templateHash,
                  templateHash == bundledTemplateHash
            else {
                return TemplateInstallStatus(
                    state: .repairRequired,
                    installedVersion: marker.templateVersion,
                    message: FinalCutStrings.text(
                        "app.status.template.invalid",
                        fallback: "Final Cut template is missing, damaged, or out of date"
                    )
                )
            }
            return TemplateInstallStatus(
                state: .installed,
                installedVersion: marker.templateVersion,
                message: FinalCutStrings.text(
                    "app.status.template.installed",
                    fallback: "Final Cut template is installed"
                )
            )
        } catch {
            return TemplateInstallStatus(
                state: .repairRequired,
                installedVersion: nil,
                message: FinalCutStrings.text(
                    "app.status.template.repair",
                    fallback: "Final Cut template requires repair"
                )
            )
        }
    }

    @discardableResult
    func installOrRepair() throws -> TemplateInstallStatus {
        try validateOperationTargets([productRootURL, destinationURL])
        try validateSource()
        let rootExisted = fileManager.fileExists(atPath: productRootURL.path)
        try createDirectory(at: productRootURL)
        let transactionID = UUID().uuidString
        let transaction = productRootURL.appendingPathComponent(
            ".transaction.\(transactionID)",
            isDirectory: true
        )
        let staging = transaction.appendingPathComponent(
            "staging",
            isDirectory: true
        )
        let backup = transaction.appendingPathComponent("backup", isDirectory: true)
        try validateOperationTargets([
            productRootURL,
            destinationURL,
            transaction,
            staging,
            backup,
        ])
        try createDirectory(at: transaction)
        var backedUpItems: [(backup: URL, original: URL)] = []
        var installedItems: [URL] = []
        var completed = false
        defer {
            if fileManager.fileExists(atPath: transaction.path) {
                try? removeItem(at: transaction)
            }
            if !completed,
               !rootExisted,
               (try? fileManager.contentsOfDirectory(
                    at: productRootURL,
                    includingPropertiesForKeys: nil
               ).isEmpty) == true
            {
                try? removeItem(at: productRootURL)
            }
        }
        do {
            try copyItem(at: sourceURL, to: staging)
            let template = staging.appendingPathComponent(Self.templateName)
            let marker = TemplateInstallMarker(
                schemaVersion: 1,
                templateVersion: templateVersion,
                effectBundleIdentifier: effectBundleIdentifier,
                effectUUID: effectUUID,
                templateSHA256: try sha256(template)
            )
            try writeMarker(marker, at: staging)
            _ = try readMarker(at: staging)

            try createDirectory(at: backup)
            for item in try fileManager.contentsOfDirectory(
                at: productRootURL,
                includingPropertiesForKeys: nil
            ) where item.standardizedFileURL != transaction.standardizedFileURL {
                let backedUp = backup.appendingPathComponent(
                    item.lastPathComponent,
                    isDirectory: item.hasDirectoryPath
                )
                try moveItem(at: item, to: backedUp, kind: .backup)
                backedUpItems.append((backup: backedUp, original: item))
            }
            try beforeCommit?()
            for item in try fileManager.contentsOfDirectory(
                at: staging,
                includingPropertiesForKeys: nil
            ) {
                let installed = productRootURL.appendingPathComponent(
                    item.lastPathComponent,
                    isDirectory: item.hasDirectoryPath
                )
                try moveItem(at: item, to: installed)
                installedItems.append(installed)
            }
            try afterCommit?()
            let installed = status()
            guard installed.state == .installed else {
                throw TemplateInstallerError.transaction(installed.message)
            }
            completed = true
            return installed
        } catch {
            for installed in installedItems.reversed() {
                try? removeItem(at: installed)
            }
            for pair in backedUpItems.reversed() {
                try? moveItem(at: pair.backup, to: pair.original)
            }
            throw TemplateInstallerError.transaction(error.localizedDescription)
        }
    }

    func prepareIfNeeded() throws -> TemplateInstallStatus {
        let current = status()
        if current.state == .installed {
            return current
        }
        try validateOperationTargets([destinationURL, markerURL(at: destinationURL)])
        if fileManager.fileExists(atPath: destinationURL.path) {
            // Automatic preparation only replaces an installation with a matching owner marker.
            guard let marker = try? readMarker(at: destinationURL),
                  marker.effectBundleIdentifier == effectBundleIdentifier,
                  marker.effectUUID == effectUUID else {
                throw TemplateInstallerError.notOwned
            }
        }
        return try installOrRepair()
    }

    func preflightInstallOrRepair() throws {
        try validateOperationTargets([productRootURL, destinationURL])
        try validateSource()
        let parent = productRootURL.deletingLastPathComponent()
        try validateOperationTargets([productRootURL])
        if fileManager.fileExists(atPath: parent.path),
           !fileManager.isWritableFile(atPath: parent.path) {
            throw TemplateInstallerError.transaction(
                FinalCutStrings.text(
                    "app.error.template.destination_not_writable",
                    fallback: "The Motion Templates destination is not writable"
                )
            )
        }
    }

    func remove() throws {
        try validateOperationTargets([destinationURL])
        guard fileManager.fileExists(atPath: destinationURL.path) else {
            return
        }
        let template = destinationURL.appendingPathComponent(Self.templateName)
        let markerOwned = (try? readMarker(at: destinationURL)).map {
            $0.effectBundleIdentifier == effectBundleIdentifier &&
                $0.effectUUID == effectUUID
        } ?? false
        let templateOwned = (try? String(contentsOf: template, encoding: .utf8))
            .map { $0.contains(effectUUID) } ?? false
        guard markerOwned || templateOwned else {
            throw TemplateInstallerError.notOwned
        }
        try removeItem(at: destinationURL)
    }

    private func validateOperationTargets(_ targets: [URL]) throws {
        let root = productRootURL.standardizedFileURL
        let canonicalRoot = root.resolvingSymlinksInPath()
        guard canonicalRoot.path == root.path else {
            throw TemplateInstallerError.symbolicLink
        }
        let destination = destinationURL.standardizedFileURL
        let canonicalDestination = destination.resolvingSymlinksInPath()
        guard canonicalDestination.path == destination.path else {
            throw TemplateInstallerError.symbolicLink
        }
        guard canonicalDestination.pathComponents == canonicalRoot.pathComponents else {
            throw TemplateInstallerError.unsafeDestination
        }
        for target in targets {
            let standardized = target.standardizedFileURL
            let canonical = standardized.resolvingSymlinksInPath()
            guard canonical.path == standardized.path else {
                throw TemplateInstallerError.symbolicLink
            }
            guard isAtOrBelow(canonical, root: canonicalRoot) else {
                throw TemplateInstallerError.unsafeDestination
            }
        }
    }

    private func isAtOrBelow(_ candidate: URL, root: URL) -> Bool {
        let candidateComponents = candidate.pathComponents
        let rootComponents = root.pathComponents
        return candidateComponents.count >= rootComponents.count
            && Array(candidateComponents.prefix(rootComponents.count)) == rootComponents
    }

    private func createDirectory(at url: URL) throws {
        try validateOperationTargets([url])
        record(.createDirectory, targets: [url])
        try fileManager.createDirectory(at: url, withIntermediateDirectories: true)
    }

    private func copyItem(at source: URL, to destination: URL) throws {
        try validateOperationTargets([destination])
        record(.copy, targets: [destination])
        try fileManager.copyItem(at: source, to: destination)
    }

    private func writeData(_ data: Data, to destination: URL) throws {
        try validateOperationTargets([destination])
        record(.write, targets: [destination])
        try data.write(to: destination)
    }

    private func moveItem(
        at source: URL,
        to destination: URL,
        kind: TemplateMutationKind = .move
    ) throws {
        try validateOperationTargets([source, destination])
        record(kind, targets: [source, destination])
        try fileManager.moveItem(at: source, to: destination)
    }

    private func removeItem(at target: URL) throws {
        try validateOperationTargets([target])
        record(.remove, targets: [target])
        try fileManager.removeItem(at: target)
    }

    private func record(_ kind: TemplateMutationKind, targets: [URL]) {
        mutationRecorder?(TemplateMutation(kind: kind, targets: targets))
    }

    private func validateSource() throws {
        var isDirectory = ObjCBool(false)
        guard fileManager.fileExists(
            atPath: sourceURL.path,
            isDirectory: &isDirectory
        ), isDirectory.boolValue else {
            throw TemplateInstallerError.missingResource(FinalCutStrings.text(
                "app.resource.gyroflow_template",
                fallback: "Gyroflow template directory"
            ))
        }
        let template = sourceURL.appendingPathComponent(Self.templateName)
        guard fileManager.fileExists(atPath: template.path) else {
            throw TemplateInstallerError.missingResource(Self.templateName)
        }
        let text = try String(contentsOf: template, encoding: .utf8)
        guard text.contains(effectUUID),
              !text.contains("Gyroflow Toolbox"),
              text.contains("Project Payload"),
              text.contains("Timing Payload")
        else {
            throw TemplateInstallerError.invalidResource(FinalCutStrings.text(
                "app.error.template.identity",
                fallback: "template identity or payload mapping"
            ))
        }
        for previewName in Self.requiredPreviewNames {
            let preview = sourceURL.appendingPathComponent(previewName)
            guard fileManager.fileExists(atPath: preview.path) else {
                throw TemplateInstallerError.missingResource(previewName)
            }
        }
    }

    private func markerURL(at root: URL) -> URL {
        root.appendingPathComponent(Self.markerName)
    }

    private func readMarker(at root: URL) throws -> TemplateInstallMarker {
        let data = try Data(contentsOf: markerURL(at: root))
        return try JSONDecoder().decode(TemplateInstallMarker.self, from: data)
    }

    private func writeMarker(_ marker: TemplateInstallMarker, at root: URL) throws {
        let data = try JSONEncoder().encode(marker)
        try writeData(data, to: markerURL(at: root))
    }

    private func sha256(_ url: URL) throws -> String {
        let data = try Data(contentsOf: url)
        return SHA256.hash(data: data)
            .map { String(format: "%02x", $0) }
            .joined()
    }
}
