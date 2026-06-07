// Tests for the streaming reasoning region helpers.
//
// Reasoning text is rendered as dim, italic lines (no blockquote `│` gutter, no
// header, no footer). Each complete line is wrapped in `*…*` with an invisible
// REASONING_SENTINEL inside both ends that the markdown renderer strips and dims.
// (Both ends so whitespace at the line edges can't break CommonMark emphasis.) The
// region auto-closes when real output or a tool call begins so the final answer
// renders as normal (non-italic) text.
//
// The in-progress (not yet newline-terminated) line renders live as a partial
// `*…*` tail so reasoning trickles in token-by-token; that tail is rebuilt in
// place on each delta and promoted to a committed line when its newline arrives.
//
// In `current` mode (the default) reasoning is *ephemeral*: only the live block is
// ever shown. Once it closes (the model answers or runs a tool) the whole block is
// sliced back out of the stream in place, so no per-block trace accumulates and
// answer text keeps its order.

#[test]
fn reasoning_region_emits_dim_italic_lines_no_gutter_header_or_footer() {
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("Let me think.\nSecond thought.");
    // While streaming, reasoning is dim+italic markup in the live stream buffer.
    let streaming = app.streaming_text().to_string();
    assert!(
        !streaming.contains("Thinking"),
        "no header expected: {streaming:?}"
    );
    assert!(
        !streaming.contains('>'),
        "no blockquote gutter expected: {streaming:?}"
    );
    assert!(
        !streaming.contains("Thought for"),
        "no footer expected: {streaming:?}"
    );
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    assert!(
        streaming.contains(&format!("*{sentinel}Let me think.{sentinel}*")),
        "first line not dim+italic: {streaming:?}"
    );
    assert!(
        streaming.contains(&format!("*{sentinel}Second thought.{sentinel}*")),
        "second line not dim+italic: {streaming:?}"
    );

    // In `current` mode (the default), closing discards the block in place: it
    // leaves the live stream entirely and never becomes a persistent message.
    app.close_reasoning_region(None);
    assert!(
        app.streaming_text().is_empty(),
        "reasoning should leave the live stream once discarded: {:?}",
        app.streaming_text()
    );
    assert!(
        !app.display_messages.iter().any(|m| m.role == "reasoning"),
        "ephemeral reasoning must not create a persistent message"
    );
}

#[test]
fn reasoning_region_closes_before_normal_output() {
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("thinking about it\n");
    // Real output begins; region must close so output is not styled as reasoning.
    app.close_reasoning_region(None);
    app.append_streaming_text("Final answer.");

    // The answer stays in the live stream and must never be styled as reasoning.
    let text = app.streaming_text();
    assert!(
        text.contains("Final answer."),
        "answer present in stream: {text:?}"
    );
    let answer_line = text
        .lines()
        .find(|l| l.contains("Final answer."))
        .expect("answer line present");
    assert!(
        !answer_line.contains(jcode_tui_markdown::REASONING_SENTINEL),
        "final answer must not be styled as reasoning: {answer_line:?}"
    );
    // The reasoning was discarded; it is no longer in the stream and no persistent
    // reasoning message was created.
    assert!(
        !text.contains(jcode_tui_markdown::REASONING_SENTINEL),
        "reasoning must not remain in the answer stream: {text:?}"
    );
    assert!(
        !app.display_messages.iter().any(|m| m.role == "reasoning"),
        "ephemeral reasoning must not create a persistent message"
    );
}

#[test]
fn reasoning_region_open_is_idempotent() {
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("a\n");
    app.open_reasoning_region(); // no-op while open
    app.append_reasoning_text("b\n");

    let text = app.streaming_text();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    assert!(
        text.contains(&format!("*{sentinel}a{sentinel}*")),
        "first chunk: {text:?}"
    );
    assert!(
        text.contains(&format!("*{sentinel}b{sentinel}*")),
        "second chunk: {text:?}"
    );
    // No extra separator burst between the two chunks.
    assert!(
        !text.contains(&format!("*{sentinel}a{sentinel}*\n\n")),
        "second chunk should not restart the region: {text:?}"
    );
}

#[test]
fn reasoning_line_split_across_deltas_stays_one_run() {
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("one ");
    app.append_reasoning_text("two\n");

    // While streaming live, the split-across-deltas line is a single emphasis run.
    let content = app.streaming_text();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    assert!(
        content.contains(&format!("*{sentinel}one two{sentinel}*")),
        "split line must be one emphasis run: {content:?}"
    );
}

#[test]
fn reasoning_region_renders_dim_italic_text_without_gutter() {
    use ratatui::style::Modifier;

    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("considering options\n");

    // The live reasoning renders dim+italic from the streaming buffer.
    let reasoning_content = app.streaming_text().to_string();

    let lines = crate::tui::markdown::render_markdown_with_width(&reasoning_content, Some(80));
    let body = lines
        .iter()
        .find(|l| {
            l.spans
                .iter()
                .any(|s| s.content.as_ref().contains("considering options"))
        })
        .expect("reasoning body line present");

    let rendered: String = body.spans.iter().map(|s| s.content.as_ref()).collect();
    // No blockquote gutter, and the sentinel is stripped from the visible text.
    assert!(!rendered.contains('│'), "no gutter expected: {rendered:?}");
    assert!(
        !rendered.contains(jcode_tui_markdown::REASONING_SENTINEL),
        "sentinel must be stripped: {rendered:?}"
    );

    let body_span = body
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains("considering options"))
        .expect("body span present");
    assert!(
        body_span.style.add_modifier.contains(Modifier::ITALIC),
        "reasoning body should be italic: {:?}",
        body_span.style
    );
}

#[test]
fn strip_reasoning_lines_removes_reasoning_keeps_answer() {
    use crate::tui::app::input::strip_reasoning_lines;

    // Build content the way the streaming buffer would: reasoning lines wrapped
    // with the sentinel, then a normal answer paragraph.
    let mut content = String::new();
    content.push_str(&jcode_tui_markdown::reasoning_line_markup("thinking one"));
    content.push_str(&jcode_tui_markdown::reasoning_line_markup("thinking two"));
    content.push('\n');
    content.push_str("Here is the answer.\n");

    let stripped = strip_reasoning_lines(&content);
    assert_eq!(stripped, "Here is the answer.");
    assert!(!stripped.contains(jcode_tui_markdown::REASONING_SENTINEL));
}

#[test]
fn strip_reasoning_lines_reasoning_only_becomes_empty() {
    use crate::tui::app::input::strip_reasoning_lines;

    let mut content = String::new();
    content.push_str(&jcode_tui_markdown::reasoning_line_markup("only thinking"));
    let stripped = strip_reasoning_lines(&content);
    assert!(stripped.trim().is_empty(), "got: {stripped:?}");
}

#[test]
fn reasoning_partial_line_renders_live_before_newline() {
    // The in-progress line (no trailing newline) must render immediately as a
    // dim+italic partial tail so reasoning streams token-by-token.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("partial thou");

    let text = app.streaming_text();
    assert!(
        text.contains(&format!("*{sentinel}partial thou{sentinel}*")),
        "partial line should render live: {text:?}"
    );
}

#[test]
fn reasoning_partial_tail_grows_in_place_without_duplication() {
    // Successive deltas of the same line replace the live tail (truncate + rebuild)
    // rather than appending duplicate fragments.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("one ");
    app.append_reasoning_text("two ");
    app.append_reasoning_text("three");

    let text = app.streaming_text();
    assert!(
        text.contains(&format!("*{sentinel}one two three{sentinel}*")),
        "tail should grow in place: {text:?}"
    );
    // The earlier partial fragments must not linger as separate runs.
    assert!(
        !text.contains(&format!("*{sentinel}one {sentinel}*")),
        "stale partial tail should be replaced, not duplicated: {text:?}"
    );
    assert_eq!(
        text.matches(sentinel).count(),
        2,
        "exactly one live emphasis run (two sentinels) expected: {text:?}"
    );
}

#[test]
fn reasoning_partial_promotes_to_committed_line_on_newline() {
    // When the newline arrives, the live tail becomes a committed line and a fresh
    // (empty) tail follows; no duplicate copies of the completed line remain.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("growing line");
    app.append_reasoning_text("\nnext");

    let text = app.streaming_text();
    // Committed first line (hard-break terminated) and a live second-line tail.
    assert!(
        text.contains(&format!("*{sentinel}growing line{sentinel}*  \n")),
        "first line should be committed with a hard break: {text:?}"
    );
    assert!(
        text.contains(&format!("*{sentinel}next{sentinel}*")),
        "second line should render live: {text:?}"
    );
    // The completed line must appear exactly once (no partial+committed duplication).
    assert_eq!(
        text.matches(&format!("*{sentinel}growing line{sentinel}*"))
            .count(),
        1,
        "completed line must not be duplicated: {text:?}"
    );
}

#[test]
fn reasoning_close_promotes_pending_partial_line() {
    // Closing the region with an in-progress (no-newline) partial promotes it to a
    // committed line exactly once, then collapses into the reasoning message.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("final thought");
    app.close_reasoning_region(None);

    // The reasoning is discarded in place on close: it leaves the live stream and
    // never becomes a persistent message.
    let _ = sentinel;
    assert!(
        app.streaming_text().is_empty(),
        "reasoning should leave the live stream once discarded: {:?}",
        app.streaming_text()
    );
    assert!(
        !app.display_messages.iter().any(|m| m.role == "reasoning"),
        "ephemeral reasoning must not create a persistent message"
    );
}

#[test]
fn reasoning_preceded_by_answer_keeps_order_and_drops_reasoning() {
    // Answer text streamed *before* a reasoning block must stay in place and in
    // order; closing the reasoning removes only the reasoning, leaving the answer.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.append_streaming_text("Intro before thinking.");
    app.open_reasoning_region();
    app.append_reasoning_text("let me think\nstep two\n");
    app.close_reasoning_region(None);
    app.append_streaming_text("Conclusion after thinking.");

    let text = app.streaming_text();
    assert!(
        !text.contains(sentinel),
        "reasoning must be fully removed: {text:?}"
    );
    let intro = text.find("Intro before thinking.").expect("intro present");
    let concl = text
        .find("Conclusion after thinking.")
        .expect("conclusion present");
    assert!(
        intro < concl,
        "answer text must keep its original order: {text:?}"
    );
    assert!(
        !app.display_messages.iter().any(|m| m.role == "reasoning"),
        "ephemeral reasoning must not create a persistent message"
    );
}

#[test]
fn multiple_reasoning_blocks_do_not_accumulate() {
    // Each reasoning block is ephemeral: closing a second block (after a commit)
    // must not leave any reasoning message behind from the first or second block.
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("first block thinking\n");
    app.close_reasoning_region(None);
    app.append_streaming_text("Answer one.");
    app.commit_pending_streaming_assistant_message();

    app.open_reasoning_region();
    app.append_reasoning_text("second block thinking\n");
    app.close_reasoning_region(None);

    let reasoning_msgs = app
        .display_messages
        .iter()
        .filter(|m| m.role == "reasoning")
        .count();
    assert_eq!(
        reasoning_msgs, 0,
        "reasoning must never accumulate as persistent messages"
    );
    assert!(
        !app.streaming_text()
            .contains(jcode_tui_markdown::REASONING_SENTINEL),
        "no reasoning markup should linger in the stream: {:?}",
        app.streaming_text()
    );
}
