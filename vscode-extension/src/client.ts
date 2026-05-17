import * as vscode from 'vscode';
import { CarpAiGrpcClient, promptToGrpcRequest } from './grpcClient';

export interface CompletionRequest {
  prompt: string;
  model?: string;
  max_tokens?: number;
  temperature?: number;
}

export interface CompletionResponse {
  text: string;
  request_id: string;
  model: string;
  usage: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  latency_ms: number;
  cached: boolean;
}

export class CarpAiClient {
  private serverUrl: string;
  private apiKey: string;
  private grpcClient?: CarpAiGrpcClient;
  private useGrpc: boolean;

  constructor(serverUrl: string, apiKey: string = '', useGrpc: boolean = true) {
    this.serverUrl = serverUrl;
    this.apiKey = apiKey;
    this.useGrpc = useGrpc;

    if (useGrpc) {
      const grpcAddress = serverUrl.replace('http://', '').replace('https://', '');
      this.grpcClient = new CarpAiGrpcClient(grpcAddress || 'localhost:50051');
    }
  }

  updateConfig(serverUrl: string, apiKey: string, useGrpc?: boolean) {
    this.serverUrl = serverUrl;
    this.apiKey = apiKey;
    if (useGrpc !== undefined) {
      this.useGrpc = useGrpc;
    }

    if (this.useGrpc && !this.grpcClient) {
      const grpcAddress = serverUrl.replace('http://', '').replace('https://', '');
      this.grpcClient = new CarpAiGrpcClient(grpcAddress || 'localhost:50051');
    }
  }

  async complete(prompt: string): Promise<CompletionResponse> {
    // Use gRPC if enabled and available
    if (this.useGrpc && this.grpcClient) {
      try {
        const grpcRequest = promptToGrpcRequest(prompt);
        const grpcResponse = await this.grpcClient.chat(grpcRequest);

        return {
          text: grpcResponse.content,
          request_id: grpcResponse.id,
          model: grpcResponse.model,
          usage: grpcResponse.usage || {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
          },
          latency_ms: 0,
          cached: false,
        };
      } catch (grpcError) {
        console.warn('gRPC failed, falling back to HTTP:', grpcError);
        // Fall back to HTTP if gRPC fails
      }
    }

    // HTTP fallback
    const request: CompletionRequest = {
      prompt,
      max_tokens: 500,
      temperature: 0.7,
    };

    try {
      const response = await fetch(`${this.serverUrl}/v1/completions`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(this.apiKey && { Authorization: `Bearer ${this.apiKey}` }),
        },
        body: JSON.stringify(request),
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Server error (${response.status}): ${errorText}`);
      }

      const data = await response.json();
      return data as CompletionResponse;
    } catch (error) {
      if (error instanceof TypeError && error.message.includes('fetch')) {
        throw new Error(
          `Cannot connect to CarpAI server at ${this.serverUrl}. Is the server running?`
        );
      }
      throw error;
    }
  }

  async streamComplete(
    prompt: string,
    onChunk: (chunk: string) => void
  ): Promise<void> {
    const request: CompletionRequest = {
      prompt,
      max_tokens: 500,
      temperature: 0.7,
    };

    try {
      const response = await fetch(`${this.serverUrl}/v1/completions/stream`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(this.apiKey && { Authorization: `Bearer ${this.apiKey}` }),
        },
        body: JSON.stringify(request),
      });

      if (!response.ok) {
        throw new Error(`Server error: ${response.status}`);
      }

      const reader = response.body?.getReader();
      if (!reader) {
        throw new Error('No response body');
      }

      const decoder = new TextDecoder();
      let buffer = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() || '';

        for (const line of lines) {
          if (line.startsWith('data: ')) {
            const data = line.slice(6);
            if (data === '[DONE]') {
              return;
            }
            try {
              const chunk = JSON.parse(data);
              if (chunk.text) {
                onChunk(chunk.text);
              }
            } catch (e) {
              console.error('Failed to parse SSE chunk:', e);
            }
          }
        }
      }
    } catch (error) {
      throw error;
    }
  }

  dispose() {
    if (this.grpcClient) {
      this.grpcClient.close();
    }
  }
}
