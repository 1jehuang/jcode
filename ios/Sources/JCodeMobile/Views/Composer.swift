import SwiftUI

/// Message composer with send/interrupt.
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
            .tint(Theme.mint)
            .focused($isFocused)
            .padding(.horizontal, 14)
            .padding(.vertical, 11)
            .background(Theme.surface)
            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.bubble, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.bubble, style: .continuous)
                    .stroke(isFocused ? Theme.mint.opacity(0.45) : Theme.border, lineWidth: 1)
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

            Button(action: onSend) {
                Image(systemName: isProcessing ? "arrow.up.to.line" : "arrow.up")
                    .font(.subheadline.weight(.bold))
                    .foregroundStyle(canSend ? Color.black : Theme.textTertiary)
                    .frame(width: 40, height: 40)
                    .background {
                        if canSend {
                            Circle().fill(Theme.mintGradient)
                        } else {
                            Circle().fill(Theme.surfaceElevated)
                        }
                    }
                    .overlay(
                        Circle().stroke(canSend ? .clear : Theme.border, lineWidth: 1)
                    )
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
            ZStack(alignment: .top) {
                Theme.background
                Hairline()
            }
            .ignoresSafeArea(edges: .bottom)
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.8), value: isProcessing)
    }

    private var canSend: Bool {
        isConnected && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}
