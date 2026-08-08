//! Declarative pointer targets and their visual feedback.
//!
//! Keeping hit geometry, cursor intent, and hover treatment in one record makes
//! an interactive control impossible to add silently: [`Registry::audit`]
//! reports every target that has no feedback assigned.

use std::time::{Duration, Instant};
use vello::kurbo::Rect;

const HOVER_TIME: Duration = Duration::from_millis(140);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    Settings,
    Sessions,
    NewSession,
    SettingsRow(usize),
    ModelRow(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// A soft rounded wash and outline, suitable for buttons and menu rows.
    Glow,
    /// Intentionally absent. This exists so the audit can catch unfinished UI.
    Missing,
}

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub id: Id,
    pub rect: Rect,
    pub feedback: Feedback,
}

impl Target {
    pub fn glow(id: Id, rect: Rect) -> Self {
        Self {
            id,
            rect,
            feedback: Feedback::Glow,
        }
    }
}

#[derive(Default)]
pub struct Registry {
    targets: Vec<Target>,
    hovered: Option<Id>,
    entered: Option<Instant>,
}

impl Registry {
    pub fn sync(&mut self, targets: Vec<Target>, pointer: (f64, f64), now: Instant) -> bool {
        let hovered = targets
            .iter()
            .rev()
            .find(|target| target.rect.contains(pointer))
            .map(|target| target.id);
        let changed = hovered != self.hovered;
        if changed {
            self.hovered = hovered;
            self.entered = hovered.map(|_| now);
        }
        self.targets = targets;
        changed
    }

    pub fn hovered(&self) -> Option<(Target, f64)> {
        self.hovered_at(Instant::now())
    }

    fn hovered_at(&self, now: Instant) -> Option<(Target, f64)> {
        let id = self.hovered?;
        let target = *self.targets.iter().find(|target| target.id == id)?;
        let elapsed = now.duration_since(self.entered?).as_secs_f64();
        let t = (elapsed / HOVER_TIME.as_secs_f64()).clamp(0.0, 1.0);
        // Smoothstep avoids a hard start or stop without maintaining a timer per control.
        Some((target, t * t * (3.0 - 2.0 * t)))
    }

    pub fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        let end = self.entered? + HOVER_TIME;
        (now < end).then(|| now + Duration::from_millis(16))
    }

    pub fn audit(&self) -> Vec<Id> {
        self.targets
            .iter()
            .filter(|target| target.feedback == Feedback::Missing)
            .map(|target| target.id)
            .collect()
    }
}

pub fn targets(frame: &crate::layout::Frame, model: &crate::Model) -> Vec<Target> {
    let mut targets = vec![
        Target::glow(Id::Sessions, frame.sessions()),
        Target::glow(Id::Settings, frame.gear()),
        Target::glow(Id::NewSession, frame.new_session()),
    ];
    if model.panel.is_open() {
        targets.extend((0..model.panel.rows().len()).map(|index| {
            Target::glow(
                Id::SettingsRow(index),
                frame.panel_row(model.panel.rows().len(), index),
            )
        }));
    }
    if model.model_picker.is_open() {
        let rows = model.model_picker.visual_rows();
        targets.extend(
            (0..rows)
                .map(|index| Target::glow(Id::ModelRow(index), frame.model_menu_row(rows, index))),
        );
    }
    targets
}

pub fn draw(
    scene: &mut vello::Scene,
    registry: &Registry,
    theme: &crate::theme::Theme,
    scale: f64,
) {
    let Some((target, amount)) = registry.hovered() else {
        return;
    };
    if target.feedback != Feedback::Glow {
        return;
    }
    let shape = target.rect.inset(-2.0).to_rounded_rect(7.0);
    scene.fill(
        vello::peniko::Fill::NonZero,
        vello::kurbo::Affine::scale(scale),
        theme.selection.with_alpha((0.12 + 0.22 * amount) as f32),
        None,
        &shape,
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(0.8 + 0.7 * amount),
        vello::kurbo::Affine::scale(scale),
        theme.text.with_alpha((0.10 + 0.20 * amount) as f32),
        None,
        &shape,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topmost_target_wins_and_missing_feedback_is_audited() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let mut registry = Registry::default();
        registry.sync(
            vec![
                Target::glow(Id::Settings, rect),
                Target {
                    id: Id::Sessions,
                    rect,
                    feedback: Feedback::Missing,
                },
            ],
            (10.0, 10.0),
            Instant::now(),
        );
        assert_eq!(
            registry.hovered().map(|(target, _)| target.id),
            Some(Id::Sessions)
        );
        assert_eq!(registry.audit(), vec![Id::Sessions]);
    }

    #[test]
    fn every_registered_desktop_control_has_feedback() {
        let model = crate::Model::default();
        let frame = crate::layout::Frame::new((1100, 720), 1.0);
        let mut registry = Registry::default();
        registry.sync(targets(&frame, &model), (-1.0, -1.0), Instant::now());
        assert_eq!(registry.audit(), Vec::<Id>::new());
    }

    #[test]
    fn hover_animates_to_completion_and_clears_on_exit() {
        let start = Instant::now();
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let mut registry = Registry::default();
        registry.sync(vec![Target::glow(Id::Settings, rect)], (10.0, 10.0), start);

        assert_eq!(registry.hovered_at(start).map(|(_, amount)| amount), Some(0.0));
        let halfway = registry
            .hovered_at(start + HOVER_TIME / 2)
            .map(|(_, amount)| amount)
            .unwrap();
        assert!(halfway > 0.0 && halfway < 1.0);
        assert_eq!(
            registry.hovered_at(start + HOVER_TIME).map(|(_, amount)| amount),
            Some(1.0)
        );
        assert!(registry.next_frame_at(start + HOVER_TIME).is_none());

        assert!(registry.sync(
            vec![Target::glow(Id::Settings, rect)],
            (30.0, 30.0),
            start + HOVER_TIME,
        ));
        assert!(registry.hovered_at(start + HOVER_TIME).is_none());
    }
}
