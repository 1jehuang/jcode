import JCodeKit
import SwiftUI

/// Top-level router: pairing when no server, chat otherwise.
struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var deepLinkError: String?

    var body: some View {
        GeometryReader { proxy in
            ZStack {
                Theme.background.ignoresSafeArea()
                if model.activeServer == nil {
                    PairingView()
                } else {
                    ChatView()
                }
            }
            .environment(\.compactEdgePads, CompactEdgePads(safeArea: proxy.safeAreaInsets))
        }
        .task {
            // Auto-connect to the most recent server on launch.
            if let server = model.activeServer, !model.isConnected {
                model.connect(to: server)
            }
        }
        .onOpenURL { url in
            guard let payload = PairURI.parse(url.absoluteString) else { return }
            Task {
                do {
                    try await model.pair(
                        gateway: payload.gateway,
                        code: payload.code,
                        deviceName: UIDevice.current.name
                    )
                } catch {
                    deepLinkError = "Pairing failed: \(error.localizedDescription)"
                }
            }
        }
        .alert("Pairing", isPresented: .init(
            get: { deepLinkError != nil },
            set: { if !$0 { deepLinkError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(deepLinkError ?? "")
        }
    }
}

/// Connection status shown in the chat header.
///
/// Connected is the steady state, so it renders as a single calm mint dot;
/// any degraded phase gets a labeled pill so the words appear exactly when
/// they carry information.
struct StatusPill: View {
    let phase: ConnectionPhase

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(color)
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            if phase != .connected {
                Text(label)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.textSecondary)
            }
        }
        .padding(.horizontal, phase == .connected ? 0 : 12)
        .padding(.vertical, 4)
        .background(phase == .connected ? .clear : Theme.surface)
        .clipShape(Capsule())
        .overlay(
            Capsule().stroke(
                phase == .connected ? .clear : Theme.border, lineWidth: 1)
        )
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Connection")
        .accessibilityValue(label)
    }

    private var color: Color {
        switch phase {
        case .connected: Theme.mint
        case .connecting, .reconnecting: Theme.warning
        case .disconnected, .failed: Theme.error
        }
    }

    private var label: String {
        switch phase {
        case .connected: "live"
        case .connecting: "connecting"
        case .reconnecting(let attempt): "retry \(attempt)"
        case .disconnected: "offline"
        case .failed: "failed"
        }
    }
}

/// Dismissible error banner.
struct ErrorBanner: View {
    let message: String
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(Theme.error)
                .accessibilityHidden(true)
            Text(message)
                .font(.footnote)
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(3)
            Spacer(minLength: 0)
            Button(action: dismiss) {
                Image(systemName: "xmark")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Dismiss error")
            .accessibilityHint("Hides this error message")
        }
        .padding(12)
        .background(Theme.error.opacity(0.12))
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(Theme.error.opacity(0.35), lineWidth: 1)
        )
        .padding(.horizontal)
        .accessibilityElement(children: .combine)
    }
}

/// Stack of dismissible notices for out-of-band server signals
/// (push notifications, interrupts, context compaction).
struct NoticeStack: View {
    let notices: [Notice]
    let onDismiss: (UUID) -> Void

    var body: some View {
        VStack(spacing: 4) {
            ForEach(notices) { notice in
                NoticeRow(notice: notice) { onDismiss(notice.id) }
            }
        }
        .padding(.horizontal)
    }
}

private struct NoticeRow: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let notice: Notice
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            Text(notice.message)
                .font(.footnote)
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(3)
            Spacer(minLength: 0)
            Button(action: dismiss) {
                Image(systemName: "xmark")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Dismiss notice")
            .accessibilityHint("Hides this notice")
        }
        .padding(12)
        .background(tint.opacity(0.12))
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(tint.opacity(0.35), lineWidth: 1)
        )
        .accessibilityElement(children: .combine)
        // Honor Reduce Motion: skip the slide/fade for motion-sensitive users.
        .transition(reduceMotion
            ? .opacity
            : .move(edge: .top).combined(with: .opacity))
    }

    private var icon: String {
        switch notice.kind {
        case .info: "info.circle.fill"
        case .notification: "bell.fill"
        case .compaction: "arrow.down.right.and.arrow.up.left"
        }
    }

    private var tint: Color {
        switch notice.kind {
        case .info: Theme.textSecondary
        case .notification: Theme.mint
        case .compaction: Theme.warning
        }
    }
}
