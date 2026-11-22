//! Presentation of locally generated Jarvis requests. Never classify typed text by shape.
use crate::DisplayMessage;
use std::borrow::Cow;

pub const JARVIS_USER_ROLE: &str = "jarvis_user";
const VOICE_TITLE: &str = "Jarvis voice request";
const FINAL_PREFIX: &str = "(The user finished speaking. Full request: ";
const FINAL_SUFFIX: &str = ". Now complete any necessary actions. Your held acknowledgement plays only if the request is unchanged, so speak again only if it no longer fits or you gave none.)";

pub fn jarvis_final_payload(text: &str) -> String {
    format!("{FINAL_PREFIX}{text:?}{FINAL_SUFFIX}")
}

impl DisplayMessage {
    pub fn jarvis_user(content: impl Into<String>) -> Self {
        Self::user(content).with_title(VOICE_TITLE)
    }

    /// Preserve ordinary user turn accounting and the untouched wire payload.
    pub fn normalize_jarvis_role(&mut self) {
        if self.role == JARVIS_USER_ROLE {
            self.role = "user".into();
            self.title = Some(VOICE_TITLE.into());
        }
    }

    pub fn is_jarvis_user(&self) -> bool {
        self.role == "user" && self.title.as_deref() == Some(VOICE_TITLE)
    }

    pub fn user_row_text(&self, detailed: bool) -> Cow<'_, str> {
        if !self.is_jarvis_user() {
            return Cow::Borrowed(&self.content);
        }
        #[derive(serde::Deserialize)]
        struct Envelope {
            trigger: String,
            background_transcript: Vec<String>,
            #[serde(default)]
            still_speaking: bool,
        }
        let (transcript, status, context, instructions) =
            if let Ok(e) = serde_json::from_str::<Envelope>(&self.content) {
                (
                    e.trigger,
                    if e.still_speaking {
                        "Interim (still speaking)"
                    } else {
                        "Complete"
                    },
                    e.background_transcript,
                    None,
                )
            } else if let Some((text, instruction)) = parse_final(&self.content) {
                (text, "Final", vec![], Some(instruction))
            } else {
                return Cow::Borrowed(&self.content);
            };
        if !detailed {
            return Cow::Owned(transcript);
        }
        let mut view = format!("Transcript:\n{transcript}\n\nStatus: {status}");
        let context = context
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if !context.is_empty() {
            view.push_str("\n\nBackground:\n");
            view.push_str(&context.join("\n"));
        }
        if let Some(instructions) = instructions {
            view.push_str("\n\nInstructions:\n");
            view.push_str(instructions);
        }
        Cow::Owned(view)
    }
}

fn parse_final(content: &str) -> Option<(String, &'static str)> {
    let body = content.strip_prefix(FINAL_PREFIX)?;
    for suffix in [
        FINAL_SUFFIX,
        ". Now acknowledge and complete any necessary actions.)",
    ] {
        if let Some(quoted) = body.strip_suffix(suffix) {
            let text = decode_debug_string(quoted)
                .or_else(|| Some(quoted.strip_prefix('"')?.strip_suffix('"')?.to_owned()))?;
            return Some((text, suffix.strip_prefix(". ")?.strip_suffix(')')?));
        }
    }
    None
}

// Finals historically use Rust's quoted Debug spelling, not JSON (notably
// \\0 and \\u{...}). Keep the model payload byte-for-byte compatible.
fn decode_debug_string(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.strip_suffix('"')?.chars();
    let mut result = String::new();
    while let Some(ch) = chars.next() {
        result.push(if ch != '\\' {
            ch
        } else {
            match chars.next()? {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '0' => '\0',
                ch @ ('\\' | '"' | '\'') => ch,
                'u' if chars.next()? == '{' => {
                    let mut value = 0u32;
                    loop {
                        let ch = chars.next()?;
                        if ch == '}' {
                            break;
                        }
                        value = value.checked_mul(16)?.checked_add(ch.to_digit(16)?)?;
                    }
                    char::from_u32(value)?
                }
                _ => return None,
            }
        });
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_final_and_multiline_background_keep_their_shape() {
        let old = "(The user finished speaking. Full request: \"yes\". Now acknowledge and complete any necessary actions.)";
        let old = DisplayMessage::jarvis_user(old);
        assert_eq!(
            old.user_row_text(true),
            "Transcript:\nyes\n\nStatus: Final\n\nInstructions:\nNow acknowledge and complete any necessary actions."
        );
        let multi = DisplayMessage::jarvis_user(
            serde_json::json!({"background_transcript":["one","two\nthree",""],"trigger":"go"})
                .to_string(),
        );
        assert_eq!(
            multi.user_row_text(true),
            "Transcript:\ngo\n\nStatus: Complete\n\nBackground:\none\ntwo\nthree"
        );
    }
    #[test]
    fn only_trusted_rows_project_and_raw_payload_survives() {
        let spoken = "check \"one\"\\two\n\t\u{1b} 👋";
        for payload in [
            serde_json::json!({"trigger":spoken,"background_transcript":["private"]}).to_string(),
            serde_json::json!({"trigger":spoken,"still_speaking":true,"background_transcript":[]})
                .to_string(),
            jarvis_final_payload(spoken),
        ] {
            let typed = DisplayMessage::user(&payload);
            assert_eq!(typed.user_row_text(false), payload);
            let mut replay = DisplayMessage::user(&payload);
            replay.role = JARVIS_USER_ROLE.into();
            replay.normalize_jarvis_role();
            assert_eq!(replay.role, "user");
            assert_eq!(replay.user_row_text(false), spoken);
            let detail = replay.user_row_text(true);
            assert!(detail.starts_with(&format!("Transcript:\n{spoken}\n\nStatus: ")));
            if payload.starts_with('(') {
                assert!(detail.contains("Status: Final\n\nInstructions:\nNow complete"));
                assert!(!detail.ends_with(')'));
            } else if payload.contains("private") {
                assert!(detail.ends_with("Status: Complete\n\nBackground:\nprivate"));
            } else {
                assert!(detail.ends_with("Status: Interim (still speaking)"));
            }
            assert_eq!(typed.user_row_text(true), payload);
            assert_eq!(replay.content, payload);
        }
    }
}
