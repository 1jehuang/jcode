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
        public var firstReplyTimeoutSeconds: Double

        public init(
            gateway: Gateway,
            authToken: String,
            maxReconnectAttempts: Int? = nil,
            baseBackoffSeconds: Double = 1.0,
            firstReplyTimeoutSeconds: Double = 15
        ) {
            self.gateway = gateway
            self.authToken = authToken
            self.maxReconnectAttempts = maxReconnectAttempts
            self.baseBackoffSeconds = baseBackoffSeconds
            self.firstReplyTimeoutSeconds = firstReplyTimeoutSeconds
        }
    }

    private let configuration: Configuration
    private let makeTransport: @Sendable () -> any WebSocketTransport
    private var transport: (any WebSocketTransport)?
    private var nextRequestID: UInt64 = 1
    private var runTask: Task<Void, Never>?
    private var continuation: AsyncStream<ConnectionOutput>.Continuation?
    private var targetSessionID: String?
    private var workingDirectory: String?
    private var subscribeRequestID: UInt64?
    private var historyRequestID: UInt64?
    private var subscribeWasReattach = false
    private var restartAsNewSession = false
    private var retryReattachAfter: UInt64?
    private var busyReattachAttempts = 0
    private var busyReattachMessage: String?
    static let maxBusyReattachAttempts = 5
    private var stopped = false
    /// Set when the server announced a reload; the next reconnect attempt
    /// skips backoff because the drop is expected and the server returns fast.
    private var expectServerReload = false
    private var receivedSinceConnect = false
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
    public func start(
        resumeSessionID: String? = nil,
        workingDirectory: String? = nil
    ) -> AsyncStream<ConnectionOutput> {
        targetSessionID = resumeSessionID
        self.workingDirectory = workingDirectory
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
                receivedSinceConnect = false
                let watchdog = startFirstReplyWatchdog(transport: transport)
                defer { watchdog.cancel() }
                try await subscribeAndSync()
                attempt = 0
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
            if restartAsNewSession {
                restartAsNewSession = false
                await transport.close()
                continue
            }
            if let seconds = retryReattachAfter {
                retryReattachAfter = nil
                await transport.close()
                busyReattachAttempts += 1
                if busyReattachAttempts > Self.maxBusyReattachAttempts {
                    yield(.phase(.failed(reason: busyReattachMessage ?? "Session is busy on another connection")))
                    return
                }
                yield(.phase(.reconnecting(attempt: busyReattachAttempts)))
                try? await Task.sleep(nanoseconds: max(seconds, 1) * 1_000_000_000)
                continue
            }
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
        let directory = sessionID == nil ? workingDirectory : nil
        subscribeWasReattach = sessionID != nil
        subscribeRequestID = try await send {
            .subscribe(
                id: $0,
                targetSessionID: sessionID,
                workingDirectory: directory,
                continueOnDisconnect: true
            )
        }
        historyRequestID = try await send { .getHistory(id: $0) }
    }

    private func receiveLoop(transport: any WebSocketTransport) async throws {
        while !Task.isCancelled && !stopped {
            guard let text = try await transport.receiveText() else { return }
            receivedSinceConnect = true
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
                    case .error(let id, let message, _)
                    where id == subscribeRequestID && subscribeWasReattach
                        && workingDirectory != nil && message.hasPrefix("Unknown session "):
                        targetSessionID = nil
                        restartAsNewSession = true
                        return
                    case .error(let id, let message, let retryAfter?)
                    where id == subscribeRequestID && subscribeWasReattach:
                        retryReattachAfter = retryAfter
                        busyReattachMessage = message
                        return
                    case .done(let id) where id == subscribeRequestID:
                        continue
                    case .history(let payload):
                        busyReattachAttempts = 0
                        busyReattachMessage = nil
                        if payload.id == historyRequestID {
                            historyRequestID = nil
                            yield(.phase(.connected))
                        }
                    case .error(let id, let message, _) where id == subscribeRequestID:
                        closeRequestedReason = message
                    default:
                        break
                    }
                    yield(.event(event))
                    if closeRequestedReason != nil { return }
                }
            }
        }
    }

    private func startFirstReplyWatchdog(transport: any WebSocketTransport) -> Task<Void, Never> {
        let seconds = configuration.firstReplyTimeoutSeconds
        return Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            guard !Task.isCancelled, let self else { return }
            if await !self.receivedSinceConnect {
                await transport.close()
            }
        }
    }

    private func yield(_ output: ConnectionOutput) {
        continuation?.yield(output)
    }
}
