import JCodeKit
import SwiftUI

/// Main conversation screen.
struct ChatView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.compactEdgePads) private var edgePads
    @State private var showSessions = false
    @State private var showModelPicker = false
    @State private var sendCount = 0

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            header

            if model.isDemo {
                DemoBanner { model.exitDemo() }
                    .readableColumn()
                    .padding(.bottom, 8)
            }

            if showConnectionBanner {
                ConnectionBanner(phase: model.session.phase) {
                    model.retryConnection()
                }
                .readableColumn()
                .padding(.bottom, 8)
            }

            if let banner = model.session.errorBanner {
                ErrorBanner(message: banner) {
                    model.dismissError()
                }
                .readableColumn()
                .padding(.bottom, 8)
            }

            if !model.session.notices.isEmpty {
                NoticeStack(
                    notices: model.session.notices,
                    onDismiss: { model.dismissNotice($0) }
                )
                .readableColumn()
                .padding(.bottom, 8)
            }

            TranscriptView(
                entries: model.session.transcript,
                isReasoning: model.session.isReasoning,
                onSuggestion: { model.draft = $0 }
            )
            .readableColumn()

            if model.session.hasPendingInterrupts {
                QueuedInterruptChip(count: model.session.pendingInterrupts.count) {
                    model.cancelQueuedInterrupts()
                }
                .readableColumn()
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
            .readableColumn()
        }
        .sheet(isPresented: $showSessions) {
            SessionsView()
        }
        .sheet(isPresented: $showModelPicker) {
            ModelPickerView()
        }
        .sensoryFeedback(.impact(weight: .light), trigger: sendCount)
        .sensoryFeedback(.impact(flexibility: .soft), trigger: finishedToolCallCount) {
            $1 > $0
        }
        .sensoryFeedback(.error, trigger: model.session.errorBanner) {
            $1 != nil
        }
    }

    private var showConnectionBanner: Bool {
        switch model.session.phase {
        case .reconnecting, .disconnected, .failed: true
        case .connected, .connecting: false
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
        HStack(spacing: 12) {
            // Sessions live top-left: switching session is the most frequent
            // navigation, and the leading edge is the cheapest thumb target for
            // a "go somewhere else" action (matching iOS back/menu convention).
            Button {
                showSessions = true
            } label: {
                Image(systemName: "line.3.horizontal")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 36, height: 36)
                    .background(Theme.surface)
                    .clipShape(Circle())
                    .overlay(Circle().stroke(Theme.border, lineWidth: 1))
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .accessibilityLabel("Sessions")
            .accessibilityHint("Switch session, manage servers, and open settings")

            // Title + model on one baseline keeps chrome lean so the transcript
            // gets the vertical space. Tapping the model opens the picker, so
            // changing model is one tap from the conversation.
            Button {
                showModelPicker = true
            } label: {
                VStack(alignment: .leading, spacing: 0) {
                    Text(model.session.sessionTitle ?? model.activeServer?.serverName ?? "jcode")
                        .font(Theme.mono(15, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    HStack(spacing: 4) {
                        Text(shortModelName(model.session.modelName ?? "model"))
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.textTertiary)
                            .lineLimit(1)
                            .truncationMode(.head)
                        Image(systemName: "chevron.down")
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(Theme.textTertiary)
                            .accessibilityHidden(true)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Model \(shortModelName(model.session.modelName ?? "unknown"))")
            .accessibilityHint("Opens the model picker")

            StatusPill(phase: model.session.phase)
        }
        .padding(.horizontal, 16)
        .readableColumn()
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
