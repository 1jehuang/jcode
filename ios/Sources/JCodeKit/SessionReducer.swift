import Foundation

/// One entry in the rendered transcript.
public struct TranscriptEntry: Equatable, Sendable, Identifiable {
    public enum Role: String, Sendable {
        case user
        case assistant
        case system
    }

    public struct ToolCall: Equatable, Sendable, Identifiable {
        public enum Status: Equatable, Sendable {
            case streamingInput
            case running
            case succeeded
            case failed(String)
        }

        public var id: String
        public var name: String
        public var input: String
        public var output: String
        public var status: Status

        public init(
            id: String, name: String, input: String = "", output: String = "",
            status: Status = .streamingInput
        ) {
            self.id = id
            self.name = name
            self.input = input
            self.output = output
            self.status = status
        }
    }

    public var id: UUID
    public var role: Role
    public var text: String
    public var reasoning: String
    public var toolCalls: [ToolCall]
    /// True while this entry is still receiving streamed content.
    public var isStreaming: Bool
    /// True while this entry is a soft-interrupt waiting for the server to
    /// inject it at a safe point. Cleared by `soft_interrupt_injected`.
    public var isQueued: Bool

    public init(
        id: UUID = UUID(),
        role: Role,
        text: String,
        reasoning: String = "",
        toolCalls: [ToolCall] = [],
        isStreaming: Bool = false,
        isQueued: Bool = false
    ) {
        self.id = id
        self.role = role
        self.text = text
        self.reasoning = reasoning
        self.toolCalls = toolCalls
        self.isStreaming = isStreaming
        self.isQueued = isQueued
    }
}

/// A transient, user-dismissible notice surfaced to the UI.
///
/// Covers out-of-band server signals that must never be silently dropped:
/// push notifications, interrupts, and context compaction events.
public struct Notice: Equatable, Sendable, Identifiable {
    public enum Kind: Equatable, Sendable {
        case info
        case notification
        case compaction
    }

    public var id: UUID
    public var kind: Kind
    public var message: String

    public init(id: UUID = UUID(), kind: Kind = .info, message: String) {
        self.id = id
        self.kind = kind
        self.message = message
    }
}

/// Full client-side session state derived from server events.
public struct SessionState: Equatable, Sendable {
    public var phase: ConnectionPhase
    public var transcript: [TranscriptEntry]
    public var sessionID: String?
    public var sessionTitle: String?
    public var allSessions: [String]
    /// Human titles for sessions in `allSessions`, keyed by session ID.
    /// Populated from `session_renamed` broadcasts and history payloads.
    public var sessionTitles: [String: String]
    public var isProcessing: Bool
    public var isReasoning: Bool
    public var modelName: String?
    public var providerName: String?
    public var availableModels: [String]
    public var reasoningEffort: String?
    public var serverVersion: String?
    public var tokenInput: UInt64
    public var tokenOutput: UInt64
    public var statusDetail: String?
    /// Live turn-level provider phase (e.g. "authenticating", "waiting").
    public var serverPhase: String?
    public var errorBanner: String?
    public var notices: [Notice]
    /// Soft-interrupt messages queued mid-run, in send order, until the
    /// server confirms injection.
    public var pendingInterrupts: [String]

    public var hasPendingInterrupts: Bool { !pendingInterrupts.isEmpty }

    /// Human title for a session in the list, falling back to nil when unknown.
    public func title(forSession sessionID: String) -> String? {
        sessionTitles[sessionID]
    }

    public init() {
        phase = .disconnected
        transcript = []
        sessionID = nil
        sessionTitle = nil
        allSessions = []
        sessionTitles = [:]
        isProcessing = false
        isReasoning = false
        modelName = nil
        providerName = nil
        availableModels = []
        reasoningEffort = nil
        serverVersion = nil
        tokenInput = 0
        tokenOutput = 0
        statusDetail = nil
        serverPhase = nil
        errorBanner = nil
        notices = []
        pendingInterrupts = []
    }
}

/// Local intents that mutate state without a server round-trip.
public enum LocalIntent: Equatable, Sendable {
    /// User submitted a message; append it optimistically.
    case userSentMessage(String)
    /// User queued a soft-interrupt message mid-run.
    case userQueuedInterrupt(String)
    /// User cancelled all queued soft-interrupts before they injected; the
    /// optimistic bubbles are removed because they will never reach the agent.
    case cancelledQueuedInterrupts
    /// Dismiss the current error banner.
    case dismissError
    /// Dismiss a transient notice by id.
    case dismissNotice(UUID)
    /// User cleared the conversation; wipe the transcript optimistically while
    /// keeping the connection and session metadata.
    case clearedConversation
    /// Reset everything (switching servers/sessions).
    case reset
}

/// Pure state machine turning connection output into session state.
///
/// All streaming, tool lifecycle, and history-sync behavior lives here so it
/// can be exhaustively unit tested on macOS without UI or network.
public enum SessionReducer {
    public static func reduce(_ state: SessionState, _ output: ConnectionOutput) -> SessionState {
        switch output {
        case .phase(let phase):
            return reducePhase(state, phase)
        case .event(let event):
            return reduceEvent(state, event)
        }
    }

    public static func reduce(_ state: SessionState, intent: LocalIntent) -> SessionState {
        var state = state
        switch intent {
        case .userSentMessage(let text):
            state.transcript.append(TranscriptEntry(role: .user, text: text))
            state.isProcessing = true
            state.errorBanner = nil
        case .userQueuedInterrupt(let text):
            state.transcript.append(TranscriptEntry(role: .user, text: text, isQueued: true))
            state.pendingInterrupts.append(text)
        case .cancelledQueuedInterrupts:
            state.transcript.removeAll { $0.isQueued }
            state.pendingInterrupts = []
        case .dismissError:
            state.errorBanner = nil
        case .dismissNotice(let id):
            state.notices.removeAll { $0.id == id }
        case .clearedConversation:
            state.transcript = []
            state.isProcessing = false
            state.isReasoning = false
            state.errorBanner = nil
        case .reset:
            state = SessionState()
        }
        return state
    }

    // MARK: - Phase

    private static func reducePhase(_ state: SessionState, _ phase: ConnectionPhase)
        -> SessionState
    {
        var state = state
        state.phase = phase
        switch phase {
        case .connected:
            state.errorBanner = nil
        case .failed(let reason):
            state.errorBanner = reason
            state.isProcessing = false
            state.isReasoning = false
        case .disconnected, .reconnecting:
            state.isProcessing = false
            state.isReasoning = false
            state.serverPhase = nil
            finishStreaming(&state)
        case .connecting:
            break
        }
        return state
    }

    // MARK: - Events

    private static func reduceEvent(_ state: SessionState, _ event: ServerEvent) -> SessionState {
        var state = state
        switch event {
        case .textDelta(let text):
            withStreamingAssistant(&state) { $0.text += text }

        case .reasoningDelta(let text):
            state.isReasoning = true
            withStreamingAssistant(&state) { $0.reasoning += text }

        case .reasoningDone:
            state.isReasoning = false

        case .textReplace(let text):
            withStreamingAssistant(&state) { $0.text = text }

        case .toolStart(let id, let name):
            withStreamingAssistant(&state) { entry in
                entry.toolCalls.append(.init(id: id, name: name))
            }

        case .toolInput(let delta):
            withStreamingAssistant(&state) { entry in
                if !entry.toolCalls.isEmpty {
                    entry.toolCalls[entry.toolCalls.count - 1].input += delta
                }
            }

        case .toolExec(let id, let name):
            withStreamingAssistant(&state) { entry in
                if let index = entry.toolCalls.firstIndex(where: { $0.id == id }) {
                    entry.toolCalls[index].status = .running
                } else {
                    entry.toolCalls.append(.init(id: id, name: name, status: .running))
                }
            }

        case .toolDone(let id, let name, let output, let error):
            withStreamingAssistant(&state) { entry in
                if let index = entry.toolCalls.firstIndex(where: { $0.id == id }) {
                    entry.toolCalls[index].output = output
                    entry.toolCalls[index].status = error.map { .failed($0) } ?? .succeeded
                } else {
                    entry.toolCalls.append(
                        .init(
                            id: id, name: name, output: output,
                            status: error.map { .failed($0) } ?? .succeeded
                        ))
                }
            }

        case .messageEnd:
            finishStreaming(&state)

        case .connectionPhase(let phase):
            state.serverPhase = phase.isEmpty ? nil : phase

        case .softInterruptInjected(let content, let displayRole, _, _):
            resolvePendingInterrupt(&state, content: content, displayRole: displayRole)

        case .retryRollback(let attempt, let max):
            // The provider is replaying the response from the top: discard all
            // partial output from the current attempt so it does not duplicate.
            if let last = state.transcript.indices.last, state.transcript[last].isStreaming {
                state.transcript.removeLast()
            }
            state.isReasoning = false
            state.statusDetail = "Retrying (\(attempt)/\(max))"

        case .done:
            state.isProcessing = false
            state.isReasoning = false
            state.serverPhase = nil
            finishStreaming(&state)
            drainPendingInterrupts(&state)

        case .interrupted:
            state.isProcessing = false
            state.isReasoning = false
            state.serverPhase = nil
            finishStreaming(&state)
            drainPendingInterrupts(&state)
            state.notices.append(Notice(message: "Interrupted"))

        case .error(_, let message, let retryAfterSecs):
            state.isProcessing = false
            state.isReasoning = false
            state.serverPhase = nil
            finishStreaming(&state)
            if let retry = retryAfterSecs {
                state.errorBanner = "\(message) (retry in \(retry)s)"
            } else {
                state.errorBanner = message
            }

        case .tokenUsage(let input, let output):
            state.tokenInput = input
            state.tokenOutput = output

        case .statusDetail(let detail):
            state.statusDetail = detail

        case .state(_, let sessionID, _, let isProcessing):
            state.sessionID = sessionID
            state.isProcessing = isProcessing

        case .sessionID(let sessionID):
            state.sessionID = sessionID

        case .sessionRenamed(let sessionID, let displayTitle):
            state.sessionTitles[sessionID] = displayTitle
            if state.sessionID == nil || state.sessionID == sessionID {
                state.sessionTitle = displayTitle
            }

        case .history(let payload):
            state = applyHistory(state, payload)

        case .modelChanged(_, let model, let error):
            if let error {
                state.errorBanner = error
            } else {
                state.modelName = model
            }

        case .reasoningEffortChanged(_, let effort, let error):
            if let error {
                state.errorBanner = error
            } else {
                state.reasoningEffort = effort
            }

        case .compactResult(_, let message, let success):
            if success {
                state.notices.append(Notice(kind: .compaction, message: message))
            } else {
                state.errorBanner = message
            }

        case .availableModelsUpdated(let models, let providerModel):
            state.availableModels = models
            if let providerModel {
                state.modelName = providerModel
            }

        case .compaction(let trigger, let tokensSaved):
            if let saved = tokensSaved, trigger != "background" {
                state.notices.append(
                    Notice(kind: .compaction, message: "Context compacted (\(saved) tokens saved)"))
            }

        case .notification(let fromName, let message):
            let prefix = fromName.map { "\($0): " } ?? ""
            state.notices.append(Notice(kind: .notification, message: prefix + message))

        case .reloading:
            state.notices.append(Notice(message: "Server is updating, reconnecting shortly"))

        case .sessionCloseRequested(let reason):
            state.isProcessing = false
            state.isReasoning = false
            finishStreaming(&state)
            state.errorBanner = reason.isEmpty ? "Server closed this session" : reason

        case .ack, .pong, .unknown:
            break
        }
        return state
    }

    private static func applyHistory(
        _ state: SessionState, _ payload: ServerEvent.HistoryPayload
    ) -> SessionState {
        var state = state
        state.sessionID = payload.sessionID
        state.providerName = payload.providerName ?? state.providerName
        state.modelName = payload.providerModel ?? state.modelName
        if !payload.availableModels.isEmpty {
            state.availableModels = payload.availableModels
        }
        if !payload.allSessions.isEmpty {
            state.allSessions = payload.allSessions
        }
        state.serverVersion = payload.serverVersion ?? state.serverVersion
        state.sessionTitle = payload.displayTitle ?? state.sessionTitle
        state.reasoningEffort = payload.reasoningEffort ?? state.reasoningEffort
        if let title = payload.displayTitle {
            state.sessionTitles[payload.sessionID] = title
        }
        // History replaces the transcript wholesale, so optimistic queued
        // bubbles are rebuilt from the server's authoritative view.
        state.pendingInterrupts = []
        if let totals = payload.totalTokens {
            state.tokenInput = totals.input
            state.tokenOutput = totals.output
        }

        // History replaces the transcript wholesale: it is the server's
        // authoritative view, used on connect and reconnect.
        //
        // One exception: a message sent optimistically just before this
        // payload was produced is not in it yet. Dropping it makes the user's
        // own bubble vanish the instant they send during connect, so trailing
        // local user entries the server has not caught up to are re-appended.
        let optimisticTail = state.transcript.reversed()
            .prefix { $0.role == .user && !$0.isQueued }
            .reversed()
        let historyUserTexts = Set(
            payload.messages.filter { $0.role == "user" }.map(\.content))
        state.transcript = payload.messages.compactMap { message in
            let role: TranscriptEntry.Role
            switch message.role {
            case "user": role = .user
            case "assistant": role = .assistant
            case "system": role = .system
            default: return nil
            }
            var toolCalls: [TranscriptEntry.ToolCall] = []
            if let data = message.toolData {
                toolCalls.append(
                    .init(
                        id: data.id,
                        name: data.name,
                        input: data.input,
                        output: data.output ?? "",
                        status: data.error.map { .failed($0) }
                            ?? (data.output != nil ? .succeeded : .running)
                    ))
            } else {
                toolCalls = message.toolCalls.map { name in
                    .init(id: name, name: name, status: .succeeded)
                }
            }
            // Skip empty assistant placeholders.
            if message.content.isEmpty && toolCalls.isEmpty {
                return nil
            }
            return TranscriptEntry(role: role, text: message.content, toolCalls: toolCalls)
        }
        state.transcript.append(
            contentsOf: optimisticTail.filter { !historyUserTexts.contains($0.text) })
        return state
    }

    // MARK: - Helpers

    /// Marks the matching queued soft-interrupt as delivered. If the injection
    /// came from another client (no optimistic bubble), appends the content so
    /// the transcript still reflects what the agent saw.
    private static func resolvePendingInterrupt(
        _ state: inout SessionState, content: String, displayRole: String?
    ) {
        if let index = state.pendingInterrupts.firstIndex(of: content) {
            state.pendingInterrupts.remove(at: index)
        }
        if let index = state.transcript.firstIndex(where: { $0.isQueued && $0.text == content })
            ?? state.transcript.firstIndex(where: { $0.isQueued })
        {
            state.transcript[index].isQueued = false
        } else {
            let role: TranscriptEntry.Role = displayRole == "system" ? .system : .user
            state.transcript.append(TranscriptEntry(role: role, text: content))
        }
    }

    /// The turn is over: anything still marked queued has either been consumed
    /// server-side or is moot, so stop showing it as pending.
    private static func drainPendingInterrupts(_ state: inout SessionState) {
        state.pendingInterrupts = []
        for index in state.transcript.indices where state.transcript[index].isQueued {
            state.transcript[index].isQueued = false
        }
    }

    /// Mutates the trailing streaming assistant entry, creating it if needed.
    private static func withStreamingAssistant(
        _ state: inout SessionState, _ mutate: (inout TranscriptEntry) -> Void
    ) {
        if let last = state.transcript.indices.last,
            state.transcript[last].role == .assistant,
            state.transcript[last].isStreaming
        {
            mutate(&state.transcript[last])
        } else {
            var entry = TranscriptEntry(role: .assistant, text: "", isStreaming: true)
            mutate(&entry)
            state.transcript.append(entry)
        }
    }

    private static func finishStreaming(_ state: inout SessionState) {
        if let last = state.transcript.indices.last, state.transcript[last].isStreaming {
            state.transcript[last].isStreaming = false
            // Drop fully-empty assistant stubs (e.g. tool-only turns that
            // were replaced or cancelled before any text arrived).
            if state.transcript[last].text.isEmpty
                && state.transcript[last].toolCalls.isEmpty
                && state.transcript[last].reasoning.isEmpty
            {
                state.transcript.removeLast()
            }
        }
    }
}
