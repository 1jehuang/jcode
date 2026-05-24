import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';

export class ChatPanel {
    private static panel: vscode.WebviewPanel | undefined;
    private static extensionPath: string;

    public static createOrShow(extensionPath: string) {
        this.extensionPath = extensionPath;

        if (this.panel) {
            this.panel.reveal(vscode.ViewColumn.Beside);
            return;
        }

        const webviewUiPath = path.join(extensionPath, 'webview-ui', 'dist');

        this.panel = vscode.window.createWebviewPanel(
            'carpaiChat',
            'CarpAI Chat',
            vscode.ViewColumn.Beside,
            {
                enableScripts: true,
                retainContextWhenHidden: true,
                localResourceRoots: [
                    vscode.Uri.file(webviewUiPath),
                ],
            }
        );

        this.panel.webview.html = this.getWebviewContent();

        this.panel.onDidDispose(() => {
            this.panel = undefined;
        });

        // Handle messages from the webview
        this.panel.webview.onDidReceiveMessage(
            (message: any) => {
                switch (message.type) {
                    case 'chat':
                        // Forward chat message to the extension host
                        vscode.commands.executeCommand('carpai.handleChat', message.message);
                        return;
                }
            },
            undefined,
            []
        );
    }

    private static getWebviewContent(): string {
        const webviewUiPath = path.join(this.extensionPath, 'webview-ui', 'dist');
        const indexPath = path.join(webviewUiPath, 'index.html');

        if (!fs.existsSync(indexPath)) {
            return `<!DOCTYPE html><html><body><p>React app not built. Run "npm run build:webview" in the extension directory.</p></body></html>`;
        }

        const html = fs.readFileSync(indexPath, 'utf-8');

        // Convert resources to webview URIs
        const webview = this.panel!.webview;
        return html.replace(
            /(src|href)="([^"]+)"/g,
            (match, attribute, filePath) => {
                if (filePath.startsWith('http') || filePath.startsWith('data:') || filePath.startsWith('#') || filePath.startsWith('/')) {
                    return match;
                }
                const diskPath = path.resolve(webviewUiPath, filePath);
                const webviewUri = webview.asWebviewUri(vscode.Uri.file(diskPath));
                return `${attribute}="${webviewUri}"`;
            }
        );
    }
}