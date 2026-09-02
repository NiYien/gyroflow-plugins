import Foundation

enum InjectedFailure: Error {
    case commit
}

@main
struct TemplateInstallerTestMain {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let source = root.appendingPathComponent("source", isDirectory: true)
        let destination = root.appendingPathComponent(
            "Movies/Motion Templates.localized/Effects.localized/NiYien/Gyroflow",
            isDirectory: true
        )
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
        let uuid = "ABAD71F5-23F5-46F6-AB08-C11603168AA4"
        let template = source.appendingPathComponent(TemplateInstaller.templateName)
        try "<filter pluginUUID=\"\(uuid)\"><parameter name=\"Project Payload\"/><parameter name=\"Timing Payload\"/></filter>"
            .write(to: template, atomically: true, encoding: .utf8)
        try Data([1, 2, 3]).write(to: source.appendingPathComponent("large.png"))
        try Data([4, 5, 6]).write(to: source.appendingPathComponent("small.png"))

        let installer = TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid
        )
        let before = installer.status()
        let installed = try installer.installOrRepair()
        try "damaged".write(
            to: destination.appendingPathComponent(TemplateInstaller.templateName),
            atomically: true,
            encoding: .utf8
        )
        let damaged = installer.status()
        let repaired = try installer.installOrRepair()
        try FileManager.default.removeItem(
            at: destination.appendingPathComponent("small.png")
        )
        let previewDamaged = installer.status()
        _ = try installer.installOrRepair()
        let preserved = try Data(
            contentsOf: destination.appendingPathComponent(TemplateInstaller.templateName)
        )
        let failing = TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            beforeCommit: { throw InjectedFailure.commit }
        )
        var rollbackFailed = false
        do {
            try failing.installOrRepair()
        } catch {
            rollbackFailed = true
        }
        let afterRollback = try Data(
            contentsOf: destination.appendingPathComponent(TemplateInstaller.templateName)
        )
        let afterCommitFailure = TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            afterCommit: { throw InjectedFailure.commit }
        )
        var afterCommitRollbackFailed = false
        do {
            try afterCommitFailure.installOrRepair()
        } catch {
            afterCommitRollbackFailed = true
        }
        let afterCommitRollback = try Data(
            contentsOf: destination.appendingPathComponent(TemplateInstaller.templateName)
        )
        try installer.remove()
        let removed = !FileManager.default.fileExists(atPath: destination.path)

        let foreign = root.appendingPathComponent("foreign/Gyroflow", isDirectory: true)
        try FileManager.default.createDirectory(at: foreign, withIntermediateDirectories: true)
        try "foreign".write(
            to: foreign.appendingPathComponent(TemplateInstaller.templateName),
            atomically: true,
            encoding: .utf8
        )
        let foreignInstaller = TemplateInstaller(
            sourceURL: source,
            destinationURL: foreign,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid
        )
        var foreignRejected = false
        do {
            try foreignInstaller.remove()
        } catch {
            foreignRejected = true
        }
        let result: [String: Any] = [
            "before": before.state.rawValue,
            "installed": installed.state.rawValue,
            "damaged": damaged.state.rawValue,
            "repaired": repaired.state.rawValue,
            "previewDamaged": previewDamaged.state.rawValue,
            "rollbackFailed": rollbackFailed,
            "rollbackPreserved": preserved == afterRollback,
            "afterCommitRollbackFailed": afterCommitRollbackFailed,
            "afterCommitRollbackPreserved": preserved == afterCommitRollback,
            "removed": removed,
            "foreignRejected": foreignRejected,
            "foreignPreserved": FileManager.default.fileExists(atPath: foreign.path),
        ]
        let data = try JSONSerialization.data(
            withJSONObject: result,
            options: [.sortedKeys]
        )
        print(String(decoding: data, as: UTF8.self))
    }
}
