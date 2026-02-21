# jcode iOS Client

> **Status:** Design
> **Updated:** 2025-02-21

A native iOS application that connects to a jcode server running on the user's laptop or desktop. The phone is a rich, touch-optimized client; all heavy lifting (LLM calls, tool execution, file I/O, git, MCP) stays on the server.

---

## Architecture

```
┌──────────────────────────────────────┐         ┌──────────────────────────────────────┐
│          iPhone (iOS App)            │         │      Laptop/Desktop (Server)         │
│                                      │   WS    │                                      │
│  ┌────────────────────────────────┐  │  over   │  ┌────────────────────────────────┐  │
│  │  SwiftUI Interface             │  │  TLS    │  │  jcode server (Rust)           │  │
│  │  - Conversation view           │◄─┼────────►│  │  - Agent + LLM providers       │  │
│  │  - Tool approval sheet         │  │         │  │  - Tool execution (bash, etc)  │  │
│  │  - Ambient mode dashboard      │  │         │  │  - Memory graph                │  │
│  │  - Server manager              │  │         │  │  - MCP servers                 │  │
│  └────────────────────────────────┘  │         │  │  - Ambient scheduler           │  │
│                                      │         │  └────────────────────────────────┘  │
│  ┌────────────────────────────────┐  │         │                                      │
│  │  Local Services                │  │         │  ┌────────────────────────────────┐  │
│  │  - Push notification handler   │  │         │  │  WebSocket Gateway (new)       │  │
│  │  - Server discovery (Bonjour)  │  │         │  │  - Listens on TCP port         │  │
│  │  - Keychain (auth tokens)      │  │         │  │  - TLS termination             │  │
│  │  - Offline message queue       │  │         │  │  - Token authentication        │  │
│  └────────────────────────────────┘  │         │  │  - Bridges to Unix socket      │  │
│                                      │         │  └────────────────────────────────┘  │
└──────────────────────────────────────┘         └──────────────────────────────────────┘
```

---

## Why This Architecture

jcode's value is **tool execution**: running shell commands, editing files, managing git repos, connecting to MCP servers. None of that is possible inside iOS's sandbox. So the server must exist regardless.

What the phone adds:
- **Mobility** - interact with jcode from the couch, on the bus, in a meeting
- **Ambient display** - phone on desk showing agent progress, task status, memory activity
- **Push notifications** - know when a task finishes, approve tool calls from lock screen
- **Touch UX** - purpose-built interface instead of terminal emulation

What the phone does NOT do:
- Run bash commands
- Access the filesystem
- Host MCP servers
- Run LLM inference locally

---

## Server-Side Changes

The jcode server currently speaks newline-delimited JSON over Unix sockets. The iOS client needs the same protocol over a network transport. Changes required:

### 1. WebSocket Gateway

A new network listener alongside the existing Unix socket. Same protocol, different transport.

```
                  ┌─────────────────────────┐
                  │      jcode server        │
                  │                          │
   Unix socket ──►│  session manager         │◄── WebSocket (new)
   (TUI client)   │  agent engine            │    (iOS client)
                  │  tool registry           │
   Debug socket ─►│  swarm coordinator       │
                  └─────────────────────────┘
```

**Location in code:** New module `src/gateway.rs` (or extend `src/server.rs`)

**Key decisions:**
- Listen on a configurable TCP port (default: `7643` - "jc" on phone keypad)
- TLS required for non-localhost connections (self-signed cert or Let's Encrypt)
- WebSocket upgrade on `/ws` endpoint
- REST endpoints for discovery and health: `GET /health`, `GET /info`
- Same `Request`/`ServerEvent` JSON protocol as Unix socket

**Minimal diff to protocol:**
- No protocol changes needed. The existing `Request` and `ServerEvent` enums work over WebSocket as-is.
- Add a `Subscribe` variant field for client type (`tui` vs `ios`) so the server can tailor events (e.g., send push-worthy notifications differently).

### 2. Authentication

Unix sockets are authenticated by filesystem permissions. Network sockets need explicit auth.

```
Pairing Flow:
                                                         
  1. User runs: jcode pair                               
     → Server generates a 6-digit pairing code           
     → Displays it in terminal                           
     → Code valid for 5 minutes                          
                                                         
  2. User enters code in iOS app                         
     → App sends code + device ID to server              
     → Server validates, returns a long-lived auth token  
     → Token stored in iOS Keychain                      
                                                         
  3. All subsequent connections use Bearer token          
     → Token included in WebSocket upgrade request       
     → Server validates against stored device list        

  Config: ~/.jcode/devices.json
  [
    {
      "id": "iphone-14-jeremy",
      "name": "Jeremy's iPhone",
      "token_hash": "sha256:...",
      "paired_at": "2025-02-21T...",
      "last_seen": "2025-02-21T..."
    }
  ]
```

### 3. Server Discovery

The iOS app needs to find the jcode server on the local network.

**Option A: Bonjour/mDNS (recommended for LAN)**
- Server advertises `_jcode._tcp` service via mDNS
- iOS discovers it automatically (NSNetServiceBrowser)
- Works on any LAN without configuration
- Rust crate: `mdns-sd` or `zeroconf`

**Option B: Tailscale/WireGuard (for remote access)**
- User's phone and laptop on the same Tailscale network
- Server binds to Tailscale IP
- Manual configuration in the iOS app (enter Tailscale hostname)
- Works from anywhere, not just LAN

**Option C: Manual IP/hostname**
- User enters `hostname:port` directly
- Fallback when mDNS doesn't work

The app should support all three, trying Bonjour first.

### 4. Push Notifications

jcode already has a notification system (`src/notifications.rs`) with ntfy.sh, desktop notifications, and email. iOS push adds another channel.

**Approach: ntfy.sh (simplest, no Apple infrastructure needed)**

ntfy.sh already supports iOS push notifications via their app. jcode already has ntfy.sh support. The iOS app just needs to subscribe to the same ntfy topic.

```
Already exists in jcode:          New for iOS:
┌──────────────────────┐          ┌──────────────────────────┐
│ notifications.rs     │          │ iOS App                  │
│                      │          │                          │
│ ntfy.sh ────────────►│──push──► │ ntfy.sh iOS SDK          │
│ desktop notify       │          │ → native push alerts     │
│ email                │          │ → lock screen widgets    │
└──────────────────────┘          └──────────────────────────┘
```

**Events worth pushing:**
- Task/message completed (agent finished a turn)
- Tool approval requested (safety system Tier 2 action)
- Ambient cycle completed (with summary)
- Server going offline / coming back online
- Swarm task assigned to you

**Future: Apple Push Notification Service (APNs)**
- Required for App Store distribution
- Needs a relay server (or direct APNs from jcode server)
- More complex but more reliable than ntfy.sh
- Can power Live Activities, Dynamic Island, and widgets

### 5. Image/File Transfer

The iOS client needs to send images (screenshots, photos) and receive file previews.

```
iOS → Server:
  - Images attached to messages (already supported: Request::Message has images field)
  - Base64-encoded in the JSON payload (existing pattern)
  - Consider chunked upload for large files

Server → iOS:
  - Code snippets with syntax highlighting (rendered client-side)
  - File tree snapshots (for browsing)
  - Image tool outputs (screenshots, diagrams)
```

---

## iOS App Design

### Screen Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Server      │     │  Session     │     │  Ambient     │
│  Discovery   │────►│  List        │────►│  Dashboard   │
│              │     │              │     │              │
│  - Scanning  │     │  - Active    │     │  - Status    │
│  - Manual    │     │  - Resume    │     │  - History   │
│  - Pair new  │     │  - New       │     │  - Schedule  │
└─────────────┘     └──────┬───────┘     └─────────────┘
                           │
                           ▼
                    ┌─────────────┐
                    │  Chat View   │
                    │              │
                    │  - Messages  │
                    │  - Tools     │
                    │  - Status    │
                    └─────────────┘
```

### Chat View (Primary)

Redesigned for touch. NOT a terminal emulator.

```
┌──────────────────────────────────────┐
│ ◄  🦊 fox on 🔥 blazing     ⚙️  ⋮  │  ← Navigation bar
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐   │
│  │ 👤 Can you refactor the auth │   │  ← User message (bubble)
│  │    module to use OAuth2?     │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌──────────────────────────────┐   │
│  │ 🤖 I'll refactor the auth   │   │  ← Assistant message
│  │    module. Let me start by   │   │
│  │    reading the current code. │   │
│  │                              │   │
│  │  ┌────────────────────────┐  │   │
│  │  │ 📄 file_read           │  │   │  ← Tool call (collapsible card)
│  │  │ src/auth.rs            │  │   │
│  │  │ ✅ 245 lines           │  │   │
│  │  └────────────────────────┘  │   │
│  │                              │   │
│  │  ┌────────────────────────┐  │   │
│  │  │ ✏️ file_edit            │  │   │  ← Another tool call
│  │  │ src/auth.rs            │  │   │
│  │  │ ⏳ running...           │  │   │
│  │  │ [View Diff]            │  │   │
│  │  └────────────────────────┘  │   │
│  │                              │   │
│  └──────────────────────────────┘   │
│                                      │
├──────────────────────────────────────┤
│ ┌──────────────────────────┐  📎 🎤 │  ← Input bar
│ │ Message jcode...         │  ⬆️    │
│ └──────────────────────────┘        │
└──────────────────────────────────────┘
```

**Key UX elements:**
- Tool calls as collapsible cards (tap to expand output)
- Diff viewer for file edits (swipe to see before/after)
- Syntax-highlighted code blocks
- Image attachments via camera/photo picker (📎)
- Voice input (🎤) for hands-free
- Swipe right on a message to reply/interrupt
- Pull down to see token usage, model info

### Ambient Dashboard

The killer feature for iOS. Shows what jcode is doing autonomously.

```
┌──────────────────────────────────────┐
│          Ambient Mode                │
├──────────────────────────────────────┤
│                                      │
│  Status: 🟢 Scheduled               │
│  Next wake: 12 minutes              │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  Last Cycle (35 min ago)      │   │
│  │                               │   │
│  │  ✅ Merged 3 duplicate        │   │
│  │     memories                  │   │
│  │  ✅ Pruned 2 stale facts      │   │
│  │  ✅ Extracted memories from    │   │
│  │     crashed session           │   │
│  │  📝 0 compactions             │   │
│  │                               │   │
│  │  [View Full Transcript]       │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  Scheduled Queue (2 items)    │   │
│  │                               │   │
│  │  ⏰ Check CI for auth PR      │   │
│  │     in 12 min (normal)        │   │
│  │  ⏰ Review stale TODO items   │   │
│  │     in 45 min (low)           │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  Memory Health                │   │
│  │  ████████████░░ 847 memories  │   │
│  │  12 new today, 3 pruned       │   │
│  └──────────────────────────────┘   │
│                                      │
│  [ Pause Ambient ] [ Run Now ]       │
│                                      │
└──────────────────────────────────────┘
```

### Tool Approval (Push Notification)

When the safety system requires approval for a Tier 2 action:

```
┌──────────────────────────────────────┐
│  🔔 jcode needs approval            │
│                                      │
│  🦊 fox wants to run:               │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  rm -rf target/              │   │
│  │                              │   │
│  │  Reason: Clean build after   │   │
│  │  dependency update           │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌─────────┐     ┌──────────────┐   │
│  │  Deny   │     │   Approve    │   │
│  └─────────┘     └──────────────┘   │
│                                      │
│  [ Always allow for this session ]   │
│                                      │
└──────────────────────────────────────┘
```

This should also work as an actionable push notification on the lock screen.

### Server Manager

```
┌──────────────────────────────────────┐
│          Servers                      │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐   │
│  │  🔥 blazing                   │   │
│  │  v0.3.3 (abc1234)            │   │
│  │  192.168.1.42:7643           │   │
│  │  🟢 Online  ·  2 sessions    │   │
│  │                               │   │
│  │  🦊 fox  · 5 min ago         │   │
│  │  🦉 owl  · 2 hours ago       │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  ❄️ frozen                    │   │
│  │  v0.3.2 (def5678)            │   │
│  │  🔴 Offline                   │   │
│  │                               │   │
│  │  [ Wake Server ]              │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  + Add Server                 │   │
│  │  Scan LAN · Manual · Pair    │   │
│  └──────────────────────────────┘   │
│                                      │
└──────────────────────────────────────┘
```

---

## Protocol Extensions

The existing `Request`/`ServerEvent` protocol works as-is over WebSocket. A few additions:

### New Request Types

```rust
// Client identifies itself on connect
#[serde(rename = "identify")]
Identify {
    id: u64,
    client_type: String,      // "ios", "tui", "web"
    device_name: String,       // "Jeremy's iPhone"
    device_id: String,         // Stable device identifier
    app_version: String,       // "1.0.0"
    capabilities: Vec<String>, // ["push", "images", "voice"]
}

// Request server info without subscribing to a session
#[serde(rename = "server_info")]
ServerInfo { id: u64 }

// Approve/deny a safety permission request
#[serde(rename = "permission_response")]
PermissionResponse {
    id: u64,
    request_id: String,
    approved: bool,
    remember: bool,  // "always allow for this session"
}
```

### New ServerEvent Types

```rust
// Permission request (push-worthy)
#[serde(rename = "permission_request")]
PermissionRequest {
    request_id: String,
    session_id: String,
    action: String,         // "shell_exec", "file_write", etc.
    detail: String,         // "rm -rf target/"
    tier: u8,               // Safety tier (1, 2, 3)
}

// Server info response
#[serde(rename = "server_info")]
ServerInfoResponse {
    id: u64,
    server_name: String,
    server_icon: String,
    version: String,
    sessions: Vec<SessionSummary>,
    ambient_status: String,
    uptime_secs: u64,
}

// Ambient cycle completed (push-worthy)
#[serde(rename = "ambient_cycle_done")]
AmbientCycleDone {
    summary: String,
    memories_modified: usize,
    next_wake_minutes: Option<u64>,
}
```

---

## Development Plan

### Phase 0: WebSocket Gateway (Rust, on Linux)

No Mac needed. Build and test entirely on Linux.

1. Add WebSocket listener to jcode server (`src/gateway.rs`)
   - Depends on: `tokio-tungstenite` (already in Cargo.toml)
   - Listen on configurable TCP port
   - Bridge WebSocket frames to existing Unix socket protocol
2. Add token-based authentication
   - Pairing command: `jcode pair`
   - Device registry: `~/.jcode/devices.json`
3. Add mDNS/Bonjour advertisement
   - Crate: `mdns-sd`
   - Advertise `_jcode._tcp` with server name, version
4. Test with `websocat` or a simple Python script

**Deliverable:** Any WebSocket client can connect to jcode server, authenticate, and interact with sessions. Testable from Linux with CLI tools.

### Phase 1: Minimal iOS Client (needs Mac)

Borrow the MacBook for initial setup, then iterate.

1. Xcode project setup
   - SwiftUI app targeting iOS 17+
   - Bonjour discovery (NSNetServiceBrowser)
   - WebSocket connection (URLSessionWebSocketTask)
2. Pairing flow
   - Enter 6-digit code
   - Store token in Keychain
3. Basic chat view
   - Send messages, display responses
   - Show streaming text deltas
   - Display tool calls as cards
4. Session management
   - List sessions, create new, resume existing

**Deliverable:** Working iOS app that can chat with jcode.

### Phase 2: Rich UX

5. Tool call cards with expandable output
6. Diff viewer for file edits
7. Syntax highlighting (use a Swift library, e.g., Splash or Highlightr)
8. Image attachments (camera + photo library)
9. Voice input (iOS Speech framework)
10. Haptic feedback for events

### Phase 3: Ambient Mode + Notifications

11. ntfy.sh push notification integration
12. Ambient dashboard (status, history, schedule, memory health)
13. Tool approval via push notification (actionable)
14. iOS widgets (WidgetKit) for ambient status on home screen
15. Live Activities for long-running tasks

### Phase 4: Polish + Distribution

16. Dark/light theme (respect system setting)
17. iPad layout (split view, sidebar)
18. Offline mode (queue messages, sync when reconnected)
19. TestFlight beta distribution
20. App Store submission (requires APNs migration from ntfy.sh)

---

## What You Need From the MacBook

**One-time setup (2-3 hours):**
- Install Xcode (free, ~20 GB download)
- Apple Developer account ($99/year for App Store, free for personal sideloading)
- Create Xcode project, configure signing
- Connect iPhone via USB, enable Developer Mode

**Ongoing development:**
- Write Swift code anywhere (even on Linux in a text editor)
- Use the Mac only for: building, signing, deploying to phone
- Could also use GitHub Actions macOS runners for CI builds
- Xcode Cloud (free tier: 25 compute hours/month) for automated builds

**Sideloading limitation (free account):**
- Apps expire every 7 days, need to re-deploy
- Limited to 3 apps per device
- No TestFlight distribution
- Worth it for prototyping; get the paid account when ready to share

---

## Tech Stack

| Component | Technology | Notes |
|-----------|-----------|-------|
| **iOS UI** | SwiftUI | Modern, declarative, good for our UX |
| **Networking** | URLSessionWebSocketTask | Native iOS WebSocket, no dependencies |
| **Discovery** | NWBrowser (Network.framework) | Modern replacement for NSNetServiceBrowser |
| **Auth tokens** | Keychain Services | Secure, persists across app installs |
| **Push notifications** | ntfy.sh (Phase 3), APNs (Phase 4) | ntfy.sh is zero-infrastructure |
| **Syntax highlighting** | Splash or Highlightr | Swift libraries for code rendering |
| **Widgets** | WidgetKit | Home screen ambient dashboard |
| **Live Activities** | ActivityKit | Lock screen task progress |
| **Server WebSocket** | tokio-tungstenite | Already a dependency |
| **Server mDNS** | mdns-sd | Lightweight, async |
| **Server TLS** | rustls | Already used via reqwest |

---

## Security Considerations

- **All network connections must use TLS** (except localhost development)
- **Auth tokens** stored in iOS Keychain, server stores only hashes
- **Pairing codes** are time-limited (5 min) and single-use
- **Device revocation** via `jcode devices revoke <name>`
- **No credentials on the phone** - API keys, OAuth tokens stay on the server
- **Tool approval** for destructive actions even when triggered from iOS
- **Rate limiting** on the WebSocket gateway to prevent abuse

---

## Open Questions

1. **Wake-on-LAN** - Can the iOS app wake a sleeping desktop? Would need WoL support in the server manager.
2. **SSH tunnel fallback** - If the server is behind NAT without Tailscale, should the app support SSH tunneling?
3. **Multiple servers** - The server manager UI supports this, but how to handle sessions spanning servers?
4. **Offline mode** - How much should the app cache? Full conversation history? Just recent messages?
5. **iPad as primary** - Should iPad support be a first-class goal or a stretch? Split view with code preview could be powerful.
6. **Keyboard shortcuts** - iPad with keyboard should feel native (Cmd+Enter to send, etc.)
