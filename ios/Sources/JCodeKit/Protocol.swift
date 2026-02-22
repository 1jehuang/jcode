import Foundation

// MARK: - Client Requests

public enum Request: Encodable, Sendable {
    case subscribe(id: UInt64, workingDir: String? = nil)
    case message(id: UInt64, content: String, images: [(String, String)] = [])
    case cancel(id: UInt64)
    case ping(id: UInt64)
    case getHistory(id: UInt64)
    case getState(id: UInt64)
    case clear(id: UInt64)
    case resumeSession(id: UInt64, sessionId: String)
    case cycleModel(id: UInt64, direction: Int8 = 1)
    case setModel(id: UInt64, model: String)
    case compact(id: UInt64)
    case softInterrupt(id: UInt64, content: String, urgent: Bool = false)
    case cancelSoftInterrupts(id: UInt64)
    case backgroundTool(id: UInt64)
    case split(id: UInt64)

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: DynamicCodingKey.self)

        switch self {
        case let .subscribe(id, workingDir):
            try container.encode("subscribe", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
            if let wd = workingDir {
                try container.encode(wd, forKey: .key("working_dir"))
            }

        case let .message(id, content, images):
            try container.encode("message", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
            try container.encode(content, forKey: .key("content"))
            if !images.isEmpty {
                let pairs = images.map { [$0.0, $0.1] }
                try container.encode(pairs, forKey: .key("images"))
            }

        case let .cancel(id):
            try container.encode("cancel", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .ping(id):
            try container.encode("ping", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .getHistory(id):
            try container.encode("get_history", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .getState(id):
            try container.encode("state", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .clear(id):
            try container.encode("clear", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .resumeSession(id, sessionId):
            try container.encode("resume_session", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
            try container.encode(sessionId, forKey: .key("session_id"))

        case let .cycleModel(id, direction):
            try container.encode("cycle_model", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
            try container.encode(direction, forKey: .key("direction"))

        case let .setModel(id, model):
            try container.encode("set_model", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
            try container.encode(model, forKey: .key("model"))

        case let .compact(id):
            try container.encode("compact", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .softInterrupt(id, content, urgent):
            try container.encode("soft_interrupt", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
            try container.encode(content, forKey: .key("content"))
            if urgent {
                try container.encode(urgent, forKey: .key("urgent"))
            }

        case let .cancelSoftInterrupts(id):
            try container.encode("cancel_soft_interrupts", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .backgroundTool(id):
            try container.encode("background_tool", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))

        case let .split(id):
            try container.encode("split", forKey: .key("type"))
            try container.encode(id, forKey: .key("id"))
        }
    }
}

// MARK: - Server Events

public enum ServerEvent: Decodable, Sendable {
    case ack(id: UInt64)
    case textDelta(text: String)
    case toolStart(id: String, name: String)
    case toolInput(delta: String)
    case toolExec(id: String, name: String)
    case toolDone(id: String, name: String, output: String, error: String?)
    case tokenUsage(input: UInt64, output: UInt64, cacheRead: UInt64?, cacheWrite: UInt64?)
    case upstreamProvider(provider: String)
    case done(id: UInt64)
    case error(id: UInt64, message: String)
    case pong(id: UInt64)
    case state(id: UInt64, sessionId: String, messageCount: Int, isProcessing: Bool)
    case sessionId(sessionId: String)
    case history(HistoryPayload)
    case reloading(newSocket: String?)
    case reloadProgress(step: String, message: String, success: Bool?, output: String?)
    case modelChanged(id: UInt64, model: String, providerName: String?, error: String?)
    case notification(Notification)
    case swarmStatus(members: [SwarmMemberStatus])
    case mcpStatus(servers: [String])
    case softInterruptInjected(content: String, point: String, toolsSkipped: Int?)
    case memoryInjected(count: Int, prompt: String, promptChars: Int, computedAgeMs: UInt64)
    case splitResponse(id: UInt64, newSessionId: String, newSessionName: String)
    case compactResult(id: UInt64, message: String, success: Bool)
    case unknown(type: String, raw: String)

    enum CodingKeys: String, CodingKey {
        case type
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: DynamicCodingKey.self)
        let type = try container.decode(String.self, forKey: .key("type"))

        switch type {
        case "ack":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            self = .ack(id: id)

        case "text_delta":
            let text = try container.decode(String.self, forKey: .key("text"))
            self = .textDelta(text: text)

        case "tool_start":
            let id = try container.decode(String.self, forKey: .key("id"))
            let name = try container.decode(String.self, forKey: .key("name"))
            self = .toolStart(id: id, name: name)

        case "tool_input":
            let delta = try container.decode(String.self, forKey: .key("delta"))
            self = .toolInput(delta: delta)

        case "tool_exec":
            let id = try container.decode(String.self, forKey: .key("id"))
            let name = try container.decode(String.self, forKey: .key("name"))
            self = .toolExec(id: id, name: name)

        case "tool_done":
            let id = try container.decode(String.self, forKey: .key("id"))
            let name = try container.decode(String.self, forKey: .key("name"))
            let output = try container.decode(String.self, forKey: .key("output"))
            let error = try container.decodeIfPresent(String.self, forKey: .key("error"))
            self = .toolDone(id: id, name: name, output: output, error: error)

        case "tokens":
            let input = try container.decode(UInt64.self, forKey: .key("input"))
            let output = try container.decode(UInt64.self, forKey: .key("output"))
            let cacheRead = try container.decodeIfPresent(UInt64.self, forKey: .key("cache_read_input"))
            let cacheWrite = try container.decodeIfPresent(UInt64.self, forKey: .key("cache_creation_input"))
            self = .tokenUsage(input: input, output: output, cacheRead: cacheRead, cacheWrite: cacheWrite)

        case "upstream_provider":
            let provider = try container.decode(String.self, forKey: .key("provider"))
            self = .upstreamProvider(provider: provider)

        case "done":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            self = .done(id: id)

        case "error":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            let message = try container.decode(String.self, forKey: .key("message"))
            self = .error(id: id, message: message)

        case "pong":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            self = .pong(id: id)

        case "state":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            let sessionId = try container.decode(String.self, forKey: .key("session_id"))
            let messageCount = try container.decode(Int.self, forKey: .key("message_count"))
            let isProcessing = try container.decode(Bool.self, forKey: .key("is_processing"))
            self = .state(id: id, sessionId: sessionId, messageCount: messageCount, isProcessing: isProcessing)

        case "session":
            let sessionId = try container.decode(String.self, forKey: .key("session_id"))
            self = .sessionId(sessionId: sessionId)

        case "history":
            let payload = try HistoryPayload(from: decoder)
            self = .history(payload)

        case "reloading":
            let newSocket = try container.decodeIfPresent(String.self, forKey: .key("new_socket"))
            self = .reloading(newSocket: newSocket)

        case "reload_progress":
            let step = try container.decode(String.self, forKey: .key("step"))
            let message = try container.decode(String.self, forKey: .key("message"))
            let success = try container.decodeIfPresent(Bool.self, forKey: .key("success"))
            let output = try container.decodeIfPresent(String.self, forKey: .key("output"))
            self = .reloadProgress(step: step, message: message, success: success, output: output)

        case "model_changed":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            let model = try container.decode(String.self, forKey: .key("model"))
            let providerName = try container.decodeIfPresent(String.self, forKey: .key("provider_name"))
            let error = try container.decodeIfPresent(String.self, forKey: .key("error"))
            self = .modelChanged(id: id, model: model, providerName: providerName, error: error)

        case "notification":
            let notif = try Notification(from: decoder)
            self = .notification(notif)

        case "swarm_status":
            let members = try container.decode([SwarmMemberStatus].self, forKey: .key("members"))
            self = .swarmStatus(members: members)

        case "mcp_status":
            let servers = try container.decode([String].self, forKey: .key("servers"))
            self = .mcpStatus(servers: servers)

        case "soft_interrupt_injected":
            let content = try container.decode(String.self, forKey: .key("content"))
            let point = try container.decode(String.self, forKey: .key("point"))
            let toolsSkipped = try container.decodeIfPresent(Int.self, forKey: .key("tools_skipped"))
            self = .softInterruptInjected(content: content, point: point, toolsSkipped: toolsSkipped)

        case "memory_injected":
            let count = try container.decode(Int.self, forKey: .key("count"))
            let prompt = try container.decodeIfPresent(String.self, forKey: .key("prompt")) ?? ""
            let promptChars = try container.decodeIfPresent(Int.self, forKey: .key("prompt_chars")) ?? 0
            let computedAgeMs = try container.decodeIfPresent(UInt64.self, forKey: .key("computed_age_ms")) ?? 0
            self = .memoryInjected(count: count, prompt: prompt, promptChars: promptChars, computedAgeMs: computedAgeMs)

        case "split_response":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            let newSessionId = try container.decode(String.self, forKey: .key("new_session_id"))
            let newSessionName = try container.decode(String.self, forKey: .key("new_session_name"))
            self = .splitResponse(id: id, newSessionId: newSessionId, newSessionName: newSessionName)

        case "compact_result":
            let id = try container.decode(UInt64.self, forKey: .key("id"))
            let message = try container.decode(String.self, forKey: .key("message"))
            let success = try container.decode(Bool.self, forKey: .key("success"))
            self = .compactResult(id: id, message: message, success: success)

        default:
            let raw = String(describing: try? JSONSerialization.data(withJSONObject: [:]))
            self = .unknown(type: type, raw: raw)
        }
    }
}

// MARK: - Supporting Types

public struct HistoryMessage: Codable, Sendable {
    public let role: String
    public let content: String
    public let toolCalls: [String]?
    public let toolData: ToolCallData?

    enum CodingKeys: String, CodingKey {
        case role, content
        case toolCalls = "tool_calls"
        case toolData = "tool_data"
    }
}

public struct ToolCallData: Codable, Sendable {
    public let id: String?
    public let name: String?
    public let input: String?
    public let output: String?
}

public struct HistoryPayload: Decodable, Sendable {
    public let id: UInt64
    public let sessionId: String
    public let messages: [HistoryMessage]
    public let providerName: String?
    public let providerModel: String?
    public let availableModels: [String]
    public let mcpServers: [String]
    public let skills: [String]
    public let totalTokens: (UInt64, UInt64)?
    public let allSessions: [String]
    public let clientCount: Int?
    public let isCanary: Bool?
    public let serverVersion: String?
    public let serverName: String?
    public let serverIcon: String?
    public let serverHasUpdate: Bool?
    public let wasInterrupted: Bool?

    enum CodingKeys: String, CodingKey {
        case id
        case sessionId = "session_id"
        case messages
        case providerName = "provider_name"
        case providerModel = "provider_model"
        case availableModels = "available_models"
        case mcpServers = "mcp_servers"
        case skills
        case totalTokens = "total_tokens"
        case allSessions = "all_sessions"
        case clientCount = "client_count"
        case isCanary = "is_canary"
        case serverVersion = "server_version"
        case serverName = "server_name"
        case serverIcon = "server_icon"
        case serverHasUpdate = "server_has_update"
        case wasInterrupted = "was_interrupted"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(UInt64.self, forKey: .id)
        sessionId = try container.decode(String.self, forKey: .sessionId)
        messages = try container.decode([HistoryMessage].self, forKey: .messages)
        providerName = try container.decodeIfPresent(String.self, forKey: .providerName)
        providerModel = try container.decodeIfPresent(String.self, forKey: .providerModel)
        availableModels = try container.decodeIfPresent([String].self, forKey: .availableModels) ?? []
        mcpServers = try container.decodeIfPresent([String].self, forKey: .mcpServers) ?? []
        skills = try container.decodeIfPresent([String].self, forKey: .skills) ?? []
        if let arr = try container.decodeIfPresent([UInt64].self, forKey: .totalTokens), arr.count == 2 {
            totalTokens = (arr[0], arr[1])
        } else {
            totalTokens = nil
        }
        allSessions = try container.decodeIfPresent([String].self, forKey: .allSessions) ?? []
        clientCount = try container.decodeIfPresent(Int.self, forKey: .clientCount)
        isCanary = try container.decodeIfPresent(Bool.self, forKey: .isCanary)
        serverVersion = try container.decodeIfPresent(String.self, forKey: .serverVersion)
        serverName = try container.decodeIfPresent(String.self, forKey: .serverName)
        serverIcon = try container.decodeIfPresent(String.self, forKey: .serverIcon)
        serverHasUpdate = try container.decodeIfPresent(Bool.self, forKey: .serverHasUpdate)
        wasInterrupted = try container.decodeIfPresent(Bool.self, forKey: .wasInterrupted)
    }
}

public struct SwarmMemberStatus: Codable, Sendable {
    public let sessionId: String
    public let friendlyName: String?
    public let status: String
    public let detail: String?
    public let role: String?

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case friendlyName = "friendly_name"
        case status, detail, role
    }
}

public struct Notification: Decodable, Sendable {
    public let fromSession: String
    public let fromName: String?
    public let notificationType: NotificationType
    public let message: String

    enum CodingKeys: String, CodingKey {
        case fromSession = "from_session"
        case fromName = "from_name"
        case notificationType = "notification_type"
        case message
    }
}

public enum NotificationType: Decodable, Sendable {
    case fileConflict(path: String, operation: String)
    case sharedContext(key: String, value: String)
    case message(scope: String?, channel: String?)

    enum CodingKeys: String, CodingKey {
        case kind, path, operation, key, value, scope, channel
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "file_conflict":
            let path = try container.decode(String.self, forKey: .path)
            let operation = try container.decode(String.self, forKey: .operation)
            self = .fileConflict(path: path, operation: operation)
        case "shared_context":
            let key = try container.decode(String.self, forKey: .key)
            let value = try container.decode(String.self, forKey: .value)
            self = .sharedContext(key: key, value: value)
        case "message":
            let scope = try container.decodeIfPresent(String.self, forKey: .scope)
            let channel = try container.decodeIfPresent(String.self, forKey: .channel)
            self = .message(scope: scope, channel: channel)
        default:
            self = .message(scope: nil, channel: nil)
        }
    }
}

// MARK: - Dynamic Coding Key

struct DynamicCodingKey: CodingKey {
    var stringValue: String
    var intValue: Int? { nil }

    init?(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { return nil }

    static func key(_ name: String) -> DynamicCodingKey {
        DynamicCodingKey(stringValue: name)!
    }
}
