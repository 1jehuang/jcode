//! Frozen repair oracles before unaffected-content COW optimization.
use super::*;

fn msg(role: Role, content: Vec<ContentBlock>) -> Message {
    let mut msg = Message::user("metadata timestamp");
    msg.role = role;
    msg.content = content.into();
    msg.tool_duration_ms = Some(91);
    msg
}
fn call(id: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: "tool".into(),
        thought_signature: Some("signature".into()),
        input: serde_json::json!({"x":"payload"}),
    }
}
fn result(id: &str, placeholder: bool) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.into(),
        content: if placeholder {
            TOOL_OUTPUT_MISSING_TEXT.into()
        } else {
            "real result".into()
        },
        is_error: Some(placeholder),
    }
}
fn text() -> ContentBlock {
    ContentBlock::Text {
        text: "large unchanged history 🦀".repeat(2048),
        cache_control: None,
    }
}
type Repair = fn(&[Message]) -> Vec<Message>;
fn repairs() -> [(&'static str, Repair, Repair); 3] {
    [
        ("hoist", legacy_hoist, hoist_late_tool_results),
        ("uses", legacy_dedupe_uses, dedupe_tool_uses),
        ("results", legacy_dedupe_results, dedupe_tool_results),
    ]
}
fn parity(history: &[Message]) {
    for (name, old, new) in repairs() {
        assert_eq!(
            serde_json::to_value(old(history)).unwrap(),
            serde_json::to_value(new(history)).unwrap(),
            "{name}"
        );
    }
}
#[test]
fn frozen_oracles_duplicate_attachment_empty_and_metadata_parity() {
    let blocks = [
        call("a"),
        call("b"),
        result("a", false),
        result("a", true),
        result("b", false),
        ContentBlock::Image {
            media_type: "image/png".into(),
            data: "encoded".into(),
        },
        ContentBlock::Text {
            text: "[Attached image associated with the preceding tool result: image]".into(),
            cache_control: None,
        },
        ContentBlock::ToolReference {
            tool_use_id: "a".into(),
            tool_name: "ref".into(),
        },
        ContentBlock::Text {
            text: "separator".into(),
            cache_control: None,
        },
    ];
    parity(&[]);
    let mut seed = 17u64;
    for _ in 0..1000 {
        let mut history = Vec::new();
        for _ in 0..7 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let role = if seed & 1 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            let count = (seed >> 32) as usize % 6;
            let mut content = Vec::new();
            for _ in 0..count {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                content.push(blocks[(seed >> 32) as usize % blocks.len()].clone());
            }
            history.push(msg(role, content));
        }
        parity(&history);
    }
}
#[test]
fn changed_and_unchanged_payload_sharing() {
    for (name, _, repair) in repairs() {
        let mut history = vec![
            msg(Role::Assistant, vec![call("a")]),
            msg(Role::User, vec![text()]),
            msg(Role::Assistant, vec![text()]),
            msg(Role::User, vec![text()]),
        ];
        match name {
            "hoist" => history.push(msg(Role::User, vec![result("a", false)])),
            "uses" => history.push(msg(Role::Assistant, vec![call("a")])),
            _ => {
                history[1].content = vec![result("a", false)].into();
                history.push(msg(Role::User, vec![result("a", true)]));
            }
        }
        parity(&history);
        let out = repair(&history);
        for original in &history[..4] {
            assert!(
                out.iter()
                    .any(|m| m.content.shares_storage_with(&original.content)),
                "{name}: unchanged content detached"
            );
        }
        assert_eq!(history.last().unwrap().content.len(), 1);
    }
}

#[test]
fn dedupe_keeps_empty_messages_and_detaches_only_changed_payloads() {
    for (old, repair, first, duplicate) in [
        (
            legacy_dedupe_uses as Repair,
            dedupe_tool_uses as Repair,
            call("a"),
            call("a"),
        ),
        (
            legacy_dedupe_results as Repair,
            dedupe_tool_results as Repair,
            result("a", false),
            result("a", true),
        ),
    ] {
        let history = vec![
            msg(Role::Assistant, vec![]),
            msg(Role::Assistant, vec![first]),
            msg(Role::User, vec![duplicate, text()]),
            msg(Role::User, vec![]),
        ];
        let output = repair(&history);
        assert_eq!(
            serde_json::to_value(&output).unwrap(),
            serde_json::to_value(old(&history)).unwrap()
        );
        assert_eq!(output.len(), 4);
        for i in [0, 1, 3] {
            assert!(output[i].content.shares_storage_with(&history[i].content));
        }
        assert!(!output[2].content.shares_storage_with(&history[2].content));
        assert_eq!(history[2].content.len(), 2);
        assert_eq!(output[2].content.len(), 1);
    }
}

#[cfg(target_os = "linux")]
#[path = "repair_cow_probe.rs"]
mod probe;

fn legacy_hoist(messages: &[Message]) -> Vec<Message> {
    use std::collections::{HashMap, HashSet};

    // Assistant message index that made each call.
    let mut call_at: HashMap<&str, usize> = HashMap::new();
    for (mi, msg) in messages.iter().enumerate() {
        if matches!(msg.role, Role::Assistant) {
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    call_at.insert(id, mi);
                }
            }
        }
    }

    // Results that are not in the message right after their call. Consecutive
    // messages of one role are merged into one later, so compare "turn"
    // positions: parallel calls stored as one user message per result, and
    // back-to-back assistant messages, each collapse into a single turn.
    let turn: Vec<usize> = {
        let mut t = 0usize;
        let mut prev: Option<bool> = None;
        messages
            .iter()
            .map(|m| {
                let is_user = matches!(m.role, Role::User);
                if prev.is_some_and(|p| p != is_user) {
                    t += 1;
                }
                prev = Some(is_user);
                t
            })
            .collect()
    };
    let mut late: HashSet<(usize, usize)> = HashSet::new();
    let mut moved: HashMap<usize, Vec<ContentBlock>> = HashMap::new();
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                continue;
            };
            let Some(&call) = call_at.get(tool_use_id.as_str()) else {
                continue; // no call anywhere: nothing to pair with
            };
            let in_place = turn[mi] == turn[call] + 1;
            if !in_place {
                // Insert after the last message of the call's turn, so a run
                // of assistant messages stays one turn.
                let anchor = (call..messages.len())
                    .take_while(|&i| turn[i] == turn[call])
                    .last()
                    .unwrap_or(call);
                let dest = moved.entry(anchor).or_default();
                for j in std::iter::once(bi).chain(attached_to_result(&msg.content, bi)) {
                    late.insert((mi, j));
                    dest.push(msg.content[j].clone());
                }
            }
        }
    }
    if late.is_empty() {
        return messages.to_vec();
    }
    jcode_logging::warn(&format!(
        "[anthropic] Moved {} late tool_result block(s) up to directly follow their tool_use",
        late.len()
    ));

    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + moved.len());
    for (mi, msg) in messages.iter().enumerate() {
        let content: Vec<ContentBlock> = msg
            .content
            .iter()
            .enumerate()
            .filter(|(bi, _)| !late.contains(&(mi, *bi)))
            .map(|(_, b)| b.clone())
            .collect();
        if !content.is_empty() {
            out.push(Message {
                content: content.into(),
                ..msg.clone()
            });
        }
        if let Some(results) = moved.remove(&mi) {
            // Same-role merging later folds this into the next user message
            // when there is one, keeping the results first.
            out.push(Message {
                role: Role::User,
                content: results.into(),
                timestamp: msg.timestamp,
                tool_duration_ms: None,
            });
        }
    }
    out
}

fn legacy_dedupe_uses(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashSet;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut duplicate_seen = false;
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, .. } = block
                && !seen.insert(id.as_str())
            {
                duplicate_seen = true;
            }
        }
    }
    if !duplicate_seen {
        return messages.to_vec();
    }

    let mut kept: HashSet<String> = HashSet::new();
    let mut dropped = 0usize;
    let out = messages
        .iter()
        .map(|msg| {
            let mut msg = msg.clone();
            msg.content.retain(|block| match block {
                ContentBlock::ToolUse { id, .. } => {
                    let keep = kept.insert(id.clone());
                    if !keep {
                        dropped += 1;
                    }
                    keep
                }
                _ => true,
            });
            msg
        })
        .collect();
    jcode_logging::warn(&format!(
        "[anthropic] Dropped {dropped} repeated tool_use block(s); each tool_use id may appear only once"
    ));
    out
}

/// Remove duplicate `tool_result` blocks so each `tool_use_id` is answered
/// exactly once, preferring real output over a synthetic placeholder.
/// Messages left with no content at all are dropped by the caller's
/// `!content.is_empty()` guard.
fn legacy_dedupe_results(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashMap;

    // Winner position per tool_use_id: the first real result if one exists,
    // otherwise the first occurrence at all.
    let mut winner: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut winner_is_real: HashMap<&str, bool> = HashMap::new();
    let mut duplicate_seen = false;

    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            else {
                continue;
            };
            let real = !is_placeholder_tool_result(content, *is_error);
            match winner_is_real.get(tool_use_id.as_str()) {
                None => {
                    winner.insert(tool_use_id, (mi, bi));
                    winner_is_real.insert(tool_use_id, real);
                }
                Some(false) if real => {
                    // Upgrade a placeholder winner to the real output.
                    winner.insert(tool_use_id, (mi, bi));
                    winner_is_real.insert(tool_use_id, true);
                    duplicate_seen = true;
                }
                Some(_) => duplicate_seen = true,
            }
        }
    }

    if !duplicate_seen {
        return messages.to_vec();
    }

    let dropped = std::cell::Cell::new(0usize);
    let out: Vec<Message> = messages
        .iter()
        .enumerate()
        .map(|(mi, msg)| {
            let mut msg = msg.clone();
            let mut bi = 0usize;
            msg.content.retain(|block| {
                let index = bi;
                bi += 1;
                let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                    return true;
                };
                let keep = winner.get(tool_use_id.as_str()) == Some(&(mi, index));
                if !keep {
                    dropped.set(dropped.get() + 1);
                }
                keep
            });
            msg
        })
        .collect();

    if dropped.get() > 0 {
        jcode_logging::warn(&format!(
            "[anthropic] Dropped {} duplicate tool_result block(s); each tool_use_id may be \
             answered only once",
            dropped.get()
        ));
    }
    out
}
