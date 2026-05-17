#!/usr/bin/env python3
"""
jcode-telegram-bridge — Connect Telegram directly into your Jcode session.

Architecture:
  Telegram user → Bot API → Bridge injects into Jcode debug socket →
  Jcode responds naturally → Bridge polls last_response → Sends back to Telegram

No subprocess spawning. No separate AI provider config. Just straight into your session.
"""
import json, os, sys, time, socket, threading, urllib.request, urllib.error, re

sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

# ─── AUTO-LOAD .env if present in data dir or script dir ───
_script_dir = os.path.dirname(os.path.abspath(__file__))
for _env_dir in (_script_dir, os.path.expanduser("~/.jcode/telegram")):
    _env_path = os.path.join(_env_dir, ".env")
    if os.path.exists(_env_path):
        with open(_env_path) as _f:
            for _line in _f:
                _line = _line.strip()
                if not _line or _line.startswith("#"):
                    continue
                if "=" in _line:
                    _k, _v = _line.split("=", 1)
                    os.environ.setdefault(_k.strip(), _v.strip())

# ─── CONFIG (from environment) ───────────────────────────────
BOT_TOKEN = os.environ.get("TELEGRAM_BOT_TOKEN", "")
if not BOT_TOKEN:
    print("❌ TELEGRAM_BOT_TOKEN not set. Create a .env file or export it.", flush=True)
    sys.exit(1)

JCODE_SOCKET = os.environ.get(
    "JCODE_DEBUG_SOCKET",
    os.path.expanduser("~/.jcode/debug.sock")  # fallback path
)
# Auto-detect common socket locations if not set
if not os.environ.get("JCODE_DEBUG_SOCKET"):
    for candidate in [
        "/run/user/1000/jcode-debug.sock",
        os.path.expanduser("~/.jcode/debug.sock"),
        "/tmp/jcode-debug.sock",
    ]:
        if os.path.exists(candidate):
            JCODE_SOCKET = candidate
            break

DATA_DIR = os.environ.get("TELEGRAM_DATA_DIR", os.path.expanduser("~/.jcode/telegram"))
BASE_URL = f"https://api.telegram.org/bot{BOT_TOKEN}"
OFFSET_FILE = os.path.join(DATA_DIR, "offset.json")
INBOX_FILE = os.path.join(DATA_DIR, "inbox.jsonl")
SENT_FILE = os.path.join(DATA_DIR, "sent.jsonl")

POLL_INTERVAL = float(os.environ.get("RESPONSE_POLL_INTERVAL", "1.5"))

os.makedirs(DATA_DIR, exist_ok=True)

def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)

# ─── TELEGRAM API ─────────────────────────────────────────────

def tg_api(method, data=None, timeout=15):
    url = f"{BASE_URL}/{method}"
    body = json.dumps(data, ensure_ascii=False).encode('utf-8') if data else None
    headers = {"Content-Type": "application/json"} if data else {}
    try:
        req = urllib.request.Request(url, data=body, headers=headers)
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        try: return json.loads(e.read())
        except: return {"ok": False, "description": f"HTTP {e.code}"}
    except Exception as e:
        return {"ok": False, "description": str(e)}

def html_escape(text):
    """Escape HTML entities so Telegram's HTML parser doesn't choke."""
    return (
        text.replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
            .replace('"', "&quot;")
    )

def markdown_to_telegram_html(text):
    """Convert Markdown formatting to Telegram's supported HTML subset.
    
    Telegram HTML supports: <b>, <i>, <code>, <pre>, <a href="">
    We convert common markdown patterns to these tags.
    """
    # Must escape HTML entities first (before adding tags)
    text = html_escape(text)
    
    # Bold: **text** or __text__
    text = re.sub(r'\*\*(.+?)\*\*', r'<b>\1</b>', text)
    text = re.sub(r'(?<!/)__(.+?)__(?!/)', r'<b>\1</b>', text)
    
    # Italic: *text* or _text_ (but not inside words with underscores)
    text = re.sub(r'(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)', r'<i>\1</i>', text)
    # Single underscores for italic (word boundaries to avoid snake_case)
    text = re.sub(r'(?<!\w)_(?!_)(.+?)(?<!\w)_(?!_)', r'<i>\1</i>', text)
    
    # Inline code: `text`
    text = re.sub(r'`(.+?)`', r'<code>\1</code>', text)
    
    # Code block: ```text``` or ```language\ntext```
    text = re.sub(r'```(?:\w+)?\n?(.+?)```', r'<pre>\1</pre>', text, flags=re.DOTALL)
    
    # Links: [text](url)
    text = re.sub(r'\[(.+?)\]\((https?://[^\s)]+)\)', r'<a href="\2">\1</a>', text)
    
    # Headers: ### text -> <b>text</b>
    text = re.sub(r'^#{1,6}\s+(.+?)$', r'<b>\1</b>', text, flags=re.MULTILINE)
    
    # List items: - text or * text -> • text
    text = re.sub(r'^[\s]*[-*]\s+(.+?)$', r'• \1', text, flags=re.MULTILINE)
    
    # Numbered lists: 1. text -> just keep as-is
    text = re.sub(r'^(\s*\d+\.\s+.+?)$', r'\1', text, flags=re.MULTILINE)
    
    return text.strip()

def send_message(chat_id, text, parse_mode="HTML"):
    # Convert Markdown formatting to Telegram-compatible HTML
    safe_text = markdown_to_telegram_html(text)
    r = tg_api("sendMessage", {"chat_id": chat_id, "text": safe_text, "parse_mode": parse_mode})
    if r.get("ok"):
        with open(SENT_FILE, "a") as f:
            f.write(json.dumps({
                "chat_id": chat_id,
                "message_id": r["result"]["message_id"],
                "text": text[:80],
                "time": time.time()
            }) + "\n")
        log(f"✅ Sent to chat {chat_id}")
        return True
    else:
        # Fallback: try plain text if HTML fails
        log(f"⚠️ HTML failed ({r.get('description','')}), retrying as plain text...")
        r2 = tg_api("sendMessage", {"chat_id": chat_id, "text": text})
        if r2.get("ok"):
            with open(SENT_FILE, "a") as f:
                f.write(json.dumps({
                    "chat_id": chat_id,
                    "message_id": r2["result"]["message_id"],
                    "text": text[:80],
                    "time": time.time()
                }) + "\n")
            log(f"✅ Sent (plain text) to chat {chat_id}")
            return True
        log(f"❌ Send failed: {r2.get('description','')}")
        return False

def send_typing(chat_id):
    tg_api("sendChatAction", {"chat_id": chat_id, "action": "typing"})

def tg_poll(offset, timeout_sec=30):
    """Long-poll Telegram for updates."""
    return tg_api("getUpdates", {
        "offset": offset,
        "timeout": timeout_sec,
        "allowed_updates": ["message"]
    }, timeout=timeout_sec + 5)

# ─── JCODE DEBUG SOCKET ──────────────────────────────────────

def jcode_cmd(command, timeout_sec=5):
    """Send a debug command to the Jcode server via Unix socket."""
    if not os.path.exists(JCODE_SOCKET):
        return f'{{"ok": false, "output": "Socket not found: {JCODE_SOCKET}"}}'
    
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout_sec)
    try:
        sock.connect(JCODE_SOCKET)
        msg = json.dumps({
            "type": "debug_command",
            "id": int(time.time() * 1000),
            "command": command
        }) + "\n"
        sock.sendall(msg.encode())
        time.sleep(0.3)
        data = b""
        sock.settimeout(timeout_sec)
        try:
            while True:
                chunk = sock.recv(8192)
                if not chunk: break
                data += chunk
        except socket.timeout:
            pass
        return data.decode()
    except ConnectionRefusedError:
        return '{"ok": false, "output": "Connection refused"}'
    except Exception as e:
        return f'{{"ok": false, "output": "Error: {e}"}}'
    finally:
        sock.close()

def inject_into_jcode(user_name, text):
    """Inject a Telegram message into the Jcode session."""
    safe_text = text.replace("\n", " ").replace("\r", "")
    message = f"📩 *Telegram from {user_name}*: {safe_text}"
    result = jcode_cmd(f"client:message:{message}")
    log(f"📨 Injected: {safe_text[:60]}...")
    return result

def get_last_response():
    """Get the last assistant response from the Jcode session by parsing client history."""
    result = jcode_cmd("client:history", timeout_sec=3)
    try:
        data = json.loads(result)
        if not isinstance(data, dict):
            return None
        output = data.get("output", data)
        if isinstance(output, str):
            try:
                output = json.loads(output)
            except:
                pass
        if isinstance(output, list):
            # Walk backwards through display messages to find last assistant text
            for msg in reversed(output):
                role = msg.get("role", "")
                content = msg.get("content", "")
                if role == "assistant" and content and isinstance(content, str):
                    # Skip tool calls (JSON wrapped in {})
                    if content.strip().startswith("{"):
                        continue
                    return content
                # Also check nested content arrays
                if isinstance(content, list):
                    texts = [c.get("text", "") for c in content if c.get("type") == "text"]
                    text = " ".join(texts).strip()
                    if text:
                        return text
    except (json.JSONDecodeError, Exception):
        pass
    return None

# ─── RESPONSE WATCHER ────────────────────────────────────────

_active_chats = set()  # chat_ids waiting for responses
_response_lock = threading.Lock()
_last_seen_response = None

def response_watcher():
    """Background thread: watches for new Jcode responses and auto-sends to Telegram."""
    global _last_seen_response
    seen = set()
    
    log(f"📡 Watching for Jcode responses every {POLL_INTERVAL}s")
    
    while True:
        try:
            resp = get_last_response()
            if resp and resp not in seen:
                seen.add(resp)
                if len(seen) > 100:
                    seen = set(list(seen)[-50:])
                
                with _response_lock:
                    _last_seen_response = resp
                    
                    # Auto-send to all active Telegram chats
                    if _active_chats:
                        for cid in list(_active_chats):
                            send_message(cid, resp)
            
            time.sleep(POLL_INTERVAL)
        except Exception as e:
            log(f"⚠️ Watcher: {e}")
            time.sleep(3)

# ─── MESSAGE HANDLER ─────────────────────────────────────────

def handle_message(user_name, text, chat_id):
    """Inject message into Jcode and acknowledge immediately (no blocking)."""
    global _last_seen_response
    
    # Register this chat for auto-response delivery
    with _response_lock:
        _active_chats.add(chat_id)
        before = _last_seen_response
    
    # Inject into Jcode (no ack — responses auto-send via watcher)
    inject_into_jcode(user_name, text)
    
    log(f"✅ Registered {chat_id} for auto-delivery")

# ─── OFFSET PERSISTENCE ──────────────────────────────────────

def load_offset():
    try:
        with open(OFFSET_FILE) as f:
            return json.load(f).get("offset", 0)
    except:
        return 0

def save_offset(offset):
    with open(OFFSET_FILE, "w") as f:
        json.dump({"offset": offset}, f)

# ─── MAIN ─────────────────────────────────────────────────────

def main():
    log(f"╔{'═'*50}╗")
    log(f"║  Jcode Telegram Bridge v1                                       ║")
    log(f"║  Bot: @{BOT_TOKEN.split(':')[0]}...")
    log(f"║  Socket: {JCODE_SOCKET}")
    log(f"║  Data: {DATA_DIR}")
    log(f"╚{'═'*50}╝")

    # Verify socket
    if not os.path.exists(JCODE_SOCKET):
        log(f"⚠️  Jcode socket not found at {JCODE_SOCKET}")
        log(f"⚠️  Make sure Jcode is running with debug_socket enabled")
        log(f"⚠️  See: https://github.com/YOUR_USER/jcode-telegram-bridge#troubleshooting")
    
    # Start watcher
    rw = threading.Thread(target=response_watcher, daemon=True)
    rw.start()

    offset = load_offset()
    log(f"📡 Polling Telegram from offset {offset}...")

    while True:
        try:
            r = tg_poll(offset)
            
            if not r.get("ok"):
                log(f"⚠️ Poll: {r.get('description','error')}")
                time.sleep(3)
                continue
            
            for update in r.get("result", []):
                new_offset = update["update_id"] + 1
                if new_offset > offset:
                    offset = new_offset

                msg = update.get("message")
                if not msg:
                    continue
                if msg.get("from", {}).get("is_bot"):
                    continue

                chat_id = msg.get("chat", {}).get("id")
                user = msg.get("from", {})
                text = msg.get("text", "") or msg.get("caption", "") or ""
                name = (user.get("first_name", "") or 
                        user.get("username", "") or 
                        str(user.get("id", "")))

                # Log to inbox
                entry = {
                    "timestamp": time.time(),
                    "chat_id": chat_id,
                    "user_id": user.get("id"),
                    "username": user.get("username", ""),
                    "first_name": user.get("first_name", ""),
                    "text": text,
                }
                with open(INBOX_FILE, "a") as f:
                    f.write(json.dumps(entry) + "\n")
                
                log(f"📩 {name}: {text[:120]}")
                
                # Process in background thread
                threading.Thread(
                    target=handle_message,
                    args=(name, text, chat_id),
                    daemon=True
                ).start()

            if r.get("result"):
                save_offset(offset)

        except KeyboardInterrupt:
            log("Bridge stopping.")
            break
        except Exception as e:
            log(f"⚠️ Error: {e}")
            time.sleep(3)

if __name__ == "__main__":
    main()
