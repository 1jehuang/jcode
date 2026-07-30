import JCodeKit
import SwiftUI

/// Collapsible tool call card with live status.
struct ToolCallCard: View {
    let call: TranscriptEntry.ToolCall
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    expanded.toggle()
                }
            } label: {
                HStack(spacing: 8) {
                    statusIcon
                        .frame(width: 16, height: 16)
                    Text(call.name)
                        .font(Theme.mono(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    if !expanded, let summary = inputSummary {
                        Text(summary)
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.textTertiary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(Theme.textTertiary)
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Tool \(call.name)")
            .accessibilityValue(statusLabel)
            .accessibilityHint(expanded ? "Collapses details" : "Expands input and output")
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
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Theme.surface)
        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                .stroke(accent, lineWidth: 1)
        )
    }

    /// Border tint hints at status without shouting.
    private var accent: Color {
        switch call.status {
        case .streamingInput, .running: Theme.mint.opacity(0.3)
        case .failed: Theme.error.opacity(0.3)
        case .succeeded: Theme.border
        }
    }

    private var statusLabel: String {
        switch call.status {
        case .streamingInput: "Preparing"
        case .running: "Running"
        case .succeeded: "Succeeded"
        case .failed: "Failed"
        }
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
                .padding(10)
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.background)
        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous))
    }
}
