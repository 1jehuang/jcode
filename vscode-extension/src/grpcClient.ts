import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import * as path from 'path';
import * as vscode from 'vscode';

// Load protobuf definitions
const PROTO_PATH = path.join(__dirname, '../../proto/jcode.proto');
const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
  keepCase: true,
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true,
});

const jcodeProto = grpc.loadPackageDefinition(packageDefinition) as any;

export interface GrpcChatRequest {
  session_id: string;
  tenant_id: string;
  messages: Array<{ role: string; content: string }>;
  model?: string;
  temperature?: number;
  max_tokens?: number;
}

export interface GrpcChatResponse {
  id: string;
  model: string;
  content: string;
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

export class CarpAiGrpcClient {
  private client: any;
  private serverAddress: string;
  private credentials: grpc.ChannelCredentials;

  constructor(serverAddress: string = 'localhost:50051', useTls: boolean = false) {
    this.serverAddress = serverAddress;
    this.credentials = useTls ? grpc.credentials.createSsl() : grpc.credentials.createInsecure();
    this.client = new jcodeProto.ChatService(
      serverAddress,
      this.credentials
    );
  }

  /**
   * Send chat request via gRPC (unary call)
   */
  async chat(request: GrpcChatRequest): Promise<GrpcChatResponse> {
    return new Promise((resolve, reject) => {
      this.client.Chat(
        request,
        { deadline: Date.now() + 60000 }, // 60s timeout
        (error: any, response: GrpcChatResponse) => {
          if (error) {
            reject(new Error(`gRPC error: ${error.message}`));
          } else {
            resolve(response);
          }
        }
      );
    });
  }

  /**
   * Stream chat response via gRPC (server streaming)
   */
  streamChat(
    request: GrpcChatRequest,
    onChunk: (chunk: string) => void,
    onComplete: () => void,
    onError: (error: Error) => void
  ): grpc.ClientReadableStream<any> {
    const call = this.client.ChatStream(request);

    let fullContent = '';

    call.on('data', (response: any) => {
      if (response.content) {
        fullContent += response.content;
        onChunk(response.content);
      }
    });

    call.on('end', () => {
      onComplete();
    });

    call.on('error', (error: any) => {
      onError(new Error(`Stream error: ${error.message}`));
    });

    return call;
  }

  /**
   * Cancel ongoing chat request
   */
  async cancelChat(sessionId: string, tenantId: string = ''): Promise<boolean> {
    return new Promise((resolve, reject) => {
      this.client.CancelChat(
        { session_id: sessionId, tenant_id: tenantId },
        (error: any, response: any) => {
          if (error) {
            reject(error);
          } else {
            resolve(response.success);
          }
        }
      );
    });
  }

  /**
   * Health check
   */
  async healthCheck(): Promise<boolean> {
    return new Promise((resolve) => {
      const deadline = new Date();
      deadline.setSeconds(deadline.getSeconds() + 5);

      this.client.waitForReady(deadline, (error?: Error) => {
        resolve(!error);
      });
    });
  }

  /**
   * Close gRPC channel
   */
  close(): void {
    this.client.close();
  }
}

/**
 * Convert simple prompt to gRPC ChatRequest format
 */
export function promptToGrpcRequest(
  prompt: string,
  sessionId: string = '',
  model: string = 'gpt-4',
  temperature: number = 0.7,
  maxTokens: number = 500
): GrpcChatRequest {
  return {
    session_id: sessionId,
    tenant_id: '',
    messages: [
      {
        role: 'user',
        content: prompt,
      },
    ],
    model,
    temperature,
    max_tokens: maxTokens,
  };
}
