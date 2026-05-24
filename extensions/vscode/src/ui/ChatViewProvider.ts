import * as vscode from "vscode";

export class ChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "carpai.chatView";
  private _view?: vscode.WebviewView;

  constructor(private readonly _extensionUri: vscode.Uri) {}

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ) {
    this._view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this._extensionUri],
    };
    webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);

    // Handle messages from webview
    webviewView.webview.onDidReceiveMessage((message) => {
      switch (message.command) {
        case "sendMessage":
          this._handleSendMessage(message.text);
          break;
      }
    });
  }

  private async _handleSendMessage(text: string) {
    // TODO: Call carpai-sdk chat_completion
    this._view?.webview.postMessage({
      type: "response",
      text: `Echo: ${text}`,
    });
  }

  private _getHtmlForWebview(webview: vscode.Webview): string {
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>CarpAI Chat</title>
  <style>
    body { padding: 16px; font-family: var(--vscode-font-family); }
    #messages { height: 300px; overflow-y: auto; margin-bottom: 12px; border: 1px solid var(--vscode-panel-border); padding: 8px; }
    .message { margin-bottom: 8px; padding: 8px; border-radius: 4px; }
    .user { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
    .assistant { background: var(--vscode-editor-background); }
    #input-area { display: flex; gap: 8px; }
    #input { flex: 1; padding: 8px; }
    button { padding: 8px 16px; }
  </style>
</head>
<body>
  <div id="messages"></div>
  <div id="input-area">
    <input id="input" placeholder="Ask CarpAI..." />
    <button id="send">Send</button>
  </div>
  <script>
    const vscode = acquireVsCodeApi();
    const messagesDiv = document.getElementById("messages");
    const input = document.getElementById("input");
    const sendBtn = document.getElementById("send");

    function addMessage(text, role) {
      const div = document.createElement("div");
      div.className = /`message /${role}/`;
      div.textContent = text;
      messagesDiv.appendChild(div);
      messagesDiv.scrollTop = messagesDiv.scrollHeight;
    }

    sendBtn.onclick = () => {
      const text = input.value.trim();
      if (text) {
        addMessage(text, "user");
        vscode.postMessage({ command: "sendMessage", text });
        input.value = "";
      }
    };

    window.addEventListener("message", (event) => {
      if (event.data.type === "response") {
        addMessage(event.data.text, "assistant");
      }
    });
  </script>
</body>
</html>`;
  }
}
