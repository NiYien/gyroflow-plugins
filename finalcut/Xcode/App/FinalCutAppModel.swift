import AppKit
import Foundation
import SwiftUI
import UniformTypeIdentifiers

enum FinalCutWorkflowStage: Equatable {
    case selectFCPXML
    case processing
    case saved
}

@MainActor
final class FinalCutAppModel: ObservableObject {
    @Published var templateStatus = TemplateInstallStatus(
        state: .notInstalled,
        installedVersion: nil,
        message: FinalCutStrings.text("app.status.checking_template")
    )
    @Published var installationMessage = ""
    @Published var templatePreparationError: String?
    @Published var selectedFCPXML: URL?
    @Published var routeDState: SandboxedRouteDState = .idle
    @Published var preparedProject: PreparedReplacementProject?
    @Published var savedProject: SavedReplacementProject?
    @Published var workflowMessage = FinalCutStrings.text("app.status.choose_input")

    let appVersion: String
    let effectVersion: String
    let templateVersion: String

    private let installer: TemplateInstaller?
    private let workflow: SandboxedRouteDWorkflow

    init(
        bundle: Bundle = .main,
        workflow: SandboxedRouteDWorkflow? = nil,
        processBatch: ((ResolvedFCPXMLInput) throws -> RouteDBatchProcessorOutput)? = nil,
        openURL: ((URL) -> Bool)? = nil
    ) {
        appVersion = bundle.object(
            forInfoDictionaryKey: "CFBundleShortVersionString"
        ) as? String ?? FinalCutStrings.text("app.value.unknown")
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
        let productionProcess = processBatch ?? { source in
            let result = try RouteDProcessor.processBatchFCPXML(
                source.data,
                documentURL: source.xmlURL
            )
            guard let fcpxml = result["fcpxml"],
                  let report = result["report"]
            else {
                throw CocoaError(.fileReadCorruptFile)
            }
            return RouteDBatchProcessorOutput(fcpxml: fcpxml, report: report)
        }
        let productionOpen = openURL ?? { url in
            NSWorkspace.shared.open(url)
        }
        self.workflow = workflow ?? SandboxedRouteDWorkflow(
            process: productionProcess,
            open: productionOpen
        )
        refreshTemplateStatus()
    }

    var isBusy: Bool {
        routeDState == .loadingInput || routeDState == .processing || routeDState == .saving
    }

    var workflowStage: FinalCutWorkflowStage {
        if savedProject != nil, routeDState == .saved {
            return .saved
        }
        return selectedFCPXML == nil ? .selectFCPXML : .processing
    }

    var detectedEffectCount: Int? {
        preparedProject?.report.occurrenceCount
    }

    var changedTargets: [RouteDBatchTarget] {
        preparedProject?.report.targets.filter { $0.action != .skipped } ?? []
    }

    var skippedTargets: [RouteDBatchTarget] {
        preparedProject?.report.targets.filter { $0.action == .skipped } ?? []
    }

    var errorMessage: String? {
        guard case let .failed(message) = routeDState else {
            return nil
        }
        return message
    }

    func refreshTemplateStatus() {
        templateStatus = installer?.status() ?? TemplateInstallStatus(
            state: .repairRequired,
            installedVersion: nil,
            message: FinalCutStrings.text("app.status.template.unavailable")
        )
    }

    func prepareTemplateIfNeeded() {
        do {
            templateStatus = try requireInstaller().prepareIfNeeded()
            templatePreparationError = nil
        } catch {
            templatePreparationError = error.localizedDescription
            refreshTemplateStatus()
        }
    }

    func installOrRepair() {
        do {
            templateStatus = try requireInstaller().installOrRepair()
            installationMessage = FinalCutStrings.text("app.installation.footer")
        } catch {
            installationMessage = error.localizedDescription
            refreshTemplateStatus()
        }
    }

    func removeTemplate() {
        do {
            try requireInstaller().remove()
            installationMessage = FinalCutStrings.text("app.installation.removed")
            refreshTemplateStatus()
        } catch {
            installationMessage = error.localizedDescription
        }
    }

    func chooseFCPXML() {
        let panel = NSOpenPanel()
        let fcpxmldType = UTType(
            importedAs: "com.apple.finalcutpro.xmld",
            conformingTo: .package
        )
        panel.allowedContentTypes = [
            UTType(filenameExtension: "fcpxml") ?? .xml,
            fcpxmldType,
        ]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.treatsFilePackagesAsDirectories = false
        panel.message = FinalCutStrings.text("app.panel.open_fcpxml")
        guard panel.runModal() == .OK, let selection = panel.url else {
            return
        }
        loadFCPXML(selection)
    }

    func selectFCPXML(_ selection: URL) {
        workflow.selectInput(selection)
        selectedFCPXML = workflow.source?.selectionURL
        synchronizeWorkflow()
        if selectedFCPXML != nil {
            process()
        }
    }

    func loadFCPXML(_ selection: URL) {
        let job = workflow.makeInputSelectionJob(selection)
        selectedFCPXML = nil
        synchronizeWorkflow()
        DispatchQueue.global(qos: .userInitiated).async {
            let result = job.run()
            DispatchQueue.main.async { [weak self] in
                guard let self else {
                    return
                }
                let accepted = self.workflow.acceptInputSelection(result, from: job)
                self.selectedFCPXML = self.workflow.source?.selectionURL
                self.synchronizeWorkflow()
                if accepted, self.selectedFCPXML != nil {
                    self.process()
                }
            }
        }
    }

    func cancelActiveWork() {
        workflow.cancelActiveWork()
        synchronizeWorkflow()
    }

    func retryOpenSavedProject() {
        _ = workflow.reopenSavedProject()
        synchronizeWorkflow()
    }

    func revealSavedProject() {
        guard let destination = savedProject?.destination else {
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([destination])
    }

    func process() {
        guard !isBusy, let job = workflow.makePreparationJob() else {
            synchronizeWorkflow()
            return
        }
        synchronizeWorkflow()
        DispatchQueue.global(qos: .userInitiated).async {
            let result = job.run()
            DispatchQueue.main.async { [weak self] in
                guard let self else {
                    return
                }
                let accepted = self.workflow.acceptPreparation(result, from: job)
                self.synchronizeWorkflow()
                if accepted, self.routeDState == .preview {
                    self.saveReplacement()
                }
            }
        }
    }

    func saveReplacement() {
        guard let job = workflow.makeSaveJob() else {
            synchronizeWorkflow()
            return
        }
        synchronizeWorkflow()
        DispatchQueue.global(qos: .userInitiated).async {
            let result = job.run()
            DispatchQueue.main.async { [weak self] in
                guard let self else {
                    return
                }
                _ = self.workflow.acceptSave(result, from: job)
                self.synchronizeWorkflow()
            }
        }
    }

    private func synchronizeWorkflow() {
        routeDState = workflow.state
        preparedProject = workflow.preparedProject
        savedProject = workflow.savedProject
        switch workflow.state {
        case .idle:
            workflowMessage = FinalCutStrings.text("app.status.choose_input")
        case .loadingInput:
            workflowMessage = FinalCutStrings.text("app.status.loading_input")
        case .inputReady:
            workflowMessage = FinalCutStrings.text("app.status.input_ready")
        case .processing:
            workflowMessage = FinalCutStrings.text("app.status.generating")
        case .preview:
            workflowMessage = FinalCutStrings.text("app.status.replacement_ready")
        case .saving:
            workflowMessage = FinalCutStrings.text("app.status.saving")
        case .saved:
            workflowMessage = FinalCutStrings.text("app.status.saved")
        case let .failed(message):
            workflowMessage = message
        }
    }

    private func requireInstaller() throws -> TemplateInstaller {
        guard let installer else {
            throw TemplateInstallerError.missingResource(
                FinalCutStrings.text("app.resource.production_template")
            )
        }
        return installer
    }
}
