import JCodeKit
import SwiftUI

/// Friendly placeholder for a fresh session, centered in the canvas.
///
/// Beyond the affordance text, it offers one-tap starter prompts so the first
/// interaction costs a single tap instead of composing from scratch.
struct EmptyTranscript: View {
    var onSuggestion: ((String) -> Void)? = nil

    private static let suggestions = [
        "What's the state of this repo?",
        "Run the tests",
        "Summarize recent changes",
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            BrandMark(size: 36)
                .padding(.bottom, 4)
            Text("New session")
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.textPrimary)
            Text("Send a message to start driving this session.")
                .font(.subheadline)
                .foregroundStyle(Theme.textSecondary)
            if let onSuggestion {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Self.suggestions, id: \.self) { suggestion in
                        Button {
                            onSuggestion(suggestion)
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "arrow.turn.down.right")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(Theme.textTertiary)
                                    .accessibilityHidden(true)
                                Text(suggestion)
                                    .font(.subheadline)
                                    .foregroundStyle(Theme.textPrimary)
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 14)
                            .frame(minHeight: 44)
                            .background(Theme.surfaceElevated)
                            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(PressableButtonStyle(scale: 0.98))
                        .accessibilityHint("Fills the composer with this prompt")
                    }
                }
                .padding(.top, 8)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
        .accessibilityElement(children: .contain)
    }
}

/// One transcript entry, presented like a Jcode Desktop panel: the user's
/// prompt is a numbered, softly tinted card; the assistant's reply sits on the
/// page with no role caption, with reasoning and tool calls inline above it.
struct EntryView: View {
    let entry: TranscriptEntry
    /// 1-based prompt number, matching the CLI and Desktop prompt counter.
    var promptNumber: Int? = nil
    /// How many prompts newer than this one exist (0 = latest prompt).
    var promptDistance: Int = 0

    var body: some View {
        switch entry.role {
        case .user:
            PromptCard(
                text: entry.text,
                number: promptNumber,
                distance: promptDistance,
                isQueued: entry.isQueued
            )
        case .assistant:
            VStack(alignment: .leading, spacing: 8) {
                if !entry.reasoning.isEmpty {
                    ReasoningDisclosure(text: entry.reasoning)
                }
                if !entry.toolCalls.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(entry.toolCalls) { call in
                            ToolCallCard(call: call)
                        }
                    }
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
                .multilineTextAlignment(.leading)
                .frame(maxWidth: .infinity, alignment: .leading)
                .copyContextMenu(entry.text)
        }
    }
}

/// Desktop's prompt card: a small mono prompt-number badge beside a
/// left-aligned card whose tint follows the CLI rainbow, newest in violet.
struct PromptCard: View {
    let text: String
    let number: Int?
    let distance: Int
    let isQueued: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            if let number {
                Text("\(number)")
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(minWidth: 20, minHeight: 20)
                    .padding(.horizontal, number > 9 ? 4 : 0)
                    .background(background)
                    .clipShape(Capsule())
                    .padding(.top, 4)
                    .accessibilityLabel("Prompt \(number)")
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(text)
                    .font(.body)
                    .foregroundStyle(Theme.textPrimary)
                    .multilineTextAlignment(.leading)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(background)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.bubble, style: .continuous))
                    .textSelection(.enabled)
                    .copyContextMenu(text)
                if isQueued {
                    Label("queued", systemImage: "clock")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textTertiary)
                        .padding(.leading, 4)
                        .accessibilityLabel("Queued")
                        .accessibilityHint("Delivers after the current response")
                }
            }
            Spacer(minLength: 24)
        }
    }

    private var background: Color { Theme.promptBackground(distance: distance) }
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

/// Reasoning shown the way Desktop does it: inline, left aligned, in a
/// smaller muted font, with no label, card, or border. On a phone the full
/// thought would bury the answer, so it starts clamped to three lines and a
/// tap expands it.
struct ReasoningDisclosure: View {
    let text: String
    @State private var expanded = false

    var body: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) {
                expanded.toggle()
            }
        } label: {
            Text(text.trimmingCharacters(in: .whitespacesAndNewlines))
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(expanded ? nil : 3)
                .multilineTextAlignment(.leading)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .copyContextMenu(text)
        .accessibilityLabel("Reasoning")
        .accessibilityValue(firstLine)
        .accessibilityHint(expanded ? "Collapses the reasoning" : "Expands the full reasoning")
    }

    private var firstLine: String {
        text.split(separator: "\n", omittingEmptySubsequences: true)
            .first.map(String.init) ?? text
    }
}
