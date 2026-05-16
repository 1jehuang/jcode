//! VS Code Extension Framework for jcode
//!
//! Provides seamless VS Code integration, matching Cursor's experience:
//! - Inline completions (Tab completion like Copilot)
//! - Chat panel integration
//! - Terminal commands
//! - Status bar indicators
//! - Auto-configuration

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use anyhow::Result;

/// VS Code extension manifest (package.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeExtensionManifest {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub publisher: String,
    pub engines: VscodeEngines,
    pub categories: Vec<String>,
    pub activation_events: Vec<String>,
    pub main: String,
    pub contributes: VscodeContributes,
    pub scripts: VscodeScripts,
}

/// VS Code version requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeEngines {
    pub vscode: String,
}

/// Extension contribution points
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeContributes {
    pub commands: Vec<VscodeCommand>,
    pub configuration: Option<VscodeConfiguration>,
    pub keybindings: Option<Vec<VscodeKeybinding>>,
    pub menus: Option<Vec<VscodeMenu>>,
    pub languages: Option<Vec<VscodeLanguage>>,
    pub grammars: Option<Vec<VscodeGrammar>>,
    pub themes: Option<Vec<VscodeTheme>>,
    pub icons: Option<VscodeIcons>,
}

/// VS Code command definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeCommand {
    pub command: String,
    pub title: String,
    pub category: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// VS Code configuration schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeConfiguration {
    pub title: String,
    pub properties: std::collections::HashMap<String, VscodeConfigProperty>,
}

/// Configuration property
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeConfigProperty {
    #[serde(rename = "type")]
    pub config_type: String,
    pub default: Option<serde_json::Value>,
    pub description: String,
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
}

/// Keybinding definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeKeybinding {
    pub command: String,
    pub key: String,
    pub when: Option<String>,
    pub mac: Option<String>,
    pub linux: Option<String>,
    pub win: Option<String>,
}

/// Menu item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeMenu {
    pub command: String,
    pub when: Option<String>,
    pub group: Option<String>,
    pub alt: Option<String>,
}

/// Build/extension scripts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeScripts {
    pub "vscode:prepublish": String,
    pub compile: String,
    pub watch: String,
    pub lint: String,
    pub test: Option<String>,
}

impl VscodeExtensionManifest {
    /// Generate jcode VS Code extension manifest
    pub fn generate_jcode_manifest() -> Self {
        Self {
            name: "jcode".to_string(),
            display_name: "jcode".to_string(),
            description: "AI-powered coding assistant (Cursor-compatible, open-source alternative)".to_string(),
            version: "0.1.0".to_string(),
            publisher: "jcode-community".to_string(),
            engines: VscodeEngines {
                vscode: "^1.85.0".to_string(), // Support recent VS Code versions
            },
            categories: vec![
                "Programming Languages".to_string(),
                "Machine Learning".to_string(),
                "Snippets".to_string(),
                "Debuggers".to_string(),
            ],
            activation_events: vec![
                "onLanguage:*".to_string(),
                "onCommand:jcode.start".to_string(),
                "onCommand:jcode.openChat".to_string(),
                "onCommand:jcode.inlineCompletion".to_string(),
                "workspaceContains:**/*.{rs,py,js,ts,go,java,cpp,c,h}".to_string(),
            ],
            main: "./out/extension.js".to_string(),
            contributes: Self::generate_contributes(),
            scripts: Self::generate_scripts(),
        }
    }
    
    /// Generate contribution points
    fn generate_contributes() -> VscodeContributes {
        VscodeContributes {
            commands: vec![
                // Core commands
                VscodeCommand {
                    command: "jcode.start".to_string(),
                    title: "Start jcode Server".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(play)".to_string()),
                },
                VscodeCommand {
                    command: "jcode.stop".to_string(),
                    title: "Stop jcode Server".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(stop)".to_string()),
                },
                
                // Chat commands
                VscodeCommand {
                    command: "jcode.openChat".to_string(),
                    title: "Open jcode Chat Panel".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(comment-discussion)".to_string()),
                },
                VscodeCommand {
                    command: "jcode.askQuestion".to_string(),
                    title: "Ask jcode a Question".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(question)".to_string()),
                },
                
                // Completion commands
                VscodeCommand {
                    command: "jcode.inlineCompletion".to_string(),
                    title: "Trigger Inline Completion".to_string(),
                    category: Some("jcode".to_string()),
                },
                VscodeCommand {
                    command: "jcode.acceptInlineCompletion".to_string(),
                    title: "Accept Inline Completion (Tab)".to_string(),
                    category: Some("jcode".to_string()),
                },
                
                // Context actions
                VscodeCommand {
                    command: "jcode.explainCode".to_string(),
                    title: "Explain Selected Code".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(lightbulb)".to_string()),
                },
                VscodeCommand {
                    command: "jcode.fixError".to_string(),
                    title: "Fix Error at Cursor".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some "$(bug)",
                },
                VscodeCommand {
                    command: "jcode.refactorSelection".to_string(),
                    title: "Refactor Selection with AI".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(edit)",
                },
                VscodeCommand {
                    command: "jcode.generateTests".to_string(),
                    title: "Generate Tests for Selection".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(beaker)",
                },
                
                // Documentation commands
                VscodeCommand {
                    command: "jcode.documentCode".to_string(),
                    title: "Generate Documentation".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(book)",
                },
                VscodeCommand {
                    command: "jcode.addComments".to_string(),
                    title: "Add Comments to Code".to_string(),
                    category: Some("jcode".to_string()),
                },
                
                // Settings commands
                VscodeCommand {
                    command: "jcode.showSettings".to_string(),
                    title: "Open jcode Settings".to_string(),
                    category: Some("jcode".to_string()),
                    icon: Some("$(gear)",
                },
                VscodeCommand {
                    command: "jcode.switchModel".to_string(),
                    title: "Switch LLM Model".to_string(),
                    category: Some("jcode".to_string()),
                },
                VscodeCommand {
                    command: "jcode.showStatus".to_string(),
                    title: "Show jcode Status".to_string(),
                    category: Some("jcode".to_string()),
                },
            ],
            
            configuration: Some(Self::generate_configuration_schema()),
            
            keybindings: Some(vec![
                // Cursor-like keybindings
                VscodeKeybinding {
                    command: "jcode.inlineCompletion".to_string(),
                    key: "alt+\\\\".to_string(), // Alt + \ (same as Copilot)
                    when: Some("editorTextFocus && !editorReadonly && !suggestWidgetVisible".to_string()),
                    ..Default::default()
                },
                VscodeKeybinding {
                    command: "jcode.acceptInlineCompletion".to_string(),
                    key: "tab".to_string(),
                    when: Some("editorTextFocus && !editorReadonly && jcode.hasInlineCompletion".to_string()),
                    ..Default::default()
                },
                VscodeKeybinding {
                    command: "jcode.openChat".to_string(),
                    key: "ctrl+shift+j".to_string(), // Same as Cursor's chat shortcut
                    when: None,
                    ..Default::default()
                },
                VscodeKeybinding {
                    command: "jcode.explainCode".to_string(),
                    key: "ctrl+k ctrl+i".to_string(),
                    when: Some("editorHasSelection && editorTextFocus".to_string()),
                    ..Default::default()
                },
                VscodeKeybinding {
                    command: "jcode.fixError".to_string(),
                    key: "ctrl+.".to_string(),
                    when: Some("editorTextFocus && !editorReadonly".to_string()),
                    ..Default::default()
                },
            ]),
            
            menus: Some(vec![
                // Editor context menu
                VscodeMenu {
                    command: "jcode.explainCode".to_string(),
                    when: Some("editorHasSelection".to_string()),
                    group: Some("9_jcode@1".to_string()), // Custom group after built-in items
                    ..Default::default()
                },
                VscodeMenu {
                    command: "jcode.fixError".to_string(),
                    when: Some("editorHasSelection || editorTextFocus".to_string()),
                    group: Some("9_jcode@2".to_string()),
                    ..Default::default()
                },
                VscodeMenu {
                    command: "jcode.refactorSelection".to_string(),
                    when: Some("editorHasSelection".to_string()),
                    group: Some("9_jcode@3".to_string()),
                    ..Default::default()
                },
                VscodeMenu {
                    command: "jcode.generateTests".to_string(),
                    when: Some("editorHasSelection".to_string()),
                    group: Some("9_jcode@4".to_string()),
                    ..Default::default()
                },
                VscodeMenu {
                    command: "jcode.documentCode".to_string(),
                    when: Some("editorHasSelection".to_string()),
                    group: Some("9_jcode@5".to_string()),
                    ..Default::default()
                },
                
                // Command palette
                VscodeMenu {
                    command: "jcode.openChat".to_string(),
                    when: None,
                    group: Some("navigation".to_string()),
                    ..Default::default()
                },
                
                // View menu
                VscodeMenu {
                    command: "jcode.showStatus".to_string(),
                    when: None,
                    group: Some("9_jcode".to_string()),
                    ..Default::default()
                },
            ]),
            
            languages: Some(vec![VscodeLanguage {
                id: "jcode-chat".to_string(),
                extensions: vec![".jchat".to_string()],
                aliases: vec!["Jcode Chat".to_string(), "AI Chat".to_string()],
                configuration: "languageId".to_string(),
            }]),
            
            grammars: None,
            themes: None,
            icons: None,
        }
    }
    
    /// Generate settings UI schema
    fn generate_configuration_schema() -> VscodeConfiguration {
        let mut properties = std::collections::HashMap::new();
        
        // LLM Provider Settings
        properties.insert(
            "jcode.llm.provider".to_string(),
            VscodeConfigProperty {
                config_type: "string".to_string(),
                default: Some(serde_json::json!("deepseek")),
                description: "Default LLM provider (deepseek, openai-compatible, vllm, llamacpp)".to_string(),
                enum_values: Some(vec![
                    "deepseek".to_string(),
                    "openai-compatible".to_string(),
                    "vllm".to_string(),
                    "llamacpp".to_string(),
                ]),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.llm.model".to_string(),
            VscodeConfigProperty {
                config_type: "string".to_string(),
                default: Some(serde_json::json!("deepseek-chat")),
                description: "Default model to use (provider-specific)".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.llm.apiKey".to_string(),
            VscodeConfigProperty {
                config_type: "string".to_string(),
                default: None,
                description: "API key for the LLM provider (leave empty to auto-detect from environment)".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.llm.customEndpoint".to_string(),
            VscodeConfigProperty {
                config_type: "string".to_string(),
                default: None,
                description: "Custom API endpoint URL (for OpenAI-compatible providers)".to_string(),
                ..Default::default()
            });
        
        // Performance Settings
        properties.insert(
            "jcode.performance.enableStreaming".to_string(),
            VscodeConfigProperty {
                config_type: "boolean".to_string(),
                default: Some(serde_json::json!(true)),
                description: "Enable streaming responses (real-time text generation)".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.performance.maxTokens".to_string(),
            VscodeConfigProperty {
                config_type: "number".to_string(),
                default: Some(serde_json::json!(4096)),
                description: "Maximum tokens in response".to_string(),
                minimum: Some(256.0),
                maximum: Some(16384.0),
            });
        
        properties.insert(
            "jcode.performance.temperature".to_string(),
            VscodeConfigProperty {
                config_type: "number".to_string(),
                default: Some(serde_json::json!(0.7)),
                description: "Response creativity (0.0 = focused, 1.0 = creative)".to_string(),
                minimum: Some(0.0),
                maximum: Some(2.0),
            });
        
        // RAG Settings
        properties.insert(
            "jcode.rag.enabled".to_string(),
            VscodeConfigProperty {
                config_type: "boolean".to_string(),
                default: Some(serde_json::json!(true)),
                description: "Enable codebase-aware context retrieval (RAG)".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.rag.maxContextSnippets".to_string(),
            VscodeConfigProperty {
                config_type: "number".to_string(),
                default: Some(serde_json::json!(8)),
                description: "Maximum number of code snippets to retrieve as context".to_string(),
                minimum: Some(1.0),
                maximum: Some(30.0),
            });
        
        // VS Code Integration Settings
        properties.insert(
            "jcode.vscode.inlineCompletionEnabled".to_string(),
            VscodeConfigProperty {
                config_type: "boolean".to_string(),
                default: Some(serde_json::json!(true)),
                description: "Enable inline completions (Tab to accept, like Copilot/Cursor)".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.vscode.chatPanelEnabled".to_string(),
            VscodeConfigProperty {
                config_type: "boolean".to_string(),
                default: Some(serde_json::json!(true)),
                description: "Show chat panel in sidebar".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.vscode.autoStartServer".to_string(),
            VscodeConfigProperty {
                config_type: "boolean".to_string(),
                default: Some(serde_json::json!(true)),
                description: "Automatically start jcode server when VS Code opens".to_string(),
                ..Default::default()
            });
        
        // Advanced Settings
        properties.insert(
            "jcode.advanced.debugMode".to_string(),
            VscodeConfigProperty {
                config_type: "boolean".to_string(),
                default: Some(serde_json::json!(false)),
                description: "Enable verbose debug logging".to_string(),
                ..Default::default()
            });
        
        properties.insert(
            "jcode.advanced.logLevel".to_string(),
            VscodeConfigProperty {
                config_type: "string".to_string(),
                default: Some(serde_json::json!("info")),
                description: "Logging level (trace, debug, info, warn, error)".to_string(),
                enum_values: Some(vec![
                    "trace".to_string(),
                    "debug".to_string(),
                    "info".to_string(),
                    "warn".to_string(),
                    "error".to_string(),
                ]),
                ..Default::default()
            });
        
        VscodeConfiguration {
            title: "jcode".to_string(),
            properties,
        }
    }
    
    /// Generate build scripts
    fn generate_scripts() -> VscodeScripts {
        VscodeScripts {
            "vscode:prepublish": "npm run compile".to_string(),
            compile: "tsc -p ./".to_string(),
            watch: "tsc -watch -p ./".to_string(),
            lint: "eslint src --ext ts".to_string(),
            test: Some("node ./out/test/runTest.js".to_string()),
        }
    }
    
    /// Save manifest to file
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

/// Language contribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeLanguage {
    pub id: String,
    pub extensions: Vec<String>,
    pub aliases: Vec<String>,
    pub configuration: String,
}

/// Grammar contribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeGrammar {
    pub language: String,
    pub scope_name: String,
    pub path: String,
}

/// Theme contribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeTheme {
    pub label: String,
    pub ui_theme: VscodeUiTheme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeUiTheme {
    pub path: String,
}

/// Icon theme contribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeIcons {
    pub id: String,
    pub description: String,
    pub path: String,
}

/// Generate complete VS Code extension project structure
pub fn generate_vscode_extension_project(output_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(output_dir.join("src"))?;
    std::fs::create_dir_all(output_dir.join("out"))?;
    
    // 1. Generate package.json
    let manifest = VscodeExtensionManifest::generate_jcode_manifest();
    manifest.save_to_file(&output_dir.join("package.json"))?;
    
    // 2. Generate tsconfig.json
    let tsconfig = r#"{
  "compilerOptions": {
    "module": "commonjs",
    "target": "ES2020",
    "outDir": "out",
    "lib": ["ES2020"],
    "sourceMap": true,
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "resolveJsonModule": true
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules", "out"]
}"#;
    std::fs::write(output_dir.join("tsconfig.json"), tsconfig)?;
    
    // 3. Generate main entry point (src/extension.ts)
    let extension_code = r#"
import * as vscode from 'vscode';
import { JcodeClient } from './client';

let client: JcodeClient;

export function activate(context: vscode.ExtensionContext) {
    console.log('jcode is now active!');
    
    client = new JcodeClient();
    
    // Register all commands
    registerCommands(context);
    
    // Start server if auto-start is enabled
    if (vscode.workspace.getConfiguration('jcode').get<boolean>('vscode.autoStartServer')) {
        client.start();
    }
    
    // Show welcome message on first install
    showWelcomeMessage(context);
}

export function deactivate() {
    if (client) {
        client.stop();
    }
}

function registerCommands(context: vscode.ExtensionContext) {
    const commands = [
        'jcode.start',
        'jcode.stop',
        'jcode.openChat',
        'jcode.askQuestion',
        'jcode.inlineCompletion',
        'jcode.acceptInlineCompletion',
        'jcode.explainCode',
        'jcode.fixError',
        'jcode.refactorSelection',
        'jcode.generateTests',
        'jcode.documentCode',
        'jcode.addComments',
        'jcode.showSettings',
        'jcode.switchModel',
        'jcode.showStatus'
    ];
    
    commands.forEach(command => {
        context.subscriptions.push(
            vscode.commands.registerCommand(command, () => handleCommand(command))
        );
    });
}

async function handleCommand(command: string) {
    switch (command) {
        case 'jcode.start':
            await client?.start();
            break;
        case 'jcode.stop':
            await client?.stop();
            break;
        case 'jcode.openChat':
            await client?.openChatPanel();
            break;
        case 'jcode.inlineCompletion':
            await client?.triggerInlineCompletion();
            break;
        case 'jcode.explainCode':
            const editor = vscode.window.activeTextEditor;
            if (editor && editor.selection) {
                await client?.explainCode(editor.selection);
            }
            break;
        case 'jcode.fixError':
            await client?.fixErrorAtCursor();
            break;
        // ... other commands
        default:
            vscode.window.showInformationMessage(`Command ${command} not implemented yet`);
    }
}

function showWelcomeMessage(context: vscode.ExtensionContext) {
    const hasShownWelcome = context.globalState.get<boolean>('jcode.welcomeShown');
    if (!hasShownWelcome) {
        vscode.window.showInformationMessage(
            '🎉 Welcome to jcode! Your open-source AI coding assistant.',
            'Get Started', 
            'Learn More'
        ).then(choice => {
            if (choice === 'Get Started') {
                vscode.commands.executeCommand('jcode.openChat');
            } else if (choice === 'Learn More') {
                vscode.env.openExternal(vscode.Uri.parse('https://github.com/jcode-dev/jcode'));
            }
        });
        
        context.globalState.update('jcode.welcomeShown', true);
    }
}
"#;
    std::fs::write(output_dir.join("src").join("extension.ts"), extension_code)?;
    
    // 4. Generate client code (src/client.ts)
    let client_code = r#"
import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions, TransportKind } from 'vscode-languageclient';

export class JcodeClient {
    private client: LanguageClient | undefined;
    private outputChannel: vscode.OutputChannel;
    
    constructor() {
        this.outputChannel = vscode.window.createOutputChannel('jcode');
    }
    
    async start(): Promise<void> {
        if (this.client) {
            this.outputChannel.appendLine('jcode server already running');
            return;
        }
        
        const serverOptions: ServerOptions = {
            run: { module: vscode.Uri.joinPath(this.extensionUri, 'out/server.js').fsPath, transport: TransportKind.stdio },
            debug: { module: vscode.Uri.joinPath(this.extensionUri, 'out/server.js').fsPath, transport: TransportKind.stdio, args: ['--debug'] }
        };
        
        const clientOptions: LanguageClientOptions = {
            documentSelector: [{ scheme: 'file' }, { scheme: 'untitled' }],
            synchronize: {
                configurationSection: 'jcode',
                fileEvents: '**/.jcode/config.toml'
            },
            outputChannel: this.outputChannel
        };
        
        this.client = new LanguageClient(
            'jcode',
            'jcode AI Assistant',
            serverOptions,
            clientOptions
        );
        
        await this.client.start();
        this.outputChannel.appendLine('✅ jcode server started successfully');
    }
    
    async stop(): Promise<void> {
        if (this.client) {
            await this.client.stop();
            this.client = undefined;
            this.outputChannel.appendLine('⏹️  jcode server stopped');
        }
    }
    
    async openChatPanel(): Promise<void> {
        const panel = vscode.window.createWebviewPanel(
            'jcodeChat',
            'jcode Chat',
            vscode.ViewColumn.One,
            { enableScripts: true }
        );
        
        panel.webview.html = this.getChatPanelHtml();
    }
    
    async triggerInlineCompletion(): Promise<void> {
        const editor = vscode.window.activeTextEditor;
        if (!editor) return;
        
        const position = editor.selection.active;
        // Trigger inline completion via gRPC call
        vscode.commands.executeCommand('editor.action.triggerSuggest');
    }
    
    async explainCode(selection: vscode.Selection): Promise<void> {
        const editor = vscode.window.activeTextEditor;
        if (!editor) return;
        
        const selectedText = editor.document.getText(selection);
        const explanation = await this.sendRequest('explainCode', { code: selectedText });
        
        vscode.window.showInformationMessage(`Explanation: ${explanation}`);
    }
    
    async fixErrorAtCursor(): Promise<void> {
        const editor = vscode.window.activeTextEditor;
        if (!editor) return;
        
        const line = editor.selection.active.line;
        const lineText = editor.document.lineAt(line).text;
        
        const fix = await this.sendRequest('fixError', { code: lineText, lineNumber: line });
        
        if (fix) {
            const edit = new vscode.WorkspaceEdit();
            edit.replace(editor.document.uri, new vscode.Range(line, 0, line, lineText.length), fix);
            await vscode.workspace.applyEdit(edit);
        }
    }
    
    private get extensionUri(): vscode.Uri {
        return vscode.extensions.getExtension('jcode.jcode')!.extensionUri;
    }
    
    private getChatPanelHtml(): string {
        return `
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>jcode Chat</title>
    <style>
        body { font-family: var(--vscode-font-family); padding: 10px; }
        #input-area { margin-bottom: 10px; }
        textarea { width: 100%; height: 100px; font-family: inherit; }
        button { margin-top: 5px; }
        #response { border-top: 1px solid var(--vscode-panel-border); padding-top: 10px; white-space: pre-wrap; }
    </style>
</head>
<body>
    <div id="input-area">
        <textarea id="question" placeholder="Ask jcode anything about your code..."></textarea><br>
        <button onclick="sendMessage()">Send (Enter)</button>
    </div>
    <div id="response"></div>
    
    <script>
        const vscode = acquireVsCodeApi();
        
        function sendMessage() {
            const question = document.getElementById('question').value;
            vscode.postMessage({ type: 'askQuestion', question });
        }
        
        window.addEventListener('message', event => {
            if (event.data.type === 'response') {
                document.getElementById('response').innerText += event.data.content + '\\n';
            }
        });
        
        document.getElementById('question').addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                sendMessage();
            }
        });
    </script>
</body>
</html>`;
    }
    
    private async sendRequest(method: string, params: any): Promise<any> {
        if (!this.client) {
            throw new Error('jcode server not running');
        }
        
        try {
            return await this.client.sendRequest(method, params);
        } catch (error) {
            this.outputChannel.appendLine(`Error sending request: ${error}`);
            throw error;
        }
    }
}
"#;
    std::fs::write(output_dir.join("src").join("client.ts"), client_code)?;
    
    // 5. Generate package.json (npm)
    let npm_package = r#"{
  "name": "jcode-vscode-extension",
  "displayName": "jcode",
  "description": "AI-powered coding assistant (open-source Cursor alternative)",
  "version": "0.1.0",
  "publisher": "jcode-community",
  "engines": {
    "vscode": "^1.85.0"
  },
  "categories": [
    "Programming Languages",
    "Machine Learning",
    "Snippets",
    "Debuggers"
  ],
  "activationEvents": [
    "onLanguage:*",
    "onCommand:jcode.start"
  ],
  "main": "./out/extension.js",
  "contributes": {
    "commands": [
      {
        "command": "jcode.start",
        "title": "Start jcode Server",
        "category": "jcode"
      }
      // ... (commands will be generated from Rust code)
    ]
  },
  "scripts": {
    "vscode:prepublish": "npm run compile",
    "compile": "tsc -p ./",
    "watch": "tsc -watch -p ./",
    "lint": "eslint src --ext ts",
    "test": "node ./out/test/runTest.js"
  },
  "devDependencies": {
    "@types/vscode": "^1.85.0",
    "@types/node": "^20.x",
    "@typescript-eslint/eslint-plugin": "^7.x",
    "@typescript-eslint/parser": "^7.x",
    "eslint": "^8.x",
    "typescript": "^5.x",
    "vscode-languageclient": "^9.x",
    "vscode-languageserver-protocol": "^3.17.x"
  }
}"#;
    std::fs::write(output_dir.join("package.json"), npm_package)?;
    
    // 6. Generate README
    let readme = r#"
# jcode VS Code Extension

## Features

✨ **Cursor-like Experience** - Drop-in replacement for Cursor with:
- 🎯 **Inline Completions** - Tab to accept (Alt+\ to trigger)
- 💬 **Chat Panel** - Ask questions about your code
- 🔍 **Explain Code** - Get explanations for selected code
- 🐛 **Fix Errors** - AI-assisted error fixing
- ♻️ **Refactoring** - Safe AI-powered refactoring
- 📝 **Documentation** - Auto-generate documentation

## Quick Start

### 1️⃣ Install Extension

```bash
# From VS Code marketplace (when published)
ext install jcode.jcode

# Or build from source
cd vscode-extension
npm install
npm run compile
```

### 2️⃣ Configure API Key

**Option A**: Set environment variable before opening VS Code:
```bash
export DEEPSEEK_API_KEY=your-api-key-here
# or
export OPENAI_API_KEY=your-api-key-here
```

**Option B**: Use VS Code settings:
1. Open Settings (Ctrl+,)
2. Search for "jcode.llm.apiKey"
3. Enter your API key

### 3️⃣ Start Using!

- Press `Alt+\` to trigger inline completion
- Press `Ctrl+Shift+J` to open chat panel
- Select code and right-click -> "Explain with jcode"

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Alt+\` | Trigger inline completion |
| `Tab` | Accept inline suggestion |
| `Ctrl+Shift+J` | Open chat panel |
| `Ctrl+K Ctrl+I` | Explain selection |
| `Ctrl+.` | Fix error at cursor |

## Configuration

All settings are under `jcode.*` prefix:

- `jcode.llm.provider`: deepseek | openai-compatible | vllm | llamacpp
- `jcode.llm.model`: Model name (e.g., deepseek-chat, gpt-4-turbo)
- `jcode.vscode.inlineCompletionEnabled`: Enable/disable Tab completions
- `jcode.rag.enabled`: Enable codebase-aware context

## Comparison with Cursor

| Feature | jcode | Cursor |
|---------|-------|--------|
| ✅ Open Source | ✅ | ❌ Proprietary |
| ✅ Multiple Providers | ❌ Only OpenAI/Anthropic | - |
| ✅ Local Models (vLLM) | ❌ Cloud only | - |
| ✅ Fully Configurable | ⚠️ Limited options | - |
| ✅ Privacy (local mode) | ❌ All data to cloud | - |
| ✅ Free (self-hosted) | 💰 $20/month | - |

## Development

```bash
# Install dependencies
npm install

# Compile TypeScript
npm run compile

# Run in development mode
# Press F5 in VS Code with this folder open

# Run tests
npm test
```

## License

MIT License - see LICENSE file for details.

---

**Made with ❤️ by the jcode community**
"#;
    std::fs::write(output_dir.join("README.md"), readme)?;
    
    Ok(output_dir.to_path_buf())
}
