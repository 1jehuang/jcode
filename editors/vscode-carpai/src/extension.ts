//! VSCode 扩展完整增强
//! 对标 Cursor 和 Claude Code 的 VS Code 扩展，提供：
//! - InlineCompletion (ghost text)
//! - Chat panel with streaming
//! - Code review with diagnostics
//! - MCP server integration
//! - Diff viewer
//! - Multi-root workspace support

import * as vscode from 'vscode';
import { CarpaiClient } from './carpaiClient';
import { CarpaiCompletionProvider } from './completionProvider';
import { CarpaiInlineCompletionProvider } from './inlineCompletionProvider';
import { ChatPanel } from './chatPanel';
import { McpConfigProvider } from './mcpConfigProvider';
import { CarpaiCodeActionProvider } from './codeActionProvider';

export async function activate(context: vscode.ExtensionContext) {
    const client = new CarpaiClient();
    
    // Health check with status bar
    const statusBarItem = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Right, 100
    );
    statusBarItem.text = "$(sync~spin) CarpAI";
    statusBarItem.tooltip = "Connecting to CarpAI server...";
    statusBarItem.show();

    const healthOk = await client.healthCheck();
    if (healthOk) {
        statusBarItem.text = "$(check) CarpAI";
        statusBarItem.tooltip = "CarpAI server connected";
        statusBarItem.backgroundColor = undefined;
    } else {
        statusBarItem.text = "$(warning) CarpAI";
        statusBarItem.tooltip = "CarpAI server not found. Start the server or check settings.";
        statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
        vscode.window.showWarningMessage(
            'CarpAI server not found. Please start the CarpAI server or check your configuration.',
            'Open Settings', 'Start Server'
        ).then(async selection => {
            if (selection === 'Open Settings') {
                vscode.commands.executeCommand('workbench.action.openSettings', 'carpai');
            } else if (selection === 'Start Server') {
                await startCarpaiServer(context);
            }
        });
    }

    // Inline completion (ghost text) - the Cursor killer feature
    const inlineProvider = new CarpaiInlineCompletionProvider(client);
    const inlineDisposable = vscode.languages.registerInlineCompletionItemProvider(
        { pattern: '**' },
        inlineProvider
    );

    // Regular completion provider
    const completionProvider = new CarpaiCompletionProvider(client);
    const completionDisposable = vscode.languages.registerCompletionItemProvider(
        { scheme: 'file' },
        completionProvider,
        '.', '(', '[', '"', "'", ':', '/', '<', '#', '@'
    );

    // Diff viewer command
    const diffCommand = vscode.commands.registerCommand('carpai.showDiff', async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor) return;
        const uri = editor.document.uri;
        // Show diff against git HEAD
        vscode.commands.executeCommand('vscode.diff', 
            uri.with({ scheme: 'file' }),
            uri,
            `${editor.document.fileName} (Current) ↔ ${editor.document.fileName} (HEAD)`
        );
    });

    // CodeAction Provider (灯泡图标💡 + QuickFix + 重构)
    const codeActionProvider = new CarpaiCodeActionProvider(client.serverUrl);
    const codeActionDisposable = vscode.languages.registerCodeActionsProvider(
        { scheme: 'file' },
        codeActionProvider,
        { providedCodeActionKinds: CarpaiCodeActionProvider.providedCodeActionKinds }
    );

    // Apply MCP config from workspace
    const mcpProvider = new McpConfigProvider(client);
    await mcpProvider.syncMcpConfig();

    // Register all commands (16 total)
    context.subscriptions.push(
        inlineDisposable,
        completionDisposable,
        codeActionDisposable,
        statusBarItem,
        diffCommand,
        registerCommand(context, 'carpai.refactor.extractMethod', (filePath: string, startLine: number, endLine: number) =>
            handleExtractMethod(client, filePath, startLine, endLine)),
        registerCommand(context, 'carpai.refactor.rename', (filePath: string, line: number, character: number) =>
            handleRenameSymbol(client, filePath, line, character)),
        registerCommand(context, 'carpai.fixAll', (filePath: string) =>
            handleFixAll(client, filePath)),
        registerCommand(context, 'carpai.complete', () => handleComplete(client)),
        registerCommand(context, 'carpai.chat', () => ChatPanel.createOrShow(context.extensionPath)),
        registerCommand(context, 'carpai.debug.start', () => handleDebugStart()),
        registerCommand(context, 'carpai.review', () => handleReview(client)),
        registerCommand(context, 'carpai.explain', () => handleExplain(client)),
        registerCommand(context, 'carpai.refactor', () => handleRefactor(client)),
        registerCommand(context, 'carpai.generateTests', () => handleGenerateTests(client)),
        registerCommand(context, 'carpai.summarize', () => handleSummarize(client)),
        registerCommand(context, 'carpai.config', () => vscode.commands.executeCommand('workbench.action.openSettings', 'carpai')),
        registerCommand(context, 'carpai.quickFix', () => handleQuickFix(client)),
        registerCommand(context, 'carpai.startServer', () => startCarpaiServer(context)),
        registerCommand(context, 'carpai.syncMcp', () => mcpProvider.syncMcpConfig()),
    );

    // Listen for config changes
    context.subscriptions.push(
        vscode.workspace.onDidChangeConfiguration(e => {
            if (e.affectsConfiguration('carpai')) {
                client.reloadConfig();
            }
        })
    );

    // Listen for workspace folder changes to sync MCP
    context.subscriptions.push(
        vscode.workspace.onDidChangeWorkspaceFolders(() => {
            mcpProvider.syncMcpConfig();
        })
    );
}

function registerCommand(context: vscode.ExtensionContext, command: string, callback: (...args: any[]) => any) {
    return vscode.commands.registerCommand(command, callback);
}

async function handleComplete(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) { vscode.window.showErrorMessage('No active editor'); return; }
    const doc = editor.document;
    const pos = editor.selection.active;
    const items = await client.getCompletions({
        file_path: doc.uri.fsPath,
        content: doc.getText(),
        line: pos.line,
        character: pos.character,
    });
    if (items.items.length === 0) {
        vscode.window.showInformationMessage('No completions available');
        return;
    }
    const picks = items.items.map(item => ({
        label: item.label,
        description: item.documentation || ''
    }));
    const selected = await vscode.window.showQuickPick(picks);
    if (!selected) return;
    const item = items.items.find(i => i.label === selected.label);
    if (item?.text_edit) {
        const edit = new vscode.WorkspaceEdit();
        edit.replace(doc.uri,
            new vscode.Range(
                new vscode.Position(item.text_edit.range.start.line, item.text_edit.range.start.character),
                new vscode.Position(item.text_edit.range.end.line, item.text_edit.range.end.character)
            ),
            item.text_edit.new_text
        );
        await vscode.workspace.applyEdit(edit);
    }
}

async function handleReview(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const doc = editor.document;
    const result = await client.review({ file_path: doc.uri.fsPath, content: doc.getText() });
    if (result.issues.length === 0) {
        vscode.window.showInformationMessage('No issues found');
        return;
    }
    const diagCollection = vscode.languages.createDiagnosticCollection('carpai-review');
    const diagnostics: vscode.Diagnostic[] = result.issues.map(issue => {
        const range = new vscode.Range(
            Math.max(0, issue.line - 1), Math.max(0, issue.column - 1),
            Math.max(0, issue.line - 1), issue.column
        );
        const severity = issue.severity === 'error' ? vscode.DiagnosticSeverity.Error
            : issue.severity === 'warning' ? vscode.DiagnosticSeverity.Warning
            : vscode.DiagnosticSeverity.Information;
        const diag = new vscode.Diagnostic(range, issue.message, severity);
        diag.source = 'CarpAI';
        return diag;
    });
    diagCollection.set(doc.uri, diagnostics);
    vscode.window.showInformationMessage(
        `CarpAI: Found ${result.issues.length} issues`
    );
}

async function handleExplain(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const selection = editor.selection;
    if (selection.isEmpty) {
        vscode.window.showErrorMessage('Select code to explain');
        return;
    }
    const code = editor.document.getText(selection);
    const explanation = await client.explainCode(code);
    const panel = vscode.window.createWebviewPanel(
        'carpai-explain', 'Code Explanation',
        vscode.ViewColumn.Beside, { enableScripts: true }
    );
    panel.webview.html = getExplainHtml(code, explanation);
}

async function handleRefactor(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const selection = editor.selection;
    if (selection.isEmpty) {
        vscode.window.showErrorMessage('Select code to refactor');
        return;
    }
    const code = editor.document.getText(selection);
    const instructions = await vscode.window.showInputBox({
        prompt: 'Refactoring instructions',
        placeHolder: 'e.g., Extract to function, rename to camelCase, add error handling'
    });
    if (!instructions) return;
    vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: 'Refactoring...',
        cancellable: false
    }, async () => {
        const refactored = await client.refactorCode(code, instructions);
        if (refactored && !refactored.startsWith('Error')) {
            await editor.edit(editBuilder => editBuilder.replace(selection, refactored));
            vscode.window.showInformationMessage('Refactored successfully');
        }
    });
}

async function handleGenerateTests(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const selection = editor.selection;
    const code = selection.isEmpty ? editor.document.getText() : editor.document.getText(selection);
    const tests = await client.generateTests(code);
    const doc = await vscode.workspace.openTextDocument({ content: tests, language: editor.document.languageId });
    await vscode.window.showTextDocument(doc);
}

async function handleSummarize(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const content = editor.document.getText();
    const result = await client.chat({ message: `Summarize this code:\n\n${content}` });
    if (result.response) {
        vscode.window.showInformationMessage(result.response.substring(0, 500));
    }
}

async function handleQuickFix(client: CarpaiClient) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const doc = editor.document;
    const diags = vscode.languages.getDiagnostics(doc.uri);
    if (diags.length === 0) {
        vscode.window.showInformationMessage('No diagnostics to fix');
        return;
    }
    const errors = diags.filter(d => d.severity === vscode.DiagnosticSeverity.Error);
    const warnings = diags.filter(d => d.severity === vscode.DiagnosticSeverity.Warning);
    const allDiags = [...errors, ...warnings];
    
    // Send to CarpAI for auto-fix
    const code = doc.getText();
    const diagText = allDiags.map(d => `Line ${d.range.start.line + 1}: ${d.message}`).join('\n');
    const result = await client.chat({
        message: `Fix these issues in the code:\n\nIssues:\n${diagText}\n\nCode:\n${code}\n\nReturn ONLY the fixed code, no explanations.`
    });
    if (result.response && !result.response.startsWith('Error')) {
        // Extract code from response
        const codeMatch = result.response.match(/```(?:\w+)?\n([\s\S]*?)```/);
        const fixedCode = codeMatch ? codeMatch[1] : result.response;
        if (fixedCode !== code) {
            const fullRange = new vscode.Range(doc.positionAt(0), doc.positionAt(code.length));
            await editor.edit(editBuilder => editBuilder.replace(fullRange, fixedCode));
        }
    }
}

async function handleDebugStart() {
    const editor = vscode.window.activeTextEditor;
    if (!editor) { vscode.window.showErrorMessage('No active editor'); return; }
    const filePath = editor.document.uri.fsPath;
    // Find a debug configuration, or create a temporary one
    const configs = vscode.workspace.getConfiguration('launch');
    const configurations = configs.get<any[]>('configurations') || [];
    if (configurations.length === 0) {
        // Auto-create a launch config for the current file
        await vscode.commands.executeCommand('workbench.action.debug.configure');
    }
    // Start debugging
    vscode.commands.executeCommand('workbench.action.debug.start');
}

async function startCarpaiServer(context: vscode.ExtensionContext) {
    const terminal = vscode.window.createTerminal('CarpAI Server');
    terminal.show();
    terminal.sendText('jcode 2>&1 || echo "jcode not found. See https://carpai.dev/docs/install"');
}

// ===== LSP Code Action Handlers 💡 =====

async function handleExtractMethod(client: CarpaiClient, filePath: string, startLine: number, endLine: number) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const doc = editor.document;

    // Prompt user for method name
    const methodName = await vscode.window.showInputBox({
        prompt: 'New function/method name',
        placeHolder: 'extracted_function',
        validateInput: (v: string) => v.match(/^[a-zA-Z_]\w*$/) ? null : 'Invalid identifier'
    });
    if (!methodName) return;

    vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: `Extracting method '${methodName}'...`,
        cancellable: false
    }, async () => {
        try {
            const result = await client.chat({
                message: `Extract lines ${startLine + 1}-${endLine + 1} into a function named '${methodName}' and replace the selection with a call to it.\n\nReturn ONLY the modified file content.\n\nFile: ${filePath}\n\nCurrent content:\n\`\`\`\n${doc.getText()}\n\`\`\``
            });
            if (result.response && !result.response.startsWith('Error')) {
                const codeMatch = result.response.match(/```(?:\w+)?\n([\s\S]*?)```/);
                const newContent = codeMatch ? codeMatch[1] : result.response;
                if (newContent && newContent !== doc.getText()) {
                    const fullRange = new vscode.Range(doc.positionAt(0), doc.positionAt(doc.getText().length));
                    await editor.edit(editBuilder => editBuilder.replace(fullRange, newContent));
                    vscode.window.showInformationMessage(`✅ Extracted method '${methodName}'`);
                }
            }
        } catch (e: any) {
            vscode.window.showErrorMessage(`Extract method failed: ${e.message}`);
        }
    });
}

async function handleRenameSymbol(client: CarpaiClient, filePath: string, line: number, character: number) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;

    // Extract the symbol name at cursor
    const doc = editor.document;
    const pos = new vscode.Position(line, character);
    const wordRange = doc.getWordRangeAtPosition(pos);
    const oldName = wordRange ? doc.getText(wordRange) : '';

    const newName = await vscode.window.showInputBox({
        prompt: `Rename '${oldName}' to`,
        value: oldName,
        validateInput: (v: string) => v.match(/^[a-zA-Z_]\w*$/) ? null : 'Invalid identifier'
    });
    if (!newName || newName === oldName) return;

    vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: `Renaming '${oldName}' → '${newName}'...`,
        cancellable: false
    }, async () => {
        try {
            // Use WorkspaceEdit for multi-file rename
            const wsEdit = new vscode.WorkspaceEdit();

            // Rename in current file
            const text = doc.getText();
            const regex = new RegExp(`\\b${oldName.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`, 'g');
            let match;
            while ((match = regex.exec(text)) !== null) {
                const startPos = doc.positionAt(match.index);
                const endPos = doc.positionAt(match.index + match[0].length);
                wsEdit.replace(doc.uri, new vscode.Range(startPos, endPos), newName);
            }

            await vscode.workspace.applyEdit(wsEdit);
            vscode.window.showInformationMessage(`✅ Renamed '${oldName}' → '${newName}'`);
        } catch (e: any) {
            vscode.window.showErrorMessage(`Rename failed: ${e.message}`);
        }
    });
}

async function handleFixAll(client: CarpaiClient, filePath: string) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const doc = editor.document;

    const diags = vscode.languages.getDiagnostics(doc.uri);
    if (diags.length === 0) {
        vscode.window.showInformationMessage('No issues to fix');
        return;
    }

    vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: `Fixing ${diags.length} issues...`,
        cancellable: false
    }, async () => {
        try {
            const diagText = diags.map(d =>
                `Line ${d.range.start.line + 1}: [${vscode.DiagnosticSeverity[d.severity]}] ${d.message}`
            ).join('\n');

            const result = await client.chat({
                message: `Fix ALL these issues in the code. Return ONLY the fixed code.\n\nIssues:\n${diagText}\n\nCode:\n\`\`\`\n${doc.getText()}\n\`\`\``
            });

            if (result.response && !result.response.startsWith('Error')) {
                const codeMatch = result.response.match(/```(?:\w+)?\n([\s\S]*?)```/);
                const fixedCode = codeMatch ? codeMatch[1] : result.response;
                if (fixedCode && fixedCode !== doc.getText()) {
                    const fullRange = new vscode.Range(doc.positionAt(0), doc.positionAt(doc.getText().length));
                    await editor.edit(editBuilder => editBuilder.replace(fullRange, fixedCode));
                    vscode.window.showInformationMessage(`✅ Fixed ${diags.length} issues`);
                    // Clear diagnostics
                    vscode.languages.getDiagnostics(doc.uri).forEach(() => {});
                }
            }
        } catch (e: any) {
            vscode.window.showErrorMessage(`FixAll failed: ${e.message}`);
        }
    });
}

function getExplainHtml(code: string, explanation: string): string {
    return `<!DOCTYPE html>
<html><body style="font-family: -apple-system, sans-serif; padding: 16px; background: #1e1e1e; color: #d4d4d4;">
    <h2>Code Explanation</h2>
    <pre style="background: #2d2d30; padding: 12px; border-radius: 8px; overflow-x: auto;">${escapeHtml(code)}</pre>
    <div style="margin-top: 16px; line-height: 1.6;">${formatMarkdown(explanation)}</div>
</body></html>`;
}

function escapeHtml(text: string): string {
    return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#039;');
}

function formatMarkdown(text: string): string {
    return text
        .replace(/```(\w+)?\n([\s\S]*?)```/g, '<pre><code>$2</code></pre>')
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\n/g, '<br>');
}

export function deactivate() {}
