import Testing

@testable import JCodeKit

@Suite("ComposerRules")
struct ComposerRulesTests {
    @Test func returnKeySendsWhenIdle() {
        #expect(
            ComposerRules.submitAction(draft: "hello\n", isConnected: true, isProcessing: false)
                == .send("hello")
        )
    }

    @Test func returnKeyQueuesWhileProcessing() {
        #expect(
            ComposerRules.submitAction(draft: "next task\n", isConnected: true, isProcessing: true)
                == .queue("next task")
        )
    }

    @Test func emptyOrWhitespaceDraftIsIgnored() {
        for draft in ["", "\n", "   ", " \n\t "] {
            #expect(
                ComposerRules.submitAction(draft: draft, isConnected: true, isProcessing: false)
                    == .ignore,
                "draft \(draft.debugDescription) should not send"
            )
        }
    }

    @Test func disconnectedNeverSends() {
        #expect(
            ComposerRules.submitAction(draft: "hi", isConnected: false, isProcessing: false)
                == .ignore
        )
        #expect(!ComposerRules.canSubmit(draft: "hi", isConnected: false))
    }

    @Test func shiftReturnInsertsNewline() {
        #expect(
            ComposerRules.submitAction(
                draft: "line one", isConnected: true, isProcessing: false, wantsNewline: true
            ) == .newline
        )
    }

    @Test func trailingNewlineIsDetectedAsSubmit() {
        #expect(ComposerRules.isReturnKeySubmit("hello\n"))
        #expect(ComposerRules.isReturnKeySubmit("multi\nline\n"))
    }

    @Test func nonTrailingNewlineIsNotSubmit() {
        // Pasting multi-line content must not auto-send.
        #expect(!ComposerRules.isReturnKeySubmit("line one\nline two"))
        #expect(!ComposerRules.isReturnKeySubmit("no newline at all"))
    }

    @Test func newlineOnlyDraftIsNotSubmit() {
        // Return pressed on an empty composer is a no-op, not an empty send.
        #expect(!ComposerRules.isReturnKeySubmit("\n"))
        #expect(!ComposerRules.isReturnKeySubmit("   \n"))
    }

    @Test func submitPreservesInternalNewlinesAndStripsEdges() {
        #expect(
            ComposerRules.submitAction(
                draft: "  first\nsecond  \n", isConnected: true, isProcessing: false
            ) == .send("first\nsecond")
        )
    }
}
