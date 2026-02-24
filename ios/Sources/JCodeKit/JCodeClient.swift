import Foundation

public struct MessageContent: Sendable {
    public let role: MessageRole
    public let text: String
    public let toolCalls: [ToolCallInfo]

    public init(role: MessageRole, text: String, toolCalls: [ToolCallInfo] = []) {
        self.role = role
        self.text = text
        self.toolCalls = toolCalls
    }
}

public enum MessageRole: String, Sendable {
    case user
    case assistant
    case system
}

public struct ToolCallInfo: Sendable {
    public let id: String
    public let name: String
    public var input: String
    public var output: String?
    public var error: String?
    public var state: ToolCallState

    public init(id: String, name: String) {
        self.id = id
        self.name = name
        self.input = ""
        self.output = nil
        self.error = nil
        self.state = .streaming
    }
}

public enum ToolCallState: Sendable {
    case streaming
    case executing
    case done
    case failed
}

public struct ServerInfo: Sendable {
    public var sessionId: String = ""
    public var serverName: String?
    public var serverIcon: String?
    public var serverVersion: String?
    public var providerName: String?
    public var providerModel: String?
    public var availableModels: [String] = []
    public var allSessions: [String] = []
    public var isCanary: Bool = false
    public var wasInterrupted: Bool = false
    public var totalInputTokens: UInt64 = 0
    public var totalOutputTokens: UInt64 = 0
}

public struct TokenUpdate: Sendable {
    public let input: UInt64
    public let output: UInt64
    public let cacheRead: UInt64?
    public let cacheWrite: UInt64?
}

public struct InterruptInfo: Sendable {
    public let message: String

    public init(message: String = "Interrupted") {
        self.message = message
    }
}

@MainActor
public protocol JCodeClientDelegate: AnyObject {
    func clientDidConnect(serverInfo: ServerInfo)
    func clientDidDisconnect(error: String?)
    func clientDidReceiveText(_ text: String)
    func clientDidReplaceText(_ text: String)
    func clientDidStartTool(_ tool: ToolCallInfo)
    func clientDidReceiveToolInput(_ delta: String)
    func clientDidExecuteTool(id: String, name: String)
    func clientDidFinishTool(id: String, name: String, output: String, error: String?)
    func clientDidFinishTurn(id: UInt64)
    func clientDidReceiveError(id: UInt64, message: String)
    func clientDidUpdateTokens(_ update: TokenUpdate)
    func clientDidChangeModel(model: String, provider: String?)
    func clientDidReceiveHistory(messages: [HistoryMessage])
    func clientDidInterrupt(_ interrupt: InterruptInfo)
}

@MainActor
public extension JCodeClientDelegate {
    func clientDidReplaceText(_ text: String) {}
    func clientDidReceiveToolInput(_ delta: String) {}
    func clientDidUpdateTokens(_ update: TokenUpdate) {}
    func clientDidChangeModel(model: String, provider: String?) {}
    func clientDidReceiveHistory(messages: [HistoryMessage]) {}
    func clientDidInterrupt(_ interrupt: InterruptInfo) {}
}

public actor JCodeClient {
    private let connection: JCodeConnection
    private nonisolated(unsafe) weak var _delegate: (any JCodeClientDelegate)?
    private var serverInfo = ServerInfo()
    private var eventTask: Task<Void, Never>?

    public init(host: String, port: UInt16 = 7643, authToken: String) {
        self.connection = JCodeConnection(host: host, port: port, authToken: authToken)
    }

    public func setDelegate(_ delegate: any JCodeClientDelegate) {
        self._delegate = delegate
    }

    public func connect(workingDir: String? = nil) async throws {
        let stream = await connection.events()
        eventTask = Task { [weak self] in
            for await event in stream {
                guard let self else { return }
                await self.handleEvent(event)
            }
        }
        try await connection.connect(workingDir: workingDir)
        let _ = try await connection.requestHistory()
    }

    public func disconnect() async {
        eventTask?.cancel()
        eventTask = nil
        await connection.disconnect()
    }

    public func send(_ message: String) async throws -> UInt64 {
        try await connection.sendMessage(message)
    }

    public func cancel() async throws {
        try await connection.cancelGeneration()
    }

    public func interrupt(_ message: String, urgent: Bool = false) async throws {
        try await connection.interrupt(message, urgent: urgent)
    }

    public func switchSession(_ sessionId: String) async throws {
        try await connection.resumeSession(sessionId)
        let _ = try await connection.requestHistory()
    }

    public func changeModel(_ model: String) async throws {
        try await connection.setModel(model)
    }

    public func refreshHistory() async throws {
        let _ = try await connection.requestHistory()
    }

    public func getServerInfo() -> ServerInfo {
        serverInfo
    }

    private func handleEvent(_ event: JCodeConnection.Event) async {
        switch event {
        case .stateChanged(let state):
            switch state {
            case .connected:
                break
            case .disconnected:
                await callDelegate { $0.clientDidDisconnect(error: nil) }
            case .error(let msg):
                await callDelegate { $0.clientDidDisconnect(error: msg) }
            case .connecting:
                break
            }

        case .serverEvent(let serverEvent):
            await handleServerEvent(serverEvent)
        }
    }

    private func handleServerEvent(_ event: ServerEvent) async {
        switch event {
        case .sessionId(let sid):
            serverInfo.sessionId = sid

        case .history(let payload):
            serverInfo.sessionId = payload.sessionId
            serverInfo.serverName = payload.serverName
            serverInfo.serverIcon = payload.serverIcon
            serverInfo.serverVersion = payload.serverVersion
            serverInfo.providerName = payload.providerName
            serverInfo.providerModel = payload.providerModel
            serverInfo.availableModels = payload.availableModels
            serverInfo.allSessions = payload.allSessions
            serverInfo.isCanary = payload.isCanary ?? false
            serverInfo.wasInterrupted = payload.wasInterrupted ?? false
            if let tokens = payload.totalTokens {
                serverInfo.totalInputTokens = tokens.0
                serverInfo.totalOutputTokens = tokens.1
            }
            let info = serverInfo
            let msgs = payload.messages
            await callDelegate { d in
                d.clientDidConnect(serverInfo: info)
                d.clientDidReceiveHistory(messages: msgs)
            }

        case .textDelta(let text):
            await callDelegate { $0.clientDidReceiveText(text) }

        case .textReplace(let text):
            await callDelegate { $0.clientDidReplaceText(text) }

        case .toolStart(let id, let name):
            let info = ToolCallInfo(id: id, name: name)
            await callDelegate { $0.clientDidStartTool(info) }

        case .toolInput(let delta):
            await callDelegate { $0.clientDidReceiveToolInput(delta) }

        case .toolExec(let id, let name):
            await callDelegate { $0.clientDidExecuteTool(id: id, name: name) }

        case .toolDone(let id, let name, let output, let error):
            await callDelegate { $0.clientDidFinishTool(id: id, name: name, output: output, error: error) }

        case .done(let id):
            await callDelegate { $0.clientDidFinishTurn(id: id) }

        case .error(let id, let message):
            await callDelegate { $0.clientDidReceiveError(id: id, message: message) }

        case .tokenUsage(let input, let output, let cacheRead, let cacheWrite):
            serverInfo.totalInputTokens += input
            serverInfo.totalOutputTokens += output
            let update = TokenUpdate(input: input, output: output, cacheRead: cacheRead, cacheWrite: cacheWrite)
            await callDelegate { $0.clientDidUpdateTokens(update) }

        case .modelChanged(_, let model, let provider, _):
            serverInfo.providerModel = model
            if let p = provider { serverInfo.providerName = p }
            await callDelegate { $0.clientDidChangeModel(model: model, provider: provider) }

        case .upstreamProvider:
            break

        case .interrupted:
            await callDelegate { $0.clientDidInterrupt(InterruptInfo()) }

        case .ack, .pong, .state, .reloading, .reloadProgress,
             .notification, .swarmStatus, .mcpStatus,
             .softInterruptInjected, .memoryInjected,
             .splitResponse, .compactResult, .stdinRequest, .unknown:
            break
        }
    }

    private nonisolated func callDelegate(_ block: @MainActor @Sendable (any JCodeClientDelegate) -> Void) async {
        await MainActor.run {
            guard let delegate = self._delegate else { return }
            block(delegate)
        }
    }
}
