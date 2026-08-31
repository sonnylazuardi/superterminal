/**
 * Newline-delimited JSON framing for the CONTROL plane (02 §1, Q14).
 *
 * Kept separate from the socket so partial-chunk behaviour is unit-testable
 * without any I/O.
 */

import { ProtocolError } from './errors.js';

/** Defensive cap; control messages are small (02 §11). */
export const MAX_LINE_BYTES = 4 * 1024 * 1024;

const NEWLINE = 0x0a;

export class NdjsonDecoder {
  private buf: Uint8Array = new Uint8Array(0);
  private readonly decoder = new TextDecoder('utf-8', { fatal: false });
  private readonly maxLineBytes: number;

  constructor(maxLineBytes = MAX_LINE_BYTES) {
    this.maxLineBytes = maxLineBytes;
  }

  /** Bytes currently held back waiting for their terminating newline. */
  get pending(): number {
    return this.buf.length;
  }

  reset(): void {
    this.buf = new Uint8Array(0);
  }

  /**
   * Feed one chunk; returns every complete message it completed, in order.
   * Throws `ProtocolError` on an over-long line or a line that is not JSON —
   * both are connection-fatal for the caller.
   */
  push(chunk: Uint8Array): unknown[] {
    const merged = new Uint8Array(this.buf.length + chunk.length);
    merged.set(this.buf, 0);
    merged.set(chunk, this.buf.length);

    const out: unknown[] = [];
    let start = 0;
    for (let i = 0; i < merged.length; i++) {
      if (merged[i] !== NEWLINE) continue;
      const line = merged.subarray(start, i);
      start = i + 1;
      if (line.length > this.maxLineBytes) {
        this.buf = new Uint8Array(0);
        throw new ProtocolError(`control line of ${line.length} bytes exceeds the 4 MiB cap`);
      }
      const text = this.decoder.decode(line).trim();
      if (text.length === 0) continue; // tolerate keep-alive blank lines
      try {
        out.push(JSON.parse(text));
      } catch (err) {
        this.buf = new Uint8Array(0);
        throw new ProtocolError(`malformed control JSON: ${(err as Error).message}`);
      }
    }

    const rest = merged.subarray(start);
    if (rest.length > this.maxLineBytes) {
      this.buf = new Uint8Array(0);
      throw new ProtocolError(`control line exceeds the 4 MiB cap before any newline`);
    }
    this.buf = rest.length === 0 ? new Uint8Array(0) : new Uint8Array(rest);
    return out;
  }
}

const encoder = new TextEncoder();

/** Serialise one message as a single NDJSON frame. */
export function encodeFrame(message: unknown): Uint8Array {
  return encoder.encode(`${JSON.stringify(message)}\n`);
}
