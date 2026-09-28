import { Ajv, type ValidateFunction } from "ajv";
import { HarnessError } from "./errors.js";
import type { SessionToolDefinition, ToolConfiguration } from "./protocol.js";

export interface ToolExecutionContext {
  sessionId: string;
  callId: string;
  /** Aborted on cancellation, disconnect, or callback timeout. Cooperate to stop side effects. */
  signal: AbortSignal;
}

export interface ToolResult {
  output: string;
  error?: string;
}

/** A session-local tool, or an explicit replacement for a tool with the same name. */
export interface SdkTool extends SessionToolDefinition {
  execute(input: Record<string, unknown>, context: ToolExecutionContext):
    string | ToolResult | Promise<string | ToolResult>;
  /** Callback deadline in milliseconds. Default 60000, maximum 60000. */
  timeoutMs?: number;
}

export interface SessionToolsOptions {
  /** Omit/null to inherit configured defaults. [] exposes no built-in or MCP tools. */
  enabled?: string[] | null;
  /** Deny entries take precedence, including over custom tools. */
  disabled?: string[];
  /** Custom tools are added to the selection, replacing same-name tools in this session only. */
  custom?: SdkTool[];
}

type Handler = { tool: SdkTool; validate: ValidateFunction };
export interface PreparedTools {
  wire: ToolConfiguration;
  handlers: Map<string, Handler>;
}

/** Validate before changing either the local callback table or the remote policy. */
export function prepareTools(options: SessionToolsOptions): PreparedTools {
  const handlers = new Map<string, Handler>();
  const ajv = new Ajv({ strict: false, allErrors: true, addUsedSchema: false });
  const custom = (options.custom ?? []).map((tool) => {
    if (handlers.has(tool.name)) throw new HarnessError("invalid_option", `duplicate tool: ${tool.name}`);
    if (typeof tool.execute !== "function") throw new HarnessError("invalid_option", `${tool.name} needs an execute callback`);
    const timeout = tool.timeoutMs ?? 60_000;
    if (!Number.isInteger(timeout) || timeout < 1 || timeout > 60_000) {
      throw new HarnessError("invalid_option", "tool timeoutMs must be 1..60000");
    }
    // Snapshot definitions so mutations after registration cannot change callback validation.
    const definition: SessionToolDefinition = JSON.parse(JSON.stringify({
      name: tool.name, description: tool.description, parameters: tool.parameters,
    }));
    if (!definition.parameters || definition.parameters.type !== "object") {
      throw new HarnessError("invalid_option", `${tool.name} parameters must be an object JSON Schema`);
    }
    let validate: ValidateFunction;
    try {
      validate = ajv.compile(definition.parameters);
      if ("$async" in validate && validate.$async) throw new Error("async schemas are not supported");
    } catch (error) {
      throw new HarnessError("invalid_option", `invalid schema for ${tool.name}: ${String(error)}`);
    }
    handlers.set(tool.name, { tool: { ...tool, ...definition, timeoutMs: timeout }, validate });
    return definition;
  });
  return {
    wire: { enabled: options.enabled == null ? options.enabled : [...options.enabled], disabled: [...(options.disabled ?? [])], custom },
    handlers,
  };
}

/** Connection-local dispatch. Events on observing connections never execute callbacks. */
export class ToolCallbacks {
  readonly sessions = new Map<string, Map<string, Handler>>();
  private readonly active = new Map<string, { sessionId: string; controller: AbortController }>();
  private readonly seen = new Set<string>();

  constructor(
    private readonly submit: (sessionId: string, callId: string, result: ToolResult) => Promise<void>,
    private readonly report: (error: unknown) => void,
  ) {}

  dispatch(sessionId: string, callId: string, name: string, input: unknown): void {
    const handlers = this.sessions.get(sessionId);
    if (!handlers) return; // Low-level clients can handle tool_call themselves.
    const key = JSON.stringify([sessionId, callId]);
    if (this.seen.has(key)) return;
    this.seen.add(key);
    const controller = new AbortController();
    this.active.set(key, { sessionId, controller });
    const handler = handlers.get(name);
    void this.execute(handler, input, { sessionId, callId, signal: controller.signal }, controller)
      .then((result) => {
        // Cancellation/disconnect must not send a stale result to a later turn.
        if (!controller.signal.aborted || controller.signal.reason === "timeout") {
          return this.submit(sessionId, callId, result);
        }
      })
      .catch(this.report)
      .finally(() => this.active.delete(key));
  }

  private async execute(handler: Handler | undefined, input: unknown, context: ToolExecutionContext,
    controller: AbortController): Promise<ToolResult> {
    if (!handler) return { output: "", error: "SDK tool callback is not registered" };
    if (!handler.validate(input)) {
      return { output: "", error: `Invalid tool input: ${JSON.stringify(handler.validate.errors)}` };
    }
    let timer: NodeJS.Timeout | undefined;
    let onAbort: (() => void) | undefined;
    try {
      const aborted = new Promise<never>((_, reject) => {
        onAbort = () => reject(new Error(controller.signal.reason === "timeout" ? "SDK tool callback timed out" : "SDK tool callback cancelled"));
        controller.signal.addEventListener("abort", onAbort, { once: true });
        timer = setTimeout(() => controller.abort("timeout"), handler.tool.timeoutMs);
        timer.unref?.();
      });
      const result = await Promise.race([
        Promise.resolve().then(() => {
          controller.signal.throwIfAborted();
          return handler.tool.execute(input as Record<string, unknown>, context);
        }),
        aborted,
      ]);
      if (typeof result === "string") return { output: result };
      if (!result || typeof result.output !== "string" || (result.error !== undefined && typeof result.error !== "string")) {
        throw new Error("SDK tool must return a string or { output: string, error?: string }");
      }
      return { output: result.output, error: result.error };
    } catch (error) {
      return { output: "", error: error instanceof Error ? error.message : String(error) };
    } finally {
      if (timer) clearTimeout(timer);
      if (onAbort) controller.signal.removeEventListener("abort", onAbort);
    }
  }

  abort(sessionId?: string): void {
    for (const entry of this.active.values()) {
      if (sessionId === undefined || entry.sessionId === sessionId) entry.controller.abort("cancelled");
    }
    // Call IDs are unique within a turn. Bound deduplication memory to that turn.
    for (const key of this.seen) {
      if (sessionId === undefined || JSON.parse(key)[0] === sessionId) this.seen.delete(key);
    }
  }

  close(): void {
    this.abort();
    this.sessions.clear();
  }
}
