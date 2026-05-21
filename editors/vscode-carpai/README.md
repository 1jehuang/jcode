# CarpAI VS Code Extension

AI-powered coding assistant with advanced code completion, collaboration, and debugging features.

## Features

### 🤖 AI Code Completion
- Real-time AI-powered code suggestions
- Context-aware completions based on your code
- Support for multiple programming languages

### 💬 Chat Interface
- Interactive chat with AI
- Get explanations for code
- Generate code from natural language

### 🔍 Code Review
- Automated code quality analysis
- Security vulnerability detection
- Best practices checking

### ✨ Code Understanding
- Explain selected code
- Generate documentation
- Summarize files

### 🔧 Code Generation
- Generate unit tests
- Refactor code
- Convert code between languages

## Requirements

- CarpAI server running locally
- Node.js 18+ for development

## Installation

1. Install the extension from the VS Code Marketplace
2. Start the CarpAI server:
   ```bash
   cargo run --release
   ```
3. Configure the server URL in settings (default: `http://localhost:8080`)

## Commands

| Command | Shortcut | Description |
|---------|----------|-------------|
| CarpAI: Complete Code | Ctrl+Shift+Space | Trigger AI code completion |
| CarpAI: Open Chat | Ctrl+Shift+C | Open chat panel |
| CarpAI: Explain Code | Ctrl+Shift+E | Explain selected code |
| CarpAI: Code Review | - | Run code review |
| CarpAI: Refactor Code | - | Refactor selected code |
| CarpAI: Generate Tests | - | Generate unit tests |
| CarpAI: Summarize Code | - | Summarize current file |
| CarpAI: Configure | - | Open settings |

## Configuration

```json
{
    "carpai.server.url": "http://localhost:8080",
    "carpai.completion.enabled": true,
    "carpai.completion.triggerCharacters": [".", "(", "[", "\"", "'"],
    "carpai.debug.enabled": true,
    "carpai.collaboration.enabled": true
}
```

## Development

```bash
# Install dependencies
npm install

# Compile TypeScript
npm run compile

# Watch for changes
npm run watch

# Run tests
npm run test
```

## License

MIT