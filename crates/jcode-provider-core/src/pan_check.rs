//! Local pre-send check for payment card numbers (PANs).
//!
//! Gateway guardrails match card shapes with a regex and no checksum, so a list
//! of four-digit issue numbers reads as a card (measured 2026-09-25, OpenRouter
//! `[CREDIT_CARD]`). This check is stricter: a known issuer prefix, a brand
//! length, a card-style grouping, and a valid Luhn checksum. Callers use it to
//! fail closed before sending and to say *where* the match is, never what it is.

use serde_json::Value;

/// Where a card-number-shaped value was found. Holds no part of the value:
/// `role` is a fixed label, never text copied from the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanFinding {
    pub message_index: usize,
    pub role: &'static str,
}

/// First message in an OpenAI-style `messages` array that contains a PAN.
/// Every string in the message is scanned (content parts, tool arguments,
/// tool output), because the whole message is what leaves the machine.
pub fn find_pan_in_messages(messages: &Value) -> Option<PanFinding> {
    messages
        .as_array()?
        .iter()
        .enumerate()
        .find(|(_, message)| value_contains_pan(message))
        .map(|(message_index, message)| PanFinding {
            message_index,
            role: role_label(message.get("role").and_then(Value::as_str)),
        })
}

/// The request's role mapped onto a closed set, so a crafted role (for example
/// from `extra_body`) cannot carry the blocked value into the error.
fn role_label(role: Option<&str>) -> &'static str {
    const KNOWN: [&str; 6] = [
        "system",
        "developer",
        "user",
        "assistant",
        "tool",
        "function",
    ];
    role.and_then(|role| KNOWN.into_iter().find(|known| *known == role))
        .unwrap_or("unknown")
}

fn value_contains_pan(value: &Value) -> bool {
    match value {
        Value::String(text) => text_contains_pan(text),
        Value::Array(items) => items.iter().any(value_contains_pan),
        Value::Object(map) => map.values().any(value_contains_pan),
        _ => false,
    }
}

/// Whether `text` contains a card number: 15 or 16 contiguous digits, or
/// exactly the 4-4-4-4 or 4-6-5 groups split by one repeated space or dash,
/// not glued to letters or other digits, with an issuer prefix and a valid
/// Luhn checksum. A longer run of groups (an issue list) is not a card.
pub fn text_contains_pan(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() || (i > 0 && bytes[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let mut groups: Vec<usize> = Vec::new();
        let mut digits: Vec<u8> = Vec::new();
        let mut separator: Option<u8> = None;
        let mut j = i;
        loop {
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                digits.push(bytes[j] - b'0');
                j += 1;
            }
            groups.push(j - start);
            let splits = j + 1 < bytes.len()
                && matches!(bytes[j], b' ' | b'-')
                && separator.is_none_or(|sep| sep == bytes[j])
                && bytes[j + 1].is_ascii_digit();
            if !splits {
                break;
            }
            separator = Some(bytes[j]);
            j += 1;
        }
        let glued = j < bytes.len() && bytes[j].is_ascii_alphanumeric();
        if !glued && card_grouping(&groups) && is_card_number(&digits) {
            return true;
        }
        i = j.max(i + 1);
    }
    false
}

fn card_grouping(groups: &[usize]) -> bool {
    // Contiguous runs stop at 16: 13- and 19-digit runs are mostly message
    // ids and u64 hashes in agent transcripts, and real cards of those
    // lengths are rare enough to leave to the gateway guardrail.
    matches!(groups, [15] | [16] | [4, 4, 4, 4] | [4, 6, 5])
}

fn is_card_number(digits: &[u8]) -> bool {
    let prefix = |n: usize| {
        digits
            .iter()
            .take(n)
            .fold(0u32, |acc, d| acc * 10 + u32::from(*d))
    };
    let len = digits.len();
    let brand = match digits.first() {
        Some(4) => matches!(len, 13 | 16 | 19),
        Some(5) => (51..=55).contains(&prefix(2)) && len == 16,
        Some(2) => (2221..=2720).contains(&prefix(4)) && len == 16,
        Some(3) => matches!(prefix(2), 34 | 37) && len == 15,
        Some(6) => (prefix(4) == 6011 || prefix(2) == 65) && (16..=19).contains(&len),
        _ => false,
    };
    brand && luhn_valid(digits)
}

fn luhn_valid(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = u32::from(d);
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Published processor test numbers, assembled at runtime so no literal
    // card number sits in the source (a guardrail reading this file would
    // otherwise block the session that opened it).
    fn visa() -> String {
        "4242".repeat(4)
    }
    fn visa_alt() -> String {
        format!("4{}", "1".repeat(15))
    }
    fn mastercard() -> String {
        format!("{}4444", "5".repeat(12))
    }
    fn amex() -> String {
        ["3782", "822463", "10005"].concat()
    }

    fn grouped(pan: &str, sep: char) -> String {
        let s = sep.to_string();
        pan.as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join(&s)
    }

    #[test]
    fn real_card_shapes_match() {
        for pan in [visa(), visa_alt(), mastercard(), amex()] {
            assert!(text_contains_pan(&format!("card {pan} end")), "contiguous");
        }
        assert!(text_contains_pan(&grouped(&visa(), ' ')));
        assert!(text_contains_pan(&grouped(&mastercard(), '-')));
        assert!(text_contains_pan(&format!("amex {} end.", amex_grouped())));
    }

    fn amex_grouped() -> String {
        ["3782", "822463", "10005"].join(" ")
    }

    #[test]
    fn the_2026_09_25_false_positives_do_not_match() {
        for text in [
            "for n in 1113 1114 1115 1116; do gh issue view $n; done",
            "1487 1489 1354 1356",
            "viewports 1000 1200 1366 1440",
            // Four-digit groups inside a longer list are a list, not a card.
            &format!("prs 1113 {}", grouped(&visa(), ' ')),
            "2026-09-25 10:48:19",
            "request_hash=14816942678214418706",
            &format!("sha 4a3f9e0c1b2d{}aa", visa()),
            // A 19-digit id with a Visa prefix: runs longer than 16 are ids.
            &format!("id {}", "4".repeat(18) + "4"),
        ] {
            assert!(!text_contains_pan(text), "false positive: {text}");
        }
    }

    #[test]
    fn luhn_and_issuer_both_gate() {
        // Correct shape, broken checksum.
        let mut bad = visa();
        bad.replace_range(15..16, "3");
        assert!(!text_contains_pan(&bad));
        // Valid Luhn, no issuer prefix.
        assert!(!text_contains_pan(&"0".repeat(16)));
        // Glued to letters: part of an identifier.
        assert!(!text_contains_pan(&format!("id{}", visa())));
    }

    #[test]
    fn finding_names_the_message_not_the_value() {
        let messages = serde_json::json!([
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": [{"type": "text", "text": "issues 1113 1114 1115 1116"}]},
            {"role": "tool", "tool_call_id": "t1", "content": format!("row: {}", visa())},
        ]);
        let finding = find_pan_in_messages(&messages).expect("tool output holds a PAN");
        assert_eq!(finding.message_index, 2);
        assert_eq!(finding.role, "tool");
        assert!(!format!("{finding:?}").contains("4242"));
    }

    #[test]
    fn a_card_shaped_role_is_never_echoed() {
        // `extra_body` can replace `messages`, so the role is untrusted text.
        let messages = serde_json::json!([{"role": visa(), "content": "hi"}]);
        let finding = find_pan_in_messages(&messages).expect("the role holds a PAN");
        assert_eq!(finding.role, "unknown");
        assert!(!format!("{finding:?}").contains("4242"));
    }
}
