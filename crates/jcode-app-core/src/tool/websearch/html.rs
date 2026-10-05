/// Detect whether an HTML body is an anti-bot/captcha challenge rather than a
/// real results page. DuckDuckGo (and similar) serve these with HTTP 200, so a
/// successful status plus zero parsed results is ambiguous without this check.
///
/// Returns a short human-readable reason when a challenge page is detected.
pub(super) fn detect_anti_bot_page(html: &str) -> Option<&'static str> {
    let lowered = html.to_ascii_lowercase();
    const MARKERS: &[(&str, &str)] = &[
        ("anomaly-modal", "anomaly challenge"),
        ("anomaly.js", "anomaly challenge"),
        ("dpn=1", "anomaly challenge"),
        ("captcha", "captcha"),
        ("g-recaptcha", "recaptcha"),
        ("are you a robot", "bot check"),
        ("unusual traffic", "bot check"),
        ("verify you are human", "human verification"),
        ("challenge-platform", "cloudflare challenge"),
        ("cf-challenge", "cloudflare challenge"),
    ];
    for (needle, reason) in MARKERS {
        if lowered.contains(needle) {
            return Some(reason);
        }
    }
    None
}

/// Decode the few HTML entities the scraping engines care about.
pub(super) fn html_decode(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .trim()
        .to_string()
}

pub(super) mod search_regex {
    use regex::Regex;
    use std::sync::OnceLock;

    fn compile_regex(pattern: &str, label: &str) -> Option<Regex> {
        match Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(err) => {
                crate::logging::warn(&format!(
                    "websearch: failed to compile static regex {label}: {}",
                    err
                ));
                None
            }
        }
    }

    macro_rules! static_regex {
        ($name:ident, $pat:expr_2021) => {
            pub fn $name() -> Option<&'static Regex> {
                static RE: OnceLock<Option<Regex>> = OnceLock::new();
                RE.get_or_init(|| compile_regex($pat, stringify!($name)))
                    .as_ref()
            }
        };
    }

    static_regex!(
        result_link,
        r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#
    );
    static_regex!(
        result_snippet,
        r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#
    );
    static_regex!(tag, r"<[^>]+>");
    static_regex!(
        bing_result_block,
        r#"(?s)<li[^>]*class="[^"]*\bb_algo\b[^"]*"[^>]*>(.*?)</li>"#
    );
    static_regex!(
        bing_link,
        r#"(?s)<h2[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>(.*?)</a>\s*</h2>"#
    );
    static_regex!(
        bing_caption,
        r#"(?s)<div[^>]*class="[^"]*\bb_caption\b[^"]*"[^>]*>.*?<p[^>]*>(.*?)</p>"#
    );
}
