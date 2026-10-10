import { describe, expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { CoexSnapshotDecodeError, decodeCoexSnapshot } from './coex-snapshot';
import fixture from './coex-windows.rust.fixture.json';
const clone = () => structuredClone(fixture.snapshots.full);
const known = (value: unknown) => ({ status: 'known', value });
describe('actual Rust Windows v2 wire boundary', () => {
  it('pins the compiled production source hashes and roundtrips all actual Rust outputs', () => {
    for (const [path, hash] of Object.entries(fixture.producer.sourceSha256)) {
      expect(createHash('sha256').update(readFileSync(new URL('../../../' + path, import.meta.url))).digest('hex'), path).toBe(hash);
    }
    for (const wire of Object.values(fixture.snapshots)) expect(decodeCoexSnapshot(wire)).toEqual(wire);
  });
  it('retains lossless maximum LUID, family-specific indices, IPv6 bits/scope and unassociated masked RAS', () => {
    const wire = decodeCoexSnapshot(fixture.snapshots.full);
    if (wire.schemaVersion !== 2 || wire.sources.addresses.rows.status !== 'known' || wire.sources.ras.rows.status !== 'known') throw Error('fixture');
    expect(wire.sources.addresses.rows.value.map(row => row.interface.ifIndex)).toEqual([known(17), known(42)]);
    expect(wire.sources.addresses.rows.value[1].address).toBe('fe80::1234');
    expect(wire.sources.addresses.rows.value[1].scopeId).toEqual(known(42));
    expect(wire.sources.addresses.rows.value[0].interface.luid).toEqual(known('18446744073709551615'));
    expect(wire.sources.ras.rows.value).toHaveLength(2);
    expect(wire.sources.ras.rows.value[0].name.status).toBe('unknown');
    expect(wire.sources.ras.rows.value[1].interface.status).toBe('unknown');
    expect(JSON.stringify(wire)).not.toContain('12025550123');
  });
  it('keeps an independent IPv6 error and known empty rows without claiming absence or classification', () => {
    const v = decodeCoexSnapshot(fixture.snapshots.partial);
    if (v.schemaVersion !== 2) throw Error('fixture');
    expect(v.sources.routes6.rows.status).toBe('unknown'); expect(v.sources.addresses.rows.status).toBe('known');
    expect(v.sources.routes6.error).toEqual(known('Windows GetIpForwardTable2 failed (code 5)'));
    expect(v.objects.status).toBe('unknown'); expect(v.commandCleanup.status).toBe('unknown'); expect(v.classification.status).toBe('unknown');
    const empty = decodeCoexSnapshot(fixture.snapshots.empty); if (empty.schemaVersion !== 2) throw Error('fixture');
    for (const source of Object.values(empty.sources)) { expect(source.rows).toEqual(known([])); expect(source.complete.status).toBe('unknown'); }
  });
  it.each([18446744073709551615, '18446744073709551616', '01', '0', '-1', '1.5', '1e3'])('rejects unsafe LUID %j', (value) => {
    const v = clone(); Object.assign(v.sources.adapters.rows.value[0].interface.luid, { value });
    expect(() => decodeCoexSnapshot(v)).toThrow(CoexSnapshotDecodeError);
  });
  it.each(['.12025550123', '  .12025550123'])('rejects an unmasked telephone entry without echoing it', (value) => {
    const v = clone(); Object.assign(v.sources.ras.rows.value[0].name, known(value)); delete (v.sources.ras.rows.value[0].name as {reason?:string}).reason;
    try { decodeCoexSnapshot(v); throw Error('unexpected acceptance'); } catch (e) { expect(e).toBeInstanceOf(CoexSnapshotDecodeError); expect(String(e)).not.toContain('12025550123'); }
  });
  it('rejects invented adapter family index, scoped-address strings, malformed IPv6 and fabricated cleanup', () => {
    for (const mutate of [
      (v: ReturnType<typeof clone>) => Object.assign(v.sources.adapters.rows.value[0].interface.ifIndex, known(17)),
      (v: ReturnType<typeof clone>) => { v.sources.addresses.rows.value[1].address = 'fe80::1234%42'; },
      (v: ReturnType<typeof clone>) => { v.sources.addresses.rows.value[1].address = 'fe80:::1234'; },
      (v: ReturnType<typeof clone>) => Object.assign(v.commandCleanup, known('nativeQueryClosed')),
      (v: ReturnType<typeof clone>) => { v.platform = 'darwin'; },
    ]) { const v = clone(); mutate(v); expect(() => decodeCoexSnapshot(v)).toThrow(CoexSnapshotDecodeError); }
  });
  it('rejects a positive completeness claim with an unknown scope or errored source', () => {
    const v = clone(); Object.assign(v.sources.adapters.complete, known(true)); delete (v.sources.adapters.complete as {reason?:string}).reason;
    expect(() => decodeCoexSnapshot(v)).toThrow(CoexSnapshotDecodeError);
  });
  it('rejects oversized source rows and preserves a busy source as Unknown', () => {
    const v = clone(); v.sources.adapters.rows.value = Array(129).fill(v.sources.adapters.rows.value[0]);
    expect(() => decodeCoexSnapshot(v)).toThrow();
    const busy = decodeCoexSnapshot(fixture.snapshots.busy); if (busy.schemaVersion !== 2) throw Error('fixture');
    expect(busy.sources.ras.rows.status).toBe('unknown'); expect(busy.observation.status).toBe('unknown');
  });
});
