import Foundation

public actor JCodeConnection {
    public enum State: Sendable {
        case disconnected
        case connecting
        case connected(sessionId: String)
        case error(String)
    }

    public enum Event: Sendable {
        case stateChanged(State)
        case serverEvent(ServerEvent)
    }

    private var webSocket: URLSessionWebSocketTask?
    private var urlSession: URLSession?
    private var state: State = .disconnected
    private var nextId: UInt64 = 1
    private var eventContinuation: AsyncStream<Event>.Continuation?
    private let authToken: String
    private let serverURL: URL

    public init(host: String, port: UInt16 = 7643, authToken: String) {
        var components = URLComponents()
        components.scheme = "ws"
        components.host = host
        components.port = Int(port)
        components.path = "/ws"
        components.queryItems = [URLQueryItem(name: "token", value: authToken)]
        self.serverURL = components.url!
        self.authToken = authToken
    }

    public func events() -> AsyncStream<Event> {
        AsyncStream { continuation in
            self.eventContinuation = continuation
        }
    }

    public func connect(workingDir: String? = nil) async throws {
        setState(.connecting)

        let session = URLSession(configuration: .default)
        self.urlSession = session

        let task = session.webSocketTask(with: serverURL)
        self.webSocket = task
        task.resume()

        startReceiving()

        let id = nextId
        nextId += 1
        try await send(.subscribe(id: id, workingDir: workingDir))

        setState(.connected(sessionId: ""))
    }

    public func disconnect() {
        webSocket?.cancel(with: .normalClosure, reason: nil)
        webSocket = nil
        urlSession = nil
        setState(.disconnected)
        eventContinuation?.finish()
        eventContinuation = nil
    }

    public func sendMessage(_ content: String, images: [(String, String)] = []) async throws -> UInt64 {
        let id = nextId
        nextId += 1
        try await send(.message(id: id, content: content, images: images))
        return id
    }

    public func cancelGeneration() async throws {
        let id = nextId
        nextId += 1
        try await send(.cancel(id: id))
    }

    public func requestHistory() async throws -> UInt64 {
        let id = nextId
        nextId += 1
        try await send(.getHistory(id: id))
        return id
    }

    public func ping() async throws {
        let id = nextId
        nextId += 1
        try await send(.ping(id: id))
    }

    public func resumeSession(_ sessionId: String) async throws {
        let id = nextId
        nextId += 1
        try await send(.resumeSession(id: id, sessionId: sessionId))
    }

    public func setModel(_ model: String) async throws {
        let id = nextId
        nextId += 1
        try await send(.setModel(id: id, model: model))
    }

    public func interrupt(_ content: String, urgent: Bool = false) async throws {
        let id = nextId
        nextId += 1
        try await send(.softInterrupt(id: id, content: content, urgent: urgent))
    }

    // MARK: - Private

    private func send(_ request: Request) async throws {
        guard let webSocket else {
            throw ConnectionError.notConnected
        }
        let data = try JSONEncoder().encode(request)
        guard let text = String(data: data, encoding: .utf8) else {
            throw ConnectionError.encodingFailed
        }
        try await webSocket.send(.string(text))
    }

    private func startReceiving() {
        webSocket?.receive { [weak self] result in
            Task { [weak self] in
                guard let self else { return }
                await self.handleReceive(result)
            }
        }
    }

    private func handleReceive(_ result: Result<URLSessionWebSocketTask.Message, Error>) {
        switch result {
        case .success(let message):
            switch message {
            case .string(let text):
                if let data = text.data(using: .utf8),
                   let event = try? JSONDecoder().decode(ServerEvent.self, from: data) {
                    if case .sessionId(let sid) = event {
                        setState(.connected(sessionId: sid))
                    }
                    eventContinuation?.yield(.serverEvent(event))
                }
            case .data:
                break
            @unknown default:
                break
            }
            startReceiving()

        case .failure(let error):
            setState(.error(error.localizedDescription))
            eventContinuation?.yield(.stateChanged(.error(error.localizedDescription)))
        }
    }

    private func setState(_ newState: State) {
        state = newState
        eventContinuation?.yield(.stateChanged(newState))
    }
}

public enum ConnectionError: Error, Sendable {
    case encodingFailed
    case notConnected
    case invalidResponse
}
