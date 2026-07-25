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
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    expanded.toggle()
                }
            } label: {
                HStack(alignment: .top, spacing: 8) {
                    statusIcon
                        // Keep the icon on the first line's baseline band even
                        // when the intent wraps to a second line.
                        .frame(height: 18)
                    VStack(alignment: .leading, spacing: 2) {
                        if let intent {
                            // Intent leads: it is the answer to "what is it doing?"
                            Text(intent)
                                .font(.subheadline)
                                .foregroundStyle(Theme.textPrimary)
                                .lineLimit(2)
                                .multilineTextAlignment(.leading)
                            HStack(spacing: 4) {
                                Text(call.name)
                                    .font(Theme.mono(11, weight: .medium))
                                    .foregroundStyle(Theme.textSecondary)
                                if !expanded, let subject {
                                    Text(subject)
                                        .font(Theme.mono(11))
                                        .foregroundStyle(Theme.textTertiary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                }
                            }
                        } else {
                            // No stated intent (older server, or MCP passthrough):
                            // fall back to the previous name + subject line.
                            HStack(spacing: 8) {
                                Text(call.name)
                                    .font(Theme.mono(13, weight: .medium))
                                    .foregroundStyle(Theme.textPrimary)
                                if !expanded, let subject {
                                    Text(subject)
                                        .font(Theme.mono(11))
                                        .foregroundStyle(Theme.textTertiary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                }
                            }
                        }
                    }
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(Theme.textTertiary)
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                        .frame(width: 44, height: 44, alignment: .trailing)
                }
                .contentShape(Rectangle())
            }
            .accessibilityLabel(intent.map { "\($0), tool \(call.name)" } ?? "Tool \(call.name)")
            .accessibilityValue(statusText)
            .accessibilityHint(expanded ? "Collapses the details" : "Expands input and output")
            if expanded {
                if !call.input.isEmpty {
                    codeBlock(call.input)
                }
                if !call.output.isEmpty {
                    codeBlock(String(call.output.prefix(2000)))
                }
                if case let .failed(message) = call.status {
                    Text(message)
                        .font(Theme.mono(12))
                        .foregroundStyle(Theme.error)
                }
            }
        }
        .padding(12)
        .background(Theme.surfaceElevated)
        .clipShape(RoundedRectangle(cornerRadius: 10))
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

    @ViewBuilder
    private var statusIcon: some View {
        switch call.status {
        case .streamingInput, .running:
            ProgressView()
                .controlSize(.mini)
                .tint(Theme.mint)
        case .succeeded:
            Image(systemName: "checkmark.circle.fill")
                .font(.caption)
                .foregroundStyle(Theme.mint)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .font(.caption)
                .foregroundStyle(Theme.error)
        }
    }

    private func codeBlock(_ text: String) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(text)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.textSecondary)
                .padding(8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.background)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}
