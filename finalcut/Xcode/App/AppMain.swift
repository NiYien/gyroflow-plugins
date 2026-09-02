import AppKit
import Darwin
import SwiftUI
import UniformTypeIdentifiers

@MainActor
final class FinalCutAppModel: ObservableObject {
    @Published var templateStatus = TemplateInstallStatus(
        state: .notInstalled,
        installedVersion: nil,
        message: "Checking template status"
    )
    @Published var operationMessage = ""
    @Published var selectedFCPXML: URL?
    @Published var processedProjectName = ""
    @Published var projectProcessingMessage = ""
    @Published var routeDState: OneClickRouteDState = .idle
    @Published var preparedProject: PreparedRouteDProject?
    @Published var failedTargets: [RouteDBatchTarget] = []

    let appVersion: String
    let effectVersion: String
    let templateVersion: String
    private let installer: TemplateInstaller?
    private let routeD: OneClickRouteDWorkflow

    init(
        bundle: Bundle = .main,
        routeD: OneClickRouteDWorkflow? = nil
    ) {
        appVersion = bundle.object(
            forInfoDictionaryKey: "CFBundleShortVersionString"
        ) as? String ?? "unknown"
        templateVersion = appVersion
        let effectInfo = bundle.bundleURL
            .appendingPathComponent("Contents/PlugIns", isDirectory: true)
            .appendingPathComponent(
                "GyroflowNiYienFinalCutEffect.pluginkit",
                isDirectory: true
            )
            .appendingPathComponent("Contents/Info.plist")
        let effectDictionary = NSDictionary(contentsOf: effectInfo)
        effectVersion = effectDictionary?["CFBundleShortVersionString"] as? String
            ?? appVersion
        installer = try? TemplateInstaller.production(bundle: bundle)
        self.routeD = routeD ?? OneClickRouteDWorkflow(
            dependencies: OneClickRouteDProduction.dependencies()
        )
        self.routeD.onStateChange = { [weak self] state in
            DispatchQueue.main.async {
                self?.routeDState = state
                self?.projectProcessingMessage = Self.message(for: state)
            }
        }
        processedProjectName = "Current Project — Gyroflow Processed"
        refreshStatus()
    }

    func refreshStatus() {
        templateStatus = installer?.status() ?? TemplateInstallStatus(
            state: .repairRequired,
            installedVersion: nil,
            message: "Bundled template resources are unavailable"
        )
    }

    func installOrRepair() {
        do {
            templateStatus = try requireInstaller().installOrRepair()
            operationMessage = "Template installed. Restart Final Cut Pro to refresh effects."
        } catch {
            operationMessage = error.localizedDescription
            refreshStatus()
        }
    }

    func removeTemplate() {
        do {
            try requireInstaller().remove()
            operationMessage = "Gyroflow NiYien template removed."
            refreshStatus()
        } catch {
            operationMessage = error.localizedDescription
        }
    }

    func chooseFCPXML() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [
            UTType(filenameExtension: "fcpxml") ?? .xml,
            UTType(filenameExtension: "fcpxmld") ?? .package,
        ]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.message =
            "Choose one complete .fcpxml file or .fcpxmld package exported after editing."
        guard panel.runModal() == .OK, let url = panel.url else {
            return
        }
        selectedFCPXML = url
        processedProjectName =
            url.deletingPathExtension().lastPathComponent + " — Gyroflow Processed"
        processManualSelection(url)
    }

    func processCurrentProject() {
        guard validateProcessedName() else {
            return
        }
        routeDState = .exporting
        projectProcessingMessage = Self.message(for: .exporting)
        let routeD = self.routeD
        let name = processedProjectName
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            routeD.processCurrentProject(processedName: name)
            DispatchQueue.main.async {
                self?.synchronizeRouteD()
            }
        }
    }

    func confirmImport() {
        guard let preparedProject else {
            return
        }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "fcpxml") ?? .xml]
        panel.canCreateDirectories = true
        panel.nameFieldStringValue =
            preparedProject.report.processedProjectName
                .replacingOccurrences(of: "/", with: "-")
                + ".fcpxml"
        panel.message =
            "Confirm where to save the separate processed FCPXML before Final Cut imports it as a new project."
        guard panel.runModal() == .OK, let destination = panel.url else {
            projectProcessingMessage = "Import cancelled. No processed project was written."
            return
        }
        routeD.confirmImport(to: destination)
        synchronizeRouteD()
    }

    func cancelPreview() {
        routeD.cancelPreview()
        synchronizeRouteD()
    }

    var processingIsBusy: Bool {
        matchesBusyState(routeDState)
    }

    private func processManualSelection(_ source: URL) {
        guard validateProcessedName() else {
            return
        }
        routeDState = .processing
        projectProcessingMessage = Self.message(for: .processing)
        let routeD = self.routeD
        let name = processedProjectName
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let accessed = source.startAccessingSecurityScopedResource()
            defer {
                if accessed {
                    source.stopAccessingSecurityScopedResource()
                }
            }
            routeD.processManual(selection: source, processedName: name)
            DispatchQueue.main.async {
                self?.synchronizeRouteD()
            }
        }
    }

    private func synchronizeRouteD() {
        routeDState = routeD.state
        preparedProject = routeD.preparedProject
        failedTargets = routeD.failedTargets
        projectProcessingMessage = Self.message(for: routeD.state)
    }

    private func validateProcessedName() -> Bool {
        if processedProjectName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            projectProcessingMessage = "Enter a processed project name."
            return false
        }
        return true
    }

    private func matchesBusyState(_ state: OneClickRouteDState) -> Bool {
        state == .exporting || state == .processing || state == .importing
    }

    private static func message(for state: OneClickRouteDState) -> String {
        switch state {
        case .idle:
            return "Ready. Automatic and manual FCPXML workflows are available."
        case .exporting:
            return "Exporting the current complete Final Cut project…"
        case .processing:
            return "Validating projects and building an all-or-nothing Route D preview…"
        case .preview:
            return "Preview ready. Review every target before confirming import."
        case .importing:
            return "Writing the separate processed FCPXML and opening it in Final Cut…"
        case let .completed(destination):
            return "Opened \(destination.lastPathComponent) for Final Cut import. Final Cut assigns the imported project a new UID."
        case let .failed(message):
            return "Automatic processing stopped: \(message) Use the manual FCPXML workflow or retry after correcting the issue."
        }
    }

    private func requireInstaller() throws -> TemplateInstaller {
        guard let installer else {
            throw TemplateInstallerError.missingResource("production template")
        }
        return installer
    }
}

struct FinalCutAppView: View {
    @StateObject private var model = FinalCutAppModel()

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                Text("Gyroflow NiYien Final Cut")
                    .font(.title2)

                GroupBox("Installation") {
                    Grid(alignment: .leading, horizontalSpacing: 20, verticalSpacing: 6) {
                        GridRow { Text("App"); Text(model.appVersion) }
                        GridRow { Text("FxPlug XPC"); Text(model.effectVersion) }
                        GridRow { Text("Template"); Text(model.templateVersion) }
                        GridRow { Text("Status"); Text(model.templateStatus.message) }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    HStack {
                        Button(
                            model.templateStatus.state == .notInstalled
                                ? "Install Template"
                                : "Install / Repair"
                        ) {
                            model.installOrRepair()
                        }
                        .frame(minHeight: 44)
                        Button("Remove Template") {
                            model.removeTemplate()
                        }
                        .frame(minHeight: 44)
                        .disabled(model.templateStatus.state == .notInstalled)
                    }
                    if !model.operationMessage.isEmpty {
                        Text(model.operationMessage)
                            .foregroundStyle(.secondary)
                    }
                }

                GroupBox("Process the current Final Cut project") {
                    VStack(alignment: .leading, spacing: 16) {
                        Text(
                            "One click exports the complete project, matches exact sibling .gyroflow projects, "
                                + "updates or inserts supported NiYien effects, and stops at a full preview. "
                                + "Importing creates a new processed project; the original project is preserved. "
                                + "Final Cut assigns the imported project a new UID."
                        )
                        .foregroundStyle(.secondary)

                        LabeledContent("Processed project name") {
                            TextField(
                                "Processed project name",
                                text: $model.processedProjectName
                            )
                            .textFieldStyle(.roundedBorder)
                        }

                        Button("Process Current Final Cut Project") {
                            model.processCurrentProject()
                        }
                        .buttonStyle(.borderedProminent)
                        .frame(minHeight: 44)
                        .disabled(model.processingIsBusy)
                        .accessibilityHint(
                            "Requests Accessibility permission only when pressed, exports the current project, and prepares a preview without importing."
                        )

                        Divider()

                        VStack(alignment: .leading, spacing: 8) {
                            Text("Manual fallback")
                                .font(.headline)
                            Text(
                                "Choose FCPXML or FCPXMLD if automatic export is unavailable. "
                                    + "The same batch validation and preview are used."
                            )
                            .foregroundStyle(.secondary)
                            Button("Choose FCPXML or FCPXMLD…") {
                                model.chooseFCPXML()
                            }
                            .frame(minHeight: 44)
                            .disabled(model.processingIsBusy)
                            .accessibilityHint(
                                "Choose one .fcpxml file or one .fcpxmld package containing root Info.fcpxml."
                            )
                        }

                        if let selected = model.selectedFCPXML {
                            Text(selected.path)
                                .font(.caption)
                                .textSelection(.enabled)
                        }

                        if model.processingIsBusy {
                            HStack(spacing: 8) {
                                ProgressView()
                                    .controlSize(.small)
                                Text(model.projectProcessingMessage)
                            }
                            .accessibilityElement(children: .combine)
                        } else if !model.projectProcessingMessage.isEmpty {
                            Text(model.projectProcessingMessage)
                                .foregroundStyle(model.routeDState.isFailure ? .red : .secondary)
                        }

                        if let prepared = model.preparedProject {
                            BatchPreviewView(
                                prepared: prepared,
                                confirm: model.confirmImport,
                                cancel: model.cancelPreview
                            )
                        } else if !model.failedTargets.isEmpty {
                            VStack(alignment: .leading, spacing: 8) {
                                Label("Failed", systemImage: "exclamationmark.triangle.fill")
                                    .font(.headline)
                                    .foregroundStyle(.red)
                                ForEach(model.failedTargets) { target in
                                    BatchTargetRow(target: target)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(24)
        }
        .frame(minWidth: 720, minHeight: 640)
    }
}

private struct BatchPreviewView: View {
    let prepared: PreparedRouteDProject
    let confirm: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Divider()
            Text("Batch Preview")
                .font(.title3.weight(.semibold))
            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 6) {
                GridRow { Text("Original"); Text(prepared.report.originalProjectName) }
                GridRow { Text("Processed"); Text(prepared.report.processedProjectName) }
                GridRow {
                    Label("Inserted \(prepared.report.insertedCount)", systemImage: "plus.circle")
                    Label("Updated \(prepared.report.updatedCount)", systemImage: "arrow.triangle.2.circlepath")
                }
                GridRow {
                    Label("Skipped \(prepared.report.skippedCount)", systemImage: "forward.end")
                    Label("Failed \(prepared.report.failedCount)", systemImage: "xmark.octagon")
                }
            }
            .accessibilityElement(children: .contain)

            VStack(alignment: .leading, spacing: 8) {
                ForEach(prepared.report.targets) { target in
                    BatchTargetRow(target: target)
                }
            }

            Label(
                "The original Final Cut project remains unchanged",
                systemImage: "checkmark.shield"
            )
            .foregroundStyle(.secondary)

            HStack(spacing: 12) {
                Button("Confirm Import as New Project", action: confirm)
                    .buttonStyle(.borderedProminent)
                    .frame(minHeight: 44)
                    .accessibilityHint(
                        "Choose a separate processed FCPXML destination, then open it in Final Cut for import."
                    )
                Button("Cancel Preview", action: cancel)
                    .frame(minHeight: 44)
            }
        }
    }
}

private struct BatchTargetRow: View {
    let target: RouteDBatchTarget

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .foregroundStyle(color)
                .frame(width: 18)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(target.clipName)
                        .fontWeight(.medium)
                    Text(actionTitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Text(target.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(actionTitle): \(target.clipName). \(target.detail)")
    }

    private var actionTitle: String {
        switch target.action {
        case "inserted": return "Inserted"
        case "updated": return "Updated"
        case "skipped": return "Skipped"
        default: return "Failed"
        }
    }

    private var icon: String {
        switch target.action {
        case "inserted": return "plus.circle.fill"
        case "updated": return "arrow.triangle.2.circlepath.circle.fill"
        case "skipped": return "forward.end.circle.fill"
        default: return "xmark.octagon.fill"
        }
    }

    private var color: Color {
        switch target.action {
        case "inserted": return .green
        case "updated": return .blue
        case "skipped": return .secondary
        default: return .red
        }
    }
}

@main
struct GyroflowFinalCutApplication: App {
    init() {
        guard CommandLine.arguments.contains("--install-template-and-quit") else {
            return
        }
        do {
            let installer = try TemplateInstaller.production()
            let status = try installer.installOrRepair()
            guard status.state == .installed else {
                fputs("Final Cut template verification failed after install\n", stderr)
                exit(TemplateInstallExitCode.verificationFailed)
            }
            print("Final Cut template installed: \(status.message)")
            exit(TemplateInstallExitCode.success)
        } catch {
            fputs("Final Cut template install failed: \(error.localizedDescription)\n", stderr)
            exit(TemplateInstallExitCode.installFailed)
        }
    }

    var body: some Scene {
        WindowGroup("Gyroflow NiYien Final Cut") {
            FinalCutAppView()
        }
    }
}
