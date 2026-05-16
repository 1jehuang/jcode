# Telegram Bridge — Connect Jcode to Telegram

This directory provides a bridge between Jcode and Telegram, allowing users to message a Jcode instance directly via a Telegram bot.

## Architecture

```
Telegram user → Bot API → Bridge polls getUpdates → Injects into Jcode debug socket → Jcode responds naturally → Bridge polls last_response → Sends back to Telegram
```

- **Zero dependencies** — pure Python stdlib
- **No webhooks needed** — simple long-polling
- **Direct session injection** — messages appear in Jcode's terminal as `📩 *Telegram from X*: ...`
- **Auto .env loading** — bridge reads config from `~/.jcode/telegram/.env` on startup

## Quick Start

```bash
# Install
bash <(curl -s https://raw.githubusercontent.com/LAG-Yadav/jcode-telegram-bridge/main/install.sh)

# Or after cloning the jcode repo:
cp -r scripts/telegram-bridge ~/.jcode/telegram

# Configure
nano ~/.jcode/telegram/.env
# Set: TELEGRAM_BOT_TOKEN=your_token_here

# Start
tgstart
```

## Prerequisites

1. **Jcode** with `debug_socket` enabled in `~/.jcode/config.toml`:
   ```toml
   [display]
   debug_socket = true
   ```
2. A **Telegram bot token** from [@BotFather](https://t.me/botfather)
3. **Python 3** (stdlib only — no pip packages needed)

## Files

| File | Purpose |
|------|---------|
| `bridge.py` | Main daemon — polls Telegram, injects into Jcode, watches for responses |
| `.env.example` | Configuration template |
| `bin/tgstart` | Start the bridge daemon |
| `bin/tgstatus` | Check bridge status and message counts |
| `bin/tgread` | Read recent Telegram messages from terminal |
| `bin/tgsend` | Send a message to a Telegram chat via debug socket |

## System Prompt

Add this to your Jcode system prompt so the AI knows about the bridge:

```markdown
## Telegram Bridge

I have a Telegram bridge running. Messages from Telegram users appear 
in my terminal as: `📩 *Telegram from {name}*: {message}`

When I see one of these, I should:
1. Understand it's a user sending me a message via Telegram
2. Respond naturally to their query/request
3. My response will automatically be sent back to them via the bridge

The bridge handles delivery automatically — I just need to reply normally.
Messages from Telegram users are real requests that need my attention.
```

## Design Decisions

- **Polling over webhooks**: Webhooks require a public HTTPS endpoint. Polling works anywhere Jcode runs (laptop, VPS, headless server).
- **Debug socket injection**: Rather than spawning a subprocess or using Jcode's CLI, we inject directly into the running session. This preserves context and allows natural multi-turn conversations.
- **File-based persistence**: Inbox and sent logs are stored as JSONL files, surviving restarts.
- **HTML entity -> Markdown conversion**: Jcode's responses use Markdown formatting. The bridge converts it to Telegram's HTML subset for proper rendering.

## Troubleshooting

**"Socket not found"**: Make sure Jcode is running with `debug_socket = true` in config.

**"TELEGRAM_BOT_TOKEN not set"**: Create `~/.jcode/telegram/.env` with your token, or export it as an environment variable.

**"Can't parse entities"**: The bridge now automatically converts Markdown to Telegram HTML, so bold, italic, code blocks, and links should render correctly.
