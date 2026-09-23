import JCodeKit
import SwiftUI

/// Message composer with send/interrupt.
///
/// Submit behavior (which draft sends, which queues, which is ignored) lives in
/// `ComposerRules` so it is unit tested without a UI. This view only handles
/// focus, the Return key, and rendering.
struct Composer: View {
    @Environment(\.compactEdgePads) private var edgePads
    @FocusState private var isFocused: Bool
    @Binding var draft: String
    let isProcessing: Bool
    let isConnected: Bool
    let onSend: () -> Void
    let onInterrupt: () -> Void

    var body: some View {
        HStack(alignment: .bottom, spacing: 10) {
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
            .tint(Theme.textPrimary)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(Theme.background)
            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                    .stroke(isFocused ? Theme.borderFocus : Theme.border, lineWidth: 1)
            )
            .animation(.easeOut(duration: 0.15), value: isFocused)

            if isProcessing {
                Button(action: onInterrupt) {
                    Image(systemName: "stop.fill")
                        .font(.subheadline.weight(.bold))
                        .foregroundStyle(Theme.error)
                        .frame(width: 40, height: 40)
                        .background(Theme.error.opacity(0.14))
                        .clipShape(Circle())
                        .overlay(Circle().stroke(Theme.error.opacity(0.32), lineWidth: 1))
                        .frame(width: 44, height: 44)
                        .contentShape(Circle())
                }
                .buttonStyle(PressableButtonStyle())
                .accessibilityLabel("Stop")
                .accessibilityHint("Interrupt the current response")
                .transition(.scale.combined(with: .opacity))
            }

            Button(action: submit) {
                Image(systemName: "arrow.up")
                    .font(.body.weight(.bold))
                    .foregroundStyle(canSend ? Theme.onAccent : Theme.textTertiary)
                    .frame(width: 36, height: 36)
                    .background(canSend ? Theme.accent : Theme.surfaceElevated)
                    .clipShape(Circle())
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .buttonStyle(PressableButtonStyle())
            .disabled(!canSend)
            .animation(.easeOut(duration: 0.15), value: canSend)
            .accessibilityLabel(isProcessing ? "Queue message" : "Send message")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .padding(.bottom, edgePads.bottom)
        .background(alignment: .top) {
            Theme.surface
                .ignoresSafeArea(edges: .bottom)
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.8), value: isProcessing)
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
