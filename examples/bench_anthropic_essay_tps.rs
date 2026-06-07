use anyhow::Result;
use futures::StreamExt;
use jcode::message::{ContentBlock, Message, Role, StreamEvent};
use jcode::provider::Provider;
use jcode::provider::anthropic::AnthropicProvider;
use std::time::Instant;

async fn run_one_with_retry(
    provider: &AnthropicProvider,
    label: &str,
    words: usize,
    retries: usize,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        match run_one(provider, label, words).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let msg = e.to_string();
                let is_rate_limit = msg.contains("429") || msg.contains("rate_limit");
                if is_rate_limit && attempt < retries {
                    attempt += 1;
                    let backoff = 30u64 * attempt as u64;
                    eprintln!(
                        "[{label}] rate limited (attempt {attempt}/{retries}); waiting {backoff}s..."
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
}

async fn run_one(provider: &AnthropicProvider, label: &str, words: usize) -> Result<()> {
    let prompt = format!(
        "Write a very long essay of at least {words} words about the architecture, maintainability, reliability, performance, testing strategy, provider abstraction, TUI complexity, security model, and long-term engineering risks of a Rust terminal AI coding agent codebase like jcode. Be specific and detailed. Do not use tools. Do not stop early."
    );
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt,
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];
    let start = Instant::now();
    let mut first_ms = None;
    let mut last_ms = None;
    let mut chars = 0usize;
    let mut input_tokens = None;
    let mut output_tokens = None;
    let mut cache_read = None;
    let mut cache_write = None;
    let mut stream = provider.complete(&messages, &[], "", None).await?;
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta(text) => {
                let now = start.elapsed().as_millis();
                first_ms.get_or_insert(now);
                last_ms = Some(now);
                chars += text.len();
            }
            StreamEvent::TokenUsage {
                input_tokens: it,
                output_tokens: ot,
                cache_read_input_tokens: cr,
                cache_creation_input_tokens: cw,
            } => {
                if it.is_some() {
                    input_tokens = it;
                }
                if ot.is_some() {
                    output_tokens = ot;
                }
                if cr.is_some() {
                    cache_read = cr;
                }
                if cw.is_some() {
                    cache_write = cw;
                }
            }
            StreamEvent::Error { message, .. } => anyhow::bail!(message),
            _ => {}
        }
    }
    let total_ms = start.elapsed().as_millis();
    let first = first_ms.unwrap_or(total_ms);
    let last = last_ms.unwrap_or(total_ms);
    let gen_ms = last.saturating_sub(first).max(1);
    let out = output_tokens.unwrap_or(0);
    let gen_tps = out as f64 / (gen_ms as f64 / 1000.0);
    let total_tps = out as f64 / (total_ms.max(1) as f64 / 1000.0);
    println!(
        "{label},{first},{last},{total_ms},{gen_ms},{chars},{},{},{},{},{gen_tps:.2},{total_tps:.2}",
        input_tokens.map(|v| v.to_string()).unwrap_or_default(),
        out,
        cache_read.map(|v| v.to_string()).unwrap_or_default(),
        cache_write.map(|v| v.to_string()).unwrap_or_default()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let words = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(3000);

    // Force the direct Anthropic API-key path when requested (or when an API
    // key is present and OAuth is not), so fast mode is exercised on the
    // Console API rather than the subscription OAuth route. Fast mode / priority
    // tier is gated by usage credits on the API account.
    let force_api_key = std::env::var("BENCH_ANTHROPIC_API_KEY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    println!(
        "tier,first_ms,last_text_ms,total_ms,generation_ms,chars,input_tokens,output_tokens,cache_read,cache_write,gen_output_tok_s,total_output_tok_s"
    );
    let standard = AnthropicProvider::new();
    standard.set_model("claude-opus-4-8")?;
    standard.set_service_tier("off")?;
    let fast = AnthropicProvider::new();
    fast.set_model("claude-opus-4-8")?;
    fast.set_service_tier("priority")?;

    if force_api_key {
        // false = API key (not OAuth)
        standard.pin_credential_mode_for_doctor(false)?;
        fast.pin_credential_mode_for_doctor(false)?;
        eprintln!("[bench] forcing direct Anthropic API-key credential mode");
    }

    run_one_with_retry(&standard, "standard_only", words, 4).await?;
    // Cool-down gap to avoid back-to-back rate limiting between the two runs.
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
    run_one_with_retry(&fast, "auto", words, 4).await?;
    Ok(())
}
