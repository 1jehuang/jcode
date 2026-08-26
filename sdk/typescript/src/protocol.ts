/**
 * Wire types for the jcode harness API (protocol v1).
 *
 * Mirrors `crates/jcode-harness-api` exactly: request tags live under `req`,
 * event tags under `ev`, and every frame carries `v`. Keep this file in sync
 * with the Rust enums; `test/schema-parity.test.ts` fails the build if the
 * tag sets drift apart.
 */

export const API_VERSION_MAJOR = 1;
export const API_VERSION_MINOR = 0;

export type PermissionDecision = "allow" | "allow_always" | "deny";

export type ErrorCode =
  | "unsupported_version"
  | "unknown_request"
  | "unknown_session"
  | "invalid_request"
  | "internal";

export interface SessionInfo {
  session_id: string;
  working_dir?: string;
  title?: string;
  status: string;
  /** Approximate size of the stored transcript, in bytes. */
  transcript_bytes?: number;
  archived?: boolean;
  archived_at_ms?: number;
}

export interface ModelRouteInfo {
  model: string;
  provider: string;
  api_method: string;
  available: boolean;
  detail: string;
}

export interface TextMatch {
  path: string;
  line: number;
  column: number;
  preview: string;
}

export interface HistoryMessage {
  /** "user" | "assistant" | "tool" */
  role: string;
  content: string;
}

export type RenderedImageSource =
  | { kind: "user_input" }
  | { kind: "tool_result"; tool_name: string }
  | { kind: "other"; role: string };

export type RenderedImageAnchor =
  | { kind: "tool_call"; id: string }
  | { kind: "user_prompt"; ordinal: number };

export interface RenderedImage {
  media_type: string;
  data: string;
  label?: string;
  source: RenderedImageSource;
  anchor?: RenderedImageAnchor;
}

/** Base64 image attachment: [mediaType, base64Data]. */
export type ImageAttachment = [string, string];

export type ApiRequest =
  | { req: "hello"; min_version: number; max_version: number; client: string }
  | { req: "list_sessions"; include_archived?: boolean; limit?: number }
  | { req: "archive_session"; session_id: string }
  | { req: "restore_session"; session_id: string }
  | { req: "set_retention_policy"; archive_after_days?: number }
  | { req: "create_session"; working_dir?: string }
  | { req: "attach_session"; session_id: string }
  | { req: "fork_session"; session_id: string }
  | { req: "detach_session"; session_id: string }
  | {
      req: "send_message";
      session_id: string;
      content: string;
      images?: ImageAttachment[];
      no_reply?: boolean;
    }
  | { req: "cancel"; session_id: string }
  | {
      req: "soft_interrupt";
      session_id: string;
      content: string;
      urgent?: boolean;
    }
  | { req: "get_history"; session_id: string }
  | { req: "peek_session"; session_id: string; limit?: number }
  | { req: "clear"; session_id: string }
  | { req: "rewind"; session_id: string; message_index: number }
  | {
      req: "permission_response";
      session_id: string;
      request_id: string;
      decision: PermissionDecision;
    }
  | { req: "list_models"; session_id: string }
  | { req: "get_runtime_info"; session_id: string }
  | { req: "set_api_key"; provider: string; api_key: string }
  | { req: "clear_api_key"; provider: string }
  | { req: "read_file"; session_id: string; path: string; max_bytes?: number }
  | { req: "find_files"; session_id: string; query: string; limit?: number }
  | { req: "search_text"; session_id: string; query: string; path?: string; limit?: number }
  | { req: "file_status"; session_id: string; path: string }
  | { req: "set_model"; session_id: string; model: string }
  | { req: "set_reasoning_effort"; session_id: string; effort: string }
  | { req: "compact"; session_id: string }
  | { req: "rename_session"; session_id: string; title?: string }
  | { req: "rewind_undo"; session_id: string }
  | { req: "cancel_soft_interrupts"; session_id: string }
  | { req: "ping" };

export type ApiEvent =
  | { ev: "hello_ok"; version: number; server: string; capabilities?: string[] }
  | { ev: "ok" }
  | { ev: "error"; code: ErrorCode; message: string }
  | { ev: "sessions"; sessions: SessionInfo[] }
  | { ev: "attached"; session: SessionInfo }
  | { ev: "session_forked"; session: SessionInfo }
  | { ev: "history"; session_id: string; messages: HistoryMessage[]; images?: RenderedImage[] }
  | { ev: "pong" }
  | { ev: "text_delta"; session_id: string; text: string }
  | { ev: "reasoning_delta"; session_id: string; text: string }
  | { ev: "reasoning_done"; session_id: string; duration_secs?: number }
  | { ev: "tool_start"; session_id: string; call_id: string; name: string }
  | { ev: "tool_input_delta"; session_id: string; call_id: string; delta: string }
  | { ev: "tool_exec"; session_id: string; call_id: string; name: string }
  | {
      ev: "tool_done";
      session_id: string;
      call_id: string;
      name: string;
      output: string;
      error?: string;
    }
  | { ev: "side_pane_images"; session_id: string; images: RenderedImage[] }
  | {
      ev: "token_usage";
      session_id: string;
      input: number;
      output: number;
      cache_read_input?: number;
    }
  | { ev: "turn_done"; session_id: string }
  | {
      ev: "wake_requested";
      session_id: string;
      reason: string;
      notification: string;
    }
  | {
      ev: "background_progress";
      session_id: string;
      task_id: string;
      label: string;
      percent?: number;
      summary: string;
      done?: boolean;
    }
  | { ev: "message_accepted"; session_id: string }
  | {
      ev: "permission_request";
      session_id: string;
      request_id: string;
      tool_name: string;
      description: string;
    }
  | { ev: "session_status"; session_id: string; status: string }
  | { ev: "connection_phase"; session_id: string; phase: string }
  | {
      ev: "model_info";
      session_id: string;
      provider?: string;
      model?: string;
      reasoning_effort?: string;
    }
  | { ev: "models"; session_id: string; models: string[]; current?: string }
  | {
      ev: "runtime_info";
      session_id: string;
      provider?: string;
      model?: string;
      reasoning_effort?: string;
      routes: ModelRouteInfo[];
    }
  | { ev: "credential_updated"; provider: string; configured: boolean }
  | {
      ev: "file_content";
      session_id: string;
      path: string;
      content: string;
      size: number;
      truncated: boolean;
    }
  | { ev: "files"; session_id: string; paths: string[] }
  | { ev: "text_matches"; session_id: string; matches: TextMatch[] }
  | {
      ev: "file_status";
      session_id: string;
      path: string;
      exists: boolean;
      kind: string;
      size?: number;
      modified_ms?: number;
    }
  | { ev: "compacted"; session_id: string; message: string }
  | {
      ev: "session_renamed";
      session_id: string;
      title?: string;
      display_title: string;
    };

/**
 * An event kind this SDK does not know about.
 *
 * The harness may add events at any time within protocol v1, so one can arrive
 * at runtime. It is deliberately *not* part of `ApiEvent`: a member with
 * `ev: string` widens the discriminant, and TypeScript then refuses to narrow
 * `event.ev === "text_delta"` to the text-delta member, leaving every field
 * typed `unknown`. Forward compatibility is a runtime property, and paying for
 * it with the type safety of the ninety-nine percent case is a bad trade.
 *
 * Handle these with a `default` branch, or filter with `isKnownEvent`.
 */
export interface UnknownApiEvent {
  ev: string;
  [key: string]: unknown;
}

/** Any frame off the wire, known or not. Narrow with `isKnownEvent`. */
export type AnyApiEvent = ApiEvent | UnknownApiEvent;

export type ApiEventKind = ApiEvent["ev"];

export interface ClientFrame {
  v: number;
  id: number;
  [key: string]: unknown;
}

export type ServerFrame = { v: number; reply_to?: number } & UnknownApiEvent;

/** Every event tag the SDK knows about, for drift checks and routing. */
export const KNOWN_EVENT_KINDS = [
  "hello_ok",
  "ok",
  "error",
  "sessions",
  "attached",
  "session_forked",
  "history",
  "side_pane_images",
  "pong",
  "text_delta",
  "reasoning_delta",
  "reasoning_done",
  "tool_start",
  "tool_input_delta",
  "tool_exec",
  "tool_done",
  "token_usage",
  "turn_done",
  "wake_requested",
  "background_progress",
  "message_accepted",
  "permission_request",
  "session_status",
  "connection_phase",
  "model_info",
  "models",
  "runtime_info",
  "credential_updated",
  "file_content",
  "files",
  "text_matches",
  "file_status",
  "compacted",
  "session_renamed",
] as const;

/** Every request tag the SDK can send. */
export const KNOWN_REQUEST_KINDS = [
  "hello",
  "list_sessions",
  "archive_session",
  "restore_session",
  "set_retention_policy",
  "create_session",
  "attach_session",
  "fork_session",
  "detach_session",
  "send_message",
  "cancel",
  "soft_interrupt",
  "get_history",
  "peek_session",
  "clear",
  "rewind",
  "permission_response",
  "list_models",
  "get_runtime_info",
  "set_api_key",
  "clear_api_key",
  "read_file",
  "find_files",
  "search_text",
  "file_status",
  "set_model",
  "set_reasoning_effort",
  "compact",
  "rename_session",
  "rewind_undo",
  "cancel_soft_interrupts",
  "ping",
] as const;

export function isKnownEvent(frame: AnyApiEvent): frame is ApiEvent {
  return (KNOWN_EVENT_KINDS as readonly string[]).includes(frame.ev);
}
