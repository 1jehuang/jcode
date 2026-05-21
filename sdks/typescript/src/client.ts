/**
 * Main JCode client
 */

import axios, { AxiosInstance } from 'axios';
import { CompletionClient } from './completion';
import { CrdtClient } from './crdt';
import { SsoClient } from './sso';

export class JCodeClient {
  public completion: CompletionClient;
  public crdt: CrdtClient;
  public sso: SsoClient;

  private axios: AxiosInstance;

  constructor(baseUrl: string = 'http://localhost:8080', apiKey?: string, timeout: number = 30000) {
    this.axios = axios.create({
      baseURL: baseUrl,
      timeout,
      headers: {
        'Content-Type': 'application/json',
        ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      },
    });

    this.completion = new CompletionClient(baseUrl, apiKey, timeout);
    this.crdt = new CrdtClient(baseUrl, apiKey, timeout);
    this.sso = new SsoClient(baseUrl, apiKey, timeout);
  }

  async healthCheck(): Promise<Record<string, unknown>> {
    const response = await this.axios.get('/health');
    return response.data;
  }

  async getVersion(): Promise<Record<string, unknown>> {
    const response = await this.axios.get('/version');
    return response.data;
  }

  async complete(
    content: string,
    language: string,
    cursorLine: number = 0,
    cursorColumn: number = 0,
    filePath?: string
  ) {
    return this.completion.complete(content, language, cursorLine, cursorColumn, filePath);
  }

  close(): void {
    this.crdt.closeWebSocket();
  }
}