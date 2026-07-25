import JCodeKit
import SwiftUI

/// Message composer with send/interrupt.
///
/// Submit behavior (which draft sends, which queues, which is ignored) lives in
/// `ComposerRules` so it is unit tested without a UI. This view only handles
/// focus, the Return key, and rendering.
struct Composer: View {
    @Environment(\.compactEdgePads) private var edgePads
    @Binding var draft: String
    let isProcessing: Bool
    let isConnected: Bool
    let onSend: () -> Void
    let onInterrupt: () -> Void

    @FocusState private var isFocused: Bool

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(
                isProcessing ? "Queue a message..." : "Message",
                text: $draft,
                axis: .vertical
            )
            .lineLimit(1...6)
            .font(.body)
            .foregroundStyle(Theme.textPrimary)
            .focused($isFocused)
            .submitLabel(.send)
            // Hardware keyboards (and iPad/Mac) fire onSubmit for Return.
            .onSubmit(submit)
            // A vertical-axis TextField inserts "\n" for Return on the software
            // keyboard instead of firing onSubmit, so treat a trailing newline
            // as a submit. Pasted multi-line text is unaffected (only a
            // *trailing* newline counts).
            .onChange(of: draft) { _, newValue in
                guard ComposerRules.isReturnKeySubmit(newValue) else { return }
                draft = ComposerRules.normalize(newValue)
                submit()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Theme.surface)
            .clipShape(RoundedRectangle(cornerRadius: 20))
            .overlay(
                RoundedRectangle(cornerRadius: 20)
                    .stroke(Theme.border, lineWidth: 1)
            )

            if isProcessing {
                Button(action: onInterrupt) {
                    Image(systemName: "stop.fill")
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Theme.error)
                        .frame(width: 44, height: 44)
                        .background(Theme.surface)
                        .clipShape(Circle())
                }
                .accessibilityLabel("Stop")
                .accessibilityHint("Interrupt the current response")
            }

            Button(action: submit) {
                Image(systemName: "arrow.up")
                    .font(.body.weight(.bold))
                    .foregroundStyle(isConnected ? .black : Theme.textSecondary)
                    .frame(width: 44, height: 44)
                    .background(isConnected ? Theme.mint : Theme.surfaceElevated)
                    .clipShape(Circle())
            }
            .disabled(!canSend)
            .accessibilityLabel(isProcessing ? "Queue message" : "Send message")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .padding(.bottom, edgePads.bottom)
        .background(Theme.background)
    }

    /// Send if the rules allow it, keeping the keyboard up for the next message.
    private func submit() {
        guard canSend else {
            // Return on an all-whitespace draft: clear it rather than leaving
            // stray newlines behind.
            if !draft.isEmpty, ComposerRules.normalize(draft).isEmpty { draft = "" }
            return
        }
        onSend()
    }

    private var canSend: Bool {
        ComposerRules.canSubmit(draft: draft, isConnected: isConnected)
    }
}
