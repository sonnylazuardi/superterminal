import { describe, expect, test } from 'bun:test';
import { MAX_LINE_BYTES, NdjsonDecoder, encodeFrame } from './framing.js';
import { ProtocolError } from './errors.js';

const enc = (s: string) => new TextEncoder().encode(s);

describe('NdjsonDecoder', () => {
  test('decodes one whole line', () => {
    const d = new NdjsonDecoder();
    expect(d.push(enc('{"t":"ok","id":1,"result":{}}\n'))).toEqual([
      { t: 'ok', id: 1, result: {} },
    ]);
    expect(d.pending).toBe(0);
  });

  test('decodes several messages arriving in one chunk', () => {
    const d = new NdjsonDecoder();
    const out = d.push(enc('{"t":"a"}\n{"t":"b"}\n{"t":"c"}\n'));
    expect(out).toEqual([{ t: 'a' }, { t: 'b' }, { t: 'c' }]);
  });

  test('holds a partial line until its newline arrives', () => {
    const d = new NdjsonDecoder();
    expect(d.push(enc('{"t":"ev.workspace",'))).toEqual([]);
    expect(d.pending).toBeGreaterThan(0);
    expect(d.push(enc('"revision":7'))).toEqual([]);
    expect(d.push(enc('}\n'))).toEqual([{ t: 'ev.workspace', revision: 7 }]);
    expect(d.pending).toBe(0);
  });

  test('splits a chunk that ends mid-message after a complete one', () => {
    const d = new NdjsonDecoder();
    expect(d.push(enc('{"t":"a"}\n{"t":"b'))).toEqual([{ t: 'a' }]);
    expect(d.push(enc('"}\n'))).toEqual([{ t: 'b' }]);
  });

  test('byte-at-a-time delivery still reassembles', () => {
    const d = new NdjsonDecoder();
    const bytes = enc('{"t":"ev.server_shutting_down","reason":"idle"}\n');
    const out: unknown[] = [];
    for (const b of bytes) out.push(...d.push(new Uint8Array([b])));
    expect(out).toEqual([{ t: 'ev.server_shutting_down', reason: 'idle' }]);
  });

  test('multi-byte UTF-8 split across chunks', () => {
    const d = new NdjsonDecoder();
    const bytes = enc('{"t":"ev.title","title":"héllo — ✓"}\n');
    const mid = 20;
    expect(d.push(bytes.subarray(0, mid))).toEqual([]);
    expect(d.push(bytes.subarray(mid))).toEqual([{ t: 'ev.title', title: 'héllo — ✓' }]);
  });

  test('blank keep-alive lines are ignored', () => {
    const d = new NdjsonDecoder();
    expect(d.push(enc('\n\n{"t":"a"}\n\n'))).toEqual([{ t: 'a' }]);
  });

  test('malformed JSON throws ProtocolError and clears the buffer', () => {
    const d = new NdjsonDecoder();
    expect(() => d.push(enc('not json\n'))).toThrow(ProtocolError);
    expect(d.pending).toBe(0);
    expect(d.push(enc('{"t":"a"}\n'))).toEqual([{ t: 'a' }]);
  });

  test('an over-long line aborts', () => {
    const d = new NdjsonDecoder(64);
    expect(() => d.push(enc('x'.repeat(100)))).toThrow(/exceeds the 4 MiB cap/);
  });

  test('a completed over-long line aborts', () => {
    const d = new NdjsonDecoder(16);
    expect(() => d.push(enc(`${'x'.repeat(32)}\n`))).toThrow(ProtocolError);
  });

  test('the default cap is 4 MiB', () => {
    expect(MAX_LINE_BYTES).toBe(4 * 1024 * 1024);
  });

  test('reset drops buffered bytes', () => {
    const d = new NdjsonDecoder();
    d.push(enc('{"t":"a"'));
    d.reset();
    expect(d.pending).toBe(0);
    // The fragment is gone, so the continuation is no longer valid JSON.
    expect(() => d.push(enc('}\n'))).toThrow(ProtocolError);
  });
});

describe('encodeFrame', () => {
  test('appends exactly one newline', () => {
    const frame = new TextDecoder().decode(encodeFrame({ t: 'hello' }));
    expect(frame).toBe('{"t":"hello"}\n');
  });

  test('round-trips through the decoder', () => {
    const d = new NdjsonDecoder();
    const msg = { t: 'tab.create', id: 7, session: 1, spawn: { cols: 200, rows: 60 } };
    expect(d.push(encodeFrame(msg))).toEqual([msg]);
  });
});
