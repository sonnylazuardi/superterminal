import { describe, expect, test } from 'bun:test';
import { join } from 'node:path';
import {
  clientStatePath,
  createClientStatePersister,
  EMPTY_CLIENT_STATE,
  loadClientState,
  parseClientState,
  serializeClientState,
  type ClientState,
} from './client-state.js';

const state = (
  window: ClientState['window'],
  verticalTabs: boolean | null,
  sidebarWidth: number | null = null,
): ClientState => ({
  window,
  verticalTabs,
  sidebarWidth,
});

describe('clientStatePath', () => {
  test('lives beside the daemon state on Linux and under LOCALAPPDATA on Windows', () => {
    expect(clientStatePath({ env: { XDG_STATE_HOME: '/state' } })).toBe(
      '/state/superterminal/client.json',
    );
    expect(
      clientStatePath({ platform: 'win32', env: { LOCALAPPDATA: 'C:\\Users\\x\\AppData\\Local' } }),
    ).toBe(join('C:\\Users\\x\\AppData\\Local', 'superterminal', 'client.json'));
  });
});

describe('parseClientState', () => {
  test('round-trips what serializeClientState writes', () => {
    const original = state({ width: 1017, height: 655 }, false, 260);
    expect(parseClientState(serializeClientState(original))).toEqual({
      state: original,
      warnings: [],
    });
  });

  test('omits unknown fields from the file rather than writing nulls', () => {
    expect(JSON.parse(serializeClientState(EMPTY_CLIENT_STATE))).toEqual({ version: 1 });
    expect(JSON.parse(serializeClientState(state(null, true)))).toEqual({
      version: 1,
      verticalTabs: true,
    });
  });

  test('a corrupt file yields the empty state with one warning, never a throw', () => {
    const { state: parsed, warnings } = parseClientState('{ not json');
    expect(parsed).toEqual(EMPTY_CLIENT_STATE);
    expect(warnings).toHaveLength(1);
    expect(parseClientState('[1,2]').state).toEqual(EMPTY_CLIENT_STATE);
    expect(parseClientState('"x"').state).toEqual(EMPTY_CLIENT_STATE);
  });

  test('a bad window size is dropped field-wise; the layout survives', () => {
    for (const bad of [
      '{"window":{"width":-1,"height":600},"verticalTabs":false}',
      '{"window":{"width":"800","height":600},"verticalTabs":false}',
      '{"window":{"width":99999,"height":600},"verticalTabs":false}',
      '{"window":{"width":800},"verticalTabs":false}',
    ]) {
      const { state: parsed, warnings } = parseClientState(bad);
      expect(parsed).toEqual(state(null, false));
      expect(warnings).toHaveLength(1);
    }
  });

  test('a sidebar width outside its bounds is dropped; a fractional one rounds', () => {
    expect(parseClientState('{"sidebarWidth":100}')).toEqual({
      state: EMPTY_CLIENT_STATE,
      warnings: ['[superterminal] client state: remembered sidebar width ignored'],
    });
    expect(parseClientState('{"sidebarWidth":"220"}').state.sidebarWidth).toBeNull();
    expect(parseClientState('{"sidebarWidth":250.4}').state.sidebarWidth).toBe(250);
    expect(JSON.parse(serializeClientState(state(null, null, 300)))).toEqual({
      version: 1,
      sidebarWidth: 300,
    });
  });

  test('an unknown verticalTabs type is dropped without touching the size', () => {
    const { state: parsed, warnings } = parseClientState(
      '{"window":{"width":800,"height":600},"verticalTabs":"yes"}',
    );
    expect(parsed).toEqual(state({ width: 800, height: 600 }, null));
    expect(warnings).toHaveLength(1);
  });
});

describe('loadClientState', () => {
  test('a missing file is the normal first run: empty state, no warning', () => {
    const missing = () => {
      throw Object.assign(new Error('ENOENT'), { code: 'ENOENT' });
    };
    const result = loadClientState({ path: '/nowhere/client.json', readFile: missing });
    expect(result).toEqual({ state: EMPTY_CLIENT_STATE, path: '/nowhere/client.json', warnings: [] });
  });

  test('any other read error is reported', () => {
    const denied = () => {
      throw Object.assign(new Error('EACCES'), { code: 'EACCES' });
    };
    expect(loadClientState({ path: '/x', readFile: denied }).warnings).toHaveLength(1);
  });
});

describe('createClientStatePersister', () => {
  test('debounces, skips no-ops and flushes the last value on demand', () => {
    const writes: ClientState[] = [];
    const initial = state({ width: 800, height: 600 }, true);
    const persister = createClientStatePersister({
      path: '/tmp/client.json',
      initial,
      debounceMs: 60_000, // never fires inside the test; flush() drives it
      write: (_path, s) => writes.push(s),
    });

    persister.push(initial); // identical to what is on disk
    persister.flush();
    expect(writes).toEqual([]);

    persister.push(state({ width: 900, height: 600 }, true));
    persister.push(state({ width: 1000, height: 700 }, true));
    persister.flush();
    expect(writes).toEqual([state({ width: 1000, height: 700 }, true)]);

    persister.push(state({ width: 1000, height: 700 }, false));
    persister.push(state({ width: 1000, height: 700 }, true)); // back to what was written
    persister.flush();
    expect(writes).toHaveLength(1);

    persister.push(state({ width: 1000, height: 700 }, false));
    persister.stop();
    persister.flush();
    expect(writes).toHaveLength(1);
  });

  test('a failed write is reported and retried on the next change', () => {
    let fail = true;
    const writes: ClientState[] = [];
    const errors: unknown[] = [];
    const persister = createClientStatePersister({
      path: '/tmp/client.json',
      initial: EMPTY_CLIENT_STATE,
      write: (_path, s) => {
        if (fail) throw new Error('disk full');
        writes.push(s);
      },
      onError: (err) => errors.push(err),
    });
    persister.push(state(null, false));
    persister.flush();
    expect(errors).toHaveLength(1);
    expect(writes).toEqual([]);

    fail = false;
    persister.push(state(null, true));
    persister.flush();
    expect(writes).toEqual([state(null, true)]);
  });
});
