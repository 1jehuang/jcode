use super::{register, SlashResult, list, lookup};

pub(crate) async fn register() {
    register(
        "help",
        "Show available slash commands",
        "/help [command]",
        std::sync::Arc::new(|args: &str| {
            let rt = match tokio::runtime::Handle::try_current() {
                Ok(h) => h,
                Err(_) => return SlashResult::Err("No async runtime".into()),
            };
            let cmd = args.trim().to_string();
            let result = rt.block_on(async move {
                if cmd.is_empty() {
                    let cmds = list().await;
                    let mut out = String::from("Available slash commands (30+):\n");
                    for c in &cmds {
                        out.push_str(&format!("  /{:<12} {}\n", c.name, c.description));
                    }
                    out.push_str("\nUse /help <name> for details.\n");
                    out
                } else {
                    match lookup(&cmd).await {
                        Some(info) => format!("  /{} — {}\n  Usage: {}\n", info.name, info.description, info.usage),
                        None => format!("Unknown command: /{}", cmd),
                    }
                }
            });
            SlashResult::Ok(result)
        }),
    )
    .await;
}
