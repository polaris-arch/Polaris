import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import fixture from './coex-runtime.rust.fixture.json';
import { decodeCoexRuntimeState } from './coex-runtime';
const latest = () => structuredClone(fixture.states.latest);
describe('actual runtime Rust serializer boundary', () => {
  it('pins the actual producer sources', () => {
    for (const [path, sha] of Object.entries(fixture.producer.sourceSha256)) expect(createHash('sha256').update(readFileSync(new URL('../../../' + path, import.meta.url))).digest('hex'), path).toBe(sha);
  });
  it.each(Object.entries(fixture.states))('decodes actual %s state without changing inner trust', (_name, state) => {
    const decoded = decodeCoexRuntimeState(state); expect(decoded).toEqual(state);
    if (decoded.report.status === 'known') { expect(decoded.report.value.snapshot.classification.status).toBe('unknown'); for (const fact of Object.values(decoded.report.value.snapshot.context)) expect(fact.status).toBe('unknown'); }
  });
  it.each(['sessionId', 'lifecycleGeneration', 'configGeneration', 'networkEpoch'] as const)('Unknown %s cannot be latest', key => {
    const raw = latest(); const unknown = { status: 'unknown', reason: 'fixture unknown' };
    Object.assign(raw.binding[key], unknown); Object.assign(raw.report.value.acquisition.startBinding[key], unknown); Object.assign(raw.report.value.acquisition.endBinding[key], unknown);
    expect(() => decodeCoexRuntimeState(raw)).toThrow();
  });
  it.each(['sessionId', 'lifecycleGeneration', 'configGeneration', 'networkEpoch'] as const)('mismatched %s cannot be latest', key => {
    const raw = latest(); raw.report.value.acquisition.endBinding[key].value = '999'; expect(() => decodeCoexRuntimeState(raw)).toThrow();
  });
  it.each([1, '01', '-1', '18446744073709551616', '', '1.5', null])('rejects unsafe revision %j', revision => {
    const raw = latest(); Object.assign(raw, { reportRevision: revision }); expect(() => decodeCoexRuntimeState(raw)).toThrow();
  });
  it('uses monotonic order and permits Unix display clock to go backwards', () => {
    const raw = latest(); raw.report.value.acquisition.startedAtUnixMillis = '200'; raw.report.value.acquisition.endedAtUnixMillis = '100'; expect(() => decodeCoexRuntimeState(raw)).not.toThrow();
    raw.report.value.acquisition.startedAtMonotonicMillis = '2'; raw.report.value.acquisition.endedAtMonotonicMillis = '1'; expect(() => decodeCoexRuntimeState(raw)).toThrow();
  });
  it('never relabels lifecycle Ready as BeforePolarisTun evidence', () => {
    const raw = latest(); Object.assign(raw.report.value.snapshot.context.observationPhase, { status: 'known', value: 'beforePolarisTun' }); expect(() => decodeCoexRuntimeState(raw)).toThrow();
  });
  it('rejects latest without current-session report and impossible pending activity', () => {
    const raw = structuredClone(fixture.states.crashed); Object.assign(raw, { freshness: 'latest' }); expect(() => decodeCoexRuntimeState(raw)).toThrow();
    const pending = structuredClone(fixture.states.staleBusy); Object.assign(pending, { activity: 'idle' }); expect(() => decodeCoexRuntimeState(pending)).toThrow();
  });
  it('retains five independent Unknown Windows sources and errors', () => {
    const state = decodeCoexRuntimeState(fixture.states.windowsUnavailable); expect(state.freshness).toBe('unavailable');
    if (state.report.status !== 'known' || state.report.value.snapshot.schemaVersion !== 2) throw Error('fixture');
    for (const source of Object.values(state.report.value.snapshot.sources)) { expect(source.rows.status).toBe('unknown'); expect(source.complete.status).toBe('unknown'); expect(source.compartment.status).toBe('unknown'); expect(source.error.status).toBe('known'); }
  });
});
