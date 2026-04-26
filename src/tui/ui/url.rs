use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn url_regex() -> &'static Regex {
    static URL_REGEX: OnceLock<Regex> = OnceLock::new();
    URL_REGEX.get_or_init(|| {
        Regex::new(r#"(?i)(?:https?://|mailto:|file://)[^\s<>'\"]+"#)
            .expect("URL regex should compile")
    })
}
