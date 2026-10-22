import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import fixture from './coex-runtime.rust.fixture.json';
import { decodeCoexRuntimeState } from './coex-runtime';
const latest = () => structuredClone(fixture.states.latest);
describe('historical actual Rust export and current source boundary', () => {
  it('pins the historical export and partitions its original nine-file source footprint', () => {
    const compatibility = fixture.sourceCompatibility;
    expect(compatibility.newSerializerExecution).toBe(false);
    expect(compatibility.currentStatesReexported).toBe(false);
    expect(compatibility.originalStatesCanonicalSha256)
      .toBe('285de8e6e9ff31d6a499e4e606f36661d8bda0a2b0866c3c13c06e52cf63d874');
    // Preserve original historical identity; a source refresh is never a serializer run.
    expect(createHash('sha256').update(JSON.stringify(fixture.producer)).digest('hex'))
      .toBe('eed3e1a21b6913fb5cc8f1525b0a15bf13119518960b42fd7767d29448135fd7');
    expect(createHash('sha256').update(JSON.stringify(fixture.states)).digest('hex'))
      .toBe(compatibility.originalStatesCanonicalSha256);
    const changed = [...compatibility.changedObserverSources,
      ...compatibility.changedControlSources, ...compatibility.changedTestSources];
    const classified = [...changed, ...compatibility.unchangedOriginalSources];
    const sourcePaths = Object.keys(fixture.producer.sourceSha256).sort();
    expect(sourcePaths).toHaveLength(9);
    expect(classified).toHaveLength(9);
    expect(new Set(classified).size).toBe(9);
    expect([...classified].sort()).toEqual(sourcePaths);
    expect(Object.keys(compatibility.sourceSha256).sort()).toEqual(sourcePaths);
    expect(Object.keys(compatibility.sourceReview).sort()).toEqual(sourcePaths);
    expect(compatibility.changedObserverSources).toContain('src-tauri/src/runtime/proxy/coex_observer.rs');
    expect(compatibility.changedControlSources).toContain('src-tauri/src/runtime/proxy/lifecycle.rs');
    expect(compatibility.changedControlSources).toContain('src-tauri/src/runtime/proxy/recovery.rs');
    expect(compatibility.unchangedOriginalSources).not.toContain('src-tauri/src/runtime/proxy/recovery.rs');
    for (const path of changed) {
      expect(compatibility.sourceSha256[path as keyof typeof compatibility.sourceSha256])
        .not.toBe(fixture.producer.sourceSha256[path as keyof typeof fixture.producer.sourceSha256]);
    }
    for (const path of compatibility.unchangedOriginalSources) {
      expect(compatibility.sourceSha256[path as keyof typeof compatibility.sourceSha256])
        .toBe(fixture.producer.sourceSha256[path as keyof typeof fixture.producer.sourceSha256]);
    }
    for (const [path, sha] of Object.entries(compatibility.sourceSha256)) {
      expect(createHash('sha256').update(readFileSync(new URL('../../../' + path, import.meta.url))).digest('hex'), path).toBe(sha);
    }
  });
  it('pins the separate HOT proxy supplement without relabeling it as historical or execution evidence', () => {
    const integration = fixture.sourceCompatibility.hotProxyIntegration;
    const paths = [
      'src-tauri/src/runtime/proxy/mesh_route_report.rs',
      'src-tauri/src/runtime/proxy/native_auto.rs',
      'src-tauri/src/runtime/proxy/pending_changes.rs',
      'src-tauri/src/runtime/proxy/runtime_config.rs',
      'src-tauri/src/runtime/proxy/runtime_config/tests/mod.rs',
      'src-tauri/src/runtime/proxy/tests/hot_switch.rs',
      'src-tauri/src/runtime/proxy/tests/hot_switch/auto_select.rs',
      'src-tauri/src/runtime/proxy/tests/lifecycle.rs',
      'src-tauri/src/runtime/proxy/tests/mesh_route_report.rs',
      'src-tauri/src/runtime/proxy/tests/real_core_cleanup.rs',
    ].sort();
    expect(integration.newSerializerExecution).toBe(false);
    expect(integration.currentStatesReexported).toBe(false);
    expect(Object.keys(integration.sourceSha256).sort()).toEqual(paths);
    expect(Object.keys(integration.sourceReview).sort()).toEqual(paths);
    expect(integration.addedSources).toEqual([
      'src-tauri/src/runtime/proxy/runtime_config.rs',
      'src-tauri/src/runtime/proxy/runtime_config/tests/mod.rs',
    ]);
    const classified = [...integration.addedSources, ...integration.changedSources];
    expect(classified).toHaveLength(paths.length);
    expect(new Set(classified).size).toBe(paths.length);
    expect(classified.sort()).toEqual(paths);
    for (const path of paths) expect(Object.keys(fixture.producer.sourceSha256)).not.toContain(path);
    for (const [path, sha] of Object.entries(integration.sourceSha256)) {
      expect(createHash('sha256').update(readFileSync(new URL('../../../' + path, import.meta.url))).digest('hex'), path).toBe(sha);
      expect(integration.sourceReview[path as keyof typeof integration.sourceReview].length).toBeGreaterThan(0);
    }
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
