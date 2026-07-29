import JCodeKit
import SwiftUI

/// Inline banner shown while the connection is down, with a manual retry.
///
/// The automatic reconnect loop keeps running underneath; the button just
/// short-circuits the backoff wait for impatient humans.
struct ConnectionBanner: View {
    let phase: ConnectionPhase
    let onRetry: () -> Void

    var body: some View {
        BannerStrip(
            icon: "wifi.slash",
            tint: Theme.warning,
            message: label,
            lineLimit: 2
        ) {
            InlineActionButton(title: "Retry", tint: Theme.mint, action: onRetry)
                .accessibilityLabel("Retry connection")
                .accessibilityHint("Reconnects to the server now")
        }
        .padding(.horizontal, 16)
    }

    private var label: String {
        switch phase {
        case .reconnecting(let attempt):
            "Connection lost, retrying (attempt \(attempt))"
        case .failed:
            "Connection failed"
        default:
            "Offline"
        }
    }
}

/// Chip shown above the composer while soft-interrupt messages wait to be
/// injected into the running turn, with a cancel affordance.
struct QueuedInterruptChip: View {
    let count: Int
    let onCancel: () -> Void

    var body: some View {
        BannerStrip(
            icon: "clock",
            tint: Theme.mint,
            message: count == 1 ? "1 message queued" : "\(count) messages queued",
            lineLimit: 1
        ) {
            InlineActionButton(title: "Cancel", tint: Theme.error, action: onCancel)
                .accessibilityLabel("Cancel queued messages")
                .accessibilityHint("Removes messages waiting to interrupt the response")
        }
        .padding(.horizontal, 16)
    }
}

/// Text action sized for touch, used inside inline banners.
struct InlineActionButton: View {
    let title: String
    let tint: Color
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(tint)
                .padding(.horizontal, 12)
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(PressableButtonStyle(scale: 0.96))
    }
}
