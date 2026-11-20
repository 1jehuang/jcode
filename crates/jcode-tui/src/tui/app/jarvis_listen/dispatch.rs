//! Pending voice input. Keep local utterance IDs out of provider prompts.
use super::envelope_trigger;
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(super) fn trace(session: Option<&str>, id: u64, stage: &str) {
    crate::logging::info(&format!(
        "JARVIS_DISPATCH session={} utterance={id} stage={stage} monotonic_ms={}",
        session.unwrap_or("none"),
        jcode_voice::timing::monotonic_ms()
    ));
}

pub(super) async fn submit(
    app: &mut super::App,
    remote: &mut super::RemoteConnection,
    text: &str,
    interleave: bool,
) -> anyhow::Result<u64> {
    if interleave {
        let id = remote
            .soft_interrupt(text.to_owned(), vec![], false)
            .await?;
        app.track_pending_soft_interrupt(id, text.to_owned());
        Ok(id)
    } else {
        app.push_display_message(super::DisplayMessage::user(text.to_owned()));
        app.begin_remote_send(remote, text.to_owned(), vec![], false)
            .await
    }
}

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
        if self.queue.len() + self.open.len() >= LIMIT {
            return "queue_full";
        }
        let provisional =
            serde_json::from_str::<Value>(&text).is_ok_and(|v| v["still_speaking"] == true);
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
            let Ok(Value::Object(mut v)) = serde_json::from_str::<Value>(&r.text) else {
                return "invalid_final";
            };
            v.insert("trigger".into(), Value::String(text));
            v.remove("still_speaking");
            r.text = Value::Object(v).to_string();
            r.provisional = false;
            return "merged_final";
        }
        let Some(pos) = self.open.iter().position(|(i, _)| *i == id) else {
            return "unmatched_final";
        };
        self.open.remove(pos);
        self.queue.push_back(Request {id, at: Instant::now(), text: format!("(The user finished speaking. Full request: {text:?}. Now complete any necessary actions. Your held acknowledgement plays only if the request is unchanged, so speak again only if it no longer fits or you gave none.)"), provisional:false, continuation:true});
        "queued_final"
    }
    pub fn sent(&mut self, r: &Request) {
        if r.provisional {
            self.open.push_back((r.id, r.at));
        }
    }
    pub fn expire(&mut self, mut log: impl FnMut(u64)) {
        self.queue.retain(|r| {
            let keep = r.at.elapsed() < Duration::from_secs(30);
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
        assert_eq!(
            envelope_trigger(&d.pop().unwrap().text).unwrap(),
            "two complete"
        );
        assert!(d.pop().unwrap().continuation);
        assert_eq!(d.finalize(1, "again".into()), "unmatched_final");
    }
    #[test]
    fn expired_requests_and_orphaned_partials_are_reported_once() {
        let mut d = Dispatch::default();
        d.enqueue(5, env("one"));
        d.enqueue(6, env("two"));
        let mut first = d.pop().unwrap();
        first.at -= Duration::from_secs(181);
        d.sent(&first);
        d.queue.front_mut().unwrap().at -= Duration::from_secs(31);
        let mut expired = Vec::new();
        d.expire(|id| expired.push(id));
        expired.sort();
        assert_eq!(expired, [5, 6]);
        d.expire(|_| panic!("duplicate expiry"));
        assert_eq!(d.finalize(5, "late".into()), "unmatched_final");
    }
    #[test]
    fn busy_agent_receives_both_identical_phrases_as_separate_wire_requests() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut app = crate::tui::app::tests::create_test_app();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let mut remote = super::super::RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let mut lines = BufReader::new(peer).lines();
            let mut queue = Dispatch::default();
            let text = env("check that");
            for id in [71, 72] {
                assert_eq!(queue.enqueue(id, text.clone()), "queued");
            }
            let mut ids = vec![];
            while let Some(r) = queue.pop() {
                let id = submit(&mut app, &mut remote, &r.text, true).await.unwrap();
                ids.push(id);
                let record = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let v: Value = serde_json::from_str(&record).unwrap();
                assert_eq!(v["content"], text);
                assert_eq!(v["id"], id);
            }
            assert_eq!(ids.len(), 2);
            assert_ne!(ids[0], ids[1]);
            let tracked: Vec<_> = app
                .pending_soft_interrupt_requests
                .iter()
                .map(|(id, _)| *id)
                .collect();
            assert_eq!(tracked, ids);
        });
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
