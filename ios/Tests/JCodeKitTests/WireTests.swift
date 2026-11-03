import Foundation
import Testing

@testable import JCodeKit

// MARK: - Gateway / PairURI

@Test func gatewayBuildsEndpoints() {
    let gateway = Gateway(host: "devbox.tailnet.ts.net")
    #expect(gateway.healthURL.absoluteString == "http://devbox.tailnet.ts.net:7643/health")
    #expect(gateway.pairURL.absoluteString == "http://devbox.tailnet.ts.net:7643/pair")
    #expect(gateway.webSocketURL.absoluteString == "ws://devbox.tailnet.ts.net:7643/ws")
}

@Test func pairURIParsesQRPayload() {
    let payload = PairURI.parse("jcode://pair?host=mybox.ts.net&port=7643&code=123456")
    #expect(payload?.gateway.host == "mybox.ts.net")
    #expect(payload?.gateway.port == 7643)
    #expect(payload?.code == "123456")
}

@Test func pairURIDefaultsPort() {
    let payload = PairURI.parse("jcode://pair?host=mybox&code=987654")
    #expect(payload?.gateway.port == Gateway.defaultPort)
}

@Test func pairURIRejectsGarbage() {
    #expect(PairURI.parse("https://example.com/pair?host=x&code=1") == nil)
    #expect(PairURI.parse("jcode://pair?host=&code=1") == nil)
    #expect(PairURI.parse("jcode://pair?host=x") == nil)
    #expect(PairURI.parse("not a uri") == nil)
}

// MARK: - Request encoding (must match crates/jcode-protocol/src/wire.rs)

private func encodedObject(_ request: Request) throws -> [String: Any] {
    let line = try request.encodedLine()
    let data = line.data(using: .utf8)!
    return try JSONSerialization.jsonObject(with: data) as! [String: Any]
}

@Test func encodesMessageRequest() throws {
    let object = try encodedObject(.message(id: 7, content: "hello"))
    #expect(object["type"] as? String == "message")
    #expect(object["id"] as? UInt64 == 7)
    #expect(object["content"] as? String == "hello")
}

@Test func encodesSubscribeWithTargetSession() throws {
    let object = try encodedObject(
        .subscribe(id: 1, targetSessionID: "sess_abc", workingDir: "/Users/dev/proj"))
    #expect(object["type"] as? String == "subscribe")
    #expect(object["target_session_id"] as? String == "sess_abc")
    #expect(object["working_dir"] as? String == "/Users/dev/proj")

    let bare = try encodedObject(.subscribe(id: 2, targetSessionID: nil, workingDir: nil))
    #expect(bare["target_session_id"] == nil)
    #expect(bare["working_dir"] == nil)
}

/// The real server rejects a subscribe whose working_dir is absent or empty
/// ("Subscribe requires the client's working directory"), so an empty string
/// must be omitted rather than sent through as "".
@Test func omitsEmptyWorkingDirOnSubscribe() throws {
    let object = try encodedObject(.subscribe(id: 3, targetSessionID: nil, workingDir: ""))
    #expect(object["working_dir"] == nil)
}

@Test func encodesControlRequests() throws {
    #expect(try encodedObject(.cancel(id: 3))["type"] as? String == "cancel")
    #expect(try encodedObject(.ping(id: 4))["type"] as? String == "ping")
    #expect(try encodedObject(.getHistory(id: 5))["type"] as? String == "get_history")
    #expect(try encodedObject(.clear(id: 6))["type"] as? String == "clear")
    #expect(
        try encodedObject(.cancelSoftInterrupts(id: 8))["type"] as? String
            == "cancel_soft_interrupts")

    let soft = try encodedObject(.softInterrupt(id: 9, content: "also do x", urgent: true))
    #expect(soft["type"] as? String == "soft_interrupt")
    #expect(soft["content"] as? String == "also do x")
    #expect(soft["urgent"] as? Bool == true)

    let resume = try encodedObject(.resumeSession(id: 10, sessionID: "sess_x"))
    #expect(resume["type"] as? String == "resume_session")
    #expect(resume["session_id"] as? String == "sess_x")

    let model = try encodedObject(.setModel(id: 11, model: "claude-sonnet-4"))
    #expect(model["type"] as? String == "set_model")
    #expect(model["model"] as? String == "claude-sonnet-4")

    let rename = try encodedObject(.renameSession(id: 12, title: "My session"))
    #expect(rename["type"] as? String == "rename_session")
    #expect(rename["title"] as? String == "My session")
}

@Test func encodesReasoningEffortAndCompact() throws {
    let effort = try encodedObject(.setReasoningEffort(id: 13, effort: "high"))
    #expect(effort["type"] as? String == "set_reasoning_effort")
    #expect(effort["id"] as? UInt64 == 13)
    #expect(effort["effort"] as? String == "high")

    let compact = try encodedObject(.compact(id: 14))
    #expect(compact["type"] as? String == "compact")
    #expect(compact["id"] as? UInt64 == 14)
}

// MARK: - ServerEvent decoding (fixtures mirror real server output)

@Test func decodesStreamingEvents() throws {
    #expect(
        try ServerEvent.decode(line: #"{"type":"text_delta","text":"Hel"}"#)
            == .textDelta(text: "Hel"))
    #expect(
        try ServerEvent.decode(line: #"{"type":"reasoning_delta","text":"hmm"}"#)
            == .reasoningDelta(text: "hmm"))
    #expect(
        try ServerEvent.decode(line: #"{"type":"reasoning_done","duration_secs":1.5}"#)
            == .reasoningDone(durationSecs: 1.5))
    #expect(
        try ServerEvent.decode(line: #"{"type":"text_replace","text":"clean"}"#)
            == .textReplace(text: "clean"))
    #expect(try ServerEvent.decode(line: #"{"type":"message_end"}"#) == .messageEnd)
    #expect(try ServerEvent.decode(line: #"{"type":"done","id":3}"#) == .done(id: 3))
    #expect(try ServerEvent.decode(line: #"{"type":"interrupted"}"#) == .interrupted)
}

@Test func decodesToolLifecycle() throws {
    #expect(
        try ServerEvent.decode(line: #"{"type":"tool_start","id":"t1","name":"bash"}"#)
            == .toolStart(id: "t1", name: "bash"))
    #expect(
        try ServerEvent.decode(line: #"{"type":"tool_input","delta":"{\"cmd\""}"#)
            == .toolInput(delta: "{\"cmd\""))
    #expect(
        try ServerEvent.decode(line: #"{"type":"tool_exec","id":"t1","name":"bash"}"#)
            == .toolExec(id: "t1", name: "bash"))
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"tool_done","id":"t1","name":"bash","output":"ok"}"#)
            == .toolDone(id: "t1", name: "bash", output: "ok", error: nil))
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"tool_done","id":"t2","name":"bash","output":"","error":"boom"}"#)
            == .toolDone(id: "t2", name: "bash", output: "", error: "boom"))
}

@Test func decodesErrorAndStatus() throws {
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"error","id":1,"message":"rate limited","retry_after_secs":30}"#)
            == .error(id: 1, message: "rate limited", retryAfterSecs: 30))
    #expect(
        try ServerEvent.decode(line: #"{"type":"tokens","input":1200,"output":340}"#)
            == .tokenUsage(input: 1200, output: 340))
    #expect(
        try ServerEvent.decode(line: #"{"type":"status_detail","detail":"thinking"}"#)
            == .statusDetail(detail: "thinking"))
    #expect(
        try ServerEvent.decode(
            line:
                #"{"type":"state","id":2,"session_id":"s1","message_count":4,"is_processing":true}"#
        ) == .state(id: 2, sessionID: "s1", messageCount: 4, isProcessing: true))
}

@Test func decodesSessionEvents() throws {
    #expect(
        try ServerEvent.decode(line: #"{"type":"session","session_id":"sess_1"}"#)
            == .sessionID(sessionID: "sess_1"))
    #expect(
        try ServerEvent.decode(
            line:
                #"{"type":"session_renamed","session_id":"sess_1","display_title":"Fix bug"}"#)
            == .sessionRenamed(sessionID: "sess_1", displayTitle: "Fix bug"))
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"model_changed","id":5,"model":"gpt-5","provider_name":"openai"}"#)
            == .modelChanged(id: 5, model: "gpt-5", error: nil))
}

@Test func decodesHistoryPayload() throws {
    let line = """
        {"type":"history","id":2,"session_id":"sess_9","messages":[\
        {"role":"user","content":"hi"},\
        {"role":"assistant","content":"hello!","tool_calls":["bash"]},\
        {"role":"assistant","content":"","tool_data":{"id":"t1","name":"read","input":"{}","output":"data"}}\
        ],"provider_name":"anthropic","provider_model":"claude-sonnet-4",\
        "available_models":["claude-sonnet-4","claude-opus-4"],\
        "total_tokens":[1500,800],"all_sessions":["sess_9","sess_8"],\
        "server_version":"v0.26.11","display_title":"My chat"}
        """
    guard case let .history(payload) = try ServerEvent.decode(line: line) else {
        Issue.record("expected history event")
        return
    }
    #expect(payload.sessionID == "sess_9")
    #expect(payload.messages.count == 3)
    #expect(payload.messages[0].role == "user")
    #expect(payload.messages[1].toolCalls == ["bash"])
    #expect(payload.messages[2].toolData?.name == "read")
    #expect(payload.providerModel == "claude-sonnet-4")
    #expect(payload.availableModels.count == 2)
    #expect(payload.totalTokens == .init(input: 1500, output: 800))
    #expect(payload.allSessions == ["sess_9", "sess_8"])
    #expect(payload.serverVersion == "v0.26.11")
    #expect(payload.displayTitle == "My chat")
}

@Test func unknownEventTypesAreTolerated() throws {
    let event = try ServerEvent.decode(
        line: #"{"type":"some_future_event","payload":{"x":1}}"#)
    #expect(event == .unknown(type: "some_future_event"))
}

@Test func decodesTurnLifecycleSignals() throws {
    #expect(
        try ServerEvent.decode(line: #"{"type":"connection_phase","phase":"authenticating"}"#)
            == .connectionPhase(phase: "authenticating"))
    #expect(
        try ServerEvent.decode(line: #"{"type":"retry_rollback","attempt":2,"max":5}"#)
            == .retryRollback(attempt: 2, max: 5))
    #expect(
        try ServerEvent.decode(
            line:
                #"{"type":"soft_interrupt_injected","content":"also fix y","point":"C","tools_skipped":1}"#
        )
            == .softInterruptInjected(
                content: "also fix y", displayRole: nil, point: "C", toolsSkipped: 1))
    #expect(
        try ServerEvent.decode(
            line:
                #"{"type":"soft_interrupt_injected","content":"note","display_role":"system","point":"A"}"#
        )
            == .softInterruptInjected(
                content: "note", displayRole: "system", point: "A", toolsSkipped: nil))
}

@Test func decodesEffortAndCompactResponses() throws {
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"reasoning_effort_changed","id":3,"effort":"high"}"#)
            == .reasoningEffortChanged(id: 3, effort: "high", error: nil))
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"reasoning_effort_changed","id":4,"error":"unsupported"}"#)
            == .reasoningEffortChanged(id: 4, effort: nil, error: "unsupported"))
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"compact_result","id":5,"message":"Compaction started","success":true}"#
        )
            == .compactResult(id: 5, message: "Compaction started", success: true))
}

@Test func decodesServerLifecycleEvents() throws {
    #expect(
        try ServerEvent.decode(line: #"{"type":"reloading"}"#)
            == .reloading(newSocket: nil))
    #expect(
        try ServerEvent.decode(line: #"{"type":"reloading","new_socket":"/tmp/jcode.sock"}"#)
            == .reloading(newSocket: "/tmp/jcode.sock"))
    #expect(
        try ServerEvent.decode(
            line: #"{"type":"session_close_requested","reason":"taken over"}"#)
            == .sessionCloseRequested(reason: "taken over"))
}

@Test func historyCarriesReasoningEffort() throws {
    let line =
        #"{"type":"history","id":1,"session_id":"s","messages":[],"reasoning_effort":"medium"}"#
    guard case let .history(payload) = try ServerEvent.decode(line: line) else {
        Issue.record("expected history event")
        return
    }
    #expect(payload.reasoningEffort == "medium")
}

@Test func malformedLinesThrow() {
    #expect(throws: WireError.self) {
        try ServerEvent.decode(line: "not json")
    }
    #expect(throws: WireError.self) {
        try ServerEvent.decode(line: #"{"no_type":true}"#)
    }
}
