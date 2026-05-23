import { CarpaiClient } from './carpaiClient';

// Extend CarpaiClient with inline completion and MCP server registration

export interface InlineCompletionRequest {
    file_path: string;
    content: string;
    line: number;
    character: number;
    line_prefix: string;
    line_suffix: string;
    context_window: string;
    language: string;
}

export interface InlineCompletionResult {
    text: string;
}

export interface InlineCompletionResponse {
    completions: InlineCompletionResult[];
}

export interface McpServerRegistration {
    name: string;
    command: string;
    args: string[];
    env: Record<string, string>;
}

declare module './carpaiClient' {
    interface CarpaiClient {
        getInlineCompletions(request: InlineCompletionRequest): Promise<InlineCompletionResponse>;
        registerMcpServer(config: McpServerRegistration): Promise<boolean>;
        reloadConfig(): void;
    }
}

// Patch CarpaiClient prototype
const originalGetCompletions = (CarpaiClient.prototype as any).getCompletions;

(CarpaiClient.prototype as any).getInlineCompletions = async function(
    request: InlineCompletionRequest
): Promise<InlineCompletionResponse> {
    try {
        const axios = require('axios');
        const baseUrl = (this as any).baseUrl;
        const response = await axios.post(
            `${baseUrl}/api/v1/inline-completions`,
            request,
            { timeout: 3000 }
        );
        return response.data;
    } catch {
        return { completions: [] };
    }
};

(CarpaiClient.prototype as any).registerMcpServer = async function(
    config: McpServerRegistration
): Promise<boolean> {
    try {
        const axios = require('axios');
        const baseUrl = (this as any).baseUrl;
        const response = await axios.post(
            `${baseUrl}/api/v1/mcp/register`,
            config,
            { timeout: 5000 }
        );
        return response.status === 200;
    } catch {
        return false;
    }
};

(CarpaiClient.prototype as any).reloadConfig = function(): void {
    const vscode = require('vscode');
    const config = vscode.workspace.getConfiguration('carpai');
    (this as any).baseUrl = config.get('server.url', 'http://localhost:8080');
};

export {};
