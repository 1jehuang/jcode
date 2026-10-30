import Foundation
import Testing

@testable import JCodeKit

/// Scriptable in-memory transport for Connection tests.
actor FakeTransport: WebSocketTransport {
    enum Behavior {
        case succeed
        case failConnect
        case unauthorized
    }

    let behavior: Behavior
    private(set) var sentLines: [String] = []
    private(set) var connectedURL: URL?
    private var incoming: [String] = []
    private var waiters: [CheckedContinuation<String?, Never>] = []
    private var closed = false

    init(behavior: Behavior = .succeed) {
        self.behavior = behavior
    }

    func connect(url: URL, authToken: String) async throws {
        connectedURL = url
        if behavior == .failConnect {
            throw TransportError.notConnected
        }
        if behavior == .unauthorized {
            throw TransportError.unauthorized
        }
    }

    func send(text: String) async throws {
        if closed { throw TransportError.notConnected }
        sentLines.append(text)
    }

    func receiveText() async throws -> String? {
        if closed { return nil }
        if !incoming.isEmpty {
            return incoming.removeFirst()
        }
        return await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    func close() async {
        closed = true
        for waiter in waiters {
            waiter.resume(returning: nil)
        }
        waiters.removeAll()
    }

    /// Test helper: push a server frame to the client.
    func push(_ line: String) {
        if let waiter = waiters.first {
            waiters.removeFirst()
            waiter.resume(returning: line)
        } else {
            incoming.append(line)
        }
    }
}

private func makeConnection(transport: FakeTransport) -> Connection {
    Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: 1,
            baseBackoffSeconds: 0.01
        ),
        makeTransport: { transport }
    )
}

@Test func connectSubscribesAndSyncsHistory() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    // connecting -> connected
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))

    // Wait for the subscribe + get_history requests to land.
    var sent: [String] = []
    for _ in 0..<50 {
        sent = await transport.sentLines
        if sent.count >= 2 { break }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    #expect(sent.count == 2)
    #expect(sent[0].contains("\"type\":\"subscribe\""))
    #expect(sent[1].contains("\"type\":\"get_history\""))

    await connection.stop()
}

@Test func secureGatewayConnectsUsingWSS() async throws {
    let transport = FakeTransport()
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local", secure: true),
            authToken: "tok",
            maxReconnectAttempts: 1
        ),
        makeTransport: { transport }
    )
    let stream = await connection.start()
    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))
    #expect(await transport.connectedURL?.absoluteString == "wss://test.local:7643/ws")
    await connection.stop()
}

@Test func decodesPushedEvents() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))

    await transport.push(#"{"type":"text_delta","text":"hi"}"#)
    #expect(await iterator.next() == .event(.textDelta(text: "hi")))

    // Multiple newline-delimited events in one frame.
    await transport.push("{\"type\":\"message_end\"}\n{\"type\":\"done\",\"id\":1}")
    #expect(await iterator.next() == .event(.messageEnd))
    #expect(await iterator.next() == .event(.done(id: 1)))

    await connection.stop()
}

@Test func sendAssignsMonotonicRequestIDs() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    _ = await iterator.next()
    _ = await iterator.next()

    // Wait until the automatic subscribe/get_history sends (IDs 1-2) finish
    // so they cannot interleave with our test sends.
    for _ in 0..<100 {
        if await transport.sentLines.count >= 2 { break }
        try await Task.sleep(nanoseconds: 5_000_000)
    }

    let first = try await connection.send { .message(id: $0, content: "a") }
    let second = try await connection.send { .ping(id: $0) }
    #expect(second == first + 1)

    await connection.stop()
}

@Test func failedConnectReportsFailureAfterRetries() async throws {
    let transport = FakeTransport(behavior: .failConnect)
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var phases: [ConnectionPhase] = []
    for await output in stream {
        if case let .phase(phase) = output {
            phases.append(phase)
            if case .failed = phase { break }
        }
    }
    #expect(phases.first == .connecting)
    #expect(phases.contains(.reconnecting(attempt: 1)))
    if case .failed = phases.last {
    } else {
        Issue.record("expected failed phase, got \(phases)")
    }
    await connection.stop()
}

@Test func trackSessionIDForResubscribe() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    _ = await iterator.next()  // connecting
    _ = await iterator.next()  // connected

    await transport.push(#"{"type":"session","session_id":"sess_42"}"#)
    #expect(await iterator.next() == .event(.sessionID(sessionID: "sess_42")))

    await connection.stop()
}

@Test func sessionCloseRequestStopsReconnecting() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))

    await transport.push(#"{"type":"session_close_requested","reason":"taken over"}"#)
    #expect(
        await iterator.next()
            == .event(.sessionCloseRequested(reason: "taken over")))

    // The loop must terminate with a failed phase instead of reconnecting.
    var sawFailed = false
    while let output = await iterator.next() {
        if case .phase(.reconnecting) = output {
            Issue.record("must not reconnect after session_close_requested")
            break
        }
        if case .phase(.failed(let reason)) = output {
            #expect(reason == "taken over")
            sawFailed = true
            break
        }
    }
    #expect(sawFailed)
    await connection.stop()
}

@Test func reloadingTriggersFastReconnect() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))

    await transport.push(#"{"type":"reloading"}"#)
    #expect(await iterator.next() == .event(.reloading(newSocket: nil)))

    // Simulate the server dropping the socket for the restart.
    await transport.close()

    // The connection reconnects (the shared FakeTransport accepts again).
    var phases: [ConnectionPhase] = []
    while let output = await iterator.next() {
        if case let .phase(phase) = output {
            phases.append(phase)
            if phase == .connected { break }
        }
    }
    #expect(phases.contains(.reconnecting(attempt: 1)))
    #expect(phases.last == .connected)

    await connection.stop()
}

@Test func unauthorizedStopsReconnectingAndAsksForRePair() async throws {
    let transport = FakeTransport(behavior: .unauthorized)
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: nil,  // would retry forever without the 401 short-circuit
            baseBackoffSeconds: 0.01
        ),
        makeTransport: { transport }
    )
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    let final = await iterator.next()
    guard case .phase(.failed(let reason))? = final else {
        Issue.record("expected .failed phase, got \(String(describing: final))")
        return
    }
    #expect(reason.contains("Re-pair"))

    await connection.stop()
}
