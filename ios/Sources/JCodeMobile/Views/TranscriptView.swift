import JCodeKit
import SwiftUI

/// Scrolling transcript with pinned-to-bottom auto-follow.
///
/// Auto-scroll only engages while the user is at (or near) the bottom, so
/// scrolling up to read history is never hijacked by streaming output. A
/// jump-to-latest button appears whenever the view is unpinned. Short threads
/// are anchored to the bottom (chat convention); once the content exceeds the
/// viewport it scrolls normally. An empty session shows a centered placeholder.
struct TranscriptView: View {
    let entries: [TranscriptEntry]
    let isReasoning: Bool

    /// True while the viewport is at (or near) the bottom of the content.
    @State private var isPinnedToBottom = true

    /// Distance from the bottom below which the view counts as pinned.
    private static let pinThreshold: CGFloat = 56

    var body: some View {
        if entries.isEmpty && !isReasoning {
            EmptyTranscript()
        } else {
            GeometryReader { viewport in
                scroller(viewportHeight: viewport.size.height)
            }
        }
    }

    private func scroller(viewportHeight: CGFloat) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                // A flexible top spacer pushes short content to the bottom of
                // the viewport; it collapses to zero once content overflows.
                LazyVStack(alignment: .leading, spacing: 16) {
                    Spacer(minLength: 0)
                    ForEach(entries) { entry in
                        EntryView(entry: entry)
                            .id(entry.id)
                    }
                    if isReasoning {
                        thinkingRow
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .frame(minHeight: max(0, viewportHeight - 16), alignment: .bottom)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(
                    GeometryReader { content in
                        Color.clear.preference(
                            key: BottomDistanceKey.self,
                            value: content.frame(in: .named("transcript")).maxY
                                - viewportHeight
                        )
                    }
                )
            }
            .coordinateSpace(name: "transcript")
            .scrollDismissesKeyboard(.interactively)
            .onPreferenceChange(BottomDistanceKey.self) { distance in
                MainActor.assumeIsolated {
                    let pinned = distance < Self.pinThreshold
                    if pinned != isPinnedToBottom {
                        isPinnedToBottom = pinned
                    }
                }
            }
            .onChange(of: entries.last?.text) {
                guard isPinnedToBottom else { return }
                proxy.scrollTo("bottom", anchor: .bottom)
            }
            .onChange(of: entries.count) {
                // Follow new entries when pinned; always follow the user's
                // own sends so their message never lands off-screen.
                if isPinnedToBottom || entries.last?.role == .user {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .overlay(alignment: .bottomTrailing) {
                if !isPinnedToBottom {
                    ScrollToBottomButton {
                        withAnimation(.easeOut(duration: 0.15)) {
                            proxy.scrollTo("bottom", anchor: .bottom)
                        }
                    }
                    .padding(.trailing, 16)
                    .padding(.bottom, 8)
                }
            }
        }
    }

    private var thinkingRow: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
                .tint(Theme.textTertiary)
            Text("thinking")
                .font(Theme.mono(12))
                .foregroundStyle(Theme.textTertiary)
        }
    }
}

/// How far the content's bottom edge sits below the viewport's bottom edge.
private struct BottomDistanceKey: PreferenceKey {
    static let defaultValue: CGFloat = 0

    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

/// Floating jump-to-latest affordance shown while scrolled up.
private struct ScrollToBottomButton: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "arrow.down")
                .font(.body.weight(.semibold))
                .foregroundStyle(Theme.textPrimary)
                .frame(width: 44, height: 44)
                .background(Theme.surfaceElevated)
                .clipShape(Circle())
                .overlay(Circle().stroke(Theme.border, lineWidth: 1))
        }
        .accessibilityLabel("Scroll to bottom")
        .accessibilityHint("Jumps to the latest message")
        .transition(.opacity)
    }
}
