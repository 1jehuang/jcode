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
  API_VERSION_MAJOR,
  type AnyApiEvent,
  type ApiEvent,
  type ApiRequest,
  type HistoryMessage,
  type ImageAttachment,
  type PermissionDecision,
  type ServerFrame,
  type SessionInfo,
} from "./protocol.js";



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

  private constructor(transport: Transport, requestTimeoutMs: number) {
    super();
    this.setMaxListeners(0);
    this.transport = transport;
    this.requestTimeoutMs = requestTimeoutMs;
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
    const transport =
      options.transport ?? (await unixSocketTransport(options.socketPath ?? apiSocketPath()));
    const client = new JcodeClient(transport, options.requestTimeoutMs ?? 30_000);
    const frame = await client.request({
      req: "hello",
      min_version: API_VERSION_MAJOR,
      max_version: API_VERSION_MAJOR,
      client: options.clientName ?? "jcode-sdk-ts",
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

  async listSessions(): Promise<SessionInfo[]> {
    const frame = await this.expectReply({ req: "list_sessions" }, "sessions");
    return frame.sessions ?? [];
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
    images: ImageAttachment[] = [],
    options: { waitForAccept?: boolean; acceptTimeoutMs?: number } = {},
  ): Promise<void> {
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
    const frame = await this.expectReply(
      { req: "get_history", session_id: sessionId },
      "history",
    );
    return frame.messages ?? [];
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
      done = true;
      if (resolveNext) {
        const resolve = resolveNext;
        resolveNext = undefined;
        resolve({ value: undefined as never, done: true });
      }
    };
    this.on("event", onEvent);
    this.once("close", onClose);

    const stop = (): Promise<IteratorResult<ApiEvent>> => {
      done = true;
      this.off("event", onEvent);
      this.off("close", onClose);
      return Promise.resolve({ value: undefined as never, done: true });
    };

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
   * Send a message and collect the assistant reply until the turn ends.
   *
   * The convenience path for scripts: one call in, the text and tool calls of
   * one turn out. Streaming consumers should use `events()` instead.
   */
  async run(
    sessionId: string,
    content: string,
    options: {
      images?: ImageAttachment[];
      onEvent?: (event: ApiEvent) => void;
      /**
       * Auto-answer permission prompts.
       *
       * Only meaningful when the server advertises the `permissions`
       * capability; otherwise no prompt is ever issued and this is a no-op.
       */
      autoApprove?: boolean;
    } = {},
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

export { HarnessError };
