import JCodeKit
import SwiftUI

/// Collapsible tool call card with live status.
///
/// The header leads with the agent's stated intent ("Verify the build passes")
/// rather than only the tool name, so scanning a transcript tells you *why*
/// each call happened without expanding anything. Extraction lives in
/// `ToolCallSummary` (unit tested, streaming-tolerant).
struct ToolCallCard: View {
    let call: TranscriptEntry.ToolCall
    // `-jcodeExpandTools YES` starts rows expanded (screenshot tooling only).
    @State private var expanded = UserDefaults.standard.bool(forKey: "jcodeExpandTools")
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    expanded.toggle()
                }
            } label: {
                // Desktop presents tool calls inline, with no contrasting card:
                // one monochrome glyph whose color carries status, then text.
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    statusGlyph
                    VStack(alignment: .leading, spacing: 1) {
                        if let intent {
                            Text(intent)
                                .font(.subheadline)
                                .foregroundStyle(Theme.textPrimary)
                                .lineLimit(2)
                                .multilineTextAlignment(.leading)
                        }
                        HStack(spacing: 6) {
                            Text(call.name)
                                .font(Theme.mono(intent == nil ? 13 : 11, weight: .medium))
                                .foregroundStyle(intent == nil ? Theme.textPrimary : Theme.textSecondary)
                            if !expanded, let subject {
                                Text(subject)
                                    .font(Theme.mono(11))
                                    .foregroundStyle(Theme.textTertiary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                        }
                    }
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(Theme.textTertiary)
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                        .accessibilityHidden(true)
                }
                .padding(.vertical, 6)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(intent.map { "\($0), tool \(call.name)" } ?? "Tool \(call.name)")
            .accessibilityValue(statusText)
            .accessibilityHint(expanded ? "Collapses the details" : "Expands input and output")
            if expanded {
                VStack(alignment: .leading, spacing: 8) {
                    if !call.input.isEmpty {
                        codeText(call.input)
                    }
                    if !call.input.isEmpty && !call.output.isEmpty {
                        Hairline()
                    }
                    if !call.output.isEmpty {
                        codeText(String(call.output.prefix(2000)))
                    }
                    if case let .failed(message) = call.status {
                        Text(message)
                            .font(Theme.mono(12))
                            .foregroundStyle(Theme.error)
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.codeBackground)
                .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                .padding(.leading, 22)
                .padding(.bottom, 6)
            }
        }
    }

    /// The agent's stated reason for this call, when it provided one.
    private var intent: String? { ToolCallSummary.intent(from: call.input) }

    /// What the call operates on (command, file, query) for the secondary line.
    private var subject: String? { ToolCallSummary.subject(from: call.input) }

    private var statusText: String {
        switch call.status {
        case .streamingInput, .running: "Running"
        case .succeeded: "Succeeded"
        case .failed: "Failed"
        }
    }

    private var isRunning: Bool {
        switch call.status {
        case .streamingInput, .running: true
        case .succeeded, .failed: false
        }
    }

    /// Desktop's rule: the tool glyph itself carries status (warn while
    /// running, ok when done, error on failure) with a gentle pulse while live.
    private var statusGlyph: some View {
        let color: Color = switch call.status {
        case .streamingInput, .running: Theme.warning
        case .succeeded: Theme.ok
        case .failed: Theme.error
        }
        return Image(systemName: Self.symbol(for: call.name))
            .font(.caption.weight(.medium))
            .foregroundStyle(color)
            .frame(width: 14)
            .phaseAnimator(isRunning && !reduceMotion ? [1.0, 0.45] : [1.0]) { glyph, opacity in
                glyph.opacity(opacity)
            } animation: { _ in .easeInOut(duration: 0.7) }
            .accessibilityHidden(true)
    }

    /// SF Symbol stand-ins for Desktop's embedded monochrome tool icons.
    static func symbol(for name: String) -> String {
        switch name {
        case "bash": "terminal"
        case "read": "doc.text"
        case "write": "square.and.pencil"
        case "edit", "multiedit", "patch", "apply_patch": "pencil"
        case "ls", "glob", "find": "folder"
        case "grep", "agentgrep": "magnifyingglass"
        case "websearch": "globe"
        case "webfetch", "browser": "safari"
        case "memory": "brain"
        case "todo", "todowrite", "todoread": "checklist"
        default: name.contains(":") ? "puzzlepiece.extension" : "wrench.and.screwdriver"
        }
    }

    private func codeText(_ text: String) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(text)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.textSecondary)
                .textSelection(.enabled)
        }
    }
}
