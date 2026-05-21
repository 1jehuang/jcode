/**
 * Data models for JCode SDK
 */

export interface SessionInfo {
  session_id: string;
  user_id?: string;
  provider_id?: string;
  created_at: string;
  expires_at: string;
  is_active: boolean;
}

export interface ProviderInfo {
  id: string;
  name: string;
  provider_type: string;
  enabled: boolean;
  model?: string;
}

export interface MetricsData {
  timestamp: string;
  name: string;
  value: number;
  labels: Record<string, string>;
}

export interface CompletionContext {
  file_path: string;
  line: number;
  column: number;
  prefix: string;
  expected_type?: string;
  scope: string;
  parent_symbol?: string;
}

export interface CompletionCandidate {
  label: string;
  kind: string;
  detail?: string;
  documentation?: string;
  insert_text: string;
  rank_score: number;
  is_multiline?: boolean;
}

export interface EditOperation {
  operation_id: string;
  document_id: string;
  client_id: string;
  position: number;
  content: string;
  delete_length?: number;
  timestamp: string;
  version: string;
}

export interface CrdtDocumentInfo {
  document_id: string;
  title: string;
  content: string;
  version: string;
  last_modified: string;
  collaborators: string[];
}

export interface SsoUserInfo {
  sub: string;
  email?: string;
  email_verified?: boolean;
  name?: string;
  nickname?: string;
  picture?: string;
  tenant_id?: string;
  groups: string[];
  roles: string[];
  claims: Record<string, string>;
}

export interface SsoProviderConfig {
  id: string;
  name: string;
  provider_type: string;
  client_id: string;
  client_secret?: string;
  issuer_url?: string;
  discovery_url?: string;
  enabled?: boolean;
}

export interface ErrorResponse {
  error: string;
  message: string;
  code?: number;
  details?: Record<string, unknown>;
}

export interface CompletionRequest {
  content: string;
  language: string;
  cursor_line: number;
  cursor_column: number;
  file_path?: string;
}

export interface CompletionResponse {
  candidates: CompletionCandidate[];
}

export interface EditRequest {
  position: number;
  content: string;
  delete_length?: number;
}

export interface TokenResponse {
  access_token: string;
  token_type?: string;
  refresh_token?: string;
  id_token?: string;
  expires_in?: number;
  scope?: string;
}