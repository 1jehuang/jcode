import Foundation
import Testing

@testable import JCodeKit

/// Demo mode is the App Review and first-run path: with no jcode server on the
/// network the app must still be fully explorable. These tests pin that the
/// scripted transport speaks the real protocol through the real `Connection`
/// and `SessionReducer`, and never touches the network.
@Suite("DemoTransport")
struct DemoTransportTests {
    /// Drives a demo connection and folds every output into session state.
    private func runDemo(
        sending prompts: [String],
        until isDone: @escaping @Sendable (SessionState) -> Bool
    ) async throws -> SessionState {
        let connection = Connection(
            configuration: .init(
                gateway: Gateway(host: DemoTransport.host),
                authToken: "demo",
                workingDir: "/demo"
            ),
            makeTransport: { DemoTransport(speed: 0) }
        )
        let stream = await connection.start()
        var state = SessionState()
        var sent = false
        for await output in stream {
            state = SessionReducer.reduce(state, output)
            if !sent, state.sessionID != nil {
                sent = true
                for prompt in prompts {
                    try await connection.send { .message(id: $0, content: prompt) }
                }
            }
            if isDone(state) { break }
        }
        await connection.stop()
        return state
    }

    @Test("connects and reports a session without any network")
    func connectsOffline() async throws {
        let state = try await runDemo(sending: []) { $0.sessionID != nil }
        #expect(state.sessionID == DemoTransport.sessionID)
        #expect(state.phase == .connected)
    }

    @Test("history announces the demo provider and model list")
    func historyPopulatesModels() async throws {
        let state = try await runDemo(sending: []) { !$0.availableModels.isEmpty }
        #expect(state.providerName == "demo")
        #expect(state.modelName == DemoTransport.defaultModel)
        #expect(state.availableModels == DemoTransport.availableModels)
        #expect(state.serverVersion == "demo")
    }

    @Test("a prompt streams reasoning, a tool call, and an answer")
    func promptProducesFullTurn() async throws {
        let state = try await runDemo(sending: ["Run the tests"]) { state in
            state.transcript.contains {
                $0.role == .assistant && !$0.text.isEmpty && !$0.toolCalls.isEmpty
                    && !$0.isStreaming
            }
        }
        let assistant = try #require(state.transcript.last { $0.role == .assistant })
        #expect(!assistant.reasoning.isEmpty)
        #expect(assistant.toolCalls.count == 1)
        #expect(assistant.toolCalls[0].name == "bash")
        #expect(assistant.toolCalls[0].status == .succeeded)
        #expect(assistant.text.contains("94"))
        // The user echo is added by the app's intent reducer, not the server,
        // so it is absent here; the scripted server records it in history.
    }

    @Test("every starter suggestion produces a scripted answer")
    func suggestionsAllAnswer() async throws {
        for suggestion in DemoScript.suggestions {
            let reply = DemoScript.forPrompt(suggestion)
            #expect(!reply.answer.isEmpty)
            #expect(!reply.reasoning.isEmpty)
            #expect(reply.tool != nil)
        }
    }

    @Test("an unrecognized prompt still gets an answer that discloses the demo")
    func fallbackDisclosesDemo() {
        let reply = DemoScript.forPrompt("zzzz unknown prompt")
        #expect(reply.answer.lowercased().contains("demo"))
    }

    @Test("model changes are echoed back so the picker settles")
    func modelChangeEchoes() async throws {
        let transport = DemoTransport(speed: 0)
        try await transport.connect(url: Gateway(host: DemoTransport.host).webSocketURL, authToken: "demo")
        try await transport.send(text: #"{"id":1,"type":"set_model","model":"openai-api:gpt-5.5"}"#)
        let line = try #require(try await transport.receiveText())
        #expect(try ServerEvent.decode(line: line) == .modelChanged(id: 1, model: "openai-api:gpt-5.5", error: nil))
        await transport.close()
    }

    @Test("closing ends the receive stream instead of hanging")
    func closeEndsStream() async throws {
        let transport = DemoTransport(speed: 0)
        try await transport.connect(url: Gateway(host: DemoTransport.host).webSocketURL, authToken: "demo")
        await transport.close()
        let next = try await transport.receiveText()
        #expect(next == nil)
    }

    @Test("chunking preserves the original string")
    func chunkingRoundTrips() {
        let text = "the quick brown fox"
        #expect(text.chunked(6).joined() == text)
        #expect("".chunked(4).isEmpty)
    }
}

/// Regression coverage for the transcript race that made a user's own bubble
/// disappear when a `history` payload arrived just after an optimistic send.
@Suite("History and optimistic sends")
struct OptimisticHistoryTests {
    @Test("a just-sent message survives a history payload that predates it")
    func optimisticSendSurvivesHistory() {
        var state = SessionState()
        state = SessionReducer.reduce(state, intent: .userSentMessage("hello there"))
        #expect(state.transcript.contains { $0.role == .user && $0.text == "hello there" })

        // Server history produced before the message landed.
        state = SessionReducer.reduce(
            state,
            .event(
                .history(
                    .init(id: 1, sessionID: "s", messages: [])
                )))
        #expect(state.transcript.contains { $0.role == .user && $0.text == "hello there" })
    }

    @Test("history does not duplicate a message it already contains")
    func noDuplicateOnceServerCatchesUp() {
        var state = SessionState()
        state = SessionReducer.reduce(state, intent: .userSentMessage("hello there"))
        state = SessionReducer.reduce(
            state,
            .event(
                .history(
                    .init(
                        id: 1, sessionID: "s",
                        messages: [HistoryMessage(role: "user", content: "hello there")]
                    )
                )))
        #expect(state.transcript.filter { $0.text == "hello there" }.count == 1)
    }

    @Test("history still replaces stale assistant content")
    func historyRemainsAuthoritative() {
        var state = SessionState()
        state = SessionReducer.reduce(
            state,
            .event(
                .history(
                    .init(
                        id: 1, sessionID: "s",
                        messages: [HistoryMessage(role: "assistant", content: "stale")]
                    )
                )))
        state = SessionReducer.reduce(
            state,
            .event(
                .history(
                    .init(
                        id: 2, sessionID: "s",
                        messages: [HistoryMessage(role: "assistant", content: "fresh")]
                    )
                )))
        #expect(state.transcript.map(\.text) == ["fresh"])
    }
}
