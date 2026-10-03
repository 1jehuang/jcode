import Foundation
import Testing

@testable import JCodeKit

private func run(_ events: [ConnectionOutput], from state: SessionState = SessionState())
    -> SessionState
{
    events.reduce(state) { SessionReducer.reduce($0, $1) }
}

private func event(_ line: String) -> ConnectionOutput {
    .event(try! ServerEvent.decode(line: line))
}

// MARK: - Streaming text

@Test func ackDoesNotMarkProcessing() {
    let state = run([event(#"{"type":"ack","id":1}"#)])
    #expect(state.isProcessing == false)
}

@Test func streamsAssistantText() {
    let state = run([
        .phase(.connected),
        event(#"{"type":"ack","id":1}"#),
        event(#"{"type":"text_delta","text":"Hel"}"#),
        event(#"{"type":"text_delta","text":"lo"}"#),
        event(#"{"type":"done","id":1}"#),
    ])
    #expect(state.transcript.count == 1)
    #expect(state.transcript[0].role == .assistant)
    #expect(state.transcript[0].text == "Hello")
    #expect(state.transcript[0].isStreaming == false)
    #expect(state.isProcessing == false)
}

@Test func userMessageThenResponse() {
    var state = SessionState()
    state = SessionReducer.reduce(state, intent: .userSentMessage("hi"))
    #expect(state.isProcessing)
    state = run(
        [
            event(#"{"type":"text_delta","text":"hey"}"#),
            event(#"{"type":"done","id":1}"#),
        ], from: state)
    #expect(state.transcript.map(\.role) == [.user, .assistant])
    #expect(state.transcript[1].text == "hey")
}

@Test func textReplaceOverwritesStreamedText() {
    let state = run([
        event(#"{"type":"text_delta","text":"garbled{{tool"}"#),
        event(#"{"type":"text_replace","text":"clean prefix"}"#),
    ])
    #expect(state.transcript[0].text == "clean prefix")
}

@Test func messageEndSplitsTurns() {
    let state = run([
        event(#"{"type":"text_delta","text":"first"}"#),
        event(#"{"type":"message_end"}"#),
        event(#"{"type":"text_delta","text":"second"}"#),
        event(#"{"type":"done","id":1}"#),
    ])
    #expect(state.transcript.map(\.text) == ["first", "second"])
}

// MARK: - Reasoning

@Test func reasoningStreamsSeparately() {
    var state = run([
        event(#"{"type":"reasoning_delta","text":"thinking..."}"#)
    ])
    #expect(state.isReasoning)
    #expect(state.transcript[0].reasoning == "thinking...")
    #expect(state.transcript[0].text == "")

    state = run(
        [
            event(#"{"type":"reasoning_done","duration_secs":2.0}"#),
            event(#"{"type":"text_delta","text":"answer"}"#),
        ], from: state)
    #expect(state.isReasoning == false)
    #expect(state.transcript[0].text == "answer")
}

// MARK: - Tool lifecycle

@Test func toolLifecycleTransitions() {
    var state = run([
        event(#"{"type":"tool_start","id":"t1","name":"bash"}"#),
        event(#"{"type":"tool_input","delta":"{\"command\":"}"#),
        event(#"{"type":"tool_input","delta":"\"ls\"}"}"#),
    ])
    #expect(state.transcript[0].toolCalls.count == 1)
    #expect(state.transcript[0].toolCalls[0].status == .streamingInput)
    #expect(state.transcript[0].toolCalls[0].input == #"{"command":"ls"}"#)

    state = run([event(#"{"type":"tool_exec","id":"t1","name":"bash"}"#)], from: state)
    #expect(state.transcript[0].toolCalls[0].status == .running)

    state = run(
        [event(#"{"type":"tool_done","id":"t1","name":"bash","output":"file.txt"}"#)],
        from: state)
    #expect(state.transcript[0].toolCalls[0].status == .succeeded)
    #expect(state.transcript[0].toolCalls[0].output == "file.txt")
}

@Test func toolFailureRecorded() {
    let state = run([
        event(#"{"type":"tool_start","id":"t1","name":"bash"}"#),
        event(#"{"type":"tool_exec","id":"t1","name":"bash"}"#),
        event(#"{"type":"tool_done","id":"t1","name":"bash","output":"","error":"exit 1"}"#),
    ])
    #expect(state.transcript[0].toolCalls[0].status == .failed("exit 1"))
}

@Test func multipleToolsInOneTurn() {
    let state = run([
        event(#"{"type":"tool_start","id":"t1","name":"read"}"#),
        event(#"{"type":"tool_done","id":"t1","name":"read","output":"a"}"#),
        event(#"{"type":"tool_start","id":"t2","name":"bash"}"#),
        event(#"{"type":"tool_done","id":"t2","name":"bash","output":"b"}"#),
        event(#"{"type":"text_delta","text":"done"}"#),
        event(#"{"type":"done","id":1}"#),
    ])
    #expect(state.transcript.count == 1)
    #expect(state.transcript[0].toolCalls.map(\.name) == ["read", "bash"])
    #expect(state.transcript[0].text == "done")
}

// MARK: - Errors and interrupts

@Test func errorSetsBannerAndStopsProcessing() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = run(
        [
            event(
                #"{"type":"error","id":1,"message":"overloaded","retry_after_secs":15}"#)
        ], from: state)
    #expect(state.errorBanner == "overloaded (retry in 15s)")
    #expect(state.isProcessing == false)

    state = SessionReducer.reduce(state, intent: .dismissError)
    #expect(state.errorBanner == nil)
}

@Test func interruptStopsProcessingAndNotes() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = run(
        [
            event(#"{"type":"text_delta","text":"partial"}"#),
            event(#"{"type":"interrupted"}"#),
        ], from: state)
    #expect(state.isProcessing == false)
    #expect(state.transcript.last?.text == "partial")
    #expect(state.transcript.last?.isStreaming == false)
    #expect(state.notices.contains { $0.message == "Interrupted" })
}

// MARK: - Notices (notifications / compaction / dismissal)

@Test func notificationBecomesDismissibleNotice() {
    let state = run([
        event(#"{"type":"notification","from_name":"swarm","message":"build done"}"#)
    ])
    #expect(state.notices.count == 1)
    #expect(state.notices[0].kind == .notification)
    #expect(state.notices[0].message == "swarm: build done")
}

@Test func notificationWithoutSenderHasNoPrefix() {
    let state = run([
        event(#"{"type":"notification","message":"heads up"}"#)
    ])
    #expect(state.notices[0].message == "heads up")
}

@Test func foregroundCompactionSurfacesNotice() {
    let state = run([
        event(#"{"type":"compaction","trigger":"manual","tokens_saved":1234}"#)
    ])
    #expect(state.notices.count == 1)
    #expect(state.notices[0].kind == .compaction)
    #expect(state.notices[0].message.contains("1234"))
}

@Test func backgroundCompactionIsSilent() {
    let state = run([
        event(#"{"type":"compaction","trigger":"background","tokens_saved":50}"#)
    ])
    #expect(state.notices.isEmpty)
}

@Test func dismissNoticeRemovesOnlyThatNotice() {
    var state = run([
        event(#"{"type":"notification","message":"first"}"#),
        event(#"{"type":"notification","message":"second"}"#),
    ])
    #expect(state.notices.count == 2)
    let firstID = state.notices[0].id
    state = SessionReducer.reduce(state, intent: .dismissNotice(firstID))
    #expect(state.notices.count == 1)
    #expect(state.notices[0].message == "second")
}

@Test func clearedConversationWipesTranscriptButKeepsSession() {
    var state = run([
        event(#"{"type":"session","session_id":"sess_keep"}"#),
        event(#"{"type":"text_delta","text":"stale"}"#),
        event(#"{"type":"done","id":1}"#),
    ])
    #expect(!state.transcript.isEmpty)
    state = SessionReducer.reduce(state, intent: .clearedConversation)
    #expect(state.transcript.isEmpty)
    #expect(state.isProcessing == false)
    #expect(state.sessionID == "sess_keep")
}

@Test func emptyStreamingStubIsDropped() {
    let state = run([
        event(#"{"type":"ack","id":1}"#),
        event(#"{"type":"interrupted"}"#),
    ])
    #expect(state.transcript.isEmpty)
}

// MARK: - History sync

@Test func historyReplacesTranscript() {
    var state = run([
        event(#"{"type":"text_delta","text":"stale"}"#)
    ])
    let history = """
        {"type":"history","id":2,"session_id":"sess_1","messages":[\
        {"role":"user","content":"question"},\
        {"role":"assistant","content":"answer"}\
        ],"provider_model":"claude-sonnet-4","available_models":["a","b"],\
        "all_sessions":["sess_1","sess_2"],"total_tokens":[100,50],\
        "server_version":"v1","display_title":"T"}
        """
    state = run([event(history)], from: state)
    #expect(state.transcript.map(\.text) == ["question", "answer"])
    #expect(state.sessionID == "sess_1")
    #expect(state.modelName == "claude-sonnet-4")
    #expect(state.availableModels == ["a", "b"])
    #expect(state.allSessions == ["sess_1", "sess_2"])
    #expect(state.tokenInput == 100)
    #expect(state.tokenOutput == 50)
    #expect(state.sessionTitle == "T")
}

@Test func historySkipsEmptyAssistantStubs() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"user","content":"q"},\
        {"role":"assistant","content":""},\
        {"role":"tool","content":"raw tool output"}\
        ]}
        """
    let state = run([event(history)])
    #expect(state.transcript.count == 1)
    #expect(state.transcript[0].role == .user)
}

@Test func historyAttachesToolResultsToTheAssistantTurn() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"user","content":"run it"},\
        {"role":"assistant","content":"Running it.","tool_calls":["bash"]},\
        {"role":"tool","content":"card-test\\n","tool_data":\
        {"id":"toolu_1","name":"bash","input":{"command":"echo card-test"}}},\
        {"role":"assistant","content":"OK"}\
        ]}
        """
    let state = run([event(history)])
    #expect(state.transcript.map(\.role) == [.user, .assistant, .assistant])
    let calls = state.transcript[1].toolCalls
    #expect(calls.count == 1)
    #expect(calls.first?.id == "toolu_1")
    #expect(calls.first?.name == "bash")
    #expect(calls.first?.input == #"{"command":"echo card-test"}"#)
    #expect(calls.first?.output == "card-test\n")
    #expect(calls.first?.status == .succeeded)
    #expect(state.transcript[2].text == "OK")
}

@Test func historyResultsFillSameNamedCallsInOrder() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"assistant","content":"Two runs.","tool_calls":["bash","bash"]},\
        {"role":"tool","content":"one","tool_data":{"id":"toolu_1","name":"bash","input":{}}},\
        {"role":"tool","content":"two","tool_data":{"id":"toolu_2","name":"bash","input":{}}}\
        ]}
        """
    let calls = run([event(history)]).transcript[0].toolCalls
    #expect(calls.map(\.id) == ["toolu_1", "toolu_2"])
    #expect(calls.map(\.output) == ["one", "two"])
}

@Test func historyToolErrorOutputIsShownAsFailed() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"assistant","content":"Reading.","tool_calls":["read"]},\
        {"role":"tool","content":"Error: file not found\\nat /tmp/x","tool_data":\
        {"id":"toolu_1","name":"read","input":{"file_path":"/tmp/x"}}},\
        {"role":"tool","content":"--- Command finished with exit code: 2 ---","tool_data":\
        {"id":"toolu_2","name":"bash","input":{"command":"false"}}}\
        ]}
        """
    let calls = run([event(history)]).transcript[0].toolCalls
    #expect(calls.first?.status == .failed("Error: file not found"))
    #expect(calls.first?.output == "Error: file not found\nat /tmp/x")
    #expect(calls.last?.status == .failed("--- Command finished with exit code: 2 ---"))
}

@Test func historyToolOutputMentioningErrorsIsSucceeded() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"assistant","content":"Grep.","tool_calls":["grep"]},\
        {"role":"tool","content":"src/a.rs: error: handled\\nExit code: 0","tool_data":\
        {"id":"toolu_1","name":"grep","input":{}}}\
        ]}
        """
    #expect(run([event(history)]).transcript[0].toolCalls.first?.status == .succeeded)
}

@Test func historyToolResultWithoutAssistantGetsItsOwnEntry() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"user","content":"q"},\
        {"role":"tool","content":"out","tool_data":{"id":"t9","name":"read","input":"{}"}}\
        ]}
        """
    let state = run([event(history)])
    #expect(state.transcript.count == 2)
    #expect(state.transcript[1].role == .assistant)
    #expect(state.transcript[1].toolCalls.first?.name == "read")
    #expect(state.transcript[1].toolCalls.first?.output == "out")
}

@Test func historyMapsToolData() {
    let history = """
        {"type":"history","id":1,"session_id":"s","messages":[\
        {"role":"assistant","content":"used a tool","tool_data":\
        {"id":"t1","name":"bash","input":"{}","output":"ok"}}\
        ]}
        """
    let state = run([event(history)])
    #expect(state.transcript[0].toolCalls.count == 1)
    #expect(state.transcript[0].toolCalls[0].status == .succeeded)
}

// MARK: - Connection phases

@Test func disconnectFinishesStreamingEntries() {
    let state = run([
        .phase(.connected),
        event(#"{"type":"text_delta","text":"part"}"#),
        .phase(.reconnecting(attempt: 1)),
    ])
    #expect(state.phase == .reconnecting(attempt: 1))
    #expect(state.transcript[0].isStreaming == false)
    #expect(state.isProcessing == false)
}

@Test func failureSetsBanner() {
    let state = run([.phase(.failed(reason: "Could not reach server"))])
    #expect(state.errorBanner == "Could not reach server")
}

@Test func reconnectClearsBannerOnConnected() {
    var state = run([.phase(.failed(reason: "down"))])
    state = run([.phase(.connected)], from: state)
    #expect(state.errorBanner == nil)
}

// MARK: - Session metadata

@Test func sessionRenameAppliesToCurrentSession() {
    var state = run([event(#"{"type":"session","session_id":"sess_1"}"#)])
    state = run(
        [
            event(
                #"{"type":"session_renamed","session_id":"sess_1","display_title":"Renamed"}"#)
        ], from: state)
    #expect(state.sessionTitle == "Renamed")

    state = run(
        [
            event(
                #"{"type":"session_renamed","session_id":"other","display_title":"Nope"}"#)
        ], from: state)
    #expect(state.sessionTitle == "Renamed")
}

@Test func modelChangeUpdatesOrErrors() {
    var state = run([event(#"{"type":"model_changed","id":1,"model":"gpt-5"}"#)])
    #expect(state.modelName == "gpt-5")

    state = run(
        [
            event(
                #"{"type":"model_changed","id":2,"model":"","error":"unknown model"}"#)
        ], from: state)
    #expect(state.errorBanner == "unknown model")
    #expect(state.modelName == "gpt-5")
}

@Test func resetClearsEverything() {
    var state = run([
        event(#"{"type":"text_delta","text":"data"}"#),
        event(#"{"type":"tokens","input":5,"output":6}"#),
    ])
    state = SessionReducer.reduce(state, intent: .reset)
    #expect(state == SessionState())
}

// MARK: - Queued soft-interrupts

@Test func queuedInterruptTracksPendingState() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = SessionReducer.reduce(state, intent: .userQueuedInterrupt("also do y"))
    #expect(state.hasPendingInterrupts)
    #expect(state.pendingInterrupts == ["also do y"])
    #expect(state.transcript.last?.isQueued == true)
    #expect(state.transcript.last?.text == "also do y")
}

@Test func injectionClearsMatchingQueuedInterrupt() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = SessionReducer.reduce(state, intent: .userQueuedInterrupt("also do y"))
    state = run(
        [
            event(
                #"{"type":"soft_interrupt_injected","content":"also do y","point":"C"}"#)
        ], from: state)
    #expect(state.pendingInterrupts.isEmpty)
    #expect(state.transcript.last?.isQueued == false)
    #expect(state.transcript.last?.text == "also do y")
}

@Test func injectionFromAnotherClientAppendsEntry() {
    let state = run([
        event(
            #"{"type":"soft_interrupt_injected","content":"external note","display_role":"system","point":"A"}"#
        )
    ])
    #expect(state.transcript.count == 1)
    #expect(state.transcript[0].role == .system)
    #expect(state.transcript[0].text == "external note")
    #expect(state.transcript[0].isQueued == false)
}

@Test func cancelledQueuedInterruptsRemovesOptimisticBubbles() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = SessionReducer.reduce(state, intent: .userQueuedInterrupt("nevermind"))
    state = SessionReducer.reduce(state, intent: .cancelledQueuedInterrupts)
    #expect(state.pendingInterrupts.isEmpty)
    #expect(!state.transcript.contains { $0.isQueued })
    #expect(state.transcript.map(\.text) == ["go"])
}

@Test func turnCompletionDrainsPendingInterrupts() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = SessionReducer.reduce(state, intent: .userQueuedInterrupt("late note"))
    state = run(
        [
            event(#"{"type":"text_delta","text":"answer"}"#),
            event(#"{"type":"done","id":1}"#),
        ], from: state)
    #expect(state.pendingInterrupts.isEmpty)
    #expect(!state.transcript.contains { $0.isQueued })
}

@Test func historyResyncClearsPendingInterrupts() {
    var state = SessionReducer.reduce(SessionState(), intent: .userQueuedInterrupt("queued"))
    #expect(state.hasPendingInterrupts)
    state = run(
        [
            event(
                #"{"type":"history","id":1,"session_id":"s","messages":[{"role":"user","content":"q"}]}"#
            )
        ], from: state)
    #expect(state.pendingInterrupts.isEmpty)
}

// MARK: - Turn-level connection phase

@Test func connectionPhaseTracksAndClears() {
    var state = run([event(#"{"type":"connection_phase","phase":"authenticating"}"#)])
    #expect(state.serverPhase == "authenticating")

    state = run([event(#"{"type":"done","id":1}"#)], from: state)
    #expect(state.serverPhase == nil)

    state = run([event(#"{"type":"connection_phase","phase":"waiting"}"#)], from: state)
    state = run([.phase(.reconnecting(attempt: 1))], from: state)
    #expect(state.serverPhase == nil)
}

// MARK: - Retry rollback

@Test func retryRollbackDiscardsPartialOutput() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = run(
        [
            event(#"{"type":"reasoning_delta","text":"thinking"}"#),
            event(#"{"type":"text_delta","text":"partial answer"}"#),
            event(#"{"type":"retry_rollback","attempt":1,"max":5}"#),
        ], from: state)
    // Only the user message survives; the partial assistant entry is discarded.
    #expect(state.transcript.map(\.role) == [.user])
    #expect(state.isReasoning == false)
    #expect(state.statusDetail == "Retrying (1/5)")

    // The replayed response streams into a fresh entry without duplication.
    state = run(
        [
            event(#"{"type":"text_delta","text":"clean answer"}"#),
            event(#"{"type":"done","id":1}"#),
        ], from: state)
    #expect(state.transcript.map(\.text) == ["go", "clean answer"])
}

@Test func retryRollbackKeepsCommittedTurns() {
    let state = run([
        event(#"{"type":"text_delta","text":"first"}"#),
        event(#"{"type":"message_end"}"#),
        event(#"{"type":"text_delta","text":"second partial"}"#),
        event(#"{"type":"retry_rollback","attempt":1,"max":3}"#),
    ])
    #expect(state.transcript.map(\.text) == ["first"])
}

// MARK: - Session titles

@Test func renameOfOtherSessionStillRecordsTitle() {
    var state = run([event(#"{"type":"session","session_id":"sess_1"}"#)])
    state = run(
        [
            event(
                #"{"type":"session_renamed","session_id":"other","display_title":"Other work"}"#)
        ], from: state)
    #expect(state.sessionTitle == nil)
    #expect(state.title(forSession: "other") == "Other work")
}

@Test func historyRecordsTitleForSessionList() {
    let state = run([
        event(
            #"{"type":"history","id":1,"session_id":"sess_1","messages":[],"all_sessions":["sess_1","sess_2"],"display_title":"Main"}"#
        )
    ])
    #expect(state.title(forSession: "sess_1") == "Main")
    #expect(state.title(forSession: "sess_2") == nil)
    #expect(state.sessionTitle == "Main")
}

// MARK: - Reasoning effort

@Test func reasoningEffortChangeUpdatesOrErrors() {
    var state = run([
        event(#"{"type":"reasoning_effort_changed","id":1,"effort":"high"}"#)
    ])
    #expect(state.reasoningEffort == "high")

    state = run(
        [
            event(#"{"type":"reasoning_effort_changed","id":2,"error":"unsupported"}"#)
        ], from: state)
    #expect(state.errorBanner == "unsupported")
    #expect(state.reasoningEffort == "high")
}

@Test func historyCarriesReasoningEffortIntoState() {
    let state = run([
        event(
            #"{"type":"history","id":1,"session_id":"s","messages":[],"reasoning_effort":"medium"}"#
        )
    ])
    #expect(state.reasoningEffort == "medium")
}

// MARK: - Compaction result

@Test func compactResultSuccessBecomesNotice() {
    let state = run([
        event(#"{"type":"compact_result","id":1,"message":"Compaction started","success":true}"#)
    ])
    #expect(state.notices.count == 1)
    #expect(state.notices[0].kind == .compaction)
    #expect(state.errorBanner == nil)
}

@Test func compactResultFailureBecomesError() {
    let state = run([
        event(#"{"type":"compact_result","id":1,"message":"Nothing to compact","success":false}"#)
    ])
    #expect(state.notices.isEmpty)
    #expect(state.errorBanner == "Nothing to compact")
}

// MARK: - Server lifecycle events

@Test func reloadingSurfacesNotice() {
    let state = run([event(#"{"type":"reloading"}"#)])
    #expect(state.notices.count == 1)
    #expect(state.notices[0].message.contains("updating"))
}

@Test func sessionCloseRequestedStopsProcessingWithReason() {
    var state = SessionReducer.reduce(SessionState(), intent: .userSentMessage("go"))
    state = run(
        [
            event(#"{"type":"text_delta","text":"partial"}"#),
            event(#"{"type":"session_close_requested","reason":"taken over by TUI"}"#),
        ], from: state)
    #expect(state.isProcessing == false)
    #expect(state.errorBanner == "taken over by TUI")
    #expect(state.transcript.last?.isStreaming == false)
}
