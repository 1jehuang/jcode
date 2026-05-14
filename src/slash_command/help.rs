use super::{register as reg, SlashResult, list, lookup};

pub(crate) async fn register() {
    reg("help", "Show available slash commands", "/help [command]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            let rt = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            let result = rt.block_on(async move {
                if a.trim().is_empty() {
                    let cmds = list().await;
                    let mut out = format!("Available slash commands ({}):\n", cmds.len());
                    for c in &cmds { out.push_str(&format!("  /{:<12} {}\n", c.name, c.description)); }
                    out.push_str("\n/help <name> for details.\n");
                    out
                } else {
                    match lookup(a.trim()).await {
                        Some(i) => format!("  /{} — {}\n  Usage: {}\n", i.name, i.description, i.usage),
                        None => format!("Unknown: /{}", a.trim()),
                    }
                }
            });
            SlashResult::Ok(result)
        }),
    ).await;
}
