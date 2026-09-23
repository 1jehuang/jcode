import SwiftUI

/// Design tokens, mirrored from Jcode Desktop's default "Warm neutral" theme
/// (`jcode-desktop-ui/src/theme.rs`): charcoal and stone surfaces, ivory type,
/// restrained sandstone focus. Keep role names aligned with Desktop so the two
/// clients read as one product.
enum Theme {
    /// Desktop `BG`: the canvas behind every page (sheets, pairing).
    static let background = Color(hex: 0x1C1A18)
    /// Desktop `PANEL_BG`: the raised page a conversation lives on.
    static let surface = Color(hex: 0x25221F)
    /// Desktop `HEADER_BG` / `USER_BG`: backing behind the page, prompt cards.
    static let surfaceElevated = Color(hex: 0x302B27)
    /// Desktop `CODE_BG`: code blocks and expanded tool output.
    static let codeBackground = Color(hex: 0x1C1A18)
    /// Desktop `PANEL_BORDER` / `TOOL_BORDER`.
    static let border = Color(hex: 0x403A34)
    /// Desktop `PANEL_BORDER_FOCUS`: focused input outline.
    static let borderFocus = Color(hex: 0x87796B)
    /// Desktop `ACCENT`: sandstone. Used sparingly for primary actions.
    static let accent = Color(hex: 0xB6A08A)
    /// Ink placed on an accent fill.
    static let onAccent = Color(hex: 0x1C1A18)
    static let link = Color(hex: 0xC6B5A2)
    /// Desktop `TEXT` / `TEXT_DIM` / `TEXT_FAINT`.
    static let textPrimary = Color(hex: 0xE4DDD3)
    static let textSecondary = Color(hex: 0xA79D91)
    static let textTertiary = Color(hex: 0x9B9084)
    /// Desktop `OK` / `WARN` / `ERROR` semantic states.
    static let ok = Color(hex: 0xA6B68E)
    static let warning = Color(hex: 0xD0B17D)
    static let error = Color(hex: 0xE18C85)

    /// Desktop's prompt wash: the CLI prompt-number rainbow in reverse hue
    /// order. The newest prompt is violet; older prompts step toward red while
    /// the tint fades exponentially back into plain card paper.
    static func promptBackground(distance: Int) -> Color {
        let rainbow: [UInt32] = [0xFF5050, 0xFFA050, 0xFFE650, 0x50DC64, 0x50C8DC, 0x648CFF, 0xB464FF]
        let d = max(0, distance)
        let tint = rainbow[rainbow.count - 1 - min(d, rainbow.count - 1)]
        let strength = 0.05 * exp(-0.4 * Double(d))
        return Color(hex: 0x302B27, blending: tint, amount: strength)
    }

    /// Corner radius scale (Desktop: 8px controls, 12px prompt cards).
    enum Radius {
        static let small: CGFloat = 8
        static let medium: CGFloat = 12
        static let large: CGFloat = 14
        static let bubble: CGFloat = 12
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

    /// `base` with `tint` composited over it at `amount` opacity (opaque result).
    init(hex base: UInt32, blending tint: UInt32, amount: Double) {
        func channel(_ value: UInt32, _ shift: UInt32) -> Double {
            Double((value >> shift) & 0xFF) / 255.0
        }
        func mix(_ shift: UInt32) -> Double {
            channel(base, shift) * (1 - amount) + channel(tint, shift) * amount
        }
        self.init(red: mix(16), green: mix(8), blue: mix(0))
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

/// Caps a reading column on wide screens.
///
/// The app is universal, and a chat transcript stretched across a 13" iPad is
/// unreadable (and looks unfinished to a reviewer). Content is centered in a
/// measured column instead, matching how every mail/chat app handles regular
/// width. Narrow devices are unaffected.
struct ReadableColumn: ViewModifier {
    static let maxWidth: CGFloat = 720

    func body(content: Content) -> some View {
        content
            .frame(maxWidth: Self.maxWidth)
            .frame(maxWidth: .infinity)
    }
}

extension View {
    /// Centers this view in a reading-width column on wide screens.
    func readableColumn() -> some View {
        modifier(ReadableColumn())
    }
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

/// The canonical Jcode halftone donut, shared with Desktop and the website.
struct BrandMark: View {
    var size: CGFloat = 40

    var body: some View {
        Image("BrandMark")
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}
