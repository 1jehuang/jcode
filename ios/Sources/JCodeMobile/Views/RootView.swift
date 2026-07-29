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

/// Connection status pill shown in the chat header.
struct StatusPill: View {
    let phase: ConnectionPhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 6) {
            ZStack {
                if isLive && !reduceMotion {
                    Circle()
                        .fill(color.opacity(0.35))
                        .frame(width: 14, height: 14)
                        .scaleEffect(pulse ? 1.35 : 0.8)
                        .opacity(pulse ? 0 : 1)
                        .animation(
                            .easeOut(duration: 1.6).repeatForever(autoreverses: false),
                            value: pulse
                        )
                }
                Circle()
                    .fill(color)
                    .frame(width: 7, height: 7)
            }
            .frame(width: 14, height: 14)
            .accessibilityHidden(true)
            Text(label)
                .font(Theme.mono(11, weight: .medium))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
        }
        .padding(.leading, 8)
        .padding(.trailing, 12)
        .padding(.vertical, 6)
        .background(Theme.surface)
        .clipShape(Capsule())
        .overlay(Capsule().stroke(isLive ? color.opacity(0.35) : Theme.border, lineWidth: 1))
        .onAppear { pulse = true }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Connection")
        .accessibilityValue(label)
    }

    @State private var pulse = false

    private var isLive: Bool {
        if case .connected = phase { return true }
        return false
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
        BannerStrip(
            icon: "exclamationmark.triangle.fill",
            tint: Theme.error,
            message: message
        ) {
            DismissButton(
                label: "Dismiss error",
                hint: "Hides this error message",
                action: dismiss
            )
        }
        .padding(.horizontal, 16)
    }
}

/// Stack of dismissible notices for out-of-band server signals
/// (push notifications, interrupts, context compaction).
struct NoticeStack: View {
    let notices: [Notice]
    let onDismiss: (UUID) -> Void

    var body: some View {
        VStack(spacing: 6) {
            ForEach(notices) { notice in
                NoticeRow(notice: notice) { onDismiss(notice.id) }
            }
        }
        .padding(.horizontal, 16)
    }
}

private struct NoticeRow: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let notice: Notice
    let dismiss: () -> Void

    var body: some View {
        BannerStrip(icon: icon, tint: tint, message: notice.message) {
            DismissButton(
                label: "Dismiss notice",
                hint: "Hides this notice",
                action: dismiss
            )
        }
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
