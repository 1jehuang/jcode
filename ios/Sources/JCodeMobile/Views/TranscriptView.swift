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
    /// True while the server is working on a turn.
    var isProcessing: Bool = false
    var onSuggestion: ((String) -> Void)? = nil

    /// True while the viewport is at (or near) the bottom of the content.
    @State private var isPinnedToBottom = true

    /// Distance from the bottom below which the view counts as pinned.
    private static let pinThreshold: CGFloat = 56

    var body: some View {
        if entries.isEmpty && !isReasoning {
            EmptyTranscript(onSuggestion: onSuggestion)
        } else {
            GeometryReader { viewport in
                scroller(viewportHeight: viewport.size.height)
            }
        }
    }

    private func scroller(viewportHeight: CGFloat) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                // Content reads top-down like a document; autoscroll keeps
                // the latest entry visible once content overflows.
                LazyVStack(alignment: .leading, spacing: 16) {
                    let numbers = promptNumbers
                    ForEach(entries) { entry in
                        EntryView(
                            entry: entry,
                            promptNumber: numbers[entry.id],
                            promptDistance: numbers[entry.id].map { promptCount - $0 } ?? 0
                        )
                        .id(entry.id)
                    }
                    if let activity {
                        ActivityLabel(text: activity)
                            .padding(.top, 2)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
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

    /// Desktop's activity label: Thinking, Running tools, or Responding.
    private var activity: String? {
        if isReasoning { return "Thinking" }
        guard isProcessing else { return nil }
        guard let last = entries.last, last.role == .assistant else { return "Working" }
        let toolRunning = last.toolCalls.contains { call in
            switch call.status {
            case .streamingInput, .running: true
            case .succeeded, .failed: false
            }
        }
        if toolRunning { return "Running tools" }
        return last.text.isEmpty ? "Working" : "Responding"
    }

    /// 1-based numbers for user prompts, as the CLI and Desktop count them.
    private var promptNumbers: [UUID: Int] {
        var result: [UUID: Int] = [:]
        var n = 0
        for entry in entries where entry.role == .user {
            n += 1
            result[entry.id] = n
        }
        return result
    }

    private var promptCount: Int {
        entries.reduce(0) { $0 + ($1.role == .user ? 1 : 0) }
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
                .font(.subheadline.weight(.bold))
                .foregroundStyle(Theme.textPrimary)
                .frame(width: 38, height: 38)
                .background(Theme.surfaceElevated)
                .clipShape(Circle())
                .overlay(Circle().stroke(Theme.border, lineWidth: 1))
                .shadow(color: .black.opacity(0.35), radius: 8, y: 3)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(PressableButtonStyle())
        .accessibilityLabel("Scroll to bottom")
        .accessibilityHint("Jumps to the latest message")
        .transition(.scale(scale: 0.8).combined(with: .opacity))
    }
}

/// Desktop's activity indicator: a small rotating ring of eight dots whose
/// highlight steps around, beside a muted label. Reduce Motion gets a static
/// marker.
struct ActivityLabel: View {
    let text: String
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 8) {
            if reduceMotion {
                Circle()
                    .fill(Theme.accent)
                    .frame(width: 6, height: 6)
                    .frame(width: 14, height: 14)
            } else {
                TimelineView(.periodic(from: .now, by: 0.125)) { context in
                    DotSpinner(step: Int(context.date.timeIntervalSinceReferenceDate * 8) % 8)
                }
            }
            Text(text)
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(text)
    }
}

/// Eight dots on a ring; the highlight rotates one dot per step.
struct DotSpinner: View {
    let step: Int

    var body: some View {
        ZStack {
            ForEach(0..<8, id: \.self) { i in
                let age = (step - i + 8) % 8
                Circle()
                    .fill(Theme.accent)
                    .frame(width: 3, height: 3)
                    .opacity(max(0.18, 1 - Double(age) * 0.14))
                    .offset(y: -5.5)
                    .rotationEffect(.degrees(Double(i) * 45))
            }
        }
        .frame(width: 14, height: 14)
        .accessibilityHidden(true)
    }
}
