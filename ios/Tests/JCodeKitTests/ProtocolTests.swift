import Foundation
@testable import JCodeKit

nonisolated(unsafe) var passed = 0
nonisolated(unsafe) var failed = 0

func check(_ condition: Bool, _ msg: String = "", file: String = #file, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL [\(file.split(separator: "/").last ?? ""):\(line)] \(msg)")
    }
}

func assertEqual<T: Equatable>(_ a: T, _ b: T, _ msg: String = "", file: String = #file, line: Int = #line) {
    if a == b {
        passed += 1
    } else {
        failed += 1
        print("  FAIL [\(file.split(separator: "/").last ?? ""):\(line)] \(msg.isEmpty ? "\(a) != \(b)" : msg)")
    }
}

func assertNil<T>(_ value: T?, _ msg: String = "", file: String = #file, line: Int = #line) {
    if value == nil {
        passed += 1
    } else {
        failed += 1
        print("  FAIL [\(file.split(separator: "/").last ?? ""):\(line)] expected nil, got \(value!) \(msg)")
    }
}

func decodeEvent(_ json: String) throws -> ServerEvent {
    try JSONDecoder().decode(ServerEvent.self, from: json.data(using: .utf8)!)
}

func encodeRequest(_ req: Request) throws -> [String: Any] {
    let data = try JSONEncoder().encode(req)
    return try JSONSerialization.jsonObject(with: data) as! [String: Any]
}

// MARK: - Request Encoding

print("Protocol Tests")
print("==============")

do {
    print("  Request encoding...")

    let json = try encodeRequest(.subscribe(id: 1, workingDir: "/tmp/test"))
    assertEqual(json["type"] as? String, "subscribe")
    assertEqual(json["id"] as? UInt64, 1)
    assertEqual(json["working_dir"] as? String, "/tmp/test")

    let json2 = try encodeRequest(.message(id: 42, content: "hello world"))
    assertEqual(json2["type"] as? String, "message")
    assertEqual(json2["id"] as? UInt64, 42)
    assertEqual(json2["content"] as? String, "hello world")

    let json3 = try encodeRequest(.cancel(id: 5))
    assertEqual(json3["type"] as? String, "cancel")
    assertEqual(json3["id"] as? UInt64, 5)

    let json4 = try encodeRequest(.softInterrupt(id: 9, content: "stop", urgent: true))
    assertEqual(json4["type"] as? String, "soft_interrupt")
    assertEqual(json4["content"] as? String, "stop")
    assertEqual(json4["urgent"] as? Bool, true)
}

// MARK: - ServerEvent Decoding

do {
    print("  ServerEvent decoding...")

    let e1 = try decodeEvent(#"{"type":"text_delta","text":"hello"}"#)
    if case .textDelta(let text) = e1 { assertEqual(text, "hello") }
    else { check(false, "Expected textDelta") }

    let e2 = try decodeEvent(#"{"type":"tool_start","id":"tool_1","name":"shell_exec"}"#)
    if case .toolStart(let id, let name) = e2 {
        assertEqual(id, "tool_1"); assertEqual(name, "shell_exec")
    } else { check(false, "Expected toolStart") }

    let e3 = try decodeEvent(#"{"type":"tool_done","id":"t1","name":"bash","output":"ok","error":null}"#)
    if case .toolDone(let id, let name, let output, let error) = e3 {
        assertEqual(id, "t1"); assertEqual(name, "bash"); assertEqual(output, "ok"); assertNil(error)
    } else { check(false, "Expected toolDone") }

    let e4 = try decodeEvent(#"{"type":"tokens","input":1000,"output":200,"cache_read_input":500}"#)
    if case .tokenUsage(let input, let output, let cacheRead, let cacheWrite) = e4 {
        assertEqual(input, 1000); assertEqual(output, 200); assertEqual(cacheRead, 500); assertNil(cacheWrite)
    } else { check(false, "Expected tokenUsage") }

    let e5 = try decodeEvent(#"{"type":"done","id":7}"#)
    if case .done(let id) = e5 { assertEqual(id, 7) }
    else { check(false, "Expected done") }

    let e6 = try decodeEvent(#"{"type":"error","id":3,"message":"something broke"}"#)
    if case .error(let id, let message) = e6 {
        assertEqual(id, 3); assertEqual(message, "something broke")
    } else { check(false, "Expected error") }

    let e7 = try decodeEvent(#"{"type":"session","session_id":"fox_abc123"}"#)
    if case .sessionId(let sid) = e7 { assertEqual(sid, "fox_abc123") }
    else { check(false, "Expected sessionId") }

    let e8 = try decodeEvent(#"{"type":"pong","id":99}"#)
    if case .pong(let id) = e8 { assertEqual(id, 99) }
    else { check(false, "Expected pong") }

    let e9 = try decodeEvent(#"{"type":"model_changed","id":2,"model":"gpt-4o","provider_name":"openai"}"#)
    if case .modelChanged(let id, let model, let provider, let error) = e9 {
        assertEqual(id, 2); assertEqual(model, "gpt-4o"); assertEqual(provider, "openai"); assertNil(error)
    } else { check(false, "Expected modelChanged") }

    let e10 = try decodeEvent(#"{"type":"future_event","data":"stuff"}"#)
    if case .unknown(let type, _) = e10 { assertEqual(type, "future_event") }
    else { check(false, "Expected unknown") }
}

// MARK: - History

do {
    print("  History decoding...")

    let json = """
    {"type":"history","id":1,"session_id":"fox","messages":[{"role":"user","content":"hi"}],
     "provider_name":"claude","provider_model":"claude-sonnet-4-20250514",
     "server_version":"v0.4.1","server_name":"blazing","server_icon":"🔥"}
    """
    let event = try decodeEvent(json)
    if case .history(let p) = event {
        assertEqual(p.sessionId, "fox")
        assertEqual(p.messages.count, 1)
        assertEqual(p.messages[0].role, "user")
        assertEqual(p.messages[0].content, "hi")
        assertEqual(p.providerName, "claude")
        assertEqual(p.serverName, "blazing")
        assertEqual(p.serverIcon, "🔥")
        assertEqual(p.serverVersion, "v0.4.1")
    } else { check(false, "Expected history") }
}

// MARK: - Notifications

do {
    print("  Notification decoding...")

    let json = """
    {"type":"notification","from_session":"sess_a","from_name":"fox",
     "notification_type":{"kind":"file_conflict","path":"src/main.rs","operation":"wrote"},
     "message":"fox edited src/main.rs"}
    """
    let event = try decodeEvent(json)
    if case .notification(let n) = event {
        assertEqual(n.fromSession, "sess_a")
        assertEqual(n.fromName, "fox")
        assertEqual(n.message, "fox edited src/main.rs")
        if case .fileConflict(let path, let op) = n.notificationType {
            assertEqual(path, "src/main.rs"); assertEqual(op, "wrote")
        } else { check(false, "Expected file_conflict") }
    } else { check(false, "Expected notification") }
}

// MARK: - Swarm

do {
    print("  Swarm decoding...")

    let json = """
    {"type":"swarm_status","members":[
        {"session_id":"s1","friendly_name":"fox","status":"running","role":"coordinator"}
    ]}
    """
    let event = try decodeEvent(json)
    if case .swarmStatus(let members) = event {
        assertEqual(members.count, 1)
        assertEqual(members[0].friendlyName, "fox")
        assertEqual(members[0].role, "coordinator")
        assertEqual(members[0].status, "running")
    } else { check(false, "Expected swarmStatus") }
}

// MARK: - Pairing types

do {
    print("  Pairing types...")

    let pr = try JSONDecoder().decode(PairResponse.self, from:
        #"{"token":"abc123","server_name":"jcode","server_version":"v0.4.1"}"#.data(using: .utf8)!)
    assertEqual(pr.token, "abc123")
    assertEqual(pr.serverName, "jcode")

    let hr = try JSONDecoder().decode(HealthResponse.self, from:
        #"{"status":"ok","version":"v0.4.1","gateway":true}"#.data(using: .utf8)!)
    assertEqual(hr.status, "ok")
    check(hr.gateway, "gateway should be true")
}

// MARK: - Request roundtrip

do {
    print("  Request roundtrip...")

    let requests: [Request] = [
        .ping(id: 1), .cancel(id: 2), .clear(id: 3), .getHistory(id: 4),
        .getState(id: 5), .setModel(id: 6, model: "claude-sonnet-4-20250514"),
        .compact(id: 7), .split(id: 8), .backgroundTool(id: 9),
        .resumeSession(id: 10, sessionId: "fox"),
        .cycleModel(id: 11, direction: -1),
    ]
    for req in requests {
        let json = try encodeRequest(req)
        check(json["type"] is String, "Missing type")
        check(json["id"] is UInt64 || json["id"] is Int, "Missing id")
    }
}

// MARK: - Results

print("")
if failed == 0 {
    print("All \(passed) assertions passed ✅")
} else {
    print("\(passed) passed, \(failed) FAILED ❌")
    exit(1)
}
