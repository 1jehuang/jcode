import Foundation

/// Client request to a jcode server.
///
/// Wire format mirrors `crates/jcode-protocol/src/wire.rs` (`#[serde(tag = "type")]`,
/// snake_case tags). Only the requests the iOS app uses are modeled; the server
/// ignores fields it does not expect.
public enum Request: Equatable, Sendable {
    case subscribe(id: UInt64, targetSessionID: String?, workingDir: String?)
    case message(id: UInt64, content: String)
    case cancel(id: UInt64)
    case softInterrupt(id: UInt64, content: String, urgent: Bool)
    case cancelSoftInterrupts(id: UInt64)
    case ping(id: UInt64)
    case getHistory(id: UInt64)
    case resumeSession(id: UInt64, sessionID: String)
    case setModel(id: UInt64, model: String)
    case setReasoningEffort(id: UInt64, effort: String)
    case compact(id: UInt64)
    case renameSession(id: UInt64, title: String?)
    case clear(id: UInt64)

    public var id: UInt64 {
        switch self {
        case let .subscribe(id, _, _), let .message(id, _), let .cancel(id),
            let .softInterrupt(id, _, _), let .cancelSoftInterrupts(id),
            let .ping(id), let .getHistory(id), let .resumeSession(id, _),
            let .setModel(id, _), let .setReasoningEffort(id, _), let .compact(id),
            let .renameSession(id, _), let .clear(id):
            return id
        }
    }

    /// Encodes the request as a single JSON line (no trailing newline).
    public func encodedLine() throws -> String {
        var object: [String: Any] = ["id": id]
        switch self {
        case let .subscribe(_, targetSessionID, workingDir):
            object["type"] = "subscribe"
            if let targetSessionID {
                object["target_session_id"] = targetSessionID
            }
            // The server rejects a subscribe without an absolute working_dir.
            // A phone has no directory on the host, so it echoes back the one
            // the gateway advertised at pair/health time.
            if let workingDir, !workingDir.isEmpty {
                object["working_dir"] = workingDir
            }
        case let .message(_, content):
            object["type"] = "message"
            object["content"] = content
        case .cancel:
            object["type"] = "cancel"
        case let .softInterrupt(_, content, urgent):
            object["type"] = "soft_interrupt"
            object["content"] = content
            object["urgent"] = urgent
        case .cancelSoftInterrupts:
            object["type"] = "cancel_soft_interrupts"
        case .ping:
            object["type"] = "ping"
        case .getHistory:
            object["type"] = "get_history"
        case let .resumeSession(_, sessionID):
            object["type"] = "resume_session"
            object["session_id"] = sessionID
        case let .setModel(_, model):
            object["type"] = "set_model"
            object["model"] = model
        case let .setReasoningEffort(_, effort):
            object["type"] = "set_reasoning_effort"
            object["effort"] = effort
        case .compact:
            object["type"] = "compact"
        case let .renameSession(_, title):
            object["type"] = "rename_session"
            if let title {
                object["title"] = title
            }
        case .clear:
            object["type"] = "clear"
        }
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        guard let line = String(data: data, encoding: .utf8) else {
            throw WireError.encodingFailed
        }
        return line
    }
}

/// A tool call as represented in history payloads.
public struct ToolCallRecord: Equatable, Sendable {
    public var id: String
    public var name: String
    public var input: String
    public var output: String?
    public var error: String?

    public init(id: String, name: String, input: String, output: String?, error: String?) {
        self.id = id
        self.name = name
        self.input = input
        self.output = output
        self.error = error
    }
}

/// A message in conversation history (response to `get_history`).
public struct HistoryMessage: Equatable, Sendable {
    public var role: String
    public var content: String
    public var toolCalls: [String]
    public var toolData: ToolCallRecord?

    public init(
        role: String, content: String, toolCalls: [String] = [], toolData: ToolCallRecord? = nil
    ) {
        self.role = role
        self.content = content
        self.toolCalls = toolCalls
        self.toolData = toolData
    }
}

/// Server event from a jcode server. Unknown event types decode as `.unknown`
/// so newer servers never break older apps.
public enum ServerEvent: Equatable, Sendable {
    case ack(id: UInt64)
    case textDelta(text: String)
    case reasoningDelta(text: String)
    case reasoningDone(durationSecs: Double?)
    case textReplace(text: String)
    case toolStart(id: String, name: String)
    case toolInput(delta: String)
    case toolExec(id: String, name: String)
    case toolDone(id: String, name: String, output: String, error: String?)
    case tokenUsage(input: UInt64, output: UInt64)
    case statusDetail(detail: String)
    case connectionPhase(phase: String)
    case softInterruptInjected(content: String, displayRole: String?, point: String, toolsSkipped: Int?)
    case retryRollback(attempt: Int, max: Int)
    case messageEnd
    case interrupted
    case done(id: UInt64)
    case error(id: UInt64, message: String, retryAfterSecs: UInt64?)
    case pong(id: UInt64)
    case state(id: UInt64, sessionID: String, messageCount: Int, isProcessing: Bool)
    case sessionID(sessionID: String)
    case sessionRenamed(sessionID: String, displayTitle: String)
    case history(HistoryPayload)
    case modelChanged(id: UInt64, model: String, error: String?)
    case reasoningEffortChanged(id: UInt64, effort: String?, error: String?)
    case compactResult(id: UInt64, message: String, success: Bool)
    case availableModelsUpdated(models: [String], providerModel: String?)
    case compaction(trigger: String, tokensSaved: UInt64?)
    case notification(fromName: String?, message: String)
    case reloading(newSocket: String?)
    case sessionCloseRequested(reason: String)
    case unknown(type: String)

    public struct HistoryPayload: Equatable, Sendable {
        public var id: UInt64
        public var sessionID: String
        public var messages: [HistoryMessage]
        public var providerName: String?
        public var providerModel: String?
        public var availableModels: [String]
        public var totalTokens: TokenTotals?
        public var allSessions: [String]
        public var serverVersion: String?
        public var displayTitle: String?
        public var reasoningEffort: String?

        public struct TokenTotals: Equatable, Sendable {
            public var input: UInt64
            public var output: UInt64

            public init(input: UInt64, output: UInt64) {
                self.input = input
                self.output = output
            }
        }

        public init(
            id: UInt64,
            sessionID: String,
            messages: [HistoryMessage],
            providerName: String? = nil,
            providerModel: String? = nil,
            availableModels: [String] = [],
            totalTokens: TokenTotals? = nil,
            allSessions: [String] = [],
            serverVersion: String? = nil,
            displayTitle: String? = nil,
            reasoningEffort: String? = nil
        ) {
            self.id = id
            self.sessionID = sessionID
            self.messages = messages
            self.providerName = providerName
            self.providerModel = providerModel
            self.availableModels = availableModels
            self.totalTokens = totalTokens
            self.allSessions = allSessions
            self.serverVersion = serverVersion
            self.displayTitle = displayTitle
            self.reasoningEffort = reasoningEffort
        }
    }

    /// Decodes one newline-delimited JSON event line.
    public static func decode(line: String) throws -> ServerEvent {
        guard let data = line.data(using: .utf8),
            let parsed = try? JSONSerialization.jsonObject(with: data),
            let object = parsed as? [String: Any]
        else {
            throw WireError.invalidJSON(line: line)
        }
        guard let type = object["type"] as? String else {
            throw WireError.missingType(line: line)
        }
        let json = JSONObject(object)
        switch type {
        case "ack":
            return .ack(id: json.uint64("id"))
        case "text_delta":
            return .textDelta(text: json.string("text"))
        case "reasoning_delta":
            return .reasoningDelta(text: json.string("text"))
        case "reasoning_done":
            return .reasoningDone(durationSecs: json.optionalDouble("duration_secs"))
        case "text_replace":
            return .textReplace(text: json.string("text"))
        case "tool_start":
            return .toolStart(id: json.string("id"), name: json.string("name"))
        case "tool_input":
            return .toolInput(delta: json.string("delta"))
        case "tool_exec":
            return .toolExec(id: json.string("id"), name: json.string("name"))
        case "tool_done":
            return .toolDone(
                id: json.string("id"),
                name: json.string("name"),
                output: json.string("output"),
                error: json.optionalString("error")
            )
        case "tokens":
            return .tokenUsage(input: json.uint64("input"), output: json.uint64("output"))
        case "status_detail":
            return .statusDetail(detail: json.string("detail"))
        case "connection_phase":
            return .connectionPhase(phase: json.string("phase"))
        case "soft_interrupt_injected":
            return .softInterruptInjected(
                content: json.string("content"),
                displayRole: json.optionalString("display_role"),
                point: json.string("point"),
                toolsSkipped: json.optionalInt("tools_skipped")
            )
        case "retry_rollback":
            return .retryRollback(attempt: json.int("attempt"), max: json.int("max"))
        case "message_end":
            return .messageEnd
        case "interrupted":
            return .interrupted
        case "done":
            return .done(id: json.uint64("id"))
        case "error":
            return .error(
                id: json.uint64("id"),
                message: json.string("message"),
                retryAfterSecs: json.optionalUInt64("retry_after_secs")
            )
        case "pong":
            return .pong(id: json.uint64("id"))
        case "state":
            return .state(
                id: json.uint64("id"),
                sessionID: json.string("session_id"),
                messageCount: json.int("message_count"),
                isProcessing: json.bool("is_processing")
            )
        case "session":
            return .sessionID(sessionID: json.string("session_id"))
        case "session_renamed":
            return .sessionRenamed(
                sessionID: json.string("session_id"),
                displayTitle: json.string("display_title")
            )
        case "history":
            return .history(decodeHistory(json))
        case "model_changed":
            return .modelChanged(
                id: json.uint64("id"),
                model: json.string("model"),
                error: json.optionalString("error")
            )
        case "reasoning_effort_changed":
            return .reasoningEffortChanged(
                id: json.uint64("id"),
                effort: json.optionalString("effort"),
                error: json.optionalString("error")
            )
        case "compact_result":
            return .compactResult(
                id: json.uint64("id"),
                message: json.string("message"),
                success: json.bool("success")
            )
        case "available_models_updated":
            return .availableModelsUpdated(
                models: json.stringArray("available_models"),
                providerModel: json.optionalString("provider_model")
            )
        case "compaction":
            return .compaction(
                trigger: json.string("trigger"),
                tokensSaved: json.optionalUInt64("tokens_saved")
            )
        case "notification":
            return .notification(
                fromName: json.optionalString("from_name"),
                message: json.string("message")
            )
        case "reloading":
            return .reloading(newSocket: json.optionalString("new_socket"))
        case "session_close_requested":
            return .sessionCloseRequested(reason: json.string("reason"))
        default:
            return .unknown(type: type)
        }
    }

    private static func decodeHistory(_ json: JSONObject) -> HistoryPayload {
        let messages = json.objectArray("messages").map { msg -> HistoryMessage in
            var toolData: ToolCallRecord?
            if let td = msg.optionalObject("tool_data") {
                toolData = ToolCallRecord(
                    id: td.string("id"),
                    name: td.string("name"),
                    input: td.string("input"),
                    output: td.optionalString("output"),
                    error: td.optionalString("error")
                )
            }
            return HistoryMessage(
                role: msg.string("role"),
                content: msg.string("content"),
                toolCalls: msg.stringArray("tool_calls"),
                toolData: toolData
            )
        }
        var totals: HistoryPayload.TokenTotals?
        if let pair = json.raw["total_tokens"] as? [Any], pair.count == 2,
            let input = JSONObject.coerceUInt64(pair[0]),
            let output = JSONObject.coerceUInt64(pair[1])
        {
            totals = HistoryPayload.TokenTotals(input: input, output: output)
        }
        return HistoryPayload(
            id: json.uint64("id"),
            sessionID: json.string("session_id"),
            messages: messages,
            providerName: json.optionalString("provider_name"),
            providerModel: json.optionalString("provider_model"),
            availableModels: json.stringArray("available_models"),
            totalTokens: totals,
            allSessions: json.stringArray("all_sessions"),
            serverVersion: json.optionalString("server_version"),
            displayTitle: json.optionalString("display_title"),
            reasoningEffort: json.optionalString("reasoning_effort")
        )
    }
}

public enum WireError: Error, Equatable {
    case encodingFailed
    case invalidJSON(line: String)
    case missingType(line: String)
}

/// Lenient JSON accessor. The wire protocol omits absent optionals and the
/// app must never crash on a server that is newer or older than itself.
struct JSONObject {
    let raw: [String: Any]

    init(_ raw: [String: Any]) {
        self.raw = raw
    }

    func string(_ key: String) -> String {
        raw[key] as? String ?? ""
    }

    func optionalString(_ key: String) -> String? {
        raw[key] as? String
    }

    func bool(_ key: String) -> Bool {
        raw[key] as? Bool ?? false
    }

    func int(_ key: String) -> Int {
        (raw[key] as? NSNumber)?.intValue ?? 0
    }

    func optionalInt(_ key: String) -> Int? {
        (raw[key] as? NSNumber)?.intValue
    }

    func uint64(_ key: String) -> UInt64 {
        Self.coerceUInt64(raw[key] ?? 0) ?? 0
    }

    func optionalUInt64(_ key: String) -> UInt64? {
        raw[key].flatMap(Self.coerceUInt64)
    }

    func optionalDouble(_ key: String) -> Double? {
        (raw[key] as? NSNumber)?.doubleValue
    }

    func stringArray(_ key: String) -> [String] {
        raw[key] as? [String] ?? []
    }

    func objectArray(_ key: String) -> [JSONObject] {
        (raw[key] as? [[String: Any]])?.map(JSONObject.init) ?? []
    }

    func optionalObject(_ key: String) -> JSONObject? {
        (raw[key] as? [String: Any]).map(JSONObject.init)
    }

    static func coerceUInt64(_ value: Any) -> UInt64? {
        if let number = value as? NSNumber {
            return number.uint64Value
        }
        return nil
    }
}
