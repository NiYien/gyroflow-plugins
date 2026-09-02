import Foundation

enum FinalCutAXLocator {
    static let supportedBundleIdentifiers: Set<String> = [
        "com.apple.FinalCutApp",
        "com.apple.FinalCut",
    ]

    private static let menuTitles = [
        (file: "File", exportXML: "Export XML…"),
        (file: "文件", exportXML: "导出 XML…"),
    ]

    static func uniqueSupportedHost(
        _ hosts: [FinalCutRunningHost]
    ) throws -> FinalCutRunningHost {
        let supported = hosts.filter {
            supportedBundleIdentifiers.contains($0.bundleIdentifier)
        }
        guard supported.count == 1 else {
            throw FinalCutAutomationError.hostUnavailable(
                "Expected one running supported Final Cut process; found \(supported.count)."
            )
        }
        return supported[0]
    }

    static func uniquePath(
        in root: FinalCutAXNode,
        role: String,
        subrole: String? = nil,
        identifier: String? = nil,
        title: String? = nil
    ) throws -> [Int] {
        let paths = matchingPaths(
            in: root,
            role: role,
            subrole: subrole,
            identifier: identifier,
            title: title
        )
        guard paths.count == 1 else {
            let label = identifier ?? title ?? role
            throw FinalCutAutomationError.elementMissingOrAmbiguous(label)
        }
        return paths[0]
    }

    static func exportMenuPaths(in root: FinalCutAXNode) throws -> FinalCutMenuPaths {
        let filePath = try fileMenuPath(in: root)
        guard let fileNode = node(in: root, at: filePath) else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous("File / 文件")
        }
        let relativeExportPath = try exportXMLPath(in: fileNode)
        return FinalCutMenuPaths(
            file: filePath,
            exportXML: filePath + relativeExportPath
        )
    }

    static func fileMenuPath(in root: FinalCutAXNode) throws -> [Int] {
        var matches: [[Int]] = []
        for titles in menuTitles {
            matches += matchingPaths(
                in: root,
                role: "AXMenuBarItem",
                title: titles.file
            )
        }
        guard matches.count == 1 else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous("File / 文件")
        }
        return matches[0]
    }

    static func exportXMLPath(in root: FinalCutAXNode) throws -> [Int] {
        var matches: [[Int]] = []
        for titles in menuTitles {
            matches += matchingPaths(
                in: root,
                role: "AXMenuItem",
                title: titles.exportXML
            )
        }
        guard matches.count == 1 else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                "Export XML… / 导出 XML…"
            )
        }
        return matches[0]
    }

    static func savePanelControls(
        in root: FinalCutAXNode
    ) throws -> FinalCutSavePanelPaths {
        let panelPath = try uniquePath(
            in: root,
            role: "AXWindow",
            subrole: "AXDialog",
            identifier: FinalCutAXIdentifiers.savePanel
        )
        guard let panel = node(in: root, at: panelPath) else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                FinalCutAXIdentifiers.savePanel
            )
        }
        let name = try uniquePath(
            in: panel,
            role: "AXTextField",
            identifier: FinalCutAXIdentifiers.saveName
        )
        let ok = try uniquePath(
            in: panel,
            role: "AXButton",
            identifier: FinalCutAXIdentifiers.okButton
        )
        return FinalCutSavePanelPaths(
            panel: panelPath,
            name: panelPath + name,
            ok: panelPath + ok
        )
    }

    static func goToFolderControls(
        in root: FinalCutAXNode
    ) throws -> FinalCutGoToFolderPaths {
        let panelPath = try uniquePath(
            in: root,
            role: "AXSheet",
            identifier: FinalCutAXIdentifiers.goToFolder
        )
        guard let panel = node(in: root, at: panelPath) else {
            throw FinalCutAutomationError.elementMissingOrAmbiguous(
                FinalCutAXIdentifiers.goToFolder
            )
        }
        let path = try uniquePath(
            in: panel,
            role: "AXTextField",
            identifier: FinalCutAXIdentifiers.path
        )
        return FinalCutGoToFolderPaths(
            panel: panelPath,
            path: panelPath + path
        )
    }

    static func node(in root: FinalCutAXNode, at path: [Int]) -> FinalCutAXNode? {
        var current = root
        for index in path {
            guard current.children.indices.contains(index) else {
                return nil
            }
            current = current.children[index]
        }
        return current
    }

    static func dialogPaths(in root: FinalCutAXNode) -> [([Int], String?)] {
        let sheets = matchingPaths(in: root, role: "AXSheet")
        let dialogWindows = matchingPaths(
            in: root,
            role: "AXWindow",
            subrole: "AXDialog"
        )
        return (sheets + dialogWindows).map { path in
            (path, node(in: root, at: path)?.identifier)
        }
    }

    private static func matchingPaths(
        in root: FinalCutAXNode,
        role: String,
        subrole: String? = nil,
        identifier: String? = nil,
        title: String? = nil
    ) -> [[Int]] {
        var matches: [[Int]] = []
        func visit(_ node: FinalCutAXNode, path: [Int]) {
            if node.role == role,
               subrole.map({ node.subrole == $0 }) ?? true,
               identifier.map({ node.identifier == $0 }) ?? true,
               title.map({ node.title == $0 }) ?? true
            {
                matches.append(path)
            }
            for (index, child) in node.children.enumerated() {
                visit(child, path: path + [index])
            }
        }
        visit(root, path: [])
        return matches
    }
}
