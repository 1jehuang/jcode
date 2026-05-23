import * as vscode from 'vscode';
import { CarpaiClient } from './carpaiClient';

/**
 * Inline Completion Provider (ghost text)
 * 
 * Provides real-time AI code suggestions as ghost text,
 * similar to Cursor's Tab completion and GitHub Copilot.
 * 
 * Features:
 * - Automatic trigger on typing
 * - Context-aware suggestions
 * - Tab to accept
 * - Escape to dismiss
 */
export class CarpaiInlineCompletionProvider implements vscode.InlineCompletionItemProvider {
    private client: CarpaiClient;
    private debounceTimer: ReturnType<typeof setTimeout> | undefined;
    private lastRequestId = 0;

    constructor(client: CarpaiClient) {
        this.client = client;
    }

    async provideInlineCompletionItems(
        document: vscode.TextDocument,
        position: vscode.Position,
        context: vscode.InlineCompletionContext,
        token: vscode.CancellationToken
    ): Promise<vscode.InlineCompletionItem[] | undefined> {
        // Don't trigger on manual invocations if no trigger reason
        if (context.triggerKind === vscode.InlineCompletionTriggerKind.Invoke) {
            // Allow manual trigger
        }

        const config = vscode.workspace.getConfiguration('carpai');
        if (!config.get('inlineCompletion.enabled', true)) {
            return undefined;
        }

        // Check debounce
        const debounceMs = config.get('inlineCompletion.debounceMs', 150);
        await this.debounce(debounceMs);

        // Generate a request ID to handle out-of-order responses
        const requestId = ++this.lastRequestId;

        // Get context: current line + surrounding code
        const linePrefix = document.lineAt(position.line).text.substring(0, position.character);
        const lineSuffix = document.lineAt(position.line).text.substring(position.character);
        
        // Build context window (200 lines before, 50 after)
        const contextStart = Math.max(0, position.line - 200);
        const contextEnd = Math.min(document.lineCount - 1, position.line + 50);
        const contextLines: string[] = [];
        for (let i = contextStart; i <= contextEnd; i++) {
            contextLines.push(document.lineAt(i).text);
        }

        // Determine language for better suggestions
        const language = document.languageId;

        try {
            const response = await this.client.getInlineCompletions({
                file_path: document.uri.fsPath,
                content: document.getText(),
                line: position.line,
                character: position.character,
                line_prefix: linePrefix,
                line_suffix: lineSuffix,
                context_window: contextLines.join('\n'),
                language: language,
            });

            // Ignore stale responses
            if (requestId !== this.lastRequestId) {
                return undefined;
            }

            if (!response.completions || response.completions.length === 0) {
                return undefined;
            }

            return response.completions.map(comp => {
                const item = new vscode.InlineCompletionItem(
                    comp.text,
                    new vscode.Range(position, position)
                );
                // Store filter text for better matching
                item.filterText = comp.text.substring(0, Math.min(comp.text.length, 50));
                return item;
            });
        } catch {
            return undefined;
        }
    }

    private debounce(ms: number): Promise<void> {
        return new Promise(resolve => {
            if (this.debounceTimer) {
                clearTimeout(this.debounceTimer);
            }
            this.debounceTimer = setTimeout(resolve, ms);
        });
    }
}
