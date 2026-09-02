import Foundation

private func node(
    _ role: String,
    subrole: String? = nil,
    id: String? = nil,
    title: String? = nil,
    _ children: [FinalCutAXNode] = []
) -> FinalCutAXNode {
    FinalCutAXNode(
        role: role,
        subrole: subrole,
        identifier: id,
        title: title,
        children: children
    )
}

private final class FakeFinalCutAccessibilitySystem: FinalCutAccessibilitySystem {
    var trusted: Bool
    var hosts: [FinalCutRunningHost]
    var frontmostPIDs: [Int32?]
    var snapshots: [FinalCutAXNode]
    var promptRequests = 0
    var activatedPIDs: [Int32] = []
    var mutations: [FinalCutAXMutation] = []
    var time: TimeInterval = 0

    init(
        trusted: Bool,
        hosts: [FinalCutRunningHost],
        frontmostPIDs: [Int32?],
        snapshots: [FinalCutAXNode]
    ) {
        self.trusted = trusted
        self.hosts = hosts
        self.frontmostPIDs = frontmostPIDs
        self.snapshots = snapshots
    }

    func isTrusted(prompt: Bool) -> Bool {
        if prompt {
            promptRequests += 1
        }
        return trusted
    }

    func runningHosts() -> [FinalCutRunningHost] {
        hosts
    }

    func frontmostPID() -> Int32? {
        if frontmostPIDs.count > 1 {
            return frontmostPIDs.removeFirst()
        }
        return frontmostPIDs.first ?? nil
    }

    func activate(pid: Int32) throws -> Bool {
        activatedPIDs.append(pid)
        return false
    }

    func snapshot(pid: Int32) throws -> FinalCutAXNode {
        if snapshots.count > 1 {
            return snapshots.removeFirst()
        }
        guard let snapshot = snapshots.first else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous("snapshot")
        }
        return snapshot
    }

    func perform(_ mutation: FinalCutAXMutation) throws {
        mutations.append(mutation)
    }

    func monotonicTime() -> TimeInterval {
        time
    }

    func wait(interval: TimeInterval) {
        time += interval
    }
}

@main
struct FinalCutAccessibilityContractRunner {
    static func main() throws {
        var results: [String: Bool] = [:]
        let currentProject = node(
            "AXMenuButton",
            id: FinalCutAXIdentifiers.currentProject,
            title: "P0 Smooth2 Production Processed"
        )
        let savePanel = node(
            "AXWindow",
            subrole: "AXDialog",
            id: FinalCutAXIdentifiers.savePanel,
            title: nil,
            [
                node("AXTextField", id: FinalCutAXIdentifiers.saveName),
                node("AXButton", id: FinalCutAXIdentifiers.okButton),
            ]
        )
        let goToFolder = node(
            "AXSheet",
            id: FinalCutAXIdentifiers.goToFolder,
            title: nil,
            [
                node("AXTextField", id: FinalCutAXIdentifiers.path),
            ]
        )
        let englishMenu = node("AXMenuBar", id: nil, title: nil, [
            node("AXMenuBarItem", title: "File", [
                node("AXMenuItem", title: "Export XML…"),
            ]),
        ])
        let chineseMenu = node("AXMenuBar", id: nil, title: nil, [
            node("AXMenuBarItem", title: "文件", [
                node("AXMenuItem", title: "导出 XML…"),
            ]),
        ])

        results["creatorStudioAllowed"] = try FinalCutAXLocator.uniqueSupportedHost([
            FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 41),
        ]).pid == 41
        results["legacyAllowed"] = try FinalCutAXLocator.uniqueSupportedHost([
            FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCut", pid: 42),
        ]).pid == 42
        results["unsupportedRejected"] = (try? FinalCutAXLocator.uniqueSupportedHost([
            FinalCutRunningHost(bundleIdentifier: "com.example.FinalCut", pid: 43),
        ])) == nil
        results["duplicateRejected"] = (try? FinalCutAXLocator.uniqueSupportedHost([
            FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 44),
            FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCut", pid: 45),
        ])) == nil
        results["currentProjectExact"] =
            try FinalCutAXLocator.uniquePath(
                in: node("AXWindow", id: nil, title: nil, [currentProject]),
                role: "AXMenuButton",
                identifier: FinalCutAXIdentifiers.currentProject
            ) == [0]
        results["englishMenu"] = try FinalCutAXLocator.exportMenuPaths(in: englishMenu)
            == FinalCutMenuPaths(file: [0], exportXML: [0, 0])
        results["chineseMenu"] = try FinalCutAXLocator.exportMenuPaths(in: chineseMenu)
            == FinalCutMenuPaths(file: [0], exportXML: [0, 0])
        results["unknownLocaleRejected"] = (try? FinalCutAXLocator.exportMenuPaths(
            in: node("AXMenuBar", id: nil, title: nil, [
                node("AXMenuBarItem", title: "Fichier", [
                    node("AXMenuItem", title: "Exporter XML…"),
                ]),
            ])
        )) == nil
        results["savePanelExact"] = try FinalCutAXLocator.savePanelControls(
            in: node("AXApplication", id: nil, title: nil, [savePanel])
        ) == FinalCutSavePanelPaths(panel: [0], name: [0, 0], ok: [0, 1])
        results["goToFolderExact"] = try FinalCutAXLocator.goToFolderControls(
            in: node("AXApplication", id: nil, title: nil, [goToFolder])
        ) == FinalCutGoToFolderPaths(panel: [0], path: [0, 0])
        results["ambiguousControlRejected"] = (try? FinalCutAXLocator.savePanelControls(
            in: node("AXApplication", id: nil, title: nil, [
                savePanel,
                node(
                    "AXWindow",
                    subrole: "AXDialog",
                    id: FinalCutAXIdentifiers.savePanel
                ),
            ])
        )) == nil
        let standardAndDialogRoot = node("AXApplication", [
            node("AXWindow", subrole: "AXStandardWindow", id: "main-window"),
            savePanel,
        ])
        let dialogPaths = FinalCutAXLocator.dialogPaths(in: standardAndDialogRoot)
        results["identifiedStandardWindowIsNotADialog"] = dialogPaths.count == 1
            && dialogPaths[0].0 == [1]
            && dialogPaths[0].1 == FinalCutAXIdentifiers.savePanel

        var denied = FinalCutAutomationStateMachine()
        results["permissionDenied"] = (try? denied.begin(
            permissionGranted: false,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 46)]
        )) == nil && denied.state.isManualFallback && denied.mutationCount == 0

        var stateMachine = FinalCutAutomationStateMachine()
        let pid = try stateMachine.begin(
            permissionGranted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 47)]
        )
        try stateMachine.authorizeMutation(frontmostPID: pid)
        results["frontmostAccepted"] = stateMachine.mutationCount == 1
        results["pidChangeRejected"] = (try? stateMachine.authorizeMutation(
            frontmostPID: 99
        )) == nil && stateMachine.state.isManualFallback

        var dialogMachine = FinalCutAutomationStateMachine()
        _ = try dialogMachine.begin(
            permissionGranted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCut", pid: 48)]
        )
        results["saveDialogAccepted"] = (try? dialogMachine.validateDialog(
            identifier: FinalCutAXIdentifiers.savePanel
        )) != nil
        results["unknownDialogRejected"] = (try? dialogMachine.validateDialog(
            identifier: "replace-confirmation"
        )) == nil && dialogMachine.state.isManualFallback

        var timeoutMachine = FinalCutAutomationStateMachine()
        _ = try timeoutMachine.begin(
            permissionGranted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCut", pid: 49)]
        )
        results["timeoutRejected"] = (try? timeoutMachine.checkDeadline(
            now: 11,
            deadline: 10
        )) == nil && timeoutMachine.state.isManualFallback

        let appRoot = node("AXApplication", id: nil, title: nil, [
            node("AXWindow", id: nil, title: nil, [currentProject]),
            englishMenu,
        ])
        let panelRoot = node("AXApplication", id: nil, title: nil, [savePanel])
        let goToFolderRoot = node("AXApplication", id: nil, title: nil, [goToFolder])
        let deniedSystem = FakeFinalCutAccessibilitySystem(
            trusted: false,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 60)],
            frontmostPIDs: [60],
            snapshots: [appRoot]
        )
        let deniedDriver = FinalCutAccessibilityDriver(system: deniedSystem)
        results["driverPermissionDenialHasNoMutation"] = (try? deniedDriver.exportCurrentProject(
            to: URL(fileURLWithPath: "/tmp/export-denied", isDirectory: true),
            fileName: "Current.fcpxml",
            timeout: 1
        )) == nil && deniedSystem.promptRequests == 1 && deniedSystem.mutations.isEmpty

        let successSystem = FakeFinalCutAccessibilitySystem(
            trusted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 61)],
            frontmostPIDs: [61],
            snapshots: [appRoot, appRoot, panelRoot, goToFolderRoot, panelRoot]
        )
        let successDriver = FinalCutAccessibilityDriver(system: successSystem)
        let expected = try successDriver.exportCurrentProject(
            to: URL(fileURLWithPath: "/tmp/export-success", isDirectory: true),
            fileName: "Current.fcpxml",
            timeout: 1
        )
        let expectedMutations: [FinalCutAXMutation] = [
            .press([1, 0]),
            .press([1, 0, 0]),
            .showGoToFolder,
            .setValue([0, 0], "/tmp/export-success"),
            .confirmGoToFolder,
            .setValue([0, 0], "Current.fcpxml"),
            .press([0, 1]),
        ]
        results["driverStopsAfterExactSavePanel"] = expected.path == "/tmp/export-success/Current.fcpxml"
            && successSystem.activatedPIDs == [61]
            && successSystem.mutations == expectedMutations

        let activationSystem = FakeFinalCutAccessibilitySystem(
            trusted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 64)],
            frontmostPIDs: [999, 64],
            snapshots: [appRoot, appRoot, panelRoot, goToFolderRoot, panelRoot]
        )
        let activationDriver = FinalCutAccessibilityDriver(system: activationSystem)
        let activationOutput = try? activationDriver.exportCurrentProject(
            to: URL(fileURLWithPath: "/tmp/export-activation", isDirectory: true),
            fileName: "Current.fcpxml",
            timeout: 1
        )
        results["driverActivatesAllowlistedHostBeforeFirstMutation"] =
            activationOutput?.path == "/tmp/export-activation/Current.fcpxml"
            && activationSystem.activatedPIDs == [64]
            && activationSystem.mutations.count == 7
        results["driverUsesExactFrontmostCheckInsteadOfActivationReturnValue"] =
            activationOutput?.path == "/tmp/export-activation/Current.fcpxml"

        let changedPIDSystem = FakeFinalCutAccessibilitySystem(
            trusted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCutApp", pid: 62)],
            frontmostPIDs: [62, 62, 99],
            snapshots: [appRoot, appRoot]
        )
        let changedPIDDriver = FinalCutAccessibilityDriver(system: changedPIDSystem)
        results["driverRechecksPIDBeforeEveryMutation"] = (try? changedPIDDriver.exportCurrentProject(
            to: URL(fileURLWithPath: "/tmp/export-pid", isDirectory: true),
            fileName: "Current.fcpxml",
            timeout: 1
        )) == nil && changedPIDSystem.mutations == [.press([1, 0])]

        let unknownPanel = node("AXApplication", id: nil, title: nil, [
            node("AXWindow", subrole: "AXDialog", id: "replace-confirmation"),
        ])
        let unknownDialogSystem = FakeFinalCutAccessibilitySystem(
            trusted: true,
            hosts: [FinalCutRunningHost(bundleIdentifier: "com.apple.FinalCut", pid: 63)],
            frontmostPIDs: [63],
            snapshots: [appRoot, appRoot, unknownPanel]
        )
        let unknownDialogDriver = FinalCutAccessibilityDriver(system: unknownDialogSystem)
        results["driverCancelsUnknownDialog"] = (try? unknownDialogDriver.exportCurrentProject(
            to: URL(fileURLWithPath: "/tmp/export-dialog", isDirectory: true),
            fileName: "Current.fcpxml",
            timeout: 1
        )) == nil && unknownDialogSystem.mutations.last == .cancel([0])

        let data = try JSONSerialization.data(withJSONObject: results, options: [.sortedKeys])
        FileHandle.standardOutput.write(data)
    }
}
