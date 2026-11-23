//! Session-local, opt-in retention. Nothing in this module writes to disk or network.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
const MAX_SAMPLES: usize = 16_000 * 20;
const TTL: Duration = Duration::from_secs(10 * 60);
#[derive(Default)]
pub(super) struct History {
    enabled: bool,
    generation: u64,
    clips: VecDeque<Clip>,
}
struct Clip {
    id: u64,
    at: Instant,
    text: String,
    pcm: Vec<i16>,
}
#[derive(Default)]
pub(super) struct Candidate(Option<(u64, Vec<i16>)>);
impl Candidate {
    pub fn append(&mut self, samples: &[i16]) {
        if let Some((_, pcm)) = self.0.as_mut() {
            if samples.len() > MAX_SAMPLES - pcm.len() {
                self.0 = None;
            } else {
                pcm.extend_from_slice(samples);
            }
        }
    }
}
impl History {
    pub fn reset(&mut self, enabled: Option<bool>) {
        self.generation = self.generation.wrapping_add(1);
        self.clips.clear();
        if let Some(on) = enabled {
            self.enabled = on;
        }
    }
    pub fn begin(&self) -> Candidate {
        Candidate(self.enabled.then(|| (self.generation, Vec::new())))
    }
    pub fn accept(&mut self, candidate: Candidate, id: u64, text: &str) {
        let Some((generation, pcm)) = candidate.0 else {
            return;
        };
        if !self.enabled
            || generation != self.generation
            || pcm.is_empty()
            || text.trim().is_empty()
        {
            return;
        }
        self.expire();
        let clip = Clip {
            id,
            at: Instant::now(),
            text: text.chars().take(4096).collect(),
            pcm,
        };
        let index = self.clips.partition_point(|c| c.id > id);
        self.clips.insert(index, clip);
        self.clips.truncate(3);
    }
    fn expire(&mut self) {
        self.clips.retain(|c| c.at.elapsed() < TTL);
    }
    pub fn status(&mut self) -> (bool, Vec<u64>) {
        self.expire();
        (self.enabled, self.clips.iter().map(|c| c.id).collect())
    }
    pub fn clip(&mut self, id: u64) -> Option<(Vec<i16>, String)> {
        self.expire();
        let c = self.clips.iter().find(|c| c.id == id)?;
        Some((c.pcm.clone(), c.text.clone()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opt_in_caps_expiry_and_inflight_invalidation() {
        let mut h = History::default();
        assert!(h.begin().0.is_none());
        h.reset(Some(true));
        let mut stale = h.begin();
        stale.append(&[1]);
        h.reset(None);
        h.accept(stale, 99, "stale");
        let mut large = h.begin();
        large.append(&vec![1; MAX_SAMPLES]);
        large.append(&[2]);
        h.accept(large, 98, "over");
        for id in [3, 1, 4, 2] {
            let mut c = h.begin();
            c.append(&[id as i16]);
            h.accept(c, id, "final");
        }
        assert_eq!(h.status().1, [4, 3, 2]);
        assert_eq!(h.clip(3).unwrap(), (vec![3], "final".into()));
        h.clips[0].at -= TTL;
        assert_eq!(h.status().1, [3, 2]);
        h.reset(Some(false));
        assert_eq!(h.status(), (false, vec![]));
    }
}
