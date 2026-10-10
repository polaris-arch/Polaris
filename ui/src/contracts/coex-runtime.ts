import { decodeCoexSnapshot, type CoexSnapshot, type Fact } from './coex-snapshot';
export interface CoexRuntimeBinding {
  sessionId: Fact<string>; lifecycleGeneration: Fact<string>; configGeneration: Fact<string>; networkEpoch: Fact<string>;
  lifecyclePhase: 'idle' | 'starting' | 'ready' | 'stopped' | 'crashed';
}
export interface CoexRuntimeState {
  schemaVersion: 1; reportRevision: string; binding: CoexRuntimeBinding;
  freshness: 'latest' | 'stale' | 'unavailable'; activity: 'idle' | 'running' | 'pending' | 'runningWithPending';
  reason: Fact<{ code: string; detail: string }>;
  pending: Fact<{ requestedAtMonotonicMillis: string; expiresAtMonotonicMillis: string; trigger: 'startup' | 'network' | 'config' | 'manual'; manualRequested: boolean }>;
  report: Fact<{ reportRevision: string; acquisition: { startedAtUnixMillis: string; endedAtUnixMillis: string; startedAtMonotonicMillis: string; endedAtMonotonicMillis: string; startBinding: CoexRuntimeBinding; endBinding: CoexRuntimeBinding; atomic: false }; snapshot: CoexSnapshot }>;
}
export class CoexRuntimeDecodeError extends Error { constructor() { super('Invalid COEX runtime state'); this.name = 'CoexRuntimeDecodeError'; } }
function object(v: unknown): Record<string, unknown> { if (!v || typeof v !== 'object' || Array.isArray(v)) throw new CoexRuntimeDecodeError(); return v as Record<string, unknown>; }
function text(v: unknown): string { if (typeof v !== 'string' || v.length > 1048576) throw new CoexRuntimeDecodeError(); return v; }
function decimal(v: unknown): string { const s = text(v); if (!/^(0|[1-9][0-9]{0,19})$/.test(s) || BigInt(s) > BigInt('18446744073709551615')) throw new CoexRuntimeDecodeError(); return s; }
function choice<T extends string>(v: unknown, values: readonly T[]): T { if (!values.includes(v as T)) throw new CoexRuntimeDecodeError(); return v as T; }
function fact<T>(v: unknown, read: (v: unknown) => T): Fact<T> { const f = object(v); if (f.status === 'known') return { status: 'known', value: read(f.value) }; if (f.status === 'unknown') return { status: 'unknown', reason: text(f.reason) }; throw new CoexRuntimeDecodeError(); }
function binding(v: unknown): CoexRuntimeBinding { const b = object(v); return { sessionId: fact(b.sessionId, decimal), lifecycleGeneration: fact(b.lifecycleGeneration, decimal), configGeneration: fact(b.configGeneration, decimal), networkEpoch: fact(b.networkEpoch, decimal), lifecyclePhase: choice(b.lifecyclePhase, ['idle', 'starting', 'ready', 'stopped', 'crashed']) }; }
const reasonCodes = ['notCollected', 'noActiveSession', 'lifecycleChanged', 'configChanged', 'networkChanged', 'watcherUnavailable', 'sourceUnavailable', 'pendingExpired', 'workerStillRunning', 'admissionBusy', 'serviceQuarantined', 'bindingUnavailable', 'unsupportedPlatform', 'staleCompletion', 'collectionFailed'] as const;
const boundKeys = ['sessionId', 'lifecycleGeneration', 'configGeneration', 'networkEpoch'] as const;
function equalKnown(a: CoexRuntimeBinding, b: CoexRuntimeBinding): boolean { return a.lifecyclePhase === 'ready' && b.lifecyclePhase === 'ready' && boundKeys.every(key => a[key].status === 'known' && b[key].status === 'known' && a[key].value === b[key].value); }
export function decodeCoexRuntimeState(raw: unknown): CoexRuntimeState {
  const v = object(raw); if (v.schemaVersion !== 1) throw new CoexRuntimeDecodeError();
  const result: CoexRuntimeState = {
    schemaVersion: 1, reportRevision: decimal(v.reportRevision), binding: binding(v.binding), freshness: choice(v.freshness, ['latest', 'stale', 'unavailable']), activity: choice(v.activity, ['idle', 'running', 'pending', 'runningWithPending']),
    reason: fact(v.reason, rawReason => { const r = object(rawReason); return { code: choice(r.code, reasonCodes), detail: text(r.detail) }; }),
    pending: fact(v.pending, rawPending => { const p = object(rawPending); const first = decimal(p.requestedAtMonotonicMillis); const expiry = decimal(p.expiresAtMonotonicMillis); if (BigInt(expiry) - BigInt(first) !== BigInt(10000) || typeof p.manualRequested !== 'boolean') throw new CoexRuntimeDecodeError(); return { requestedAtMonotonicMillis: first, expiresAtMonotonicMillis: expiry, trigger: choice(p.trigger, ['startup', 'network', 'config', 'manual']), manualRequested: p.manualRequested }; }),
    report: fact(v.report, rawReport => {
      const r = object(rawReport); const a = object(r.acquisition); const start = decimal(a.startedAtMonotonicMillis); const end = decimal(a.endedAtMonotonicMillis); if (BigInt(end) < BigInt(start) || a.atomic !== false) throw new CoexRuntimeDecodeError();
      const snapshot = decodeCoexSnapshot(r.snapshot);
      if (snapshot.classification.status !== 'unknown' || Object.values(snapshot.context).some(f => f.status !== 'unknown')) throw new CoexRuntimeDecodeError();
      return { reportRevision: decimal(r.reportRevision), acquisition: { startedAtUnixMillis: decimal(a.startedAtUnixMillis), endedAtUnixMillis: decimal(a.endedAtUnixMillis), startedAtMonotonicMillis: start, endedAtMonotonicMillis: end, startBinding: binding(a.startBinding), endBinding: binding(a.endBinding), atomic: false }, snapshot };
    }),
  };
  const hasPending = result.pending.status === 'known';
  if (hasPending !== ['pending', 'runningWithPending'].includes(result.activity)) throw new CoexRuntimeDecodeError();
  if (result.report.status === 'known') {
    const report = result.report.value;
    if (BigInt(report.reportRevision) > BigInt(result.reportRevision)) throw new CoexRuntimeDecodeError();
    if (report.acquisition.startBinding.sessionId.status !== 'known' || result.binding.sessionId.status !== 'known' || report.acquisition.startBinding.sessionId.value !== result.binding.sessionId.value) throw new CoexRuntimeDecodeError();
    if (result.freshness === 'latest') {
      const snapshot = report.snapshot;
      const sourceContent = snapshot.schemaVersion === 1 ? snapshot.objects.status === 'known' : Object.values(snapshot.sources).some(source => source.rows.status === 'known');
      if (!sourceContent || !equalKnown(report.acquisition.startBinding, report.acquisition.endBinding) || !equalKnown(report.acquisition.endBinding, result.binding)) throw new CoexRuntimeDecodeError();
    }
  } else if (result.freshness === 'latest') throw new CoexRuntimeDecodeError();
  return result;
}
