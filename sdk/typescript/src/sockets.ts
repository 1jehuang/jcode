/**
 * Socket path resolution, mirroring `jcode-harness-api::sockets`.
 *
 * The bridge and every client must resolve the same directory or nothing can
 * connect, so the rules here follow the Rust module exactly.
 */

import os from "node:os";
import path from "node:path";

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

/** Path of the versioned harness API socket. `JCODE_API_SOCKET` overrides. */
export function apiSocketPath(): string {
  return process.env.JCODE_API_SOCKET ?? path.join(runtimeDir(), "jcode-api.sock");
}

/** Path of the internal daemon socket. `JCODE_SOCKET` overrides. */
export function legacySocketPath(): string {
  return process.env.JCODE_SOCKET ?? path.join(runtimeDir(), "jcode.sock");
}
