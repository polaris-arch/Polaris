import { decodeCoexWindows, type CoexWindowsSnapshot } from './coex-windows';
/** Frozen COEX snapshot v1. Facts are observations, never a safety verdict. */
export type UnknownFact = { status: 'unknown'; reason: string };
export type Fact<T> = { status: 'known'; value: T } | UnknownFact;
export type CoexPlatform = 'linux' | 'darwin' | 'win32' | 'android' | 'ios' | 'other';
export interface CoexAddress { address: string; prefixLen: number }
export interface CoexRoute {
  prefix: string;
  table: Fact<number | null>;
  scope: Fact<'global' | 'interfaceScoped'>;
  role: Fact<'coverageDeclaration' | 'resourceClaim'>;
}
export interface CoexPolicyRule {
  priority: number;
  lookupTable: Fact<number | null>;
  addressFamily: Fact<'ipv4' | 'ipv6'>;
  selectorScope: Fact<{ kind: 'global' } | { kind: 'limited'; selector: string }>;
  appliesToObject: Fact<boolean>;
}
export interface CoexObject {
  interface: string;
  tunnel: Fact<boolean>;
  virtualization: Fact<boolean>;
  addresses: Fact<CoexAddress[]>;
  routes: Fact<CoexRoute[]>;
  policyRules: Fact<CoexPolicyRule[]>;
  stableIdentity: Fact<string | null>;
}
export interface CoexSnapshotV1 {
  schemaVersion: 1;
  platform: CoexPlatform;
  objects: Fact<CoexObject[]>;
  observation: Fact<{ elapsedMillis: number; atomic: false }>;
  commandCleanup: Fact<'noRetainedSnapshotCommand'>;
  context: {
    observationPhase: UnknownFact;
    ownInterfaces: UnknownFact;
    criteria: UnknownFact;
    repairHistory: UnknownFact;
  };
  classification: UnknownFact;
}

export type CoexSnapshot = CoexSnapshotV1 | CoexWindowsSnapshot;

export class CoexSnapshotDecodeError extends Error {
  constructor() { super('Invalid COEX snapshot'); this.name = 'CoexSnapshotDecodeError'; }
}
const invalid = (): never => { throw new CoexSnapshotDecodeError(); };
type Decode<T> = (input: unknown) => T;
function record(input: unknown): Record<string, unknown> {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) return invalid();
  return input as Record<string, unknown>;
}
function fields(input: unknown, keys: readonly string[]): Record<string, unknown> {
  const value = record(input);
  if (Object.keys(value).length !== keys.length || keys.some((key) => !Object.prototype.hasOwnProperty.call(value, key))) return invalid();
  return value;
}
const text: Decode<string> = (v) => typeof v === 'string' ? v : invalid();
const bool: Decode<boolean> = (v) => typeof v === 'boolean' ? v : invalid();
const integer = (max = Number.MAX_SAFE_INTEGER): Decode<number> => (v) =>
  typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= max ? v : invalid();
// Rust Fact<Option<u32>>: known null means no table-number concept, not unreadable.
const tableNumber: Decode<number | null> = (v) => v === null ? null : integer(0xffffffff)(v);
const literal = <T extends string | number | boolean>(...values: readonly T[]): Decode<T> => (v) =>
  values.some((value) => value === v) ? v as T : invalid();
const list = <T>(decode: Decode<T>): Decode<T[]> => (v) => Array.isArray(v) ? Array.from(v, decode) : invalid();
function unknown(input: unknown): UnknownFact {
  const v = fields(input, ['status', 'reason']);
  return { status: literal('unknown')(v.status), reason: text(v.reason) };
}
function fact<T>(decode: Decode<T>): Decode<Fact<T>> {
  return (input) => {
    const value = record(input);
    if (value.status === 'unknown') return unknown(input);
    const v = fields(input, ['status', 'value']);
    return { status: literal('known')(v.status), value: decode(v.value) };
  };
}
const address: Decode<CoexAddress> = (input) => {
  const v = fields(input, ['address', 'prefixLen']);
  return { address: text(v.address), prefixLen: integer(128)(v.prefixLen) };
};
const route: Decode<CoexRoute> = (input) => {
  const v = fields(input, ['prefix', 'table', 'scope', 'role']);
  return {
    prefix: text(v.prefix), table: fact(tableNumber)(v.table),
    scope: fact(literal('global', 'interfaceScoped'))(v.scope),
    role: fact(literal('coverageDeclaration', 'resourceClaim'))(v.role),
  };
};
const selector: Decode<{ kind: 'global' } | { kind: 'limited'; selector: string }> = (input) => {
  const v = record(input);
  if (v.kind === 'global') { fields(v, ['kind']); return { kind: 'global' }; }
  fields(v, ['kind', 'selector']);
  return { kind: literal('limited')(v.kind), selector: text(v.selector) };
};
const policy: Decode<CoexPolicyRule> = (input) => {
  const v = fields(input, ['priority', 'lookupTable', 'addressFamily', 'selectorScope', 'appliesToObject']);
  return {
    priority: integer(0xffffffff)(v.priority), lookupTable: fact(tableNumber)(v.lookupTable),
    addressFamily: fact(literal('ipv4', 'ipv6'))(v.addressFamily),
    selectorScope: fact(selector)(v.selectorScope), appliesToObject: fact(bool)(v.appliesToObject),
  };
};
const object: Decode<CoexObject> = (input) => {
  const v = fields(input, ['interface', 'tunnel', 'virtualization', 'addresses', 'routes', 'policyRules', 'stableIdentity']);
  return {
    interface: text(v.interface), tunnel: fact(bool)(v.tunnel), virtualization: fact(bool)(v.virtualization),
    addresses: fact(list(address))(v.addresses), routes: fact(list(route))(v.routes),
    policyRules: fact(list(policy))(v.policyRules),
    stableIdentity: fact((id) => id === null ? null : text(id))(v.stableIdentity),
  };
};

/** No transport fallback, default empty list, context synthesis or classification. */
export function decodeCoexSnapshot(input: unknown): CoexSnapshot {
  if (input !== null && typeof input === 'object' && (input as Record<string, unknown>).schemaVersion === 2) return decodeCoexWindows(input, invalid);
  const v = fields(input, ['schemaVersion', 'platform', 'objects', 'observation', 'commandCleanup', 'context', 'classification']);
  const context = fields(v.context, ['observationPhase', 'ownInterfaces', 'criteria', 'repairHistory']);
  return {
    schemaVersion: literal(1)(v.schemaVersion),
    platform: literal('linux', 'darwin', 'win32', 'android', 'ios', 'other')(v.platform),
    objects: fact(list(object))(v.objects),
    observation: fact((input) => {
      const row = fields(input, ['elapsedMillis', 'atomic']);
      return { elapsedMillis: integer()(row.elapsedMillis), atomic: literal(false)(row.atomic) };
    })(v.observation),
    commandCleanup: fact(literal('noRetainedSnapshotCommand'))(v.commandCleanup),
    context: {
      observationPhase: unknown(context.observationPhase), ownInterfaces: unknown(context.ownInterfaces),
      criteria: unknown(context.criteria), repairHistory: unknown(context.repairHistory),
    },
    classification: unknown(v.classification),
  };
}
