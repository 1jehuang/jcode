//! Bounded live handler-to-ratatui readiness measurements. Never terminal paint.
use serde_json::{Value, json};
use std::{collections::VecDeque, time::Instant};
const LIMIT: usize = 256;
#[derive(Default)]
struct Samples(VecDeque<f64>);
impl Samples {
    fn push(&mut self, value: f64) {
        if self.0.len() == LIMIT {
            self.0.pop_front();
        }
        self.0.push_back(value);
    }
    fn json(&self) -> Value {
        let mut values: Vec<_> = self.0.iter().copied().collect();
        values.sort_by(f64::total_cmp);
        let percentile = |p: usize| {
            values
                .get((values.len() * p).div_ceil(100).saturating_sub(1))
                .copied()
        };
        json!({"samples": values.len(), "p99_has_100_samples": values.len() >= 100, "p50_ms": percentile(50), "p95_ms": percentile(95), "p99_ms": percentile(99), "max_ms": values.last()})
    }
}
struct Selection {
    id: String,
    started: Instant,
    selection: bool,
    transcript: bool,
    workspace: bool,
    failed: bool,
}
pub(super) struct Latency {
    opened: Instant,
    initial: bool,
    active: Option<Selection>,
    selection: Samples,
    transcript: Samples,
    workspace: Samples,
    render: Samples,
    started: u64,
    superseded: u64,
    cache_hits: u64,
    cache_misses: u64,
    failed: u64,
}
impl Default for Latency {
    fn default() -> Self {
        Self {
            opened: Instant::now(),
            initial: true,
            active: None,
            selection: Samples::default(),
            transcript: Samples::default(),
            workspace: Samples::default(),
            render: Samples::default(),
            started: 0,
            superseded: 0,
            failed: 0,
            cache_hits: 0,
            cache_misses: 0,
        }
    }
}
impl Latency {
    pub(super) fn begin(&mut self, id: String, at: Instant) {
        self.initial = false;
        if self
            .active
            .as_ref()
            .is_some_and(|s| !s.transcript && !s.failed)
        {
            self.superseded += 1;
        }
        self.started += 1;
        self.active = Some(Selection {
            id,
            started: at,
            selection: false,
            transcript: false,
            workspace: false,
            failed: false,
        });
    }
    pub(super) fn abandon(&mut self) {
        self.initial = false;
        if self
            .active
            .take()
            .is_some_and(|s| !s.transcript && !s.failed)
        {
            self.superseded += 1;
        }
    }
    pub(super) fn initial_selection(&mut self, id: Option<String>) {
        if self.initial
            && let Some(id) = id
        {
            self.begin(id, self.opened);
        }
    }
    pub(super) fn frame(
        &mut self,
        id: Option<String>,
        transcript: bool,
        duration: std::time::Duration,
    ) {
        self.render.push(duration.as_secs_f64() * 1000.0);
        if self.initial {
            if let Some(id) = id.as_ref() {
                self.begin(id.clone(), self.opened);
            }
        }
        let Some(s) = self.active.as_mut().filter(|s| Some(&s.id) == id.as_ref()) else {
            return;
        };
        let ms = s.started.elapsed().as_secs_f64() * 1000.0;
        if !s.selection {
            s.selection = true;
            self.selection.push(ms);
        }
        if transcript && !s.transcript && !s.failed {
            s.transcript = true;
            self.transcript.push(ms);
        }
    }
    pub(super) fn workspace(&mut self, id: &str) {
        if let Some(s) = self
            .active
            .as_mut()
            .filter(|s| s.id == id && !s.workspace && !s.failed)
        {
            s.workspace = true;
            self.workspace
                .push(s.started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    pub(super) fn fail(&mut self, id: &str) {
        if let Some(s) = self.active.as_mut().filter(|s| s.id == id && !s.failed) {
            s.failed = true;
            self.failed += 1;
        }
    }
    pub(super) fn cache_lookup(&mut self, hit: bool) {
        if hit {
            self.cache_hits += 1;
        } else {
            self.cache_misses += 1;
        }
    }
    fn json(&self) -> Value {
        json!({
            "open": true,
            "boundary": "handler entry (initial: picker initialization) to picker ratatui frame preparation complete, NOT terminal flush or emulator paint",
            "scope": "current picker lifetime, rolling nearest-rank distributions of successful stages only",
            "capacity_per_distribution": LIMIT,
            "started": self.started,
            "superseded_before_transcript": self.superseded,
            "failed": self.failed,
            "failure_scope": "selected transcript load returned no preview or disconnected",
            "pending_selection": self.active.as_ref().is_some_and(|s| !s.selection),
            "pending_transcript": self.active.as_ref().is_some_and(|s| !s.transcript && !s.failed),
            "selection_frame": self.selection.json(),
            "transcript_frame": self.transcript.json(),
            "workspace_facts_ready": self.workspace.json(),
            "render_duration": self.render.json(),
            "preview_cache_hits": self.cache_hits,
            "preview_cache_misses": self.cache_misses,
            "workspace_note": "Only accepted asynchronous hydration completions, cache hits are not timed"
        })
    }
}
impl super::SessionPicker {
    pub fn debug_resume_latency(&self) -> Value {
        self.resume_latency.json()
    }
    pub(super) fn latency_selection_id(&self) -> Option<String> {
        self.selected_session().map(|s| s.id.clone())
    }
    pub(super) fn latency_input_complete(&mut self, before: Option<String>, at: Instant) {
        let after = self.latency_selection_id();
        if before != after {
            if let Some(id) = after {
                self.resume_latency.begin(id, at);
            } else {
                self.resume_latency.abandon();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_navigation_and_stale_completions() {
        let mut latency = Latency::default();
        for i in 0..1000 {
            latency.begin(i.to_string(), Instant::now());
        }
        assert_eq!(latency.superseded, 999);
        latency.workspace("0");
        latency.fail("0");
        latency.frame(Some("0".into()), true, std::time::Duration::ZERO);
        assert!(latency.transcript.0.is_empty());
        assert!(latency.workspace.0.is_empty());
        latency.frame(Some("999".into()), false, std::time::Duration::ZERO);
        assert_eq!(latency.selection.0.len(), 1);
        assert!(latency.transcript.0.is_empty());
        latency.fail("999");
        latency.frame(Some("999".into()), true, std::time::Duration::ZERO);
        assert!(latency.transcript.0.is_empty());
        assert_eq!(latency.failed, 1);
        for _ in 0..1000 {
            latency.render.push(1.0);
        }
        assert_eq!(latency.render.0.len(), LIMIT);
    }
    #[test]
    fn picker_inputs_not_refresh_or_resize_start_measurements() {
        use crate::tui::session_picker::{
            PreviewExtras, SessionPicker, performance_tests::session,
        };
        use crossterm::event::{KeyCode, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let mut picker = SessionPicker::new(vec![session(1, 1), session(2, 1)]);
        // Avoid filesystem probes: these previews are already available.
        for id in ["performance-1", "performance-2"] {
            picker
                .preview_extras
                .insert(id.into(), PreviewExtras::default());
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| picker.render(f)).unwrap();
        assert_eq!(picker.debug_resume_latency()["started"], 1);
        assert_eq!(
            picker.debug_resume_latency()["selection_frame"]["samples"],
            1
        );
        assert_eq!(
            picker.debug_resume_latency()["transcript_frame"]["samples"],
            1
        );
        terminal.draw(|f| picker.render(f)).unwrap();
        picker
            .handle_overlay_key(KeyCode::Down, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(picker.debug_resume_latency()["started"], 2);
        assert_eq!(
            picker.debug_resume_latency()["transcript_frame"]["samples"],
            1
        );
        terminal.draw(|f| picker.render(f)).unwrap();
        assert_eq!(
            picker.debug_resume_latency()["transcript_frame"]["samples"],
            2
        );
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 60, 20))
            .unwrap();
        terminal.draw(|f| picker.render(f)).unwrap();
        assert_eq!(picker.debug_resume_latency()["started"], 2);
        // Reseeding is not an input event, even if the selected row disappears.
        picker.reseed_grouped(Vec::new(), vec![session(3, 1)]);
        picker
            .preview_extras
            .insert("performance-3".into(), PreviewExtras::default());
        terminal.draw(|f| picker.render(f)).unwrap();
        assert_eq!(picker.debug_resume_latency()["started"], 2);
        assert_eq!(
            picker.debug_resume_latency()["transcript_frame"]["samples"],
            2
        );
    }
    #[test]
    fn stage_samples_are_once_only_and_percentiles_are_nearest_rank() {
        let mut latency = Latency::default();
        latency.begin("a".into(), Instant::now());
        latency.frame(Some("a".into()), false, std::time::Duration::ZERO);
        assert_eq!(latency.selection.0.len(), 1);
        assert_eq!(latency.transcript.0.len(), 0);
        latency.frame(Some("a".into()), true, std::time::Duration::ZERO);
        latency.frame(Some("a".into()), true, std::time::Duration::ZERO);
        latency.workspace("a");
        latency.workspace("a");
        assert_eq!(latency.transcript.0.len(), 1);
        assert_eq!(latency.workspace.0.len(), 1);
        let mut samples = Samples::default();
        for value in 1..=100 {
            samples.push(value as f64);
        }
        let json = samples.json();
        assert_eq!(json["p50_ms"], 50.0);
        assert_eq!(json["p95_ms"], 95.0);
        assert_eq!(json["p99_ms"], 99.0);
        assert_eq!(json["max_ms"], 100.0);
        assert_eq!(json["p99_has_100_samples"], true);
    }
    #[test]
    fn pending_and_failed_preview_never_become_fast_successes() {
        use crate::tui::session_picker::{
            PendingSessionPreviewLoad, SessionPicker, performance_tests::session,
        };
        use ratatui::{Terminal, backend::TestBackend};
        let mut picker = SessionPicker::new(vec![session(1, 0)]);
        let (sender, receiver) = std::sync::mpsc::channel();
        picker.pending_preview_load = Some(PendingSessionPreviewLoad {
            session_id: "performance-1".into(),
            receiver,
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| picker.render(f)).unwrap();
        assert_eq!(
            picker.debug_resume_latency()["selection_frame"]["samples"],
            1
        );
        assert_eq!(
            picker.debug_resume_latency()["transcript_frame"]["samples"],
            0
        );
        assert_eq!(picker.debug_resume_latency()["pending_transcript"], true);
        sender.send(None).unwrap();
        terminal.draw(|f| picker.render(f)).unwrap();
        assert_eq!(picker.debug_resume_latency()["failed"], 1);
        assert_eq!(picker.debug_resume_latency()["pending_transcript"], false);
        assert!(picker.debug_resume_latency()["transcript_frame"]["p50_ms"].is_null());
    }
    #[test]
    fn empty_distributions_have_null_percentiles() {
        assert!(Samples::default().json()["p50_ms"].is_null());
        assert_eq!(Samples::default().json()["p99_has_100_samples"], false);
    }
}
