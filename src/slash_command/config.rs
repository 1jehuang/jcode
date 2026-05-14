use super::{register, SlashResult};

pub(crate) async fn register_model() {
    register("model", "Show or switch the AI model",
        "/model [model-name]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let cfg = crate::config::Config::load();
                let trimmed = args.trim();
                if trimmed.is_empty() {
                    eprintln!("\n📋 Model Config\n  Provider: {}\n  Model:    {}\n",
                        cfg.provider.default_provider.as_deref().unwrap_or("not set"),
                        cfg.provider.default_model.as_deref().unwrap_or("not set"));
                } else {
                    match crate::config::Config::set_default_model_only(Some(trimmed)) {
                        Ok(_) => eprintln!("\n✅ Default model changed to: {}\n", trimmed),
                        Err(e) => eprintln!("\n❌ Failed: {}\n", e),
                    }
                }
            });
            if args.trim().is_empty() { SlashResult::Ok("Showing model config.".into()) }
            else { SlashResult::Ok(format!("Setting model to: {}", args.trim())) }
        }),
    ).await;
}

pub(crate) async fn register_config() {
    register("config", "View or set configuration",
        "/config [get|set <key> <value>]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().splitn(3, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "get" if parts.len() >= 2 => {
                        let val = std::env::var(parts[1]).ok();
                        eprintln!("\n  {} = {}\n", parts[1], val.as_deref().unwrap_or("(not set)"));
                    }
                    "set" if parts.len() >= 3 => { std::env::set_var(parts[1], parts[2]); eprintln!("\n✅ {} = {}\n", parts[1], parts[2]); }
                    _ => {
                        eprintln!("\n📋 Config usage:\n  /config get <VAR>     Read env var\n  /config set <VAR> <V>  Set env var (session only)\n  /model <name>         Switch model\n");
                    }
                }
            });
            SlashResult::Ok("Config command.".into())
        }),
    ).await;
}

pub(crate) async fn register_env() {
    register("env", "View environment variables",
        "/env [var-name]",
        std::sync::Arc::new(|args: &str| {
            let trimmed = args.trim();
            if trimmed.is_empty() {
                eprintln!("\n📋 Environment\n");
                let mut vars: Vec<_> = std::env::vars().collect();
                vars.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in vars.iter().filter(|(k, _)| k.starts_with("CARPAI") || k.starts_with("JCODE") || k.starts_with("ANTHROPIC") || k.starts_with("OPENAI")) {
                    eprintln!("  {}={}", k, v);
                }
                eprintln!();
            } else {
                match std::env::var(trimmed) {
                    Ok(v) => eprintln!("\n  {} = {}\n", trimmed, v),
                    Err(_) => eprintln!("\n  {} (not set)\n", trimmed),
                }
            }
            SlashResult::Ok("Env info.".into())
        }),
    ).await;
}

fn spawn_async<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}
