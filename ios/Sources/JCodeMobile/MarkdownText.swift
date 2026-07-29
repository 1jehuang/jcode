import SwiftUI

/// Lightweight markdown renderer for assistant messages.
///
/// Handles fenced code blocks as monospaced cards and renders everything else
/// through SwiftUI's native AttributedString markdown (bold, italics, inline
/// code, links). Deliberately not a full CommonMark implementation.
struct MarkdownText: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(segments.enumerated()), id: \.offset) { _, segment in
                switch segment {
                case .prose(let prose):
                    Text(attributed(prose))
                        .font(.body)
                        .foregroundStyle(Theme.textPrimary)
                        .tint(Theme.mint)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                case .code(let code, let language):
                    CodeBlock(code: code, language: language)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private enum Segment {
        case prose(String)
        case code(String, language: String?)
    }

    private var segments: [Segment] {
        var result: [Segment] = []
        var prose: [String] = []
        var code: [String] = []
        var language: String?
        var inCode = false

        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if line.hasPrefix("```") {
                if inCode {
                    result.append(.code(code.joined(separator: "\n"), language: language))
                    code = []
                    inCode = false
                } else {
                    let joined = prose.joined(separator: "\n")
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                    if !joined.isEmpty {
                        result.append(.prose(joined))
                    }
                    prose = []
                    language = line.dropFirst(3).isEmpty ? nil : String(line.dropFirst(3))
                    inCode = true
                }
            } else if inCode {
                code.append(String(line))
            } else {
                prose.append(String(line))
            }
        }
        if inCode {
            result.append(.code(code.joined(separator: "\n"), language: language))
        } else {
            let joined = prose.joined(separator: "\n")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !joined.isEmpty {
                result.append(.prose(joined))
            }
        }
        return result
    }

    private func attributed(_ string: String) -> AttributedString {
        (try? AttributedString(
            markdown: string,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )) ?? AttributedString(string)
    }
}

/// Fenced code block: language chip, copy action, horizontal scroll.
private struct CodeBlock: View {
    let code: String
    let language: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text((language?.isEmpty == false ? language! : "code").lowercased())
                    .font(Theme.mono(10, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
                Spacer(minLength: 0)
                Button {
                    UIPasteboard.general.string = code
                } label: {
                    Image(systemName: "doc.on.doc")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(Theme.textSecondary)
                        .frame(width: 32, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(PressableButtonStyle(scale: 0.9))
                .accessibilityLabel("Copy code")
            }
            .padding(.leading, 12)
            .padding(.trailing, 4)
            .padding(.vertical, 2)
            .background(Theme.surfaceElevated)
            Hairline()
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.textPrimary.opacity(0.85))
                    .padding(12)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.surface)
        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                .stroke(Theme.border, lineWidth: 1)
        )
    }
}
