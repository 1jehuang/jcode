import * as vscode from 'vscode';
import { CarpAiClient } from './client';
import { ChatViewProvider } from './chatView';
import { quickApplyEdit } from './applyEdit';

let client: CarpAiClient;
let chatProvider: ChatViewProvider;

export function activate(context: vscode.ExtensionContext) {
  console.log('CarpAI extension activated');

  // Initialize CarpAI client
  const config = vscode.workspace.getConfiguration('carpai');
  const serverUrl = config.get<string>('serverUrl', 'http://localhost:8080');
  const apiKey = config.get<string>('apiKey', '');
  const useGrpc = config.get<boolean>('useGrpc', true);

  client = new CarpAiClient(serverUrl, apiKey, useGrpc);

  // Register chat view provider
  chatProvider = new ChatViewProvider(context.extensionUri, client);
  const chatView = vscode.window.registerWebviewViewProvider(
    'carpai.chatView',
    chatProvider
  );
  context.subscriptions.push(chatView);

  // Register commands
  const chatCommand = vscode.commands.registerCommand('carpai.chat', async () => {
    await vscode.commands.executeCommand('workbench.view.extension.carpai-sidebar');
  });
  context.subscriptions.push(chatCommand);

  const inlineChatCommand = vscode.commands.registerCommand(
    'carpai.inlineChat',
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage('No active editor');
        return;
      }

      const selection = editor.selection;
      const selectedText = editor.document.getText(selection);

      const prompt = await vscode.window.showInputBox({
        prompt: 'Enter your question about the selected code:',
        placeHolder: 'e.g., Explain this code',
      });

      if (!prompt) {
        return;
      }

      const fullPrompt = `${prompt}\n\n\`\`\`\n${selectedText}\n\`\`\``;
      await handleChatRequest(fullPrompt);
    }
  );
  context.subscriptions.push(inlineChatCommand);

  const explainCommand = vscode.commands.registerCommand(
    'carpai.explainCode',
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage('No active editor');
        return;
      }

      const selection = editor.selection;
      const selectedText = editor.document.getText(selection);

      if (!selectedText) {
        vscode.window.showErrorMessage('No code selected');
        return;
      }

      await handleChatRequest(`Explain this code:\n\n\`\`\`\n${selectedText}\n\`\`\``);
    }
  );
  context.subscriptions.push(explainCommand);

  const refactorCommand = vscode.commands.registerCommand(
    'carpai.refactorCode',
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage('No active editor');
        return;
      }

      const selection = editor.selection;
      const selectedText = editor.document.getText(selection);

      if (!selectedText) {
        vscode.window.showErrorMessage('No code selected');
        return;
      }

      const suggestion = await vscode.window.showInputBox({
        prompt: 'How would you like to refactor this code?',
        placeHolder: 'e.g., Extract to function, Simplify logic',
      });

      if (!suggestion) {
        return;
      }

      await handleChatRequest(
        `Refactor this code: ${suggestion}\n\n\`\`\`\n${selectedText}\n\`\`\``
      );
    }
  );
  context.subscriptions.push(refactorCommand);

  // Listen for configuration changes
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('carpai')) {
        const newConfig = vscode.workspace.getConfiguration('carpai');
        client.updateConfig(
          newConfig.get<string>('serverUrl', 'http://localhost:8080'),
          newConfig.get<string>('apiKey', ''),
          newConfig.get<boolean>('useGrpc', true)
        );
      }
    })
  );

  vscode.window.showInformationMessage('CarpAI is ready! Use Ctrl+K (Cmd+K on Mac) for inline chat.');
}

async function handleChatRequest(prompt: string) {
  try {
    vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'CarpAI',
        cancellable: false,
      },
      async (progress) => {
        progress.report({ message: 'Thinking...' });

        const response = await client.complete(prompt);

        // Show response in chat view
        chatProvider.addMessage({
          role: 'user',
          content: prompt,
        });

        chatProvider.addMessage({
          role: 'assistant',
          content: response.text,
        });

        // Show apply button
        const editor = vscode.window.activeTextEditor;
        if (editor) {
          const applyAction = await vscode.window.showInformationMessage(
            'Response received',
            'Apply to Editor',
            'Dismiss'
          );

          if (applyAction === 'Apply to Editor') {
            await quickApplyEdit(response.text, editor);
          }
        }

        // Reveal chat view
        await vscode.commands.executeCommand('workbench.view.extension.carpai-sidebar');
      }
    );
  } catch (error) {
    vscode.window.showErrorMessage(
      `CarpAI error: ${error instanceof Error ? error.message : String(error)}`
    );
  }
}

export function deactivate() {
  client.dispose();
}
