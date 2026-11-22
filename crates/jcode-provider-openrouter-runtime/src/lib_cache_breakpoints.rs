use super::*;

pub(super) fn add_cache_breakpoint(messages: &mut [Message]) -> bool {
    let mut cache_index = None;
    for (idx, msg) in messages.iter().enumerate().rev() {
        if let Role::User = msg.role
            && msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { .. }))
        {
            cache_index = Some(idx);
            break;
        }
    }

    let Some(idx) = cache_index else {
        return false;
    };

    let msg = &mut messages[idx];
    if let Some(block_index) = msg
        .content
        .iter()
        .rposition(|block| matches!(block, ContentBlock::Text { .. }))
    {
        if matches!(
            &msg.content[block_index],
            ContentBlock::Text {
                cache_control: None,
                ..
            }
        ) && let ContentBlock::Text { cache_control, .. } = &mut msg.content[block_index]
        {
            *cache_control = Some(CacheControl::ephemeral(None));
        }
        return true;
    }

    false
}
