import Foundation

/// Connection lifecycle reported to the UI.
public enum ConnectionPhase: Equatable, Sendable {
    case disconnected
    case connecting
    case connected
    /// Waiting before the next reconnect attempt.
    case reconnecting(attempt: Int)
    /// Gave up or was told to stop; `reason` is user-displayable.
    case failed(reason: String)
}

/// Everything the UI observes from a connection: lifecycle changes plus
/// decoded server events.
public enum ConnectionOutput: Equatable, Sendable {
    case phase(ConnectionPhase)
    case event(ServerEvent)
}

/// Actor owning one WebSocket connection to a jcode server.
///
/// Responsibilities:
/// - connect, authenticate, subscribe
/// - decode incoming NDJSON frames into `ServerEvent`s
/// - automatic reconnect with capped exponential backoff
/// - request-ID assignment
///
/// It deliberately knows nothing about app state; consumers feed the output
/// stream into `SessionReducer`.
public actor Connection {
    public struct Configuration: Sendable {
        public var gateway: Gateway
        public var authToken: String
        /// Maximum reconnect attempts before reporting `.failed`. Nil retries forever.
        public var maxReconnectAttempts: Int?
        /// Base backoff delay in seconds, doubled per attempt and capped at 30s.
        public var baseBackoffSeconds: Double
        /// Absolute working directory to subscribe against, as advertised by the
        /// server in its `/pair` or `/health` response. The server rejects a
        /// subscribe without one, and a phone cannot invent a host path.
        public var workingDir: String?

        public init(
            gateway: Gateway,
            authToken: String,
            maxReconnectAttempts: Int? = nil,
            baseBackoffSeconds: Double = 1.0,
            workingDir: String? = nil
        ) {
            self.gateway = gateway
            self.authToken = authToken
            self.maxReconnectAttempts = maxReconnectAttempts
            self.baseBackoffSeconds = baseBackoffSeconds
            self.workingDir = workingDir
        }
    }

    private let configuration: Configuration
    private let makeTransport: @Sendable () -> any WebSocketTransport
    private var transport: (any WebSocketTransport)?
    private var nextRequestID: UInt64 = 1
    private var runTask: Task<Void, Never>?
    private var continuation: AsyncStream<ConnectionOutput>.Continuation?
    private var targetSessionID: String?
    private var stopped = false
    /// Set when the server announced a reload; the next reconnect attempt
    /// skips backoff because the drop is expected and the server returns fast.
    private var expectServerReload = false
    /// Set when the server asked this client to close; reconnecting would
    /// fight the server, so the loop ends with a failed phase instead.
    private var closeRequestedReason: String?

    public init(
        configuration: Configuration,
        makeTransport: @escaping @Sendable () -> any WebSocketTransport = {
            URLSessionWebSocketTransport()
        }
    ) {
        self.configuration = configuration
        self.makeTransport = makeTransport
    }

    /// Starts the connection loop. The returned stream yields phase changes
    /// and decoded events until `stop()` is called or the stream is cancelled.
    public func start(resumeSessionID: String? = nil) -> AsyncStream<ConnectionOutput> {
        targetSessionID = resumeSessionID
        stopped = false
        expectServerReload = false
        closeRequestedReason = nil
        let (stream, continuation) = AsyncStream.makeStream(of: ConnectionOutput.self)
        self.continuation = continuation
        runTask = Task { await runLoop() }
        continuation.onTermination = { _ in
            Task { [weak self] in await self?.stop() }
        }
        return stream
    }

    public func stop() async {
        guard !stopped else { return }
        stopped = true
        runTask?.cancel()
        runTask = nil
        if let transport {
            await transport.close()
        }
        transport = nil
        continuation?.yield(.phase(.disconnected))
        continuation?.finish()
        continuation = nil
    }

    /// Sends a request, assigning it a fresh ID. Returns the assigned ID.
    @discardableResult
    public func send(_ build: @Sendable (UInt64) -> Request) async throws -> UInt64 {
        guard let transport else { throw TransportError.notConnected }
        let id = nextRequestID
        nextRequestID += 1
        let request = build(id)
        try await transport.send(text: request.encodedLine())
        return id
    }

    // MARK: - Internals

    private func runLoop() async {
        var attempt = 0
        while !Task.isCancelled && !stopped {
            yield(.phase(attempt == 0 ? .connecting : .reconnecting(attempt: attempt)))
            let transport = makeTransport()
            do {
                try await transport.connect(
                    url: configuration.gateway.webSocketURL,
                    authToken: configuration.authToken
                )
                self.transport = transport
                yield(.phase(.connected))
                attempt = 0
                try await subscribeAndSync()
                try await receiveLoop(transport: transport)
                // Clean close: fall through to reconnect.
            } catch {
                if Task.isCancelled || stopped { break }
                if case TransportError.unauthorized = error {
                    // Retrying with the same token can never succeed.
                    self.transport = nil
                    yield(
                        .phase(
                            .failed(
                                reason:
                                    "This device is no longer paired with the server. Re-pair to reconnect."
                            )))
                    return
                }
            }
            self.transport = nil
            if Task.isCancelled || stopped { break }
            if let reason = closeRequestedReason {
                await transport.close()
                yield(.phase(.failed(reason: reason)))
                return
            }
            attempt += 1
            if let max = configuration.maxReconnectAttempts, attempt > max {
                yield(.phase(.failed(reason: "Could not reach server after \(max) attempts")))
                return
            }
            if expectServerReload {
                // The server told us it is restarting: reconnect eagerly with a
                // short fixed delay instead of exponential backoff.
                expectServerReload = false
                try? await Task.sleep(nanoseconds: UInt64(configuration.baseBackoffSeconds * 500_000_000))
                continue
            }
            let delay = min(
                configuration.baseBackoffSeconds * pow(2.0, Double(attempt - 1)), 30.0)
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
        }
    }

    private func subscribeAndSync() async throws {
        let sessionID = targetSessionID
        try await send {
            .subscribe(
                id: $0, targetSessionID: sessionID,
                workingDir: configuration.workingDir)
        }
        try await send { .getHistory(id: $0) }
    }

    private func receiveLoop(transport: any WebSocketTransport) async throws {
        while !Task.isCancelled && !stopped {
            guard let text = try await transport.receiveText() else { return }
            // A frame may contain multiple newline-delimited events.
            for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
                if let event = try? ServerEvent.decode(line: String(line)) {
                    switch event {
                    case .sessionID(let sessionID):
                        targetSessionID = sessionID
                    case .reloading:
                        expectServerReload = true
                    case .sessionCloseRequested(let reason):
                        closeRequestedReason =
                            reason.isEmpty ? "Server closed this session" : reason
                    default:
                        break
                    }
                    yield(.event(event))
                    if closeRequestedReason != nil { return }
                }
            }
        }
    }

    private func yield(_ output: ConnectionOutput) {
        continuation?.yield(output)
    }
}
