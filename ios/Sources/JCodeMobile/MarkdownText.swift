import SwiftUI

struct MarkdownText: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(parse(text).enumerated()), id: \.offset) { _, block in
                switch block {
                case .paragraph(let text):
                    Text(inlineMarkdown(text))
                        .textSelection(.enabled)

                case .code(let language, let code):
                    VStack(alignment: .leading, spacing: 0) {
                        if !language.isEmpty {
                            Text(language)
                                .font(.caption2.weight(.medium))
                                .foregroundStyle(.secondary)
                                .padding(.horizontal, 10)
                                .padding(.top, 6)
                                .padding(.bottom, 2)
                        }
                        ScrollView(.horizontal, showsIndicators: false) {
                            Text(code)
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                                .padding(10)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.codeBackground)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))

                case .heading(let level, let text):
                    Text(inlineMarkdown(text))
                        .font(headingFont(level))
                        .textSelection(.enabled)

                case .listItem(let text):
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("•")
                            .foregroundStyle(.secondary)
                        Text(inlineMarkdown(text))
                            .textSelection(.enabled)
                    }

                case .divider:
                    Divider()
                }
            }
        }
    }

    private func headingFont(_ level: Int) -> Font {
        switch level {
        case 1: .title3.weight(.bold)
        case 2: .headline
        case 3: .subheadline.weight(.semibold)
        default: .subheadline.weight(.medium)
        }
    }

    private func inlineMarkdown(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(text)
    }
}

private enum Block {
    case paragraph(String)
    case code(language: String, code: String)
    case heading(level: Int, text: String)
    case listItem(String)
    case divider
}

private func parse(_ text: String) -> [Block] {
    var blocks: [Block] = []
    let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    var i = 0

    while i < lines.count {
        let line = lines[i]

        if line.hasPrefix("```") {
            let language = String(line.dropFirst(3)).trimmingCharacters(in: .whitespaces)
            var codeLines: [String] = []
            i += 1
            while i < lines.count && !lines[i].hasPrefix("```") {
                codeLines.append(lines[i])
                i += 1
            }
            if i < lines.count { i += 1 }
            blocks.append(.code(language: language, code: codeLines.joined(separator: "\n")))
            continue
        }

        if line.hasPrefix("# ") {
            blocks.append(.heading(level: 1, text: String(line.dropFirst(2))))
            i += 1
            continue
        }
        if line.hasPrefix("## ") {
            blocks.append(.heading(level: 2, text: String(line.dropFirst(3))))
            i += 1
            continue
        }
        if line.hasPrefix("### ") {
            blocks.append(.heading(level: 3, text: String(line.dropFirst(4))))
            i += 1
            continue
        }

        if line.hasPrefix("- ") || line.hasPrefix("* ") {
            blocks.append(.listItem(String(line.dropFirst(2))))
            i += 1
            continue
        }

        if let match = line.range(of: #"^\d+\.\s+"#, options: .regularExpression) {
            blocks.append(.listItem(String(line[match.upperBound...])))
            i += 1
            continue
        }

        if line.allSatisfy({ $0 == "-" || $0 == " " }) && line.filter({ $0 == "-" }).count >= 3 {
            blocks.append(.divider)
            i += 1
            continue
        }

        if line.trimmingCharacters(in: .whitespaces).isEmpty {
            i += 1
            continue
        }

        var paragraphLines = [line]
        i += 1
        while i < lines.count {
            let next = lines[i]
            if next.trimmingCharacters(in: .whitespaces).isEmpty ||
               next.hasPrefix("```") || next.hasPrefix("# ") ||
               next.hasPrefix("- ") || next.hasPrefix("* ") {
                break
            }
            paragraphLines.append(next)
            i += 1
        }
        blocks.append(.paragraph(paragraphLines.joined(separator: "\n")))
    }

    return blocks
}

private extension Color {
    static var codeBackground: Color {
        #if canImport(UIKit)
        Color(uiColor: .tertiarySystemBackground)
        #else
        Color.gray.opacity(0.1)
        #endif
    }
}
