# CarpAI JetBrains Plugin

AI-powered coding assistant for JetBrains IDEs with intelligent code completion, chat, and real-time collaboration.

<!-- Plugin description -->
CarpAI is an AI-powered coding assistant that brings intelligent code completion, chat, and real-time collaboration to JetBrains IDEs.

## Features

- **AI Chat**: Natural language conversations about your code
- **Intelligent Code Completion**: Context-aware suggestions powered by multiple LLMs
- **Real-time Collaboration**: Share sessions and collaborate with teammates using CRDT
- **Code Analysis**: Deep code understanding with AST parsing and call graph analysis
- **Multi-model Support**: Route requests to optimal AI models (OpenAI, Gemini, Qwen, etc.)
- **Private Deployment**: Connect to your organization's self-hosted CarpAI server

## Getting Started

1. Install the plugin
2. Open the CarpAI tool window (View → Tool Windows → CarpAI)
3. Configure your CarpAI server URL in Settings
4. Start chatting with AI about your code!

## Enterprise Features

- SSO authentication (OIDC/SAML/LDAP)
- Team session sharing
- Audit logging
- Custom model routing policies
<!-- Plugin description end -->

## Development Setup

### Prerequisites

- JDK 17 or later
- IntelliJ IDEA 2023.3 or later (for development)
- Gradle 8.5+

### Building the Plugin

```bash
# Build the plugin
./gradlew buildPlugin

# Run in development mode
./gradlew runIde

# Publish to JetBrains Marketplace
./gradlew publishPlugin
```

### Project Structure

```
editors/jetbrains-carpai/
├── src/main/kotlin/com/carpai/plugin/
│   ├── CarpaiPlugin.kt              # Plugin entry point
│   ├── CarpaiService.kt             # Main service manager
│   ├── actions/                     # IDE actions
│   │   ├── OpenChatAction.kt
│   │   ├── ExplainCodeAction.kt
│   │   └── RefactorCodeAction.kt
│   ├── collab/                      # Collaboration features
│   │   └── CollaborationService.kt
│   ├── lsp/                         # LSP client
│   │   └── CarpaiLspClient.kt
│   ├── listeners/                   # Event listeners
│   │   └── CarpaiProjectManagerListener.kt
│   ├── settings/                    # Configuration
│   │   ├── CarpaiSettings.kt
│   │   └── CarpaiSettingsConfigurable.kt
│   └── ui/                          # UI components
│       ├── CarpaiToolWindowFactory.kt
│       ├── CarpaiChatPanel.kt
│       ├── ChatMessageRenderer.kt
│       └── CarpaiStatusBarWidget.kt
├── src/main/resources/
│   ├── META-INF/
│   │   └── plugin.xml               # Plugin configuration
│   └── icons/
│       └── carpai_13x13.svg
├── build.gradle.kts                 # Build configuration
└── gradle.properties                # Gradle properties
```

## Architecture

### Core Components

1. **CarpaiService**: Central service managing LSP client and collaboration
2. **CarpaiLspClient**: Communicates with CarpAI server via LSP protocol
3. **CollaborationService**: Real-time sync using Yrs CRDT
4. **CarpaiChatPanel**: Main UI for AI conversations

### Communication Flow

```
JetBrains IDE ←→ LSP Client ←→ CarpAI Server ←→ AI Models
     ↑                                   
     └── WebSocket (Collaboration)
```

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests: `./gradlew test`
5. Submit a pull request

## License

MIT License - see LICENSE file for details

## Support

- Documentation: https://docs.carpai.example.com
- Issue Tracker: https://github.com/codecargo/CarpAI/issues
- Email: support@carpai.example.com
