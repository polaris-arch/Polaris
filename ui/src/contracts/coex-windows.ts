/** Windows v2 preserves independent sources; no object join or classifier. */
import type { Fact, UnknownFact, CoexSnapshotV1 } from './coex-snapshot';
export interface WindowsReference { alias: Fact<string>; luid: Fact<string>; ifIndex: Fact<number> }
export interface WindowsAdapter { interface: WindowsReference; ifType: Fact<number>; description: Fact<string> }
export interface WindowsAddress { interface: WindowsReference; family: 'ipv4' | 'ipv6'; address: string; prefixLen: Fact<number>; scopeId: Fact<number> }
export interface WindowsRoute { interface: WindowsReference; family: 'ipv4' | 'ipv6'; prefix: string; nextHop: Fact<string>; nextHopScopeId: Fact<number>; routeMetric: Fact<number>; interfaceMetric: Fact<number> }
export interface WindowsRas { name: Fact<string>; allUsers: boolean; interface: Fact<WindowsReference> }
export interface WindowsSource<T> { rows: Fact<T[]>; complete: Fact<boolean>; compartment: Fact<number>; error: Fact<string | null> }
export interface WindowsSources { adapters: WindowsSource<WindowsAdapter>; addresses: WindowsSource<WindowsAddress>; routes4: WindowsSource<WindowsRoute>; routes6: WindowsSource<WindowsRoute>; ras: WindowsSource<WindowsRas> }
export interface CoexWindowsSnapshot {
  schemaVersion: 2; platform: 'win32'; sources: WindowsSources; objects: UnknownFact;
  observation: CoexSnapshotV1['observation']; commandCleanup: UnknownFact;
  context: CoexSnapshotV1['context']; classification: UnknownFact;
}

export function decodeCoexWindows(input: unknown, invalid: () => never): CoexWindowsSnapshot {
  type Obj = Record<string, unknown>; type Decode<T> = (v: unknown) => T;
  const fields = (v: unknown, keys: string[]): Obj => {
    if (v === null || typeof v !== 'object' || Array.isArray(v)) return invalid();
    const obj = v as Obj;
    if (Object.keys(obj).length !== keys.length || keys.some((k) => !Object.prototype.hasOwnProperty.call(obj, k))) return invalid();
    return obj;
  };
  const text: Decode<string> = (v) => typeof v === 'string' && new TextEncoder().encode(v).length <= 1024 * 1024 ? v : invalid();
  const number = (max = 0xffffffff): Decode<number> => (v) => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= max ? v : invalid();
  const boolean: Decode<boolean> = (v) => typeof v === 'boolean' ? v : invalid();
  const family: Decode<'ipv4' | 'ipv6'> = (v) => v === 'ipv4' || v === 'ipv6' ? v : invalid();
  const unknown = (v: unknown): UnknownFact => { const o = fields(v, ['status', 'reason']); if (o.status !== 'unknown') return invalid(); return { status: 'unknown', reason: text(o.reason) }; };
  const fact = <T,>(decode: Decode<T>): Decode<Fact<T>> => (v) => {
    if (v !== null && typeof v === 'object' && (v as Obj).status === 'unknown') return unknown(v);
    const o = fields(v, ['status', 'value']); if (o.status !== 'known') return invalid(); return { status: 'known', value: decode(o.value) };
  };
  const luid: Decode<string> = (v) => { const value = text(v); return /^[1-9][0-9]{0,19}$/.test(value) && BigInt(value) <= 0xffffffffffffffffn ? value : invalid(); };
  const reference = (v: unknown, adapter = false): WindowsReference => { const o = fields(v, ['alias', 'luid', 'ifIndex']); return { alias: fact(text)(o.alias), luid: fact(luid)(o.luid), ifIndex: adapter ? unknown(o.ifIndex) : fact((x) => { const index = number()(x); return index > 0 ? index : invalid(); })(o.ifIndex) }; };
  const bareAddress = (v: unknown, f: 'ipv4' | 'ipv6'): string => {
    const value = text(v); if (value.includes('%')) return invalid();
    if (f === 'ipv4') { const parts = value.split('.'); if (parts.length !== 4 || parts.some((p) => !/^(0|[1-9][0-9]{0,2})$/.test(p) || Number(p) > 255)) return invalid(); }
    else { if (!/^[0-9a-f:]+$/i.test(value) || !value.includes(':') || value.includes(':::') || (value.startsWith(':') && !value.startsWith('::')) || (value.endsWith(':') && !value.endsWith('::'))) return invalid(); const groups = value.split(':').filter(Boolean); if (groups.length > 8 || groups.some((g) => g.length > 4) || (value.match(/::/g)?.length ?? 0) > 1 || (value.includes('::') ? groups.length >= 8 : groups.length !== 8)) return invalid(); }
    return value;
  };
  const source = <T,>(v: unknown, row: Decode<T>, max: number): WindowsSource<T> => {
    const o = fields(v, ['rows', 'complete', 'compartment', 'error']);
    if (new TextEncoder().encode(JSON.stringify(v)).length > 8 * 1024 * 1024) return invalid();
    const result = { rows: fact((rows) => { if (!Array.isArray(rows) || rows.length > max) return invalid(); return Array.from(rows, row); })(o.rows), complete: fact(boolean)(o.complete), compartment: fact((x) => { const id = number()(x); return id > 0 ? id : invalid(); })(o.compartment), error: fact((x) => x === null ? null : text(x))(o.error) };
    if (result.complete.status === 'known' && result.complete.value && (result.rows.status !== 'known' || result.compartment.status !== 'known' || result.error.status !== 'known' || result.error.value !== null)) return invalid();
    return result;
  };
  const adapter: Decode<WindowsAdapter> = (v) => { const o = fields(v, ['interface', 'ifType', 'description']); return { interface: reference(o.interface, true), ifType: fact(number())(o.ifType), description: fact(text)(o.description) }; };
  const address: Decode<WindowsAddress> = (v) => {
    const o = fields(v, ['interface', 'family', 'address', 'prefixLen', 'scopeId']); const f = family(o.family);
    return { interface: reference(o.interface), family: f, address: bareAddress(o.address, f), prefixLen: fact((x) => { const bits = number(f === 'ipv4' ? 32 : 128)(x); return bits > 0 ? bits : invalid(); })(o.prefixLen), scopeId: f === 'ipv4' ? unknown(o.scopeId) : fact(number())(o.scopeId) };
  };
  const route = (expected: 'ipv4' | 'ipv6'): Decode<WindowsRoute> => (v) => {
    const o = fields(v, ['interface', 'family', 'prefix', 'nextHop', 'nextHopScopeId', 'routeMetric', 'interfaceMetric']); const f = family(o.family); if (f !== expected) return invalid();
    const prefix = text(o.prefix); const parts = prefix.split('/'); if (parts.length !== 2 || !/^(0|[1-9][0-9]{0,2})$/.test(parts[1]) || Number(parts[1]) > (f === 'ipv4' ? 32 : 128)) return invalid(); bareAddress(parts[0], f);
    return { interface: reference(o.interface), family: f, prefix, nextHop: fact((x) => bareAddress(x, f))(o.nextHop), nextHopScopeId: f === 'ipv4' ? unknown(o.nextHopScopeId) : fact(number())(o.nextHopScopeId), routeMetric: fact(number())(o.routeMetric), interfaceMetric: fact(number())(o.interfaceMetric) };
  };
  const ras: Decode<WindowsRas> = (v) => { const o = fields(v, ['name', 'allUsers', 'interface']); return { name: fact((x) => { const name = text(x); return name.trimStart().startsWith('.') ? invalid() : name; })(o.name), allUsers: boolean(o.allUsers), interface: fact((x) => reference(x))(o.interface) }; };
  const o = fields(input, ['schemaVersion', 'platform', 'sources', 'objects', 'observation', 'commandCleanup', 'context', 'classification']); if (o.schemaVersion !== 2 || o.platform !== 'win32') return invalid();
  const s = fields(o.sources, ['adapters', 'addresses', 'routes4', 'routes6', 'ras']); const c = fields(o.context, ['observationPhase', 'ownInterfaces', 'criteria', 'repairHistory']);
  return {
    schemaVersion: 2, platform: 'win32', objects: unknown(o.objects), sources: { adapters: source(s.adapters, adapter, 128), addresses: source(s.addresses, address, 128 * 256), routes4: source(s.routes4, route('ipv4'), 4096), routes6: source(s.routes6, route('ipv6'), 4096), ras: source(s.ras, ras, 128) },
    observation: fact((v) => { const row = fields(v, ['elapsedMillis', 'atomic']); if (row.atomic !== false) return invalid(); return { elapsedMillis: number(Number.MAX_SAFE_INTEGER)(row.elapsedMillis), atomic: false as const }; })(o.observation), commandCleanup: unknown(o.commandCleanup),
    context: { observationPhase: unknown(c.observationPhase), ownInterfaces: unknown(c.ownInterfaces), criteria: unknown(c.criteria), repairHistory: unknown(c.repairHistory) }, classification: unknown(o.classification),
  };
}
