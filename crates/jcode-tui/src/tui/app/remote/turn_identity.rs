//! Wire identity fence. Only an attachment snapshot may replace a known runtime.
use crate::protocol::TurnIdentity;
use std::collections::HashSet;

#[derive(Default)]
pub(in crate::tui::app) struct TurnFence {
    session_id: Option<String>,
    generation: Option<String>,
    retired: HashSet<String>,
    latest: u64,
    completed: u64,
    pub(super) active: Option<TurnIdentity>,
}

impl TurnFence {
    pub(super) fn scoped(&self) -> bool {
        self.generation.is_some()
    }

    pub(super) fn bootstrap(
        &mut self,
        session_id: String,
        generation: String,
        latest: u64,
        active: Option<u64>,
    ) -> bool {
        if self
            .session_id
            .as_ref()
            .is_some_and(|current| current != &session_id)
        {
            *self = Self::default();
        }
        self.session_id = Some(session_id);
        if self.retired.contains(&generation) {
            return false;
        }
        if self.generation.as_ref() != Some(&generation) {
            if let Some(old) = self.generation.replace(generation.clone()) {
                self.retired.insert(old);
            }
            self.latest = 0;
            self.completed = 0;
            self.active = None;
        }
        if latest < self.latest {
            return false;
        }
        self.latest = latest;
        self.active = active
            .filter(|id| *id > self.completed)
            .map(|turn_id| TurnIdentity {
                runtime_generation: generation,
                turn_id,
            });
        if self.active.is_none() {
            self.completed = self.completed.max(latest);
        }
        true
    }

    pub(super) fn start(&mut self, identity: &TurnIdentity) -> bool {
        if self.retired.contains(&identity.runtime_generation) {
            return false;
        }
        if let Some(generation) = &self.generation {
            if generation != &identity.runtime_generation {
                return false;
            }
        } else {
            self.generation = Some(identity.runtime_generation.clone());
        }
        if identity.turn_id < self.latest || identity.turn_id <= self.completed {
            return false;
        }
        self.latest = identity.turn_id;
        self.active = Some(identity.clone());
        true
    }

    pub(super) fn accepts(&self, identity: &TurnIdentity) -> bool {
        self.active.as_ref() == Some(identity)
    }

    pub(super) fn is_latest(&self, identity: &TurnIdentity) -> bool {
        self.generation.as_ref() == Some(&identity.runtime_generation)
            && self.latest == identity.turn_id
    }

    pub(super) fn complete(&mut self, identity: &TurnIdentity) {
        if self.accepts(identity) {
            self.completed = self.completed.max(identity.turn_id);
            self.active = None;
        }
    }
}
