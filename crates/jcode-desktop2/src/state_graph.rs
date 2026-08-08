//! Executable state-space model for the desktop2 product lifecycle.
//!
//! This is deliberately orthogonal to rendering. It names the product states
//! a user can observe, defines the legal edges between them, and lets the real
//! app record those edges as window, connection, session, prompt, overlay, and
//! niri-motion events occur.

use crate::workspace::{Direction, MotionEvent, MotionState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowState {
    #[default]
    Cold,
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    #[default]
    Offline,
    Connecting,
    Online,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SessionState {
    #[default]
    Detached,
    Creating,
    Attaching,
    Attached,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TurnState {
    #[default]
    Idle,
    /// Written to the ordered command channel, not yet acknowledged.
    Sending,
    /// Accepted or producing activity/output.
    Working,
    /// The current turn is working and at least one later prompt is queued.
    WorkingQueued,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SurfaceState {
    #[default]
    Conversation,
    Overview,
    Resume,
    ModelPicker,
}

/// One node in the product state space. Independent axes stay independent:
/// opening settings does not erase that a turn is working, and moving a niri
/// column does not change which overlay owns input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct State {
    pub window: WindowState,
    pub connection: ConnectionState,
    pub session: SessionState,
    pub turn: TurnState,
    pub surface: SurfaceState,
    pub settings_open: bool,
    pub booting: bool,
    pub motion: MotionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    WindowOpened,
    WindowClosed,
    ConnectStarted,
    Connected,
    ConnectionFailed,
    NewSessionRequested,
    AttachRequested,
    SessionAttached,
    PromptSubmitted,
    PromptQueued,
    PromptAccepted,
    AgentActivity,
    TurnFinished { queued: bool },
    SurfaceOpened(SurfaceState),
    SurfaceClosed,
    SettingsChanged(bool),
    BootChanged(bool),
    Move(Direction),
    MotionSettled,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Graph {
    state: State,
    transitions: u64,
}

impl Graph {
    pub fn state(&self) -> State {
        self.state
    }

    pub fn transitions(&self) -> u64 {
        self.transitions
    }

    /// Apply one deterministic graph edge. Events are total so asynchronous
    /// duplicates (two activity packets, a repeated attach acknowledgement)
    /// are harmless self-edges rather than panics.
    pub fn apply(&mut self, event: Event) -> State {
        // Runtime callbacks may be duplicated or arrive after teardown. Keep
        // the reducer total by treating events outside an open window as
        // harmless self-edges. Opening and closing remain idempotent.
        if self.state.window != WindowState::Open
            && !matches!(event, Event::WindowOpened | Event::WindowClosed)
        {
            self.transitions = self.transitions.saturating_add(1);
            return self.state;
        }

        let mut next = self.state;
        match event {
            Event::WindowOpened => {
                next.window = WindowState::Open;
                next.booting = true;
            }
            Event::WindowClosed => {
                next.window = WindowState::Closed;
                next.connection = ConnectionState::Offline;
                next.session = SessionState::Detached;
                next.turn = TurnState::Idle;
                next.surface = SurfaceState::Conversation;
                next.settings_open = false;
                next.booting = false;
                next.motion = MotionState::Settled;
            }
            Event::ConnectStarted => {
                next.connection = ConnectionState::Connecting;
                next.session = SessionState::Detached;
                next.turn = TurnState::Idle;
            }
            Event::Connected => next.connection = ConnectionState::Online,
            Event::ConnectionFailed => {
                next.connection = ConnectionState::Failed;
                next.session = SessionState::Detached;
                next.turn = TurnState::Idle;
            }
            Event::NewSessionRequested => {
                next.session = SessionState::Creating;
                next.turn = TurnState::Idle;
            }
            Event::AttachRequested => {
                next.session = SessionState::Attaching;
                next.turn = TurnState::Idle;
            }
            Event::SessionAttached => {
                next.connection = ConnectionState::Online;
                next.session = SessionState::Attached;
            }
            Event::PromptSubmitted if next.session == SessionState::Attached => {
                next.turn = TurnState::Sending;
            }
            Event::PromptQueued if next.session == SessionState::Attached => {
                next.turn = TurnState::WorkingQueued;
            }
            Event::PromptAccepted | Event::AgentActivity => {
                if next.session == SessionState::Attached {
                    next.turn = match next.turn {
                        TurnState::WorkingQueued => TurnState::WorkingQueued,
                        _ => TurnState::Working,
                    };
                }
            }
            Event::TurnFinished { queued } => {
                if next.session == SessionState::Attached {
                    next.turn = if queued {
                        TurnState::Sending
                    } else {
                        TurnState::Idle
                    };
                }
            }
            Event::SurfaceOpened(surface) => next.surface = surface,
            Event::SurfaceClosed => next.surface = SurfaceState::Conversation,
            Event::SettingsChanged(open) => next.settings_open = open,
            Event::BootChanged(booting) => next.booting = booting,
            Event::Move(direction) => {
                next.motion = next.motion.next(MotionEvent::Move(direction));
            }
            Event::MotionSettled => {
                next.motion = next.motion.next(MotionEvent::Settle);
            }
            Event::PromptSubmitted | Event::PromptQueued => {}
        }
        self.state = next;
        self.transitions = self.transitions.saturating_add(1);
        next
    }

    /// Cross-axis invariants that define reachable product states.
    pub fn validate(&self) -> Result<(), &'static str> {
        let state = self.state;
        if state.window != WindowState::Open {
            let expected = State {
                window: state.window,
                ..State::default()
            };
            if state != expected {
                return Err("inactive window retained product state");
            }
        }
        if matches!(
            state.turn,
            TurnState::Sending | TurnState::Working | TurnState::WorkingQueued
        ) && state.session != SessionState::Attached
        {
            return Err("turn exists without an attached session");
        }
        if state.session == SessionState::Attached && state.connection != ConnectionState::Online {
            return Err("attached session is not online");
        }
        Ok(())
    }
}

impl crate::App {
    /// Fold independently-owned UI substates into the product graph. Overlay
    /// modules remain small and pure; this single frame-boundary projection
    /// prevents a close-via-Escape and a close-via-pointer from drifting into
    /// different graph nodes.
    pub(crate) fn reconcile_state_graph(&mut self) {
        let surface = if self.model.overview.is_visible() {
            SurfaceState::Overview
        } else if self.model.resume.is_open() {
            SurfaceState::Resume
        } else if self.model.model_picker.is_visible() {
            SurfaceState::ModelPicker
        } else {
            SurfaceState::Conversation
        };
        if self.state_graph.state().surface != surface {
            self.state_graph
                .apply(if surface == SurfaceState::Conversation {
                    Event::SurfaceClosed
                } else {
                    Event::SurfaceOpened(surface)
                });
        }

        let settings_open = self.model.panel.is_open();
        if self.state_graph.state().settings_open != settings_open {
            self.state_graph
                .apply(Event::SettingsChanged(settings_open));
        }

        let booting = self.model.boot.is_running();
        if self.state_graph.state().booting != booting {
            self.state_graph.apply(Event::BootChanged(booting));
        }

        let motion = self.model.workspace.state();
        if self.state_graph.state().motion != motion {
            match motion.direction() {
                Some(direction) => {
                    self.state_graph.apply(Event::Move(direction));
                }
                None => {
                    self.state_graph.apply(Event::MotionSettled);
                }
            }
        }

        debug_assert!(
            self.state_graph.validate().is_ok(),
            "invalid desktop2 state-space node: {:?}",
            self.state_graph.state()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};

    // Concrete representatives of every Event variant and every finite payload
    // value. Keep this adjacent to Event so additions cannot be overlooked in
    // review without also making this count and the exhaustive test conspicuous.
    const ALL_EVENTS: [Event; 28] = [
        Event::WindowOpened,
        Event::WindowClosed,
        Event::ConnectStarted,
        Event::Connected,
        Event::ConnectionFailed,
        Event::NewSessionRequested,
        Event::AttachRequested,
        Event::SessionAttached,
        Event::PromptSubmitted,
        Event::PromptQueued,
        Event::PromptAccepted,
        Event::AgentActivity,
        Event::TurnFinished { queued: false },
        Event::TurnFinished { queued: true },
        Event::SurfaceOpened(SurfaceState::Conversation),
        Event::SurfaceOpened(SurfaceState::Overview),
        Event::SurfaceOpened(SurfaceState::Resume),
        Event::SurfaceOpened(SurfaceState::ModelPicker),
        Event::SurfaceClosed,
        Event::SettingsChanged(false),
        Event::SettingsChanged(true),
        Event::BootChanged(false),
        Event::BootChanged(true),
        Event::Move(Direction::Left),
        Event::Move(Direction::Right),
        Event::Move(Direction::Up),
        Event::Move(Direction::Down),
        Event::MotionSettled,
    ];

    #[test]
    fn complete_user_journey_is_a_valid_path() {
        let mut graph = Graph::default();
        for event in [
            Event::WindowOpened,
            Event::ConnectStarted,
            Event::Connected,
            Event::NewSessionRequested,
            Event::SessionAttached,
            Event::PromptSubmitted,
            Event::PromptAccepted,
            Event::AgentActivity,
            Event::PromptQueued,
            Event::Move(Direction::Right),
            Event::MotionSettled,
            Event::TurnFinished { queued: true },
            Event::PromptAccepted,
            Event::TurnFinished { queued: false },
            Event::WindowClosed,
        ] {
            graph.apply(event);
            graph.validate().unwrap();
        }
        assert_eq!(graph.state().window, WindowState::Closed);
    }

    #[test]
    fn every_surface_and_motion_is_reachable_without_destroying_other_axes() {
        let mut graph = Graph::default();
        for event in [
            Event::WindowOpened,
            Event::ConnectStarted,
            Event::SessionAttached,
            Event::PromptSubmitted,
            Event::AgentActivity,
        ] {
            graph.apply(event);
        }
        for surface in [
            SurfaceState::Conversation,
            SurfaceState::Overview,
            SurfaceState::Resume,
            SurfaceState::ModelPicker,
        ] {
            graph.apply(Event::SurfaceOpened(surface));
            for direction in [
                Direction::Left,
                Direction::Right,
                Direction::Up,
                Direction::Down,
            ] {
                let state = graph.apply(Event::Move(direction));
                assert_eq!(state.surface, surface);
                assert_eq!(state.turn, TurnState::Working);
                assert_eq!(state.motion.direction(), Some(direction));
            }
        }
        graph.validate().unwrap();
    }

    #[test]
    fn invalid_cross_axis_nodes_are_rejected() {
        let mut graph = Graph::default();
        graph.state.window = WindowState::Open;
        graph.state.turn = TurnState::Sending;
        assert_eq!(
            graph.validate(),
            Err("turn exists without an attached session")
        );
    }

    #[test]
    fn every_reachable_node_and_edge_is_total_and_valid() {
        let initial = Graph::default();
        let mut seen = HashSet::from([initial.state()]);
        let mut pending = VecDeque::from([initial.state()]);
        let mut checked_edges = 0usize;

        while let Some(state) = pending.pop_front() {
            for event in ALL_EVENTS {
                let mut graph = Graph {
                    state,
                    transitions: 0,
                };
                let next = graph.apply(event);
                assert_eq!(graph.transitions(), 1, "event was not a total edge");
                graph.validate().unwrap_or_else(|error| {
                    panic!("invalid reachable edge {state:?} --{event:?}--> {next:?}: {error}")
                });
                checked_edges += 1;
                if seen.insert(next) {
                    pending.push_back(next);
                }
            }
        }

        assert!(seen.len() > 1_000, "exploration was unexpectedly shallow");
        assert_eq!(checked_edges, seen.len() * ALL_EVENTS.len());
    }

    #[test]
    fn every_motion_edge_converges_when_settled() {
        let mut graph = Graph::default();
        graph.apply(Event::WindowOpened);
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            graph.apply(Event::Move(direction));
            assert_eq!(graph.state().motion.direction(), Some(direction));
            graph.apply(Event::MotionSettled);
            assert_eq!(graph.state().motion, MotionState::Settled);
        }
    }
}
