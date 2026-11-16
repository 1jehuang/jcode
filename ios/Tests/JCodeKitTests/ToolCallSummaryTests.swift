import Testing

@testable import JCodeKit

@Suite("ToolCallSummary")
struct ToolCallSummaryTests {
    @Test func extractsIntentFromCompleteJSON() {
        let input = #"{"command":"cargo test","intent":"Verify the build passes"}"#
        #expect(ToolCallSummary.intent(from: input) == "Verify the build passes")
    }

    @Test func headlinePrefersIntentOverSubject() {
        let input = #"{"command":"cargo test","intent":"Verify the build passes"}"#
        #expect(ToolCallSummary.headline(from: input) == "Verify the build passes")
        #expect(ToolCallSummary.subject(from: input) == "cargo test")
    }

    @Test func headlineFallsBackToSubjectWithoutIntent() {
        let input = #"{"command":"ls -la"}"#
        #expect(ToolCallSummary.intent(from: input) == nil)
        #expect(ToolCallSummary.headline(from: input) == "ls -la")
    }

    @Test func recoversIntentWhileStillStreaming() {
        // Deltas arrive mid-string, so the JSON is not yet parseable.
        let partial = #"{"command":"cargo test","intent":"Verify the build"#
        #expect(ToolCallSummary.intent(from: partial) == "Verify the build")
    }

    @Test func recoversSubjectWhileStillStreaming() {
        let partial = #"{"file_path":"/tmp/notes.m"#
        #expect(ToolCallSummary.subject(from: partial) == "/tmp/notes.m")
    }

    @Test func handlesEscapedQuotesAndNewlines() {
        let input = #"{"intent":"Run \"cargo test\" twice","command":"x"}"#
        #expect(ToolCallSummary.intent(from: input) == #"Run "cargo test" twice"#)
        let multiline = #"{"intent":"line one\nline two"}"#
        #expect(ToolCallSummary.intent(from: multiline) == "line one line two")
    }

    @Test func emptyInputHasNoSummary() {
        #expect(ToolCallSummary.intent(from: "") == nil)
        #expect(ToolCallSummary.subject(from: "") == nil)
        #expect(ToolCallSummary.headline(from: "") == nil)
    }

    @Test func emptyIntentValueIsIgnored() {
        #expect(ToolCallSummary.intent(from: #"{"intent":"","command":"ls"}"#) == nil)
        #expect(ToolCallSummary.headline(from: #"{"intent":"","command":"ls"}"#) == "ls")
    }

    @Test func subjectKeyPriorityIsStable() {
        // command wins over file_path when both are present.
        let input = #"{"file_path":"/a/b.txt","command":"cat /a/b.txt"}"#
        #expect(ToolCallSummary.subject(from: input) == "cat /a/b.txt")
    }

    @Test func nonJSONInputIsShownRaw() {
        #expect(ToolCallSummary.subject(from: "plain text arg") == "plain text arg")
    }

    @Test func summaryIsAlwaysSingleLine() {
        let input = #"{"intent":"first\n\tsecond","command":"x"}"#
        let intent = ToolCallSummary.intent(from: input)
        #expect(intent != nil)
        #expect(!(intent ?? "").contains("\n"))
        #expect(!(intent ?? "").contains("\t"))
    }

    @Test func intentAmongManyKeysIsFound() {
        let input = """
            {"pattern":"foo","glob":"**/*.rs","max_files":20,\
            "intent":"Locate the gateway handler"}
            """
        #expect(ToolCallSummary.intent(from: input) == "Locate the gateway handler")
    }

    @Test func unopenedIntentValueYieldsNil() {
        // Key arrived but the value has not started; do not invent a summary.
        #expect(ToolCallSummary.intent(from: #"{"command":"ls","intent"#) == nil)
        #expect(ToolCallSummary.intent(from: #"{"command":"ls","intent":"#) == nil)
    }
}
