/**
 * Schema parity: the SDK's tag sets must match the Rust API crate.
 *
 * The SDK is hand-written, so the realistic failure mode is the Rust surface
 * growing a variant that TypeScript never learns about. This test reads the
 * Rust source directly and fails the moment the two drift, which is the only
 * check that catches an additive change nobody remembered to mirror.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  API_VERSION_MAJOR,
  KNOWN_EVENT_KINDS,
  KNOWN_REQUEST_KINDS,
} from "../dist/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const rustCrate = path.resolve(here, "../../../crates/jcode-harness-api/src");

function snakeCase(variant: string): string {
  return variant.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
}

/** Variant names of the top-level enum in a Rust file, minus `Unknown`. */
function enumVariants(file: string, enumName: string): string[] {
  const source = fs.readFileSync(path.join(rustCrate, file), "utf8");
  const start = source.indexOf(`pub enum ${enumName} {`);
  assert.notEqual(start, -1, `enum ${enumName} not found in ${file}`);
  // Stop at the enum's closing brace (column 0) so later enums in the same
  // file, such as ErrorCode, cannot leak into the comparison.
  const rest = source.slice(start);
  const end = rest.indexOf("\n}");
  const body = end === -1 ? rest : rest.slice(0, end);
  const variants: string[] = [];
  // Variants are at one indent level inside the enum body.
  for (const line of body.split("\n")) {
    const match = /^ {4}([A-Z][A-Za-z0-9]*)\s*[{(,]/.exec(line);
    if (match && match[1] !== "Unknown") variants.push(snakeCase(match[1]));
  }
  return variants;
}

test("request tags match the Rust ApiRequest enum", () => {
  const rust = enumVariants("requests.rs", "ApiRequest").sort();
  assert.deepEqual([...KNOWN_REQUEST_KINDS].sort(), rust);
});

test("event tags match the Rust ApiEvent enum", () => {
  const rust = enumVariants("events.rs", "ApiEvent").sort();
  assert.deepEqual([...KNOWN_EVENT_KINDS].sort(), rust);
});

test("send_message no_reply field matches the Rust schema", () => {
  const rust = fs.readFileSync(path.join(rustCrate, "requests.rs"), "utf8");
  const typescript = fs.readFileSync(path.resolve(here, "../src/protocol.ts"), "utf8");
  assert.match(rust, /SendMessage\s*{[\s\S]*?no_reply:\s*bool/);
  assert.match(typescript, /req:\s*"send_message"[\s\S]*?no_reply\?:\s*boolean/);
});

test("protocol major version matches the Rust constant", () => {
  const source = fs.readFileSync(path.join(rustCrate, "lib.rs"), "utf8");
  const match = /API_VERSION_MAJOR: u32 = (\d+)/.exec(source);
  assert.ok(match, "API_VERSION_MAJOR not found");
  assert.equal(API_VERSION_MAJOR, Number(match![1]));
});

test("socket path rules match the Rust resolver", async () => {
  const source = fs.readFileSync(path.join(rustCrate, "sockets.rs"), "utf8");
  for (const key of ["JCODE_RUNTIME_DIR", "XDG_RUNTIME_DIR", "JCODE_API_SOCKET", "JCODE_SOCKET"]) {
    assert.ok(source.includes(key), `${key} missing from Rust resolver`);
  }
  const sdk = fs.readFileSync(path.resolve(here, "../src/sockets.ts"), "utf8");
  for (const key of ["JCODE_RUNTIME_DIR", "XDG_RUNTIME_DIR", "JCODE_API_SOCKET", "JCODE_SOCKET"]) {
    assert.ok(sdk.includes(key), `${key} missing from SDK resolver`);
  }
  assert.ok(source.includes("jcode-api.sock") && sdk.includes("jcode-api.sock"));
});
