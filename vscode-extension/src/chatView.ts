import * as vscode from 'vscode';
import { CarpAiClient } from './client';

interface ChatMessage {
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp?: number;
}

export class ChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'carpai.chatView';

  private view?: vscode.WebviewView;
  private client: CarpAiClient;
  private messages: ChatMessage[] = [];
  private extensionUri: vscode.Uri;

  constructor(extensionUri: vscode.Uri, client: CarpAiClient) {
    this.extensionUri = extensionUri;
    this.client = client;
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ) {
    this.view = webviewView;

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this.extensionUri],
    };

    webviewView.webview.html = this.getHtmlForWebview(webviewView.webview);

    // Handle messages from webview
    webviewView.webview.onDidReceiveMessage(async (message) => {
      switch (message.type) {
        case 'sendMessage':
          await this.handleUserMessage(message.text);
          break;
        case 'clearChat':
          this.messages = [];
          this.updateWebview();
          break;
      }
    });

    this.updateWebview();
  }

  public addMessage(message: ChatMessage) {
    this.messages.push({
      ...message,
      timestamp: Date.now(),
    });
    this.updateWebview();
  }

  private async handleUserMessage(text: string) {
    if (!text.trim()) {
      return;
    }

    // Add user message
    this.addMessage({ role: 'user', content: text });

    try {
      // Show typing indicator
      this.addMessage({ role: 'assistant', content: 'Thinking...' });

      // Get response from CarpAI
      const response = await this.client.complete(text);

      // Remove typing indicator and add actual response
      this.messages.pop();
      this.addMessage({
        role: 'assistant',
        content: response.text,
      });

      // Show token usage
      vscode.window.showInformationMessage(
        `Tokens: ${response.usage.total_tokens} (${response.usage.prompt_tokens} prompt + ${response.usage.completion_tokens} completion)`
      );
    } catch (error) {
      this.messages.pop();
      this.addMessage({
        role: 'system',
        content: `Error: ${error instanceof Error ? error.message : String(error)}`,
      });
    }
  }

  private updateWebview() {
    if (this.view) {
      this.view.webview.postMessage({
        type: 'updateMessages',
        messages: this.messages,
      });
    }
  }

  private getHtmlForWebview(webview: vscode.Webview): string {
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>CarpAI Chat</title>
  <style>
    body {
      font-family: var(--vscode-font-family);
      padding: 10px;
      margin: 0;
      background-color: var(--vscode-sideBar-background);
      color: var(--vscode-foreground);
    }
    .chat-container {
      display: flex;
      flex-direction: column;
      height: calc(100vh - 100px);
    }
    .messages {
      flex: 1;
      overflow-y: auto;
      padding: 10px;
    }
    .message {
      margin-bottom: 15px;
      padding: 10px;
      border-radius: 6px;
      max-width: 90%;
    }
    .message.user {
      background-color: var(--vscode-button-background);
      color: var(--vscode-button-foreground);
      align-self: flex-end;
      margin-left: auto;
    }
    .message.assistant {
      background-color: var(--vscode-editor-background);
      border: 1px solid var(--vscode-panel-border);
    }
    .message.system {
      background-color: var(--vscode-inputValidation-errorBackground);
      color: var(--vscode-inputValidation-errorForeground);
      font-size: 0.9em;
    }
    .input-area {
      display: flex;
      gap: 8px;
      padding: 10px;
      border-top: 1px solid var(--vscode-panel-border);
    }
    textarea {
      flex: 1;
      resize: none;
      padding: 8px;
      border: 1px solid var(--vscode-input-border);
      background-color: var(--vscode-input-background);
      color: var(--vscode-input-foreground);
      border-radius: 4px;
      font-family: var(--vscode-font-family);
    }
    button {
      padding: 8px 16px;
      background-color: var(--vscode-button-background);
      color: var(--vscode-button-foreground);
      border: none;
      border-radius: 4px;
      cursor: pointer;
    }
    button:hover {
      background-color: var(--vscode-button-hoverBackground);
    }
    .clear-btn {
      background-color: var(--vscode-errorForeground);
    }
    pre {
      background-color: var(--vscode-textCodeBlock-background);
      padding: 8px;
      border-radius: 4px;
      overflow-x: auto;
    }
    code {
      font-family: var(--vscode-editor-font-family);
      font-size: var(--vscode-editor-font-size);
    }
  </style>
</head>
<body>
  <div class="chat-container">
    <div class="messages" id="messages"></div>
    <div class="input-area">
      <textarea
        id="input"
        placeholder="Ask CarpAI..."
        rows="2"
      ></textarea>
      <button onclick="sendMessage()">Send</button>
      <button class="clear-btn" onclick="clearChat()">Clear</button>
    </div>
  </div>

  <script>
    const vscode = acquireVsCodeApi();
    let messages = [];

    window.addEventListener('message', event => {
      const message = event.data;
      if (message.type === 'updateMessages') {
        messages = message.messages;
        renderMessages();
      }
    });

    function renderMessages() {
      const container = document.getElementById('messages');
      container.innerHTML = messages.map(msg => {
        const content = msg.content
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;');

        const formattedContent = content.replace(
          /\`\`\`([\\s\\S]*?)\`\`\`/g,
          '<pre><code>$1</code></pre>'
        );

        return \`<div class="message \${msg.role}">\${formattedContent}</div>\`;
      }).join('');

      container.scrollTop = container.scrollHeight;
    }

    function sendMessage() {
      const input = document.getElementById('input');
      const text = input.value.trim();
      if (text) {
        vscode.postMessage({ type: 'sendMessage', text });
        input.value = '';
      }
    }

    function clearChat() {
      vscode.postMessage({ type: 'clearChat' });
    }

    document.getElementById('input').addEventListener('keypress', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    });
  </script>
</body>
</html>`;
  }
}
