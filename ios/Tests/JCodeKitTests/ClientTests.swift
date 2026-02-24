import Foundation
@testable import JCodeKit

nonisolated(unsafe) var passed2 = 0
nonisolated(unsafe) var failed2 = 0

func check2(_ condition: Bool, _ msg: String = "", file: String = #file, line: Int = #line) {
    if condition {
        passed2 += 1
    } else {
        failed2 += 1
        print("  FAIL [\(file.split(separator: "/").last ?? ""):\(line)] \(msg)")
    }
}

func assertEqual2<T: Equatable>(_ a: T, _ b: T, _ msg: String = "", file: String = #file, line: Int = #line) {
    if a == b {
        passed2 += 1
    } else {
        failed2 += 1
        print("  FAIL [\(file.split(separator: "/").last ?? ""):\(line)] \(msg.isEmpty ? "\(a) != \(b)" : msg)")
    }
}

func runClientTests() {
    print("\nClient Tests")
    print("============")

    do {
        print("  MessageContent...")
        let msg = MessageContent(role: .user, text: "hello")
        assertEqual2(msg.role, .user)
        assertEqual2(msg.text, "hello")
        check2(msg.toolCalls.isEmpty)
    }

    do {
        print("  ToolCallInfo...")
        var tool = ToolCallInfo(id: "t1", name: "shell_exec")
        assertEqual2(tool.id, "t1")
        assertEqual2(tool.name, "shell_exec")
        assertEqual2(tool.input, "")
        check2(tool.output == nil)
        check2(tool.error == nil)

        tool.input = "{\"command\":\"ls\"}"
        tool.output = "file1.txt\nfile2.txt"
        tool.state = .done
        assertEqual2(tool.input, "{\"command\":\"ls\"}")
        assertEqual2(tool.output, "file1.txt\nfile2.txt")
    }

    do {
        print("  ServerInfo defaults...")
        let info = ServerInfo()
        assertEqual2(info.sessionId, "")
        check2(info.serverName == nil)
        check2(info.providerName == nil)
        assertEqual2(info.totalInputTokens, 0)
        assertEqual2(info.totalOutputTokens, 0)
        check2(!info.isCanary)
        check2(!info.wasInterrupted)
        check2(info.allSessions.isEmpty)
        check2(info.availableModels.isEmpty)
    }

    do {
        print("  TokenUpdate...")
        let update = TokenUpdate(input: 500, output: 100, cacheRead: 200, cacheWrite: nil)
        assertEqual2(update.input, 500)
        assertEqual2(update.output, 100)
        assertEqual2(update.cacheRead, 200)
        check2(update.cacheWrite == nil)
    }

    do {
        print("  ServerCredential codable...")
        let cred = ServerCredential(
            host: "laptop.ts.net",
            port: 7643,
            authToken: "abc123",
            serverName: "jcode",
            serverVersion: "v0.4.1",
            deviceId: "iphone-test",
            pairedAt: Date(timeIntervalSince1970: 1700000000)
        )
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        let data = try encoder.encode(cred)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let decoded = try decoder.decode(ServerCredential.self, from: data)
        assertEqual2(decoded.host, "laptop.ts.net")
        assertEqual2(decoded.port, 7643)
        assertEqual2(decoded.authToken, "abc123")
        assertEqual2(decoded.serverName, "jcode")
        assertEqual2(decoded.deviceId, "iphone-test")
    } catch {
        failed2 += 1
        print("  FAIL: ServerCredential codable: \(error)")
    }

    do {
        print("  HistoryMessage with tool data...")
        let json = """
        {"role":"assistant","content":"Let me check.",
         "tool_calls":["shell_exec"],
         "tool_data":{"id":"t1","name":"shell_exec","input":"ls","output":"files"}}
        """
        let msg = try JSONDecoder().decode(HistoryMessage.self, from: json.data(using: .utf8)!)
        assertEqual2(msg.role, "assistant")
        assertEqual2(msg.content, "Let me check.")
        assertEqual2(msg.toolCalls?.count, 1)
        assertEqual2(msg.toolCalls?.first, "shell_exec")
        assertEqual2(msg.toolData?.id, "t1")
        assertEqual2(msg.toolData?.name, "shell_exec")
    } catch {
        failed2 += 1
        print("  FAIL: HistoryMessage: \(error)")
    }

    do {
        print("  Full event sequence simulation...")
        let events = [
            #"{"type":"session","session_id":"fox_123"}"#,
            #"{"type":"text_delta","text":"I'll help "}"#,
            #"{"type":"text_delta","text":"you with that."}"#,
            #"{"type":"interrupted"}"#,
            #"{"type":"tool_start","id":"t1","name":"file_read"}"#,
            #"{"type":"tool_input","delta":"{\"path\":\""}"#,
            #"{"type":"tool_input","delta":"src/main.rs\"}"}"#,
            #"{"type":"tool_exec","id":"t1","name":"file_read"}"#,
            #"{"type":"tool_done","id":"t1","name":"file_read","output":"fn main() {}","error":null}"#,
            #"{"type":"tokens","input":500,"output":100}"#,
            #"{"type":"done","id":1}"#,
        ]

        var textParts: [String] = []
        var sawInterrupted = false
        var toolStarted = false
        var toolDone = false
        var turnDone = false

        for jsonStr in events {
            let event = try JSONDecoder().decode(ServerEvent.self, from: jsonStr.data(using: .utf8)!)
            switch event {
            case .sessionId(let sid): assertEqual2(sid, "fox_123")
            case .textDelta(let text): textParts.append(text)
            case .interrupted: sawInterrupted = true
            case .toolStart(_, let name): toolStarted = true; assertEqual2(name, "file_read")
            case .toolDone(_, _, let output, _): toolDone = true; assertEqual2(output, "fn main() {}")
            case .done: turnDone = true
            default: break
            }
        }

        assertEqual2(textParts.joined(), "I'll help you with that.")
        check2(sawInterrupted, "interrupt event should decode")
        check2(toolStarted, "tool should have started")
        check2(toolDone, "tool should have finished")
        check2(turnDone, "turn should be done")
    } catch {
        failed2 += 1
        print("  FAIL: Event sequence: \(error)")
    }

    print("")
    if failed2 == 0 {
        print("All \(passed2) client assertions passed ✅")
    } else {
        print("\(passed2) passed, \(failed2) FAILED ❌")
    }
}
