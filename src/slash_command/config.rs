use super::{register, SlashResult};

fn s<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}

pub(crate) async fn register_model() {
    register("model", "Show or switch AI model", "/model [model-name]",
        std::sync::Arc::new(|args: &str| {
            let a = args.trim().to_string();
            s(move || async move {
                let cfg = crate::config::Config::load();
                if a.is_empty() {
                    eprintln!("\n📋 Model\n  Provider: {}\n  Model:    {}\n", cfg.provider.default_provider.as_deref().unwrap_or("not set"), cfg.provider.default_model.as_deref().unwrap_or("not set"));
                } else {
                    match crate::config::Config::set_default_model_only(Some(a.trim())) {
                        Ok(_) => eprintln!("\n✅ Model changed to: {}\n", a.trim()),
                        Err(e) => eprintln!("\n❌ {}\n", e),
                    }
                }
            });
            if a.is_empty() { SlashResult::Ok("Showing model config.".into()) } else { SlashResult::Ok(format!("Setting model to: {}", a.trim())) }
        }),
    ).await;
}

pub(crate) async fn register_config() {
    register("config", "View/set config", "/config [get|set <k> <v>]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            s(move || async move {
                let parts: Vec<&str> = a.trim().splitn(3, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "get" if parts.len() >= 2 => { let v = std::env::var(parts[1]).ok(); eprintln!("\n  {} = {}\n", parts[1], v.as_deref().unwrap_or("(not set)")); }
                    "set" if parts.len() >= 3 => { unsafe { std::env::set_var(parts[1], parts[2]) }; eprintln!("\n✅ {} = {}\n", parts[1], parts[2]); }
                    _ => eprintln!("\nUsage: /config get <var> | /config set <var> <val>\n  /model <name> to switch model\n"),
                }
            });
            SlashResult::Ok("Config.".into())
        }),
    ).await;
}

pub(crate) async fn register_env() {
    register("env", "View environment", "/env [var]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            let trimmed = a.trim().to_string();
            if trimmed.is_empty() {
                let mut vars: Vec<_> = std::env::vars().filter(|(k,_)| k.starts_with("CARPAI")||k.starts_with("JCODE")||k.starts_with("ANTHROPIC")||k.starts_with("OPENAI")).collect();
                vars.sort_by(|a,b| a.0.cmp(&b.0));
                eprintln!("\n📋 Env\n");
                for (k,v) in &vars { eprintln!("  {}={}", k, v); }
                eprintln!();
            } else {
                match std::env::var(&trimmed) { Ok(v) => eprintln!("\n  {} = {}\n", trimmed, v), Err(_) => eprintln!("\n  {} (not set)\n", trimmed) }
            }
            SlashResult::Ok("Env.".into())
        }),
    ).await;
}
