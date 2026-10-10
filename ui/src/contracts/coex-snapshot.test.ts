import { describe, expect, it } from 'vitest';
import { decodeCoexSnapshot, type CoexSnapshot } from './coex-snapshot';
import { maskRustComments, moduleSource } from './rust-source.test-support';

const known = <T,>(value: T): { status: 'known'; value: T } => ({ status: 'known', value });
const unknown = (reason = 'source unreadable'): { status: 'unknown'; reason: string } => ({ status: 'unknown', reason });
export function snapshotFixture(): CoexSnapshot {
  return {
    schemaVersion: 1, platform: 'linux', objects: known([{
      interface: 'tun0', tunnel: known(false), virtualization: unknown(), stableIdentity: known(null),
      addresses: known([{ address: '10.77.2.9', prefixLen: 24 }]),
      routes: known([{ prefix: '0.0.0.0/1', table: known('100'), scope: known('interfaceScoped'), role: known('resourceClaim') }]),
      policyRules: known([{ priority: 120, lookupTable: known('100'), addressFamily: known('ipv6'),
        selectorScope: known({ kind: 'limited', selector: 'fwmark 0x2' }), appliesToObject: known(false) }]),
    }]),
    observation: known({ elapsedMillis: 1, atomic: false }), commandCleanup: known('noRetainedSnapshotCommand'),
    context: { observationPhase: unknown(), ownInterfaces: unknown(), criteria: unknown(), repairHistory: unknown() },
    classification: unknown('classification not performed'),
  };
}
function objects(v: CoexSnapshot) {
  if (v.objects.status !== 'known') throw Error('fixture objects missing');
  return v.objects.value;
}

describe('COEX v1 strict observation boundary', () => {
  it('preserves nested known/unknown, false and null without synthetic context', () => {
    expect(decodeCoexSnapshot(snapshotFixture())).toEqual(snapshotFixture());
    const v = snapshotFixture(); objects(v)[0].addresses = known([]); objects(v)[0].routes = unknown('permission denied');
    expect(decodeCoexSnapshot(v)).toEqual(v);
  });
  it.each(['darwin', 'win32', 'android', 'ios', 'other'] as const)('keeps unavailable %s as Unknown, not empty', (platform) => {
    const v = snapshotFixture(); v.platform = platform; v.objects = unknown('collector unavailable'); v.observation = unknown(); v.commandCleanup = unknown();
    expect(decodeCoexSnapshot(v)).toEqual(v);
  });
  it.each([undefined, null, [], {}, { success: true }, { schemaVersion: 2 }])('rejects absent or undecoded transport %j', (v) => {
    expect(() => decodeCoexSnapshot(v)).toThrow('Invalid COEX');
  });
  it('rejects every missing root and every unknown context replacement', () => {
    for (const key of Object.keys(snapshotFixture())) {
      const v = snapshotFixture() as unknown as Record<string, unknown>; delete v[key];
      expect(() => decodeCoexSnapshot(v), key).toThrow();
    }
    for (const key of Object.keys(snapshotFixture().context)) {
      const v = snapshotFixture(); (v.context as unknown as Record<string, unknown>)[key] = known([]);
      expect(() => decodeCoexSnapshot(v), key).toThrow();
    }
    expect(() => decodeCoexSnapshot({ ...snapshotFixture(), classification: known('safe') })).toThrow();
  });
  it('rejects missing nested fields, wrong unions, unsafe numbers and extra fields', () => {
    const objectKeys = Object.keys(objects(snapshotFixture())[0]);
    for (const key of objectKeys) {
      const v = snapshotFixture(); delete (objects(v)[0] as unknown as Record<string, unknown>)[key];
      expect(() => decodeCoexSnapshot(v), key).toThrow();
    }
    const invalidObjects = [
      { tunnel: known(0) }, { virtualization: { status: 'unknown' } }, { addresses: known(null) },
      { stableIdentity: known([]) }, { addresses: known(new Array(2)) }, { addresses: known([{ address: '::1', prefixLen: 129 }]) },
      { routes: known([{ prefix: '::/0', table: known(100), scope: known('global'), role: known('resourceClaim') }]) },
      { routes: known([{ prefix: '::/0', table: unknown(), scope: known('all'), role: known('fullVPN') }]) },
      { policyRules: known([{ priority: -1, lookupTable: unknown(), addressFamily: known('ipv4'), selectorScope: known({ kind: 'global' }), appliesToObject: known(true) }]) },
      { policyRules: known([{ priority: 1, lookupTable: unknown(), addressFamily: known('ipv4'), selectorScope: known({ kind: 'limited' }), appliesToObject: known(true) }]) },
      { policyRules: known([{ priority: 1, lookupTable: unknown(), addressFamily: known('ipv4'), selectorScope: known({ kind: 'global', selector: 'borrowed' }), appliesToObject: known(true) }]) },
    ];
    for (const patch of invalidObjects) {
      const v = snapshotFixture(); Object.assign(objects(v)[0], patch); expect(() => decodeCoexSnapshot(v)).toThrow();
    }
    for (const patch of [{ schemaVersion: 2 }, { platform: 'macos' }, { future: true },
      { observation: known({ elapsedMillis: Number.MAX_SAFE_INTEGER + 1, atomic: false }) },
      { observation: known({ elapsedMillis: 1, atomic: true }) }, { commandCleanup: known('hostNoOwner') }]) {
      expect(() => decodeCoexSnapshot({ ...snapshotFixture(), ...patch })).toThrow();
    }
  });
  it('rejects removal of every required nested wire field, including Fact tags and payloads', () => {
    const paths: string[][] = [];
    function visit(value: unknown, path: string[]) {
      if (Array.isArray(value)) value.forEach((row, index) => visit(row, [...path, String(index)]));
      else if (value !== null && typeof value === 'object') for (const [key, row] of Object.entries(value)) {
        paths.push([...path, key]); visit(row, [...path, key]);
      }
    }
    visit(snapshotFixture(), []);
    expect(paths.length).toBeGreaterThan(60);
    for (const path of paths) {
      const value = structuredClone(snapshotFixture());
      let parent: unknown = value;
      for (const key of path.slice(0, -1)) parent = (parent as Record<string, unknown>)[key];
      delete (parent as Record<string, unknown>)[path[path.length - 1]];
      expect(() => decodeCoexSnapshot(value), path.join('.')).toThrow();
    }
  });
  it('checks each actual Rust JSON nesting level separately, not just the union of keys', () => {
    const source = maskRustComments(moduleSource('src-tauri/src/commands/coexistence_snapshot'));
    // The frozen producer writes json! objects. Scan balanced braces while protecting
    // strings; unknown syntax/absent or ambiguous anchors fails rather than skipping.
    function keysAfter(marker: string, direct = false) {
      const start = source.indexOf(marker);
      expect(start, marker).toBeGreaterThanOrEqual(0);
      expect(source.indexOf(marker, start + marker.length), `ambiguous ${marker}`).toBe(-1);
      const brace = direct ? source.indexOf('{', start) : source.indexOf('json!({', start) + 6;
      expect(brace).toBeGreaterThan(start); expect(source[brace]).toBe('{');
      let depth = 0; const keys: string[] = [];
      for (let i = brace; i < source.length; i++) {
        const ch = source[i];
        if (ch === '"') {
          let end = i + 1;
          for (; end < source.length; end++) {
            if (source[end] === '\\') end++;
            else if (source[end] === '"') break;
          }
          expect(end).toBeLessThan(source.length);
          if (depth === 1 && /^\s*:/.test(source.slice(end + 1))) keys.push(source.slice(i + 1, end));
          i = end;
        } else if (ch === '{') depth++;
        else if (ch === '}' && --depth === 0) { expect(keys.length).toBeGreaterThan(0); return keys.sort(); }
      }
      throw Error(`unclosed Rust JSON object ${marker}`);
    }
    const snapshot = snapshotFixture(); const object = objects(snapshot)[0];
    expect(keysAfter('fn snapshot_wire(')).toEqual(Object.keys(snapshot).sort());
    expect(keysAfter('fn object_wire(')).toEqual(Object.keys(object).sort());
    expect(keysAfter('"context": {', true)).toEqual(Object.keys(snapshot.context).sort());
    expect(keysAfter('"addresses": fact_wire')).toEqual(['address', 'prefixLen']);
    expect(keysAfter('"routes": fact_wire')).toEqual(['prefix', 'role', 'scope', 'table']);
    expect(keysAfter('"policyRules": fact_wire')).toEqual(['addressFamily', 'appliesToObject', 'lookupTable', 'priority', 'selectorScope']);
    expect(keysAfter('PolicySelectorScope::Global =>')).toEqual(['kind']);
    expect(keysAfter('PolicySelectorScope::Limited(selector) =>')).toEqual(['kind', 'selector']);
    expect(keysAfter('fn unknown_wire(')).toEqual(['reason', 'status']);
    expect(keysAfter('fn fact_wire<T>')).toEqual(['status', 'value']);
    expect(source).toMatch(/Fact::Known\("noRetainedSnapshotCommand"\)/);
    expect(source).toMatch(/"elapsedMillis": elapsed, "atomic": false/);
  });
  it('tracks all emitted JSON keys, schema and enum strings in the actual Rust producer', () => {
    const source = maskRustComments(moduleSource('src-tauri/src/commands/coexistence_snapshot'));
    function body(name: string): string {
      const start = source.indexOf(`fn ${name}`); expect(start, name).toBeGreaterThanOrEqual(0);
      const next = source.indexOf('\nfn ', start + 1);
      return source.slice(start, next < 0 ? source.length : next);
    }
    const keys = new Set<string>();
    for (const name of ['fact_wire', 'unknown_wire', 'object_wire', 'snapshot_wire', 'collect_blocking']) {
      for (const match of body(name).matchAll(/"([A-Za-z][A-Za-z0-9]*)"\s*:/g)) keys.add(match[1]);
    }
    function allKeys(value: unknown, out = new Set<string>()): Set<string> {
      if (Array.isArray(value)) value.forEach((row) => allKeys(row, out));
      else if (value !== null && typeof value === 'object') for (const [key, row] of Object.entries(value)) { out.add(key); allKeys(row, out); }
      return out;
    }
    expect([...keys].sort()).toEqual([...allKeys(snapshotFixture())].sort());
    const version = [...body('snapshot_wire').matchAll(/"schemaVersion"\s*:\s*(\d+)/g)];
    expect(version).toHaveLength(1); expect(decodeCoexSnapshot(snapshotFixture()).schemaVersion).toBe(Number(version[0][1]));
    for (const [enumName, candidates] of [
      ['RouteScope', ['global', 'interfaceScoped']], ['RouteRole', ['coverageDeclaration', 'resourceClaim']],
      ['AddressFamily', ['ipv4', 'ipv6']],
    ] as const) {
      const emitted = [...body('object_wire').matchAll(new RegExp(`${enumName}::\\w+ => "([^"]+)"`, 'g'))].map((m) => m[1]);
      expect(emitted.sort(), enumName).toEqual([...candidates].sort());
    }
    const platforms = [...body('platform_tag').matchAll(/Platform::\w+ => "([^"]+)"/g)].map((m) => m[1]);
    expect(platforms.sort()).toEqual(['linux', 'darwin', 'win32', 'android', 'ios', 'other'].sort());
    expect(body('collect_request')).toContain('if platform != Platform::Linux {');
    expect(body('collect_request')).toContain('production COEX snapshot collector unavailable on this platform');
  });
});
