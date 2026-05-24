// CarpAI VSCode Extension Entry Point
import * as vscode from "vscode";
import { ChatViewProvider } from "./ui/ChatViewProvider";
import { registerCommands } from "./commands";

export function activate(context: vscode.ExtensionContext) {
  console.log("CarpAI extension activated");

  // Register chat view provider
  const chatProvider = new ChatViewProvider(context.extensionUri);
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider("carpai.chatView", chatProvider)
  );

  // Register commands
  registerCommands(context);

  console.log("CarpAI extension ready");
}

export function deactivate() {
  console.log("CarpAI extension deactivated");
}
