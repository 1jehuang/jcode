import Foundation

/// A fully offline, scripted jcode server used by demo mode.
///
/// Why this exists: the app is a remote control for the user's own `jcode`
/// server. A reviewer (or a first-time visitor evaluating the app) has no such
/// server on their network, so without this the app is a pairing form that can
/// never succeed and looks broken. `DemoTransport` implements the same
/// `WebSocketTransport` contract as the real socket and replies with the real
/// wire protocol, so demo mode exercises the production `Connection` and
/// `SessionReducer` paths instead of a parallel fake UI.
///
/// It never touches the network.
public actor DemoTransport: WebSocketTransport {
    /// Multiplier applied to every scripted delay. Tests use 0 to run instantly.
    private let speed: Double
    /// Frames produced but not yet read by `receiveText`.
    private var pending: [String] = []
    /// Parked `receiveText` caller, resumed by the next emitted frame.
    private var waiter: CheckedContinuation<String?, Never>?
    private var connected = false
    private var closed = false
    private var work: [Task<Void, Never>] = []

    public init(speed: Double = 1.0) {
        self.speed = speed
    }

    public func connect(url: URL, authToken: String) async throws {
        guard !closed else { throw TransportError.notConnected }
        connected = true
    }

    public func send(text: String) async throws {
        guard connected, !closed else { throw TransportError.notConnected }
        guard let data = text.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let type = object["type"] as? String
        else { return }
        let id = (object["id"] as? NSNumber)?.uint64Value ?? 0
        switch type {
        case "subscribe":
            emit(["type": "session", "session_id": Self.sessionID])
            emit(["type": "ack", "id": id])
        case "get_history":
            emit(Self.historyEvent(id: id, messages: messages, model: currentModel))
        case "message":
            let content = object["content"] as? String ?? ""
            record(role: "user", content: content)
            schedule(reply(to: content, requestID: id))
        case "cancel":
            cancelWork()
            emit(["type": "interrupted"])
            emit(["type": "done", "id": id])
        case "soft_interrupt":
            let content = object["content"] as? String ?? ""
            emit([
                "type": "soft_interrupt_injected", "content": content,
                "point": "between_tools", "tools_skipped": 0,
            ])
            record(role: "user", content: content)
            schedule(reply(to: content, requestID: id))
        case "cancel_soft_interrupts":
            emit(["type": "ack", "id": id])
        case "set_model":
            let model = object["model"] as? String ?? Self.defaultModel
            currentModel = model
            emit(["type": "model_changed", "id": id, "model": model])
        case "set_reasoning_effort":
            let effort = object["effort"] as? String ?? "medium"
            emit(["type": "reasoning_effort_changed", "id": id, "effort": effort])
        case "compact":
            emit(["type": "compaction", "trigger": "manual", "tokens_saved": 4_096])
            emit([
                "type": "compact_result", "id": id, "success": true,
                "message": "Compacted demo conversation (4096 tokens saved).",
            ])
        case "rename_session":
            let title = object["title"] as? String ?? "Demo session"
            emit([
                "type": "session_renamed", "session_id": Self.sessionID, "display_title": title,
            ])
        case "clear":
            messages = []
            emit(Self.historyEvent(id: id, messages: [], model: currentModel))
        case "ping":
            emit(["type": "pong", "id": id])
        default:
            emit(["type": "ack", "id": id])
        }
    }

    public func receiveText() async throws -> String? {
        if !pending.isEmpty {
            return pending.removeFirst()
        }
        // A closed transport reports end-of-stream, matching URLSession's
        // behavior, so the connection loop unwinds instead of erroring.
        if closed { return nil }
        guard connected else { throw TransportError.notConnected }
        return await withCheckedContinuation { continuation in
            waiter = continuation
        }
    }

    public func close() async {
        guard !closed else { return }
        closed = true
        connected = false
        cancelWork()
        pending.removeAll()
        waiter?.resume(returning: nil)
        waiter = nil
    }

    // MARK: - Scripted conversation

    public static let sessionID = "demo-session"
    public static let defaultModel = "claude-api:claude-fable-5"
    public static let availableModels = [
        "claude-api:claude-fable-5",
        "openai-api:gpt-5.5",
        "claude-api:claude-haiku-4",
    ]
    /// Host shown for the synthetic demo server. Not resolvable on purpose.
    public static let host = "demo.local"

    private var currentModel = DemoTransport.defaultModel
    private var messages: [[String: Any]] = []

    private func record(role: String, content: String) {
        messages.append(["role": role, "content": content])
    }

    private func emit(_ object: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: object),
            let line = String(data: data, encoding: .utf8)
        else { return }
        emit(line: line)
    }

    private func emit(line: String) {
        guard !closed else { return }
        if let waiter {
            self.waiter = nil
            waiter.resume(returning: line)
        } else {
            pending.append(line)
        }
    }

    private func schedule(_ steps: [Step]) {
        let task = Task { [weak self] in
            for step in steps {
                if Task.isCancelled { return }
                await self?.sleep(step.delay)
                if Task.isCancelled { return }
                await self?.emit(line: step.line)
            }
        }
        work.append(task)
    }

    private func sleep(_ seconds: Double) async {
        let scaled = seconds * speed
        guard scaled > 0 else { return }
        try? await Task.sleep(nanoseconds: UInt64(scaled * 1_000_000_000))
    }

    private func cancelWork() {
        for task in work { task.cancel() }
        work = []
    }

    /// One scripted frame: a pre-encoded JSON line and the delay before it.
    struct Step: Sendable {
        var delay: Double
        var line: String

        init(delay: Double, event: [String: Any]) {
            self.delay = delay
            let data = (try? JSONSerialization.data(withJSONObject: event)) ?? Data()
            self.line = String(data: data, encoding: .utf8) ?? "{}"
        }
    }

    /// Picks a canned answer for the prompt. Keyword matching keeps the demo
    /// feeling responsive to what the user typed without any model call.
    private func reply(to prompt: String, requestID: UInt64) -> [Step] {
        let script = DemoScript.forPrompt(prompt)
        record(role: "assistant", content: script.answer)
        var steps: [Step] = []
        steps.append(Step(delay: 0.15, event: ["type": "status_detail", "detail": "thinking"]))
        for chunk in script.reasoning.chunked(8) {
            steps.append(Step(delay: 0.03, event: ["type": "reasoning_delta", "text": chunk]))
        }
        steps.append(Step(delay: 0.1, event: ["type": "reasoning_done", "duration_secs": 1.2]))
        if let tool = script.tool {
            steps.append(
                Step(delay: 0.1, event: ["type": "tool_start", "id": tool.id, "name": tool.name]))
            for chunk in tool.input.chunked(14) {
                steps.append(Step(delay: 0.02, event: ["type": "tool_input", "delta": chunk]))
            }
            steps.append(
                Step(delay: 0.1, event: ["type": "tool_exec", "id": tool.id, "name": tool.name]))
            steps.append(
                Step(
                    delay: 0.5,
                    event: [
                        "type": "tool_done", "id": tool.id, "name": tool.name,
                        "output": tool.output,
                    ]))
        }
        for chunk in script.answer.chunked(6) {
            steps.append(Step(delay: 0.02, event: ["type": "text_delta", "text": chunk]))
        }
        steps.append(
            Step(delay: 0.05, event: ["type": "tokens", "input": 1_248, "output": 312]))
        steps.append(Step(delay: 0.0, event: ["type": "message_end"]))
        steps.append(Step(delay: 0.0, event: ["type": "done", "id": requestID]))
        return steps
    }

    private static func historyEvent(
        id: UInt64, messages: [[String: Any]], model: String
    ) -> [String: Any] {
        [
            "type": "history",
            "id": id,
            "session_id": sessionID,
            "messages": messages,
            "provider_name": "demo",
            "provider_model": model,
            "available_models": availableModels,
            "total_tokens": [1_248, 312],
            "all_sessions": [sessionID],
            "server_version": "demo",
            "display_title": "Demo session",
            "reasoning_effort": "medium",
        ]
    }
}

/// Canned answers for demo mode, kept separate from transport mechanics so the
/// content is easy to review and adjust.
public enum DemoScript {
    public struct Tool: Sendable {
        public var id: String
        public var name: String
        public var input: String
        public var output: String
    }

    public struct Reply: Sendable {
        public var reasoning: String
        public var tool: Tool?
        public var answer: String
    }

    /// Prompts offered on the empty demo transcript.
    public static let suggestions = [
        "What's the state of this repo?",
        "Run the tests",
        "Summarize recent changes",
    ]

    public static func forPrompt(_ prompt: String) -> Reply {
        let lowered = prompt.lowercased()
        if lowered.contains("test") {
            return Reply(
                reasoning: "Find the test command for this project, then run it.",
                tool: Tool(
                    id: "demo-tool-tests",
                    name: "bash",
                    input: "swift test",
                    output: """
                        Test run with 94 tests in 2 suites passed after 0.013 seconds.
                        """
                ),
                answer: """
                    All **94 tests** pass in 0.013s across 2 suites.

                    ```
                    swift test  ->  94 passed, 0 failed
                    ```

                    _This is jcode's offline demo. Pair with your own machine to \
                    run real commands._
                    """
            )
        }
        if lowered.contains("recent") || lowered.contains("change") || lowered.contains("commit") {
            return Reply(
                reasoning: "Read the recent commit log and summarize the themes.",
                tool: Tool(
                    id: "demo-tool-log",
                    name: "bash",
                    input: "git log --oneline -5",
                    output: """
                        a1b2c3d feat(mobile): offline demo mode
                        d4e5f6a fix(gateway): advertise working_dir to remote clients
                        b7c8d9e feat(mobile): session switching
                        f0a1b2c test: connection reconnect coverage
                        c3d4e5f chore: bump version
                        """
                ),
                answer: """
                    Recent work falls into three themes:

                    1. **Mobile** - offline demo mode and session switching.
                    2. **Gateway** - servers now advertise their working directory \
                    so phones can subscribe.
                    3. **Tests** - added reconnect coverage.

                    _Demo mode: connect your own server to see real history._
                    """
            )
        }
        return Reply(
            reasoning: "Summarize the repository layout and current status.",
            tool: Tool(
                id: "demo-tool-status",
                name: "bash",
                input: "git status --short && ls",
                output: """
                    ## main...origin/main
                    Sources/  Tests/  Package.swift  README.md
                    """
            ),
            answer: """
                The working tree is **clean** on `main`.

                - `Sources/` - app and shared kit
                - `Tests/` - unit tests
                - `Package.swift` - package manifest

                You are in jcode's **offline demo**, so this is a canned response. \
                Run `jcode pair` on your own machine and scan the QR code to drive \
                a real session from your phone.
                """
        )
    }
}

extension String {
    /// Splits into chunks of at most `size` characters, for streaming.
    func chunked(_ size: Int) -> [String] {
        guard size > 0, !isEmpty else { return isEmpty ? [] : [self] }
        var result: [String] = []
        var index = startIndex
        while index < endIndex {
            let end = self.index(index, offsetBy: size, limitedBy: endIndex) ?? endIndex
            result.append(String(self[index..<end]))
            index = end
        }
        return result
    }
}
