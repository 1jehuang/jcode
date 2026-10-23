use crate::message::{ContentBlock, Role};
use crate::session::StoredMessage;

fn is_legacy_peer_notification(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("[NOTIFICATION]\nYou received ") else {
        return false;
    };
    let Some((count, _)) =
        rest.split_once(" notification(s) from other agents working in this codebase:\n\n")
    else {
        return false;
    };
    !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
}

fn authentic_content<'a>(
    texts: impl Iterator<Item = &'a str> + Clone,
    has_images: bool,
    has_tool_results: bool,
) -> bool {
    if has_tool_results
        || texts.clone().next().is_some_and(|text| {
            text.trim_start().starts_with("<system-reminder>")
                || crate::todo::is_auto_poke_message(text)
                || is_legacy_peer_notification(text)
        })
        || texts
            .clone()
            .any(|text| text.trim_start().starts_with("[Scheduled task]\n"))
    {
        return false;
    }
    has_images
        || texts
            .into_iter()
            .any(|text| !text.trim().is_empty() && !super::is_attached_image_label_text(text))
}

/// Classify an incoming prompt before capturing its pre-input file boundary.
/// Use the same content rules as binding, including legacy synthetic envelopes.
pub fn is_authentic_user_input(prompt: &str, has_images: bool) -> bool {
    authentic_content(std::iter::once(prompt), has_images, false)
}

/// Human input boundaries, excluding tool continuations and synthetic notices.
/// Admission, rewind and response statistics must agree after resume.
pub fn is_authentic_user_prompt(message: &StoredMessage) -> bool {
    matches!(message.role, Role::User)
        && message.display_role.is_none()
        && authentic_content(
            message.content.iter().filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            }),
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. })),
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(prompt: &str, has_images: bool) -> StoredMessage {
        let mut content = Vec::new();
        if has_images {
            content.push(ContentBlock::Image {
                media_type: "image/png".into(),
                data: "AA==".into(),
            });
        }
        content.push(ContentBlock::Text {
            text: prompt.into(),
            cache_control: None,
        });
        StoredMessage {
            id: "input".into(),
            role: Role::User,
            content,
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    #[test]
    fn incoming_checkpoint_admission_matches_stored_prompt_classification() {
        let cases = [
            ("human prompt".to_string(), true),
            ("  λ prompt with user whitespace  ".into(), true),
            ("[NOTIFICATION]\nYou received a real letter".into(), true),
            ("[NOTIFICATION]\nYou received 1 notification(s) from other agents working in this codebase:\n\nReady.".into(), false),
            (crate::todo::build_auto_poke_message(2), false),
            (crate::todo::TODO_LONG_SESSION_REVIEW_MESSAGE.into(), false),
            ("  <system-reminder>Resume.</system-reminder>".into(), false),
            ("[Scheduled task]\nScheduled work".into(), false),
        ];
        for (prompt, expected) in cases {
            for images in [false, true] {
                assert_eq!(
                    is_authentic_user_input(&prompt, images),
                    expected,
                    "incoming: {prompt}"
                );
                assert_eq!(
                    is_authentic_user_prompt(&message(&prompt, images)),
                    expected,
                    "stored: {prompt}"
                );
            }
        }
        for prompt in [
            "",
            " \n",
            "[Attached image associated with the preceding tool result: figure]",
        ] {
            for images in [false, true] {
                assert_eq!(is_authentic_user_input(prompt, images), images);
                assert_eq!(is_authentic_user_prompt(&message(prompt, images)), images);
            }
        }
    }

    #[test]
    fn stored_provenance_and_tool_results_remain_strict() {
        let mut input = message("ordinary prompt", true);
        input.display_role = Some(crate::session::StoredDisplayRole::System);
        assert!(!is_authentic_user_prompt(&input));
        input.display_role = None;
        input.role = Role::Assistant;
        assert!(!is_authentic_user_prompt(&input));
        input.role = Role::User;
        input.content.push(ContentBlock::ToolResult {
            tool_use_id: "tool".into(),
            content: "result".into(),
            is_error: None,
        });
        assert!(!is_authentic_user_prompt(&input));
        input.content.pop();
        input.content.push(ContentBlock::Text {
            text: "[Scheduled task]\nnot a human boundary".into(),
            cache_control: None,
        });
        assert!(
            !is_authentic_user_prompt(&input),
            "scheduler envelope may occur in a later text block"
        );
    }
}
