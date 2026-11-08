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

/// Cheap per-token estimate from in-memory metadata, never transcript IO.
/// Title, saved label, and opening prompt are user intent. Other index hits
/// came from an unclassified transcript sample, so they get the lowest weight
/// and can only be upgraded by a transcript classification pass.
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
            } else if index.contains(token.as_str()) {
                Some(Source::ToolOutput)
            } else {
                None
            }
        })
        .collect()
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
        assert_eq!(
            sources,
            [Some(Source::User), Some(Source::ToolOutput), None]
        );
        assert_eq!(source_weight(&sources), 9);
    }
}
