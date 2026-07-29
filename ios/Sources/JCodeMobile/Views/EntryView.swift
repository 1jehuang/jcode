import JCodeKit
import SwiftUI

/// Friendly placeholder for a fresh session, centered in the canvas.
struct EmptyTranscript: View {
    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "terminal")
                .font(Theme.icon(30, weight: .regular))
                .foregroundStyle(Theme.mint)
                .frame(width: 72, height: 72)
                .background(Theme.surface)
                .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 22, style: .continuous)
                        .stroke(Theme.border, lineWidth: 1)
                )
                .accessibilityHidden(true)
            Text("Ready when you are")
                .font(Theme.mono(17, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            Text("Send a message to start driving this session.")
                .font(.subheadline)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }
}

/// One transcript entry: user bubble, assistant markdown, or system note.
struct EntryView: View {
    let entry: TranscriptEntry

    var body: some View {
        switch entry.role {
        case .user:
            HStack {
                Spacer(minLength: 40)
                VStack(alignment: .trailing, spacing: 5) {
                    Text(entry.text)
                        .font(.body)
                        .foregroundStyle(Theme.textPrimary)
                        .multilineTextAlignment(.leading)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(Theme.userBubble)
                        .clipShape(
                            RoundedRectangle(cornerRadius: Theme.Radius.bubble, style: .continuous)
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.Radius.bubble, style: .continuous)
                                .stroke(Theme.mint.opacity(0.22), lineWidth: 1)
                        )
                        .textSelection(.enabled)
                        .copyContextMenu(entry.text)
                    if entry.isQueued {
                        Label("queued", systemImage: "clock")
                            .font(Theme.mono(10.5))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.trailing, 4)
                            .accessibilityLabel("Queued")
                            .accessibilityHint("Delivers after the current response")
                    }
                }
            }
        case .assistant:
            VStack(alignment: .leading, spacing: 10) {
                if !entry.reasoning.isEmpty {
                    HStack(alignment: .top, spacing: 8) {
                        Rectangle()
                            .fill(Theme.border)
                            .frame(width: 2)
                            .accessibilityHidden(true)
                        Text(entry.reasoning)
                            .font(Theme.mono(11.5))
                            .italic()
                            .foregroundStyle(Theme.textTertiary)
                            .lineLimit(4)
                    }
                    .fixedSize(horizontal: false, vertical: true)
                    .copyContextMenu(entry.reasoning)
                }
                ForEach(entry.toolCalls) { call in
                    ToolCallCard(call: call)
                }
                if !entry.text.isEmpty {
                    MarkdownText(entry.text)
                        .copyContextMenu(entry.text)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .system:
            Text(entry.text)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.textTertiary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(Theme.surface.opacity(0.6))
                .clipShape(Capsule())
                .frame(maxWidth: .infinity, alignment: .center)
                .copyContextMenu(entry.text)
        }
    }
}

extension View {
    /// Long-press context menu offering to copy the given text.
    func copyContextMenu(_ text: String) -> some View {
        contextMenu {
            Button {
                UIPasteboard.general.string = text
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
            }
        }
    }
}
