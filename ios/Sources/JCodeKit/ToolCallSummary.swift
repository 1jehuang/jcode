import Foundation

/// Human-readable summaries of a tool call's streamed JSON input.
///
/// Every jcode tool schema carries a required `intent` property (see
/// `jcode-tool-core::ensure_intent_in_schema`), so the agent always states *why*
/// it is running a tool. Surfacing that intent turns an opaque `bash` card into
/// "checking the build passes", which is the difference between scanning a
/// transcript and decoding it.
///
/// Input arrives as streaming JSON deltas, so these helpers must tolerate
/// partial/truncated JSON: they parse when possible and fall back to a scan.
public enum ToolCallSummary {
    /// Keys that describe *what* a call operates on, in priority order. Used for
    /// the subtitle when the agent's stated intent is not available yet.
    static let subjectKeys = [
        "command", "file_path", "path", "query", "url", "pattern", "target",
    ]

    /// The agent's stated intent for this call, if present.
    ///
    /// Streaming means the value may still be arriving; a partial intent is
    /// better than nothing, so a prefix is returned once the string opens.
    public static func intent(from input: String) -> String? {
        guard !input.isEmpty else { return nil }
        if let value = completeString(forKey: "intent", in: input) {
            return clean(value)
        }
        // Still streaming: recover an opening (unterminated) intent string.
        if let partial = partialString(forKey: "intent", in: input) {
            return clean(partial)
        }
        return nil
    }

    /// What the call acts on (command, file, query), for the secondary line.
    public static func subject(from input: String) -> String? {
        guard !input.isEmpty else { return nil }
        for key in subjectKeys {
            if let value = completeString(forKey: key, in: input) {
                return clean(value)
            }
        }
        for key in subjectKeys {
            if let value = partialString(forKey: key, in: input) {
                return clean(value)
            }
        }
        // Not JSON at all (or no known key): show the flattened raw input.
        if !input.hasPrefix("{") {
            return clean(input)
        }
        return nil
    }

    /// The single line to show on a collapsed card: the agent's intent when it
    /// stated one, otherwise what the call operates on.
    public static func headline(from input: String) -> String? {
        intent(from: input) ?? subject(from: input)
    }

    // MARK: - Parsing

    /// Parse fully-formed JSON and read a string value.
    private static func completeString(forKey key: String, in input: String) -> String? {
        guard let data = input.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let value = object[key] as? String,
            !value.isEmpty
        else { return nil }
        return value
    }

    /// Recover a string value from JSON that is still streaming in.
    ///
    /// Scans for `"key"` then the opening quote of its value and takes everything
    /// up to the closing quote (or end of input if it has not arrived yet),
    /// honoring backslash escapes.
    private static func partialString(forKey key: String, in input: String) -> String? {
        guard let keyRange = input.range(of: "\"\(key)\"") else { return nil }
        var index = keyRange.upperBound

        // Skip whitespace and the ':' separator.
        while index < input.endIndex, input[index] != ":" {
            guard input[index].isWhitespace else { return nil }
            index = input.index(after: index)
        }
        guard index < input.endIndex else { return nil }
        index = input.index(after: index)  // past ':'
        while index < input.endIndex, input[index].isWhitespace {
            index = input.index(after: index)
        }
        // Value must be a string.
        guard index < input.endIndex, input[index] == "\"" else { return nil }
        index = input.index(after: index)

        var value = ""
        var escaped = false
        while index < input.endIndex {
            let character = input[index]
            if escaped {
                switch character {
                case "n": value.append("\n")
                case "t": value.append("\t")
                case "r": value.append("\r")
                default: value.append(character)
                }
                escaped = false
            } else if character == "\\" {
                escaped = true
            } else if character == "\"" {
                break  // value closed
            } else {
                value.append(character)
            }
            index = input.index(after: index)
        }
        return value.isEmpty ? nil : value
    }

    /// Collapse whitespace to one line and trim, so a summary never wraps or
    /// smuggles newlines into the header.
    private static func clean(_ value: String) -> String {
        value
            .split(whereSeparator: { $0.isNewline || $0 == "\t" })
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespaces)
    }
}
