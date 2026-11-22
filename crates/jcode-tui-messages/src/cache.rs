use crate::DisplayMessage;
use jcode_config_types::{DiagramDisplayMode, DiffDisplayMode, LatexRenderingMode};
use ratatui::layout::Alignment;
use ratatui::text::{Line, Span};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageCacheKey {
    width: u16,
    diff_mode: DiffDisplayMode,
    message_hash: u64,
    content_len: usize,
    diagram_mode: DiagramDisplayMode,
    centered: bool,
    mermaid_epoch: u64,
    mermaid_aspect_bucket: Option<u16>,
    show_agentgrep_output: bool,
    show_bash_output: bool,
    tool_call_details: bool,
    latex_rendering: LatexRenderingMode,
    show_link_urls: bool,
}

#[derive(Default)]
struct MessageCacheState {
    entries: HashMap<MessageCacheKey, Arc<Vec<Line<'static>>>>,
    order: VecDeque<MessageCacheKey>,
}

impl MessageCacheState {
    #[cfg(test)]
    fn get(&self, key: &MessageCacheKey) -> Option<Vec<Line<'static>>> {
        self.entries.get(key).map(|lines| lines.as_ref().clone())
    }

    fn insert(&mut self, key: MessageCacheKey, lines: Vec<Line<'static>>) {
        let arc = Arc::new(lines);
        if let std::collections::hash_map::Entry::Occupied(mut entry) =
            self.entries.entry(key.clone())
        {
            entry.insert(arc);
            return;
        }

        self.entries.insert(key.clone(), arc);
        self.order.push_back(key);

        while self.order.len() > MESSAGE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

static MESSAGE_CACHE: OnceLock<Mutex<MessageCacheState>> = OnceLock::new();

fn message_cache() -> &'static Mutex<MessageCacheState> {
    MESSAGE_CACHE.get_or_init(|| Mutex::new(MessageCacheState::default()))
}

/// Release cached rendered lines without altering source messages. Existing
/// callers retain their own lines, and the next render rebuilds missing entries.
pub fn clear_message_cache() {
    let Some(cache) = MESSAGE_CACHE.get() else {
        return;
    };
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache = MessageCacheState::default();
}

const MESSAGE_CACHE_LIMIT: usize = 2048;

/// Runtime-sensitive inputs that affect message rendering but are not intrinsic to a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MessageCacheContext {
    pub diagram_mode: DiagramDisplayMode,
    pub centered: bool,
    pub mermaid_epoch: u64,
    pub mermaid_aspect_bucket: Option<u16>,
    pub show_agentgrep_output: bool,
    pub show_bash_output: bool,
    pub tool_call_details: bool,
    pub latex_rendering: LatexRenderingMode,
    pub show_link_urls: bool,
}

pub fn left_pad_lines_for_centered_mode(lines: &mut [Line<'static>], width: u16) {
    let max_line_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let pad = (width as usize).saturating_sub(max_line_width) / 2;
    if pad == 0 {
        return;
    }

    let pad_str = " ".repeat(pad);
    for line in lines {
        line.spans.insert(0, Span::raw(pad_str.clone()));
        line.alignment = Some(Alignment::Left);
    }
}

pub fn centered_wrap_width(width: u16, centered: bool, centered_max_width: usize) -> usize {
    let width = width as usize;
    if centered {
        width.min(centered_max_width).max(1)
    } else {
        width.max(1)
    }
}

pub fn get_cached_message_lines<F>(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: DiffDisplayMode,
    context: MessageCacheContext,
    render: F,
) -> Vec<Line<'static>>
where
    F: FnOnce(&DisplayMessage, u16, DiffDisplayMode) -> Vec<Line<'static>>,
{
    project_cached_message_lines(
        msg,
        width,
        diff_mode,
        context,
        render,
        <[Line<'static>]>::to_vec,
    )
}

/// Inspect cached rendered lines without cloning their spans or text. Misses use
/// the same renderer and cache key as `get_cached_message_lines`.
/// The projection runs under the cache lock and must not reenter this cache.
pub fn project_cached_message_lines<F, P, R>(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: DiffDisplayMode,
    context: MessageCacheContext,
    render: F,
    project: P,
) -> R
where
    F: FnOnce(&DisplayMessage, u16, DiffDisplayMode) -> Vec<Line<'static>>,
    P: FnOnce(&[Line<'static>]) -> R,
{
    if cfg!(test) {
        return project(&render(msg, width, diff_mode));
    }

    let key = message_cache_key(msg, width, diff_mode, context);
    let mut cache = match message_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    project_with_cache(&mut cache, key, || render(msg, width, diff_mode), project)
}

fn message_cache_key(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: DiffDisplayMode,
    context: MessageCacheContext,
) -> MessageCacheKey {
    MessageCacheKey {
        width,
        diff_mode,
        message_hash: msg.stable_cache_hash(),
        content_len: msg.content.len(),
        diagram_mode: context.diagram_mode,
        centered: context.centered,
        mermaid_epoch: context.mermaid_epoch,
        mermaid_aspect_bucket: context.mermaid_aspect_bucket,
        show_agentgrep_output: context.show_agentgrep_output,
        show_bash_output: context.show_bash_output,
        tool_call_details: context.tool_call_details,
        latex_rendering: context.latex_rendering,
        show_link_urls: context.show_link_urls,
    }
}

fn project_with_cache<R>(
    cache: &mut MessageCacheState,
    key: MessageCacheKey,
    render: impl FnOnce() -> Vec<Line<'static>>,
    project: impl FnOnce(&[Line<'static>]) -> R,
) -> R {
    if let Some(lines) = cache.entries.get(&key) {
        return project(lines);
    }
    let lines = render();
    let result = project(&lines);
    cache.insert(key, lines);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> MessageCacheContext {
        MessageCacheContext {
            diagram_mode: DiagramDisplayMode::default(),
            centered: false,
            mermaid_epoch: 0,
            mermaid_aspect_bucket: None,
            show_agentgrep_output: true,
            show_bash_output: true,
            tool_call_details: true,
            latex_rendering: LatexRenderingMode::Image,
            show_link_urls: false,
        }
    }

    #[test]
    fn projection_borrows_same_lines_on_miss_and_hit_and_caches_empty_results() {
        let msg = DisplayMessage::todos("{}");
        let key = message_cache_key(&msg, 80, DiffDisplayMode::Off, context());
        let mut cache = MessageCacheState::default();
        let lines = vec![Line::from("owned text".to_owned())];
        let text_ptr = lines[0].spans[0].content.as_ptr();
        let ptr = project_with_cache(
            &mut cache,
            key.clone(),
            || lines,
            |lines| {
                assert_eq!(lines[0].spans[0].content.as_ptr(), text_ptr);
                lines.as_ptr()
            },
        );
        project_with_cache(
            &mut cache,
            key.clone(),
            || panic!("cache hit rendered"),
            |lines| {
                assert_eq!(lines.as_ptr(), ptr);
                assert_eq!(lines[0].spans[0].content.as_ptr(), text_ptr);
            },
        );
        cache.insert(key.clone(), Vec::new());
        assert!(project_with_cache(
            &mut cache,
            key,
            || panic!("empty hit rendered"),
            |lines| lines.is_empty()
        ));
    }

    #[test]
    fn projection_uses_every_message_key_and_render_context_input() {
        let mut msg = DisplayMessage::todos("{}");
        msg.title = Some("Pinned todos".into());
        let context = context();
        let key = message_cache_key(&msg, 80, DiffDisplayMode::Off, context);
        let mut cache = MessageCacheState::default();
        cache.insert(key.clone(), vec![Line::from("cached")]);
        let mut changed_keys = vec![
            message_cache_key(&msg, 79, DiffDisplayMode::Off, context),
            message_cache_key(&msg, 80, DiffDisplayMode::Inline, context),
        ];
        macro_rules! changed_context {
            ($field:ident, $value:expr) => {
                changed_keys.push(message_cache_key(
                    &msg,
                    80,
                    DiffDisplayMode::Off,
                    MessageCacheContext {
                        $field: $value,
                        ..context
                    },
                ));
            };
        }
        changed_context!(diagram_mode, DiagramDisplayMode::Margin);
        changed_context!(centered, true);
        changed_context!(mermaid_epoch, 1);
        changed_context!(mermaid_aspect_bucket, Some(7));
        changed_context!(show_agentgrep_output, false);
        changed_context!(show_bash_output, false);
        changed_context!(tool_call_details, false);
        changed_context!(latex_rendering, LatexRenderingMode::Unicode);
        changed_context!(show_link_urls, true);
        let mut changed = msg.clone();
        changed.title = None;
        changed_keys.push(message_cache_key(
            &changed,
            80,
            DiffDisplayMode::Off,
            context,
        ));
        changed = msg.clone();
        changed.content = "[]".into();
        changed_keys.push(message_cache_key(
            &changed,
            80,
            DiffDisplayMode::Off,
            context,
        ));
        changed = msg.clone();
        changed.role = "system".into();
        changed_keys.push(message_cache_key(
            &changed,
            80,
            DiffDisplayMode::Off,
            context,
        ));
        changed = msg.clone();
        changed.content = r#"{"todos":[]}"#.into();
        changed_keys.push(message_cache_key(
            &changed,
            80,
            DiffDisplayMode::Off,
            context,
        ));
        for changed_key in changed_keys {
            assert_ne!(changed_key, key);
            assert!(project_with_cache(
                &mut cache,
                changed_key,
                Vec::new,
                |lines| lines.is_empty()
            ));
        }
        assert!(!project_with_cache(
            &mut cache,
            key,
            || panic!("original entry lost"),
            |lines| lines.is_empty()
        ));
    }

    #[test]
    fn clearing_message_cache_releases_owned_lines_and_capacity() {
        let key = MessageCacheKey {
            width: 80,
            diff_mode: DiffDisplayMode::default(),
            message_hash: 9,
            content_len: 100,
            diagram_mode: DiagramDisplayMode::default(),
            centered: false,
            mermaid_epoch: 0,
            mermaid_aspect_bucket: None,
            show_agentgrep_output: true,
            show_bash_output: true,
            tool_call_details: true,
            latex_rendering: LatexRenderingMode::Image,
            show_link_urls: false,
        };
        let mut isolated_cache = MessageCacheState::default();
        isolated_cache.insert(key.clone(), vec![Line::from("image")]);
        let unicode_key = MessageCacheKey {
            latex_rendering: LatexRenderingMode::Unicode,
            show_link_urls: false,
            ..key.clone()
        };
        assert!(isolated_cache.get(&unicode_key).is_none());
        let link_urls_key = MessageCacheKey {
            show_link_urls: true,
            ..key.clone()
        };
        assert!(isolated_cache.get(&link_urls_key).is_none());
        assert!(isolated_cache.get(&key).is_some());
        let weak = {
            let mut cache = message_cache().lock().unwrap();
            cache.insert(key.clone(), vec![Line::from("derived text".repeat(100))]);
            Arc::downgrade(cache.entries.get(&key).unwrap())
        };
        clear_message_cache();
        assert!(weak.upgrade().is_none());
        let cache = message_cache().lock().unwrap();
        assert!(cache.entries.is_empty());
        assert_eq!(cache.entries.capacity(), 0);
        assert!(cache.order.is_empty());
        assert_eq!(cache.order.capacity(), 0);
    }

    #[test]
    fn centered_wrap_width_caps_centered_width() {
        assert_eq!(centered_wrap_width(120, true, 96), 96);
        assert_eq!(centered_wrap_width(80, true, 96), 80);
        assert_eq!(centered_wrap_width(120, false, 96), 120);
    }

    #[test]
    fn left_pad_lines_aligns_to_centered_block() {
        let mut lines = vec![Line::from("abc")];
        left_pad_lines_for_centered_mode(&mut lines, 9);
        assert_eq!(lines[0].to_string(), "   abc");
        assert_eq!(lines[0].alignment, Some(Alignment::Left));
    }
}
