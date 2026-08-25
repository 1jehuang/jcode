/**
 * Harness client: handshake, request/reply correlation, and event streaming.
 */

import net from "node:net";
import { EventEmitter } from "node:events";
import { NdjsonDecoder, encodeFrame } from "./framing.js";
import { apiSocketPath, transportEndpoint } from "./sockets.js";
import { launchInstance, type LaunchOptions, type LaunchedInstance } from "./launch.js";
import { HarnessError } from "./errors.js";
import {
  assertRetryCount,
  buildStructuredCorrectionPrompt,
  buildStructuredPrompt,
  compileStructuredSchema,
  StructuredOutputError,
  validateStructuredText,
  type StructuredOutputAttempt,
  type StructuredOutputSchema,
} from "./structured.js";
import {
  API_VERSION_MAJOR,
  type AnyApiEvent,
  type ApiEvent,
  type ApiRequest,
  type HistoryMessage,
  type ImageAttachment,
  type ModelRouteInfo,
  type PermissionDecision,
  type RenderedImage,
  type ServerFrame,
  type SessionInfo,
  type TextMatch,
} from "./protocol.js";

export interface RuntimeInfo {
  server: string;
  protocolVersion: number;
  capabilities: string[];
  healthy: true;
  sessionId: string;
  provider?: string;
  model?: string;
  providers: string[];
  routes: ModelRouteInfo[];
}

export interface FileContent {
  path: string;
  content: string;
  size: number;
  truncated: boolean;
}

export interface FileStatus {
  path: string;
  exists: boolean;
  kind: string;
  size?: number;
  modifiedMs?: number;
}



/** Minimal duplex transport so tests and future WebSockets can plug in. */
export interface Transport {
  write(data: string): void;
  onData(listener: (chunk: Buffer | string) => void): void;
  onClose(listener: (error?: Error) => void): void;
  close(): void;
}

export function unixSocketTransport(socketPath: string): Promise<Transport> {
  return new Promise((resolve, reject) => {
    // On Windows the bridge listens on a named pipe derived from this path,
    // not on the path itself.
    const socket = net.createConnection(transportEndpoint(socketPath));
    socket.setNoDelay(true);
    // A bare `connect ENOENT /run/user/1000/jcode-api.sock` names the syscall
    // and hides the actual cause: the bridge is not running. The first thing
    // anyone does with this SDK is connect, so this is the error most likely
    // to be someone's first impression of it. Say what to do about it.
    const onConnectError = (cause: NodeJS.ErrnoException) =>
      reject(connectError(socketPath, cause));
    socket.once("error", onConnectError);
    socket.once("connect", () => {
      socket.removeListener("error", onConnectError);
      // Keep a listener attached for the connection's lifetime: an unhandled
      // socket "error" (an EPIPE when the harness goes away mid-write, say)
      // is a fatal throw in Node, and callers should see a rejected request
      // instead of a dead process.
      socket.on("error", () => {});
      resolve({
        write: (data) => {
          if (socket.destroyed || socket.writableEnded) {
            throw new HarnessError("disconnected", "harness connection closed");
          }
          // A peer that vanishes between the destroyed check and the syscall
          // surfaces EPIPE, sometimes synchronously. Translate it into a
          // rejected request rather than an uncaught exception; the close
          // handler already fails everything else in flight.
          try {
            socket.write(data, () => {});
          } catch (cause) {
            throw new HarnessError("disconnected", `write failed: ${String(cause)}`);
          }
        },
        onData: (listener) => socket.on("data", listener),
        onClose: (listener) => {
          socket.on("close", () => listener());
          socket.on("error", (error) => listener(error));
        },
        close: () => socket.destroy(),
      });
    });
  });
}

/** Turn a dial failure into an error that names the likely cause and fix. */
function connectError(socketPath: string, cause: NodeJS.ErrnoException): HarnessError {
  const code = cause.code ?? "unknown";
  const hint =
    code === "ENOENT"
      ? `no harness API socket at ${socketPath}. Start the bridge with ` +
        "`jcode api-bridge`, or set JCODE_API_SOCKET to its path."
      : code === "ECONNREFUSED"
        ? `nothing is listening on ${socketPath}; a stale socket file is left over ` +
          "from a bridge that exited. Restart the bridge."
        : code === "EACCES"
          ? `permission denied on ${socketPath}: the socket belongs to another user.`
          : `could not connect to ${socketPath}: ${cause.message}`;
  const error = new HarnessError("connect_failed", hint);
  error.cause = cause;
  return error;
}

export interface ConnectOptions {
  /** Defaults to the resolved harness API socket path. */
  socketPath?: string;
  /** Client identity sent in the handshake, e.g. "my-app/1.0". */
  clientName?: string;
  /** Supply a custom transport instead of dialing a Unix socket. */
  transport?: Transport;
  /** Milliseconds before a request without a reply rejects. 0 disables. */
  requestTimeoutMs?: number;
}

export interface GlobalEventsOptions {
  /** Stop discovery and close every per-session child connection. */
  signal?: AbortSignal;
  /** Milliseconds between checks for newly created sessions. Defaults to 1000. */
  discoveryIntervalMs?: number;
  /** Maximum events waiting for the consumer before the iterator fails loudly. Defaults to 10000. */
  maxBufferedEvents?: number;
}

export interface RunOptions {
  images?: ImageAttachment[];
  onEvent?: (event: ApiEvent) => void;
  /**
   * Auto-answer permission prompts.
   *
   * Only meaningful when the server advertises the `permissions` capability;
   * otherwise no prompt is ever issued and this is a no-op.
   */
  autoApprove?: boolean;
}

export interface SendMessageOptions {
  /** Attach images to the persisted user message. */
  images?: ImageAttachment[];
  /** Persist the message as context without starting a model turn. */
  noReply?: boolean;
  /** Wait for message_accepted when starting a normal model turn. */
  waitForAccept?: boolean;
  /** Maximum acknowledgement wait in milliseconds. */
  acceptTimeoutMs?: number;
}

export interface RunStructuredOptions<T = unknown> extends RunOptions {
  /** JSON Schema the assistant's JSON response must satisfy. */
  schema: StructuredOutputSchema<T>;
  /** Corrective retries after the first invalid response. Defaults to 2. */
  maxRetries?: number;
}

interface Pending {
  resolve: (frame: ServerFrame) => void;
  reject: (error: Error) => void;
  timer?: NodeJS.Timeout;
}

/**
 * Connected harness client.
 *
 * Replies are correlated by the `reply_to` id the server echoes; anything
 * without one is a stream event and is emitted on `event` plus a per-kind
 * channel (`client.on("text_delta", ...)`).
 */
export class JcodeClient extends EventEmitter {
  private readonly transport: Transport;
  private readonly decoder = new NdjsonDecoder();
  private readonly pending = new Map<number, Pending>();
  private readonly requestTimeoutMs: number;
  private readonly socketPath?: string;
  private readonly clientName: string;
  private nextId = 1;
  private closed = false;
  private closeError?: Error;

  /** Server identity from the handshake, e.g. "jcode-harness-api-bridge/0.1.0". */
  server = "";
  /** Capability strings advertised by the server. */
  capabilities: string[] = [];

  /**
   * Whether the server advertises a capability.
   *
   * Capabilities are how a client learns what this particular server can do
   * before depending on it. `permissions`, for instance, is absent from the
   * current bridge: it never issues permission prompts, so a client that waits
   * for one waits forever. Check rather than assume.
   */
  supports(capability: string): boolean {
    return this.capabilities.includes(capability);
  }

  private constructor(
    transport: Transport,
    requestTimeoutMs: number,
    socketPath: string | undefined,
    clientName: string,
  ) {
    super();
    this.setMaxListeners(0);
    this.transport = transport;
    this.requestTimeoutMs = requestTimeoutMs;
    this.socketPath = socketPath;
    this.clientName = clientName;
    transport.onData((chunk) => this.ingest(chunk));
    transport.onClose((error) => this.handleClose(error));
  }

  /** Set when this client owns a private instance, so `close()` stops it. */
  private instance?: LaunchedInstance;

  /** State directory of the instance this client launched, if any. */
  get instanceHome(): string | undefined {
    return this.instance?.jcodeHome;
  }

  /**
   * Start a private jcode instance and connect to it.
   *
   * This is the entry point for embedding jcode as an agent engine. The
   * instance has its own state directory, its own sessions, and its own
   * sockets, so it cannot see or disturb the jcode the user runs
   * interactively. `close()` shuts it down.
   *
   * Provider logins are inherited from the user by default, because an
   * instance without credentials cannot reach a model at all. Pass
   * `inheritLogins: false` to start empty.
   */
  static async launch(options: LaunchOptions & ConnectOptions = {}): Promise<JcodeClient> {
    const instance = await launchInstance(options);
    try {
      const client = await JcodeClient.connect({
        ...options,
        socketPath: instance.socketPath,
      });
      client.instance = instance;
      return client;
    } catch (error) {
      // A launched instance whose client never connected is a stray daemon
      // holding a temp directory open, so tear it down before rethrowing.
      await instance.shutdown();
      throw error;
    }
  }

  /**
   * Connect to the jcode already running on this machine.
   *
   * Use this to automate the user's own jcode: an editor plugin, a status
   * dashboard, a hotkey tool. It shares the user's live sessions, so anything
   * done here is visible in their terminal. To embed jcode as an engine
   * instead, use {@link JcodeClient.launch}.
   */
  static async connect(options: ConnectOptions = {}): Promise<JcodeClient> {
    const socketPath = options.transport ? undefined : (options.socketPath ?? apiSocketPath());
    const transport = options.transport ?? (await unixSocketTransport(socketPath!));
    const clientName = options.clientName ?? "jcode-sdk-ts";
    const client = new JcodeClient(
      transport,
      options.requestTimeoutMs ?? 30_000,
      socketPath,
      clientName,
    );
    const frame = await client.request({
      req: "hello",
      min_version: API_VERSION_MAJOR,
      max_version: API_VERSION_MAJOR,
      client: clientName,
    });
    if (frame.ev !== "hello_ok") {
      client.close();
      throw new HarnessError("handshake_failed", `unexpected reply: ${frame.ev}`);
    }
    client.server = String(frame.server ?? "");
    client.capabilities = (frame.capabilities as string[] | undefined) ?? [];
    return client;
  }

  private ingest(chunk: Buffer | string): void {
    let frames: unknown[];
    try {
      frames = this.decoder.push(chunk);
    } catch (error) {
      this.emitSafe("error", error);
      return;
    }
    for (const raw of frames) {
      const frame = raw as ServerFrame;
      const replyTo = frame.reply_to;
      if (typeof replyTo === "number" && this.pending.has(replyTo)) {
        const waiter = this.pending.get(replyTo)!;
        this.pending.delete(replyTo);
        if (waiter.timer) clearTimeout(waiter.timer);
        waiter.resolve(frame);
        continue;
      }
      this.emit("event", frame);
      // Unknown kinds still land on `event` so a client can log them, but
      // per-kind listeners only ever see the tags they asked for.
      //
      // `error` is remapped: Node's EventEmitter treats a bare "error" event
      // with no listener as a fatal throw, so an unsolicited harness error
      // frame would crash the host process instead of being reported. Protocol
      // errors are delivered on "harness_error"; the plain "error" channel is
      // reserved for transport faults and is emitted defensively.
      this.emitSafe(frame.ev === "error" ? "harness_error" : frame.ev, frame);
    }
  }

  /** Emit without Node's fatal-throw behaviour for unlistened "error". */
  private emitSafe(event: string, payload: unknown): void {
    if (event === "error" && this.listenerCount("error") === 0) return;
    this.emit(event, payload);
  }

  private handleClose(error?: Error): void {
    this.closed = true;
    this.closeError = error ?? new Error("harness connection closed");
    for (const [, waiter] of this.pending) {
      if (waiter.timer) clearTimeout(waiter.timer);
      waiter.reject(this.closeError);
    }
    this.pending.clear();
    this.emit("close", error);
  }

  /** Send a raw request and await its reply frame. */
  request(request: ApiRequest): Promise<ServerFrame> {
    if (this.closed) return Promise.reject(this.closeError ?? new Error("client closed"));
    const id = this.nextId++;
    return new Promise<ServerFrame>((resolve, reject) => {
      const entry: Pending = { resolve, reject };
      if (this.requestTimeoutMs > 0) {
        entry.timer = setTimeout(() => {
          this.pending.delete(id);
          reject(new HarnessError("timeout", `no reply to ${request.req} within ${this.requestTimeoutMs}ms`));
        }, this.requestTimeoutMs);
        entry.timer.unref?.();
      }
      this.pending.set(id, entry);
      try {
        this.transport.write(encodeFrame({ v: API_VERSION_MAJOR, id, ...request }));
      } catch (error) {
        this.pending.delete(id);
        if (entry.timer) clearTimeout(entry.timer);
        reject(error as Error);
      }
    });
  }

  /** Send a request, rejecting when the server replies with an error frame. */
  private async requestOk(request: ApiRequest): Promise<ServerFrame> {
    const frame = await this.request(request);
    if (frame.ev === "error") {
      throw new HarnessError(String(frame.code), String(frame.message));
    }
    return frame;
  }


  /**
   * Send a request and assert the reply's event kind, narrowing its type.
   *
   * Without this the typed accessors below would need casts at every call
   * site, which is exactly where a wrong field name slips through.
   */
  private async expectReply<K extends ApiEvent["ev"]>(
    request: ApiRequest,
    kind: K,
  ): Promise<Extract<ApiEvent, { ev: K }>> {
    const frame = await this.requestOk(request);
    if (frame.ev !== kind) {
      throw new HarnessError("unexpected_reply", `expected ${kind}, got ${frame.ev}`);
    }
    return frame as unknown as Extract<ApiEvent, { ev: K }>;
  }

  // --- Curated surface -----------------------------------------------------

  async listSessions(options: { includeArchived?: boolean; limit?: number } = {}): Promise<SessionInfo[]> {
    const frame = await this.expectReply(
      { req: "list_sessions", include_archived: options.includeArchived, limit: options.limit },
      "sessions",
    );
    return frame.sessions ?? [];
  }

  async listSessionsLimited(limit: number): Promise<SessionInfo[]> {
    return this.listSessions({ limit });
  }

  /** Reversibly hide a session from the default list. No transcript is deleted. */
  async archiveSession(sessionId: string): Promise<void> {
    await this.requestOk({ req: "archive_session", session_id: sessionId });
  }

  async restoreSession(sessionId: string): Promise<void> {
    await this.requestOk({ req: "restore_session", session_id: sessionId });
  }

  /** Auto-archive inactive sessions after this many days; omit to disable. */
  async setRetentionPolicy(archiveAfterDays?: number): Promise<void> {
    await this.requestOk({ req: "set_retention_policy", archive_after_days: archiveAfterDays });
  }

  async createSession(workingDir?: string): Promise<SessionInfo> {
    const frame = await this.expectReply(
      { req: "create_session", working_dir: workingDir },
      "attached",
    );
    return frame.session;
  }

  async attachSession(sessionId: string): Promise<SessionInfo> {
    const frame = await this.expectReply(
      { req: "attach_session", session_id: sessionId },
      "attached",
    );
    return frame.session;
  }

  /** Clone a session's complete persisted context into a new idle session. */
  async forkSession(sessionId: string): Promise<SessionInfo> {
    const frame = await this.expectReply(
      { req: "fork_session", session_id: sessionId },
      "session_forked",
    );
    return frame.session;
  }

  async detachSession(sessionId: string): Promise<void> {
    await this.requestOk({ req: "detach_session", session_id: sessionId });
  }

  /**
   * Send a user message.
   *
   * The harness does not reply to `send_message` at the request level: it
   * acknowledges by emitting `message_accepted` once the agent has the
   * message. Awaiting a reply here would always time out, so the frame is
   * written and, by default, the acknowledgement event is awaited instead.
   * Pass `waitForAccept: false` for pure fire-and-forget.
   */
  async sendMessage(
    sessionId: string,
    content: string,
    options?: SendMessageOptions,
  ): Promise<void>;
  async sendMessage(
    sessionId: string,
    content: string,
    images?: ImageAttachment[],
    options?: Omit<SendMessageOptions, "images">,
  ): Promise<void>;
  async sendMessage(
    sessionId: string,
    content: string,
    imagesOrOptions: ImageAttachment[] | SendMessageOptions = [],
    legacyOptions: Omit<SendMessageOptions, "images"> = {},
  ): Promise<void> {
    const options: SendMessageOptions = Array.isArray(imagesOrOptions)
      ? { ...legacyOptions, images: imagesOrOptions }
      : imagesOrOptions;
    const images = options.images ?? [];
    if (options.noReply) {
      await this.requestOk({
        req: "send_message",
        session_id: sessionId,
        content,
        images,
        no_reply: true,
      });
      return;
    }
    const waitForAccept = options.waitForAccept ?? true;
    const accepted = waitForAccept
      ? this.waitForEvent(
          (frame) => frame.ev === "message_accepted" && frame.session_id === sessionId,
          options.acceptTimeoutMs ?? 10_000,
        )
      : undefined;
    this.notify({ req: "send_message", session_id: sessionId, content, images });
    await accepted;
  }

  /** Write a request without expecting a request-level reply. */
  notify(request: ApiRequest): void {
    if (this.closed) throw this.closeError ?? new Error("client closed");
    const id = this.nextId++;
    this.transport.write(encodeFrame({ v: API_VERSION_MAJOR, id, ...request }));
  }

  /** Resolve on the first event matching `predicate`, or on timeout. */
  private waitForEvent(
    predicate: (frame: ServerFrame) => boolean,
    timeoutMs: number,
  ): Promise<void> {
    return new Promise<void>((resolve) => {
      const finish = () => {
        clearTimeout(timer);
        this.off("event", onEvent);
        this.off("close", finish);
        resolve();
      };
      const onEvent = (frame: ServerFrame) => {
        if (predicate(frame)) finish();
      };
      // Resolving on timeout rather than rejecting keeps a missing ack from
      // failing an otherwise healthy turn: the stream is the source of truth.
      const timer = setTimeout(finish, timeoutMs);
      timer.unref?.();
      this.on("event", onEvent);
      this.once("close", finish);
    });
  }

  async cancel(sessionId: string): Promise<void> {
    await this.requestOk({ req: "cancel", session_id: sessionId });
  }

  async softInterrupt(sessionId: string, content: string, urgent = false): Promise<void> {
    await this.requestOk({ req: "soft_interrupt", session_id: sessionId, content, urgent });
  }

  async getHistory(sessionId: string): Promise<HistoryMessage[]> {
    const { messages } = await this.getHistoryWithImages(sessionId);
    return messages;
  }

  async getHistoryWithImages(
    sessionId: string,
  ): Promise<{ messages: HistoryMessage[]; images: RenderedImage[] }> {
    const frame = await this.expectReply(
      { req: "get_history", session_id: sessionId },
      "history",
    );
    return { messages: frame.messages ?? [], images: frame.images ?? [] };
  }

  async peekSession(sessionId: string, limit?: number): Promise<HistoryMessage[]> {
    const frame = await this.expectReply(
      { req: "peek_session", session_id: sessionId, limit },
      "history",
    );
    return frame.messages ?? [];
  }

  async clear(sessionId: string): Promise<void> {
    await this.requestOk({ req: "clear", session_id: sessionId });
  }

  async rewind(sessionId: string, messageIndex: number): Promise<void> {
    await this.requestOk({ req: "rewind", session_id: sessionId, message_index: messageIndex });
  }

  async respondToPermission(
    sessionId: string,
    requestId: string,
    decision: PermissionDecision,
  ): Promise<void> {
    await this.requestOk({
      req: "permission_response",
      session_id: sessionId,
      request_id: requestId,
      decision,
    });
  }

  /**
   * Models this session can switch to, and which one is serving it.
   *
   * Answered from the catalog the daemon pushes on attach, so opening a model
   * picker does not cost a round trip.
   */
  async listModels(sessionId: string): Promise<{ models: string[]; current?: string }> {
    const frame = await this.expectReply({ req: "list_models", session_id: sessionId }, "models");
    return { models: frame.models ?? [], current: frame.current };
  }

  /** Runtime identity, route catalog, protocol metadata, and a live health check. */
  async getRuntimeInfo(sessionId: string): Promise<RuntimeInfo> {
    await this.ping();
    const frame = await this.expectReply(
      { req: "get_runtime_info", session_id: sessionId },
      "runtime_info",
    );
    const routes = frame.routes ?? [];
    return {
      server: this.server,
      protocolVersion: API_VERSION_MAJOR,
      capabilities: [...this.capabilities],
      healthy: true,
      sessionId: frame.session_id,
      provider: frame.provider,
      model: frame.model,
      providers: [
        ...new Set(
          [frame.provider, ...routes.map((route) => route.provider)].filter(
            (provider): provider is string => provider !== undefined,
          ),
        ),
      ],
      routes,
    };
  }

  /** Persist an API key in jcode's owner-only provider store and hot-reload it. */
  async setApiKey(provider: string, apiKey: string): Promise<void> {
    await this.expectReply({ req: "set_api_key", provider, api_key: apiKey }, "credential_updated");
  }

  async clearApiKey(provider: string): Promise<void> {
    await this.expectReply({ req: "clear_api_key", provider }, "credential_updated");
  }

  async readFile(sessionId: string, path: string, maxBytes?: number): Promise<FileContent> {
    const frame = await this.expectReply(
      { req: "read_file", session_id: sessionId, path, max_bytes: maxBytes },
      "file_content",
    );
    return { path: frame.path, content: frame.content, size: frame.size, truncated: frame.truncated };
  }

  async findFiles(sessionId: string, query: string, limit?: number): Promise<string[]> {
    const frame = await this.expectReply(
      { req: "find_files", session_id: sessionId, query, limit },
      "files",
    );
    return frame.paths;
  }

  async searchText(
    sessionId: string,
    query: string,
    options: { path?: string; limit?: number } = {},
  ): Promise<TextMatch[]> {
    const frame = await this.expectReply(
      { req: "search_text", session_id: sessionId, query, ...options },
      "text_matches",
    );
    return frame.matches;
  }

  async fileStatus(sessionId: string, path: string): Promise<FileStatus> {
    const frame = await this.expectReply(
      { req: "file_status", session_id: sessionId, path },
      "file_status",
    );
    return {
      path: frame.path,
      exists: frame.exists,
      kind: frame.kind,
      size: frame.size,
      modifiedMs: frame.modified_ms,
    };
  }

  /**
   * Switch the session to a different model.
   *
   * `model` is an id from `listModels`. Rejects with `invalid_request` when
   * the model is unknown or the provider refuses it, rather than silently
   * leaving the session on the old one.
   */
  async setModel(sessionId: string, model: string): Promise<void> {
    await this.requestOk({ req: "set_model", session_id: sessionId, model });
  }

  /**
   * Set how much the model deliberates before answering.
   *
   * Typically "minimal" | "low" | "medium" | "high" | "xhigh" | "max", but the
   * accepted set is per-provider, so this takes a string and reports what the
   * provider says rather than guessing at a union that would go stale.
   */
  async setReasoningEffort(sessionId: string, effort: string): Promise<void> {
    await this.requestOk({ req: "set_reasoning_effort", session_id: sessionId, effort });
  }

  /**
   * Schedule compaction of the transcript so far, freeing context.
   *
   * Not synchronous: the daemon summarizes at the next safe point rather than
   * interrupting a turn, so this resolving means the request was accepted. Read
   * the history afterwards to see the result. A refusal (nothing to compact, a
   * turn in flight) rejects with `invalid_request` and the reason.
   */
  async compact(sessionId: string): Promise<string> {
    const frame = await this.expectReply({ req: "compact", session_id: sessionId }, "compacted");
    return frame.message;
  }

  /** Set a session's title. Omit `title` to restore the generated one. */
  async renameSession(sessionId: string, title?: string): Promise<void> {
    await this.requestOk({ req: "rename_session", session_id: sessionId, title });
  }

  /** Restore the history the last `rewind` removed. */
  async rewindUndo(sessionId: string): Promise<void> {
    await this.requestOk({ req: "rewind_undo", session_id: sessionId });
  }

  /** Drop soft interrupts that are queued but not yet delivered. */
  async cancelSoftInterrupts(sessionId: string): Promise<void> {
    await this.requestOk({ req: "cancel_soft_interrupts", session_id: sessionId });
  }

  async ping(): Promise<void> {
    await this.requestOk({ req: "ping" });
  }

  /**
   * Async iterator over stream events, optionally filtered to one session.
   *
   * Buffers frames that arrive between `next()` calls, so a consumer that
   * awaits slow work in the loop body does not silently drop deltas.
   */
  /**
   * Typed as the known event union so `switch (event.ev)` narrows each case.
   *
   * Unknown kinds can still arrive at runtime (the harness may add events
   * within protocol v1), so always keep a `default` branch. Use
   * `isKnownEvent` if you need to prove the distinction.
   */
  events(sessionId?: string): AsyncIterableIterator<ApiEvent> {
    const queue: ApiEvent[] = [];
    let resolveNext: ((result: IteratorResult<ApiEvent>) => void) | undefined;
    let done = false;

    const onEvent = (frame: ServerFrame) => {
      if (sessionId && "session_id" in frame && frame.session_id !== sessionId) return;
      if (resolveNext) {
        const resolve = resolveNext;
        resolveNext = undefined;
        resolve({ value: frame as ApiEvent, done: false });
      } else {
        queue.push(frame as ApiEvent);
      }
    };
    const onClose = () => {
      void stop();
    };

    const stop = (): Promise<IteratorResult<ApiEvent>> => {
      done = true;
      queue.length = 0;
      this.off("event", onEvent);
      this.off("close", onClose);
      if (resolveNext) {
        const resolve = resolveNext;
        resolveNext = undefined;
        resolve({ value: undefined as never, done: true });
      }
      return Promise.resolve({ value: undefined as never, done: true });
    };

    this.on("event", onEvent);
    this.once("close", onClose);

    return {
      [Symbol.asyncIterator]() {
        return this;
      },
      next: () => {
        if (queue.length > 0) {
          return Promise.resolve({ value: queue.shift()!, done: false });
        }
        if (done) return stop();
        return new Promise<IteratorResult<ApiEvent>>((resolve) => {
          resolveNext = resolve;
        });
      },
      return: stop,
      throw: stop,
    };
  }

  /**
   * Stream events from every session through per-session child connections.
   *
   * The bridge has one attachment per connection, so a server-wide stream is
   * implemented by discovering persisted sessions, attaching one child client
   * to each, and fanning their events into this iterator. Discovery repeats so
   * sessions created after subscription are included.
   *
   * Delivery is at-least-from-attach, not historical replay: events emitted
   * before a child finishes attaching, or during an unexpected disconnect and
   * reattach, cannot be recovered by protocol v1. Ordering is preserved within
   * each session; no total ordering across sessions is claimed.
   *
   * Calling `return()`, aborting `signal`, or closing this client closes every
   * child connection. Custom transports are rejected because the SDK cannot
   * safely clone an arbitrary transport into independent session connections.
   */
  globalEvents(options: GlobalEventsOptions = {}): AsyncIterableIterator<ApiEvent> {
    if (!this.socketPath) {
      throw new HarnessError(
        "unsupported_transport",
        "globalEvents requires a native socket connection; custom transports cannot be cloned into per-session child connections",
      );
    }
    if (this.closed) {
      throw this.closeError ?? new HarnessError("disconnected", "harness connection closed");
    }

    const discoveryIntervalMs = options.discoveryIntervalMs ?? 1_000;
    const maxBufferedEvents = options.maxBufferedEvents ?? 10_000;
    if (!Number.isFinite(discoveryIntervalMs) || discoveryIntervalMs < 0) {
      throw new HarnessError("invalid_option", "discoveryIntervalMs must be a finite number >= 0");
    }
    if (!Number.isInteger(maxBufferedEvents) || maxBufferedEvents < 1) {
      throw new HarnessError("invalid_option", "maxBufferedEvents must be a positive integer");
    }

    type Waiter = {
      resolve: (result: IteratorResult<ApiEvent>) => void;
      reject: (error: Error) => void;
    };
    type Child = {
      client: JcodeClient;
      stream: AsyncIterableIterator<ApiEvent>;
      pump?: Promise<void>;
    };

    const queue: ApiEvent[] = [];
    const children = new Map<string, Child>();
    const starting = new Set<string>();
    const startTasks = new Set<Promise<void>>();
    let waiter: Waiter | undefined;
    let stopped = false;
    let terminalError: Error | undefined;
    let discoveryTimer: NodeJS.Timeout | undefined;
    let cleanupPromise: Promise<void> | undefined;

    const settleWaiter = () => {
      if (!waiter) return;
      const pending = waiter;
      waiter = undefined;
      if (terminalError) pending.reject(terminalError);
      else pending.resolve({ value: undefined as never, done: true });
    };

    const onParentClose = (error?: Error) => {
      void finish(error);
    };
    const onAbort = () => {
      void finish();
    };

    const finish = (error?: Error): Promise<void> => {
      if (cleanupPromise) return cleanupPromise;
      stopped = true;
      terminalError = error;
      queue.length = 0;
      if (discoveryTimer) clearTimeout(discoveryTimer);
      this.off("close", onParentClose);
      options.signal?.removeEventListener("abort", onAbort);
      settleWaiter();

      const closing: Promise<unknown>[] = [];
      for (const { client, stream, pump } of children.values()) {
        closing.push(Promise.resolve(stream.return?.(undefined as never)));
        closing.push(client.close());
        if (pump) closing.push(pump);
      }
      children.clear();
      closing.push(...startTasks);
      cleanupPromise = Promise.allSettled(closing).then(() => undefined);
      return cleanupPromise;
    };

    const enqueue = (event: ApiEvent) => {
      if (stopped) return;
      if (waiter) {
        const pending = waiter;
        waiter = undefined;
        pending.resolve({ value: event, done: false });
        return;
      }
      if (queue.length >= maxBufferedEvents) {
        void finish(
          new HarnessError(
            "event_buffer_overflow",
            `globalEvents consumer fell behind ${maxBufferedEvents} buffered events`,
          ),
        );
        return;
      }
      queue.push(event);
    };

    const startChild = async (sessionId: string): Promise<void> => {
      if (stopped || children.has(sessionId) || starting.has(sessionId)) return;
      starting.add(sessionId);
      let child: JcodeClient | undefined;
      try {
        child = await JcodeClient.connect({
          socketPath: this.socketPath,
          clientName: `${this.clientName}/global-events`,
          requestTimeoutMs: this.requestTimeoutMs,
        });
        if (stopped) {
          await child.close();
          return;
        }
        const stream = child.events(sessionId);
        const childState: Child = { client: child, stream };
        children.set(sessionId, childState);
        await child.attachSession(sessionId);
        if (stopped) return;

        const pump = (async () => {
          try {
            for await (const event of stream) enqueue(event);
          } finally {
            if (children.get(sessionId) === childState) children.delete(sessionId);
            await childState.client.close();
          }
        })();
        childState.pump = pump;
        void pump;
      } catch (error) {
        if (child && children.get(sessionId)?.client === child) children.delete(sessionId);
        await child?.close();
        if (
          !stopped &&
          !(error instanceof HarnessError && error.code === "unknown_session")
        ) {
          void finish(error instanceof Error ? error : new Error(String(error)));
        }
      } finally {
        starting.delete(sessionId);
      }
    };

    const scan = async (): Promise<void> => {
      if (stopped) return;
      try {
        const sessions = await this.listSessions({ includeArchived: true });
        if (stopped) return;
        const tasks = sessions.map((session) => {
          const task = startChild(session.session_id);
          startTasks.add(task);
          void task.finally(() => startTasks.delete(task));
          return task;
        });
        await Promise.allSettled(tasks);
      } catch (error) {
        if (!stopped) void finish(error instanceof Error ? error : new Error(String(error)));
      }
      if (!stopped && discoveryIntervalMs > 0) {
        discoveryTimer = setTimeout(() => void scan(), discoveryIntervalMs);
        discoveryTimer.unref?.();
      }
    };

    this.once("close", onParentClose);
    if (options.signal?.aborted) void finish();
    else {
      options.signal?.addEventListener("abort", onAbort, { once: true });
      void scan();
    }

    return {
      [Symbol.asyncIterator]() {
        return this;
      },
      next: () => {
        if (queue.length > 0) {
          return Promise.resolve({ value: queue.shift()!, done: false });
        }
        if (terminalError) return Promise.reject(terminalError);
        if (stopped) {
          return Promise.resolve({ value: undefined as never, done: true });
        }
        if (waiter) {
          return Promise.reject(
            new HarnessError("concurrent_next", "globalEvents does not support concurrent next() calls"),
          );
        }
        return new Promise<IteratorResult<ApiEvent>>((resolve, reject) => {
          waiter = { resolve, reject };
        });
      },
      return: async () => {
        await finish();
        return { value: undefined as never, done: true };
      },
      throw: async (error?: unknown) => {
        await finish();
        throw error;
      },
    };
  }

  /**
   * Send a message and collect the assistant reply until the turn ends.
   *
   * The convenience path for scripts: one call in, the text and tool calls of
   * one turn out. Streaming consumers should use `events()` instead.
   */
  async run(
    sessionId: string,
    content: string,
    options: RunOptions = {},
  ): Promise<TurnResult> {
    const stream = this.events(sessionId);
    await this.sendMessage(sessionId, content, options.images ?? []);
    const result: TurnResult = { text: "", reasoning: "", toolCalls: [], usage: undefined };
    for await (const event of stream) {
      options.onEvent?.(event);
      switch (event.ev) {
        // No casts here: the event union narrows on `ev`. That is the point
        // of keeping the unknown-kind catch-all out of `ApiEvent`, and it is
        // also what stops a renamed wire field from compiling.
        case "text_delta":
          result.text += event.text;
          break;
        case "reasoning_delta":
          result.reasoning += event.text;
          break;
        case "tool_done":
          result.toolCalls.push({
            callId: event.call_id,
            name: event.name,
            output: event.output,
            error: event.error,
          });
          break;
        case "token_usage":
          result.usage = {
            input: event.input,
            output: event.output,
            cacheReadInput: event.cache_read_input,
          };
          break;
        case "permission_request":
          if (options.autoApprove) {
            await this.respondToPermission(sessionId, event.request_id, "allow");
          }
          break;
        case "turn_done":
          await stream.return?.(undefined as never);
          return result;
        case "error":
          await stream.return?.(undefined as never);
          throw new HarnessError(event.code ?? "internal", event.message ?? "harness error");
      }
    }
    return result;
  }

  /**
   * Send a message and parse the assistant's answer as JSON validated by schema.
   *
   * This is an SDK-level contract: it does not require a special bridge or model
   * feature. The initial prompt asks for JSON only; if the response is not JSON
   * or fails Ajv validation, the client sends a bounded number of corrective
   * prompts that include the normalized validation errors and the prior text.
   */
  async runStructured<T = unknown>(
    sessionId: string,
    content: string,
    options: RunStructuredOptions<T>,
  ): Promise<StructuredTurnResult<T>> {
    const { schema, maxRetries = 2, ...runOptions } = options;
    assertRetryCount(maxRetries);
    const validate = compileStructuredSchema(schema);
    const attempts: StructuredOutputAttempt[] = [];
    let prompt = buildStructuredPrompt(content, schema);

    for (let attemptNumber = 1; attemptNumber <= maxRetries + 1; attemptNumber += 1) {
      const turn = await this.run(sessionId, prompt, runOptions);
      const validation = validateStructuredText(turn.text, validate);
      const attempt: StructuredOutputAttempt = {
        attempt: attemptNumber,
        text: turn.text,
        errors: validation.errors,
      };
      attempts.push(attempt);
      if (validation.ok) return { ...turn, data: validation.data, attempts };
      prompt = buildStructuredCorrectionPrompt(schema, attempt);
    }

    throw new StructuredOutputError(attempts);
  }

  /**
   * Disconnect, and stop the instance if this client launched one.
   *
   * Returns a promise so a launched instance can be awaited to a stop; for a
   * plain `connect()` client it resolves immediately and can be ignored.
   */
  close(): Promise<void> {
    this.transport.close();
    const instance = this.instance;
    this.instance = undefined;
    return instance ? instance.shutdown() : Promise.resolve();
  }
}

export interface TurnResult {
  text: string;
  reasoning: string;
  toolCalls: Array<{ callId: string; name: string; output: string; error?: string }>;
  usage?: { input: number; output: number; cacheReadInput?: number };
}

export interface StructuredTurnResult<T> extends TurnResult {
  /** Parsed JSON value validated against the caller's schema. */
  data: T;
  /** All attempts, including invalid retries and the final successful response. */
  attempts: StructuredOutputAttempt[];
}

export { HarnessError };
