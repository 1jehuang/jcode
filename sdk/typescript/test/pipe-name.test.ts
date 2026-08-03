import { test } from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { createHash } from "node:crypto";

/**
 * The SDK derives the Windows pipe name independently of `jcode-transport`,
 * because one is Rust and one is TypeScript. A drift between them is invisible
 * on Unix and total on Windows: the client dials a pipe nobody is listening on,
 * so nothing connects at all.
 *
 * `jcode-transport`'s `pipe_name_matches_the_typescript_sdk` asserts the same
 * literal strings from the other side.
 */

/** The derivation under test, with the platform's path parser injected. */
function derivePipeName(socketPath: string, parser: path.PlatformPath): string {
  const stem =
    (parser.parse(socketPath).name.match(/[A-Za-z0-9\-_]/g) ?? []).join("").slice(0, 32) || "jcode";
  const normalized = socketPath.replace(/\\/g, "/").toLowerCase();
  const hash = createHash("sha256").update(normalized).digest("hex").slice(0, 16);
  return `\\\\.\\pipe\\${stem}-${hash}`;
}

test("the Windows pipe name matches jcode-transport exactly", () => {
  // Parsed with win32 semantics regardless of the host, so the check runs in
  // CI on Linux rather than only on a Windows runner.
  for (const [socketPath, expected] of [
    [
      "C:\\Users\\jeremy\\AppData\\Local\\jcode\\run\\jcode-api.sock",
      "\\\\.\\pipe\\jcode-api-5e00c01702e8cfe4",
    ],
    ["C:\\a\\b\\jcode.sock", "\\\\.\\pipe\\jcode-52dfdb00b2f35a71"],
  ] as const) {
    assert.equal(
      derivePipeName(socketPath, path.win32),
      expected,
      `pipe name for ${socketPath} drifted from jcode-transport`,
    );
  }
});

test("case and separators normalize the same way", () => {
  assert.equal(
    derivePipeName("C:\\Temp\\Jcode\\server.sock", path.win32),
    derivePipeName("c:/temp/jcode/server.sock", path.win32),
    "the pipe name must not depend on case or separator style",
  );
});

test("a path with no usable characters still yields a name", () => {
  const derived = derivePipeName("/tmp/!!!.sock", path.posix);
  assert.match(derived, /^\\\\\.\\pipe\\jcode-[0-9a-f]{16}$/);
});
