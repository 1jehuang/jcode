/** NDJSON line framing over a byte stream. */

import { StringDecoder } from "node:string_decoder";

/**
 * Incremental newline-delimited JSON decoder.
 *
 * Sockets can split both UTF-8 characters and JSON lines across chunks. Keep
 * incomplete characters and lines until more bytes arrive. Blank lines are
 * skipped, matching the Rust reader.
 */
export class NdjsonDecoder {
  private readonly utf8 = new StringDecoder("utf8");
  private buffer = "";

  push(chunk: Buffer | string): unknown[] {
    // A string is already decoded, so flush pending bytes before appending it.
    this.buffer += typeof chunk === "string" ? this.utf8.end() + chunk : this.utf8.write(chunk);
    const out: unknown[] = [];
    let index = this.buffer.indexOf("\n");
    while (index !== -1) {
      const line = this.buffer.slice(0, index).trim();
      this.buffer = this.buffer.slice(index + 1);
      if (line !== "") out.push(JSON.parse(line));
      index = this.buffer.indexOf("\n");
    }
    return out;
  }
}

export function encodeFrame(frame: unknown): string {
  return `${JSON.stringify(frame)}\n`;
}
