//! Session export to Markdown / JSON (subset of issue #10).
//!
//! This MVP covers the markdown half of the export feature: render an entire
//! session as one self-contained Markdown document with messages, tool calls,
//! tool outputs, and reasoning blocks. Suitable for pasting into PRs / bug
//! reports / docs.
//!
//! Out of scope for this MVP, tracked as follow-ups under issue #10:
//!   - HTML output with inline CSS / SVG mermaid / base64 images
//!   - Redaction (`--redact`) of API keys, bearer tokens, well-known env vars
//!   - The `/export` slash command (this PR is CLI-only via
//!     `jcode export <session> [output]`)

use anyhow::{Context, Result};
use std::path::PathBuf;

use jcode_message_types::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
    Html,
}

/// CLI entry point: export the session and print the resulting path to stdout
/// so shell pipelines can capture it.
pub fn run(
    session_ref: &str,
    output: Option<PathBuf>,
    format: ExportFormat,
    redact: bool,
) -> Result<()> {
    let absolute = export_to_path(session_ref, output, format, redact)?;
    println!("{}", absolute.display());
    Ok(())
}

/// Library entry point used by both the CLI dispatcher and the `/export`
/// slash command. Returns the canonical path the session was written to so
/// callers (e.g. the TUI, which has stdout captured by the alt-screen) can
/// display it via their own UI surface instead of dropping it into a
/// hidden stdout buffer.
pub fn export_to_path(
    session_ref: &str,
    output: Option<PathBuf>,
    format: ExportFormat,
    redact: bool,
) -> Result<PathBuf> {
    let session_id = crate::session::find_session_by_name_or_id(session_ref)?;
    let session = crate::session::Session::load(&session_id)?;

    let body = match format {
        ExportFormat::Markdown => render_markdown(&session),
        ExportFormat::Json => {
            serde_json::to_string_pretty(&session).context("failed to serialize session to JSON")?
        }
        ExportFormat::Html => render_html(&session),
    };

    let body = if redact { redact_secrets(&body) } else { body };

    let output_path = match output {
        Some(p) => p,
        None => default_output_path(&session, format),
    };

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    }
    std::fs::write(&output_path, body.as_bytes())
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    Ok(output_path
        .canonicalize()
        .unwrap_or_else(|_| output_path.clone()))
}

fn default_output_path(session: &crate::session::Session, format: ExportFormat) -> PathBuf {
    let stem = session
        .display_title()
        .map(slugify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            session
                .short_name
                .as_deref()
                .map(slugify)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| session.id.clone())
        });
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let ext = match format {
        ExportFormat::Markdown => "md",
        ExportFormat::Json => "json",
        ExportFormat::Html => "html",
    };
    PathBuf::from(format!("{stem}-{ts}.{ext}"))
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Render a session as a self-contained Markdown document.
/// Replace common secret-shaped tokens with `[REDACTED:<kind>]` markers.
///
/// Patterns covered (high-precision — won't catch every possible secret):
///   - OpenAI / Anthropic / generic SDK keys: `sk-…`, `sk-ant-…`, `sk-cp-…`
///   - GitHub tokens: `gho_…`, `ghp_…`, `ghs_…`, `ghr_…`
///   - Bearer headers: `Bearer <token>` → `Bearer [REDACTED:bearer]`
///   - z.ai-shape tokens: `<32 hex>.<24 chars>`
///   - Common env-var assignments: `ANTHROPIC_API_KEY=…`, `OPENAI_API_KEY=…`,
///     `OPENROUTER_API_KEY=…`, `ZHIPU_API_KEY=…`, `GITHUB_TOKEN=…`
///
/// Returns a new String. The original is left intact.
pub fn redact_secrets(input: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    struct Pat {
        re: Regex,
        replace: &'static str,
    }
    static PATS: OnceLock<Vec<Pat>> = OnceLock::new();
    let pats = PATS.get_or_init(|| {
        vec![
            // sk-… style keys (OpenAI, Anthropic, z.ai). Match `sk-` followed by
            // 20+ url-safe chars or dots/dashes. Anchor on word boundary to avoid
            // chewing surrounding text.
            Pat {
                re: Regex::new(r"\bsk-[A-Za-z0-9_\-\.]{20,}").unwrap(),
                replace: "[REDACTED:sk]",
            },
            // GitHub tokens.
            Pat {
                re: Regex::new(r"\bgh[opsru]_[A-Za-z0-9]{20,}").unwrap(),
                replace: "[REDACTED:github]",
            },
            // Bearer tokens (header-shape). Replace token only, keep "Bearer ".
            Pat {
                re: Regex::new(r"\b(Bearer\s+)[A-Za-z0-9_\-\.]{16,}").unwrap(),
                replace: "${1}[REDACTED:bearer]",
            },
            // z.ai-shape: 32 hex . 12+ alnum (token format used by z.ai's
            // anthropic-compatible endpoint and similar).
            Pat {
                re: Regex::new(r"\b[a-f0-9]{32}\.[A-Za-z0-9]{12,}").unwrap(),
                replace: "[REDACTED:zai]",
            },
            // Env-var assignments for known secret names (case-insensitive name,
            // stops at end of line / quote / comma).
            Pat {
                re: Regex::new(
                    r#"(?i)\b(ANTHROPIC_API_KEY|OPENAI_API_KEY|OPENAI_COMPAT_API_KEY|OPENROUTER_API_KEY|ZHIPU_API_KEY|COHERE_API_KEY|GITHUB_TOKEN|GH_TOKEN|MINIMAX_API_KEY|XAI_API_KEY|DEEPSEEK_API_KEY|FIREWORKS_API_KEY|GROQ_API_KEY|MISTRAL_API_KEY|OPENCODE_API_KEY|OPENCODE_GO_API_KEY|TOGETHER_API_KEY|PERPLEXITY_API_KEY|CEREBRAS_API_KEY|NVIDIA_API_KEY|AZURE_OPENAI_API_KEY|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|GEMINI_API_KEY|GOOGLE_API_KEY|CLAUDE_CODE_OAUTH_TOKEN|ANTHROPIC_AUTH_TOKEN)(\s*=\s*)([^\r\n,'"\s]+)"#,
                )
                .unwrap(),
                replace: "${1}${2}[REDACTED:env]",
            },
        ]
    });

    let mut out = input.to_string();
    for pat in pats {
        out = pat.re.replace_all(&out, pat.replace).into_owned();
    }
    out
}

pub fn render_markdown(session: &crate::session::Session) -> String {
    let mut out = String::new();
    let title = session
        .display_title()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| session.display_name().to_string());
    out.push_str(&format!("# {title}\n\n"));
    out.push_str(&format!("- **Session ID**: `{}`\n", session.id));
    if let Some(name) = &session.short_name {
        out.push_str(&format!("- **Name**: `{}`\n", name));
    }
    if let Some(provider) = &session.provider_key {
        out.push_str(&format!("- **Provider**: `{}`\n", provider));
    }
    if let Some(model) = &session.model {
        out.push_str(&format!("- **Model**: `{}`\n", model));
    }
    out.push_str(&format!(
        "- **Created**: {}\n",
        session.created_at.to_rfc3339()
    ));
    out.push_str(&format!(
        "- **Updated**: {}\n",
        session.updated_at.to_rfc3339()
    ));
    out.push_str(&format!("- **Messages**: {}\n\n", session.messages.len()));

    if let Some(compaction) = session.compaction.as_ref() {
        let kind = if compaction.openai_encrypted_content.is_some() {
            "native/openai-encrypted"
        } else if !compaction.summary_text.is_empty() {
            "summary-text"
        } else {
            "none"
        };
        out.push_str(&format!(
            "> Active compaction artifact present (`{}` — {} chars).\n\n",
            kind,
            artifact_chars(compaction)
        ));
    }

    out.push_str("---\n\n");

    for (idx, msg) in session.messages.iter().enumerate() {
        render_stored_message(&mut out, idx, msg);
    }

    out
}

/// HTML-escape `s`. Tight subset (no entity references for unicode beyond
/// the dangerous five), since we control the rendered surface.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

const HTML_STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --fg: #1f2328; --bg: #ffffff; --muted: #57606a;
  --code-bg: #f6f8fa; --border: #d0d7de; --accent: #0969da;
}
@media (prefers-color-scheme: dark) {
  :root { --fg: #e6edf3; --bg: #0d1117; --muted: #8b949e;
          --code-bg: #161b22; --border: #30363d; --accent: #58a6ff; }
}
body { color: var(--fg); background: var(--bg);
       font: 14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",
             Inter,Roboto,Helvetica,Arial,sans-serif;
       max-width: 880px; margin: 24px auto; padding: 0 16px; }
h1 { margin: 0 0 16px; font-size: 22px; }
.meta { font-size: 13px; color: var(--muted); margin-bottom: 16px;
        border: 1px solid var(--border); border-radius: 6px; padding: 10px 12px; }
.meta div { display: flex; gap: 8px; }
.meta strong { color: var(--fg); min-width: 96px; display: inline-block; }
.message { border-top: 1px solid var(--border); padding: 16px 0; }
.message header { font-size: 13px; color: var(--muted); margin-bottom: 8px; }
.message header .role { color: var(--accent); font-weight: 600; }
.text { white-space: pre-wrap; word-wrap: break-word; }
details { background: var(--code-bg); border: 1px solid var(--border);
          border-radius: 6px; padding: 8px 12px; margin: 8px 0; }
details summary { cursor: pointer; color: var(--muted); font-size: 13px; }
details[open] summary { margin-bottom: 8px; }
pre { background: var(--code-bg); border: 1px solid var(--border);
      border-radius: 6px; padding: 10px 12px; overflow-x: auto;
      font: 12px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace; margin: 0; }
code { font: 12px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace; }
.compaction-note { font-style: italic; color: var(--muted);
                   border-left: 3px solid var(--accent);
                   padding: 4px 12px; margin: 12px 0; }
"#;

/// Render a session as a self-contained HTML document with inline CSS.
///
/// Design goals:
///   - **Self-contained**: no external CSS/JS, no inline scripts. Safe to
///     attach to email or open from a file:// URL on a locked-down machine.
///   - **Theme-aware**: `prefers-color-scheme` media query gives a dark mode
///     fallback automatically.
///   - **Escaped**: every user/agent-supplied string runs through
///     html_escape() before insertion. We never trust message bodies.
///   - **Collapsible**: thinking + tool blocks use `<details>` so the
///     transcript reads cleanly by default but is fully auditable on click.
pub fn render_html(session: &crate::session::Session) -> String {
    use jcode_message_types::{ContentBlock, Role};

    let title = session
        .display_title()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| session.display_name().to_string());
    let title_esc = html_escape(&title);

    let mut out = String::with_capacity(4096);
    out.push_str("<!doctype html>\n<html lang=\"en\"><head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!("<title>{title_esc} — jcode session</title>\n"));
    out.push_str("<style>");
    out.push_str(HTML_STYLE);
    out.push_str("</style>\n</head><body>\n");

    out.push_str(&format!("<h1>{title_esc}</h1>\n"));
    out.push_str("<div class=\"meta\">\n");
    out.push_str(&format!(
        "<div><strong>Session ID</strong><code>{}</code></div>\n",
        html_escape(&session.id)
    ));
    if let Some(name) = &session.short_name {
        out.push_str(&format!(
            "<div><strong>Name</strong><code>{}</code></div>\n",
            html_escape(name)
        ));
    }
    if let Some(provider) = &session.provider_key {
        out.push_str(&format!(
            "<div><strong>Provider</strong><code>{}</code></div>\n",
            html_escape(provider)
        ));
    }
    if let Some(model) = &session.model {
        out.push_str(&format!(
            "<div><strong>Model</strong><code>{}</code></div>\n",
            html_escape(model)
        ));
    }
    out.push_str(&format!(
        "<div><strong>Created</strong>{}</div>\n",
        html_escape(&session.created_at.to_rfc3339())
    ));
    out.push_str(&format!(
        "<div><strong>Updated</strong>{}</div>\n",
        html_escape(&session.updated_at.to_rfc3339())
    ));
    out.push_str(&format!(
        "<div><strong>Messages</strong>{}</div>\n",
        session.messages.len()
    ));
    out.push_str("</div>\n");

    if let Some(compaction) = session.compaction.as_ref() {
        let kind = if compaction.openai_encrypted_content.is_some() {
            "native/openai-encrypted"
        } else if !compaction.summary_text.is_empty() {
            "summary-text"
        } else {
            "none"
        };
        out.push_str(&format!(
            "<div class=\"compaction-note\">Active compaction artifact present (<code>{}</code> — {} chars).</div>\n",
            html_escape(kind),
            artifact_chars(compaction)
        ));
    }

    for (idx, msg) in session.messages.iter().enumerate() {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        let timestamp = msg
            .timestamp
            .map(|t| format!(" · {}", t.format("%Y-%m-%d %H:%M:%S")))
            .unwrap_or_default();
        out.push_str("<section class=\"message\">\n");
        out.push_str(&format!(
            "<header>#{idx} <span class=\"role\">{}</span>{}</header>\n",
            role_label,
            html_escape(&timestamp)
        ));
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        out.push_str(&format!(
                            "<div class=\"text\">{}</div>\n",
                            html_escape(trimmed)
                        ));
                    }
                }
                ContentBlock::Reasoning { text } => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        out.push_str(&format!(
                            "<details><summary>thinking</summary>\n<div class=\"text\">{}</div>\n</details>\n",
                            html_escape(trimmed)
                        ));
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let pretty =
                        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                    out.push_str(&format!(
                        "<details><summary>tool: <code>{}</code></summary>\n<pre><code>{}</code></pre>\n</details>\n",
                        html_escape(name),
                        html_escape(&pretty)
                    ));
                }
                ContentBlock::ToolResult { content, .. } => {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        out.push_str(&format!(
                            "<details><summary>tool result</summary>\n<pre><code>{}</code></pre>\n</details>\n",
                            html_escape(trimmed)
                        ));
                    }
                }
                ContentBlock::Image { media_type, .. } => {
                    out.push_str(&format!(
                        "<div class=\"text\"><em>[image: {}]</em></div>\n",
                        html_escape(media_type)
                    ));
                }
                ContentBlock::OpenAICompaction { encrypted_content } => {
                    out.push_str(&format!(
                        "<div class=\"text\"><em>[OpenAI native compaction artifact: {} chars]</em></div>\n",
                        encrypted_content.len()
                    ));
                }
                ContentBlock::AnthropicThinking { thinking, .. } => {
                    let trimmed = thinking.trim();
                    if !trimmed.is_empty() {
                        out.push_str(&format!(
                            "<details><summary>thinking</summary>\n<div class=\"text\">{}</div>\n</details>\n",
                            html_escape(trimmed)
                        ));
                    }
                }
                ContentBlock::OpenAIReasoning { summary, .. } => {
                    let joined = summary.join("\n");
                    let trimmed = joined.trim();
                    if !trimmed.is_empty() {
                        out.push_str(&format!(
                            "<details><summary>reasoning</summary>\n<div class=\"text\">{}</div>\n</details>\n",
                            html_escape(trimmed)
                        ));
                    }
                }
            }
        }
        out.push_str("</section>\n");
    }

    out.push_str("</body></html>\n");
    out
}

fn artifact_chars(compaction: &jcode_session_types::StoredCompactionState) -> usize {
    compaction
        .openai_encrypted_content
        .as_ref()
        .map(|s| s.len())
        .unwrap_or_else(|| compaction.summary_text.len())
}

fn render_stored_message(out: &mut String, idx: usize, msg: &crate::session::StoredMessage) {
    let role_label = match msg.role {
        Role::User => "User",
        Role::Assistant => "Assistant",
    };
    let timestamp = msg
        .timestamp
        .map(|t| format!(" · {}", t.format("%Y-%m-%d %H:%M:%S")))
        .unwrap_or_default();
    out.push_str(&format!("## #{idx} {role_label}{timestamp}\n\n"));

    use jcode_message_types::ContentBlock;
    for block in &msg.content {
        match block {
            ContentBlock::Text { text, .. } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push_str(trimmed);
                    out.push_str("\n\n");
                }
            }
            ContentBlock::Reasoning { text } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push_str("<details><summary>thinking</summary>\n\n");
                    out.push_str(trimmed);
                    out.push_str("\n\n</details>\n\n");
                }
            }
            ContentBlock::ToolUse { name, input, .. } => {
                let pretty =
                    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                out.push_str(&format!(
                    "<details><summary>tool: <code>{name}</code></summary>\n\n```json\n{pretty}\n```\n\n</details>\n\n"
                ));
            }
            ContentBlock::ToolResult { content, .. } => {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    out.push_str("<details><summary>tool result</summary>\n\n```\n");
                    out.push_str(trimmed);
                    out.push_str("\n```\n\n</details>\n\n");
                }
            }
            ContentBlock::Image { media_type, .. } => {
                out.push_str(&format!("_[image: {media_type}]_\n\n"));
            }
            ContentBlock::OpenAICompaction { encrypted_content } => {
                out.push_str(&format!(
                    "_[OpenAI native compaction artifact: {} chars]_\n\n",
                    encrypted_content.len()
                ));
            }
            ContentBlock::AnthropicThinking { thinking, .. } => {
                let trimmed = thinking.trim();
                if !trimmed.is_empty() {
                    out.push_str("<details><summary>thinking</summary>\n\n");
                    out.push_str(trimmed);
                    out.push_str("\n\n</details>\n\n");
                }
            }
            ContentBlock::OpenAIReasoning { summary, .. } => {
                let joined = summary.join("\n");
                let trimmed = joined.trim();
                if !trimmed.is_empty() {
                    out.push_str("<details><summary>reasoning</summary>\n\n");
                    out.push_str(trimmed);
                    out.push_str("\n\n</details>\n\n");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::{ContentBlock, Role};

    fn fake_session() -> crate::session::Session {
        let mut s = crate::session::Session::create_with_id(
            "session_test_export".to_string(),
            None,
            Some("Test Export".to_string()),
        );
        s.model = Some("gpt-5.5".to_string());
        s.messages.push(crate::session::StoredMessage {
            id: "m1".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello jcode".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        s.messages.push(crate::session::StoredMessage {
            id: "m2".to_string(),
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: "thinking step 1".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file": "src/main.rs"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "fn main() {}".to_string(),
                    is_error: Some(false),
                },
                ContentBlock::Text {
                    text: "Done.".to_string(),
                    cache_control: None,
                },
            ],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        s
    }

    #[test]
    fn markdown_includes_title_metadata_and_messages() {
        let s = fake_session();
        let md = render_markdown(&s);
        assert!(md.starts_with("# Test Export\n"));
        assert!(md.contains("- **Session ID**: `session_test_export`"));
        assert!(md.contains("- **Model**: `gpt-5.5`"));
        assert!(md.contains("- **Messages**: 2"));
        assert!(md.contains("## #0 User"));
        assert!(md.contains("hello jcode"));
        assert!(md.contains("## #1 Assistant"));
        assert!(md.contains("Done."));
    }

    #[test]
    fn markdown_collapses_thinking_and_tools_in_details() {
        let s = fake_session();
        let md = render_markdown(&s);
        assert!(md.contains("<details><summary>thinking</summary>"));
        assert!(md.contains("thinking step 1"));
        assert!(md.contains("<details><summary>tool: <code>read</code></summary>"));
        assert!(md.contains("\"file\""));
        assert!(md.contains("<details><summary>tool result</summary>"));
        assert!(md.contains("fn main() {}"));
    }

    #[test]
    fn slugify_keeps_alpha_drops_punct() {
        assert_eq!(slugify("Test Export!"), "test-export");
        assert_eq!(slugify("a/b c"), "a-b-c");
        assert_eq!(slugify("___"), "");
        assert_eq!(slugify(""), "");
    }

    // ---- HTML render tests ----

    #[test]
    fn html_escape_handles_dangerous_chars() {
        assert_eq!(
            html_escape("<script>alert(\"xss\")</script>"),
            "&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;"
        );
        assert_eq!(html_escape("a & b"), "a &amp; b");
        assert_eq!(html_escape("'single'"), "&#39;single&#39;");
        // Non-dangerous unicode passes through.
        assert_eq!(html_escape("café 🚀"), "café 🚀");
    }

    #[test]
    fn render_html_self_contained_doc() {
        let s = fake_session();
        let html = render_html(&s);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>Test Export — jcode session</title>"));
        assert!(html.contains("<style>"));
        // Inline CSS, no external refs.
        assert!(!html.contains("<link "));
        assert!(!html.contains("<script"));
        // Meta block is present.
        assert!(html.contains("<strong>Session ID</strong>"));
        // Ends with closing tags.
        assert!(html.trim_end().ends_with("</body></html>"));
    }

    #[test]
    fn render_html_escapes_user_supplied_text() {
        // Build a session whose text content is literally an XSS payload.
        let mut s = fake_session();
        s.messages.push(crate::session::StoredMessage {
            id: "m_xss".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "<img src=x onerror=alert(1)>".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        let html = render_html(&s);
        assert!(!html.contains("<img src=x onerror=alert(1)>"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    }

    #[test]
    fn render_html_uses_details_for_thinking_and_tools() {
        let mut s = fake_session();
        s.messages.push(crate::session::StoredMessage {
            id: "m_think".to_string(),
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: "internal thought".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "shell".to_string(),
                    input: serde_json::json!({"cmd": "ls"}),
                },
            ],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        let html = render_html(&s);
        assert!(html.contains("<details><summary>thinking</summary>"));
        assert!(html.contains("<details><summary>tool: <code>shell</code></summary>"));
    }

    #[test]
    fn render_html_round_trips_via_run_and_redact() {
        // End-to-end: format=Html + redact=true should both produce HTML and
        // mask any embedded secrets. Here we feed in a known sk-* token and
        // assert it's gone from the rendered output.
        let mut s = fake_session();
        s.messages.push(crate::session::StoredMessage {
            id: "m_secret".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "The key is sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        let html = render_html(&s);
        let redacted = redact_secrets(&html);
        assert!(redacted.contains("[REDACTED:sk]"));
        assert!(!redacted.contains("sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    // ---- redact_secrets tests ----

    #[test]
    fn redact_replaces_sk_keys() {
        let input = "key=sk-ant-api03-abc123_DEFghi-xyz890_more later text";
        let out = redact_secrets(input);
        assert!(out.contains("[REDACTED:sk]"), "got: {out}");
        assert!(!out.contains("sk-ant-api03"));
        assert!(out.contains("later text"));
    }

    #[test]
    fn redact_replaces_github_tokens() {
        for prefix in ["gho_", "ghp_", "ghs_", "ghr_", "ghu_"] {
            let token = format!("{prefix}{}", "a".repeat(36));
            let out = redact_secrets(&token);
            assert!(
                out.contains("[REDACTED:github]"),
                "{prefix} not redacted: {out}"
            );
        }
    }

    #[test]
    fn redact_keeps_bearer_label_drops_token() {
        let out = redact_secrets("Authorization: Bearer abcdef0123456789xyz_test");
        assert!(out.contains("Bearer [REDACTED:bearer]"));
        assert!(!out.contains("abcdef0123456789xyz_test"));
    }

    #[test]
    fn redact_zai_shape_token() {
        // 32 hex . 24+ alnum
        let token = "6e915ba766fb4c3bbe4cce3b58a75523.rrc5r2uvVFFXg4ZE";
        let out = redact_secrets(&format!("token={token}"));
        assert!(out.contains("[REDACTED:zai]"), "got: {out}");
        assert!(!out.contains(token));
    }

    #[test]
    fn redact_env_var_assignments() {
        let input = r#"
ANTHROPIC_API_KEY=sk-ant-x12345678901234567890
OPENAI_API_KEY="sk-proj-y9876543210987654321"
GITHUB_TOKEN=gho_abcdefghijklmnopqrstuvwxyz1234
ZHIPU_API_KEY=mySecretToken12345
DEEPSEEK_API_KEY=anotherSecret67890
"#;
        let out = redact_secrets(input);
        // Each named env var should be redacted.
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "ZHIPU_API_KEY",
            "DEEPSEEK_API_KEY",
        ] {
            assert!(out.contains(&name.to_string()), "{name} name lost: {out}");
        }
        assert!(!out.contains("anotherSecret67890"));
        assert!(!out.contains("mySecretToken12345"));
    }

    #[test]
    fn redact_preserves_non_secret_text() {
        let input = "The function `read_file` returned 42 bytes. No secrets here.";
        assert_eq!(redact_secrets(input), input);
    }
}
