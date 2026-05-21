import * as vscode from 'vscode';
import { CarpaiClient, CompletionItem as CarpaiCompletionItem } from './carpaiClient';

export class CarpaiCompletionProvider implements vscode.CompletionItemProvider {
    private client: CarpaiClient;
    private enabled: boolean;

    constructor(client: CarpaiClient) {
        this.client = client;
        this.enabled = vscode.workspace.getConfiguration('carpai').get('completion.enabled', true);
        
        vscode.workspace.onDidChangeConfiguration((event) => {
            if (event.affectsConfiguration('carpai.completion.enabled')) {
                this.enabled = vscode.workspace.getConfiguration('carpai').get('completion.enabled', true);
            }
        });
    }

    async provideCompletionItems(
        document: vscode.TextDocument,
        position: vscode.Position,
        token: vscode.CancellationToken
    ): Promise<vscode.CompletionItem[] | vscode.CompletionList<vscode.CompletionItem> | undefined> {
        if (!this.enabled) {
            return undefined;
        }

        try {
            const file_path = document.uri.fsPath;
            const content = document.getText();
            const line = position.line;
            const character = position.character;

            const response = await this.client.getCompletions({
                file_path,
                content,
                line,
                character,
            });

            return response.items.map(this.convertToVsCodeItem);
        } catch (error) {
            console.error('CarpAI completion provider error:', error);
            return undefined;
        }
    }

    private convertToVsCodeItem(item: CarpaiCompletionItem): vscode.CompletionItem {
        const vsItem = new vscode.CompletionItem(item.label);
        
        vsItem.kind = this.convertKind(item.kind);
        
        if (item.documentation) {
            vsItem.documentation = new vscode.MarkdownString(item.documentation);
        }
        
        if (item.text_edit) {
            vsItem.textEdit = new vscode.TextEdit(
                new vscode.Range(
                    new vscode.Position(
                        item.text_edit.range.start.line,
                        item.text_edit.range.start.character
                    ),
                    new vscode.Position(
                        item.text_edit.range.end.line,
                        item.text_edit.range.end.character
                    )
                ),
                item.text_edit.new_text
            );
        } else if (item.insert_text) {
            vsItem.insertText = item.insert_text;
        }
        
        vsItem.source = 'CarpAI';
        
        return vsItem;
    }

    private convertKind(kind: number): vscode.CompletionItemKind {
        const kinds: Record<number, vscode.CompletionItemKind> = {
            1: vscode.CompletionItemKind.Text,
            2: vscode.CompletionItemKind.Method,
            3: vscode.CompletionItemKind.Function,
            4: vscode.CompletionItemKind.Constructor,
            5: vscode.CompletionItemKind.Field,
            6: vscode.CompletionItemKind.Variable,
            7: vscode.CompletionItemKind.Class,
            8: vscode.CompletionItemKind.Interface,
            9: vscode.CompletionItemKind.Module,
            10: vscode.CompletionItemKind.Property,
            11: vscode.CompletionItemKind.Unit,
            12: vscode.CompletionItemKind.Value,
            13: vscode.CompletionItemKind.Enum,
            14: vscode.CompletionItemKind.Keyword,
            15: vscode.CompletionItemKind.Snippet,
            16: vscode.CompletionItemKind.Color,
            17: vscode.CompletionItemKind.File,
            18: vscode.CompletionItemKind.Reference,
            19: vscode.CompletionItemKind.Folder,
            20: vscode.CompletionItemKind.EnumMember,
            21: vscode.CompletionItemKind.Constant,
            22: vscode.CompletionItemKind.Struct,
            23: vscode.CompletionItemKind.Event,
            24: vscode.CompletionItemKind.Operator,
            25: vscode.CompletionItemKind.TypeParameter,
        };
        
        return kinds[kind] || vscode.CompletionItemKind.Text;
    }
}