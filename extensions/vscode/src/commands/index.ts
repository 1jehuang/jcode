import * as vscode from "vscode";

export function registerCommands(context: vscode.ExtensionContext) {
  // Open Chat command
  context.subscriptions.push(
    vscode.commands.registerCommand("carpai.chat", () => {
      vscode.commands.executeCommand("workbench.view.extension.carpai-sidebar");
    })
  );

  // Explain Code command
  context.subscriptions.push(
    vscode.commands.registerCommand("carpai.explain", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) return;
      const selection = editor.document.getText(editor.selection);
      vscode.window.showInformationMessage(`Explaining: ${selection.slice(0, 50)}...`);
      // TODO: Call carpai-sdk explain
    })
  );

  // Refactor command
  context.subscriptions.push(
    vscode.commands.registerCommand("carpai.refactor", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) return;
      const selection = editor.document.getText(editor.selection);
      vscode.window.showInformationMessage(`Refactoring: ${selection.slice(0, 50)}...`);
      // TODO: Call carpai-sdk refactor
    })
  );

  // Fix Bug command
  context.subscriptions.push(
    vscode.commands.registerCommand("carpai.fix", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) return;
      const selection = editor.document.getText(editor.selection);
      vscode.window.showInformationMessage(`Fixing: ${selection.slice(0, 50)}...`);
      // TODO: Call carpai-sdk fix
    })
  );
}
