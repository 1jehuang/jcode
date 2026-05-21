import axios from 'axios';
import * as vscode from 'vscode';

export interface CompletionRequest {
    file_path: string;
    content: string;
    line: number;
    character: number;
}

export interface CompletionItem {
    label: string;
    kind: number;
    documentation?: string;
    insert_text?: string;
    text_edit?: {
        range: {
            start: { line: number; character: number };
            end: { line: number; character: number };
        };
        new_text: string;
    };
}

export interface CompletionResponse {
    items: CompletionItem[];
}

export interface ChatRequest {
    message: string;
    context?: string;
}

export interface ChatResponse {
    response: string;
    streaming?: boolean;
}

export interface ReviewRequest {
    file_path: string;
    content: string;
}

export interface ReviewResult {
    issues: ReviewIssue[];
}

export interface ReviewIssue {
    severity: 'error' | 'warning' | 'info';
    message: string;
    line: number;
    column: number;
}

export class CarpaiClient {
    private baseUrl: string;
    private apiKey?: string;

    constructor() {
        const config = vscode.workspace.getConfiguration('carpai');
        this.baseUrl = config.get('server.url', 'http://localhost:8080');
    }

    async getCompletions(request: CompletionRequest): Promise<CompletionResponse> {
        try {
            const response = await axios.post<CompletionResponse>(
                `${this.baseUrl}/api/v1/completions`,
                request,
                { timeout: 10000 }
            );
            return response.data;
        } catch (error) {
            console.error('CarpAI completion error:', error);
            return { items: [] };
        }
    }

    async chat(request: ChatRequest): Promise<ChatResponse> {
        try {
            const response = await axios.post<ChatResponse>(
                `${this.baseUrl}/api/v1/chat`,
                request,
                { timeout: 30000 }
            );
            return response.data;
        } catch (error) {
            console.error('CarpAI chat error:', error);
            return { response: 'Error connecting to CarpAI server' };
        }
    }

    async review(request: ReviewRequest): Promise<ReviewResult> {
        try {
            const response = await axios.post<ReviewResult>(
                `${this.baseUrl}/api/v1/review`,
                request,
                { timeout: 15000 }
            );
            return response.data;
        } catch (error) {
            console.error('CarpAI review error:', error);
            return { issues: [] };
        }
    }

    async explainCode(code: string): Promise<string> {
        try {
            const response = await axios.post<{ explanation: string }>(
                `${this.baseUrl}/api/v1/explain`,
                { code },
                { timeout: 20000 }
            );
            return response.data.explanation;
        } catch (error) {
            console.error('CarpAI explain error:', error);
            return 'Error connecting to CarpAI server';
        }
    }

    async refactorCode(code: string, instructions: string): Promise<string> {
        try {
            const response = await axios.post<{ refactored: string }>(
                `${this.baseUrl}/api/v1/refactor`,
                { code, instructions },
                { timeout: 30000 }
            );
            return response.data.refactored;
        } catch (error) {
            console.error('CarpAI refactor error:', error);
            return 'Error connecting to CarpAI server';
        }
    }

    async generateTests(code: string): Promise<string> {
        try {
            const response = await axios.post<{ tests: string }>(
                `${this.baseUrl}/api/v1/generate-tests`,
                { code },
                { timeout: 30000 }
            );
            return response.data.tests;
        } catch (error) {
            console.error('CarpAI generate tests error:', error);
            return 'Error connecting to CarpAI server';
        }
    }

    async healthCheck(): Promise<boolean> {
        try {
            const response = await axios.get(`${this.baseUrl}/health`, { timeout: 5000 });
            return response.status === 200;
        } catch {
            return false;
        }
    }
}