import Foundation

struct RouteDBatchTarget: Codable, Equatable, Identifiable {
    let occurrence: Int?
    let clipName: String
    let assetRef: String?
    let mediaURL: String?
    let action: String
    let detail: String
    let geometryStatus: String?
    let geometryReasons: [String]?
    let geometryDetail: String?

    var id: String {
        "\(occurrence.map(String.init) ?? "none")|\(clipName)|\(assetRef ?? "none")|\(action)"
    }

    enum CodingKeys: String, CodingKey {
        case occurrence
        case clipName = "clip_name"
        case assetRef = "asset_ref"
        case mediaURL = "media_url"
        case action
        case detail
        case geometryStatus = "geometry_status"
        case geometryReasons = "geometry_reasons"
        case geometryDetail = "geometry_detail"
    }
}

struct RouteDBatchReport: Codable, Equatable {
    let originalProjectName: String
    let processedProjectName: String
    let importToken: String
    let occurrenceCount: Int
    let insertedCount: Int
    let updatedCount: Int
    let skippedCount: Int
    let failedCount: Int
    let targets: [RouteDBatchTarget]

    enum CodingKeys: String, CodingKey {
        case originalProjectName = "original_project_name"
        case processedProjectName = "processed_project_name"
        case importToken = "import_token"
        case occurrenceCount = "occurrence_count"
        case insertedCount = "inserted_count"
        case updatedCount = "updated_count"
        case skippedCount = "skipped_count"
        case failedCount = "failed_count"
        case targets
    }
}

struct RouteDBatchProcessorOutput {
    let fcpxml: Data
    let report: Data
}

struct PreparedRouteDProject: Equatable {
    let source: ResolvedFCPXMLInput
    let fcpxml: Data
    let report: RouteDBatchReport
}

enum OneClickRouteDState: Equatable {
    case idle
    case exporting
    case processing
    case preview
    case importing
    case completed(URL)
    case failed(String)

    var isFailure: Bool {
        if case .failed = self {
            return true
        }
        return false
    }
}

struct OneClickRouteDDependencies {
    let makeWorkspace: () throws -> UniqueExportWorkspace
    let exportCurrentProject: (UniqueExportWorkspace) throws -> ResolvedFCPXMLInput
    let resolveManualInput: (URL) throws -> ResolvedFCPXMLInput
    let process: (Data, String) throws -> RouteDBatchProcessorOutput
    let write: (Data, URL, ResolvedFCPXMLInput) throws -> Void
    let open: (URL) -> Bool
}

final class OneClickRouteDWorkflow {
    private let dependencies: OneClickRouteDDependencies

    var onStateChange: ((OneClickRouteDState) -> Void)?
    private(set) var preparedProject: PreparedRouteDProject?
    private(set) var failedTargets: [RouteDBatchTarget] = []
    private(set) var state: OneClickRouteDState = .idle {
        didSet {
            onStateChange?(state)
        }
    }

    init(dependencies: OneClickRouteDDependencies) {
        self.dependencies = dependencies
    }

    func processCurrentProject(processedName: String) {
        resetPreparedState()
        state = .exporting
        do {
            let workspace = try dependencies.makeWorkspace()
            let input = try dependencies.exportCurrentProject(workspace)
            try prepare(input: input, processedName: processedName)
        } catch {
            fail(error)
        }
    }

    func processManual(selection: URL, processedName: String) {
        resetPreparedState()
        do {
            let input = try dependencies.resolveManualInput(selection)
            try prepare(input: input, processedName: processedName)
        } catch {
            fail(error)
        }
    }

    func confirmImport(to destination: URL) {
        guard state == .preview, let preparedProject else {
            return
        }
        state = .importing
        do {
            try dependencies.write(
                preparedProject.fcpxml,
                destination,
                preparedProject.source
            )
            guard dependencies.open(destination) else {
                throw CocoaError(.fileNoSuchFile)
            }
            state = .completed(destination.standardizedFileURL)
        } catch {
            fail(error)
        }
    }

    func cancelPreview() {
        preparedProject = nil
        failedTargets = []
        state = .idle
    }

    private func prepare(
        input: ResolvedFCPXMLInput,
        processedName: String
    ) throws {
        state = .processing
        let output = try dependencies.process(input.data, processedName)
        let report = try JSONDecoder().decode(RouteDBatchReport.self, from: output.report)
        guard report.failedCount == 0 else {
            throw CocoaError(.fileReadCorruptFile)
        }
        preparedProject = PreparedRouteDProject(
            source: input,
            fcpxml: output.fcpxml,
            report: report
        )
        state = .preview
    }

    private func resetPreparedState() {
        preparedProject = nil
        failedTargets = []
    }

    private func fail(_ error: Error) {
        preparedProject = nil
        let message = (error as? LocalizedError)?.errorDescription
            ?? error.localizedDescription
        failedTargets = [
            RouteDBatchTarget(
                occurrence: nil,
                clipName: "Batch processing",
                assetRef: nil,
                mediaURL: nil,
                action: "failed",
                detail: message,
                geometryStatus: "blocked",
                geometryReasons: [message],
                geometryDetail: nil
            )
        ]
        state = .failed(message)
    }
}
