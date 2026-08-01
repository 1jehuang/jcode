//! The transcript: typed messages, rich text, and pixel-accurate geometry.
//!
//! The transcript used to be one flat `String` paginated by counting `'\n'`.
//! That was wrong in three separate ways at once, and all three were visible
//! on screen: a wrapped paragraph is taller than its newline count, so tall
//! content overflowed its region and drew straight through the composer;
//! scrolling moved by logical lines while the screen moves by pixels, so the
//! two disagreed as soon as anything wrapped; and a `String` has no structure,
//! so a user's message could only be distinguished from the model's reply by
//! prefixing it with a shell-style `>`.
//!
//! This module replaces all of that:
//!
//! - [`Message`] is the unit: who said it, and their markdown source.
//! - Markdown and LaTeX come from [`jcode_render_core`], the backend-neutral
//!   document model already shared with the TUI. Nothing about emphasis, code
//!   spans, lists, tables, or math is re-implemented here; this module only
//!   maps [`StyleRole`] onto the desktop theme and Parley font attributes.
//! - Geometry is measured, never estimated: every message is laid out by
//!   Parley and reports a real height in logical units, so scrolling is in
//!   pixels and the visible window is found by walking measured blocks.

use crate::text::{ParagraphStyle, TextSystem};
use crate::theme::Theme;
use jcode_render_core::{
    Alignment, Block, BlockKind, Document, FillRole, StyleRole, StyledLine, parse_markdown,
};
use parley::{Layout, StyleProperty};
use vello::peniko::{Brush, Color};

/// Who produced a message. The transcript's structure, and the thing that
/// replaces the `>` marker: roles are styled, not labelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// The model's reasoning. Shown, because a long silent think is
    /// indistinguishable from a stall, but visibly subordinate to the answer:
    /// muted ink behind a rule, never mistaken for the reply itself.
    Reasoning,
    /// A tool call the agent made, shown as its `intent`: one line of "what
    /// is being done". At most one exists, it is always the transcript's
    /// last message, and it clears when the turn ends: a live status line
    /// pinned to the bottom of the conversation, not a log of past calls.
    Tool,
    /// A file edit the agent made: the intent it wrote, the file it touched,
    /// and the added and removed lines. Unlike [`Role::Tool`] this *stays* in
    /// the transcript, because an edit is not transient status: it changed the
    /// user's files, and "what did it change" is the one thing a reader has to
    /// be able to scroll back to.
    Edit,
    /// Something went wrong: a turn that failed, a provider that could not be
    /// reached, a connection that dropped. Rendered in the conversation
    /// rather than only in the footnote, because the footnote is hidden once a
    /// session is attached, and a failure the user cannot see is
    /// indistinguishable from an app that silently ignored them.
    Notice,
    /// A background task the agent is waiting on: its label, its status line,
    /// and a drawn bar when it reports a percentage.
    ///
    /// Like [`Role::Tool`] this is live status rather than history, so it is
    /// pinned to the tail and retired when the task finishes. Unlike the tool
    /// card there can be several at once: a turn can be waiting on a build, a
    /// test sweep, and a swarm plan at the same time, and collapsing them into
    /// one line would hide the one that is stuck.
    Progress,
}

/// One turn of the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    /// Markdown source. Kept raw so a streaming message can be re-parsed as it
    /// grows, and so copy yields what the model actually wrote.
    pub source: String,
    /// The tool call this message reports, when `role` is [`Role::Tool`].
    /// Kept so a streamed `intent` can replace the tool's bare name in place
    /// rather than appending a second line for the same call.
    pub call_id: Option<String>,
    /// How far this message has got towards the agent, when `role` is
    /// [`Role::User`]. `None` for everything the app did not send: a reply,
    /// a thought, or a user message replayed from history is not in flight,
    /// and marking it "sent" would be a claim about a past this app never saw.
    pub delivery: Option<crate::ack::Delivery>,
    /// Completion of a [`Role::Progress`] card, in per mille (0..=1000).
    /// `None` means the task is running but cannot say how far along it is,
    /// which is drawn as an indeterminate track rather than a bar stuck at
    /// zero.
    ///
    /// An integer rather than a float so a message stays `Eq` (the transcript
    /// is compared wholesale in tests and in the paint cache) and so two ticks
    /// reporting the same progress are byte-identical.
    pub permille: Option<u16>,
}

impl Message {
    /// A user message replayed from history or built in a test: on the page,
    /// but not in flight, so it carries no delivery mark.
    pub fn user(source: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            source: source.into(),
            call_id: None,
            delivery: None,
            permille: None,
        }
    }

    /// A user message this app has just handed to the connection.
    pub fn sent(source: impl Into<String>) -> Self {
        Self {
            delivery: Some(crate::ack::Delivery::Sent),
            ..Self::user(source)
        }
    }

    /// A user message typed while the agent was mid-turn. The daemon refuses
    /// a second message outright, so this one waits in the transcript's tail
    /// until the turn ends and [`Transcript::promote_oldest_queued`] sends it.
    pub fn queued(source: impl Into<String>) -> Self {
        Self {
            delivery: Some(crate::ack::Delivery::Queued),
            ..Self::user(source)
        }
    }

    pub fn assistant(source: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            source: source.into(),
            call_id: None,
            delivery: None,
            permille: None,
        }
    }

    pub fn reasoning(source: impl Into<String>) -> Self {
        Self {
            role: Role::Reasoning,
            source: source.into(),
            call_id: None,
            delivery: None,
            permille: None,
        }
    }

    pub fn tool(call_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            source: label.into(),
            call_id: Some(call_id.into()),
            delivery: None,
            permille: None,
        }
    }

    /// An edit card, rendered from a finished edit tool call.
    pub fn edit(card: &crate::edits::EditCard) -> Self {
        Self {
            role: Role::Edit,
            source: edit_source(card),
            call_id: None,
            delivery: None,
            permille: None,
        }
    }

    /// A background task's live progress card. `task_id` rides in `call_id`
    /// for the same reason a tool call's does: it is the key that lets the
    /// next tick refine this card in place instead of stacking a new row per
    /// update.
    pub fn progress(
        task_id: impl Into<String>,
        label: &str,
        summary: &str,
        percent: Option<f32>,
    ) -> Self {
        Self {
            role: Role::Progress,
            source: progress_source(label, summary),
            call_id: Some(task_id.into()),
            delivery: None,
            permille: percent.map(|percent| (percent.clamp(0.0, 100.0) * 10.0).round() as u16),
        }
    }

    /// Completion as a 0..=1 fraction, for drawing. `None` for an
    /// indeterminate task, and for every role that is not a progress card.
    pub fn fraction(&self) -> Option<f64> {
        self.permille
            .map(|permille| f64::from(permille.min(1000)) / 1000.0)
    }

    /// A failure, placed in the conversation where the user is already looking.
    pub fn notice(source: impl Into<String>) -> Self {
        Self {
            role: Role::Notice,
            source: source.into(),
            call_id: None,
            delivery: None,
            permille: None,
        }
    }
}

/// Markdown for an edit card: what changed and why, then the diff itself.
///
/// Built as markdown rather than as a bespoke block type so the card goes
/// through exactly the same parse, layout, selection, and copy path as
/// everything else in the transcript; the only special-casing downstream is the
/// per-line ink the diff body gets.
fn edit_source(card: &crate::edits::EditCard) -> String {
    let mut source = String::new();
    if let Some(intent) = card
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|intent| !intent.is_empty())
    {
        source.push_str(intent);
        // A blank line, or markdown folds the intent and the file line into one
        // paragraph and the sentence runs straight into the path.
        source.push_str("\n\n");
    }
    let files = card
        .files
        .iter()
        .map(|file| format!("`{file}`"))
        .collect::<Vec<_>>()
        .join(" ");
    // Counts, always: a truncated diff still has to say how big the change was.
    source.push_str(&format!("{files} +{} -{}\n", card.added, card.removed));
    // The fence carries the file's language, so the diff body is highlighted
    // as the code it is rather than as plain text. It travels in the markdown
    // because that is where every other code block's language comes from, and
    // one path through layout is what keeps the two from drifting.
    source.push_str("```");
    source.push_str(card.language().unwrap_or(""));
    source.push('\n');
    source.push_str(&diff_body(card));
    source.push_str("```\n");
    source
}

/// The diff body of an edit card: line numbers right-aligned into a gutter,
/// then the sign, then the line.
///
/// Alignment is the whole point. Tools emit `9- x` and `118- y`, so the code
/// starts at a different column on almost every row and the change you are
/// trying to compare is never above the thing it replaced. Padding the numbers
/// to one width puts the two sides in the same column, which is what makes a
/// diff readable at a glance.
///
/// Long diffs are truncated: a card is a summary of a change in a scrolling
/// conversation, and a thousand-line rewrite pasted into it buries every other
/// turn. The count in the header still reports the whole change, and the
/// marker says how much is not shown.
fn diff_body(card: &crate::edits::EditCard) -> String {
    let rows = card.rows();
    let width = rows
        .iter()
        .filter_map(|row| row.number)
        .map(|number| number.to_string().len())
        .max()
        .unwrap_or(0);
    let mut body = String::new();
    for row in rows.iter().take(DIFF_ROW_LIMIT) {
        let number = row
            .number
            .map(|number| number.to_string())
            .unwrap_or_default();
        let sign = match row.change {
            crate::edits::Change::Added => '+',
            crate::edits::Change::Removed => '-',
        };
        body.push_str(&format!("{number:>width$}{sign} {}\n", row.text));
    }
    if let Some(hidden) = rows.len().checked_sub(DIFF_ROW_LIMIT).filter(|n| *n > 0) {
        body.push_str(&format!(
            "… {hidden} more line{}\n",
            if hidden == 1 { "" } else { "s" }
        ));
    }
    body
}

/// Rows of a diff shown in a card before it is truncated.
pub const DIFF_ROW_LIMIT: usize = 24;

/// How much of the usual inter-block gap an edit card's parts take. The card
/// is one statement, so its lines are set as one paragraph rather than as
/// three.
const EDIT_GAP_SCALE: f64 = 0.45;

/// Markdown for a progress card: the task's label, then its status line.
///
/// One line, not two: the card is a status readout pinned to the tail, and a
/// wrapped paragraph per background task would push the conversation off the
/// page whenever a build got chatty.
fn progress_source(label: &str, summary: &str) -> String {
    let label = label.trim();
    let summary = summary.trim();
    match (label.is_empty(), summary.is_empty()) {
        (true, true) => "background task".to_string(),
        (true, false) => summary.to_string(),
        (false, true) => label.to_string(),
        (false, false) => format!("{label} · {summary}"),
    }
}

/// The conversation. Streaming appends to the trailing assistant message
/// rather than pushing a new one per delta, so a reply is one block however
/// many chunks it arrived in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcript {
    messages: Vec<Message>,
    /// How much thinking this transcript keeps. `Full` is the structural
    /// default so a transcript built in a test or from replayed history is
    /// exactly the messages it was given; the app sets the user's mode from
    /// [`crate::reasoning::ReasoningMode::from_env`] at startup, whose own
    /// default is `Current`.
    reasoning: crate::reasoning::ReasoningMode,
}

impl Default for Transcript {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            reasoning: crate::reasoning::ReasoningMode::Full,
        }
    }
}

impl Transcript {
    /// Choose what happens to streamed reasoning. See
    /// [`crate::reasoning::ReasoningMode`].
    pub fn set_reasoning_mode(&mut self, mode: crate::reasoning::ReasoningMode) {
        self.reasoning = mode;
    }

    pub fn reasoning_mode(&self) -> crate::reasoning::ReasoningMode {
        self.reasoning
    }

    pub fn is_empty(&self) -> bool {
        self.messages
            .iter()
            .all(|message| message.source.trim().is_empty())
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Mark the oldest still-pending user message as acknowledged.
    ///
    /// Oldest first because the queue is a queue: acks arrive in the order the
    /// messages were sent, so the first unacked card is the one this ack
    /// belongs to. Returns whether anything changed, so the caller can skip a
    /// redraw for an ack that answers a message this window did not send (a
    /// second client on the same session, or a replayed history). A `Queued`
    /// message is not a candidate: it has not been handed to the connection,
    /// so no ack can be about it.
    pub fn acknowledge_oldest_pending(&mut self, at: std::time::Instant) -> bool {
        let pending = self
            .messages
            .iter_mut()
            .find(|message| message.delivery == Some(crate::ack::Delivery::Sent));
        match pending {
            Some(message) => {
                message.delivery = Some(crate::ack::Delivery::Acked { at });
                true
            }
            None => false,
        }
    }

    /// Promote the oldest queued message to `Sent` and return its text for
    /// the connection, or `None` when nothing is waiting.
    ///
    /// One at a time, because the daemon takes one message per turn: sending
    /// the whole queue at a turn boundary would have every message past the
    /// first refused for the same reason it was queued. The promoted message
    /// keeps its place on the page; later streamed text lands *after* it (see
    /// [`Self::text_tail`]), which is the order the agent will actually read
    /// things in.
    pub fn promote_oldest_queued(&mut self) -> Option<String> {
        let message = self
            .messages
            .iter_mut()
            .find(|message| message.delivery == Some(crate::ack::Delivery::Queued))?;
        message.delivery = Some(crate::ack::Delivery::Sent);
        Some(message.source.clone())
    }

    /// Whether any message is still waiting for its turn to be sent.
    pub fn has_queued(&self) -> bool {
        self.messages
            .iter()
            .any(|message| message.delivery == Some(crate::ack::Delivery::Queued))
    }

    /// Every delivery mark currently on the page, for animation scheduling.
    pub fn deliveries(&self) -> impl Iterator<Item = crate::ack::Delivery> + '_ {
        self.messages.iter().filter_map(|message| message.delivery)
    }

    /// Append streamed assistant text, continuing the current reply when there
    /// is one. Without this a reply would be split into one block per network
    /// chunk, and markdown spanning a chunk boundary would never parse.
    ///
    /// Text lands *above* a live tool card: the card is pinned to the tail,
    /// so the reply grows over it rather than pushing it up mid-transcript.
    pub fn append_assistant(&mut self, text: &str) {
        // The answer supersedes the thinking that produced it: in `current`
        // mode the live thought leaves the page here, which is what keeps a
        // long turn from reading as a wall of superseded reasoning.
        self.retire_live_reasoning();
        let at = self.text_tail();
        match at.checked_sub(1).map(|index| &mut self.messages[index]) {
            Some(last) if last.role == Role::Assistant => last.source.push_str(text),
            _ => self.messages.insert(at, Message::assistant(text)),
        }
    }

    /// Append streamed reasoning, continuing the current thought. Reasoning
    /// arrives in the same delta-sized chunks as the reply, so it coalesces
    /// the same way; a new assistant message ends the thought, which is what
    /// makes "thought, then answered" read in order.
    ///
    /// In `off` mode nothing lands (the delta still proved the model is alive,
    /// which is the status line's job). In `current` mode only the paragraph
    /// being written right now is kept: a blank line means the model moved on,
    /// and the finished paragraph is dropped rather than accumulated.
    pub fn append_reasoning(&mut self, text: &str) {
        use crate::reasoning::ReasoningMode;
        if self.reasoning == ReasoningMode::Off {
            return;
        }
        let at = self.text_tail();
        match at.checked_sub(1).map(|index| &mut self.messages[index]) {
            Some(last) if last.role == Role::Reasoning => last.source.push_str(text),
            _ => self.messages.insert(at, Message::reasoning(text)),
        }
        if self.reasoning == ReasoningMode::Current {
            self.trim_live_reasoning_to_current_paragraph();
        }
    }

    /// Keep only the paragraph the model is writing now, in `current` mode.
    ///
    /// Split on a blank line, which is how prose marks "that point is
    /// finished". A chunk can carry several breaks at once, so the *last* one
    /// wins rather than the first.
    fn trim_live_reasoning_to_current_paragraph(&mut self) {
        let Some(live) = self.live_reasoning_index() else {
            return;
        };
        let message = &mut self.messages[live];
        if let Some(break_at) = message.source.rfind("\n\n") {
            let tail = message.source[break_at + 2..].trim_start().to_string();
            message.source = tail;
        }
    }

    /// The live thought's index: the trailing reasoning message, looking past
    /// the live tool card and any queued messages pinned to the tail.
    fn live_reasoning_index(&self) -> Option<usize> {
        let index = self.text_tail().checked_sub(1)?;
        (self.messages[index].role == Role::Reasoning).then_some(index)
    }

    /// Drop the live thought once it has been superseded, in `current` mode.
    ///
    /// A committed answer or an opening tool call is the proof the model
    /// stopped thinking and started doing; in `full` mode the thought stays as
    /// history instead.
    fn retire_live_reasoning(&mut self) {
        if self.reasoning != crate::reasoning::ReasoningMode::Current {
            return;
        }
        if let Some(live) = self.live_reasoning_index() {
            self.messages.remove(live);
        }
    }

    /// Start of the trailing run of queued messages.
    ///
    /// Queued messages live at the very bottom of the transcript: they are
    /// the *future* of the conversation, typed while the current turn was
    /// still producing its past, so everything the turn streams has to land
    /// above them. A queue that is not trailing cannot happen through the
    /// public methods: queued messages are only ever pushed at the end, and
    /// promotion keeps them in place while later text is inserted above the
    /// ones still waiting.
    fn queued_tail_start(&self) -> usize {
        let mut index = self.messages.len();
        while index > 0 && self.messages[index - 1].delivery == Some(crate::ack::Delivery::Queued) {
            index -= 1;
        }
        index
    }

    /// Start of the trailing run of background-progress cards, which sit just
    /// above the queue and just below the live tool card.
    ///
    /// The tail band reads top to bottom as "what is being done now" (the tool
    /// card), "what is being waited on" (the progress cards), "what happens
    /// next" (the queue), and every append path routes through these three
    /// functions so that order cannot come apart.
    fn progress_tail_start(&self) -> usize {
        let mut index = self.queued_tail_start();
        while index > 0 && self.messages[index - 1].role == Role::Progress {
            index -= 1;
        }
        index
    }

    /// Where streamed text goes: the end of the transcript, except that a live
    /// tool card and the queued messages at the tail are skipped over. One
    /// definition, so every append path keeps the card and the queue pinned
    /// and none can strand them mid-transcript.
    fn text_tail(&self) -> usize {
        let tail = self.progress_tail_start();
        match tail.checked_sub(1).map(|index| &self.messages[index]) {
            Some(last) if last.role == Role::Tool => tail - 1,
            _ => tail,
        }
    }

    /// Show the current tool call as the single live tool card.
    ///
    /// The transcript holds at most one tool message: the call running right
    /// now. A call announces itself by name the moment it opens, its streamed
    /// arguments usually carry a better line (the `intent`), and the next
    /// call takes the card over entirely, so a turn with fifty calls reads as
    /// one live line of "what is being done" instead of fifty rows of
    /// history. The reply itself is what records the turn's work.
    ///
    /// The card is always the last message: streamed text is inserted above
    /// it (see [`Self::text_tail`]), so it never drifts up mid-transcript and
    /// never has to jump back down when the next call opens.
    pub fn set_live_tool(&mut self, call_id: &str, label: &str) {
        let label = label.trim();
        if label.is_empty() {
            return;
        }
        // A call opening is the thought ending: the model is doing, not
        // thinking. (No-op outside `current` mode.)
        self.retire_live_reasoning();
        // Refine the card in place, whichever call it belongs to now: the
        // card is a slot, not a log entry. `text_tail` points *at* the card
        // when one is pinned under the streamed text and above the queue.
        let slot = self.text_tail();
        if let Some(card) = self.messages.get_mut(slot)
            && card.role == Role::Tool
        {
            card.call_id = Some(call_id.to_string());
            card.source = label.to_string();
            return;
        }
        // No card yet: open one under the text, above any queued messages.
        // The clear is a guard against a stray card left by a replayed
        // history, which must not double up.
        self.clear_live_tool();
        let at = self.text_tail();
        self.messages.insert(at, Message::tool(call_id, label));
    }

    /// Remove the live tool card. Called when the turn ends: a card left
    /// behind would claim work is still happening after it stopped.
    pub fn clear_live_tool(&mut self) {
        self.messages.retain(|message| message.role != Role::Tool);
    }

    /// Show, or refine, a background task's progress card.
    ///
    /// Keyed by task id, so a task that ticks a hundred times is one card that
    /// updates rather than a hundred rows. Cards live in the same pinned band
    /// as the live tool card, below the streamed text and above the queue, so
    /// a growing reply never strands a bar mid-transcript.
    pub fn set_progress(
        &mut self,
        task_id: &str,
        label: &str,
        summary: &str,
        percent: Option<f32>,
    ) {
        let updated = Message::progress(task_id, label, summary, percent);
        if let Some(card) = self.messages.iter_mut().find(|message| {
            message.role == Role::Progress && message.call_id.as_deref() == Some(task_id)
        }) {
            card.source = updated.source;
            card.permille = updated.permille;
            return;
        }
        // At the *end* of the progress band, so the cards read in the order
        // their tasks started rather than newest-first: a bar that jumps above
        // the ones already on screen makes a second task look like a restart.
        let at = self.queued_tail_start();
        self.messages.insert(at, updated);
    }

    /// Retire one task's progress card, because the task finished.
    ///
    /// Returns whether a card was actually removed, so the caller can skip a
    /// redraw for a completion notice about a task this window never saw start.
    pub fn clear_progress(&mut self, task_id: &str) -> bool {
        let before = self.messages.len();
        self.messages.retain(|message| {
            message.role != Role::Progress || message.call_id.as_deref() != Some(task_id)
        });
        self.messages.len() != before
    }

    /// Retire every bar. The conversation on screen is being replaced (a
    /// session switch), so bars belonging to the session being left must not
    /// be shown against the one being opened.
    pub fn clear_all_progress(&mut self) {
        self.messages
            .retain(|message| message.role != Role::Progress);
    }

    /// Whether any background task's bar is on the page. Drives the one clock
    /// the bars animate off, so a window with nothing running still sleeps.
    pub fn has_progress(&self) -> bool {
        self.messages
            .iter()
            .any(|message| message.role == Role::Progress)
    }

    /// Whether any bar on the page is indeterminate, and so needs frames to
    /// keep its segment sweeping. A page of determinate bars is a still image
    /// between ticks and must not pull the loop awake.
    pub fn has_indeterminate_progress(&self) -> bool {
        self.messages
            .iter()
            .any(|message| message.role == Role::Progress && message.permille.is_none())
    }

    /// Record a completed edit in the conversation.
    ///
    /// Placed above the live tool card, like streamed text: the card is pinned
    /// to the tail, and the edit belongs to the history that has already
    /// happened. It is never cleared, because it is the record of a change to
    /// the user's files.
    pub fn push_edit(&mut self, card: &crate::edits::EditCard) {
        let at = self.text_tail();
        self.messages.insert(at, Message::edit(card));
    }

    /// Record a failure in the conversation.
    ///
    /// Repeats are collapsed: a provider that is unreachable typically fails
    /// once per retry, and stacking twenty identical "no network" lines would
    /// bury the conversation under the same sentence. The live tool card goes
    /// with it, because a call that errored is not still running.
    pub fn push_notice(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.clear_live_tool();
        // Above the live status band: a failure is part of the turn that just
        // happened, while progress cards and the queue are about what is still
        // to come.
        let at = self.progress_tail_start();
        if at
            .checked_sub(1)
            .map(|index| &self.messages[index])
            .is_some_and(|last| last.role == Role::Notice && last.source == text)
        {
            return;
        }
        self.messages.insert(at, Message::notice(text));
    }

    /// Plain-text rendering, for tests and for copying the conversation.
    pub fn plain_text(&self) -> String {
        self.messages
            .iter()
            .map(|message| message.source.trim())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Characters in the trailing assistant reply, which is the message the
    /// streaming reveal is animating. Zero when the last turn is the user's,
    /// because nothing is arriving.
    ///
    /// A live tool card and queued messages at the tail are skipped: both are
    /// pinned under the streamed text and appear whole, so the reveal
    /// animates the text arriving above them (the same message `text_tail`
    /// puts that text in, or the two would disagree).
    pub fn streaming_len(&self) -> usize {
        match self
            .text_tail()
            .checked_sub(1)
            .map(|index| &self.messages[index])
        {
            // A notice is a status line, not prose arriving: it appears whole,
            // so it must not put the reveal back into a streaming state (which
            // would leave the failure fading in with nothing behind it).
            Some(last) if !matches!(last.role, Role::User | Role::Notice) => {
                last.source.chars().count()
            }
            _ => 0,
        }
    }
}

impl From<&[Message]> for Transcript {
    fn from(messages: &[Message]) -> Self {
        Self {
            messages: messages.to_vec(),
            ..Self::default()
        }
    }
}

/// A message laid out at a known width: its Parley layouts and their heights.
///
/// A message is several layouts rather than one, because block kinds do not
/// share a paragraph style: a code block is drawn on a wash at a different
/// inset from body copy, and a heading is a different size. Each entry
/// therefore carries its own offset within the message.
pub struct LaidMessage {
    pub role: Role,
    /// The delivery mark to draw beside this message, when it is one the app
    /// sent. Not part of the layout cache's key: an ack changes a dot and an
    /// offset, never a wrap, so re-laying the message for it would be work for
    /// nothing (see [`crate::paint::TranscriptCache`], which refreshes this
    /// field on every frame instead).
    pub delivery: Option<crate::ack::Delivery>,
    /// Completion of the progress card this message is, in per mille. Refreshed
    /// on a cache hit like `delivery`: a bar advancing changes a drawn width,
    /// never a wrap.
    pub permille: Option<u16>,
    /// Laid-out blocks, in order, with their vertical offset from the top of
    /// the message and the kind that produced them.
    pub blocks: Vec<LaidBlock>,
    /// Total height of the message in logical units, including inter-block
    /// spacing but not the gap to the next message.
    pub height: f64,
}

impl LaidMessage {
    /// Vertical inset from the top of the message to its first block. A user
    /// message is drawn in a padded card, and the live tool card pads the
    /// same way; an assistant reply is not. Shared by drawing and hit-testing
    /// so a click cannot land a padding's worth away from the glyph it aimed
    /// at.
    pub fn top_padding(&self) -> f64 {
        match self.role {
            Role::User => USER_PAD_Y,
            // A notice is a card too: it has to be visibly an interjection
            // from the app rather than a line the model wrote.
            Role::Tool | Role::Notice | Role::Progress => TOOL_PAD_Y,
            // An edit card is a card too: the diff sits on a wash that must
            // not crop the first line of it.
            Role::Edit => TOOL_PAD_Y,
            Role::Assistant | Role::Reasoning => 0.0,
        }
    }

    /// Completion as a 0..=1 fraction, or `None` for an indeterminate task.
    pub fn fraction(&self) -> Option<f64> {
        self.permille
            .map(|permille| f64::from(permille.min(1000)) / 1000.0)
    }
}

pub struct LaidBlock {
    pub layout: Layout<Brush>,
    /// The block's flattened plain text, the same string the layout was built
    /// from. Kept so a pointer selection can slice it for the clipboard
    /// without re-parsing the markdown or reading back from the GPU.
    pub source: String,
    /// Offset from the top of the message, in logical units.
    pub top: f64,
    pub height: f64,
    /// Horizontal inset from the message's text left edge. Code blocks and
    /// quotes indent; body copy does not.
    pub inset: f64,
    pub kind: BlockKind,
    /// Glyphs in `layout`, counted once at layout time. The streaming reveal
    /// needs per-block glyph totals every frame, and walking every line of
    /// every run to recount them made the count itself scale with the
    /// reply, which is per-frame work on text that has not changed.
    pub glyphs: usize,
    /// Wash rectangles for inline code spans, in logical units relative to the
    /// block's text origin. Computed once at layout time from the same Parley
    /// selection geometry the highlight bands use, because a per-frame
    /// re-measure of every `` `span` `` in a reply is work that grows with the
    /// reply while the text has not changed.
    pub washes: Vec<vello::kurbo::Rect>,
    /// Row bands for a diff body, in logical units relative to the block's
    /// text origin. Empty for every block that is not an edit card's diff.
    /// Computed at layout time for the same reason the washes are: the rows do
    /// not move once laid out.
    pub diff_bands: Vec<DiffBand>,
}

impl LaidBlock {
    /// Where this block's furniture (a code wash, a quote rule) begins,
    /// relative to the message's text left edge.
    ///
    /// Inside any list indent the block inherited, outside the block's own
    /// padding: the wash wraps the text, and both have to agree on where the
    /// block starts.
    pub fn edge(&self) -> f64 {
        (self.inset - own_pad(&self.kind)).max(0.0)
    }
}

/// Vertical rhythm inside and between messages, in logical units.
pub const BLOCK_GAP: f64 = 8.0;
pub const MESSAGE_GAP: f64 = 18.0;
/// Inset of a user message's text inside its tinted card.
pub const USER_PAD_X: f64 = 12.0;
pub const USER_PAD_Y: f64 = 8.0;
/// Corner radius of the user card. Matches the composer, so your message and
/// the box you typed it in are visibly the same object.
pub const USER_RADIUS: f64 = 6.0;
/// Inset of code-block text inside its wash.
pub const CODE_PAD_X: f64 = 10.0;
pub const CODE_PAD_Y: f64 = 6.0;
/// Indent applied to quoted text, leaving room for the quote rule.
pub const QUOTE_INSET: f64 = 12.0;
/// Indent of a display equation. Render-core lays display math out as aligned
/// rows of glyphs (a fraction's bar sits over its denominator), so the block
/// cannot be centred line by line without pulling those rows out of alignment.
/// It is set off as an indented figure instead, which is the print convention
/// and keeps the layout exactly as the math renderer measured it.
pub const MATH_INSET: f64 = 16.0;
/// Leading of a display-math block, as a multiple of the font size. Tight,
/// because the rows of a rendered equation are parts of one picture rather than
/// successive lines of prose.
pub const MATH_LINE_HEIGHT: f32 = 1.15;
/// Reasoning is not indented: it sits on the reply's measure and is set apart
/// by ink alone (dimmer, slightly smaller). A rule or an indent reads as
/// structural furniture; a thought only needs to be quieter.
pub const REASONING_INSET: f64 = 0.0;
/// Indent of the tool card's text, leaving room for the activity spinner
/// that shows the call running. Wider than the reasoning rule because the
/// spinner is a drawn object, not a hairline.
pub const TOOL_INSET: f64 = 24.0;
/// Indent of a failure notice's text, leaving room for the rule down its left
/// edge. The rule is what tells the notice apart from the reply above it
/// without spending a colour the print theme does not have.
pub const NOTICE_INSET: f64 = 16.0;
/// Vertical padding inside the live tool card.
pub const TOOL_PAD_Y: f64 = 6.0;
/// The progress bar's track: thin, because it is a readout rather than a
/// control, and the same halftone language as the rest of the app means it
/// only has to be legible, not loud.
pub const PROGRESS_BAR_HEIGHT: f64 = 3.0;
/// Gap between a progress card's label and its bar.
pub const PROGRESS_BAR_GAP: f64 = 5.0;
/// Corner radius of the bar. Half its height, so the track reads as a capsule
/// rather than as a sliver of a rectangle.
pub const PROGRESS_BAR_RADIUS: f64 = PROGRESS_BAR_HEIGHT / 2.0;
/// Width of the moving segment of an indeterminate bar, as a fraction of the
/// track. A task that cannot report a percentage still has to look alive, so
/// the segment sweeps instead of the fill growing.
pub const PROGRESS_SWEEP_FRACTION: f64 = 0.3;
/// One full sweep of an indeterminate bar.
pub const PROGRESS_SWEEP_PERIOD: std::time::Duration = std::time::Duration::from_millis(1400);
/// Reasoning is set smaller than body copy, as a multiple of it. Enough to
/// read as an aside at a glance without becoming unreadable.
pub const REASONING_SCALE: f32 = 0.92;
/// Extra space above a heading, beyond [`BLOCK_GAP`]. A heading belongs to the
/// text under it, so leading it more than it trails is what makes a reply
/// scan as sections rather than as an undifferentiated column.
pub const HEADING_LEAD: f64 = 6.0;
/// Space between consecutive items of one list. Much tighter than
/// [`BLOCK_GAP`], because render-core emits one block per item and a paragraph
/// gap between them turns a list into five separate statements.
pub const LIST_ITEM_GAP: f64 = 2.0;
/// Indent per nesting level of a list. Render-core already prefixes nested
/// items with two spaces, but in a proportional-agnostic layout an indent that
/// the wrap width also honours is what keeps a wrapped continuation line from
/// sliding back under the bullet.
pub const LIST_INDENT: f64 = 16.0;
/// Height of a thematic break's own block. The rule is drawn by the scene, so
/// the block carries no text; it only has to reserve the air the rule sits in.
pub const RULE_HEIGHT: f64 = 13.0;
/// Horizontal padding of the wash behind an inline code span, and the corner
/// radius of that wash. Small: it must read as a tint on the word rather than
/// as a box around it.
pub const INLINE_CODE_PAD_X: f64 = 2.5;
pub const INLINE_CODE_RADIUS: f64 = 3.0;
/// Vertical padding of the inline-code wash, as a fraction of the line box.
/// A wash filling the whole line box would touch the lines above and below and
/// read as a table cell, so it is inset to hug the glyphs.
const INLINE_CODE_TIGHTEN: f64 = 0.16;

/// Resolve a render-core [`StyleRole`] to a concrete theme colour. This is the
/// whole of the desktop's "theme adapter": the neutral document says *what* a
/// span means, and only this function says what colour that is.
pub fn role_color(role: StyleRole, theme: &Theme) -> Color {
    match role {
        StyleRole::Text => theme.text,
        StyleRole::Dim => theme.muted,
        StyleRole::Strong => theme.text,
        StyleRole::Code => theme.text,
        // A link keeps body ink and is marked by its underline instead. The
        // print theme has no accent hue to spend, and a muted link would read
        // as less important than the sentence it sits in.
        StyleRole::Link => theme.text,
        StyleRole::Html => theme.muted,
        StyleRole::Reasoning => theme.muted,
        StyleRole::Math => theme.text,
    }
}

/// Whether a role is marked with an underline. Only links: the underline is
/// the one typographic convention for "this goes somewhere", and it costs no
/// colour, which matters in an ink-on-paper theme.
fn role_is_underlined(role: StyleRole) -> bool {
    matches!(role, StyleRole::Link)
}

/// Whether a role implies a monospace-emphasis treatment. The whole app is
/// already a mono stack, so code is distinguished by its wash and colour
/// rather than by a family switch.
fn role_is_strong(role: StyleRole) -> bool {
    matches!(role, StyleRole::Strong)
}

/// Lay out one message to `width` logical units.
///
/// Every block is measured by Parley here, so the caller receives real
/// heights. Nothing downstream is allowed to estimate.
///
/// Production always goes through [`lay_out_message_reusing`] via the paint
/// cache, so this one-shot form exists for tests that want a message laid out
/// with no prior state.
#[cfg_attr(not(test), allow(dead_code))]
pub fn lay_out_message(
    text: &mut TextSystem,
    message: &Message,
    width: f64,
    theme: &Theme,
    base: ParagraphStyle,
    scale: f64,
) -> LaidMessage {
    lay_out_message_reusing(text, message, Vec::new(), width, theme, base, scale).0
}

/// As [`lay_out_message`], reusing block layouts from a previous laying of
/// the *same message* where their content is unchanged.
///
/// This is what makes streaming affordable. A delta appends characters to the
/// tail message, which re-parses into the same blocks as before plus a changed
/// (or new) block at the end. Re-laying all of them made a delta's cost grow
/// with the reply's length: by the end of a long answer every token burst was
/// paying for the whole message again, which is exactly the "streaming gets
/// choppier as it writes" lag. Matching the parsed blocks against `previous`
/// in order, and stopping at the first mismatch, keeps a delta's layout work
/// proportional to what actually changed while never reusing a block whose
/// text, kind, or inset differs.
///
/// Returns the laid message and how many blocks were laid fresh (the rest
/// were reused), so the cache can meter streaming work exactly.
pub fn lay_out_message_reusing(
    text: &mut TextSystem,
    message: &Message,
    previous: Vec<LaidBlock>,
    width: f64,
    theme: &Theme,
    base: ParagraphStyle,
    scale: f64,
) -> (LaidMessage, usize) {
    // Markdown and math both come from render-core, so the desktop and the TUI
    // agree on what a document *is*; only the drawing differs.
    let document: Document = parse_markdown(&message.source);
    // Reasoning is the same document machinery in a subordinate voice: muted,
    // slightly smaller, and indented behind a rule the scene draws.
    // The tool card is that voice again, indented further to leave room for
    // the spinner: it reports what the agent is doing, not what it said, so
    // it must never be mistaken for the reply.
    let subdued = match message.role {
        // A thought is the quietest voice in the transcript, so it takes the
        // faintest ink; the tool card stays merely muted because it labels
        // live work.
        Role::Reasoning => Some(theme.faint),
        // A progress card is the tool card's voice: live status about work in
        // flight, not something the model said.
        Role::Tool | Role::Progress => Some(theme.muted),
        _ => None,
    };
    // A failure is the one thing in the transcript that must not be quiet, so
    // it takes the error ink at full body size instead of the subdued voice.
    let notice = matches!(message.role, Role::Notice).then_some(theme.error);
    let (base, role_inset) = match subdued {
        Some(color) => (
            ParagraphStyle {
                font_size: base.font_size * REASONING_SCALE,
                line_height: base.line_height,
                color,
                ..base
            },
            match message.role {
                // Both leave room for a drawn object down the left edge: the
                // spinner for a tool call, the bar's track for a progress card.
                Role::Tool | Role::Progress => TOOL_INSET,
                _ => REASONING_INSET,
            },
        ),
        None => match notice {
            Some(color) => (ParagraphStyle { color, ..base }, NOTICE_INSET),
            // An edit card sits at the margin. It used to be indented to clear
            // a rule down its left edge; the rule is gone (the diff's own wash
            // and hue already mark it), so the indent would only narrow the
            // one block on the page that most wants the measure.
            None if message.role == Role::Edit => (base, 0.0),
            None => (base, 0.0),
        },
    };
    let tint = subdued.or(notice);
    let mut blocks = Vec::new();
    let mut top = 0.0;
    let mut fresh = 0usize;

    // Blocks from the previous laying, consumed front to back. Only an
    // unbroken prefix is reused: a delta appends at the end, so everything
    // before the first difference is byte-identical, and stopping at the
    // first mismatch means an edit that *shifts* blocks can never pair a
    // block with another block's layout.
    let mut previous = previous.into_iter().peekable();
    let mut matching = true;
    // Kind of the block laid immediately before this one, so the gap between
    // them can depend on the pair rather than on one of them alone.
    let mut previous_kind: Option<BlockKind> = None;
    // Width of one monospace cell, measured lazily (see below).
    let mut advance: Option<f64> = None;

    for block in &document.blocks {
        let inset = block_inset(block) + role_inset;
        let measure = (width - inset * 2.0).max(1.0);
        // Measure available in *characters*: the app is a monospace stack, so a
        // table's columns are laid out in cells, and the cell width has to come
        // from the font rather than from a guess. Measured at most once per
        // message, and only when a table asks: it costs a Parley layout, and a
        // streaming delta re-flattens every block of the tail message.
        let advance = &mut advance;
        let lines = block_lines(block, || {
            let cell = *advance.get_or_insert_with(|| text.measure_width("0", base, scale));
            if cell <= 0.0 {
                DEFAULT_TABLE_COLUMNS
            } else {
                ((measure / cell).floor() as usize).max(MIN_TABLE_COLUMNS)
            }
        });
        if lines.is_empty() {
            continue;
        }
        // A thematic break is drawn as a real rule by the scene, so its block
        // carries no text at all. Laying out render-core's `───` placeholder
        // as well would draw the rule twice, once in glyphs and once in ink.
        let is_rule = block.kind == BlockKind::ThematicBreak;
        let (source, spans) = if is_rule {
            (String::new(), Vec::new())
        } else {
            flatten(&lines)
        };
        // An edit card's code block is a diff, so each of its lines takes the
        // ink of the side it is on. Applied here, over the flattened block,
        // because "which side" is a property of the whole line and markdown has
        // no span for it.
        let is_diff =
            message.role == Role::Edit && matches!(block.kind, BlockKind::CodeBlock { .. });
        let spans = if is_diff {
            let language = match &block.kind {
                BlockKind::CodeBlock { language } => language.as_deref(),
                _ => None,
            };
            diff_spans(&source, language, theme)
        } else {
            spans
        };
        // An ordinary fenced block with a language is highlighted the same
        // way. A code block is quoted *because* it is code, and flat ink
        // throws away the one property that distinguishes it from the prose
        // around it.
        let spans = match &block.kind {
            BlockKind::CodeBlock {
                language: Some(language),
            } if !is_diff => code_spans(&source, language, theme),
            _ => spans,
        };
        // The card's header line: the counts take the diff's own ink, so
        // "how much changed, and in which direction" is answered by the same
        // two colours the body uses rather than by reading two numbers.
        let spans = if message.role == Role::Edit && block.kind == BlockKind::Paragraph {
            count_spans(&source, theme, spans)
        } else {
            spans
        };
        if !is_rule && source.trim().is_empty() {
            continue;
        }
        // Space above this block. It depends on *both* neighbours, so it is
        // applied here rather than trailing each block: successive list items
        // want to sit close enough to read as one list, while a heading wants
        // air above it, and a single uniform gap cannot do both.
        if let Some(previous_kind) = previous_kind.as_ref() {
            // An edit card's three parts (why, where, what) are one statement
            // about one change, so they are set tight. At the paragraph gap
            // the card came apart into three unrelated lines and the diff
            // stopped looking like it belonged to the file named above it.
            let gap = gap_between(previous_kind, &block.kind);
            top += if message.role == Role::Edit {
                gap * EDIT_GAP_SCALE
            } else {
                gap
            };
        }
        if matching {
            let reusable = previous.peek().is_some_and(|cached| {
                cached.kind == block.kind && cached.source == source && cached.inset == inset
            });
            if reusable {
                let mut cached = previous.next().expect("peeked");
                cached.top = top;
                top += cached.height;
                previous_kind = Some(cached.kind.clone());
                blocks.push(cached);
                continue;
            }
            matching = false;
        }
        let style = block_style(&block.kind, base, theme);
        let layout = layout_rich(
            text,
            &source,
            &spans,
            (width - inset * 2.0).max(1.0),
            style,
            Palette { theme, tint },
            scale,
        );
        fresh += 1;
        let mut height = if is_rule {
            RULE_HEIGHT
        } else {
            f64::from(layout.height()) / scale
        };
        if matches!(block.kind, BlockKind::CodeBlock { .. }) {
            height += CODE_PAD_Y * 2.0;
        }
        // A code *block* already sits on its own wash, so only spans inside
        // prose need one of their own.
        let washes = if matches!(block.kind, BlockKind::CodeBlock { .. }) {
            Vec::new()
        } else {
            inline_code_washes(&layout, &spans, scale)
        };
        // Row bands, for a diff body only: every other block is prose, and a
        // band across a paragraph would read as a table row.
        let bands = if is_diff {
            diff_bands(&layout, &source, scale)
        } else {
            Vec::new()
        };
        blocks.push(LaidBlock {
            glyphs: crate::text::glyph_count(&layout),
            layout,
            source,
            top,
            height,
            inset,
            kind: block.kind.clone(),
            washes,
            diff_bands: bands,
        });
        top += height;
        previous_kind = Some(block.kind.clone());
    }

    let mut height = top.max(0.0);
    height += match message.role {
        // The user card and the tool card both reserve their padding, so the
        // tint can never crop the text it wraps.
        Role::User => USER_PAD_Y * 2.0,
        Role::Tool | Role::Notice | Role::Edit => TOOL_PAD_Y * 2.0,
        // The bar is drawn under the label inside the same card, so the card
        // reserves its height here: measuring it anywhere else would let the
        // bar paint over the message below.
        Role::Progress => TOOL_PAD_Y * 2.0 + PROGRESS_BAR_GAP + PROGRESS_BAR_HEIGHT,
        Role::Assistant | Role::Reasoning => 0.0,
    };
    (
        LaidMessage {
            role: message.role,
            delivery: message.delivery,
            permille: message.permille,
            blocks,
            height,
        },
        fresh,
    )
}

/// Per-line ink for a diff body.
///
/// Three things are being said at once, and each gets its own ink so none of
/// them shouts over the others:
///
/// * the gutter (line number and sign) is *furniture*, so it takes the diff
///   colour at full strength: it is the fastest way to answer "which side",
///   and it is the one part of the row that is not code;
/// * the code itself is syntax-highlighted, so a diff still reads as the
///   language it is written in rather than as two blocks of flat colour;
/// * that highlighting is then pulled a little toward the diff colour, so the
///   side a line is on survives even where the syntax is loud.
///
/// A line the highlighter cannot colour (unknown language, prose, the
/// truncation marker) falls back to the diff ink alone, which is exactly what
/// this used to do for every line.
///
/// Byte ranges are over the flattened block, so a wrapped long line keeps its
/// ink across the wrap: the colour belongs to the text, not to the screen row.
fn diff_spans(source: &str, language: Option<&str>, theme: &Theme) -> Vec<SpanStyle> {
    let dark = theme.mode == crate::theme::ThemeMode::Dark;
    let mut spans = Vec::new();
    let mut at = 0usize;
    let ink = |color: Color, range: std::ops::Range<usize>| SpanStyle {
        range,
        role: StyleRole::Code,
        fill: FillRole::None,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        color: Some(color),
    };
    for line in source.split_inclusive('\n') {
        let start = at;
        let text = line.trim_end_matches('\n');
        at += line.len();
        let side = match crate::edits::classify(line) {
            Some(crate::edits::Change::Added) => theme.added,
            Some(crate::edits::Change::Removed) => theme.removed,
            // The truncation marker is not a change, so it takes the quiet
            // voice the rest of the app uses for things it is telling you
            // about itself.
            None => {
                spans.push(ink(theme.muted, start..start + text.len()));
                continue;
            }
        };
        // Past the gutter: the digits and the sign, which are drawn in the
        // diff colour, then one space, then the code.
        let gutter = gutter_len(text);
        spans.push(ink(side, start..start + gutter));
        let code_at = (gutter + 1).min(text.len());
        let code = &text[code_at..];
        let runs = crate::syntax::highlight_line(code, language, dark);
        if runs.is_empty() {
            spans.push(ink(side, start + code_at..start + text.len()));
            continue;
        }
        // Any gap between runs (whitespace the highlighter dropped) keeps the
        // diff ink, so a line is never partly uncoloured.
        let mut cursor = 0usize;
        for (range, color) in runs {
            if range.start > cursor {
                spans.push(ink(
                    side,
                    start + code_at + cursor..start + code_at + range.start,
                ));
            }
            cursor = range.end;
            spans.push(ink(
                crate::syntax::tint(color, side, DIFF_SYNTAX_TINT),
                start + code_at + range.start..start + code_at + range.end,
            ));
        }
        if code_at + cursor < text.len() {
            spans.push(ink(side, start + code_at + cursor..start + text.len()));
        }
    }
    spans
}

/// Bytes of a diff line taken by its gutter: the line number and the sign.
/// Per-token ink for an ordinary fenced code block.
/// Ink for the `+n -m` counts at the end of an edit card's header.
///
/// Only the two tokens are recoloured, and only when they are the last thing
/// on the line: an intent that happens to contain `+2` is prose, and painting
/// it green would be a lie about what changed.
fn count_spans(source: &str, theme: &Theme, mut spans: Vec<SpanStyle>) -> Vec<SpanStyle> {
    let mut tail = source.len();
    for (ink, sign) in [(theme.added, '+'), (theme.removed, '-')].into_iter().rev() {
        let head = source[..tail].trim_end();
        let Some(at) = head.rfind(sign) else { break };
        // Everything after the sign must be the digits of a count.
        if head[at + 1..].is_empty() || !head[at + 1..].bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        spans.push(SpanStyle {
            range: at..head.len(),
            role: StyleRole::Text,
            fill: FillRole::None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            color: Some(ink),
        });
        tail = at;
    }
    spans
}

/// Per-token ink for an ordinary fenced code block.
///
/// Only tokens the highlighter recognised get a span; everything else is left
/// to the block's own colour, so an unknown language renders exactly as it did
/// before this existed rather than as a wall of one accent.
fn code_spans(source: &str, language: &str, theme: &Theme) -> Vec<SpanStyle> {
    let dark = theme.mode == crate::theme::ThemeMode::Dark;
    let mut spans = Vec::new();
    let mut at = 0usize;
    for line in source.split_inclusive('\n') {
        let start = at;
        let text = line.trim_end_matches('\n');
        at += line.len();
        for (range, color) in crate::syntax::highlight_line(text, Some(language), dark) {
            spans.push(SpanStyle {
                range: start + range.start..start + range.end,
                role: StyleRole::Code,
                fill: FillRole::None,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                color: Some(color),
            });
        }
    }
    spans
}

/// Bytes of a diff line taken by its gutter: the line number and the sign.
fn gutter_len(line: &str) -> usize {
    let lead = line.len() - line.trim_start().len();
    let digits = line[lead..].len().saturating_sub(
        line[lead..]
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len(),
    );
    // The sign is one byte past the digits, when there is one at all.
    (lead + digits + 1).min(line.len())
}

/// How far a syntax colour is pulled toward the diff's own ink. Enough that a
/// line still reads as added or removed from the corner of the eye, little
/// enough that a keyword and a string stay distinguishable.
const DIFF_SYNTAX_TINT: f64 = 0.35;

/// A coloured area inside a diff body: either a whole row, or the part of a
/// row that actually differs from its counterpart on the other side.
#[derive(Clone, Copy, Debug)]
pub struct DiffBand {
    /// In logical units relative to the block's text origin. `x` is only
    /// meaningful for an emphasis band; a row band is drawn to the card's own
    /// edges, because a band as ragged as the code stops being a shape.
    pub rect: vello::kurbo::Rect,
    pub change: crate::edits::Change,
    /// Whether this is the *changed part* of the row rather than the row.
    /// Drawn stronger, and over the row band: on a one-word change in a long
    /// line, the row colour says a line changed and this says which word.
    pub emphasis: bool,
}

/// Coloured areas for a laid-out diff body: one band per screen row, plus a
/// stronger band over the part of each paired row that actually differs.
///
/// Derived from the same Parley geometry the selection bands use, so a band
/// cannot drift from the text it marks, and a line long enough to wrap gets
/// one band per screen row instead of one box around both.
fn diff_bands(layout: &Layout<Brush>, source: &str, scale: f64) -> Vec<DiffBand> {
    let mut bands = Vec::new();
    // Each line's byte range in the flattened block, and which side it is on.
    let mut lines = Vec::new();
    let mut at = 0usize;
    for line in source.split_inclusive('\n') {
        let start = at;
        let end = at + line.trim_end_matches('\n').len();
        at += line.len();
        if let Some(change) = crate::edits::classify(line)
            && start < end
        {
            lines.push((start, end, change));
        }
    }
    for &(start, end, change) in &lines {
        for band in crate::select::layout_bands(layout, (start, end), scale) {
            bands.push(DiffBand {
                rect: band.rect,
                change,
                emphasis: false,
            });
        }
    }
    // Pair each removed line with the added line under it. That pairing is
    // what an edit tool emits (`42- old` then `42+ new`), and it is the only
    // case where "what changed within the line" is a question with an answer.
    for pair in lines.windows(2) {
        let [
            (old_start, old_end, old_change),
            (new_start, new_end, new_change),
        ] = pair
        else {
            continue;
        };
        use crate::edits::Change::{Added, Removed};
        if *old_change != Removed || *new_change != Added {
            continue;
        }
        let old = &source[*old_start..*old_end];
        let new = &source[*new_start..*new_end];
        // Past the gutter on both sides: the line numbers differ as a matter
        // of course, and diffing them would mark every row as changed.
        let old_code = gutter_len(old).min(old.len());
        let new_code = gutter_len(new).min(new.len());
        let Some((old_range, new_range)) = changed_span(&old[old_code..], &new[new_code..]) else {
            continue;
        };
        for (base, offset, range, change) in [
            (*old_start, old_code, old_range, Removed),
            (*new_start, new_code, new_range, Added),
        ] {
            for band in crate::select::layout_bands(
                layout,
                (base + offset + range.start, base + offset + range.end),
                scale,
            ) {
                bands.push(DiffBand {
                    rect: band.rect,
                    change,
                    emphasis: true,
                });
            }
        }
    }
    bands
}

/// The part of each of two lines that differs, as byte ranges into each.
///
/// The common prefix and suffix are peeled off and what is left is the change.
/// That is not a minimal edit script, and deliberately so: a real word-diff of
/// a renamed variable marks every occurrence separately, which is *more*
/// marks than the line has meaning, while prefix/suffix peeling marks the one
/// contiguous region the eye should go to. `None` when the whole line changed
/// (nothing to narrow) or nothing did.
fn changed_span(old: &str, new: &str) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    if old == new {
        return None;
    }
    let mut prefix = 0;
    while prefix < old.len()
        && prefix < new.len()
        && old.as_bytes()[prefix] == new.as_bytes()[prefix]
    {
        prefix += 1;
    }
    while prefix > 0 && (!old.is_char_boundary(prefix) || !new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old.as_bytes()[old.len() - 1 - suffix] == new.as_bytes()[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }
    let old_range = prefix..old.len() - suffix;
    let new_range = prefix..new.len() - suffix;
    // A change that covers essentially the whole line is not worth narrowing:
    // the row band already says so, and a second band over the same pixels is
    // just a darker row.
    let shared = prefix + suffix;
    let shortest = old.len().min(new.len());
    if shared * 100 < shortest * MIN_SHARED_PERCENT {
        return None;
    }
    Some((old_range, new_range))
}

/// How much of the shorter line must be unchanged before the changed part is
/// worth marking on its own.
const MIN_SHARED_PERCENT: usize = 15;

/// Horizontal inset for a block, relative to the message's text column.
///
/// Two indents compose here. The block's *own* kind indents it (code sits in
/// from its wash, a quote clears its rule), and the list it is written inside
/// indents it again: a fenced block or a table under item 2 belongs to item 2,
/// and drawing it at the margin breaks the list open around it.
fn block_inset(block: &Block) -> f64 {
    let own = own_pad(&block.kind);
    // A list item's own depth already places it, so only *other* blocks take
    // the enclosing list's indent, and they hang under the item's text rather
    // than under its bullet.
    let nested = match block.kind {
        // A loose item's *second* paragraph is emitted as another list-item
        // block with no marker of its own. It is continuation prose, so it
        // hangs under the item's text; left at the margin it read as a new
        // paragraph that had escaped the list.
        BlockKind::ListItem { depth, .. } if !starts_with_marker(block) => {
            (depth + 1) as f64 * LIST_INDENT
        }
        BlockKind::ListItem { .. } => 0.0,
        _ => block.list_depth as f64 * LIST_INDENT,
    };
    own + nested
}

/// Whether a list-item block carries a marker (`• `, `1. `) of its own, as
/// opposed to being a continuation paragraph of the item above it. Render-core
/// emits the marker as a leading dim span, so its absence is what distinguishes
/// the two.
fn starts_with_marker(block: &Block) -> bool {
    let Some(first) = block.lines.first().and_then(|line| line.spans.first()) else {
        return false;
    };
    if first.role != StyleRole::Dim {
        return false;
    }
    let text = first.text.trim_start();
    text.starts_with('\u{2022}')
        || text.split_once(". ").is_some_and(|(digits, _)| {
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        })
}

/// The part of a block's inset that is the block's *own* padding, as opposed to
/// the indent it inherits from an enclosing list.
///
/// The scene needs the two apart: a code block's wash and a quote's rule sit at
/// the block's *edge*, which is inside the list indent but outside the block's
/// own padding. Deriving the edge as `inset - own_pad` keeps the furniture and
/// the text from disagreeing about where the block is, which is what left a
/// nested code block's wash back at the margin while its text was indented.
pub fn own_pad(kind: &BlockKind) -> f64 {
    match kind {
        BlockKind::CodeBlock { .. } => CODE_PAD_X,
        BlockKind::BlockQuote => QUOTE_INSET,
        BlockKind::MathDisplay => MATH_INSET,
        // Nested items step in, so depth is visible as position rather than
        // only as leading spaces inside the text.
        BlockKind::ListItem { depth, .. } => *depth as f64 * LIST_INDENT,
        _ => 0.0,
    }
}

/// Vertical space between two adjacent blocks, in logical units.
///
/// Vertical rhythm is most of the difference between a reply that scans and one
/// that reads as a wall, and it is a property of the *pair*, not of either
/// block alone. Render-core emits one block per list item, so a uniform gap
/// scattered a five-item list down the page as five paragraphs; a heading, on
/// the other hand, belongs to the text under it, so it needs more air above
/// than below or a section groups with the one before it.
fn gap_between(above: &BlockKind, below: &BlockKind) -> f64 {
    use BlockKind::{Heading, ListItem, MathDisplay};
    match (above, below) {
        // Items of the *same* list. Leading alone already separates them, so
        // the gap only has to keep them from touching. A bullet list followed
        // immediately by a numbered one is two lists, not one, and gets the
        // paragraph gap so the reader sees the boundary.
        (
            ListItem {
                ordered: above_ordered,
                ..
            },
            ListItem {
                ordered: below_ordered,
                ..
            },
        ) if above_ordered == below_ordered => LIST_ITEM_GAP,
        // A heading is a label for what follows, so it sits close to it.
        (Heading { .. }, _) => BLOCK_GAP,
        // A heading or an equation is introduced, so it is led generously.
        (_, Heading { .. } | MathDisplay) => BLOCK_GAP + HEADING_LEAD,
        // An equation is a figure: it needs air on the way out as well.
        (MathDisplay, _) => BLOCK_GAP + HEADING_LEAD,
        _ => BLOCK_GAP,
    }
}

/// Paragraph style for a block kind. Headings step up in size; everything else
/// inherits the body style, because a chat transcript is body copy with
/// occasional structure, not a document.
fn block_style(kind: &BlockKind, base: ParagraphStyle, theme: &Theme) -> ParagraphStyle {
    match kind {
        BlockKind::Heading { level } => ParagraphStyle {
            font_size: base.font_size * heading_scale(*level),
            bold: true,
            color: theme.text,
            ..base
        },
        BlockKind::CodeBlock { .. } => ParagraphStyle {
            color: theme.text,
            ..base
        },
        BlockKind::BlockQuote => ParagraphStyle {
            color: theme.muted,
            ..base
        },
        // Display math arrives as rows that are *parts of one glyph picture*: a
        // fraction's bar belongs between its numerator and denominator. Body
        // leading is set for reading successive lines of prose, and at that
        // spacing a fraction comes apart into three unrelated lines, so math
        // is set tight enough for the rows to read as one expression.
        BlockKind::MathDisplay => ParagraphStyle {
            line_height: MATH_LINE_HEIGHT,
            color: theme.text,
            ..base
        },
        _ => base,
    }
}

/// Heading sizes, as multiples of body copy. Big enough that a heading reads
/// as a heading at a glance, but an h1 in a chat reply is still a sentence,
/// not a cover page.
fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.55,
        2 => 1.35,
        3 => 1.18,
        _ => 1.05,
    }
}

/// The styled lines a block contributes.
///
/// Three kinds need front-end treatment. Tables are left to the front-end by
/// render-core because column widths depend on the measure, so `columns` (the
/// measure in monospace cells, computed lazily because only a table needs it)
/// is what bounds them. Quotes arrive with a terminal `│ ` bar on every line,
/// which the desktop replaces with a real drawn rule. Task-list items arrive as
/// literal `[x]` text, which is the terminal's checkbox and reads as source
/// here.
fn block_lines(block: &Block, columns: impl FnOnce() -> usize) -> Vec<StyledLine> {
    if block.kind == BlockKind::Table && block.lines.is_empty() {
        return table_lines(&block.table, &block.alignments, columns());
    }
    if block.kind == BlockKind::BlockQuote {
        return block
            .lines
            .iter()
            .map(|line| StyledLine {
                spans: strip_quote_bar(&line.spans),
                alignment: line.alignment,
            })
            .collect();
    }
    // Render-core indents a nested list item with leading spaces, which is the
    // only tool a terminal has. The desktop indents the whole block instead
    // (see `block_inset`), so keeping the spaces as well would double the
    // indent and, worse, leave a wrapped continuation line sitting under the
    // bullet rather than under the text.
    if let BlockKind::ListItem { depth, .. } = block.kind {
        return block
            .lines
            .iter()
            .map(|line| StyledLine {
                spans: checkbox_spans(&if depth > 0 {
                    strip_leading_indent(&line.spans)
                } else {
                    line.spans.clone()
                }),
                alignment: line.alignment,
            })
            .collect();
    }
    block.lines.clone()
}

/// Replace a task list's literal `[ ] ` / `[x] ` marker with a drawn checkbox.
///
/// The terminal has no glyph for a checkbox that is not text, so render-core
/// emits the source form. On screen that reads as unrendered markdown sitting
/// next to a bullet that *was* rendered, which is exactly the inconsistency
/// this transcript exists to avoid.
fn checkbox_spans(spans: &[jcode_render_core::StyledSpan]) -> Vec<jcode_render_core::StyledSpan> {
    let mut spans = spans.to_vec();
    // The marker is its own span, emitted right after the bullet, so only the
    // first two spans can carry it.
    for span in spans.iter_mut().take(2) {
        match span.text.as_str() {
            "[ ] " => span.text = "\u{2610} ".to_string(),
            "[x] " => span.text = "\u{2611} ".to_string(),
            _ => continue,
        }
        break;
    }
    spans
}

/// Lay a GFM table out as aligned columns bounded by `columns` monospace cells.
///
/// The app is a monospace stack throughout, so padding each cell to its
/// column's width produces true columns. Three things make this more than a
/// `join`. Columns are *budgeted*: a table wider than the measure had its right
/// edge run off the page, so wide columns are squeezed and their cells wrap
/// inside the column rather than overflowing it. Cells honour the delimiter
/// row's alignment, because a right-aligned numeric column that renders
/// left-aligned misreads the author. And a separator row is emitted under the
/// header, so a table reads as a table rather than as bold text above some
/// rows.
fn table_lines(rows: &[Vec<String>], alignments: &[Alignment], columns: usize) -> Vec<StyledLine> {
    use jcode_render_core::{StyledSpan, TextAttrs};

    if rows.is_empty() {
        return Vec::new();
    }
    let count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if count == 0 {
        return Vec::new();
    }
    let widths = column_widths(rows, count, columns);

    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // Wrap every cell to its column, then emit as many physical lines as
        // the tallest cell needs. Truncating instead would hide content, and a
        // table is usually the densest thing in a reply.
        let wrapped: Vec<Vec<String>> = (0..count)
            .map(|column| {
                let cell = row.get(column).map(String::as_str).unwrap_or("");
                wrap_cell(cell, widths[column])
            })
            .collect();
        let physical = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for line in 0..physical {
            let mut text = String::new();
            for (column, width) in widths.iter().enumerate() {
                let cell = wrapped[column].get(line).map(String::as_str).unwrap_or("");
                text.push_str(&pad_cell(cell, *width, alignment_of(alignments, column)));
                if column + 1 < count {
                    text.push_str(COLUMN_GAP);
                }
            }
            let mut span = StyledSpan::plain(text.trim_end().to_string());
            // The header is the one piece of table structure worth carrying in
            // the text: it says which way to read the rest.
            if index == 0 {
                span = span.with_attrs(TextAttrs {
                    bold: true,
                    ..TextAttrs::none()
                });
            }
            lines.push(StyledLine::from_spans(vec![span]));
        }
        if index == 0 {
            lines.push(StyledLine::from_spans(vec![StyledSpan::new(
                header_rule(&widths, count),
                StyleRole::Dim,
            )]));
        }
    }
    lines
}

/// Gap between two table columns, in monospace cells.
const COLUMN_GAP: &str = "  ";

/// The rule under a table's header row, drawn in the text so it wraps and
/// scrolls with the columns it belongs to.
fn header_rule(widths: &[usize], count: usize) -> String {
    let mut rule = String::new();
    for (column, width) in widths.iter().enumerate() {
        rule.push_str(&"\u{2500}".repeat(*width));
        if column + 1 < count {
            rule.push_str(&"\u{2500}".repeat(COLUMN_GAP.len()));
        }
    }
    rule
}

/// Column widths for a table: each column's widest cell, squeezed to fit the
/// measure when the natural widths overflow it.
///
/// Squeezing takes from the widest column first, so a table of one long prose
/// column and three short ones narrows the prose rather than shredding the
/// short ones into one character each.
fn column_widths(rows: &[Vec<String>], count: usize, columns: usize) -> Vec<usize> {
    use unicode_width::UnicodeWidthStr;

    let mut widths: Vec<usize> = (0..count)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.width())
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();

    let gaps = count.saturating_sub(1) * COLUMN_GAP.len();
    let available = columns.saturating_sub(gaps);
    if available == 0 {
        return widths;
    }
    // Never squeeze below this: a column narrower than a short word wraps into
    // a vertical stack of letters, which is less readable than a table that is
    // merely tight.
    let floor = (available / count).clamp(1, MIN_COLUMN_WIDTH);
    while widths.iter().sum::<usize>() > available {
        let Some((widest, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > floor)
            .max_by_key(|(index, width)| (**width, std::cmp::Reverse(*index)))
        else {
            break;
        };
        widths[widest] -= 1;
    }
    widths
}

/// Measure to fall back to when the font reports no advance at all, and the
/// narrowest measure a table is laid out against. A table squeezed below this
/// is unreadable either way, so it is allowed to be the one thing that
/// overflows rather than being shredded into single letters.
const DEFAULT_TABLE_COLUMNS: usize = 80;
const MIN_TABLE_COLUMNS: usize = 16;

/// Narrowest a squeezed table column may become, in monospace cells.
const MIN_COLUMN_WIDTH: usize = 6;

/// Alignment of a column, defaulting to left when the delimiter row said
/// nothing (or when a row has more cells than the header declared).
fn alignment_of(alignments: &[Alignment], column: usize) -> Alignment {
    alignments.get(column).copied().unwrap_or(Alignment::Left)
}

/// Pad `cell` to `width` cells according to `alignment`.
fn pad_cell(cell: &str, width: usize, alignment: Alignment) -> String {
    use unicode_width::UnicodeWidthStr;

    let pad = width.saturating_sub(cell.width());
    match alignment {
        Alignment::Left => format!("{cell}{}", " ".repeat(pad)),
        Alignment::Right => format!("{}{cell}", " ".repeat(pad)),
        Alignment::Center => {
            let left = pad / 2;
            format!("{}{cell}{}", " ".repeat(left), " ".repeat(pad - left))
        }
    }
}

/// Wrap one cell to `width` cells, breaking at spaces where possible and
/// mid-word only for a word that cannot fit at all.
fn wrap_cell(cell: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;

    if width == 0 {
        return vec![String::new()];
    }
    if cell.width() <= width {
        return vec![cell.to_string()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in cell.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if candidate.width() <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        // A single word wider than the column: hard-break it, because leaving
        // it whole would push the table past the measure it was budgeted for.
        let mut rest = word;
        while rest.width() > width {
            let split = rest
                .char_indices()
                .take_while(|(index, _)| rest[..*index].width() < width)
                .last()
                .map(|(index, _)| index)
                .unwrap_or(rest.len());
            let split = split.max(rest.chars().next().map_or(1, char::len_utf8));
            lines.push(rest[..split].to_string());
            rest = &rest[split..];
        }
        line = rest.to_string();
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Drop the leading run of spaces from a line's first span.
fn strip_leading_indent(
    spans: &[jcode_render_core::StyledSpan],
) -> Vec<jcode_render_core::StyledSpan> {
    let mut spans = spans.to_vec();
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start_matches(' ').to_string();
        if first.text.is_empty() && spans.len() > 1 {
            spans.remove(0);
        }
    }
    spans
}

/// Drop the *outermost* terminal quote bar from a quoted line.
///
/// The desktop draws the outer quote as a real rule down the block's left edge,
/// so keeping its `│` as well would mark the quote twice. Bars for deeper
/// nesting are kept: the block carries only one rule, so a quote inside a quote
/// would otherwise render identically to the quote around it, and "who is being
/// quoted here" is the whole content of that distinction.
fn strip_quote_bar(spans: &[jcode_render_core::StyledSpan]) -> Vec<jcode_render_core::StyledSpan> {
    let mut spans = spans.to_vec();
    if spans
        .first()
        .is_some_and(|first| first.text.starts_with('\u{2502}'))
    {
        let first = &mut spans[0];
        // Exactly one bar: render-core emits the whole gutter (`│ │ `) as a
        // single span, and trimming all of them would flatten a nested quote
        // onto the outer one.
        let rest = first.text.strip_prefix('\u{2502}').unwrap_or(&first.text);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        first.text = rest.to_string();
        if first.text.is_empty() && spans.len() > 1 {
            spans.remove(0);
        }
    }
    spans
}

/// A span's byte range within the flattened source, plus its styling.
pub struct SpanStyle {
    pub range: std::ops::Range<usize>,
    pub role: StyleRole,
    /// Background fill role. [`FillRole::Code`] is what marks an inline code
    /// span, and [`inline_code_washes`] turns it into the rectangles the scene
    /// fills behind the glyphs.
    pub fill: FillRole,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// Explicit ink, overriding both the span's role colour and the message's
    /// tint. Only diff lines use it: an added line is green and a removed one
    /// red regardless of the role the markdown gave the text.
    pub color: Option<Color>,
}

/// Flatten styled lines into one string plus byte-ranged styling. Parley wants
/// a single string with ranged properties, which is exactly the shape the
/// neutral model already has.
pub fn flatten(lines: &[StyledLine]) -> (String, Vec<SpanStyle>) {
    let mut source = String::new();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            source.push('\n');
        }
        for span in &line.spans {
            let start = source.len();
            source.push_str(&span.text);
            spans.push(SpanStyle {
                range: start..source.len(),
                role: span.role,
                fill: span.fill,
                bold: span.attrs.bold || role_is_strong(span.role),
                italic: span.attrs.italic,
                underline: span.attrs.underline || role_is_underlined(span.role),
                strikethrough: span.attrs.strikethrough,
                color: None,
            });
        }
    }
    (source, spans)
}

/// Lay out rich text: one Parley layout carrying per-span colour and weight.
///
/// This is the reason emphasis, code, links, and math can look different from
/// body copy without the transcript having to draw them as separate
/// paragraphs, which would break wrapping across a style boundary.
/// How a block's spans get their ink: the theme they resolve against, and an
/// optional override. Bundled rather than passed as two arguments because they
/// are one decision, and a caller that had the theme but forgot the tint would
/// silently draw reasoning in body colour.
#[derive(Clone, Copy)]
pub struct Palette<'a> {
    pub theme: &'a Theme,
    /// Overrides every span's role colour when set. Reasoning uses this so a
    /// `**bold**` word inside a thought stays in the aside's muted ink instead
    /// of jumping to full-strength body colour.
    pub tint: Option<Color>,
}

impl Palette<'_> {
    fn color(&self, role: StyleRole) -> Color {
        self.tint.unwrap_or_else(|| role_color(role, self.theme))
    }
}

pub fn layout_rich(
    text: &mut TextSystem,
    source: &str,
    spans: &[SpanStyle],
    width: f64,
    style: ParagraphStyle,
    palette: Palette<'_>,
    scale: f64,
) -> Layout<Brush> {
    text.layout_rich(source, width as f32, style, scale, &mut |builder| {
        for span in spans {
            if span.range.is_empty() {
                continue;
            }
            let color = span.color.unwrap_or_else(|| palette.color(span.role));
            builder.push(
                StyleProperty::Brush(Brush::Solid(color)),
                span.range.clone(),
            );
            if span.bold {
                builder.push(
                    StyleProperty::FontWeight(parley::FontWeight::BOLD),
                    span.range.clone(),
                );
            }
            if span.italic {
                builder.push(
                    StyleProperty::FontStyle(parley::FontStyle::Italic),
                    span.range.clone(),
                );
            }
            if span.underline {
                builder.push(StyleProperty::Underline(true), span.range.clone());
            }
            if span.strikethrough {
                builder.push(StyleProperty::Strikethrough(true), span.range.clone());
            }
        }
    })
}

/// Wash rectangles for the inline-code spans of a laid-out block, in logical
/// units relative to the block's text origin.
///
/// Inline code used to be invisible: the neutral model marked the span with
/// [`FillRole::Code`] but nothing drew that fill, so `` `--flag` `` read
/// exactly like the prose around it and the one thing a code span is *for*,
/// saying "this is literal", was lost. The rectangles come from Parley's own
/// selection geometry, the same source the highlight bands use, so a wash
/// cannot drift from the glyphs it is behind, and a span that wraps across a
/// line yields one rectangle per line rather than one box around both.
///
/// Computed once per layout rather than per frame: a reply's code spans do not
/// move once laid out, and re-deriving them every frame would make drawing
/// cost grow with the reply.
fn inline_code_washes(
    layout: &Layout<Brush>,
    spans: &[SpanStyle],
    scale: f64,
) -> Vec<vello::kurbo::Rect> {
    let mut washes = Vec::new();
    for span in spans {
        if span.fill != FillRole::Code || span.range.is_empty() {
            continue;
        }
        for band in crate::select::layout_bands(layout, (span.range.start, span.range.end), scale) {
            // Hug the glyphs rather than filling the line box: a wash spanning
            // the full leading touches its neighbours and reads as a table row.
            let tighten = band.rect.height() * INLINE_CODE_TIGHTEN;
            washes.push(vello::kurbo::Rect::new(
                band.rect.x0 - INLINE_CODE_PAD_X,
                band.rect.y0 + tighten,
                band.rect.x1 + INLINE_CODE_PAD_X,
                band.rect.y1 - tighten,
            ));
        }
    }
    washes
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{block_lines, table_lines};

    fn theme() -> Theme {
        Theme::print_light()
    }

    fn base() -> ParagraphStyle {
        ParagraphStyle {
            font_size: crate::layout::BODY_SIZE,
            line_height: crate::layout::BODY_LEADING as f32,
            ..Default::default()
        }
    }

    fn laid(source: &str) -> LaidMessage {
        let mut text = TextSystem::default();
        lay_out_message(
            &mut text,
            &Message::assistant(source),
            600.0,
            &theme(),
            base(),
            1.75,
        )
    }

    /// A streamed delta re-lays only what changed: the unchanged prefix of
    /// the message keeps its block layouts. This is the property that keeps a
    /// delta's cost flat as the reply grows instead of scaling with it.
    #[test]
    fn a_delta_reuses_the_unchanged_block_prefix() {
        let mut text = TextSystem::default();
        let before = "first paragraph.\n\nsecond paragraph.\n\nthird paragr";
        let after = "first paragraph.\n\nsecond paragraph.\n\nthird paragraph grew.";
        let first = lay_out_message(
            &mut text,
            &Message::assistant(before),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        let (second, fresh) = lay_out_message_reusing(
            &mut text,
            &Message::assistant(after),
            first.blocks,
            600.0,
            &theme(),
            base(),
            1.75,
        );
        assert_eq!(second.blocks.len(), 3);
        assert_eq!(fresh, 1, "a tail-only delta re-laid more than the tail");
    }

    /// Reuse must stop at the first difference: a block whose text changed,
    /// even mid-message, is re-laid along with everything after it, so an
    /// edit can never wear another block's layout.
    #[test]
    fn a_mid_message_change_invalidates_from_that_block_on() {
        let mut text = TextSystem::default();
        let first = lay_out_message(
            &mut text,
            &Message::assistant("alpha.\n\nbravo.\n\ncharlie."),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        let (second, fresh) = lay_out_message_reusing(
            &mut text,
            &Message::assistant("alpha.\n\nCHANGED.\n\ncharlie."),
            first.blocks,
            600.0,
            &theme(),
            base(),
            1.75,
        );
        assert_eq!(fresh, 2, "reuse continued past a changed block");
        assert_eq!(second.blocks[1].source, "CHANGED.");
        assert_eq!(second.blocks[2].source, "charlie.");
    }

    /// The reusing path must lay out identically to a cold laying: same
    /// blocks, same offsets, same total height. Reuse is an optimisation, not
    /// a different renderer.
    #[test]
    fn reuse_changes_nothing_about_the_result() {
        let mut text = TextSystem::default();
        let before = "prose first.\n\n```rust\nlet x = 1;\n```\n\nthen more pro";
        let after = "prose first.\n\n```rust\nlet x = 1;\n```\n\nthen more prose after.";
        let warm_start = lay_out_message(
            &mut text,
            &Message::assistant(before),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        let (warm, _) = lay_out_message_reusing(
            &mut text,
            &Message::assistant(after),
            warm_start.blocks,
            600.0,
            &theme(),
            base(),
            1.75,
        );
        let cold = lay_out_message(
            &mut text,
            &Message::assistant(after),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        assert_eq!(warm.blocks.len(), cold.blocks.len());
        assert_eq!(warm.height, cold.height, "reuse drifted the height");
        for (a, b) in warm.blocks.iter().zip(cold.blocks.iter()) {
            assert_eq!(a.source, b.source);
            assert_eq!(a.top, b.top, "reuse drifted a block offset");
            assert_eq!(a.height, b.height, "reuse drifted a block height");
        }
    }

    /// Streaming must continue one reply, not create a block per chunk:
    /// markdown spanning a chunk boundary would otherwise never parse.
    #[test]
    fn streaming_deltas_accumulate_into_one_message() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("hi"));
        transcript.append_assistant("**bo");
        transcript.append_assistant("ld**");
        assert_eq!(transcript.messages().len(), 2);
        assert_eq!(transcript.messages()[1].source, "**bold**");
    }

    /// A tool call is one transcript card however many events it produced:
    /// the name that opened it and the streamed intent that refined it must
    /// land on the same message, or every call would render twice.
    #[test]
    fn a_tool_call_refines_one_line_in_place() {
        let mut transcript = Transcript::default();
        transcript.set_live_tool("call_1", "bash");
        transcript.set_live_tool("call_1", "check the build");
        assert_eq!(transcript.messages().len(), 1);
        assert_eq!(transcript.messages()[0].role, Role::Tool);
        assert_eq!(transcript.messages()[0].source, "check the build");
    }

    /// A failure lands in the conversation, and it retires the live tool card:
    /// a call that errored is not still running, and leaving its card up would
    /// claim work is happening after it stopped.
    #[test]
    fn a_failure_is_recorded_and_retires_the_live_card() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("summarise the file"));
        transcript.set_live_tool("call_1", "read the file");
        transcript.push_notice("no network connection: dns error");
        let roles: Vec<_> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::User, Role::Notice]);
        assert!(transcript.plain_text().contains("no network connection"));
    }

    /// A provider that is unreachable fails once per retry. Twenty identical
    /// lines would bury the conversation under the same sentence, so repeats
    /// collapse; a *different* failure is still worth its own line.
    #[test]
    fn repeated_identical_failures_collapse() {
        let mut transcript = Transcript::default();
        transcript.push_notice("no network connection: dns error");
        transcript.push_notice("no network connection: dns error");
        transcript.push_notice("no network connection: dns error");
        assert_eq!(transcript.messages().len(), 1);
        transcript.push_notice("disconnected: the harness closed the connection");
        assert_eq!(transcript.messages().len(), 2);
    }

    /// A notice appears whole. If it counted as streaming text the reveal
    /// would start sweeping a status line in, which reads as the failure
    /// slowly typing itself out.
    #[test]
    fn a_failure_is_not_treated_as_arriving_text() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("go"));
        transcript.push_notice("no network connection: dns error");
        assert_eq!(transcript.streaming_len(), 0);
    }

    /// An edit is the one tool result that survives the turn: the live card is
    /// cleared when the turn ends, the edit card is not, because it records a
    /// change to the user's files.
    #[test]
    fn an_edit_card_outlives_the_turn() {
        let card = crate::edits::EditCard {
            intent: Some("rename the field".into()),
            files: vec!["src/lib.rs".into()],
            diff: "10- old\n10+ new\n".into(),
            added: 1,
            removed: 1,
        };
        let mut transcript = Transcript::default();
        transcript.set_live_tool("call_1", "rename the field");
        transcript.push_edit(&card);
        transcript.clear_live_tool();
        let roles: Vec<Role> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::Edit]);
        let source = &transcript.messages()[0].source;
        assert!(
            source.contains("rename the field"),
            "intent missing: {source}"
        );
        assert!(source.contains("src/lib.rs"), "file missing: {source}");
        assert!(source.contains("+1 -1"), "counts missing: {source}");
        assert!(source.contains("10+ new"), "diff missing: {source}");
    }

    /// The live tool card stays pinned to the tail when an edit lands, so the
    /// "what is running now" line does not end up buried above the history.
    #[test]
    fn an_edit_card_lands_above_the_live_tool_card() {
        let card = crate::edits::EditCard {
            intent: None,
            files: vec!["a.rs".into()],
            diff: "1+ fn main() {}\n".into(),
            added: 1,
            removed: 0,
        };
        let mut transcript = Transcript::default();
        transcript.set_live_tool("call_1", "write the file");
        transcript.push_edit(&card);
        transcript.set_live_tool("call_2", "run the tests");
        let roles: Vec<Role> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::Edit, Role::Tool]);
    }

    /// A ticking task is one card that updates, not one row per tick: a build
    /// that reports a hundred times must not push the conversation off screen.
    #[test]
    fn progress_ticks_refine_one_card() {
        let mut transcript = Transcript::default();
        transcript.set_progress("t1", "bash", "10% · compiling", Some(10.0));
        transcript.set_progress("t1", "bash", "90% · linking", Some(90.0));
        let cards: Vec<&Message> = transcript
            .messages()
            .iter()
            .filter(|message| message.role == Role::Progress)
            .collect();
        assert_eq!(cards.len(), 1, "a tick added a row instead of updating one");
        assert_eq!(cards[0].source, "bash · 90% · linking");
        assert_eq!(cards[0].fraction(), Some(0.9));
    }

    /// Two tasks are two bars, in the order they started: collapsing them would
    /// hide which of several waits is the slow one.
    #[test]
    fn several_tasks_keep_their_own_bars_in_start_order() {
        let mut transcript = Transcript::default();
        transcript.set_progress("first", "bash", "5%", Some(5.0));
        transcript.set_progress("second", "swarm", "working", None);
        transcript.set_progress("first", "bash", "50%", Some(50.0));
        let ids: Vec<&str> = transcript
            .messages()
            .iter()
            .filter(|message| message.role == Role::Progress)
            .map(|message| message.call_id.as_deref().expect("a task id"))
            .collect();
        assert_eq!(ids, vec!["first", "second"]);
        assert!(transcript.has_indeterminate_progress());
    }

    /// A finished task retires its own bar and only its own.
    #[test]
    fn a_finished_task_retires_only_its_own_bar() {
        let mut transcript = Transcript::default();
        transcript.set_progress("a", "bash", "50%", Some(50.0));
        transcript.set_progress("b", "bash", "20%", Some(20.0));
        assert!(transcript.clear_progress("a"));
        assert!(
            !transcript.clear_progress("a"),
            "clearing a retired bar reported a change"
        );
        let ids: Vec<&str> = transcript
            .messages()
            .iter()
            .filter(|message| message.role == Role::Progress)
            .map(|message| message.call_id.as_deref().expect("a task id"))
            .collect();
        assert_eq!(ids, vec!["b"]);
        assert!(transcript.has_progress());
        assert!(!transcript.has_indeterminate_progress());
        transcript.clear_all_progress();
        assert!(!transcript.has_progress());
    }

    /// The tail band's order: streamed text above, then the live tool card,
    /// then the bars, then anything the user queued. A bar that drifts into the
    /// middle of the conversation would read as history.
    #[test]
    fn progress_cards_sit_between_the_tool_card_and_the_queue() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("go"));
        transcript.set_live_tool("call_1", "wait for the build");
        transcript.set_progress("t1", "bash", "40%", Some(40.0));
        transcript.push(Message::queued("and then deploy"));
        transcript.append_assistant("Building. ");
        let roles: Vec<Role> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::Progress,
                Role::User,
            ],
            "the live status band came apart"
        );
    }

    /// An out-of-range percentage is clamped rather than drawn past the track's
    /// end: a task reporting 420% is a bug in the task, not licence to paint
    /// over the message below.
    #[test]
    fn a_cards_fraction_is_clamped_to_the_track() {
        let mut transcript = Transcript::default();
        transcript.set_progress("t1", "bash", "over", Some(420.0));
        transcript.set_progress("t2", "bash", "under", Some(-5.0));
        let fractions: Vec<Option<f64>> = transcript
            .messages()
            .iter()
            .filter(|message| message.role == Role::Progress)
            .map(Message::fraction)
            .collect();
        assert_eq!(fractions, vec![Some(1.0), Some(0.0)]);
    }

    /// Added and removed lines take their own ink, so a diff is read by
    /// scanning colour rather than by inspecting the first character of every
    /// line. Ink is checked at the span level, since that is what the layout
    /// hands to Parley.
    #[test]
    fn diff_lines_take_their_side_s_ink() {
        let theme = theme();
        // No language: every span falls back to the side's own ink, which is
        // one span for the gutter and one for the code.
        let spans = super::diff_spans("10- old\n10+ new\n… 2 more lines\n", None, &theme);
        let inks: Vec<Color> = spans.iter().map(|span| span.color.expect("ink")).collect();
        assert_eq!(
            inks,
            vec![
                theme.removed,
                theme.removed,
                theme.added,
                theme.added,
                theme.muted
            ],
            "a truncation marker must not be coloured as a change"
        );
    }

    /// With a language, the code is syntax-highlighted while the gutter keeps
    /// the diff's own ink: a diff must stay scannable by side *and* readable
    /// as the language it is written in.
    #[test]
    fn a_diff_is_highlighted_but_still_takes_sides() {
        let theme = theme();
        let source = "10+ let x = \"hi\";\n";
        let spans = super::diff_spans(source, Some("rs"), &theme);
        let gutter = spans.first().expect("a gutter span");
        assert_eq!(gutter.range, 0..3, "the gutter is the digits and the sign");
        assert_eq!(gutter.color, Some(theme.added));
        // Some span past the gutter must differ from the plain added ink, or
        // nothing was highlighted at all.
        assert!(
            spans
                .iter()
                .skip(1)
                .any(|span| span.color != Some(theme.added)),
            "the code was not highlighted"
        );
        // And every span must stay inside the line.
        assert!(spans.iter().all(|span| span.range.end <= source.len()));
    }

    /// A long diff is summarised rather than pasted whole: the card is one
    /// turn in a conversation, and the counts still report the full change.
    #[test]
    fn a_long_diff_is_truncated() {
        let diff: String = (1..=200).map(|line| format!("{line}+ x\n")).collect();
        let card = crate::edits::EditCard {
            intent: None,
            files: vec!["a.rs".into()],
            diff,
            added: 200,
            removed: 0,
        };
        let source = super::edit_source(&card);
        let rows = source
            .lines()
            .filter(|line| crate::edits::classify(line).is_some())
            .count();
        assert_eq!(rows, super::DIFF_ROW_LIMIT);
        assert!(
            source.contains(&format!("{} more lines", 200 - super::DIFF_ROW_LIMIT)),
            "{source}"
        );
        assert!(
            source.contains("+200 -0"),
            "the counts report the whole change"
        );
    }

    /// Line numbers are padded to one width, so both sides of a change start
    /// at the same column and can actually be compared.
    #[test]
    fn diff_gutters_are_aligned() {
        let card = crate::edits::EditCard {
            intent: None,
            files: vec!["a.rs".into()],
            diff: "9- old\n10+ new\n".into(),
            added: 1,
            removed: 1,
        };
        let source = super::edit_source(&card);
        let rows: Vec<&str> = source
            .lines()
            .filter(|line| crate::edits::classify(line).is_some())
            .collect();
        assert_eq!(rows, vec![" 9- old", "10+ new"]);
    }

    /// The fence carries the file's language, so the body goes through the
    /// same highlighting path as any other fenced block.
    #[test]
    fn an_edit_card_names_its_language() {
        let card = crate::edits::EditCard {
            intent: None,
            files: vec!["src/main.rs".into()],
            diff: "1+ x\n".into(),
            added: 1,
            removed: 0,
        };
        assert!(super::edit_source(&card).contains("```rs\n"));
    }

    /// Only the part of a paired row that differs is marked, and the marks
    /// are inside the two lines they belong to.
    #[test]
    fn only_the_changed_part_of_a_row_is_marked() {
        let (old, new) =
            super::changed_span("let alpha = 1;", "let fade = 1;").expect("a narrowed change");
        assert_eq!(&"let alpha = 1;"[old], "alpha");
        assert_eq!(&"let fade = 1;"[new], "fade");
        // Identical lines have nothing to mark, and a wholly different line is
        // not worth narrowing: the row band already says so.
        assert_eq!(super::changed_span("same", "same"), None);
        assert_eq!(super::changed_span("abc", "xyz"), None);
    }

    /// The card is a slot, not a log: the next call takes it over, and text
    /// arriving between calls is inserted *above* it, so the card is always
    /// the transcript's last message. At most one tool message exists.
    #[test]
    fn the_live_tool_card_is_singular() {
        let mut transcript = Transcript::default();
        transcript.set_live_tool("call_1", "read the config");
        transcript.append_assistant("Found it. ");
        let roles: Vec<_> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![Role::Assistant, Role::Tool],
            "streamed text did not land above the live card"
        );
        transcript.set_live_tool("call_2", "run the tests");
        let roles: Vec<_> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::Assistant, Role::Tool]);
        assert_eq!(transcript.messages()[1].source, "run the tests");
        // Back-to-back calls reuse the card in place.
        transcript.set_live_tool("call_3", "check the diff");
        let tools = transcript
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .count();
        assert_eq!(tools, 1, "a second call added a card instead of taking it");
        assert_eq!(transcript.messages()[1].source, "check the diff");
    }

    /// The card is pinned to the tail: however the turn interleaves text,
    /// reasoning, and calls, the live tool card is the last message, so it
    /// always renders at the bottom of the conversation.
    #[test]
    fn the_live_tool_card_stays_at_the_bottom() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("go"));
        transcript.set_live_tool("call_1", "read the config");
        transcript.append_reasoning("thinking about it ");
        transcript.append_assistant("Found it. ");
        transcript.append_assistant("Fixing now. ");
        assert_eq!(
            transcript.messages().last().map(|m| m.role),
            Some(Role::Tool),
            "streamed text pushed the live card off the tail"
        );
        // And the text above it coalesced normally rather than fragmenting
        // around the card.
        let roles: Vec<_> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![Role::User, Role::Reasoning, Role::Assistant, Role::Tool]
        );
        assert_eq!(transcript.messages()[2].source, "Found it. Fixing now. ");
    }

    /// The turn ending removes the card: a card left behind would claim work
    /// is still happening after it stopped.
    #[test]
    fn the_tool_card_clears_when_the_turn_ends() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("go"));
        transcript.set_live_tool("call_1", "running tests");
        transcript.append_assistant("done");
        transcript.clear_live_tool();
        assert!(
            transcript.messages().iter().all(|m| m.role != Role::Tool),
            "a finished turn left its tool card behind"
        );
        assert_eq!(transcript.messages().len(), 2);
    }

    /// A blank label is noise, not a card: an empty intent must not blank an
    /// existing label or create an empty message.
    #[test]
    fn a_blank_tool_label_changes_nothing() {
        let mut transcript = Transcript::default();
        transcript.set_live_tool("call_1", "   ");
        assert!(transcript.messages().is_empty());
        transcript.set_live_tool("call_1", "list the crate");
        transcript.set_live_tool("call_1", "");
        assert_eq!(transcript.messages()[0].source, "list the crate");
    }

    /// The card is a status readout pinned under the stream, not part of it:
    /// it must not count toward the reveal, or every call would rewind the
    /// cursor to sweep a one-line label while the reply above it stalls.
    #[test]
    fn the_tool_card_does_not_count_toward_the_reveal() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("go"));
        transcript.append_assistant("The fix is in the layout module.");
        let reply_len = transcript.streaming_len();
        assert!(reply_len > 0);
        transcript.set_live_tool("call_1", "running tests");
        assert_eq!(
            transcript.streaming_len(),
            reply_len,
            "the live card changed the streaming length"
        );
    }

    /// Reasoning coalesces like a reply, and the answer that follows it starts
    /// a new message: without this, "thought, then answered" would render as
    /// one undifferentiated block.
    #[test]
    fn reasoning_accumulates_then_yields_to_the_answer() {
        let mut transcript = Transcript::default();
        transcript.append_reasoning("first ");
        transcript.append_reasoning("thought");
        transcript.append_assistant("the answer");
        let roles: Vec<_> = transcript.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::Reasoning, Role::Assistant]);
        assert_eq!(transcript.messages()[0].source, "first thought");
    }

    /// `current` mode is the point of the feature: only the thought being
    /// written right now is on screen, and only its current paragraph. Without
    /// the paragraph trim a long think still grows without bound, which is
    /// exactly the wall of text the mode exists to prevent.
    #[test]
    fn current_mode_keeps_only_the_paragraph_being_written() {
        let mut transcript = Transcript::default();
        transcript.set_reasoning_mode(crate::reasoning::ReasoningMode::Current);
        transcript.append_reasoning("first point, now settled.\n\n");
        transcript.append_reasoning("second point, still being");
        assert_eq!(
            transcript.plain_text(),
            "second point, still being",
            "a finished paragraph was kept in current mode"
        );
    }

    /// A committed answer and an opening tool call are both proof the model
    /// stopped thinking: in `current` mode the thought leaves with them.
    #[test]
    fn current_mode_retires_the_thought_when_the_model_acts() {
        let mut answered = Transcript::default();
        answered.set_reasoning_mode(crate::reasoning::ReasoningMode::Current);
        answered.append_reasoning("thinking");
        answered.append_assistant("the answer");
        assert_eq!(
            answered
                .messages()
                .iter()
                .map(|m| m.role)
                .collect::<Vec<_>>(),
            vec![Role::Assistant]
        );

        let mut called = Transcript::default();
        called.set_reasoning_mode(crate::reasoning::ReasoningMode::Current);
        called.append_reasoning("thinking");
        called.set_live_tool("call_1", "running tests");
        assert_eq!(
            called.messages().iter().map(|m| m.role).collect::<Vec<_>>(),
            vec![Role::Tool]
        );
    }

    /// The next thought after a tool call is shown: `current` means "the live
    /// one", not "only the first one".
    #[test]
    fn current_mode_shows_the_next_thought() {
        let mut transcript = Transcript::default();
        transcript.set_reasoning_mode(crate::reasoning::ReasoningMode::Current);
        transcript.append_reasoning("first thought");
        transcript.set_live_tool("call_1", "reading the file");
        transcript.append_reasoning("second thought");
        assert!(transcript.plain_text().contains("second thought"));
        assert!(
            !transcript.plain_text().contains("first thought"),
            "a superseded thought came back"
        );
    }

    /// `full` keeps history, which is the classic behavior and must not be
    /// changed by the trimming that `current` does.
    #[test]
    fn full_mode_keeps_every_thought() {
        let mut transcript = Transcript::default();
        transcript.set_reasoning_mode(crate::reasoning::ReasoningMode::Full);
        transcript.append_reasoning("first point.\n\nsecond point");
        transcript.append_assistant("the answer");
        assert!(transcript.plain_text().contains("first point."));
        assert_eq!(
            transcript
                .messages()
                .iter()
                .map(|m| m.role)
                .collect::<Vec<_>>(),
            vec![Role::Reasoning, Role::Assistant]
        );
    }

    /// `off` means no thinking reaches the transcript at all; the delta still
    /// drove the status line, which is a separate concern.
    #[test]
    fn off_mode_drops_reasoning_entirely() {
        let mut transcript = Transcript::default();
        transcript.set_reasoning_mode(crate::reasoning::ReasoningMode::Off);
        transcript.append_reasoning("thinking hard");
        assert!(transcript.is_empty());
    }

    /// Reasoning must read as subordinate by ink and size alone: dimmer than
    /// the reply and set slightly smaller, on the same measure. Equal
    /// treatment would make a thought indistinguishable from the reply; an
    /// indent would make it read as a quoted block.
    #[test]
    fn reasoning_is_set_apart_from_the_reply() {
        let mut text = TextSystem::default();
        let theme = theme();
        let lay = |text: &mut TextSystem, message: &Message| {
            lay_out_message(text, message, 600.0, &theme, base(), 1.75)
        };
        let thought = lay(&mut text, &Message::reasoning("a thought"));
        let reply = lay(&mut text, &Message::assistant("a thought"));
        assert_eq!(
            thought.blocks[0].inset, reply.blocks[0].inset,
            "reasoning was indented instead of merely dimmed"
        );
        assert!(
            thought.height < reply.height
                || thought.blocks[0].layout.width() < reply.blocks[0].layout.width(),
            "reasoning was set at the same size as the reply"
        );
    }

    /// Emphasis inside a thought must stay muted. Without the tint, a bold
    /// word in reasoning would be drawn in full-strength body ink and read as
    /// louder than the answer beneath it.
    #[test]
    fn emphasis_inside_reasoning_stays_muted() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::reasoning("**loud** and quiet"),
            600.0,
            &theme,
            base(),
            1.75,
        );
        let brushes: Vec<_> = laid.blocks[0]
            .layout
            .lines()
            .flat_map(|line| line.items().collect::<Vec<_>>())
            .filter_map(|item| match item {
                parley::PositionedLayoutItem::GlyphRun(run) => Some(run.style().brush.clone()),
                _ => None,
            })
            .collect();
        assert!(!brushes.is_empty(), "no glyph runs to check");
        for brush in brushes {
            assert_eq!(
                brush,
                Brush::Solid(theme.faint),
                "a span in reasoning escaped the faint tint"
            );
        }
    }

    /// The marker is gone: role is carried in the model, so nothing needs to
    /// prefix a caret onto the user's own words.
    #[test]
    fn a_user_message_carries_no_marker_text() {
        let transcript = Transcript::from(&[Message::user("hello")][..]);
        assert!(
            !transcript.plain_text().contains('>'),
            "a shell prompt marker leaked into the transcript text"
        );
    }

    /// Markdown reaches the layout as *styling*, not as literal asterisks.
    #[test]
    fn emphasis_is_styling_rather_than_punctuation() {
        let document = parse_markdown("**bold** and *italic* and `code`");
        let lines: Vec<_> = document.lines().cloned().collect();
        let (source, spans) = flatten(&lines);
        assert!(
            !source.contains('*') && !source.contains('`'),
            "markdown punctuation survived into the drawn text: {source:?}"
        );
        assert!(
            spans.iter().any(|span| span.bold),
            "no bold span was produced"
        );
        assert!(
            spans.iter().any(|span| span.italic),
            "no italic span was produced"
        );
        assert!(
            spans.iter().any(|span| span.role == StyleRole::Code),
            "no code span was produced"
        );
    }

    /// LaTeX is rendered, not echoed. render-core turns `$x^2$` into Unicode
    /// math; the desktop must be showing that rather than the source.
    #[test]
    fn inline_latex_renders_as_math() {
        let document = parse_markdown("the value $x^2$ grows");
        let text: String = document
            .lines()
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains('\u{00b2}'),
            "inline latex was not rendered to math: {text:?}"
        );
        assert!(!text.contains("x^2"), "raw latex source survived: {text:?}");
    }

    #[test]
    fn display_latex_becomes_its_own_block() {
        let document = parse_markdown("before\n\n$$\\frac{a}{b}$$\n\nafter");
        assert!(
            document
                .blocks
                .iter()
                .any(|block| block.kind == BlockKind::MathDisplay),
            "display math did not become a math block"
        );
    }

    /// Height must be measured, not counted: a paragraph that wraps is taller
    /// than a paragraph that does not, even with the same newline count.
    #[test]
    fn measured_height_grows_with_wrapping() {
        let short = laid("one line");
        let long = laid(&"alpha bravo charlie delta echo foxtrot ".repeat(8));
        assert!(
            long.height > short.height * 2.0,
            "wrapped text measured {:.1}, barely more than {:.1}",
            long.height,
            short.height
        );
    }

    /// A user message reserves its card padding, so the tint cannot crop the
    /// text it wraps.
    #[test]
    fn a_user_message_reserves_its_card_padding() {
        let mut text = TextSystem::default();
        let user = lay_out_message(
            &mut text,
            &Message::user("hello"),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        let assistant = lay_out_message(
            &mut text,
            &Message::assistant("hello"),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        assert!(
            user.height >= assistant.height + USER_PAD_Y * 2.0 - 0.01,
            "user card did not reserve padding: {:.1} vs {:.1}",
            user.height,
            assistant.height
        );
    }

    /// Structure survives into laid-out blocks, so the renderer can draw a
    /// code wash without re-parsing.
    #[test]
    fn code_blocks_keep_their_kind() {
        let laid = laid("text\n\n```rust\nfn main() {}\n```\n");
        assert!(
            laid.blocks
                .iter()
                .any(|block| matches!(block.kind, BlockKind::CodeBlock { .. })),
            "code block lost its kind"
        );
    }

    /// Awkward and hostile inputs must lay out rather than panic. A transcript
    /// renders whatever a model emits, so this is a real input space.
    #[test]
    fn hostile_markdown_lays_out_without_panicking() {
        for source in [
            "",
            "\n\n\n",
            "```",
            "```rust",
            "$$",
            "$x^",
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "> quote\n> more",
            "- a\n  - b\n    - c",
            "#".repeat(40).as_str(),
            "ünïcödé 中文 🎉 **bold**",
            &"word ".repeat(500),
        ] {
            let _ = laid(source);
        }
    }

    /// Tables are laid out rather than silently dropped: render-core leaves
    /// their width-dependent layout to the front-end, and a front-end that
    /// ignores that renders nothing at all.
    #[test]
    fn tables_produce_visible_lines() {
        let laid = laid("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(!laid.blocks.is_empty(), "table produced no drawable blocks");
    }

    /// Table cells line up into columns. A naive `join(" ")` adapter passes
    /// the test above while rendering an unreadable ragged block, so the
    /// alignment itself has to be asserted: every row's second column has to
    /// start at the same cell.
    #[test]
    fn table_columns_are_aligned() {
        let rows = vec![
            vec!["frame".to_string(), "direction".to_string()],
            vec!["hello".to_string(), "client".to_string()],
            vec!["a-much-longer-frame".to_string(), "server".to_string()],
        ];
        let lines = table_lines(&rows, &[], 80);
        let seconds: Vec<usize> = lines
            .iter()
            .map(|line| line.plain_text())
            // The header rule is solid, so it has no column boundary to find.
            .filter(|text| !text.starts_with('\u{2500}'))
            .map(|text| {
                text.find("  ")
                    .map(|index| {
                        text[index..].trim_start().as_ptr() as usize - text.as_ptr() as usize
                    })
                    .unwrap_or(0)
            })
            .collect();
        assert!(
            seconds.windows(2).all(|pair| pair[0] == pair[1]),
            "table columns did not align: {seconds:?}"
        );
    }

    /// A table wider than its measure is squeezed to fit, not run off the page.
    /// This is the failure a reader notices first: the right-hand columns of a
    /// wide table were simply not on screen.
    #[test]
    fn wide_tables_fit_the_measure() {
        use unicode_width::UnicodeWidthStr;

        let rows = vec![
            vec![
                "a very wide heading indeed".to_string(),
                "another wide heading here".to_string(),
                "third wide heading again".to_string(),
            ],
            vec![
                "lorem ipsum dolor sit amet consectetur adipiscing".to_string(),
                "sed do eiusmod tempor incididunt ut labore".to_string(),
                "magna aliqua ut enim ad minim veniam".to_string(),
            ],
        ];
        let columns = 40;
        for line in table_lines(&rows, &[], columns) {
            let text = line.plain_text();
            assert!(
                text.width() <= columns,
                "table line overflows the measure ({} > {columns}): {text:?}",
                text.width()
            );
        }
    }

    /// A squeezed cell wraps inside its column instead of losing its tail: a
    /// table is often the densest thing in a reply, so truncation hides the
    /// answer.
    #[test]
    fn squeezed_cells_wrap_rather_than_truncate() {
        let rows = vec![
            vec!["h".to_string(), "heading".to_string()],
            vec![
                "one two three four five six seven".to_string(),
                "x".to_string(),
            ],
        ];
        let text: String = table_lines(&rows, &[], 24)
            .iter()
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        for word in ["one", "seven"] {
            assert!(text.contains(word), "cell text lost {word:?}: {text:?}");
        }
    }

    /// A delimiter row's alignment is honoured: a right-aligned numeric column
    /// that renders left-aligned misreads what the author wrote.
    #[test]
    fn table_alignment_follows_the_delimiter_row() {
        let document = parse_markdown(
            "| n |
|--:|
| 1 |
| 1000 |",
        );
        let table = document
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::Table)
            .expect("no table block");
        assert_eq!(table.alignments.first().copied(), Some(Alignment::Right));
        let lines = table_lines(&table.table, &table.alignments, 80);
        let short = lines
            .iter()
            .map(|line| line.plain_text())
            .find(|text| text.trim() == "1")
            .expect("no short row");
        assert!(
            short.starts_with(' '),
            "right-aligned cell was not padded on the left: {short:?}"
        );
    }

    /// A table's header is separated from its body by a rule, so the table
    /// reads as a table rather than as bold text sitting above some rows.
    #[test]
    fn tables_rule_off_their_header() {
        let lines = table_lines(
            &[
                vec!["a".to_string(), "b".to_string()],
                vec!["1".to_string(), "2".to_string()],
            ],
            &[],
            80,
        );
        let second = lines[1].plain_text();
        assert!(
            second.chars().all(|glyph| glyph == '\u{2500}'),
            "no header rule under the header row: {second:?}"
        );
    }

    /// A fenced block written under a list item belongs to that item. Indenting
    /// only the item pulled the code back to the margin, which broke the list
    /// open around it.
    #[test]
    fn blocks_nested_in_a_list_keep_the_list_indent() {
        let document = parse_markdown(
            "1. step one

   ```rust
   let x = 1;
   ```
",
        );
        let code = document
            .blocks
            .iter()
            .find(|block| matches!(block.kind, BlockKind::CodeBlock { .. }))
            .expect("no code block");
        assert_eq!(code.list_depth, 1, "code block lost its list context");
        assert!(
            block_inset(code) > CODE_PAD_X,
            "nested code block was not indented into its list item"
        );
    }

    /// A table streams in one character at a time without panicking or losing
    /// its columns. Half a delimiter row is not a table yet, and the block a
    /// prefix parses into changes shape as the rows arrive, which is exactly
    /// the case a width-dependent layout can get wrong.
    #[test]
    fn tables_survive_being_streamed() {
        let source = "| a | bb |\n|:--|--:|\n| 1 | 2000 |\n| 3 | 4 |\n";
        let mut text = TextSystem::default();
        for end in source
            .char_indices()
            .map(|(index, _)| index)
            .chain([source.len()])
        {
            let laid = lay_out_message(
                &mut text,
                &Message::assistant(&source[..end]),
                600.0,
                &theme(),
                base(),
                1.75,
            );
            assert!(laid.height >= 0.0);
        }
    }

    /// Copying a table pastes usable text. The transcript's rule is "what you
    /// see is what you paste", so the columns come along, but no line may carry
    /// trailing padding: pasted into an editor that is what shows up as a
    /// ragged block of invisible whitespace.
    #[test]
    fn copied_table_lines_carry_no_trailing_padding() {
        let laid = laid("| a | bbbb |\n|---|---|\n| 1 | 2 |\n");
        let table = laid
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::Table)
            .expect("no table block");
        for line in table.source.lines() {
            assert_eq!(
                line.trim_end(),
                line,
                "copied table line carries trailing padding: {line:?}"
            );
        }
    }

    /// A table laid out to a narrow measure still fits it, and still has every
    /// column. The squeeze is the only thing standing between a narrow window
    /// and a table drawn off the page, so it is asserted through the real
    /// layout rather than only through `table_lines`.
    #[test]
    fn narrow_windows_still_fit_their_tables() {
        let mut text = TextSystem::default();
        let source = "| field | meaning | bytes |\n|:--|:-:|--:|\n\
                      | `kind` | which frame this is and how to read it | 1 |\n\
                      | `payload` | length-prefixed line-delimited JSON | 4096 |\n";
        for width in [220.0, 320.0, 600.0] {
            let laid = lay_out_message(
                &mut text,
                &Message::assistant(source),
                width,
                &theme(),
                base(),
                1.75,
            );
            let table = laid
                .blocks
                .iter()
                .find(|block| block.kind == BlockKind::Table)
                .expect("no table block");
            // Wrapping inside a cell is fine; wrapping the *row* is what the
            // budget exists to prevent, because a wrapped row breaks the
            // columns the reader is scanning down.
            let rows = table.source.lines().count();
            assert!(rows >= 4, "table lost rows at width {width}: {rows}");
            for header in ["field", "meaning", "bytes"] {
                assert!(
                    table.source.contains(header),
                    "table lost the {header:?} column at width {width}"
                );
            }
        }
    }

    /// A nested block's furniture starts at the block's edge, which is inside
    /// the list indent it inherited. Drawing the wash from the message margin
    /// left it visibly detached from the text it is supposed to wrap.
    #[test]
    fn nested_block_furniture_follows_its_indent() {
        let laid = laid("1. step one\n\n   ```rust\n   let x = 1;\n   ```\n");
        let code = laid
            .blocks
            .iter()
            .find(|block| matches!(block.kind, BlockKind::CodeBlock { .. }))
            .expect("no code block");
        assert!(
            code.edge() >= LIST_INDENT,
            "nested code wash sat at the margin (edge {})",
            code.edge()
        );
        assert!(
            (code.inset - code.edge() - CODE_PAD_X).abs() < f64::EPSILON,
            "wash and text disagree about the block's padding"
        );
    }

    /// A loose item's continuation paragraph hangs under the item's text. Left
    /// at the margin it read as a new paragraph that had escaped the list.
    #[test]
    fn list_continuation_paragraphs_hang_under_their_item() {
        let document = parse_markdown("1. step one\n\n   then dispatch on it.\n");
        let items: Vec<&Block> = document
            .blocks
            .iter()
            .filter(|block| matches!(block.kind, BlockKind::ListItem { .. }))
            .collect();
        assert_eq!(items.len(), 2, "expected the item and its continuation");
        assert!(
            block_inset(items[1]) > block_inset(items[0]),
            "continuation paragraph was not indented under its item"
        );
    }

    /// A task list renders as checkboxes. `[x]` on screen is markdown source
    /// sitting next to a bullet that *was* rendered.
    #[test]
    fn task_lists_render_as_checkboxes() {
        let document = parse_markdown(
            "- [ ] todo
- [x] done",
        );
        let text: String = document
            .blocks
            .iter()
            .flat_map(|block| block_lines(block, || 80))
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains("[ ]") && !text.contains("[x]"),
            "raw task markers survived: {text:?}"
        );
        assert!(
            text.contains('\u{2610}') && text.contains('\u{2611}'),
            "no checkboxes drawn: {text:?}"
        );
    }

    /// A quote inside a quote stays visibly deeper. The block carries one drawn
    /// rule, so stripping *every* bar flattened the inner quote onto the outer
    /// one, and "who is being quoted here" is the whole point of the nesting.
    #[test]
    fn nested_quotes_keep_their_inner_bars() {
        let document = parse_markdown("> outer\n> > inner\n");
        let quote = document
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::BlockQuote)
            .expect("no quote block");
        let lines = block_lines(quote, || 80);
        let text: Vec<String> = lines.iter().map(StyledLine::plain_text).collect();
        assert!(
            text.iter().any(|line| line.trim() == "outer"),
            "outer quote kept its bar: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains('\u{2502}') && line.contains("inner")),
            "nested quote lost its depth: {text:?}"
        );
    }

    /// A quote is drawn as a rule by the renderer, so the terminal's `│`
    /// prefix must not also survive into the text.
    #[test]
    fn quotes_do_not_carry_a_terminal_bar() {
        let document = parse_markdown("> quoted line\n> second line");
        let quote = document
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::BlockQuote)
            .expect("no quote block");
        let lines = block_lines(quote, || 80);
        for line in &lines {
            assert!(
                !line.plain_text().contains('\u{2502}'),
                "quote bar survived: {:?}",
                line.plain_text()
            );
        }
    }

    /// An inline code span must be visibly literal. The neutral model has
    /// always marked it with `FillRole::Code`, but nothing drew that fill, so
    /// `` `--flag` `` read exactly like the prose around it. The wash is what
    /// carries that meaning now, so a code span has to produce one.
    #[test]
    fn an_inline_code_span_gets_a_wash() {
        let mut text = TextSystem::default();
        let theme = theme();
        let plain = lay_out_message(
            &mut text,
            &Message::assistant("pass the flag to it"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let coded = lay_out_message(
            &mut text,
            &Message::assistant("pass the `--flag` to it"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        assert!(
            plain.blocks[0].washes.is_empty(),
            "prose with no code span drew a wash"
        );
        assert_eq!(
            coded.blocks[0].washes.len(),
            1,
            "an inline code span drew no wash, so it is invisible"
        );
    }

    /// The wash must sit behind the code span's own glyphs, not the whole line.
    /// A wash covering the paragraph would read as a code *block*, which is a
    /// different claim about the text.
    #[test]
    fn the_wash_covers_the_span_rather_than_the_line() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("a long sentence with `code` set inside of it"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let block = &laid.blocks[0];
        let wash = block.washes.first().expect("no wash");
        let line_width = f64::from(block.layout.width());
        assert!(wash.x0 > 0.0, "the wash started at the line's left edge");
        assert!(
            wash.width() < line_width / 2.0,
            "the wash spanned {:.1} of a {line_width:.1} line, so it reads as a code block",
            wash.width()
        );
        assert!(
            wash.height() < block.height,
            "the wash filled the whole line box instead of hugging the glyphs"
        );
    }

    /// A code span that wraps must be washed per line. One box around both
    /// halves would cover the text between them on the first line.
    #[test]
    fn a_wrapped_code_span_is_washed_line_by_line() {
        let mut text = TextSystem::default();
        let theme = theme();
        // Narrow enough that the span itself has to break.
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("x `aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`"),
            60.0,
            &theme,
            base(),
            1.0,
        );
        assert!(
            laid.blocks[0].washes.len() > 1,
            "a wrapped span produced one wash, so it boxes across lines"
        );
    }

    /// A code block draws its own wash across its full measure, so its spans
    /// must not draw a second one on top of it.
    #[test]
    fn a_code_block_does_not_double_wash_its_lines() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("```\nlet x = 1;\n```"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let block = laid
            .blocks
            .iter()
            .find(|block| matches!(block.kind, BlockKind::CodeBlock { .. }))
            .expect("no code block");
        assert!(
            block.washes.is_empty(),
            "a code block washed its spans on top of its own wash"
        );
    }

    /// A link is marked by an underline rather than by a colour, so the
    /// flattened span has to carry one even though the markdown did not.
    #[test]
    fn a_link_is_underlined() {
        let document = parse_markdown("see [the docs](https://example.com) for more");
        let lines = block_lines(&document.blocks[0], || 80);
        let (_, spans) = flatten(&lines);
        assert!(
            spans
                .iter()
                .any(|span| span.role == StyleRole::Link && span.underline),
            "a link span carried no underline, so it is indistinguishable from prose"
        );
    }

    /// The rule of a thematic break is drawn by the scene. If the block also
    /// laid render-core's `───` placeholder out, the break would be drawn twice.
    #[test]
    fn a_thematic_break_carries_no_glyphs() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("above\n\n---\n\nbelow"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let rule = laid
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::ThematicBreak)
            .expect("the break was dropped instead of reserving its air");
        assert_eq!(rule.glyphs, 0, "the break drew dashes as well as its rule");
        assert!(rule.height > 0.0, "the break reserved no room for its rule");
    }

    /// A list is one object. Render-core emits a block per item, so a uniform
    /// paragraph gap would scatter the items down the page as separate
    /// statements: items must sit tighter than paragraphs do.
    #[test]
    fn list_items_sit_tighter_than_paragraphs() {
        let mut text = TextSystem::default();
        let theme = theme();
        let lay = |text: &mut TextSystem, source: &str| {
            lay_out_message(
                text,
                &Message::assistant(source),
                600.0,
                &theme,
                base(),
                1.0,
            )
        };
        let list = lay(&mut text, "- one\n- two\n- three");
        let paragraphs = lay(&mut text, "one\n\ntwo\n\nthree");
        assert_eq!(
            list.blocks.len(),
            3,
            "the list did not produce three blocks"
        );
        let item_gap = list.blocks[1].top - (list.blocks[0].top + list.blocks[0].height);
        let para_gap =
            paragraphs.blocks[1].top - (paragraphs.blocks[0].top + paragraphs.blocks[0].height);
        assert!(
            item_gap < para_gap,
            "list items were spaced like paragraphs ({item_gap} vs {para_gap})"
        );
        assert!(item_gap > 0.0, "list items were allowed to touch");
    }

    /// A heading belongs to the text under it. Leading it more than it trails
    /// is what makes a reply scan as sections rather than as one column.
    #[test]
    fn a_heading_is_led_more_than_it_trails() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("intro text\n\n## Section\n\nbody text"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let heading = laid
            .blocks
            .iter()
            .position(|block| matches!(block.kind, BlockKind::Heading { .. }))
            .expect("no heading");
        let above = laid.blocks[heading].top
            - (laid.blocks[heading - 1].top + laid.blocks[heading - 1].height);
        let below =
            laid.blocks[heading + 1].top - (laid.blocks[heading].top + laid.blocks[heading].height);
        assert!(
            above > below,
            "the heading grouped with the text above it ({above} above, {below} below)"
        );
    }

    /// A display equation is a figure: indented off the measure and given air
    /// on both sides, so it does not read as a stray line of prose.
    #[test]
    fn display_math_is_set_off_as_a_figure() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("before\n\n$$\\frac{a}{b}$$\n\nafter"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let index = laid
            .blocks
            .iter()
            .position(|block| block.kind == BlockKind::MathDisplay)
            .expect("no display math block");
        assert!(
            laid.blocks[index].inset > 0.0,
            "the equation sat on the body measure instead of being set off"
        );
        let above =
            laid.blocks[index].top - (laid.blocks[index - 1].top + laid.blocks[index - 1].height);
        assert!(
            above > BLOCK_GAP,
            "the equation was led like a paragraph ({above})"
        );
    }

    /// Reuse must not change the geometry it is an optimisation for. The
    /// A nested item steps in, and does so geometrically rather than by keeping
    /// The rows of a rendered equation are parts of one picture, so they must be
    /// set tighter than prose. At body leading a fraction comes apart into three
    /// unrelated lines with its bar floating between them.
    #[test]
    fn display_math_is_set_tighter_than_prose() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("$$\\frac{a + b}{c}$$"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let math = laid
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::MathDisplay)
            .expect("no display math block");
        assert_eq!(math.layout.len(), 3, "the fraction did not lay out as rows");
        let rows = math.layout.len() as f64;
        let prose = f64::from(base().font_size) * f64::from(base().line_height) * rows;
        assert!(
            math.height < prose,
            "the equation was set at prose leading ({} vs {prose})",
            math.height
        );
    }

    /// A nested item steps in, and does so geometrically rather than by keeping
    /// render-core's leading spaces: an indent the wrap width also honours is
    /// what keeps a wrapped continuation line from sliding back under the
    /// bullet, which leading spaces cannot do.
    #[test]
    fn a_nested_list_item_is_indented_without_its_padding_spaces() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("- outer\n  - inner\n- outer again"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let inner = &laid.blocks[1];
        assert!(
            inner.inset > laid.blocks[0].inset,
            "a nested item sat at the same x as its parent"
        );
        assert!(
            !inner.source.starts_with(' '),
            "the nested item kept its padding spaces as well as its indent: {:?}",
            inner.source
        );
        assert_eq!(
            laid.blocks[2].inset, laid.blocks[0].inset,
            "the list did not step back out again"
        );
    }

    /// A bullet list followed immediately by a numbered one is two lists. They
    /// must be separated like paragraphs, or the numbers read as a continuation
    /// of the bullets.
    #[test]
    fn two_adjacent_lists_are_separated() {
        let mut text = TextSystem::default();
        let theme = theme();
        let laid = lay_out_message(
            &mut text,
            &Message::assistant("- one\n- two\n\n1. first\n2. second"),
            600.0,
            &theme,
            base(),
            1.0,
        );
        let within = laid.blocks[1].top - (laid.blocks[0].top + laid.blocks[0].height);
        let between = laid.blocks[2].top - (laid.blocks[1].top + laid.blocks[1].height);
        assert!(
            between > within,
            "the numbered list ran straight on from the bullets ({between} vs {within})"
        );
    }

    /// Reuse must not change the geometry it is an optimisation for. The
    /// pair-aware gaps read the *previous* block's kind, so a reused prefix has
    /// to keep reporting it or a streaming list would re-space as it arrives.
    #[test]
    fn reuse_preserves_the_pair_aware_gaps() {
        let mut text = TextSystem::default();
        let theme = theme();
        let source = "- one\n- two\n- three\n\n## Head\n\nbody";
        let message = Message::assistant(source);
        let first = lay_out_message(&mut text, &message, 600.0, &theme, base(), 1.0);
        let (again, fresh) = lay_out_message_reusing(
            &mut text,
            &message,
            first.blocks,
            600.0,
            &theme,
            base(),
            1.0,
        );
        assert_eq!(fresh, 0, "an unchanged message re-laid its blocks");
        let fresh_lay = lay_out_message(&mut text, &message, 600.0, &theme, base(), 1.0);
        let tops: Vec<f64> = again.blocks.iter().map(|block| block.top).collect();
        let expected: Vec<f64> = fresh_lay.blocks.iter().map(|block| block.top).collect();
        assert_eq!(tops, expected, "reuse moved the blocks it reused");
        assert_eq!(again.height, fresh_lay.height, "reuse changed the height");
    }
}
