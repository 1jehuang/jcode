import SwiftUI

/// Design tokens. Dark, calm, terminal-native; mint accent for live state.
enum Theme {
    static let background = Color(hex: 0x0F0F14)
    static let surface = Color(hex: 0x1A1A1F)
    static let surfaceElevated = Color(hex: 0x242429)
    static let border = Color.white.opacity(0.08)
    static let borderStrong = Color.white.opacity(0.14)
    static let mint = Color(hex: 0x4DD9A6)
    static let mintTint = Color(hex: 0x4DD9A6).opacity(0.15)
    static let textPrimary = Color.white.opacity(0.92)
    static let textSecondary = Color.white.opacity(0.55)
    static let textTertiary = Color.white.opacity(0.35)
    static let warning = Color(hex: 0xF59E0B)
    static let error = Color(hex: 0xD94D59)

    /// Accent fill for primary actions; a touch of depth without a rainbow.
    static let mintGradient = LinearGradient(
        colors: [Color(hex: 0x5FE3B3), Color(hex: 0x36C08D)],
        startPoint: .top,
        endPoint: .bottom
    )

    /// Fill for the user's own message bubbles.
    static let userBubble = LinearGradient(
        colors: [Color(hex: 0x4DD9A6).opacity(0.22), Color(hex: 0x4DD9A6).opacity(0.12)],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )

    /// Very subtle top-lit sheen for chrome surfaces (header, composer).
    static let chrome = LinearGradient(
        colors: [Color.white.opacity(0.05), Color.white.opacity(0.0)],
        startPoint: .top,
        endPoint: .bottom
    )

    /// Corner radius scale.
    enum Radius {
        static let small: CGFloat = 10
        static let medium: CGFloat = 14
        static let large: CGFloat = 18
        static let bubble: CGFloat = 20
    }

    static func mono(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }

    /// Decorative icon font (SF Symbols) at a fixed point size.
    static func icon(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }
}

extension Color {
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255.0,
            green: Double((hex >> 8) & 0xFF) / 255.0,
            blue: Double(hex & 0xFF) / 255.0
        )
    }
}

/// Extra edge padding for chrome pinned to an edge with no system inset.
///
/// Home-button devices (iPhone SE class) report a zero bottom safe-area inset,
/// so edge-pinned chrome needs explicit breathing room there; Dynamic Island
/// devices already get it from the system insets. Derived from the root
/// GeometryReader in RootView and injected via the environment: reading
/// UIKit window insets during a SwiftUI body evaluation creates an
/// AttributeGraph cycle that corrupts view-hierarchy updates.
struct CompactEdgePads: Equatable {
    var top: CGFloat = 0
    var bottom: CGFloat = 0

    /// Derives the pads from the container's safe-area insets.
    init(safeArea: EdgeInsets) {
        top = safeArea.top < 24 ? 12 : 0
        bottom = safeArea.bottom > 0 ? 0 : 12
    }

    init() {}
}

extension EnvironmentValues {
    @Entry var compactEdgePads = CompactEdgePads()
}

/// Card container used across screens.
struct Card<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.surface)
            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.large, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.large, style: .continuous)
                    .stroke(Theme.border, lineWidth: 1)
            )
    }
}

/// Hairline rule used to separate chrome from content.
struct Hairline: View {
    var body: some View {
        Rectangle()
            .fill(Theme.border)
            .frame(height: 1)
            .accessibilityHidden(true)
    }
}

/// Shared chrome for the inline banner/notice family (error, offline, notices).
///
/// Keeps every inline strip on the same radius, padding, tint math, and
/// dismiss/action affordance so the stack reads as one system.
struct BannerStrip<Trailing: View>: View {
    let icon: String
    let tint: Color
    let message: String
    var lineLimit: Int = 3
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(tint)
                .frame(width: 18)
                .accessibilityHidden(true)
            Text(message)
                .font(.footnote)
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(lineLimit)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            trailing
        }
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .padding(.vertical, 2)
        .frame(minHeight: 48)
        .background(tint.opacity(0.12))
        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                .stroke(tint.opacity(0.32), lineWidth: 1)
        )
        .accessibilityElement(children: .combine)
    }
}

/// Compact circular dismiss control sized for touch.
struct DismissButton: View {
    let label: String
    let hint: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.caption.weight(.bold))
                .foregroundStyle(Theme.textSecondary)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityHint(hint)
    }
}

/// Scales and dims slightly on press: makes taps feel connected on iOS.
struct PressableButtonStyle: ButtonStyle {
    var scale: CGFloat = 0.94

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(.spring(response: 0.25, dampingFraction: 0.7), value: configuration.isPressed)
    }
}
