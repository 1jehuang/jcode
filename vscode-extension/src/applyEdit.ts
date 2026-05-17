import * as vscode from 'vscode';

export interface TextEdit {
  filePath: string;
  oldText: string;
  newText: string;
}

/**
 * Parse AI response to extract code edits
 * Supports formats:
 * - Markdown code blocks with file path comments
 * - Unified diff format
 * - Direct replacement hints
 */
export function parseEdits(response: string): TextEdit[] {
  const edits: TextEdit[] = [];

  // Pattern 1: ```lang filepath:xxx ... ```
  const codeBlockPattern = /```(\w+)?\s*(?:filepath:\s*([^\s]+))?\n([\s\S]*?)```/g;
  let match;

  while ((match = codeBlockPattern.exec(response)) !== null) {
    const [, lang, filePath, code] = match;
    if (filePath && code) {
      edits.push({
        filePath,
        oldText: '', // Will be filled by comparing with actual file
        newText: code.trim(),
      });
    }
  }

  // Pattern 2: Diff format @@ ... @@
  const diffPattern = /--- a\/([^\n]+)\n\+\+\+ b\/[^\n]+\n@@ -\d+,\d+ \+\d+,\d+ @@\n([\s\S]*?)(?=\n---|\n$|$)/g;
  while ((match = diffPattern.exec(response)) !== null) {
    const [, filePath, diff] = match;
    edits.push({
      filePath,
      oldText: '',
      newText: diff,
    });
  }

  return edits;
}

/**
 * Apply edits to workspace files with preview
 */
export async function applyEditsWithPreview(edits: TextEdit[]): Promise<void> {
  if (edits.length === 0) {
    vscode.window.showInformationMessage('No edits to apply');
    return;
  }

  const editGroup = new vscode.WorkspaceEdit();

  for (const edit of edits) {
    const uri = vscode.Uri.file(edit.filePath);

    try {
      const document = await vscode.workspace.openTextDocument(uri);
      const fullRange = new vscode.Range(
        document.positionAt(0),
        document.positionAt(document.getText().length)
      );

      // If oldText is empty, replace entire file
      // Otherwise, find and replace specific section
      if (edit.oldText) {
        const startIdx = document.getText().indexOf(edit.oldText);
        if (startIdx === -1) {
          vscode.window.showWarningMessage(`Could not find original text in ${edit.filePath}`);
          continue;
        }

        const startPos = document.positionAt(startIdx);
        const endPos = document.positionAt(startIdx + edit.oldText.length);
        const range = new vscode.Range(startPos, endPos);

        editGroup.replace(uri, range, edit.newText);
      } else {
        // Replace entire file
        editGroup.replace(uri, fullRange, edit.newText);
      }
    } catch (error) {
      vscode.window.showErrorMessage(`Failed to process ${edit.filePath}: ${error}`);
    }
  }

  // Show preview before applying
  const confirm = await vscode.window.showInformationMessage(
    `Apply ${edits.length} edit(s) to ${new Set(edits.map(e => e.filePath)).size} file(s)?`,
    { modal: true },
    'Apply',
    'Cancel'
  );

  if (confirm === 'Apply') {
    const success = await vscode.workspace.applyEdit(editGroup);
    if (success) {
      vscode.window.showInformationMessage('Edits applied successfully');

      // Save all modified documents
      await Promise.all(
        edits.map(async (edit) => {
          const doc = await vscode.workspace.openTextDocument(edit.filePath);
          await doc.save();
        })
      );
    } else {
      vscode.window.showErrorMessage('Failed to apply edits');
    }
  }
}

/**
 * Quick apply: Apply first code block from AI response
 */
export async function quickApplyEdit(response: string, activeEditor?: vscode.TextEditor): Promise<void> {
  if (!activeEditor) {
    vscode.window.showErrorMessage('No active editor');
    return;
  }

  const edits = parseEdits(response);

  if (edits.length > 0) {
    await applyEditsWithPreview(edits);
  } else {
    // Try to extract any code block
    const codeMatch = response.match(/```[\w]*\n([\s\S]*?)```/);
    if (codeMatch) {
      const code = codeMatch[1].trim();
      const document = activeEditor.document;
      const selection = activeEditor.selection;

      const edit = new vscode.WorkspaceEdit();
      edit.replace(document.uri, selection, code);

      const confirm = await vscode.window.showInformationMessage(
        'Replace selected code with AI suggestion?',
        { modal: true },
        'Apply',
        'Cancel'
      );

      if (confirm === 'Apply') {
        await vscode.workspace.applyEdit(edit);
      }
    } else {
      vscode.window.showInformationMessage('No code blocks found in response');
    }
  }
}
