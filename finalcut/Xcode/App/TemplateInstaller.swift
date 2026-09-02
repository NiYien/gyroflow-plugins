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

enum TemplateInstallerError: LocalizedError {
    case missingResource(String)
    case invalidResource(String)
    case notOwned
    case transaction(String)

    var errorDescription: String? {
        switch self {
        case let .missingResource(name):
            return "Bundled template resource is missing: \(name)"
        case let .invalidResource(message):
            return "Bundled template resource is invalid: \(message)"
        case .notOwned:
            return "Refusing to remove a template directory not owned by Gyroflow NiYien"
        case let .transaction(message):
            return "Template installation transaction failed: \(message)"
        }
    }
}

final class TemplateInstaller {
    static let markerName = ".gyroflow-install.json"
    static let templateName = "Gyroflow NiYien.moef"
    static let requiredPreviewNames = ["large.png", "small.png"]

    let sourceURL: URL
    let destinationURL: URL
    let templateVersion: String
    let effectBundleIdentifier: String
    let effectUUID: String

    private let fileManager: FileManager
    private let beforeCommit: (() throws -> Void)?
    private let afterCommit: (() throws -> Void)?

    init(
        sourceURL: URL,
        destinationURL: URL,
        templateVersion: String,
        effectBundleIdentifier: String,
        effectUUID: String,
        fileManager: FileManager = .default,
        beforeCommit: (() throws -> Void)? = nil,
        afterCommit: (() throws -> Void)? = nil
    ) {
        self.sourceURL = sourceURL.standardizedFileURL
        self.destinationURL = destinationURL.standardizedFileURL
        self.templateVersion = templateVersion
        self.effectBundleIdentifier = effectBundleIdentifier
        self.effectUUID = effectUUID
        self.fileManager = fileManager
        self.beforeCommit = beforeCommit
        self.afterCommit = afterCommit
    }

    static func production(bundle: Bundle = .main) throws -> TemplateInstaller {
        guard let resources = bundle.resourceURL else {
            throw TemplateInstallerError.missingResource("App Resources")
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
            throw TemplateInstallerError.transaction("Movies directory is unavailable")
        }
        let destination = movies
            .appendingPathComponent("Motion Templates.localized", isDirectory: true)
            .appendingPathComponent("Effects.localized", isDirectory: true)
            .appendingPathComponent("NiYien", isDirectory: true)
            .appendingPathComponent("Gyroflow", isDirectory: true)
        let version = bundle.object(
            forInfoDictionaryKey: "CFBundleShortVersionString"
        ) as? String ?? "unknown"
        return TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            templateVersion: version,
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: "ABAD71F5-23F5-46F6-AB08-C11603168AA4"
        )
    }

    func status() -> TemplateInstallStatus {
        guard fileManager.fileExists(atPath: destinationURL.path) else {
            return TemplateInstallStatus(
                state: .notInstalled,
                installedVersion: nil,
                message: "Final Cut template is not installed"
            )
        }
        do {
            let marker = try readMarker(at: destinationURL)
            let template = destinationURL.appendingPathComponent(Self.templateName)
            let templateHash = try sha256(template)
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
                  marker.templateSHA256 == templateHash
            else {
                return TemplateInstallStatus(
                    state: .repairRequired,
                    installedVersion: marker.templateVersion,
                    message: "Final Cut template is missing, damaged, or out of date"
                )
            }
            return TemplateInstallStatus(
                state: .installed,
                installedVersion: marker.templateVersion,
                message: "Final Cut template is installed"
            )
        } catch {
            return TemplateInstallStatus(
                state: .repairRequired,
                installedVersion: nil,
                message: "Final Cut template requires repair"
            )
        }
    }

    @discardableResult
    func installOrRepair() throws -> TemplateInstallStatus {
        try validateSource()
        let parent = destinationURL.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: parent,
            withIntermediateDirectories: true
        )
        let transactionID = UUID().uuidString
        let staging = parent.appendingPathComponent(
            ".Gyroflow.staging.\(transactionID)",
            isDirectory: true
        )
        let backup = parent.appendingPathComponent(
            ".Gyroflow.backup.\(transactionID)",
            isDirectory: true
        )
        var movedExisting = false
        var movedNew = false
        defer {
            try? fileManager.removeItem(at: staging)
            try? fileManager.removeItem(at: backup)
        }
        do {
            try fileManager.copyItem(at: sourceURL, to: staging)
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

            if fileManager.fileExists(atPath: destinationURL.path) {
                try fileManager.moveItem(at: destinationURL, to: backup)
                movedExisting = true
            }
            try beforeCommit?()
            try fileManager.moveItem(at: staging, to: destinationURL)
            movedNew = true
            try afterCommit?()
            let installed = status()
            guard installed.state == .installed else {
                throw TemplateInstallerError.transaction(installed.message)
            }
            if movedExisting {
                try fileManager.removeItem(at: backup)
                movedExisting = false
            }
            return installed
        } catch {
            if movedNew && fileManager.fileExists(atPath: destinationURL.path) {
                try? fileManager.removeItem(at: destinationURL)
                movedNew = false
            }
            if movedExisting && !fileManager.fileExists(atPath: destinationURL.path) {
                try? fileManager.moveItem(at: backup, to: destinationURL)
                movedExisting = false
            }
            throw TemplateInstallerError.transaction(error.localizedDescription)
        }
    }

    func remove() throws {
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
        try fileManager.removeItem(at: destinationURL)
    }

    private func validateSource() throws {
        var isDirectory = ObjCBool(false)
        guard fileManager.fileExists(
            atPath: sourceURL.path,
            isDirectory: &isDirectory
        ), isDirectory.boolValue else {
            throw TemplateInstallerError.missingResource("Gyroflow template directory")
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
            throw TemplateInstallerError.invalidResource("template identity or payload mapping")
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
        try data.write(to: markerURL(at: root), options: .atomic)
    }

    private func sha256(_ url: URL) throws -> String {
        let data = try Data(contentsOf: url)
        return SHA256.hash(data: data)
            .map { String(format: "%02x", $0) }
            .joined()
    }
}
