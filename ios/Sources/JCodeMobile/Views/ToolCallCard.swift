import JCodeKit
import SwiftUI

/// Collapsible tool call card with live status.
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
                HStack(spacing: 8) {
                    statusIcon
                    Text(call.name)
                        .font(Theme.mono(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                    if !expanded, let summary = inputSummary {
                        Text(summary)
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.textTertiary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(Theme.textTertiary)
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                }
                .contentShape(Rectangle())
            }
            .accessibilityLabel("Tool \(call.name)")
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
        .padding(8)
        .background(Theme.surfaceElevated)
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }

    /// One-line human summary of the tool input for the collapsed header,
    /// so most calls never need expanding (cheaper than a tap + read).
    private var inputSummary: String? {
        let input = call.input
        guard !input.isEmpty else { return nil }
        // Common shape: {"command": "..."} / {"file_path": "..."}; fall back
        // to the raw (single-line) input when it is not JSON.
        if let data = input.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            for key in ["command", "file_path", "path", "query", "url"] {
                if let value = obj[key] as? String, !value.isEmpty {
                    return value
                }
            }
        }
        let flat = input.replacingOccurrences(of: "\n", with: " ")
        return flat.isEmpty ? nil : flat
    }

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
        .background(Theme.background)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}
