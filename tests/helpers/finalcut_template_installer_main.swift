import Foundation

enum InjectedFailure: Error {
    case commit
}

private final class MutationLog {
    var mutations: [TemplateMutation] = []

    func record(_ mutation: TemplateMutation) {
        mutations.append(mutation)
    }
}

@main
struct TemplateInstallerTestMain {
    private static func recursiveContents(
        of directory: URL,
        fileManager: FileManager = .default
    ) -> [URL] {
        guard let enumerator = fileManager.enumerator(
            at: directory,
            includingPropertiesForKeys: nil
        ) else {
            return []
        }
        return enumerator.compactMap { $0 as? URL }
    }

    private static func isAtOrBelow(_ candidate: URL, root: URL) -> Bool {
        let candidate = candidate.standardizedFileURL.resolvingSymlinksInPath()
        let root = root.standardizedFileURL.resolvingSymlinksInPath()
        let candidateComponents = candidate.pathComponents
        let rootComponents = root.pathComponents
        return candidateComponents.count >= rootComponents.count
            && Array(candidateComponents.prefix(rootComponents.count)) == rootComponents
    }

    static func main() throws {
        let root = URL(
            fileURLWithPath: CommandLine.arguments[1],
            isDirectory: true
        ).resolvingSymlinksInPath()
        let source = root.appendingPathComponent("source", isDirectory: true)
        let destination = root.appendingPathComponent(
            "Movies/Motion Templates.localized/Effects.localized/NiYien/Gyroflow",
            isDirectory: true
        )
        let vendorRoot = destination.deletingLastPathComponent()
        let productRoot = destination
        let mutationLog = MutationLog()
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
            productRootURL: productRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            mutationRecorder: mutationLog.record
        )
        try installer.preflightInstallOrRepair()
        let preflightPassedWithoutMutation = mutationLog.mutations.isEmpty
        let before = installer.status()
        let installed = try installer.prepareIfNeeded()
        let mutationsAfterInstall = mutationLog.mutations.count
        _ = try installer.prepareIfNeeded()
        let automaticPreparationIsIdempotent = mutationLog.mutations.count == mutationsAfterInstall
        let sourceTemplateText = try String(contentsOf: template, encoding: .utf8)
        try (sourceTemplateText + "\n").write(
            to: template,
            atomically: true,
            encoding: .utf8
        )
        let bundledDrift = installer.status()
        let bundledDriftRepaired = try installer.prepareIfNeeded()
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
        var transactionSnapshot: [URL] = []
        let containmentProbe = TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            productRootURL: productRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            beforeCommit: {
                transactionSnapshot = recursiveContents(of: vendorRoot)
            },
            mutationRecorder: mutationLog.record
        )
        _ = try containmentProbe.installOrRepair()
        let transactionSnapshotContained = transactionSnapshot.allSatisfy {
            isAtOrBelow($0, root: destination)
        }
        let preserved = try Data(
            contentsOf: destination.appendingPathComponent(TemplateInstaller.templateName)
        )
        let failing = TemplateInstaller(
            sourceURL: source,
            destinationURL: destination,
            productRootURL: productRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            beforeCommit: { throw InjectedFailure.commit },
            mutationRecorder: mutationLog.record
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
            productRootURL: productRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            afterCommit: { throw InjectedFailure.commit },
            mutationRecorder: mutationLog.record
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
            productRootURL: productRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            mutationRecorder: mutationLog.record
        )
        var foreignRejected = false
        do {
            try foreignInstaller.remove()
        } catch {
            foreignRejected = true
        }
        let automaticForeign = TemplateInstaller(
            sourceURL: source, destinationURL: foreign, productRootURL: foreign,
            templateVersion: "2.1.2", effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid
        )
        var automaticForeignRejected = false
        do {
            _ = try automaticForeign.prepareIfNeeded()
        } catch {
            automaticForeignRejected = true
        }

        let siblingDestination = vendorRoot.appendingPathComponent(
            "Other",
            isDirectory: true
        )
        let prefixDestination = URL(
            fileURLWithPath: productRoot.path + "-Escape",
            isDirectory: true
        )
        let escapedDestinations = [
            siblingDestination,
            prefixDestination,
        ]
        var escapedInstallationsRejected = true
        for escapedDestination in escapedDestinations {
            let escapedInstaller = TemplateInstaller(
                sourceURL: source,
                destinationURL: escapedDestination,
                productRootURL: productRoot,
                templateVersion: "2.1.2",
                effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
                effectUUID: uuid,
                mutationRecorder: mutationLog.record
            )
            do {
                try escapedInstaller.installOrRepair()
                escapedInstallationsRejected = false
            } catch {
                // Rejection is the expected containment behavior.
            }
        }
        let escapedInstallsCreatedNothing = escapedDestinations.allSatisfy {
            !FileManager.default.fileExists(atPath: $0.path)
        }
        var escapedRepairsRejected = true
        var escapedRemovalsRejected = true
        var escapedTargetsPreserved = true
        for escapedDestination in escapedDestinations {
            try FileManager.default.createDirectory(
                at: escapedDestination,
                withIntermediateDirectories: true
            )
            let escapedTemplate = escapedDestination.appendingPathComponent(
                TemplateInstaller.templateName
            )
            try "<filter pluginUUID=\"\(uuid)\"/>".write(
                to: escapedTemplate,
                atomically: true,
                encoding: .utf8
            )
            let escapedInstaller = TemplateInstaller(
                sourceURL: source,
                destinationURL: escapedDestination,
                productRootURL: productRoot,
                templateVersion: "2.1.2",
                effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
                effectUUID: uuid,
                mutationRecorder: mutationLog.record
            )
            do {
                try escapedInstaller.installOrRepair()
                escapedRepairsRejected = false
            } catch {
                // Rejection is the expected containment behavior.
            }
            do {
                try escapedInstaller.remove()
                escapedRemovalsRejected = false
            } catch {
                // Rejection is the expected containment behavior.
            }
            escapedTargetsPreserved = escapedTargetsPreserved
                && FileManager.default.fileExists(atPath: escapedTemplate.path)
        }

        let exactSymlinkParent = root.appendingPathComponent(
            "Exact Symlink/Movies/Motion Templates.localized/Effects.localized/NiYien",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: exactSymlinkParent,
            withIntermediateDirectories: true
        )
        let exactSymlinkTarget = root.appendingPathComponent(
            "Exact Symlink Target",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: exactSymlinkTarget,
            withIntermediateDirectories: true
        )
        let exactSymlinkRoot = exactSymlinkParent.appendingPathComponent(
            "Gyroflow",
            isDirectory: true
        )
        try FileManager.default.createSymbolicLink(
            at: exactSymlinkRoot,
            withDestinationURL: exactSymlinkTarget
        )
        let exactSymlinkInstaller = TemplateInstaller(
            sourceURL: source,
            destinationURL: exactSymlinkRoot,
            productRootURL: exactSymlinkRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            mutationRecorder: mutationLog.record
        )
        var exactDestinationSymlinkRejectedByGuard = false
        do {
            try exactSymlinkInstaller.installOrRepair()
        } catch TemplateInstallerError.symbolicLink {
            exactDestinationSymlinkRejectedByGuard = true
        }

        let realMovies = root.appendingPathComponent("Real Movies", isDirectory: true)
        let linkedMovies = root.appendingPathComponent("Linked Movies", isDirectory: true)
        try FileManager.default.createDirectory(at: realMovies, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(
            at: linkedMovies,
            withDestinationURL: realMovies
        )
        let realAncestorProductRoot = realMovies.appendingPathComponent(
            "Motion Templates.localized/Effects.localized/NiYien/Gyroflow",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: realAncestorProductRoot,
            withIntermediateDirectories: true
        )
        let linkedAncestorProductRoot = linkedMovies.appendingPathComponent(
            "Motion Templates.localized/Effects.localized/NiYien/Gyroflow",
            isDirectory: true
        )
        let ancestorSymlinkInstaller = TemplateInstaller(
            sourceURL: source,
            destinationURL: linkedAncestorProductRoot,
            productRootURL: linkedAncestorProductRoot,
            templateVersion: "2.1.2",
            effectBundleIdentifier: "com.niyien.gyroflow.finalcut.effect",
            effectUUID: uuid,
            mutationRecorder: mutationLog.record
        )
        var ancestorSymlinkRejectedByGuard = false
        do {
            try ancestorSymlinkInstaller.installOrRepair()
        } catch TemplateInstallerError.symbolicLink {
            ancestorSymlinkRejectedByGuard = true
        }

        let mutationTargets = mutationLog.mutations.flatMap(\.targets)
        let mutationTargetsCanonicalAndContained = mutationTargets.allSatisfy {
            $0.standardizedFileURL.resolvingSymlinksInPath() == $0.standardizedFileURL
                && isAtOrBelow($0, root: productRoot)
        }
        let mutationKinds = Set(mutationLog.mutations.map(\.kind))
        let allMutationKindsObserved = mutationKinds == Set(TemplateMutationKind.allCases)
        let result: [String: Any] = [
            "before": before.state.rawValue,
            "preflightPassedWithoutMutation": preflightPassedWithoutMutation,
            "automaticPreparationIsIdempotent": automaticPreparationIsIdempotent,
            "automaticForeignRejected": automaticForeignRejected,
            "installed": installed.state.rawValue,
            "bundledDrift": bundledDrift.state.rawValue,
            "bundledDriftRepaired": bundledDriftRepaired.state.rawValue,
            "damaged": damaged.state.rawValue,
            "repaired": repaired.state.rawValue,
            "previewDamaged": previewDamaged.state.rawValue,
            "transactionSnapshotContained": transactionSnapshotContained,
            "rollbackFailed": rollbackFailed,
            "rollbackPreserved": preserved == afterRollback,
            "afterCommitRollbackFailed": afterCommitRollbackFailed,
            "afterCommitRollbackPreserved": preserved == afterCommitRollback,
            "removed": removed,
            "foreignRejected": foreignRejected,
            "foreignPreserved": FileManager.default.fileExists(atPath: foreign.path),
            "escapedInstallationsRejected": escapedInstallationsRejected,
            "escapedInstallsCreatedNothing": escapedInstallsCreatedNothing,
            "escapedRepairsRejected": escapedRepairsRejected,
            "escapedRemovalsRejected": escapedRemovalsRejected,
            "escapedTargetsPreserved": escapedTargetsPreserved,
            "exactDestinationSymlinkRejectedByGuard": exactDestinationSymlinkRejectedByGuard,
            "ancestorSymlinkRejectedByGuard": ancestorSymlinkRejectedByGuard,
            "mutationTargetsCanonicalAndContained": mutationTargetsCanonicalAndContained,
            "allMutationKindsObserved": allMutationKindsObserved,
        ]
        let data = try JSONSerialization.data(
            withJSONObject: result,
            options: [.sortedKeys]
        )
        print(String(decoding: data, as: UTF8.self))
    }
}
