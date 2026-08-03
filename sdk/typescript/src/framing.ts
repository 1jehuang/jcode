/** NDJSON line framing over a byte stream. */

/**
 * Incremental newline-delimited JSON decoder.
 *
 * Sockets split frames anywhere, so decoding per chunk is wrong; this keeps a
 * remainder buffer and only emits complete lines. Blank lines are skipped,
 * matching the Rust reader.
 */
export class NdjsonDecoder {
  private buffer = "";

  push(chunk: Buffer | string): unknown[] {
    this.buffer += typeof chunk === "string" ? chunk : chunk.toString("utf8");
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
