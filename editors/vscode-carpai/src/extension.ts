import * as vscode from 'vscode';
import { CarpaiClient } from './carpaiClient';
import { CarpaiCompletionProvider } from './completionProvider';
import { ChatPanel } from './chatPanel';

export async function activate(context: vscode.ExtensionContext) {
    const client = new CarpaiClient();
    
    const healthCheck = await client.healthCheck();
    if (!healthCheck) {
        vscode.window.showWarningMessage(
            'CarpAI server not found. Please start the CarpAI server or check your configuration.'
        );
    }

    const completionProvider = new CarpaiCompletionProvider(client);
    
    const completionDisposable = vscode.languages.registerCompletionItemProvider(
        { scheme: 'file' },
        completionProvider,
        '.', '(', '[', '"', "'"
    );

    const completeCommand = vscode.commands.registerCommand('carpai.complete', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }

        const document = editor.document;
        const position = editor.selection.active;
        const file_path = document.uri.fsPath;
        const content = document.getText();

        const response = await client.getCompletions({
            file_path,
            content,
            line: position.line,
            character: position.character,
        });

        if (response.items.length === 0) {
            vscode.window.showInformationMessage('No completions available');
            return;
        }

        const items = response.items.map(item => ({
            label: item.label,
            description: item.documentation || ''
        }));

        const selected = await vscode.window.showQuickPick(items);
        if (selected) {
            const item = response.items.find(i => i.label === selected.label);
            if (item && item.text_edit) {
                const edit = new vscode.WorkspaceEdit();
                edit.replace(
                    document.uri,
                    new vscode.Range(
                        new vscode.Position(item.text_edit.range.start.line, item.text_edit.range.start.character),
                        new vscode.Position(item.text_edit.range.end.line, item.text_edit.range.end.character)
                    ),
                    item.text_edit.new_text
                );
                await vscode.workspace.applyEdit(edit);
            }
        }
    });

    const chatCommand = vscode.commands.registerCommand('carpai.chat', () => {
        ChatPanel.createOrShow(context.extensionPath);
    });

    const reviewCommand = vscode.commands.registerCommand('carpai.review', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }

        const document = editor.document;
        const file_path = document.uri.fsPath;
        const content = document.getText();

        const result = await client.review({ file_path, content });

        if (result.issues.length === 0) {
            vscode.window.showInformationMessage('No issues found in the code');
            return;
        }

        const issuesBySeverity = {
            error: result.issues.filter(i => i.severity === 'error'),
            warning: result.issues.filter(i => i.severity === 'warning'),
            info: result.issues.filter(i => i.severity === 'info')
        };

        const diagnosticCollection = vscode.languages.createDiagnosticCollection('carpai-review');
        const diagnostics: vscode.Diagnostic[] = [];

        for (const issue of result.issues) {
            const severity = issue.severity === 'error' 
                ? vscode.DiagnosticSeverity.Error 
                : issue.severity === 'warning' 
                    ? vscode.DiagnosticSeverity.Warning 
                    : vscode.DiagnosticSeverity.Information;

            const range = new vscode.Range(
                new vscode.Position(issue.line - 1, issue.column - 1),
                new vscode.Position(issue.line - 1, issue.column)
            );

            diagnostics.push(new vscode.Diagnostic(range, issue.message, severity));
        }

        diagnosticCollection.set(document.uri, diagnostics);

        const message = `Found ${result.issues.length} issues: ${issuesBySeverity.error.length} errors, ${issuesBySeverity.warning.length} warnings, ${issuesBySeverity.info.length} info`;
        vscode.window.showInformationMessage(message);
    });

    const explainCommand = vscode.commands.registerCommand('carpai.explain', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }

        const selection = editor.selection;
        if (selection.isEmpty) {
            vscode.window.showErrorMessage('Please select some code to explain');
            return;
        }

        const code = editor.document.getText(selection);
        const explanation = await client.explainCode(code);

        const panel = vscode.window.createWebviewPanel(
            'carpai-explain',
            'Code Explanation',
            vscode.ViewColumn.Beside,
            {}
        );

        panel.webview.html = `
            <!DOCTYPE html>
            <html>
            <body style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; padding: 16px; background: #1e1e1e; color: #d4d4d4;">
                <h2>Code Explanation</h2>
                <pre style="background: #2d2d30; padding: 12px; border-radius: 8px; overflow-x: auto;">${escapeHtml(code)}</pre>
                <div style="margin-top: 16px; line-height: 1.6;">${formatMarkdown(explanation)}</div>
            </body>
            </html>
        `;
    });

    const refactorCommand = vscode.commands.registerCommand('carpai.refactor', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }

        const selection = editor.selection;
        if (selection.isEmpty) {
            vscode.window.showErrorMessage('Please select some code to refactor');
            return;
        }

        const code = editor.document.getText(selection);
        const instructions = await vscode.window.showInputBox({
            prompt: 'Enter refactoring instructions',
            placeHolder: 'e.g., "Make this function more readable"'
        });

        if (!instructions) return;

        const refactored = await client.refactorCode(code, instructions);

        await editor.edit(editBuilder => {
            editBuilder.replace(selection, refactored);
        });

        vscode.window.showInformationMessage('Code refactored successfully');
    });

    const generateTestsCommand = vscode.commands.registerCommand('carpai.generateTests', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }

        const selection = editor.selection;
        const code = selection.isEmpty 
            ? editor.document.getText() 
            : editor.document.getText(selection);

        const tests = await client.generateTests(code);

        const doc = await vscode.workspace.openTextDocument({
            content: tests,
            language: 'typescript'
        });
        await vscode.window.showTextDocument(doc);
    });

    const configCommand = vscode.commands.registerCommand('carpai.config', () => {
        vscode.commands.executeCommand('workbench.action.openSettings', 'carpai');
    });

    const summarizeCommand = vscode.commands.registerCommand('carpai.summarize', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }

        const document = editor.document;
        const content = document.getText();

        const summary = await client.chat({
            message: `Summarize this code:\n\n${content}`
        });

        vscode.window.showInformationMessage(summary.response);
    });

    context.subscriptions.push(
        completionDisposable,
        completeCommand,
        chatCommand,
        reviewCommand,
        explainCommand,
        refactorCommand,
        generateTestsCommand,
        configCommand,
        summarizeCommand
    );

    context.subscriptions.push(
        vscode.window.registerWebviewPanelSerializer('carpaiChat', {
            async deserializeWebviewPanel(panel) {
                ChatPanel.createOrShow(context.extensionPath);
            }
        })
    );

    vscode.window.onDidChangeActiveTextEditor(async (editor) => {
        if (editor) {
            const health = await client.healthCheck();
            if (!health) {
                vscode.window.showWarningMessage(
                    'CarpAI server is not running. Some features may not work.'
                );
            }
        }
    });
}

function escapeHtml(text: string): string {
    return text
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#039;');
}

function formatMarkdown(text: string): string {
    return text
        .replace(/```(\w+)?\n([\s\S]*?)```/g, '<pre><code>$2</code></pre>')
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\n/g, '<br>');
}

export function deactivate() {}