/**
 * CRDT collaborative editing client
 */

import axios, { AxiosInstance } from 'axios';
import WebSocket from 'ws';
import { CrdtDocumentInfo, EditOperation, EditRequest } from './models';

export class CrdtClient {
  private axios: AxiosInstance;
  private wsUrl: string;
  private wsConnection?: WebSocket;
  private onEditCallback?: (operation: EditOperation) => void;

  constructor(baseUrl: string, apiKey?: string, timeout: number = 30000) {
    this.axios = axios.create({
      baseURL: baseUrl,
      timeout,
      headers: {
        'Content-Type': 'application/json',
        ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      },
    });

    this.wsUrl = baseUrl.replace('http://', 'ws://').replace('https://', 'wss://');
  }

  async createDocument(title: string, content: string = ''): Promise<CrdtDocumentInfo> {
    const response = await this.axios.post<CrdtDocumentInfo>('/api/v1/crdt/documents', {
      title,
      content,
    });

    return response.data;
  }

  async getDocument(documentId: string): Promise<CrdtDocumentInfo> {
    const response = await this.axios.get<CrdtDocumentInfo>(
      `/api/v1/crdt/documents/${documentId}`
    );

    return response.data;
  }

  async updateDocument(
    documentId: string,
    title?: string
  ): Promise<CrdtDocumentInfo> {
    const response = await this.axios.patch<CrdtDocumentInfo>(
      `/api/v1/crdt/documents/${documentId}`,
      title ? { title } : {}
    );

    return response.data;
  }

  async deleteDocument(documentId: string): Promise<void> {
    await this.axios.delete(`/api/v1/crdt/documents/${documentId}`);
  }

  async listDocuments(): Promise<CrdtDocumentInfo[]> {
    const response = await this.axios.get<CrdtDocumentInfo[]>('/api/v1/crdt/documents');
    return response.data;
  }

  async applyEdit(
    documentId: string,
    position: number,
    content: string,
    deleteLength: number = 0
  ): Promise<EditOperation> {
    const request: EditRequest = {
      position,
      content,
      delete_length: deleteLength,
    };

    const response = await this.axios.post<EditOperation>(
      `/api/v1/crdt/documents/${documentId}/edit`,
      request
    );

    return response.data;
  }

  connectWebSocket(
    documentId: string,
    clientId: string,
    onEdit?: (operation: EditOperation) => void
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      this.onEditCallback = onEdit;
      const wsUrl = `${this.wsUrl}/api/v1/crdt/ws/${documentId}?client_id=${clientId}`;

      this.wsConnection = new WebSocket(wsUrl);

      this.wsConnection.on('open', () => {
        resolve();
      });

      this.wsConnection.on('message', (data: WebSocket.Data) => {
        try {
          const message = JSON.parse(data.toString());
          if (message.type === 'edit' && message.payload) {
            const operation: EditOperation = message.payload;
            this.onEditCallback?.(operation);
          }
        } catch {
          // Ignore invalid messages
        }
      });

      this.wsConnection.on('error', (error) => {
        reject(error);
      });

      this.wsConnection.on('close', () => {
        this.wsConnection = undefined;
      });
    });
  }

  sendEdit(
    documentId: string,
    position: number,
    content: string,
    deleteLength: number = 0
  ): void {
    if (!this.wsConnection) {
      throw new Error('WebSocket not connected');
    }

    const message = JSON.stringify({
      type: 'edit',
      payload: {
        document_id: documentId,
        position,
        content,
        delete_length: deleteLength,
      },
    });

    this.wsConnection.send(message);
  }

  closeWebSocket(): void {
    this.wsConnection?.close();
    this.wsConnection = undefined;
    this.onEditCallback = undefined;
  }
}