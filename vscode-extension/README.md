# CarpAI VSCode Extension

AI coding assistant with multi-agent collaboration, intelligent memory, and blazing fast performance.

## Features

- **Chat Panel**: Sidebar chat interface for AI conversations
- **Inline Chat**: Press `Ctrl+K` (Cmd+K on Mac) to chat about selected code
- **Quick Actions**: Right-click to explain or refactor code
- **Apply Edits**: One-click apply of AI-generated code changes
- **Streaming Responses**: Real-time token-by-token display

## Installation

1. Install CarpAI Server: https://github.com/1jehuang/jcode
2. Start the server: `jcode serve`
3. Install this extension from VSCode Marketplace (coming soon)
4. Configure server URL in Settings → CarpAI

## Usage

### Chat Panel
1. Click CarpAI icon in activity bar
2. Type your question in the chat input
3. View responses with syntax highlighting

### Inline Chat
1. Select code in editor
2. Press `Ctrl+K` (Cmd+K on Mac)
3. Ask questions about the selected code

### Quick Actions
- Right-click code → "CarpAI: Explain This Code"
- Right-click code → "CarpAI: Refactor This Code"

### Apply Edits
When AI returns code blocks, click "Apply to Editor" to preview and apply changes.

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `carpai.serverUrl` | `http://localhost:8080` | CarpAI Server URL |
| `carpai.apiKey` | `""` | API Key (optional) |
| `carpai.enableCache` | `true` | Enable response caching |
| `carpai.model` | `gpt-4` | Default model |

## Development

```bash
npm install
npm run compile
```

Press F5 in VSCode to debug the extension.

## License

MIT OR Apache-2.0
