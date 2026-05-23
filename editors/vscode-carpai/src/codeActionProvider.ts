//! CarpAI CodeActionProvider — 为 VS Code 提供 LSP Code Actions
//! 对标 Cursor: 光灯泡💡 + QuickFix + 重构
//!
//! 功能:
//! - 在诊断位置显示灯泡图标
//! - QuickFix: 自动修复常见错误
//! - Refactor: 提取方法、重命名符号、移动符号
//! - FixAll: 一键修复所有可自动修复问题

import * as vscode from 'vscode';

/**
 * CodeAction 提供者 — 在编辑器中出现诊断时触发💡
 */
export class CarpaiCodeActionProvider implements vscode.CodeActionProvider<vscode.CodeAction> {
    public static readonly providedCodeActionKinds = [
        vscode.CodeActionKind.QuickFix,
        vscode.CodeActionKind.RefactorExtract,
        vscode.CodeActionKind.Refactor,
        vscode.CodeActionKind.SourceOrganizeImports,
        vscode.CodeActionKind.SourceFixAll,
    ];

    private serverUrl: string;

    constructor(serverUrl: string) {
        this.serverUrl = serverUrl;
    }

    /**
     * 提供 Code Actions — 在用户点击💡或按 Ctrl+. 时调用
     */
    async provideCodeActions(
        document: vscode.TextDocument,
        range: vscode.Range | vscode.Selection,
        context: vscode.CodeActionContext,
        _token: vscode.CancellationToken
    ): Promise<vscode.CodeAction[]> {
        const actions: vscode.CodeAction[] = [];

        // 1. 从上下文诊断生成 QuickFix
        for (const diagnostic of context.diagnostics) {
            const fixes = this.quickFixFromDiagnostic(diagnostic, document);
            actions.push(...fixes);
        }

        const line = range.start.line;
        const character = range.start.character;

        // 2. 从 CarpAI 后端获取 Code Actions
        const serverActions = await this.fetchServerCodeActions(document, line, character);
        actions.push(...serverActions);

        // 3. 本地重构操作 (多行选择时)
        if (!range.isEmpty && range.start.line !== range.end.line) {
            actions.push(this.createExtractMethodAction(document, range));
        }

        // 4. 重命名符号 (任何位置)
        actions.push(this.createRenameSymbolAction(document, line, character));

        // 5. FixAll (有诊断时)
        if (context.diagnostics.length > 0) {
            actions.push(this.createFixAllAction(document));
        }

        return actions;
    }

    /**
     * 从诊断生成 QuickFix
     */
    private quickFixFromDiagnostic(
        diagnostic: vscode.Diagnostic,
        document: vscode.TextDocument
    ): vscode.CodeAction[] {
        const fixes: vscode.CodeAction[] = [];
        const message = diagnostic.message.toLowerCase();

        // 根据错误类型生成对应的修复
        let fixTitle: string | null = null;
        let newText: string | null = null;

        if (message.includes('unused variable') || message.includes('unused import')) {
            const varName = this.extractNameFromDiagnostic(diagnostic.message);
            fixTitle = `Remove unused '${varName}'`;
            newText = '';
        } else if (message.includes('unused `Ok`') || message.includes('unused ok')) {
            fixTitle = 'Add let _ = ...';
            newText = 'let _ = ';
        } else if (message.includes('missing documentation')) {
            fixTitle = 'Add /// documentation';
            const lineText = document.lineAt(diagnostic.range.start.line).text;
            const indent = lineText.match(/^\s*/)?.[0] || '';
            newText = `${indent}/// TODO: Document this\n`;
        } else if (message.includes('cannot find') || message.includes('not found')) {
            fixTitle = 'Search for similar symbols...';
        }

        if (fixTitle) {
            const fix = new vscode.CodeAction(fixTitle, vscode.CodeActionKind.QuickFix);
            fix.diagnostics = [diagnostic];
            fix.isPreferred = true;

            if (newText !== null) {
                const edit = new vscode.WorkspaceEdit();
                // 在行首插入修复
                const insertPos = new vscode.Position(diagnostic.range.start.line, 0);
                edit.insert(document.uri, insertPos, newText);
                fix.edit = edit;
            }

            fixes.push(fix);
        }

        return fixes;
    }

    /**
     * 从后端获取 Code Actions
     */
    private async fetchServerCodeActions(
        document: vscode.TextDocument,
        line: number,
        character: number
    ): Promise<vscode.CodeAction[]> {
        try {
            const filePath = document.uri.fsPath;
            const response = await fetch(
                `${this.serverUrl}/api/lsp/codeAction`,
                {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        textDocument: { uri: document.uri.toString() },
                        range: {
                            start: { line, character },
                            end: { line, character }
                        },
                        context: { diagnostics: [] }
                    }),
                }
            );

            if (!response.ok) return [];

            const serverActions: any[] = await response.json();
            return serverActions.map((sa: any) => {
                const kind = this.mapCodeActionKind(sa.kind);
                const action = new vscode.CodeAction(sa.title, kind);

                if (sa.command) {
                    action.command = {
                        title: sa.command.title || sa.title,
                        command: sa.command.command,
                        arguments: sa.command.arguments || [],
                    };
                }

                if (sa.isPreferred) {
                    action.isPreferred = true;
                }

                return action;
            });
        } catch {
            return [];
        }
    }

    /**
     * 创建提取方法操作
     */
    private createExtractMethodAction(
        document: vscode.TextDocument,
        range: vscode.Range
    ): vscode.CodeAction {
        const action = new vscode.CodeAction(
            'Extract to function...',
            vscode.CodeActionKind.RefactorExtract
        );

        action.command = {
            title: 'Extract Method',
            command: 'carpai.refactor.extractMethod',
            arguments: [
                document.uri.fsPath,
                range.start.line,
                range.end.line,
            ],
        };

        return action;
    }

    /**
     * 创建重命名符号操作
     */
    private createRenameSymbolAction(
        document: vscode.TextDocument,
        line: number,
        character: number
    ): vscode.CodeAction {
        const action = new vscode.CodeAction(
            'Rename symbol...',
            vscode.CodeActionKind.Refactor
        );

        action.command = {
            title: 'Rename Symbol',
            command: 'carpai.refactor.rename',
            arguments: [
                document.uri.fsPath,
                line,
                character,
            ],
        };

        return action;
    }

    /**
     * 创建 FixAll 操作
     */
    private createFixAllAction(
        document: vscode.TextDocument
    ): vscode.CodeAction {
        const action = new vscode.CodeAction(
            'Fix all auto-fixable issues',
            vscode.CodeActionKind.SourceFixAll
        );

        action.command = {
            title: 'Fix All',
            command: 'carpai.fixAll',
            arguments: [document.uri.fsPath],
        };

        return action;
    }

    /**
     * 映射 LSP kind 到 VS Code CodeActionKind
     */
    private mapCodeActionKind(kind: string | null): vscode.CodeActionKind {
        if (!kind) return vscode.CodeActionKind.QuickFix;

       	switch (kind) {
            case 'quickfix': return vscode.CodeActionKind.QuickFix;
            case 'refactor.extract.function': return vscode.CodeActionKind.RefactorExtract;
            case 'refactor.rename': return vscode.CodeActionKind.Refactor;
            case 'refactor.move': return vscode.CodeActionKind.Refactor;
            case 'refactor': return vscode.CodeActionKind.Refactor;
            case 'source.organizeImports': return vscode.CodeActionKind.SourceOrganizeImports;
            case 'source.fixAll': return vscode.CodeActionKind.SourceFixAll;
            default: return vscode.CodeActionKind.QuickFix;
        }
    }

    /**
     * 从诊断消息中提取变量名
     */
    private extractNameFromDiagnostic(message: string): string {
        // 模式: "unused variable `xxx`" 或 "unused import `xxx`"
        const match = message.match(/`([^`]+)`/);
        return match ? match[1] : 'item';
    }
}
