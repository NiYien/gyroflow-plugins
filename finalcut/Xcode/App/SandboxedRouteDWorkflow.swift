import Foundation

enum RouteDBatchTargetAction: String, Codable, Equatable {
    case updatedProject = "updated_project"
    case timingOnly = "timing_only"
    case skipped
}

enum RouteDBatchSkipReason: String, Codable, Equatable {
    case missingProject = "missing_project"
    case permissionDenied = "permission_denied"
    case invalidProject = "invalid_project"
    case incompatibleProject = "incompatible_project"
    case payloadTooLarge = "payload_too_large"
    case ambiguousMedia = "ambiguous_media"
    case unsupportedStructure = "unsupported_structure"
    case invalidTiming = "invalid_timing"
    case blockedGeometry = "blocked_geometry"
}

struct RouteDBatchTarget: Codable, Equatable {
    let occurrence: Int
    let clipName: String
    let assetRef: String?
    let mediaURL: String?
    let expectedProjectPath: String?
    let projectDisplayName: String?
    let action: RouteDBatchTargetAction
    let skipReason: RouteDBatchSkipReason?
    let detail: String
    let geometryStatus: String?
    let geometryReasons: [String]
    let geometryDetail: String?

    enum CodingKeys: String, CodingKey {
        case occurrence
        case clipName = "clip_name"
        case assetRef = "asset_ref"
        case mediaURL = "media_url"
        case expectedProjectPath = "expected_project_path"
        case projectDisplayName = "project_display_name"
        case action
        case skipReason = "skip_reason"
        case detail
        case geometryStatus = "geometry_status"
        case geometryReasons = "geometry_reasons"
        case geometryDetail = "geometry_detail"
    }
}

extension RouteDBatchTarget {
    var mediaBasename: String {
        guard let mediaURL,
              let url = URL(string: mediaURL),
              !url.lastPathComponent.isEmpty
        else {
            return FinalCutStrings.text(
                "app.target.media_unknown",
                fallback: "Unknown media"
            )
        }
        return url.lastPathComponent
    }

    var structuralIdentity: String {
        "\(clipName) · #\(occurrence) · \(mediaBasename)"
    }

    var expectedProjectText: String {
        String(
            format: FinalCutStrings.text(
                "app.label.expected_project",
                fallback: "Expected project: %@"
            ),
            locale: Locale.current,
            arguments: [
                expectedProjectPath ?? FinalCutStrings.text(
                    "app.target.project_unknown",
                    fallback: "Unknown project path"
                )
            ]
        )
    }

    var actionName: String {
        switch action {
        case .updatedProject:
            return FinalCutStrings.text("app.target.action.updated", fallback: "Updated project")
        case .timingOnly:
            return FinalCutStrings.text("app.target.action.timing", fallback: "Timing only")
        case .skipped:
            return FinalCutStrings.text("app.target.action.skipped", fallback: "Skipped")
        }
    }

    var statusDetail: String {
        switch action {
        case .updatedProject:
            return FinalCutStrings.text(
                "app.target.detail.updated",
                fallback: "Loaded the matching sibling Gyroflow project."
            )
        case .timingOnly:
            return FinalCutStrings.text(
                "app.target.detail.timing",
                fallback: "Kept the embedded project and updated effect timing."
            )
        case .skipped:
            guard let skipReason else {
                return FinalCutStrings.text("app.target.action.skipped", fallback: "Skipped")
            }
            switch skipReason {
            case .missingProject:
                return FinalCutStrings.text(
                    "app.skip.missing_project",
                    fallback: "Skipped because no matching sibling .gyroflow project was found."
                )
            case .permissionDenied:
                return FinalCutStrings.text(
                    "app.skip.permission_denied",
                    fallback: "Skipped because the media or project could not be read."
                )
            case .invalidProject:
                return FinalCutStrings.text(
                    "app.skip.invalid_project",
                    fallback: "Skipped because the Gyroflow project is invalid."
                )
            case .incompatibleProject:
                return FinalCutStrings.text(
                    "app.skip.incompatible_project",
                    fallback: "Skipped because the Gyroflow project is incompatible."
                )
            case .payloadTooLarge:
                return FinalCutStrings.text(
                    "app.skip.payload_too_large",
                    fallback: "Skipped because the embedded project is too large."
                )
            case .ambiguousMedia:
                return FinalCutStrings.text(
                    "app.skip.ambiguous_media",
                    fallback: "Skipped because the media match is ambiguous."
                )
            case .unsupportedStructure:
                return FinalCutStrings.text(
                    "app.skip.unsupported_structure",
                    fallback: "Skipped because the FCPXML structure is unsupported."
                )
            case .invalidTiming:
                return FinalCutStrings.text(
                    "app.skip.invalid_timing",
                    fallback: "Skipped because the effect timing is invalid."
                )
            case .blockedGeometry:
                return FinalCutStrings.text(
                    "app.skip.blocked_geometry",
                    fallback: "Skipped because the project geometry is incompatible."
                )
            }
        }
    }

    var accessibilitySummary: String {
        [actionName, structuralIdentity, expectedProjectText, statusDetail]
            .joined(separator: ". ")
    }
}

struct RouteDBatchReport: Codable, Equatable {
    let originalProjectName: String
    let occurrenceCount: Int
    let updatedProjectCount: Int
    let timingOnlyCount: Int
    let skippedCount: Int
    let targets: [RouteDBatchTarget]
    let behavior: String?

    init(
        originalProjectName: String,
        occurrenceCount: Int,
        updatedProjectCount: Int,
        timingOnlyCount: Int,
        skippedCount: Int,
        targets: [RouteDBatchTarget],
        behavior: String? = nil
    ) {
        self.originalProjectName = originalProjectName
        self.occurrenceCount = occurrenceCount
        self.updatedProjectCount = updatedProjectCount
        self.timingOnlyCount = timingOnlyCount
        self.skippedCount = skippedCount
        self.targets = targets
        self.behavior = behavior
    }

    enum CodingKeys: String, CodingKey {
        case originalProjectName = "original_project_name"
        case occurrenceCount = "occurrence_count"
        case updatedProjectCount = "updated_project_count"
        case timingOnlyCount = "timing_only_count"
        case skippedCount = "skipped_count"
        case targets
        case behavior
    }
}

extension RouteDBatchReport {
    func matchingTargets(issuesOnly: Bool, query: String) -> [RouteDBatchTarget] {
        let normalizedQuery = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return targets.filter { target in
            if issuesOnly, target.action != .skipped {
                return false
            }
            guard !normalizedQuery.isEmpty else {
                return true
            }
            return [
                target.clipName,
                target.mediaBasename,
                target.projectDisplayName ?? "",
                target.expectedProjectPath ?? "",
            ].contains { $0.localizedCaseInsensitiveContains(normalizedQuery) }
        }
    }
}

struct RouteDBatchProcessorOutput {
    let fcpxml: Data
    let report: Data
}

struct PreparedReplacementProject: Equatable {
    let source: ResolvedFCPXMLInput
    let fcpxml: Data
    let report: RouteDBatchReport
}

struct SavedReplacementProject: Equatable {
    let destination: URL
    let warning: String?
}

struct RouteDInputSnapshot: Equatable {
    let generation: UInt64
    let source: ResolvedFCPXMLInput
}

final class RouteDInputSelectionJob: @unchecked Sendable {
    let id: UUID
    let generation: UInt64

    private let selection: URL
    private let access: SecurityScopedAccess

    init(
        id: UUID,
        generation: UInt64,
        selection: URL,
        access: SecurityScopedAccess
    ) {
        self.id = id
        self.generation = generation
        self.selection = selection
        self.access = access
    }

    func run() -> Result<ResolvedFCPXMLInput, Error> {
        do {
            return .success(try access.withAccess(to: [selection]) {
                try FCPXMLDocumentInput.resolve(selection)
            })
        } catch {
            return .failure(error)
        }
    }
}

final class RouteDPreparationJob: @unchecked Sendable {
    let id: UUID
    let input: RouteDInputSnapshot

    private let access: SecurityScopedAccess
    private let processBatch: (ResolvedFCPXMLInput) throws -> RouteDBatchProcessorOutput

    init(
        id: UUID,
        input: RouteDInputSnapshot,
        access: SecurityScopedAccess,
        processBatch: @escaping (ResolvedFCPXMLInput) throws -> RouteDBatchProcessorOutput
    ) {
        self.id = id
        self.input = input
        self.access = access
        self.processBatch = processBatch
    }

    func run() -> Result<PreparedReplacementProject, Error> {
        do {
            let output = try access.withAccess(to: [input.source.selectionURL]) {
                try processBatch(input.source)
            }
            let report = try JSONDecoder().decode(
                RouteDBatchReport.self,
                from: output.report
            )
            let prepared = PreparedReplacementProject(
                source: input.source,
                fcpxml: output.fcpxml,
                report: report
            )
            guard report.updatedProjectCount + report.timingOnlyCount > 0 else {
                throw SandboxedRouteDWorkflowError.noReplacementTargets(prepared)
            }
            return .success(prepared)
        } catch {
            return .failure(error)
        }
    }
}

private enum RouteDSaveJobError: Error {
    case stale
}

final class RouteDSaveGenerationGate: @unchecked Sendable {
    private let lock = NSLock()
    private var currentGeneration: UInt64 = 0

    func update(to generation: UInt64) {
        lock.lock()
        currentGeneration = generation
        lock.unlock()
    }

    func commitIfCurrent<T>(
        _ generation: UInt64,
        operation: () throws -> T
    ) throws -> T {
        lock.lock()
        defer { lock.unlock() }
        guard currentGeneration == generation else {
            throw RouteDSaveJobError.stale
        }
        return try operation()
    }
}

final class RouteDSaveJob: @unchecked Sendable {
    let id: UUID
    let generation: UInt64
    let destination: URL

    private let preparedProject: PreparedReplacementProject
    private let access: SecurityScopedAccess
    private let store: ReplacementProjectStore
    private let generationGate: RouteDSaveGenerationGate

    init(
        id: UUID,
        generation: UInt64,
        preparedProject: PreparedReplacementProject,
        access: SecurityScopedAccess,
        store: ReplacementProjectStore,
        generationGate: RouteDSaveGenerationGate
    ) {
        self.id = id
        self.generation = generation
        destination = preparedProject.source.selectionURL.standardizedFileURL
        self.preparedProject = preparedProject
        self.access = access
        self.store = store
        self.generationGate = generationGate
    }

    func run() -> Result<URL, Error> {
        do {
            try access.withAccess(to: [destination]) {
                let staged = try store.stage(
                    preparedProject.fcpxml,
                    replacing: preparedProject.source
                )
                defer { store.discard(staged) }
                try generationGate.commitIfCurrent(generation) {
                    try store.commit(
                        staged,
                        replacing: preparedProject.source
                    )
                }
            }
            return .success(destination)
        } catch {
            return .failure(error)
        }
    }
}

enum SandboxedRouteDState: Equatable {
    case idle
    case loadingInput
    case inputReady
    case processing
    case preview
    case saving
    case saved
    case failed(String)

    var isFailed: Bool {
        if case .failed = self {
            return true
        }
        return false
    }

    var isSaved: Bool {
        if case .saved = self {
            return true
        }
        return false
    }
}

final class SandboxedRouteDWorkflow {
    private let access: SecurityScopedAccess
    private let processBatch: (ResolvedFCPXMLInput) throws -> RouteDBatchProcessorOutput
    private let store: ReplacementProjectStore
    private let open: (URL) -> Bool
    private let saveGenerationGate = RouteDSaveGenerationGate()

    private(set) var state: SandboxedRouteDState = .idle
    private(set) var source: ResolvedFCPXMLInput?
    private(set) var preparedProject: PreparedReplacementProject?
    private(set) var savedProject: SavedReplacementProject?
    private(set) var inputGeneration: UInt64 = 0
    private var activeInputSelectionID: UUID?
    private var activeProcessingID: UUID?
    private var activeSaveID: UUID?

    init(
        access: SecurityScopedAccess = .live,
        store: ReplacementProjectStore = ReplacementProjectStore(),
        process: @escaping (ResolvedFCPXMLInput) throws -> RouteDBatchProcessorOutput,
        open: @escaping (URL) -> Bool
    ) {
        self.access = access
        self.store = store
        processBatch = process
        self.open = open
    }

    func selectInput(_ selection: URL) {
        let job = makeInputSelectionJob(selection)
        acceptInputSelection(job.run(), from: job)
    }

    func makeInputSelectionJob(_ selection: URL) -> RouteDInputSelectionJob {
        inputGeneration &+= 1
        saveGenerationGate.update(to: inputGeneration)
        activeInputSelectionID = nil
        activeProcessingID = nil
        activeSaveID = nil
        resetPreparedState()
        source = nil
        let job = RouteDInputSelectionJob(
            id: UUID(),
            generation: inputGeneration,
            selection: selection,
            access: access
        )
        activeInputSelectionID = job.id
        state = .loadingInput
        return job
    }

    @discardableResult
    func acceptInputSelection(
        _ result: Result<ResolvedFCPXMLInput, Error>,
        from job: RouteDInputSelectionJob
    ) -> Bool {
        guard activeInputSelectionID == job.id,
              inputGeneration == job.generation
        else {
            return false
        }
        activeInputSelectionID = nil
        switch result {
        case let .success(input):
            source = input
            state = .inputReady
        case let .failure(error):
            source = nil
            fail(error)
        }
        return true
    }

    func makePreparationJob() -> RouteDPreparationJob? {
        guard let source else {
            fail(SandboxedRouteDWorkflowError.inputRequired)
            return nil
        }
        resetPreparedState()
        let id = UUID()
        activeProcessingID = id
        state = .processing
        return RouteDPreparationJob(
            id: id,
            input: RouteDInputSnapshot(
                generation: inputGeneration,
                source: source
            ),
            access: access,
            processBatch: processBatch
        )
    }

    @discardableResult
    func acceptPreparation(
        _ result: Result<PreparedReplacementProject, Error>,
        from job: RouteDPreparationJob
    ) -> Bool {
        guard activeProcessingID == job.id,
              inputGeneration == job.input.generation
        else {
            return false
        }
        activeProcessingID = nil
        switch result {
        case let .success(prepared):
            preparedProject = prepared
            savedProject = nil
            state = .preview
        case let .failure(error):
            if let workflowError = error as? SandboxedRouteDWorkflowError,
               case let .noReplacementTargets(prepared) = workflowError {
                preparedProject = prepared
                savedProject = nil
                state = .failed(workflowError.localizedDescription)
            } else {
                fail(error)
            }
        }
        return true
    }

    func process() {
        guard let job = makePreparationJob() else {
            return
        }
        acceptPreparation(job.run(), from: job)
    }

    func saveReplacingSource() {
        guard let job = makeSaveJob() else {
            return
        }
        acceptSave(job.run(), from: job)
    }

    func makeSaveJob() -> RouteDSaveJob? {
        guard let preparedProject, state == .preview else {
            fail(SandboxedRouteDWorkflowError.previewRequired)
            return nil
        }
        let id = UUID()
        activeSaveID = id
        state = .saving
        return RouteDSaveJob(
            id: id,
            generation: inputGeneration,
            preparedProject: preparedProject,
            access: access,
            store: store,
            generationGate: saveGenerationGate
        )
    }

    @discardableResult
    func acceptSave(_ result: Result<URL, Error>, from job: RouteDSaveJob) -> Bool {
        guard activeSaveID == job.id, inputGeneration == job.generation else {
            return false
        }
        activeSaveID = nil
        switch result {
        case let .success(destination):
            let warning = open(destination)
                ? nil
                : FinalCutStrings.text(
                    "app.warning.open_saved_failed",
                    fallback: "The XML was replaced, but it could not be opened in Final Cut."
                )
            let saved = SavedReplacementProject(
                destination: destination,
                warning: warning
            )
            savedProject = saved
            state = .saved
        case let .failure(error):
            fail(error)
        }
        return true
    }

    func cancelActiveWork() {
        guard state == .loadingInput || state == .processing || state == .saving else {
            return
        }
        inputGeneration &+= 1
        saveGenerationGate.update(to: inputGeneration)
        activeInputSelectionID = nil
        activeProcessingID = nil
        activeSaveID = nil
        if state == .loadingInput || state == .processing {
            resetPreparedState()
            state = source == nil ? .idle : .inputReady
        } else {
            savedProject = nil
            state = preparedProject == nil ? .inputReady : .preview
        }
    }

    @discardableResult
    func reopenSavedProject() -> Bool {
        guard let savedProject else {
            return false
        }
        let opened = open(savedProject.destination)
        self.savedProject = SavedReplacementProject(
            destination: savedProject.destination,
            warning: opened
                ? nil
                : FinalCutStrings.text(
                    "app.warning.open_saved_failed",
                    fallback: "The XML was replaced, but it could not be opened in Final Cut."
                )
        )
        state = .saved
        return opened
    }

    private func resetPreparedState() {
        preparedProject = nil
        savedProject = nil
    }

    private func fail(_ error: Error) {
        preparedProject = nil
        savedProject = nil
        let message = (error as? LocalizedError)?.errorDescription
            ?? error.localizedDescription
        state = .failed(message)
    }
}

private enum SandboxedRouteDWorkflowError: LocalizedError {
    case inputRequired
    case noReplacementTargets(PreparedReplacementProject)
    case previewRequired

    var errorDescription: String? {
        switch self {
        case .inputRequired:
            return FinalCutStrings.text(
                "app.error.route.input_required",
                fallback: "Select an FCPXML input before processing."
            )
        case .noReplacementTargets:
            return FinalCutStrings.text(
                "app.error.route.no_targets",
                fallback: "The batch did not update any replacement targets."
            )
        case .previewRequired:
            return FinalCutStrings.text(
                "app.error.route.preview_required",
                fallback: "Preview a replacement project before saving."
            )
        }
    }
}
