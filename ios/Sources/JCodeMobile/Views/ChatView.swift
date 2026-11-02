import JCodeKit
import SwiftUI

/// Main conversation screen.
struct ChatView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.compactEdgePads) private var edgePads
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var showSettings = false
    @State private var sendCount = 0
    @State private var bannerVisible = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            header

            if bannerVisible {
                ConnectionBanner(phase: model.session.phase) {
                    model.retryConnection()
                }
                .padding(.bottom, 8)
                .transition(reduceMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
            }

            if let banner = model.session.errorBanner {
                ErrorBanner(message: banner) {
                    model.dismissError()
                }
                .padding(.bottom, 8)
            }

            if !model.session.notices.isEmpty {
                NoticeStack(
                    notices: model.session.notices,
                    onDismiss: { model.dismissNotice($0) }
                )
                .padding(.bottom, 8)
            }

            TranscriptView(
                entries: model.session.transcript,
                isReasoning: model.session.isReasoning,
                onSuggestion: { model.draft = $0 }
            )

            if model.session.hasPendingInterrupts {
                QueuedInterruptChip(count: model.session.pendingInterrupts.count) {
                    model.cancelQueuedInterrupts()
                }
                .padding(.bottom, 8)
            }

            Composer(
                draft: $model.draft,
                isProcessing: model.session.isProcessing,
                isConnected: model.isConnected,
                onSend: {
                    sendCount += 1
                    model.sendDraft()
                },
                onInterrupt: { model.interrupt() }
            )
        }
        .sheet(isPresented: $showSettings) {
            SettingsView()
        }
        .task(id: connectionBannerDelay) {
            if let delay = connectionBannerDelay, delay > 0 {
                try? await Task.sleep(nanoseconds: delay)
                if Task.isCancelled { return }
            }
            withAnimation(.easeInOut(duration: 0.3)) {
                bannerVisible = connectionBannerDelay != nil
            }
        }
        .sensoryFeedback(.impact(weight: .light), trigger: sendCount)
        .sensoryFeedback(.impact(flexibility: .soft), trigger: finishedToolCallCount) {
            $1 > $0
        }
        .sensoryFeedback(.error, trigger: model.session.errorBanner) {
            $1 != nil
        }
    }

    private var connectionBannerDelay: UInt64? {
        switch model.session.phase {
        case .reconnecting: 2_000_000_000
        case .disconnected, .failed: 0
        case .connected, .connecting: nil
        }
    }

    /// Finished tool calls on the streaming (last) entry; drives a subtle
    /// tick as tools complete without scanning the whole transcript.
    private var finishedToolCallCount: Int {
        model.session.transcript.last?.toolCalls.filter { call in
            switch call.status {
            case .succeeded, .failed: true
            case .streamingInput, .running: false
            }
        }.count ?? 0
    }

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.session.sessionTitle ?? model.activeServer?.serverName ?? "jcode")
                    .font(Theme.mono(15, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let modelName = model.session.modelName {
                    Text(shortModelName(modelName))
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: 8)
            StatusPill(phase: model.session.phase)
            Button {
                showSettings = true
            } label: {
                Image(systemName: "ellipsis")
                    .font(.subheadline.weight(.bold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 36, height: 36)
                    .background(Theme.surface)
                    .clipShape(Circle())
                    .overlay(Circle().stroke(Theme.border, lineWidth: 1))
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .buttonStyle(PressableButtonStyle())
            .accessibilityLabel("Settings")
            .accessibilityHint("Sessions, model, and servers")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .padding(.top, edgePads.top)
        .background(alignment: .bottom) {
            ZStack(alignment: .bottom) {
                Theme.background
                Theme.chrome
                Hairline()
            }
            .ignoresSafeArea(edges: .top)
        }
    }

    /// Strips the auth-route prefix ("claude-api:claude-fable-5" -> "claude-fable-5")
    /// so the header shows the model, not plumbing.
    private func shortModelName(_ name: String) -> String {
        if let idx = name.firstIndex(of: ":"), idx != name.startIndex {
            return String(name[name.index(after: idx)...])
        }
        return name
    }
}
