/**
 * Socket path resolution, mirroring `jcode-harness-api::sockets`.
 *
 * The bridge and every client must resolve the same directory or nothing can
 * connect, so the rules here follow the Rust module exactly.
 */

import os from "node:os";
import path from "node:path";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

export function runtimeDir(): string {
  const explicit = process.env.JCODE_RUNTIME_DIR;
  if (explicit) return explicit;
  const xdg = process.env.XDG_RUNTIME_DIR;
  if (xdg) return xdg;
  if (process.platform === "darwin" && process.env.TMPDIR) {
    return process.env.TMPDIR;
  }
  return path.join(os.tmpdir(), `jcode-${userDiscriminator()}`);
}

function userDiscriminator(): string {
  const raw =
    process.platform === "win32"
      ? (process.env.USERNAME ?? process.env.USER)
      : (process.env.UID ?? process.env.USER);
  return sanitize(raw ?? "user");
}

function sanitize(raw: string): string {
  const out = raw
    .split("")
    .filter((ch) => /[A-Za-z0-9\-_]/.test(ch))
    .slice(0, 64)
    .join("");
  return out === "" ? "user" : out;
}

/**
 * Endpoint the SDK dials for a given socket path.
 *
 * On Unix that is the path itself. On Windows there is no socket file: the
 * bridge publishes a named pipe, and the name it actually published is
 * recorded in a sidecar next to the path. See `transportEndpointCandidates`.
 * The untagged derivation still mirrors `jcode-transport` exactly, and the
 * two are pinned together by a test on each side.
 */
export function transportEndpoint(socketPath: string): string {
  return transportEndpointCandidates(socketPath)[0];
}

/**
 * Candidate endpoints for a socket path, most specific first.
 *
 * A Windows server records the pipe name it actually published in a
 * `<socketPath>.pipe-name` sidecar, because the name now also encodes the
 * server's integrity level (an elevated server publishes a `-high-` name) and
 * may be a private fallback name when the shared name was denied. Node cannot
 * read its own integrity level, so the sidecar, not a derivation, is the only
 * way an elevated client finds an elevated server.
 */
export function transportEndpointCandidates(socketPath: string): string[] {
  if (process.platform !== "win32") return [socketPath];

  const names: string[] = [];
  const recorded = recordedPipeName(socketPath);
  if (recorded) names.push(recorded);
  const derived = derivedPipeName(socketPath);
  if (!names.includes(derived)) names.push(derived);
  return names;
}

function recordedPipeName(socketPath: string): string | null {
  try {
    const recorded = readFileSync(`${socketPath}.pipe-name`, "utf8").trim();
    return recorded.startsWith("\\\\.\\pipe\\") ? recorded : null;
  } catch {
    return null;
  }
}

/**
 * The historical derivation. An untagged, medium-integrity server still
 * publishes exactly this name, so it stays the fallback candidate.
 */
function derivedPipeName(socketPath: string): string {
  // Same rule as the Rust side: a readable stem for diagnosis, plus a hash of
  // the normalized path so two different paths never collide.
  const stem =
    (path.parse(socketPath).name.match(/[A-Za-z0-9\-_]/g) ?? []).join("").slice(0, 32) || "jcode";
  const normalized = socketPath.replace(/\\/g, "/").toLowerCase();
  const hash = createHash("sha256").update(normalized).digest("hex").slice(0, 16);
  return `\\\\.\\pipe\\${stem}-${hash}`;
}

/** Path of the versioned harness API socket. `JCODE_API_SOCKET` overrides. */
export function apiSocketPath(): string {
  return process.env.JCODE_API_SOCKET ?? path.join(runtimeDir(), "jcode-api.sock");
}

/** Path of the internal daemon socket. `JCODE_SOCKET` overrides. */
export function legacySocketPath(): string {
  return process.env.JCODE_SOCKET ?? path.join(runtimeDir(), "jcode.sock");
}
