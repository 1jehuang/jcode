import * as vscode from 'vscode';

export class ChatPanel {
    private static panel: vscode.WebviewPanel | undefined;
    private static extensionPath: string;

    public static createOrShow(extensionPath: string) {
        this.extensionPath = extensionPath;

        if (this.panel) {
            this.panel.reveal(vscode.ViewColumn.Beside);
            return;
        }

        this.panel = vscode.window.createWebviewPanel(
            'carpaiChat',
            'CarpAI Chat',
            vscode.ViewColumn.Beside,
            {
                enableScripts: true,
                retainContextWhenHidden: true,
            }
        );

        this.panel.webview.html = this.getWebviewContent();

        this.panel.onDidDispose(() => {
            this.panel = undefined;
        });
    }

    private static getWebviewContent(): string {
        return `
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>CarpAI Chat</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            padding: 16px;
            background: #1e1e1e;
            color: #d4d4d4;
            min-height: 100vh;
            display: flex;
            flex-direction: column;
        }
        #chat-container {
            flex: 1;
            overflow-y: auto;
            margin-bottom: 16px;
        }
        .message {
            margin-bottom: 16px;
            padding: 12px;
            border-radius: 8px;
        }
        .user-message {
            background: #007acc;
            color: white;
            margin-left: auto;
            max-width: 80%;
        }
        .assistant-message {
            background: #2d2d30;
            border: 1px solid #3c3c3c;
        }
        .message-content {
            white-space: pre-wrap;
            word-break: break-all;
        }
        .message-role {
            font-weight: 600;
            margin-bottom: 4px;
            font-size: 12px;
            opacity: 0.7;
        }
        #input-container {
            display: flex;
            gap: 8px;
        }
        #message-input {
            flex: 1;
            padding: 12px;
            border: 1px solid #3c3c3c;
            border-radius: 8px;
            background: #2d2d30;
            color: #d4d4d4;
            font-family: inherit;
            font-size: 14px;
            outline: none;
        }
        #send-button {
            padding: 12px 24px;
            background: #007acc;
            color: white;
            border: none;
            border-radius: 8px;
            cursor: pointer;
            font-size: 14px;
        }
        #send-button:hover {
            background: #005a9e;
        }
        .typing-indicator {
            display: inline-block;
            animation: typing 1.5s infinite;
        }
        @keyframes typing {
            0%, 100% { opacity: 0.4; }
            50% { opacity: 1; }
        }
    </style>
</head>
<body>
    <div id="chat-container">
        <div class="message assistant-message">
            <div class="message-role">CarpAI</div>
            <div class="message-content">Hello! I'm CarpAI, your AI coding assistant. How can I help you today?</div>
        </div>
    </div>
    <div id="input-container">
        <input type="text" id="message-input" placeholder="Type your message...">
        <button id="send-button">Send</button>
    </div>

    <script>
        const vscode = acquireVsCodeApi();
        const chatContainer = document.getElementById('chat-container');
        const messageInput = document.getElementById('message-input');
        const sendButton = document.getElementById('send-button');

        function addMessage(role, content, isTyping = false) {
            const messageDiv = document.createElement('div');
            messageDiv.className = role === 'user' ? 'message user-message' : 'message assistant-message';
            
            const roleDiv = document.createElement('div');
            roleDiv.className = 'message-role';
            roleDiv.textContent = role === 'user' ? 'You' : 'CarpAI';
            
            const contentDiv = document.createElement('div');
            contentDiv.className = 'message-content';
            contentDiv.textContent = isTyping ? 'Typing...' : content;
            
            messageDiv.appendChild(roleDiv);
            messageDiv.appendChild(contentDiv);
            chatContainer.appendChild(messageDiv);
            chatContainer.scrollTop = chatContainer.scrollHeight;
            return messageDiv;
        }

        async function sendMessage() {
            const message = messageInput.value.trim();
            if (!message) return;

            messageInput.value = '';
            addMessage('user', message);
            
            const typingDiv = addMessage('assistant', '', true);

            vscode.postMessage({
                type: 'chat',
                message: message
            });
        }

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.type === 'chatResponse') {
                document.querySelectorAll('.assistant-message').forEach(el => {
                    const content = el.querySelector('.message-content');
                    if (content && content.textContent === 'Typing...') {
                        content.textContent = message.response;
                    }
                });
            }
        });

        sendButton.addEventListener('click', sendMessage);
        messageInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                sendMessage();
            }
        });
    </script>
</body>
</html>
        `;
    }
}