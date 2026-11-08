//! Classify transcript strings by origin while streaming JSON.
//!
//! A query word the user typed is much stronger evidence of the conversation
//! they want than the same word inside a tool dump or a reasoning trace. The
//! walker tracks the enclosing message role and content block type so every
//! decoded string carries the source it came from, across jcode, Claude Code,
//! Codex, and similar role/type tagged transcript formats.
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    ToolOutput,
    Thinking,
    ToolInput,
    /// Metadata, system reminders, and untagged strings.
    #[default]
    Other,
    Assistant,
    User,
}

impl Source {
    /// Relative strength of a match found in this source.
    pub(crate) fn weight(self) -> u32 {
        match self {
            Source::User => 8,
            Source::Assistant => 4,
            Source::Other | Source::ToolInput => 2,
            Source::Thinking | Source::ToolOutput => 1,
        }
    }

    fn is_low(self) -> bool {
        matches!(
            self,
            Source::ToolInput | Source::ToolOutput | Source::Thinking
        )
    }
}

/// Weight of the best source per token, summed across query tokens.
pub(crate) fn source_weight(best: &[Option<Source>]) -> u32 {
    best.iter().flatten().map(|source| source.weight()).sum()
}

/// Keep the stronger of two per-token observations.
pub(crate) fn stronger(a: Option<Source>, b: Option<Source>) -> Option<Source> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.weight() > a.weight() { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Starts a section of the in-memory search index. The marker is a separate
/// whitespace-delimited word, so it never joins a query word, and a control
/// character, so a typed query never matches it.
const MARK: char = '\u{1f}';

impl Source {
    const ALL: [Source; 6] = [
        Source::ToolOutput,
        Source::Thinking,
        Source::ToolInput,
        Source::Other,
        Source::Assistant,
        Source::User,
    ];

    fn tag(self) -> char {
        match self {
            Source::ToolOutput => '0',
            Source::Thinking => '1',
            Source::ToolInput => '2',
            Source::Other => '3',
            Source::Assistant => '4',
            Source::User => '5',
        }
    }

    fn from_tag(tag: char) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.tag() == tag)
    }

    /// Index of this source in [`SourceBuckets`].
    fn slot(self) -> usize {
        self.tag() as usize - '0' as usize
    }
}

/// Marks where the following search index text came from.
pub(crate) fn push_mark(index: &mut String, source: Source) {
    index.push(' ');
    index.push(MARK);
    index.push(source.tag());
    index.push(' ');
}

/// Origin of the index text at byte `pos`. Unmarked text, including indexes
/// built before sections existed, is treated as untagged metadata.
fn source_at(index: &str, pos: usize) -> Source {
    index[..pos]
        .rfind(MARK)
        .and_then(|at| index[at + MARK.len_utf8()..].chars().next())
        .and_then(Source::from_tag)
        .unwrap_or(Source::Other)
}

/// Bounds per-token work on the keystroke path for very common words.
const MAX_CLASSIFIED_MATCHES: usize = 64;

/// Best origin among a token's occurrences in a sectioned search index.
fn best_index_source(index: &str, token: &str) -> Option<Source> {
    let mut best = None;
    for (pos, _) in index.match_indices(token).take(MAX_CLASSIFIED_MATCHES) {
        best = stronger(best, Some(source_at(index, pos)));
        if best == Some(Source::User) {
            break;
        }
    }
    best
}

/// Cheap per-token estimate from in-memory metadata, never transcript IO.
/// Title, saved label, and opening prompt are user intent. Other index hits
/// use the section they were indexed under, and a transcript classification
/// pass can still upgrade them.
pub(crate) fn metadata_sources(
    intent_fields: &[&str],
    index: &str,
    tokens: &[String],
) -> Vec<Option<Source>> {
    tokens
        .iter()
        .map(|token| {
            if intent_fields
                .iter()
                .any(|field| field.contains(token.as_str()))
            {
                Some(Source::User)
            } else {
                best_index_source(index, token)
            }
        })
        .collect()
}

/// Bounded per-source transcript samples. Sampling each source separately
/// keeps a flood of tool output from evicting the user's prompts.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceBuckets {
    text: [String; 6],
}

impl SourceBuckets {
    /// Byte budget for one source, out of `total` for all of them. User prompts
    /// and replies get most of it since they are the strongest evidence.
    fn budget(source: Source, total: usize) -> usize {
        match source {
            Source::User => total * 3 / 8,
            Source::Assistant => total / 4,
            _ => total / 16,
        }
    }

    pub(crate) fn push(&mut self, source: Source, text: &str, total: usize) {
        crate::tui::session_picker::loading::push_sampled_search_text(
            &mut self.text[source.slot()],
            text,
            Self::budget(source, total),
        );
    }

    pub(crate) fn merge(&mut self, other: &Self, total: usize) {
        for source in Source::ALL {
            self.push(source, &other.text[source.slot()], total);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.iter().all(String::is_empty)
    }

    /// Appends every non-empty sample under its section marker, strongest first.
    pub(crate) fn render(&self, index: &mut String) {
        for source in Source::ALL.into_iter().rev() {
            let text = self.text[source.slot()].trim();
            if !text.is_empty() {
                push_mark(index, source);
                index.push_str(text);
            }
        }
    }
}

/// Collects decoded transcript strings into per-source samples.
pub(crate) struct BucketSink<'a> {
    pub(crate) buckets: &'a mut SourceBuckets,
    pub(crate) total: usize,
}

impl Sink for BucketSink<'_> {
    fn text(&mut self, text: &str, source: Source) -> Result<(), &'static str> {
        let text = text.trim();
        if !text.is_empty() {
            self.buckets.push(source, text, self.total);
        }
        Ok(())
    }
}

/// Source of a preview or stored message role, such as `user` or `tool`.
pub(crate) fn message_role_source(role: &str) -> Source {
    role_source(role).unwrap_or(Source::Other)
}

pub(crate) trait Sink {
    /// Receives each decoded string. An error stops traversal.
    fn text(&mut self, text: &str, source: Source) -> Result<(), &'static str>;
}

pub(crate) struct Walk<'a, S> {
    pub(crate) sink: &'a mut S,
    pub(crate) source: Source,
}

impl<'a, S: Sink> Walk<'a, S> {
    pub(crate) fn new(sink: &'a mut S) -> Self {
        Self {
            sink,
            source: Source::Other,
        }
    }

    fn child(&mut self, source: Source) -> Walk<'_, S> {
        Walk {
            sink: &mut *self.sink,
            source,
        }
    }
}

impl<'de, S: Sink> DeserializeSeed<'de> for Walk<'_, S> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        de.deserialize_any(self)
    }
}

impl<'de, S: Sink> Visitor<'de> for Walk<'_, S> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("transcript JSON")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        let source = if value.trim_start().starts_with("<system-reminder>") {
            Source::Other
        } else {
            self.source
        };
        self.sink.text(value, source).map_err(E::custom)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<(), A::Error> {
        let source = self.source;
        while seq.next_element_seed(self.child(source))?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<(), A::Error> {
        let inherited = self.source;
        let mut tagged = inherited;
        while let Some(key) = map.next_key_seed(KeySeed)? {
            let tag = match key {
                Key::Role => map.next_value_seed(TagSeed(role_source))?,
                Key::Type => map.next_value_seed(TagSeed(type_source))?,
                Key::Force(source) => {
                    map.next_value_seed(self.child(source))?;
                    continue;
                }
                Key::Body => {
                    map.next_value_seed(self.child(tagged))?;
                    continue;
                }
                Key::Plain => {
                    // IDs, paths, and timestamps beside a user prompt are not
                    // something the user typed. Never raise them above Other.
                    let source = if tagged.weight() > Source::Other.weight() {
                        Source::Other
                    } else {
                        tagged
                    };
                    map.next_value_seed(self.child(source))?;
                    continue;
                }
            };
            // Tags inside tool payloads or reasoning (for example a tool input
            // containing `"role": "user"`) must not promote that content.
            if let Some(tag) = tag
                && !(tagged.is_low() && tag.weight() > tagged.weight())
            {
                tagged = tag;
            }
        }
        Ok(())
    }
}

fn role_source(role: &str) -> Option<Source> {
    Some(match role.to_ascii_lowercase().as_str() {
        "user" | "human" => Source::User,
        "assistant" | "model" | "ai" => Source::Assistant,
        "tool" | "function" | "toolresult" | "tool_result" => Source::ToolOutput,
        "system" | "developer" => Source::Other,
        _ => return None,
    })
}

fn type_source(kind: &str) -> Option<Source> {
    let kind = kind.to_ascii_lowercase();
    Some(match kind.as_str() {
        "thinking"
        | "redacted_thinking"
        | "reasoning"
        | "reasoning_trace"
        | "agent_reasoning"
        | "agent_reasoning_raw_content" => Source::Thinking,
        "tool_use" | "server_tool_use" | "function_call" | "custom_tool_call" | "tool_call"
        | "local_shell_call" | "web_search_call" => Source::ToolInput,
        "user" | "user_message" => Source::User,
        "assistant" | "agent_message" => Source::Assistant,
        _ if kind.ends_with("_output") || kind.ends_with("_result") => Source::ToolOutput,
        _ => return None,
    })
}

enum Key {
    Role,
    Type,
    Body,
    Plain,
    Force(Source),
}

struct KeySeed;
impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Key;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Key, D::Error> {
        de.deserialize_str(self)
    }
}
impl Visitor<'_> for KeySeed {
    type Value = Key;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("object key")
    }
    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Key, E> {
        Ok(match key {
            "role" => Key::Role,
            "type" => Key::Type,
            "content" | "text" | "message" | "parts" | "summary" | "value" | "body" | "prompt"
            | "display" => Key::Body,
            "thinking" | "signature" | "encrypted_content" | "reasoning" | "reasoning_content" => {
                Key::Force(Source::Thinking)
            }
            "input" | "arguments" | "tool_input" | "params" | "parameters" => {
                Key::Force(Source::ToolInput)
            }
            "toolUseResult" | "tool_use_result" | "output" | "tool_output" | "stdout"
            | "stderr" => Key::Force(Source::ToolOutput),
            "meta" | "metadata" => Key::Force(Source::Other),
            _ => Key::Plain,
        })
    }
}

/// Reads a string tag value. Non-string values are skipped.
struct TagSeed(fn(&str) -> Option<Source>);
impl<'de> DeserializeSeed<'de> for TagSeed {
    type Value = Option<Source>;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for TagSeed {
    type Value = Option<Source>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("tag")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok((self.0)(value))
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collect(Vec<(String, Source)>);
    impl Sink for Collect {
        fn text(&mut self, text: &str, source: Source) -> Result<(), &'static str> {
            self.0.push((text.to_owned(), source));
            Ok(())
        }
    }

    fn classify(json: &str) -> Vec<(String, Source)> {
        let mut sink = Collect::default();
        Walk::new(&mut sink)
            .deserialize(&mut serde_json::Deserializer::from_str(json))
            .unwrap();
        sink.0
    }

    fn source_of(json: &str, needle: &str) -> Source {
        classify(json)
            .into_iter()
            .find(|(text, _)| text == needle)
            .unwrap_or_else(|| panic!("{needle} not found"))
            .1
    }

    #[test]
    fn jcode_messages_classify_prompts_replies_tools_and_reasoning() {
        let json = r#"{"meta":{"title":"TITLE"},"append_messages":[
            {"id":"m1","role":"user","content":[{"type":"text","text":"PROMPT"}]},
            {"id":"m2","role":"user","content":[{"type":"text","text":"<system-reminder>REMINDER"}]},
            {"id":"m3","role":"assistant","content":[
                {"type":"reasoning_trace","text":"THOUGHT"},
                {"type":"text","text":"REPLY"},
                {"type":"tool_use","id":"t","name":"bash","input":{"command":"CMD","role":"user"}}]},
            {"id":"m4","role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"OUT"}]}]}"#;
        assert_eq!(source_of(json, "TITLE"), Source::Other);
        assert_eq!(source_of(json, "PROMPT"), Source::User);
        assert_eq!(source_of(json, "<system-reminder>REMINDER"), Source::Other);
        assert_eq!(source_of(json, "THOUGHT"), Source::Thinking);
        assert_eq!(source_of(json, "REPLY"), Source::Assistant);
        assert_eq!(source_of(json, "CMD"), Source::ToolInput);
        assert_eq!(source_of(json, "OUT"), Source::ToolOutput);
        // Identifiers beside a prompt are not something the user typed.
        assert_eq!(source_of(json, "m1"), Source::Other);
    }

    #[test]
    fn claude_code_and_codex_records_classify() {
        let claude = r#"{"cwd":"/repo","type":"user","message":{"role":"user","content":[
            {"type":"tool_result","content":"RESULT"}]},"toolUseResult":{"stdout":"STDOUT"}}"#;
        assert_eq!(source_of(claude, "RESULT"), Source::ToolOutput);
        assert_eq!(source_of(claude, "STDOUT"), Source::ToolOutput);
        assert_eq!(source_of(claude, "/repo"), Source::Other);
        let thinking = r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"PONDER"},{"type":"text","text":"ANSWER"}]}}"#;
        assert_eq!(source_of(thinking, "PONDER"), Source::Thinking);
        assert_eq!(source_of(thinking, "ANSWER"), Source::Assistant);
        let codex_prompt = r#"{"type":"response_item","payload":{"type":"message","role":"user",
            "content":[{"type":"input_text","text":"ASK"}]}}"#;
        assert_eq!(source_of(codex_prompt, "ASK"), Source::User);
        let codex_call = r#"{"type":"response_item","payload":{"type":"function_call",
            "arguments":"ARGS"}}"#;
        assert_eq!(source_of(codex_call, "ARGS"), Source::ToolInput);
        let codex_out = r#"{"type":"response_item","payload":{"type":"custom_tool_call_output",
            "output":"OUTPUT"}}"#;
        assert_eq!(source_of(codex_out, "OUTPUT"), Source::ToolOutput);
        let codex_event =
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"SAID"}}"#;
        assert_eq!(source_of(codex_event, "SAID"), Source::User);
    }

    #[test]
    fn weights_prefer_user_then_assistant_then_tools_and_thinking() {
        assert!(Source::User.weight() > Source::Assistant.weight());
        assert!(Source::Assistant.weight() > Source::ToolInput.weight());
        assert!(Source::ToolInput.weight() > Source::ToolOutput.weight());
        assert_eq!(Source::Thinking.weight(), Source::ToolOutput.weight());
        assert_eq!(
            stronger(Some(Source::ToolOutput), Some(Source::User)),
            Some(Source::User)
        );
        assert_eq!(
            stronger(None, Some(Source::Thinking)),
            Some(Source::Thinking)
        );
        let tokens = vec!["deploy".to_owned(), "api".to_owned(), "zzz".to_owned()];
        let sources = metadata_sources(&["deploy the thing"], "deploy api", &tokens);
        assert_eq!(sources, [Some(Source::User), Some(Source::Other), None]);
        assert_eq!(source_weight(&sources), 10);
    }

    #[test]
    fn sectioned_index_reports_strongest_section_per_token() {
        let mut index = String::from("title words");
        push_mark(&mut index, Source::ToolOutput);
        index.push_str("grep zebra kiwi");
        push_mark(&mut index, Source::User);
        index.push_str("find the kiwi");
        push_mark(&mut index, Source::Thinking);
        index.push_str("pondering mango");
        let tokens: Vec<String> = ["zebra", "kiwi", "mango", "title"].map(String::from).into();
        assert_eq!(
            metadata_sources(&[], &index, &tokens),
            [
                Some(Source::ToolOutput),
                Some(Source::User),
                Some(Source::Thinking),
                Some(Source::Other),
            ]
        );
    }

    #[test]
    fn buckets_keep_user_prompts_when_tool_output_floods_the_budget() {
        let mut buckets = SourceBuckets::default();
        let mut sink = BucketSink {
            buckets: &mut buckets,
            total: 1024,
        };
        Walk {
            sink: &mut sink,
            source: Source::User,
        }
        .deserialize(&mut serde_json::Deserializer::from_str(
            r#"[{"type":"text","text":"ORIGINAL ASK"}]"#,
        ))
        .unwrap();
        let flood = "x".repeat(4096);
        for _ in 0..8 {
            buckets.push(Source::ToolOutput, &flood, 1024);
        }
        let mut index = String::new();
        buckets.render(&mut index);
        let tokens = vec!["original".to_owned(), "ask".to_owned()];
        assert!(index.len() < 1024);
        assert_eq!(
            metadata_sources(&[], &index.to_lowercase(), &tokens),
            [Some(Source::User), Some(Source::User)]
        );
    }
}
