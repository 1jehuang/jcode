import {
  Ajv,
  type AnySchema,
  type ErrorObject,
  type JSONSchemaType,
  type ValidateFunction,
} from "ajv";
import { HarnessError } from "./errors.js";

/** JSON Schema accepted by {@link JcodeClient.runStructured}. */
export type StructuredOutputSchema<T = unknown> = JSONSchemaType<T> | AnySchema;

/** A normalized parse or JSON Schema validation problem. */
export interface StructuredValidationIssue {
  /** JSON Pointer path to the invalid value. Empty string means the root. */
  path: string;
  /** Ajv keyword, or `parse` when the text was not valid JSON. */
  keyword: string;
  /** Human-readable validation message. */
  message: string;
  /** Keyword-specific validation metadata from Ajv, when available. */
  params?: Record<string, unknown>;
}

/** One model attempt made by `runStructured`. */
export interface StructuredOutputAttempt {
  /** One-based attempt number. */
  attempt: number;
  /** Raw assistant text produced for this attempt. */
  text: string;
  /** Empty for the successful attempt. */
  errors: StructuredValidationIssue[];
}

/** Raised after all bounded structured-output attempts fail validation. */
export class StructuredOutputError extends HarnessError {
  readonly attempts: StructuredOutputAttempt[];
  readonly validationErrors: StructuredValidationIssue[];
  readonly lastText: string;

  constructor(attempts: StructuredOutputAttempt[]) {
    const last = attempts[attempts.length - 1];
    const count = attempts.length;
    const summary = last?.errors.map(formatIssue).join("; ") || "no structured output attempts ran";
    super(
      "structured_output_invalid",
      `model did not produce valid structured output after ${count} attempt${count === 1 ? "" : "s"}: ${summary}`,
    );
    this.name = "StructuredOutputError";
    this.attempts = attempts;
    this.validationErrors = last?.errors ?? [];
    this.lastText = last?.text ?? "";
  }
}

const ajv = new Ajv({
  allErrors: true,
  coerceTypes: false,
  removeAdditional: false,
  useDefaults: false,
  strict: false,
  validateSchema: true,
});

export function compileStructuredSchema<T>(schema: StructuredOutputSchema<T>): ValidateFunction<T> {
  try {
    return ajv.compile<T>(schema as AnySchema);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new HarnessError("structured_schema_invalid", message);
  }
}

export type StructuredValidationResult<T> =
  | { ok: true; data: T; errors: [] }
  | { ok: false; data?: undefined; errors: StructuredValidationIssue[] };

export function validateStructuredText<T>(
  text: string,
  validate: ValidateFunction<T>,
): StructuredValidationResult<T> {
  const parsed = parseJsonFromText(text);
  if (!parsed.ok) return { ok: false, errors: [parsed.error] };

  if (validate(parsed.value)) {
    return { ok: true, data: parsed.value as T, errors: [] };
  }

  return { ok: false, errors: normalizeAjvErrors(validate.errors ?? []) };
}

export function buildStructuredPrompt<T>(content: string, schema: StructuredOutputSchema<T>): string {
  return `${content}\n\n${structuredInstructions(schema)}`;
}

export function buildStructuredCorrectionPrompt<T>(
  schema: StructuredOutputSchema<T>,
  attempt: StructuredOutputAttempt,
): string {
  return [
    "Your previous response did not satisfy the required structured-output contract.",
    "Return a corrected response as JSON only, with no markdown, prose, code fences, or comments.",
    "It must validate against this JSON Schema:",
    "```json",
    stableStringify(schema),
    "```",
    "Validation errors:",
    ...attempt.errors.map((issue) => `- ${formatIssue(issue)}`),
    "Previous response:",
    "```",
    truncate(attempt.text, 4_000),
    "```",
  ].join("\n");
}

export function assertRetryCount(maxRetries: number): void {
  if (!Number.isSafeInteger(maxRetries) || maxRetries < 0) {
    throw new HarnessError("invalid_request", "maxRetries must be a non-negative safe integer");
  }
}

export function formatIssue(issue: StructuredValidationIssue): string {
  const path = issue.path || "/";
  return `${path} ${issue.message}`;
}

function structuredInstructions<T>(schema: StructuredOutputSchema<T>): string {
  return [
    "Return the answer as JSON only, with no markdown, prose, code fences, or comments.",
    "The JSON value must validate against this JSON Schema:",
    "```json",
    stableStringify(schema),
    "```",
  ].join("\n");
}

function normalizeAjvErrors(errors: ErrorObject[]): StructuredValidationIssue[] {
  return errors.map((error) => ({
    path: error.instancePath || "/",
    keyword: error.keyword,
    message: error.message ?? `failed ${error.keyword} validation`,
    params: error.params as Record<string, unknown>,
  }));
}

function parseJsonFromText(text: string):
  | { ok: true; value: unknown }
  | { ok: false; error: StructuredValidationIssue } {
  const candidates = jsonCandidates(text);
  let lastError = "input was empty";
  for (const candidate of candidates) {
    try {
      return { ok: true, value: JSON.parse(candidate) };
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
  }
  return {
    ok: false,
    error: {
      path: "/",
      keyword: "parse",
      message: `invalid JSON: ${lastError}`,
    },
  };
}

function jsonCandidates(text: string): string[] {
  const trimmed = text.trim();
  if (!trimmed) return [];

  const candidates = [trimmed];
  const fenced = /^```(?:json)?\s*([\s\S]*?)\s*```$/i.exec(trimmed);
  if (fenced?.[1]) candidates.push(fenced[1].trim());

  const container = firstJsonContainer(trimmed);
  if (container && !candidates.includes(container)) candidates.push(container);
  return candidates;
}

function firstJsonContainer(text: string): string | undefined {
  const start = text.search(/[\[{]/);
  if (start < 0) return undefined;

  const stack: string[] = [];
  let inString = false;
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    const char = text[index];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === '"') {
        inString = false;
      }
      continue;
    }

    if (char === '"') {
      inString = true;
      continue;
    }
    if (char === "{" || char === "[") {
      stack.push(char === "{" ? "}" : "]");
      continue;
    }
    if (char === "}" || char === "]") {
      if (stack.pop() !== char) return undefined;
      if (stack.length === 0) return text.slice(start, index + 1);
    }
  }
  return undefined;
}

function stableStringify(value: unknown): string {
  return JSON.stringify(sortJson(value), null, 2);
}

function sortJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJson);
  if (!value || typeof value !== "object") return value;
  const record = value as Record<string, unknown>;
  return Object.fromEntries(Object.keys(record).sort().map((key) => [key, sortJson(record[key])]));
}

function truncate(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  return `${value.slice(0, maxChars)}\n… truncated ${value.length - maxChars} chars`;
}
