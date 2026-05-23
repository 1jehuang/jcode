import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { CarpaiClient } from './carpaiClient';

/**
 * MCP Config Provider
 * 
 * Syncs CarpAI MCP server configuration with VS Code's MCP integration.
 * Supports:
 * - Reading .jcode/mcp.json (CarpAI native)
 * - Reading .vscode/mcp.json (VSCode/Cursor compatible)
 * - Auto-detecting MCP servers from workspace config
 * - Generating .vscode/mcp.json from CarpAI config
 */
export class McpConfigProvider {
    private client: CarpaiClient;

    constructor(client: CarpaiClient) {
        this.client = client;
    }

    /**
     * Sync MCP configuration from workspace to CarpAI server.
     */
    public async syncMcpConfig(): Promise<void> {
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders) return;

        for (const folder of workspaceFolders) {
            await this.syncFolderMcp(folder.uri.fsPath);
        }
    }

    private async syncFolderMcp(folderPath: string): Promise<void> {
        const carpaiConfigPath = path.join(folderPath, '.jcode', 'mcp.json');
        const vscodeConfigPath = path.join(folderPath, '.vscode', 'mcp.json');
        const cursorConfigPath = path.join(folderPath, '.cursor', 'mcp.json');

        let config: any = null;

        // Load config from highest priority source
        if (fs.existsSync(vscodeConfigPath)) {
            config = JSON.parse(fs.readFileSync(vscodeConfigPath, 'utf-8'));
        } else if (fs.existsSync(cursurConfigPath)) {
            config = JSON.parse(fs.readFileSync(cursurConfigPath, 'utf-8'));
        } else if (fs.existsSync(carpaiConfigPath)) {
            config = JSON.parse(fs.readFileSync(carpaiConfigPath, 'utf-8'));
        }

        if (!config || !config.servers) return;

        // Register MCP servers with CarpAI backend
        for (const [name, serverConfig] of Object.entries(config.servers)) {
            const sc = serverConfig as any;
            const command = sc.command || sc.type === 'sse' ? undefined : undefined;
            const args = sc.args || [];
            const env = sc.env || {};

            if (command) {
                await this.client.registerMcpServer({
                    name,
                    command,
                    args,
                    env,
                });
            }
        }

        // If .vscode/mcp.json doesn't exist but .jcode/mcp.json does, generate it
        if (fs.existsSync(carpaiConfigPath) && !fs.existsSync(vscodeConfigPath) && !fs.existsSync(cursorConfigPath)) {
            this.generateVscodeConfig(folderPath, carpaiConfigPath);
        }
    }

    private generateVscodeConfig(folderPath: string, carpaiConfigPath: string): void {
        try {
            const carpaiConfig = JSON.parse(fs.readFileSync(carpaiConfigPath, 'utf-8'));
            if (!carpaiConfig.servers) return;

            const vscodeConfig: any = { servers: {} };
            for (const [name, serverConfig] of Object.entries(carpaiConfig.servers)) {
                const sc = serverConfig as any;
                vscodeConfig.servers[name] = {
                    type: 'stdio',
                    command: sc.command,
                    args: sc.args || [],
                    env: sc.env || {},
                };
            }

            const vscodeDir = path.join(folderPath, '.vscode');
            if (!fs.existsSync(vscodeDir)) {
                fs.mkdirSync(vscodeDir, { recursive: true });
            }
            fs.writeFileSync(
                path.join(vscodeDir, 'mcp.json'),
                JSON.stringify(vscodeConfig, null, 2)
            );
        } catch (e) {
            console.error('Failed to generate VSCode MCP config:', e);
        }
    }

    /**
     * Detect VSCode/Cursor MCP config and add status bar display.
     */
    public async getStatus(): Promise<string> {
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders) return 'No workspace open';

        let configCount = 0;
        let serverCount = 0;

        for (const folder of workspaceFolders) {
            const carpaiPath = path.join(folder.uri.fsPath, '.jcode', 'mcp.json');
            const vscodePath = path.join(folder.uri.fsPath, '.vscode', 'mcp.json');
            const cursorPath = path.join(folder.uri.fsPath, '.cursor', 'mcp.json');

            if (fs.existsSync(carpaiPath)) { configCount++; }
            if (fs.existsSync(vscodePath)) { configCount++; }
            if (fs.existsSync(cursorPath)) { configCount++; }

            for (const p of [carpaiPath, vscodePath, cursorPath]) {
                if (fs.existsSync(p)) {
                    try {
                        const config = JSON.parse(fs.readFileSync(p, 'utf-8'));
                        serverCount += Object.keys(config.servers || {}).length;
                    } catch {}
                }
            }
        }

        return `${configCount} configs, ${serverCount} servers`;
    }
}
