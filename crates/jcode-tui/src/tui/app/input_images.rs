//! Explicit ownership of composer attachment markers, independent of literal text.
use super::App;
use std::sync::Arc;

impl super::input::PreparedInput {
    pub(super) fn expanded_raw_input(&self) -> String {
        let text = self.image_markers.without_owned_markers(&self.raw_input);
        super::input::expand_paste_placeholders_with(&self.pasted_contents, &text)
    }

    pub(super) fn routing_input(&self) -> &str {
        self.routing_input.as_deref().unwrap_or(&self.raw_input)
    }
}

pub(super) fn take_prepared_input(app: &mut App) -> super::input::PreparedInput {
    // Implicit restore markers must not change the user's command intent.
    let mut routing_input =
        super::input::expand_paste_placeholders_with(&app.pasted_contents, &app.input);
    if super::input::promote_dropped_images(app) {
        routing_input =
            super::input::expand_paste_placeholders_with(&app.pasted_contents, &app.input);
    }
    app.ensure_input_image_markers();
    let transport_text = app.input_without_owned_image_markers();
    let image_markers = std::mem::take(&mut app.input_image_markers);
    let raw_input = std::mem::take(&mut app.input);
    app.record_prompt_history(&raw_input);
    let expanded = super::input::expand_paste_placeholders(app, &transport_text);
    let routing_input = Some(routing_input);
    let pasted_contents = std::mem::take(&mut app.pasted_contents);
    let images = app.take_pending_image_payloads();
    app.cursor_pos = 0;
    app.clear_input_undo_history();
    super::input::PreparedInput {
        raw_input,
        expanded,
        images,
        image_markers,
        routing_input,
        pasted_contents,
    }
}

pub(super) fn attach_image(app: &mut App, media_type: String, base64_data: String) {
    let size_kb = base64_data.len() / 1024;
    app.ensure_input_image_markers();
    app.remember_input_undo_state();
    super::input::expand_paste_for_edit(app);
    app.pending_images.push((media_type.clone(), base64_data));
    let placeholder = format!("[image {}]", app.pending_images.len());
    let at = app.cursor_pos;
    app.input.insert_str(at, &placeholder);
    app.cursor_pos += placeholder.len();
    app.input_images_inserted(at, placeholder.len(), Some(app.pending_images.len()));
    app.sync_model_picker_preview_from_input();
    app.set_status_notice(format!("Pasted {} ({} KB)", media_type, size_kb));
}

/// A span is owned only when created by attachment insertion or restored with
/// this metadata. Matching `[image N]` text never establishes ownership.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct InputImageMarkers {
    text: String,
    spans: Vec<(usize, usize, usize)>,
}

fn owned_marker_matches(text: Option<&str>, number: usize) -> bool {
    let marker = format!("[image {number}]");
    text.is_some_and(|text| text == marker || text.strip_prefix(' ') == Some(marker.as_str()))
}

impl InputImageMarkers {
    fn without_owned_markers(&self, input: &str) -> String {
        let mut text = input.to_string();
        if self.text == text {
            for &(start, end, _) in self.spans.iter().rev() {
                if text.get(start..end).is_some() {
                    text.replace_range(start..end, "");
                }
            }
        }
        text
    }

    pub(super) fn from_saved(value: &serde_json::Value) -> Self {
        match value.get("image_markers") {
            None => Self::default(),
            Some(value) => match serde_json::from_value(value.clone()) {
                Ok(markers) => markers,
                Err(error) => {
                    crate::logging::warn(&format!(
                        "Invalid saved image marker provenance: {error}"
                    ));
                    Self::default()
                }
            },
        }
    }
}

pub(super) type InputUndoSnapshot = (
    String,
    usize,
    Arc<Vec<(String, String)>>,
    InputImageMarkers,
    Arc<Vec<String>>,
);

impl App {
    pub(super) fn take_pending_image_payloads(&mut self) -> Vec<(String, String)> {
        self.input_image_markers = Default::default();
        std::mem::take(&mut self.pending_images)
            .into_iter()
            .filter(|(media_type, data)| !media_type.is_empty() && !data.is_empty())
            .collect()
    }

    pub(super) fn undo_input_change(&mut self) {
        self.input_typing_undo = None;
        if let Some(draft) = self.input_undo_stack.pop() {
            self.history_draft = None;
            self.restore_input_draft(draft);
            self.reset_tab_completion();
            self.sync_model_picker_preview_from_input();
            self.set_status_notice("↶ Input restored");
        } else {
            self.set_status_notice("Nothing to undo");
        }
    }

    pub(super) fn input_draft_snapshot(&self) -> InputUndoSnapshot {
        let images = self
            .input_undo_stack
            .last()
            .filter(|snapshot| snapshot.2.as_ref() == &self.pending_images)
            .map(|snapshot| Arc::clone(&snapshot.2))
            .unwrap_or_else(|| Arc::new(self.pending_images.clone()));
        let pasted_contents = self
            .input_undo_stack
            .last()
            .filter(|snapshot| snapshot.4.as_ref() == &self.pasted_contents)
            .map(|snapshot| Arc::clone(&snapshot.4))
            .unwrap_or_else(|| Arc::new(self.pasted_contents.clone()));
        (
            self.input.clone(),
            self.cursor_pos.min(self.input.len()),
            images,
            self.input_image_markers.clone(),
            pasted_contents,
        )
    }

    pub(super) fn take_input_draft(&mut self) -> InputUndoSnapshot {
        self.ensure_input_image_markers();
        (
            std::mem::take(&mut self.input),
            std::mem::replace(&mut self.cursor_pos, 0),
            Arc::new(std::mem::take(&mut self.pending_images)),
            std::mem::take(&mut self.input_image_markers),
            Arc::new(std::mem::take(&mut self.pasted_contents)),
        )
    }

    pub(super) fn restore_input_draft(&mut self, draft: InputUndoSnapshot) {
        let (text, cursor, images, markers, pasted_contents) = draft;
        self.input = text;
        self.cursor_pos = super::input::floor_char_boundary(&self.input, cursor);
        self.pending_images = Arc::unwrap_or_clone(images);
        self.input_image_markers = markers;
        self.pasted_contents = Arc::unwrap_or_clone(pasted_contents);
        self.ensure_input_image_markers();
    }

    /// Missing/legacy metadata cannot distinguish a user literal from a marker.
    /// Preserve all existing text as literal and append fresh, owned markers.
    pub(super) fn ensure_input_image_markers(&mut self) {
        let mut seen = vec![false; self.pending_images.len()];
        let mut previous_end = 0;
        let valid = self
            .input_image_markers
            .spans
            .iter()
            .all(|&(start, end, n)| {
                let valid = n > 0
                    && n <= seen.len()
                    && !seen[n - 1]
                    && start >= previous_end
                    && start < end
                    && owned_marker_matches(self.input.get(start..end), n);
                if valid {
                    seen[n - 1] = true;
                    previous_end = end;
                }
                valid
            });
        if self.input_image_markers.text != self.input || !valid {
            self.input_image_markers.spans.clear();
        }
        let mut visible = vec![false; self.pending_images.len()];
        for &(_, _, n) in &self.input_image_markers.spans {
            visible[n - 1] = true;
        }
        for (index, present) in visible.into_iter().enumerate() {
            if !present {
                let start = self.input.len();
                // The fresh span owns its separator too, so transport text
                // remains exact without changing visual or command boundaries.
                if self
                    .input
                    .chars()
                    .last()
                    .is_some_and(|c| !c.is_whitespace())
                {
                    self.input.push(' ');
                }
                self.input.push_str(&format!("[image {}]", index + 1));
                self.input_image_markers
                    .spans
                    .push((start, self.input.len(), index + 1));
            }
        }
        self.input_image_markers.text.clone_from(&self.input);
    }

    /// Rebase only spans that were owned before this exact insertion. An edit
    /// inside a marker damages that attachment, not an identical user literal.
    pub(super) fn input_images_inserted(
        &mut self,
        at: usize,
        len: usize,
        new_image: Option<usize>,
    ) {
        self.input_image_markers
            .spans
            .retain_mut(|(start, end, _)| {
                if at > *start && at < *end {
                    return false;
                }
                if at <= *start {
                    *start += len;
                    *end += len;
                }
                true
            });
        if let Some(number) = new_image {
            self.input_image_markers.spans.push((at, at + len, number));
        }
        self.reconcile_input_images();
    }

    pub(super) fn replace_input_range(&mut self, start: usize, end: usize, text: &str) {
        self.ensure_input_image_markers();
        self.cursor_pos = start;
        self.remove_input_range(start, end);
        super::input::expand_paste_for_edit(self);
        let at = self.cursor_pos;
        self.input.insert_str(at, text);
        self.input_images_inserted(at, text.len(), None);
        self.cursor_pos = at + text.len();
    }

    pub(super) fn remove_input_range(&mut self, mut start: usize, mut end: usize) -> String {
        if start == end {
            return String::new();
        }
        self.ensure_input_image_markers();
        for &(at, marker_end, _) in &self.input_image_markers.spans {
            if at < end && marker_end > start {
                start = start.min(at);
                end = end.max(marker_end);
            }
        }
        let mut retained = vec![false; self.pasted_contents.len()];
        for (at, label_end, index) in
            super::input::paste_placeholder_spans(&self.pasted_contents, &self.input)
        {
            retained[index] = at >= end || label_end <= start;
        }
        let mut index = 0;
        self.pasted_contents.retain(|_| {
            let keep = retained[index];
            index += 1;
            keep
        });
        self.input_image_markers
            .spans
            .retain_mut(|(at, marker_end, _)| {
                if *at < end && *marker_end > start {
                    return false;
                }
                if *at >= end {
                    *at -= end - start;
                    *marker_end -= end - start;
                }
                true
            });
        let removed = self.input.drain(start..end).collect();
        self.cursor_pos = start;
        self.reconcile_input_images();
        removed
    }

    /// Retain payloads only for intact, explicitly owned spans and rewrite only
    /// those spans when numbering changes. Literal lookalikes remain untouched.
    pub(super) fn reconcile_input_images(&mut self) {
        let mut spans = std::mem::take(&mut self.input_image_markers.spans);
        spans.retain(|&(start, end, n)| {
            n > 0
                && n <= self.pending_images.len()
                && owned_marker_matches(self.input.get(start..end), n)
        });
        spans.sort_unstable_by_key(|span| span.0);
        let mut keep = vec![false; self.pending_images.len()];
        for &(_, _, n) in &spans {
            keep[n - 1] = true;
        }
        let mut next = 0;
        let numbering: Vec<usize> = keep
            .iter()
            .map(|&keep| {
                if keep {
                    next += 1;
                    next
                } else {
                    0
                }
            })
            .collect();
        for index in (0..spans.len()).rev() {
            let (start, end, n) = spans[index];
            let separator = if self.input[start..end].starts_with(' ') {
                " "
            } else {
                ""
            };
            let replacement = format!("{separator}[image {}]", numbering[n - 1]);
            if self.cursor_pos >= end {
                self.cursor_pos = self.cursor_pos - (end - start) + replacement.len();
            } else if self.cursor_pos > start {
                self.cursor_pos = start + replacement.len();
            }
            self.input.replace_range(start..end, &replacement);
            for later in spans.iter_mut().skip(index + 1) {
                later.0 = later.0 - (end - start) + replacement.len();
                later.1 = later.1 - (end - start) + replacement.len();
            }
            spans[index] = (start, start + replacement.len(), numbering[n - 1]);
        }
        let mut index = 0;
        self.pending_images.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
        let removed = keep.iter().filter(|&&retained| !retained).count();
        if removed > 0 {
            self.set_status_notice(format!(
                "Detached {removed} image{} · {} attached",
                if removed == 1 { "" } else { "s" },
                self.pending_images.len()
            ));
        }
        self.input_image_markers.spans = spans;
        self.input_image_markers.text.clone_from(&self.input);
    }

    /// Transport text does not carry owned markers. On content-only restores,
    /// fresh owned spans can therefore be added without claiming any literal.
    pub(super) fn input_without_owned_image_markers(&self) -> String {
        self.input_image_markers.without_owned_markers(&self.input)
    }

    pub(super) fn remember_input_undo_state(&mut self) {
        self.ensure_input_image_markers();
        self.input_typing_undo = None;
        let snapshot = self.input_draft_snapshot();
        if self.input_undo_stack.last() == Some(&snapshot) {
            return;
        }
        if self.input_undo_stack.len() >= Self::INPUT_UNDO_LIMIT {
            self.input_undo_stack.remove(0);
        }
        self.input_undo_stack.push(snapshot);
    }

    pub(super) fn clear_input_undo_history(&mut self) {
        self.input_undo_stack.clear();
        self.input_typing_undo = None;
        self.history_draft = None;
    }

    pub(super) fn toggle_input_stash(&mut self) {
        if let Some(stashed) = self.stashed_input.take() {
            let current = self.take_input_draft();
            self.restore_input_draft(stashed);
            if current.0.is_empty() && current.2.is_empty() {
                self.set_status_notice("📋 Input restored from stash");
            } else {
                self.stashed_input = Some(current);
                self.set_status_notice("📋 Swapped input with stash");
            }
        } else if !self.input.is_empty() || !self.pending_images.is_empty() {
            self.stashed_input = Some(self.take_input_draft());
            self.set_status_notice("📋 Input stashed");
        }
    }
}

pub(super) fn retrieve_pending_message_for_edit(app: &mut App) -> bool {
    if !app.input.is_empty() {
        return false;
    }

    let mut parts: Vec<String> = Vec::new();
    let mut had_pending = false;

    if !app.pending_soft_interrupts.is_empty() {
        parts.extend(std::mem::take(&mut app.pending_soft_interrupts));
        app.pending_soft_interrupt_requests.clear();
        had_pending = true;
    }
    if let Some(msg) = app.interleave_message.take()
        && !msg.is_empty()
    {
        app.pending_images.append(&mut app.interleave_images);
        parts.push(msg);
        had_pending = true;
    }
    if !app.queued_messages.is_empty() {
        for text in std::mem::take(&mut app.queued_messages) {
            parts.push(text);
        }
        app.pending_images.append(&mut app.queued_images);
        if !app.has_queued_followups() {
            app.pending_queued_dispatch = false;
        }
        had_pending = true;
    }

    if !parts.is_empty() {
        app.input_image_markers = Default::default();
        // These were queued as prompts. Leading owned markers keep recalled
        // shell/slash-looking text from becoming an executable command.
        app.ensure_input_image_markers();
        let at = app.input.len();
        let text = parts.join("\n\n");
        app.input.push_str(&text);
        app.input_images_inserted(at, text.len(), None);
        app.cursor_pos = app.input.len();
        let count = parts.len();
        app.set_status_notice(format!(
            "Retrieved {} pending message{} for editing",
            count,
            if count == 1 { "" } else { "s" }
        ));
    } else if let Some(command) = app.queued_commands.pop() {
        // Commands are recalled one at a time (newest first): joining several
        // would turn them into a single malformed command.
        app.input = command;
        app.cursor_pos = app.input.len();
        app.sync_model_picker_preview_from_input();
        app.set_status_notice("Retrieved queued command for editing");
        return true;
    }

    had_pending
}
