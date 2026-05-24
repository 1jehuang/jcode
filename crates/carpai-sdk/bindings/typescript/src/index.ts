// TypeScript bindings for @carpai/sdk

export interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
}

export interface SessionResponse {
  id: string;
  title?: string;
  state: string;
  message_count: number;
  created_at: string;
}

export function init(): void;
export function version(): string;
export function chat_completion(
  serverUrl: string,
  apiKey: string,
  messages: ChatMessage[],
  model?: string
): Promise<string>;
export function create_session(
  serverUrl: string,
  apiKey: string,
  title?: string
): Promise<string>;
export function append_message(
  serverUrl: string,
  apiKey: string,
  sessionId: string,
  role: string,
  content: string
): Promise<string>;
