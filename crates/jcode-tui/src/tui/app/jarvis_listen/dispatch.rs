//! Pending voice input. Keep local utterance IDs out of provider prompts.
use super::envelope_trigger;
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(super) struct Request {
    pub id: u64,
    pub text: String,
    pub at: Instant,
    pub provisional: bool,
    pub continuation: bool,
}
#[derive(Default)]
pub(super) struct Dispatch {
    queue: VecDeque<Request>,
    open: VecDeque<(u64, Instant)>,
    seen: VecDeque<u64>,
}
const LIMIT: usize = 16;
impl Dispatch {
    pub fn enqueue(&mut self, id: u64, text: String) -> &'static str {
        if self.seen.contains(&id) {
            return "duplicate";
        }
        if envelope_trigger(&text).is_none() {
            return "invalid";
        }
        if self.queue.len() >= LIMIT {
            return "queue_full";
        }
        let provisional = serde_json::from_str::<Value>(&text)
            .ok()
            .is_some_and(|v| v["still_speaking"] == true);
        self.seen.push_back(id);
        if self.seen.len() > 128 {
            self.seen.pop_front();
        }
        self.queue.push_back(Request {
            id,
            text,
            at: Instant::now(),
            provisional,
            continuation: false,
        });
        "queued"
    }
    pub fn finalize(&mut self, id: u64, text: String) -> &'static str {
        if text.trim().is_empty() || text.len() > 16384 || text.contains('\0') {
            return "invalid_final";
        }
        if let Some(r) = self.queue.iter_mut().find(|r| r.id == id && r.provisional) {
            let Ok(mut v) = serde_json::from_str::<Value>(&r.text) else {
                return "invalid_final";
            };
            v["trigger"] = Value::String(text);
            v.as_object_mut().unwrap().remove("still_speaking");
            r.text = v.to_string();
            r.provisional = false;
            return "merged_final";
        }
        let Some(pos) = self.open.iter().position(|(i, _)| *i == id) else {
            return "unmatched_final";
        };
        let (_, at) = self.open.remove(pos).unwrap();
        self.queue.push_front(Request {id, at, text: format!("(The user finished speaking. Full request: {text:?}. Now acknowledge and complete any necessary actions.)"), provisional:false, continuation:true});
        "queued_final"
    }
    pub fn sent(&mut self, r: &Request) {
        if r.provisional {
            self.open.push_back((r.id, r.at));
        }
    }
    pub fn expire(&mut self, mut log: impl FnMut(u64)) {
        self.queue.retain(|r| {
            let keep = r.continuation || r.at.elapsed() < Duration::from_secs(30);
            if !keep {
                log(r.id)
            }
            keep
        });
        self.open.retain(|(id, at)| {
            let keep = at.elapsed() < Duration::from_secs(180);
            if !keep {
                log(*id)
            }
            keep
        });
    }
    pub fn clear(&mut self, mut log: impl FnMut(u64)) {
        for r in self.queue.drain(..) {
            log(r.id)
        }
        for (id, _) in self.open.drain(..) {
            log(id)
        }
        self.seen.clear();
    }
    pub fn pop(&mut self) -> Option<Request> {
        self.queue.pop_front()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn env(s: &str) -> String {
        serde_json::json!({"trigger":s,"still_speaking":true,"background_transcript":[]})
            .to_string()
    }
    #[test]
    fn overlapping_requests_finalize_by_id_not_position() {
        let mut d = Dispatch::default();
        assert_eq!(d.enqueue(1, env("one")), "queued");
        assert_eq!(d.enqueue(2, env("two")), "queued");
        assert_eq!(d.finalize(2, "two complete".into()), "merged_final");
        let a = d.pop().unwrap();
        d.sent(&a);
        assert_eq!(d.finalize(1, "one complete".into()), "queued_final");
        assert!(d.pop().unwrap().continuation);
        assert_eq!(
            envelope_trigger(&d.pop().unwrap().text).unwrap(),
            "two complete"
        );
        assert_eq!(d.finalize(1, "again".into()), "unmatched_final");
    }
    #[test]
    fn bounded_and_deduplicated_by_id() {
        let mut d = Dispatch::default();
        for id in 0..LIMIT as u64 {
            assert_eq!(d.enqueue(id, env("same")), "queued");
        }
        assert_eq!(d.enqueue(0, env("same")), "duplicate");
        assert_eq!(d.enqueue(64, env("another")), "queue_full");
        d.clear(|_| {});
        assert!(d.pop().is_none());
        assert_eq!(d.enqueue(0, "bad".into()), "invalid");
    }
}
