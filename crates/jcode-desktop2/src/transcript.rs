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
    Block, BlockKind, Document, FillRole, StyleRole, StyledLine, parse_markdown,
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
        }
    }

    pub fn reasoning(source: impl Into<String>) -> Self {
        Self {
            role: Role::Reasoning,
            source: source.into(),
            call_id: None,
            delivery: None,
        }
    }

    pub fn tool(call_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            source: label.into(),
            call_id: Some(call_id.into()),
            delivery: None,
        }
    }

    /// An edit card, rendered from a finished edit tool call.
    pub fn edit(card: &crate::edits::EditCard) -> Self {
        Self {
            role: Role::Edit,
            source: edit_source(card),
            call_id: None,
            delivery: None,
        }
    }

    /// A failure, placed in the conversation where the user is already looking.
    pub fn notice(source: impl Into<String>) -> Self {
        Self {
            role: Role::Notice,
            source: source.into(),
            call_id: None,
            delivery: None,
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
    source.push_str("```\n");
    source.push_str(card.diff.trim_end());
    source.push_str("\n```\n");
    source
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

    /// Where streamed text goes: the end of the transcript, except that a live
    /// tool card and the queued messages at the tail are skipped over. One
    /// definition, so every append path keeps the card and the queue pinned
    /// and none can strand them mid-transcript.
    fn text_tail(&self) -> usize {
        let tail = self.queued_tail_start();
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
        // Above the queued messages: a failure is part of the turn that just
        // happened, while the queue is what happens next.
        let at = self.queued_tail_start();
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
            Role::Tool | Role::Notice => TOOL_PAD_Y,
            // An edit card is a card too: the diff sits on a wash that must
            // not crop the first line of it.
            Role::Edit => TOOL_PAD_Y,
            Role::Assistant | Role::Reasoning => 0.0,
        }
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
        Role::Tool => Some(theme.muted),
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
                Role::Tool => TOOL_INSET,
                _ => REASONING_INSET,
            },
        ),
        None => match notice {
            Some(color) => (ParagraphStyle { color, ..base }, NOTICE_INSET),
            // An edit card is indented to clear the rule down its left edge,
            // the same as a notice: the rule is what marks it, so the text must
            // not sit on top of it.
            None if message.role == Role::Edit => (base, NOTICE_INSET),
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

    for block in &document.blocks {
        let inset = block_inset(&block.kind) + role_inset;
        let lines = block_lines(block);
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
        let spans =
            if message.role == Role::Edit && matches!(block.kind, BlockKind::CodeBlock { .. }) {
                diff_spans(&source, theme)
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
            top += gap_between(previous_kind, &block.kind);
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
        blocks.push(LaidBlock {
            glyphs: crate::text::glyph_count(&layout),
            layout,
            source,
            top,
            height,
            inset,
            kind: block.kind.clone(),
            washes,
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
        Role::Assistant | Role::Reasoning => 0.0,
    };
    (
        LaidMessage {
            role: message.role,
            delivery: message.delivery,
            blocks,
            height,
        },
        fresh,
    )
}

/// Per-line ink for a diff body: added lines green, removed lines red, and
/// anything else (a `...` truncation marker) left in the block's own colour.
///
/// Byte ranges over the flattened block, so a wrapped long line keeps its ink
/// across the wrap: the colour belongs to the text, not to the screen row.
fn diff_spans(source: &str, theme: &Theme) -> Vec<SpanStyle> {
    let mut spans = Vec::new();
    let mut at = 0usize;
    for line in source.split_inclusive('\n') {
        let range = at..at + line.trim_end_matches('\n').len();
        at += line.len();
        let color = match crate::edits::classify(line) {
            Some(crate::edits::Change::Added) => theme.added,
            Some(crate::edits::Change::Removed) => theme.removed,
            None => continue,
        };
        spans.push(SpanStyle {
            range,
            role: StyleRole::Code,
            fill: FillRole::None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            color: Some(color),
        });
    }
    spans
}

/// Horizontal inset for a block kind, relative to the message's text column.
fn block_inset(kind: &BlockKind) -> f64 {
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

/// Heading sizes, as multiples of body copy. Restrained on purpose: an h1 in a
/// chat reply is a sentence, not a cover page.
fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.32,
        2 => 1.18,
        3 => 1.08,
        _ => 1.0,
    }
}

/// The styled lines a block contributes.
///
/// Two kinds need front-end treatment. Tables are left to the front-end by
/// render-core because column widths depend on the measure. Quotes arrive with
/// a terminal `│ ` bar on every line, which the desktop replaces with a real
/// drawn rule; keeping both would mark the quote twice.
fn block_lines(block: &Block) -> Vec<StyledLine> {
    if block.kind == BlockKind::Table && block.lines.is_empty() {
        return table_lines(&block.table);
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
    if matches!(block.kind, BlockKind::ListItem { depth, .. } if depth > 0) {
        return block
            .lines
            .iter()
            .map(|line| StyledLine {
                spans: strip_leading_indent(&line.spans),
                alignment: line.alignment,
            })
            .collect();
    }
    block.lines.clone()
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

/// Drop the leading terminal quote-bar span from a quoted line.
fn strip_quote_bar(spans: &[jcode_render_core::StyledSpan]) -> Vec<jcode_render_core::StyledSpan> {
    let mut spans = spans.to_vec();
    if spans
        .first()
        .is_some_and(|first| first.text.contains('\u{2502}'))
    {
        let first = &mut spans[0];
        first.text = first.text.trim_start_matches(['\u{2502}', ' ']).to_string();
        if first.text.is_empty() {
            spans.remove(0);
        }
    }
    spans
}

/// Lay a GFM table out as aligned columns.
///
/// The app is a monospace stack throughout, so padding each cell to the widest
/// in its column produces true columns. A naive `join` would put every row's
/// second cell at a different x, which reads worse than the markdown source.
fn table_lines(rows: &[Vec<String>]) -> Vec<StyledLine> {
    use jcode_render_core::{StyledSpan, TextAttrs};
    use unicode_width::UnicodeWidthStr;

    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.width())
                .max()
                .unwrap_or(0)
        })
        .collect();

    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let mut text = String::new();
            for (column, width) in widths.iter().enumerate() {
                let cell = row.get(column).map(String::as_str).unwrap_or("");
                text.push_str(cell);
                // No trailing padding on the last column: invisible, and it
                // makes the line measure wider than its ink.
                if column + 1 < columns {
                    text.push_str(&" ".repeat(width.saturating_sub(cell.width()) + 2));
                }
            }
            let mut span = StyledSpan::plain(text);
            // The header is the one piece of table structure worth carrying:
            // it says which way to read the rest.
            if index == 0 {
                span = span.with_attrs(TextAttrs {
                    bold: true,
                    ..TextAttrs::none()
                });
            }
            StyledLine::from_spans(vec![span])
        })
        .collect()
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

    /// Added and removed lines take their own ink, so a diff is read by
    /// scanning colour rather than by inspecting the first character of every
    /// line. Ink is checked at the span level, since that is what the layout
    /// hands to Parley.
    #[test]
    fn diff_lines_take_their_side_s_ink() {
        let theme = theme();
        let spans = super::diff_spans("10- old\n10+ new\n...\n", &theme);
        let inks: Vec<Color> = spans.iter().map(|span| span.color.expect("ink")).collect();
        assert_eq!(
            inks,
            vec![theme.removed, theme.added],
            "a truncation marker must not be coloured as a change"
        );
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
    /// alignment itself has to be asserted.
    #[test]
    fn table_columns_are_aligned() {
        let rows = vec![
            vec!["frame".to_string(), "direction".to_string()],
            vec!["hello".to_string(), "client".to_string()],
            vec!["a-much-longer-frame".to_string(), "server".to_string()],
        ];
        let lines = table_lines(&rows);
        let starts: Vec<usize> = lines
            .iter()
            .map(|line| {
                let text = line.plain_text();
                text.find(|c: char| c != ' ')
                    .map(|_| {
                        // Column two starts after the padded first cell.
                        text.len() - text.trim_start_matches(|c: char| c != ' ').len()
                    })
                    .unwrap_or(0)
            })
            .collect();
        let text: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        let second_column: Vec<usize> = text
            .iter()
            .map(|line| line.rfind("  ").map(|i| i + 2).unwrap_or(0))
            .collect();
        assert!(
            second_column.windows(2).all(|w| w[0] == w[1]),
            "table columns did not align: {text:?} (starts {starts:?})"
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
        let lines = block_lines(quote);
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
        let lines = block_lines(&document.blocks[0]);
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
