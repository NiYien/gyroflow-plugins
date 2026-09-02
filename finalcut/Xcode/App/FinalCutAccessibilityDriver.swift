import AppKit
import ApplicationServices
import Foundation

enum FinalCutAXMutation: Equatable {
    case press([Int])
    case setValue([Int], String)
    case cancel([Int])
    case showGoToFolder
    case confirmGoToFolder
}

protocol FinalCutAccessibilitySystem: AnyObject {
    func isTrusted(prompt: Bool) -> Bool
    func runningHosts() -> [FinalCutRunningHost]
    func frontmostPID() -> Int32?
    func activate(pid: Int32) throws -> Bool
    func snapshot(pid: Int32) throws -> FinalCutAXNode
    func perform(_ mutation: FinalCutAXMutation) throws
    func monotonicTime() -> TimeInterval
    func wait(interval: TimeInterval)
}

final class SystemFinalCutAccessibilitySystem: FinalCutAccessibilitySystem {
    private var elementsByPath: [[Int]: AXUIElement] = [:]
    private var visitedNodes = 0

    func isTrusted(prompt: Bool) -> Bool {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        return AXIsProcessTrustedWithOptions([key: prompt] as CFDictionary)
    }

    func runningHosts() -> [FinalCutRunningHost] {
        NSWorkspace.shared.runningApplications.compactMap { application in
            guard let identifier = application.bundleIdentifier else {
                return nil
            }
            return FinalCutRunningHost(
                bundleIdentifier: identifier,
                pid: application.processIdentifier
            )
        }
    }

    func frontmostPID() -> Int32? {
        NSWorkspace.shared.frontmostApplication?.processIdentifier
    }

    func activate(pid: Int32) throws -> Bool {
        guard let application = NSRunningApplication(processIdentifier: pid),
              application.bundleIdentifier.map(
                  FinalCutAXLocator.supportedBundleIdentifiers.contains
              ) == true
        else {
            throw FinalCutAutomationError.hostUnavailable(
                "Unable to activate the selected supported Final Cut process."
            )
        }
        let requestActivation = {
            application.activate(options: [.activateAllWindows])
        }
        if Thread.isMainThread {
            return requestActivation()
        }
        return DispatchQueue.main.sync(execute: requestActivation)
    }

    func snapshot(pid: Int32) throws -> FinalCutAXNode {
        elementsByPath.removeAll(keepingCapacity: true)
        visitedNodes = 0
        return try snapshot(
            element: AXUIElementCreateApplication(pid),
            path: [],
            depth: 0
        )
    }

    func perform(_ mutation: FinalCutAXMutation) throws {
        switch mutation {
        case .showGoToFolder:
            try postKey(code: 5, flags: [.maskCommand, .maskShift])
            return
        case .confirmGoToFolder:
            try postKey(code: 36, flags: [])
            return
        case .press, .setValue, .cancel:
            break
        }
        let path: [Int]
        switch mutation {
        case let .press(value), let .cancel(value):
            path = value
        case let .setValue(value, _):
            path = value
        case .showGoToFolder, .confirmGoToFolder:
            preconditionFailure("fixed keyboard mutations return before AX path lookup")
        }
        guard let element = elementsByPath[path] else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                "stale Accessibility path"
            )
        }
        let status: AXError
        switch mutation {
        case .press:
            status = AXUIElementPerformAction(element, kAXPressAction as CFString)
        case let .setValue(_, value):
            status = AXUIElementSetAttributeValue(
                element,
                kAXValueAttribute as CFString,
                value as CFTypeRef
            )
        case .cancel:
            status = AXUIElementPerformAction(element, kAXCancelAction as CFString)
        case .showGoToFolder, .confirmGoToFolder:
            preconditionFailure("fixed keyboard mutations return before AX action")
        }
        guard status == .success else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                "Accessibility mutation failed with status \(status.rawValue)"
            )
        }
    }

    func monotonicTime() -> TimeInterval {
        ProcessInfo.processInfo.systemUptime
    }

    func wait(interval: TimeInterval) {
        Thread.sleep(forTimeInterval: interval)
    }

    private func postKey(code: CGKeyCode, flags: CGEventFlags) throws {
        guard let source = CGEventSource(stateID: .combinedSessionState),
              let keyDown = CGEvent(
                  keyboardEventSource: source,
                  virtualKey: code,
                  keyDown: true
              ),
              let keyUp = CGEvent(
                  keyboardEventSource: source,
                  virtualKey: code,
                  keyDown: false
              )
        else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                "fixed keyboard event"
            )
        }
        keyDown.flags = flags
        keyUp.flags = flags
        keyDown.post(tap: .cghidEventTap)
        keyUp.post(tap: .cghidEventTap)
    }

    private func snapshot(
        element: AXUIElement,
        path: [Int],
        depth: Int
    ) throws -> FinalCutAXNode {
        guard depth <= 24, visitedNodes < 8_192 else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                "Accessibility tree exceeds safety bounds"
            )
        }
        visitedNodes += 1
        elementsByPath[path] = element
        let children = attribute(element, kAXChildrenAttribute as CFString) as? [AXUIElement]
            ?? []
        return try FinalCutAXNode(
            role: attribute(element, kAXRoleAttribute as CFString) as? String ?? "",
            subrole: attribute(element, kAXSubroleAttribute as CFString) as? String,
            identifier: attribute(element, kAXIdentifierAttribute as CFString) as? String,
            title: attribute(element, kAXTitleAttribute as CFString) as? String,
            children: children.enumerated().map { index, child in
                try snapshot(element: child, path: path + [index], depth: depth + 1)
            }
        )
    }

    private func attribute(
        _ element: AXUIElement,
        _ name: CFString
    ) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name, &value) == .success else {
            return nil
        }
        return value
    }
}

final class FinalCutAccessibilityDriver {
    private let system: FinalCutAccessibilitySystem
    private(set) var stateMachine = FinalCutAutomationStateMachine()

    init(
        system: FinalCutAccessibilitySystem = SystemFinalCutAccessibilitySystem()
    ) {
        self.system = system
    }

    func exportCurrentProject(
        to directory: URL,
        fileName: String,
        timeout: TimeInterval = 20
    ) throws -> URL {
        var targetPID: Int32?
        do {
            guard system.isTrusted(prompt: false) else {
                _ = system.isTrusted(prompt: true)
                _ = try stateMachine.begin(permissionGranted: false, hosts: [])
                throw FinalCutAutomationError.permissionRequired
            }
            let pid = try stateMachine.begin(
                permissionGranted: true,
                hosts: system.runningHosts()
            )
            targetPID = pid
            let deadline = system.monotonicTime() + timeout
            try focus(pid: pid, deadline: deadline)
            let initial = try system.snapshot(pid: pid)
            _ = try FinalCutAXLocator.uniquePath(
                in: initial,
                role: "AXMenuButton",
                identifier: FinalCutAXIdentifiers.currentProject
            )
            let fileMenu = try FinalCutAXLocator.fileMenuPath(in: initial)
            stateMachine.transition(to: .openingExportMenu(pid))
            try perform(.press(fileMenu), pid: pid)

            let openedMenu = try system.snapshot(pid: pid)
            let exportXML = try FinalCutAXLocator.exportXMLPath(in: openedMenu)
            try perform(.press(exportXML), pid: pid)
            stateMachine.transition(to: .configuringSavePanel(pid))

            _ = try waitForSavePanel(pid: pid, deadline: deadline)
            try perform(.showGoToFolder, pid: pid)
            let goToFolder = try waitForGoToFolder(pid: pid, deadline: deadline)
            try perform(.setValue(goToFolder.path, directory.path), pid: pid)
            try perform(.confirmGoToFolder, pid: pid)
            let savePanel = try waitForSavePanel(pid: pid, deadline: deadline)
            try perform(.setValue(savePanel.name, fileName), pid: pid)
            try perform(.press(savePanel.ok), pid: pid)
            stateMachine.transition(to: .waitingForOutput(pid))
            return directory.appendingPathComponent(fileName)
        } catch {
            if let targetPID {
                try? cancelCurrentDialog(pid: targetPID)
            }
            throw error
        }
    }

    private func waitForSavePanel(
        pid: Int32,
        deadline: TimeInterval
    ) throws -> FinalCutSavePanelPaths {
        while true {
            try stateMachine.checkDeadline(
                now: system.monotonicTime(),
                deadline: deadline
            )
            let current = try system.snapshot(pid: pid)
            if let controls = try? FinalCutAXLocator.savePanelControls(in: current) {
                try stateMachine.validateDialog(identifier: FinalCutAXIdentifiers.savePanel)
                return controls
            }
            try rejectUnexpectedDialogs(
                in: current,
                allowed: [
                    FinalCutAXIdentifiers.savePanel,
                    FinalCutAXIdentifiers.goToFolder,
                ],
                pid: pid
            )
            system.wait(interval: 0.05)
        }
    }

    private func waitForGoToFolder(
        pid: Int32,
        deadline: TimeInterval
    ) throws -> FinalCutGoToFolderPaths {
        while true {
            try stateMachine.checkDeadline(
                now: system.monotonicTime(),
                deadline: deadline
            )
            let current = try system.snapshot(pid: pid)
            if let controls = try? FinalCutAXLocator.goToFolderControls(in: current) {
                try stateMachine.validateDialog(identifier: FinalCutAXIdentifiers.goToFolder)
                return controls
            }
            try rejectUnexpectedDialogs(
                in: current,
                allowed: [
                    FinalCutAXIdentifiers.savePanel,
                    FinalCutAXIdentifiers.goToFolder,
                ],
                pid: pid
            )
            system.wait(interval: 0.05)
        }
    }

    private func rejectUnexpectedDialogs(
        in root: FinalCutAXNode,
        allowed: Set<String>,
        pid: Int32
    ) throws {
        for dialog in FinalCutAXLocator.dialogPaths(in: root) {
            let identifier = dialog.1 ?? "missing identifier"
            guard allowed.contains(identifier) else {
                try? perform(.cancel(dialog.0), pid: pid)
                try stateMachine.validateDialog(identifier: dialog.1)
                return
            }
        }
    }

    private func cancelCurrentDialog(pid: Int32) throws {
        let current = try system.snapshot(pid: pid)
        if let dialog = FinalCutAXLocator.dialogPaths(in: current).first {
            try perform(.cancel(dialog.0), pid: pid)
        }
    }

    private func focus(pid: Int32, deadline: TimeInterval) throws {
        _ = try system.activate(pid: pid)
        while system.frontmostPID() != pid {
            try stateMachine.checkDeadline(
                now: system.monotonicTime(),
                deadline: deadline
            )
            system.wait(interval: 0.05)
        }
    }

    private func perform(_ mutation: FinalCutAXMutation, pid: Int32) throws {
        try stateMachine.authorizeMutation(frontmostPID: system.frontmostPID())
        try system.perform(mutation)
    }

}
