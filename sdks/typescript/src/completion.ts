/**
 * Code completion client
 */

import axios, { AxiosInstance } from 'axios';
import { CompletionCandidate, CompletionContext, CompletionRequest, CompletionResponse } from './models';

export class CompletionClient {
  private axios: AxiosInstance;

  constructor(baseUrl: string, apiKey?: string, timeout: number = 30000) {
    this.axios = axios.create({
      baseURL: baseUrl,
      timeout,
      headers: {
        'Content-Type': 'application/json',
        ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      },
    });
  }

  async complete(
    content: string,
    language: string,
    cursorLine: number = 0,
    cursorColumn: number = 0,
    filePath?: string
  ): Promise<CompletionCandidate[]> {
    const request: CompletionRequest = {
      content,
      language,
      cursor_line: cursorLine,
      cursor_column: cursorColumn,
      file_path: filePath,
    };

    const response = await this.axios.post<CompletionResponse>(
      '/api/v1/completions',
      request
    );

    return response.data.candidates;
  }

  async completeStream(
    content: string,
    language: string,
    cursorLine: number = 0,
    cursorColumn: number = 0,
    filePath?: string,
    onCandidate?: (candidate: CompletionCandidate) => void
  ): Promise<CompletionCandidate[]> {
    const request: CompletionRequest = {
      content,
      language,
      cursor_line: cursorLine,
      cursor_column: cursorColumn,
      file_path: filePath,
    };

    const response = await this.axios.post('/api/v1/completions/stream', request, {
      responseType: 'stream',
    });

    const candidates: CompletionCandidate[] = [];

    for await (const chunk of response.data as AsyncIterable<string>) {
      const lines = chunk.split('\n').filter((line: string) => line.trim());
      for (const line of lines) {
        try {
          const candidate = JSON.parse(line);
          candidates.push(candidate);
          onCandidate?.(candidate);
        } catch {
          // Ignore invalid JSON lines
        }
      }
    }

    return candidates;
  }

  async getContext(
    content: string,
    cursorLine: number,
    cursorColumn: number
  ): Promise<CompletionContext> {
    const response = await this.axios.post<CompletionContext>(
      '/api/v1/completions/context',
      {
        content,
        cursor_line: cursorLine,
        cursor_column: cursorColumn,
      }
    );

    return response.data;
  }

  async getStats(): Promise<Record<string, unknown>> {
    const response = await this.axios.get('/api/v1/completions/stats');
    return response.data;
  }
}