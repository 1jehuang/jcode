use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyAction {
    PermissionDecision {
        request_id: String,
        approved: bool,
        message: Option<String>,
    },
    DirectiveReply {
        cycle_id: String,
        text: String,
    },
}

pub struct SendEmailRequest<'a> {
    pub smtp_host: &'a str,
    pub smtp_port: u16,
    pub from: &'a str,
    pub to: &'a str,
    pub password: Option<&'a str>,
    pub subject: &'a str,
    pub body: &'a str,
    pub cycle_id: Option<&'a str>,
    pub html_override: Option<&'a str>,
}

pub async fn send_email(request: SendEmailRequest<'_>) -> Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let html_body = match request.html_override {
        Some(html) => html.to_string(),
        None => markdown_to_html_email(request.body),
    };

    let mut builder = Message::builder()
        .from(request.from.parse()?)
        .to(request.to.parse()?)
        .subject(request.subject)
        .header(ContentType::TEXT_HTML);

    if let Some(cid) = request.cycle_id {
        let msg_id = format!("<ambient-{}@jcode>", cid);
        builder = builder.message_id(Some(msg_id));
    }

    let email = builder.body(html_body)?;

    let mut transport_builder =
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(request.smtp_host)?
            .port(request.smtp_port);

    if let Some(pw) = request.password {
        transport_builder = transport_builder
            .credentials(Credentials::new(request.from.to_string(), pw.to_string()));
    }

    let transport = transport_builder.build();
    transport.send(email).await?;
    Ok(())
}

pub fn poll_imap_once(host: &str, port: u16, user: &str, pass: &str) -> Result<Vec<ReplyAction>> {
    let client = imap::ClientBuilder::new(host, port).connect()?;
    let mut session = client
        .login(user, pass)
        .map_err(|(e, _)| anyhow::anyhow!("IMAP login failed: {}", e))?;

    session.select("INBOX")?;

    let reply_search = session.search("UNSEEN HEADER In-Reply-To \"@jcode>\"")?;
    let button_search = session.search("UNSEEN SUBJECT \"[jcode-perm:\"")?;

    let mut all_seqs: Vec<_> = reply_search.into_iter().chain(button_search).collect();
    all_seqs.sort_unstable();
    all_seqs.dedup();

    if all_seqs.is_empty() {
        session.logout()?;
        return Ok(Vec::new());
    }

    let seq_set: String = all_seqs
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let mut actions = Vec::new();
    let messages = session.fetch(&seq_set, "RFC822")?;
    for message in messages.iter() {
        if let Some(body) = message.body()
            && let Some(parsed) = mail_parser::MessageParser::default().parse(body)
        {
            let in_reply_to = parsed.in_reply_to().as_text().unwrap_or("").to_string();
            let subject = parsed.subject().unwrap_or("");

            let cycle_id = if in_reply_to.contains("@jcode>") {
                in_reply_to
                    .trim_start_matches("<ambient-")
                    .trim_end_matches("@jcode>")
                    .to_string()
            } else if let Some(start) = subject.find("[jcode-perm:") {
                let rest = &subject[start + "[jcode-perm:".len()..];
                rest.split(']').next().unwrap_or("").to_string()
            } else {
                continue;
            };

            let body_text = parsed
                .body_text(0)
                .map(|s| strip_quoted_reply(&s))
                .unwrap_or_default();

            let effective_text = if body_text.trim().is_empty() {
                subject.to_string()
            } else {
                body_text
            };

            if effective_text.trim().is_empty() {
                continue;
            }

            if cycle_id.starts_with("req_") {
                let (approved, message) = parse_permission_reply(effective_text.trim());
                actions.push(ReplyAction::PermissionDecision {
                    request_id: cycle_id,
                    approved,
                    message,
                });
            } else {
                actions.push(ReplyAction::DirectiveReply {
                    cycle_id,
                    text: effective_text.trim().to_string(),
                });
            }
        }
    }

    session.store(&seq_set, "+FLAGS (\\Seen)")?;
    session.logout()?;
    Ok(actions)
}

pub fn extract_permission_id(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for word in lower.split_whitespace() {
        if word.starts_with("req_") {
            return Some(word.to_string());
        }
    }
    None
}

pub fn parse_permission_reply(text: &str) -> (bool, Option<String>) {
    let lower = text.to_lowercase();
    let first_line = lower.lines().next().unwrap_or("").trim();

    let approve_words = [
        "approve", "approved", "yes", "lgtm", "go ahead", "ok", "sure",
    ];
    let deny_words = ["deny", "denied", "no", "reject", "rejected", "stop", "nope"];

    let has_approve = approve_words.iter().any(|w| first_line.contains(w));
    let has_deny = deny_words.iter().any(|w| first_line.contains(w));
    let approved = has_approve && !has_deny;

    let message = if text.trim().len() > 20 {
        Some(text.trim().to_string())
    } else {
        None
    };

    (approved, message)
}

pub fn build_permission_email_html(
    action: &str,
    description: &str,
    request_id: &str,
    reply_to: &str,
) -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y-%m-%d %H:%M:%S UTC").to_string();

    let approve_subj_raw = format!("[jcode-perm:{}] Approved", request_id);
    let deny_subj_raw = format!("[jcode-perm:{}] Denied", request_id);
    let approve_subject = urlencoding::encode(&approve_subj_raw);
    let deny_subject = urlencoding::encode(&deny_subj_raw);
    let approve_body = urlencoding::encode("Approved");
    let deny_body = urlencoding::encode("Denied");

    let approve_href = format!(
        "mailto:{}?subject={}&body={}",
        reply_to, approve_subject, approve_body
    );
    let deny_href = format!(
        "mailto:{}?subject={}&body={}",
        reply_to, deny_subject, deny_body
    );

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  body {{
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    color: #1a1a1a;
    line-height: 1.6;
    max-width: 640px;
    margin: 0 auto;
    padding: 20px;
    background: #f5f5f5;
  }}
  .container {{
    background: #ffffff;
    border-radius: 8px;
    padding: 24px 28px;
    border: 1px solid #e0e0e0;
  }}
  h1 {{
    font-size: 1.3em;
    color: #2d2d2d;
    border-bottom: 2px solid #f59e0b;
    padding-bottom: 6px;
    margin-top: 0;
  }}
  .field {{
    margin-bottom: 12px;
  }}
  .field-label {{
    font-weight: 600;
    color: #555;
    font-size: 0.85em;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }}
  .field-value {{
    margin-top: 2px;
    color: #1a1a1a;
  }}
  .request-id {{
    font-family: monospace;
    background: #f0f0f0;
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 0.85em;
  }}
  .buttons {{
    margin-top: 24px;
    text-align: center;
  }}
  .btn {{
    display: inline-block;
    padding: 12px 32px;
    border-radius: 6px;
    text-decoration: none;
    font-weight: 600;
    font-size: 1em;
    margin: 0 8px;
  }}
  .btn-approve {{
    background: #22c55e;
    color: #ffffff;
  }}
  .btn-deny {{
    background: #ef4444;
    color: #ffffff;
  }}
  .timestamp {{
    margin-top: 16px;
    font-size: 0.8em;
    color: #888;
  }}
  .hint {{
    margin-top: 8px;
    font-size: 0.8em;
    color: #999;
    font-style: italic;
  }}
  .footer {{
    margin-top: 20px;
    padding-top: 12px;
    border-top: 1px solid #e0e0e0;
    font-size: 0.8em;
    color: #888;
  }}
</style>
</head>
<body>
<div class="container">
  <h1>Permission Request</h1>
  <div class="field">
    <div class="field-label">Action</div>
    <div class="field-value"><strong>{action}</strong></div>
  </div>
  <div class="field">
    <div class="field-label">Description</div>
    <div class="field-value">{description}</div>
  </div>
  <div class="field">
    <div class="field-label">Request ID</div>
    <div class="field-value"><span class="request-id">{request_id}</span></div>
  </div>
  <div class="buttons">
    <a href="{approve_href}" class="btn btn-approve">Approve</a>
    <a href="{deny_href}" class="btn btn-deny">Deny</a>
  </div>
  <div class="hint">Clicking opens a pre-filled email — just hit Send.</div>
  <div class="hint">Or reply to this email with "Approved" or "Denied".</div>
  <div class="timestamp">Sent at {timestamp}</div>
</div>
<div class="footer">
  Sent by jcode ambient mode
</div>
</body>
</html>"#
    )
}

fn markdown_to_html_email(markdown: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(markdown, options);
    let mut html_content = String::new();
    html::push_html(&mut html_content, parser);

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  body {{
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    color: #1a1a1a;
    line-height: 1.6;
    max-width: 640px;
    margin: 0 auto;
    padding: 20px;
    background: #f5f5f5;
  }}
  .container {{
    background: #ffffff;
    border-radius: 8px;
    padding: 24px 28px;
    border: 1px solid #e0e0e0;
  }}
  h1, h2, h3 {{
    color: #2d2d2d;
    margin-top: 1.2em;
    margin-bottom: 0.4em;
  }}
  h1 {{ font-size: 1.3em; border-bottom: 2px solid #6366f1; padding-bottom: 6px; }}
  h2 {{ font-size: 1.1em; }}
  strong {{ color: #111; }}
  ul, ol {{ padding-left: 1.4em; }}
  li {{ margin-bottom: 4px; }}
  code {{
    background: #f0f0f0;
    padding: 2px 5px;
    border-radius: 3px;
    font-size: 0.9em;
  }}
  pre {{
    background: #1e1e2e;
    color: #cdd6f4;
    padding: 12px 16px;
    border-radius: 6px;
    overflow-x: auto;
    font-size: 0.85em;
  }}
  pre code {{
    background: none;
    padding: 0;
    color: inherit;
  }}
  table {{
    border-collapse: collapse;
    width: 100%;
    margin: 1em 0;
  }}
  th, td {{
    border: 1px solid #ddd;
    padding: 6px 10px;
    text-align: left;
  }}
  th {{ background: #f8f8f8; font-weight: 600; }}
  .footer {{
    margin-top: 20px;
    padding-top: 12px;
    border-top: 1px solid #e0e0e0;
    font-size: 0.8em;
    color: #888;
  }}
</style>
</head>
<body>
<div class="container">
{html_content}
</div>
<div class="footer">
  Sent by jcode ambient mode
</div>
</body>
</html>"#
    )
}

fn strip_quoted_reply(text: &str) -> String {
    text.lines()
        .take_while(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('>')
                && trimmed != "--"
                && trimmed != "-- "
                && !trimmed.starts_with("On ")
                || trimmed.is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_html_email() {
        let md = "**Ambient Cycle Summary:**\n\n- Cleaned 3 memories\n- Status: Complete\n";
        let html = markdown_to_html_email(md);
        assert!(html.contains("<strong>Ambient Cycle Summary:</strong>"));
        assert!(html.contains("<li>"));
        assert!(html.contains("jcode ambient mode"));
    }

    #[test]
    fn test_strip_quoted_reply() {
        let email = "Thanks, please clean up the test data.\n\n> On Mon, Feb 9, 2026 jcode wrote:\n> Ambient cycle complete.\n";
        let stripped = strip_quoted_reply(email);
        assert!(stripped.contains("clean up the test data"));
        assert!(!stripped.contains("Ambient cycle complete"));
    }

    #[test]
    fn test_strip_quoted_reply_signature() {
        let email = "Focus on memory gardening.\n--\nJeremy\n";
        let stripped = strip_quoted_reply(email);
        assert!(stripped.contains("Focus on memory gardening"));
        assert!(!stripped.contains("Jeremy"));
    }

    #[test]
    fn test_parse_permission_reply_approve() {
        let (approved, _) = parse_permission_reply("Yes, go ahead");
        assert!(approved);
        let (approved, _) = parse_permission_reply("Approved");
        assert!(approved);
        let (approved, _) = parse_permission_reply("LGTM");
        assert!(approved);
        let (approved, _) = parse_permission_reply("sure thing");
        assert!(approved);
        let (approved, _) = parse_permission_reply("ok");
        assert!(approved);
    }

    #[test]
    fn test_parse_permission_reply_deny() {
        let (approved, _) = parse_permission_reply("No, too risky");
        assert!(!approved);
        let (approved, _) = parse_permission_reply("Denied");
        assert!(!approved);
        let (approved, _) = parse_permission_reply("reject this");
        assert!(!approved);
        let (approved, _) = parse_permission_reply("nope");
        assert!(!approved);
        let (approved, _) = parse_permission_reply("Stop, don't do that");
        assert!(!approved);
    }

    #[test]
    fn test_parse_permission_reply_ambiguous_defaults_deny() {
        let (approved, _) = parse_permission_reply("hmm let me think about it");
        assert!(!approved);
        let (approved, _) = parse_permission_reply("");
        assert!(!approved);
    }

    #[test]
    fn test_parse_permission_reply_message() {
        let (_, message) = parse_permission_reply("yes");
        assert!(message.is_none());

        let (_, message) =
            parse_permission_reply("Approved, but please use a feature branch for this");
        assert!(message.is_some());
    }

    #[test]
    fn test_extract_permission_id() {
        assert_eq!(
            extract_permission_id("approve req_123"),
            Some("req_123".to_string())
        );
        assert_eq!(extract_permission_id("nothing here"), None);
    }

    #[test]
    fn test_build_permission_email_html() {
        let html = build_permission_email_html(
            "apply patch",
            "Touch Cargo.toml",
            "req_123",
            "jcode@example.com",
        );
        assert!(html.contains("Permission Request"));
        assert!(html.contains("req_123"));
        assert!(html.contains("mailto:jcode@example.com"));
    }
}
