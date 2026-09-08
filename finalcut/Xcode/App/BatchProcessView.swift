import SwiftUI

struct BatchProcessView: View {
    @StateObject private var model = FinalCutAppModel()
    @State private var processedVideosExpanded = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                inputStage

                if let report = model.preparedProject?.report {
                    reportStage(report)
                }

                if let saved = model.savedProject,
                   let warning = saved.warning {
                    openRecovery(warning)
                }

            }
            .frame(maxWidth: 680, alignment: .leading)
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        .frame(minWidth: 520, idealWidth: 580, minHeight: 180, idealHeight: 240)
        .task { model.prepareTemplateIfNeeded() }
    }

    private var inputStage: some View {
        stageContainer(title: FinalCutStrings.text("app.step.choose_fcpxml")) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 12) {
                    if let selection = model.selectedFCPXML {
                        Label(selection.lastPathComponent, systemImage: "doc")
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(selection.path)
                            .textSelection(.enabled)
                            .accessibilityLabel(FinalCutStrings.format(
                                "app.accessibility.selected_fcpxml",
                                selection.lastPathComponent
                            ))
                    } else {
                        Text(FinalCutStrings.text("app.selection.no_fcpxml"))
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    currentActionButton(
                        FinalCutStrings.text("app.action.choose_fcpxml"),
                        systemImage: "doc.badge.plus",
                        isCurrent: model.workflowStage == .selectFCPXML,
                        disabled: model.isBusy,
                        action: {
                            processedVideosExpanded = false
                            model.chooseFCPXML()
                        }
                    )
                }

                if model.isBusy {
                    HStack(spacing: 10) {
                        ProgressView()
                            .controlSize(.small)
                        Text(model.workflowMessage)
                            .foregroundStyle(.secondary)
                        Spacer(minLength: 8)
                        Button(FinalCutStrings.text("app.action.cancel")) {
                            model.cancelActiveWork()
                        }
                    }
                    .accessibilityElement(children: .combine)
                }
                if let message = model.errorMessage ?? model.templatePreparationError {
                    Text(message)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .accessibilityLabel(message)
                }
            }
        }
    }

    private func reportStage(_ report: RouteDBatchReport) -> some View {
        DisclosureGroup(isExpanded: $processedVideosExpanded) {
            LazyVStack(alignment: .leading, spacing: 8) {
                ForEach(report.targets, id: \.occurrence) { target in
                    BatchTargetStatusRow(target: target, expandedByDefault: false)
                }
            }
            .padding(.top, 8)
        } label: {
            Label(
                FinalCutStrings.plural(
                    "app.label.processed_videos",
                    count: report.occurrenceCount
                ),
                systemImage: "film.stack"
            )
            .font(.headline)
        }
        .padding(12)
        .background(Color.secondary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
    }

    private func openRecovery(_ warning: String) -> some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                Label(warning, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                HStack(spacing: 8) {
                    currentActionButton(
                        FinalCutStrings.text("app.action.retry_open"),
                        systemImage: "play.rectangle.on.rectangle",
                        isCurrent: true,
                        disabled: false,
                        action: model.retryOpenSavedProject
                    )
                    Button {
                        model.revealSavedProject()
                    } label: {
                        Label(
                            FinalCutStrings.text("app.action.reveal_output"),
                            systemImage: "folder"
                        )
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func stageContainer<Content: View>(
        title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        GroupBox {
            content()
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 4)
        } label: {
            Text(title)
                .font(.headline)
        }
    }

    @ViewBuilder
    private func currentActionButton(
        _ title: String,
        systemImage: String,
        isCurrent: Bool,
        disabled: Bool,
        action: @escaping () -> Void
    ) -> some View {
        if isCurrent {
            Button(action: action) {
                Label(title, systemImage: systemImage)
                    .frame(minHeight: 24)
            }
            .buttonStyle(.borderedProminent)
            .disabled(disabled)
        } else {
            Button(action: action) {
                Label(title, systemImage: systemImage)
                    .frame(minHeight: 24)
            }
            .buttonStyle(.bordered)
            .disabled(disabled)
        }
    }
}

private struct BatchTargetStatusRow: View {
    let target: RouteDBatchTarget
    @State private var detailsExpanded: Bool

    init(target: RouteDBatchTarget, expandedByDefault: Bool) {
        self.target = target
        _detailsExpanded = State(initialValue: expandedByDefault)
    }

    var body: some View {
        DisclosureGroup(isExpanded: $detailsExpanded) {
            VStack(alignment: .leading, spacing: 4) {
                Text(target.expectedProjectText)
                if target.detail != target.statusDetail {
                    Text(target.detail)
                }
                ForEach(target.geometryReasons, id: \.self) { reason in
                    Text(reason)
                }
                if let detail = target.geometryDetail, !detail.isEmpty {
                    Text(detail)
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .textSelection(.enabled)
            .padding(.top, 4)
        } label: {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: symbolName)
                    .foregroundStyle(symbolColor)
                    .frame(width: 18)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(target.structuralIdentity)
                        .font(.callout.weight(.medium))
                    Text(target.statusDetail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(target.accessibilitySummary)
        .padding(8)
        .background(Color.secondary.opacity(0.06), in: RoundedRectangle(cornerRadius: 6))
    }

    private var symbolName: String {
        switch target.action {
        case .updatedProject:
            return "checkmark.circle"
        case .timingOnly:
            return "clock.arrow.circlepath"
        case .skipped:
            return "exclamationmark.triangle"
        }
    }

    private var symbolColor: Color {
        target.action == .skipped ? .orange : .secondary
    }
}
