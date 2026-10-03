import Foundation
import Testing

@testable import JCodeKit

/// Scriptable in-memory transport for Connection tests.
actor FakeTransport: WebSocketTransport {
    enum Behavior {
        case succeed
        case failConnect
        case unauthorized
        case unauthorizedOnSend
        case unreachableOnSend
        case silent
    }

    let behavior: Behavior
    let autoReply: String?
    let repliesToHistory: Bool
    private(set) var sentLines: [String] = []
    private var incoming: [String] = []
    private var waiters: [CheckedContinuation<String?, Never>] = []
    private var stalledSends: [CheckedContinuation<Void, Never>] = []
    private var closed = false

    init(behavior: Behavior = .succeed, autoReply: String? = nil, repliesToHistory: Bool = true) {
        self.behavior = behavior
        self.autoReply = autoReply
        self.repliesToHistory = repliesToHistory
    }

    func connect(url: URL, authToken: String) async throws {
        if behavior == .failConnect {
            throw TransportError.notConnected
        }
        if behavior == .unauthorized {
            throw TransportError.unauthorized
        }
    }

    func send(text: String) async throws {
        if closed { throw TransportError.notConnected }
        if behavior == .unauthorizedOnSend { throw TransportError.unauthorized }
        if behavior == .unreachableOnSend { throw URLError(.cannotConnectToHost) }
        if behavior == .silent {
            await withCheckedContinuation { stalledSends.append($0) }
            throw URLError(.cancelled)
        }
        sentLines.append(text)
        guard let object = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
            let id = object["id"] as? Int
        else { return }
        if let autoReply, sentLines.count == 1 {
            push(autoReply.replacingOccurrences(of: #""id":1"#, with: "\"id\":\(id)"))
        }
        if repliesToHistory, object["type"] as? String == "get_history" {
            push(#"{"type":"history","id":\#(id),"session_id":"sess_test","messages":[]}"#)
        }
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
        for send in stalledSends {
            send.resume()
        }
        stalledSends.removeAll()
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

private func makeConnection(
    transport: FakeTransport, maxReconnectAttempts: Int? = 1
) -> Connection {
    Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: maxReconnectAttempts,
            baseBackoffSeconds: 0.01
        ),
        makeTransport: { transport }
    )
}

private func waitForSentLines(_ transport: FakeTransport, count: Int) async throws -> [String] {
    var sent: [String] = []
    for _ in 0..<100 {
        sent = await transport.sentLines
        if sent.count >= count { break }
        try await Task.sleep(nanoseconds: 5_000_000)
    }
    return sent
}

private func expectLive(_ iterator: inout AsyncStream<ConnectionOutput>.Iterator) async {
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))
    guard case .event(.history)? = await iterator.next() else {
        Issue.record("expected the history reply after going live")
        return
    }
}

@Test func unauthorizedOnFirstSendAsksForRePair() async throws {
    let transport = FakeTransport(behavior: .unauthorizedOnSend)
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: nil,
            baseBackoffSeconds: 0.01
        ),
        makeTransport: { transport }
    )
    let stream = await connection.start(workingDirectory: "/repo")
    var phases: [ConnectionPhase] = []
    for await output in stream {
        if case let .phase(phase) = output {
            phases.append(phase)
            if case .failed = phase { break }
            if case .reconnecting = phase {
                Issue.record("must not reconnect after a 401")
                break
            }
        }
    }
    guard case .failed(let reason)? = phases.last else {
        Issue.record("expected failed phase, got \(phases)")
        return
    }
    #expect(reason.contains("Re-pair"))
    await connection.stop()
}

@Test(.timeLimit(.minutes(1))) func unreachableServerNeverReportsConnected() async throws {
    let transport = FakeTransport(behavior: .unreachableOnSend)
    let connection = makeConnection(transport: transport, maxReconnectAttempts: 2)
    let stream = await connection.start(workingDirectory: "/repo")
    var phases: [ConnectionPhase] = []
    for await output in stream {
        guard case .phase(let phase) = output else { continue }
        phases.append(phase)
        if case .failed = phase { break }
    }
    #expect(!phases.contains(.connected))
    #expect(phases.contains(.reconnecting(attempt: 1)))
    await connection.stop()
}

@Test func newSessionSubscribeSendsWorkingDirectory() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start(workingDirectory: "/Users/me/repo")
    var iterator = stream.makeAsyncIterator()
    _ = await iterator.next()
    _ = await iterator.next()

    let sent = try await waitForSentLines(transport, count: 1)
    #expect(sent.first?.contains("\"working_dir\":\"\\/Users\\/me\\/repo\"") == true)
    await connection.stop()
}

@Test func reattachSubscribeOmitsWorkingDirectory() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start(
        resumeSessionID: "sess_1", workingDirectory: "/Users/me/repo")
    var iterator = stream.makeAsyncIterator()
    _ = await iterator.next()
    _ = await iterator.next()

    let sent = try await waitForSentLines(transport, count: 1)
    #expect(sent.first?.contains("\"target_session_id\":\"sess_1\"") == true)
    #expect(sent.first?.contains("working_dir") == false)
    await connection.stop()
}

@Test func subscribeRejectionFailsWithoutReconnecting() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport, maxReconnectAttempts: nil)
    let stream = await connection.start(workingDirectory: "/missing")
    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))
    _ = try await waitForSentLines(transport, count: 1)

    let message = "Remote working directory must exist and be a directory on the server: /missing"
    await transport.push(#"{"type":"error","id":1,"message":"\#(message)"}"#)

    var sawFailed = false
    while let output = await iterator.next() {
        if case .phase(.reconnecting) = output {
            Issue.record("must not reconnect after a rejected subscribe")
            break
        }
        if case .phase(.failed(let reason)) = output {
            #expect(reason == message)
            sawFailed = true
            break
        }
    }
    #expect(sawFailed)
    await connection.stop()
}

@Test(.timeLimit(.minutes(1))) func rejectedReattachStartsNewSessionInWorkspace() async throws {
    let first = FakeTransport()
    let second = FakeTransport()
    let transports = TransportQueue([first, second])
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: nil,
            baseBackoffSeconds: 0.01
        ),
        makeTransport: { transports.next() }
    )
    let stream = await connection.start(resumeSessionID: "sess_gone", workingDirectory: "/repo")
    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    #expect(await iterator.next() == .phase(.connected))
    _ = try await waitForSentLines(first, count: 1)

    let message = "Unknown session 'sess_gone' or session has no working directory"
    await first.push(#"{"type":"error","id":1,"message":"\#(message)"}"#)

    let sent = try await waitForSentLines(second, count: 1)
    #expect(sent.first?.contains("target_session_id") == false)
    #expect(sent.first?.contains("\"working_dir\":\"\\/repo\"") == true)

    await second.push(#"{"type":"session","session_id":"sess_new"}"#)
    while let output = await iterator.next() {
        if case .phase(.failed) = output {
            Issue.record("a dropped session must not fail the connection")
            break
        }
        if case .event(.error) = output {
            Issue.record("the stale-session error must not reach the UI")
            break
        }
        if case .event(.sessionID(let id)) = output {
            #expect(id == "sess_new")
            break
        }
    }
    await connection.stop()
}

@Test(.timeLimit(.minutes(1))) func busyReattachRetriesSameSessionThenGivesUp() async throws {
    let message = "Session 'sess_live' is already live but could not be shared safely with this connection."
    let made = MadeTransports()
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: nil,
            baseBackoffSeconds: 0.01
        ),
        makeTransport: {
            let transport = FakeTransport(
                autoReply: #"{"type":"error","id":1,"message":"\#(message)","retry_after_secs":0}"#)
            made.append(transport)
            return transport
        }
    )
    let stream = await connection.start(resumeSessionID: "sess_live", workingDirectory: "/repo")
    var failure: String?
    for await output in stream {
        if case .phase(.failed(let reason)) = output {
            failure = reason
            break
        }
    }
    var subscribes: [String] = []
    for transport in made.all {
        subscribes += await transport.sentLines.filter { $0.contains("\"subscribe\"") }
    }
    #expect(failure == message)
    #expect(subscribes.count == Connection.maxBusyReattachAttempts + 1)
    #expect(subscribes.allSatisfy { $0.contains("\"target_session_id\":\"sess_live\"") })
    await connection.stop()
}

final class MadeTransports: @unchecked Sendable {
    private let lock = NSLock()
    private var transports: [FakeTransport] = []

    func append(_ transport: FakeTransport) {
        lock.lock()
        transports.append(transport)
        lock.unlock()
    }

    var all: [FakeTransport] {
        lock.lock()
        defer { lock.unlock() }
        return transports
    }
}

@Test(.timeLimit(.minutes(1))) func silentServerTimesOutAndRetries() async throws {
    let made = MadeTransports()
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: 2,
            baseBackoffSeconds: 0.01,
            firstReplyTimeoutSeconds: 0.2
        ),
        makeTransport: {
            let transport = FakeTransport(behavior: .silent)
            made.append(transport)
            return transport
        }
    )
    let stream = await connection.start(workingDirectory: "/repo")
    var phases: [ConnectionPhase] = []
    for await output in stream {
        guard case .phase(let phase) = output else { continue }
        phases.append(phase)
        if case .failed = phase { break }
    }
    #expect(phases.contains(.reconnecting(attempt: 1)))
    #expect(phases.contains(.reconnecting(attempt: 2)))
    #expect(!phases.contains(.connected))
    #expect(made.all.count == 3)
    await connection.stop()
}

@Test(.timeLimit(.minutes(1))) func otherReattachRejectionDoesNotStartNewSession() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport, maxReconnectAttempts: nil)
    let stream = await connection.start(resumeSessionID: "sess_x", workingDirectory: "/repo")
    var iterator = stream.makeAsyncIterator()
    _ = await iterator.next()
    _ = try await waitForSentLines(transport, count: 1)
    await transport.push(#"{"type":"error","id":1,"message":"Session is closed"}"#)
    var failure: String?
    while let output = await iterator.next() {
        if case .phase(.failed(let reason)) = output {
            failure = reason
            break
        }
    }
    #expect(failure == "Session is closed")
    let sent = await transport.sentLines.filter { $0.contains("\"subscribe\"") }
    #expect(sent.count == 1)
    await connection.stop()
}

@Test(.timeLimit(.minutes(1))) func rejectedReattachWithoutWorkspaceStillFails() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport, maxReconnectAttempts: nil)
    let stream = await connection.start(resumeSessionID: "sess_gone")
    var iterator = stream.makeAsyncIterator()
    _ = await iterator.next()
    _ = await iterator.next()
    _ = try await waitForSentLines(transport, count: 1)

    let message = "Unknown session 'sess_gone' or session has no working directory"
    await transport.push(#"{"type":"error","id":1,"message":"\#(message)"}"#)

    var sawFailed = false
    while let output = await iterator.next() {
        if case .phase(.failed(let reason)) = output {
            #expect(reason == message)
            sawFailed = true
            break
        }
    }
    #expect(sawFailed)
    await connection.stop()
}

final class TransportQueue: @unchecked Sendable {
    private let lock = NSLock()
    private var transports: [FakeTransport]

    init(_ transports: [FakeTransport]) {
        self.transports = transports
    }

    func next() -> FakeTransport {
        lock.lock()
        defer { lock.unlock() }
        return transports.count > 1 ? transports.removeFirst() : transports[0]
    }
}

@Test func turnErrorsDoNotStopTheConnection() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start(workingDirectory: "/repo")
    var iterator = stream.makeAsyncIterator()
    await expectLive(&iterator)
    _ = try await waitForSentLines(transport, count: 2)

    await transport.push(#"{"type":"error","id":7,"message":"rate limited"}"#)
    #expect(
        await iterator.next()
            == .event(.error(id: 7, message: "rate limited", retryAfterSecs: nil)))
    await transport.push(#"{"type":"text_delta","text":"still here"}"#)
    #expect(await iterator.next() == .event(.textDelta(text: "still here")))
    await connection.stop()
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
    #expect(sent[0].contains("\"continue_on_disconnect\":true"))
    #expect(sent[1].contains("\"type\":\"get_history\""))

    await connection.stop()
}

@Test func decodesPushedEvents() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    await expectLive(&iterator)

    await transport.push(#"{"type":"text_delta","text":"hi"}"#)
    #expect(await iterator.next() == .event(.textDelta(text: "hi")))

    // Multiple newline-delimited events in one frame.
    await transport.push("{\"type\":\"message_end\"}\n{\"type\":\"done\",\"id\":9}")
    #expect(await iterator.next() == .event(.messageEnd))
    #expect(await iterator.next() == .event(.done(id: 9)))

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
    await expectLive(&iterator)

    await transport.push(#"{"type":"session","session_id":"sess_42"}"#)
    #expect(await iterator.next() == .event(.sessionID(sessionID: "sess_42")))

    await connection.stop()
}

@Test func sessionCloseRequestStopsReconnecting() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    await expectLive(&iterator)

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

@Test(.timeLimit(.minutes(1))) func reloadingTriggersFastReconnect() async throws {
    let transport = FakeTransport()
    let transports = TransportQueue([transport, FakeTransport()])
    let connection = Connection(
        configuration: .init(
            gateway: Gateway(host: "test.local"),
            authToken: "tok",
            maxReconnectAttempts: 1,
            baseBackoffSeconds: 0.01
        ),
        makeTransport: { transports.next() }
    )
    let stream = await connection.start()

    var iterator = stream.makeAsyncIterator()
    await expectLive(&iterator)

    await transport.push(#"{"type":"reloading"}"#)
    #expect(await iterator.next() == .event(.reloading(newSocket: nil)))

    // Simulate the server dropping the socket for the restart.
    await transport.close()

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

@Test(.timeLimit(.minutes(1))) func staysConnectingUntilHistoryRequestIsAnswered() async throws {
    let transport = FakeTransport(repliesToHistory: false)
    let connection = makeConnection(transport: transport)
    let stream = await connection.start(resumeSessionID: "sess_1")
    var iterator = stream.makeAsyncIterator()
    #expect(await iterator.next() == .phase(.connecting))
    _ = try await waitForSentLines(transport, count: 2)

    await transport.push(#"{"type":"history","id":1,"session_id":"sess_1","messages":[]}"#)
    guard case .event(.history(let resumeReply))? = await iterator.next() else {
        Issue.record("expected the resume history to be forwarded")
        return
    }
    #expect(resumeReply.id == 1)

    await transport.push(#"{"type":"history","id":2,"session_id":"sess_1","messages":[]}"#)
    #expect(await iterator.next() == .phase(.connected))
    guard case .event(.history(let syncReply))? = await iterator.next() else {
        Issue.record("expected the synced history after going live")
        return
    }
    #expect(syncReply.id == 2)
    await connection.stop()
}

@Test func subscribeAcknowledgementIsNotForwarded() async throws {
    let transport = FakeTransport()
    let connection = makeConnection(transport: transport)
    let stream = await connection.start(resumeSessionID: "sess_1")
    var iterator = stream.makeAsyncIterator()
    await expectLive(&iterator)

    await transport.push(#"{"type":"done","id":1}"#)
    await transport.push(#"{"type":"done","id":7}"#)
    #expect(await iterator.next() == .event(.done(id: 7)))
    await connection.stop()
}
