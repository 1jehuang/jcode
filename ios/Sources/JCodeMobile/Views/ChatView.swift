import JCodeKit
import SwiftUI

/// Main conversation screen.
struct ChatView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.compactEdgePads) private var edgePads
    @State private var showSettings = false
    @State private var sendCount = 0

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            header

            if showConnectionBanner {
                ConnectionBanner(phase: model.session.phase) {
                    model.retryConnection()
                }
                .padding(.bottom, 8)
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
            // Single-line header: title + model on one baseline keeps chrome
            // lean so the transcript gets the vertical space.
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(model.session.sessionTitle ?? model.activeServer?.serverName ?? "jcode")
                    .font(Theme.mono(16, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                    .layoutPriority(1)
                if let modelName = model.session.modelName {
                    Text(shortModelName(modelName))
                        .font(Theme.mono(11))
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer()
            StatusPill(phase: model.session.phase)
            Button {
                showSettings = true
            } label: {
                Image(systemName: "ellipsis")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 44, height: 44)
                    .background(Theme.surfaceElevated)
                    .clipShape(Circle())
                    .overlay(Circle().stroke(Theme.border, lineWidth: 1))
            }
            .accessibilityLabel("Settings")
            .accessibilityHint("Sessions, model, and servers")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .padding(.top, edgePads.top)
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
