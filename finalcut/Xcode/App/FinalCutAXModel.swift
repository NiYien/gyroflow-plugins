import Foundation

enum FinalCutAXIdentifiers {
    static let currentProject = "editor/timelineContainer/toolbar/projectNamePopUpButton"
    static let savePanel = "save-panel"
    static let goToFolder = "GoToWindow"
    static let saveName = "saveAsNameTextField"
    static let path = "PathTextField"
    static let okButton = "OKButton"
}

struct FinalCutRunningHost: Equatable {
    let bundleIdentifier: String
    let pid: Int32
}

struct FinalCutAXNode: Equatable {
    let role: String
    let subrole: String?
    let identifier: String?
    let title: String?
    let children: [FinalCutAXNode]

    init(
        role: String,
        subrole: String? = nil,
        identifier: String?,
        title: String?,
        children: [FinalCutAXNode]
    ) {
        self.role = role
        self.subrole = subrole
        self.identifier = identifier
        self.title = title
        self.children = children
    }
}

struct FinalCutMenuPaths: Equatable {
    let file: [Int]
    let exportXML: [Int]
}

struct FinalCutSavePanelPaths: Equatable {
    let panel: [Int]
    let name: [Int]
    let ok: [Int]
}

struct FinalCutGoToFolderPaths: Equatable {
    let panel: [Int]
    let path: [Int]
}

enum FinalCutAutomationState: Equatable {
    case idle
    case locatingHost(Int32)
    case openingExportMenu(Int32)
    case configuringSavePanel(Int32)
    case waitingForOutput(Int32)
    case completed(URL)
    case manualFallback(String)

    var isManualFallback: Bool {
        if case .manualFallback = self {
            return true
        }
        return false
    }
}

enum FinalCutAutomationError: LocalizedError, Equatable {
    case permissionRequired
    case hostUnavailable(String)
    case elementMissingOrAmbiguous(String)
    case frontmostProcessChanged
    case unexpectedDialog(String)
    case timedOut

    var errorDescription: String? {
        switch self {
        case .permissionRequired:
            return "Accessibility permission is required; use the manual FCPXML workflow meanwhile."
        case let .hostUnavailable(message):
            return message
        case let .elementMissingOrAmbiguous(name):
            return "Final Cut Accessibility element is missing or ambiguous: \(name)"
        case .frontmostProcessChanged:
            return "Final Cut is no longer the same frontmost application."
        case let .unexpectedDialog(identifier):
            return "Final Cut showed an unsupported dialog: \(identifier)"
        case .timedOut:
            return "Final Cut project export timed out."
        }
    }
}

struct FinalCutAutomationStateMachine {
    private(set) var state: FinalCutAutomationState = .idle
    private(set) var mutationCount = 0
    private var expectedPID: Int32?

    mutating func begin(
        permissionGranted: Bool,
        hosts: [FinalCutRunningHost]
    ) throws -> Int32 {
        guard permissionGranted else {
            throw fail(.permissionRequired)
        }
        do {
            let host = try FinalCutAXLocator.uniqueSupportedHost(hosts)
            expectedPID = host.pid
            state = .locatingHost(host.pid)
            return host.pid
        } catch let error as FinalCutAutomationError {
            throw fail(error)
        }
    }

    mutating func authorizeMutation(frontmostPID: Int32?) throws {
        guard let expectedPID, frontmostPID == expectedPID else {
            throw fail(.frontmostProcessChanged)
        }
        mutationCount += 1
    }

    mutating func validateDialog(identifier: String?) throws {
        guard [
            FinalCutAXIdentifiers.savePanel,
            FinalCutAXIdentifiers.goToFolder,
        ].contains(identifier) else {
            throw fail(.unexpectedDialog(identifier ?? "missing identifier"))
        }
    }

    mutating func checkDeadline(now: TimeInterval, deadline: TimeInterval) throws {
        guard now <= deadline else {
            throw fail(.timedOut)
        }
    }

    mutating func transition(to state: FinalCutAutomationState) {
        self.state = state
    }

    @discardableResult
    private mutating func fail(
        _ error: FinalCutAutomationError
    ) -> FinalCutAutomationError {
        state = .manualFallback(error.localizedDescription)
        return error
    }
}
