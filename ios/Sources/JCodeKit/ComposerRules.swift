import Foundation

/// Pure rules for the chat composer, so keyboard/submit behavior is unit
/// testable on macOS instead of only reachable through SwiftUI.
///
/// The view layer owns focus and rendering; every decision about *whether* a
/// keystroke or tap should send, and *what* it sends, lives here.
public enum ComposerRules {
    /// What the app should do with the current draft when the user submits.
    public enum SubmitAction: Equatable, Sendable {
        /// Send as a normal message (agent is idle).
        case send(String)
        /// Queue as a soft-interrupt (agent is mid-run).
        case queue(String)
        /// Do nothing: empty draft, or not connected.
        case ignore
        /// Insert a newline instead of sending (Shift-Return style).
        case newline
    }

    /// Trailing whitespace/newlines are stripped: iOS software keyboards deliver
    /// Return as a literal "\n" appended to the bound text, so a submit must not
    /// treat that trailing newline as content.
    public static func normalize(_ draft: String) -> String {
        draft.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// True when the draft has anything worth sending and we can reach a server.
    public static func canSubmit(draft: String, isConnected: Bool) -> Bool {
        isConnected && !normalize(draft).isEmpty
    }

    /// Decide what a submit gesture (Return key or send button) should do.
    ///
    /// - Parameters:
    ///   - draft: current composer text, possibly with a trailing newline from
    ///     the software keyboard's Return.
    ///   - isConnected: whether a live server connection exists.
    ///   - isProcessing: whether the agent is currently running a turn.
    ///   - wantsNewline: true when the gesture explicitly asked for a line break
    ///     (Shift-Return on a hardware keyboard).
    public static func submitAction(
        draft: String,
        isConnected: Bool,
        isProcessing: Bool,
        wantsNewline: Bool = false
    ) -> SubmitAction {
        if wantsNewline { return .newline }
        let text = normalize(draft)
        guard isConnected, !text.isEmpty else { return .ignore }
        return isProcessing ? .queue(text) : .send(text)
    }

    /// Whether a raw draft change came from the Return key on the software
    /// keyboard. SwiftUI's `TextField(axis: .vertical)` inserts a newline rather
    /// than firing `onSubmit`, so the view watches for this to emulate submit.
    ///
    /// Only a *trailing* newline counts, and only when there is other content:
    /// pasting multi-line text must not auto-send.
    public static func isReturnKeySubmit(_ draft: String) -> Bool {
        guard let last = draft.last, last.isNewline else { return false }
        return !normalize(draft).isEmpty
    }
}
