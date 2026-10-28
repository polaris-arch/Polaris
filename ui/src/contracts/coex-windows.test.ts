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
  it('retains actual Rust IPv4-mapped IPv6 address, nextHop and normalized prefix without changing family or scope', () => {
    const v = decodeCoexSnapshot(fixture.snapshots.mappedV6);
    if (v.schemaVersion !== 2 || v.sources.addresses.rows.status !== 'known' || v.sources.routes6.rows.status !== 'known') throw Error('fixture');
    const address = v.sources.addresses.rows.value[1]; const route = v.sources.routes6.rows.value[0];
    expect(address.family).toBe('ipv6'); expect(address.address).toBe('::ffff:192.0.2.9'); expect(address.scopeId).toEqual(known(42));
    expect(route.family).toBe('ipv6'); expect(route.prefix).toBe('::ffff:c000:200/120');
    expect(route.nextHop).toEqual(known('::ffff:192.0.2.1')); expect(route.nextHopScopeId).toEqual(known(42));
  });
  it('retains the equivalent IPv4-tail prefix spelling from the actual Rust serializer-only seam', () => {
    const v = decodeCoexSnapshot(fixture.snapshots.mappedV6WireSpelling);
    if (v.schemaVersion !== 2 || v.sources.routes6.rows.status !== 'known') throw Error('fixture');
    expect(v.sources.routes6.rows.value[0].family).toBe('ipv6'); expect(v.sources.routes6.rows.value[0].prefix).toBe('::ffff:192.0.2.0/120');
  });
  it.each(['::ffff:192.0.2.1', '::192.0.2.1', '1:2:3:4:5:6:192.0.2.1', '1:2:3:4:5::192.0.2.1'])('accepts a valid final IPv4 tail as IPv6: %s', (address) => {
    const v = clone(); v.sources.addresses.rows.value[1].address = address;
    expect(decodeCoexSnapshot(v)).toEqual(v);
  });
  it.each(['::ffff:192.0.2.256', '::ffff:192.00.2.1', '::ffff:192.0.2', '::ffff:192.0.2.1.5',
    '::ffff:+192.0.2.1', '::ffff:192.0.2.1:1', '::ffff:192.0.2.1%42', ':::ffff:192.0.2.1',
    '1:2:3:4:5:192.0.2.1', '1:2:3:4:5:6:7:192.0.2.1', '1:2:3:4:5:6::192.0.2.1',
    '1::2::192.0.2.1'])('rejects an invalid IPv4-tailed IPv6 in address/nextHop/prefix: %s', (address) => {
    for (const field of ['address', 'nextHop', 'prefix']) {
      const v = clone();
      if (field === 'address') v.sources.addresses.rows.value[1].address = address;
      else if (field === 'nextHop') v.sources.routes6.rows.value[0].nextHop.value = address;
      else v.sources.routes6.rows.value[0].prefix = address + '/120';
      expect(() => decodeCoexSnapshot(v), field).toThrow(CoexSnapshotDecodeError);
    }
  });
  it('retains four independent sources when actual Rust wire degrades only an oversized legal address source', () => {
    const v = decodeCoexSnapshot(fixture.snapshots.oversizedSource); if (v.schemaVersion !== 2) throw Error('fixture');
    expect(v.sources.addresses.rows.status).toBe('unknown'); expect(v.sources.addresses.complete.status).toBe('unknown');
    expect(v.sources.addresses.error).toEqual(known('Windows source wire byte limit exceeded'));
    for (const key of ['adapters', 'routes4', 'routes6', 'ras'] as const) expect(v.sources[key].rows.status).toBe('known');
  });
  it('applies the same precise 8 MiB source JSON budget, independently of field and row budgets', () => {
    // Synthetic wire-size boundary only; not a provider or device fixture.
    const v = clone(); const source = v.sources.addresses;
    source.rows.value = Array.from({ length: 16000 }, () => structuredClone(source.rows.value[1]));
    const limit = 8 * 1024 * 1024; const bytes = () => new TextEncoder().encode(JSON.stringify(source)).length;
    const remaining = limit - bytes(); expect(remaining).toBeGreaterThan(0);
    const extra = Math.floor(remaining / source.rows.value.length);
    for (const row of source.rows.value) row.interface.alias.value += 'x'.repeat(extra);
    source.rows.value[0].interface.alias.value += 'x'.repeat(limit - bytes());
    expect(bytes()).toBe(limit); expect(decodeCoexSnapshot(v)).toEqual(v);
    source.rows.value[0].interface.alias.value += 'x'; expect(bytes()).toBe(limit + 1);
    expect(() => decodeCoexSnapshot(v)).toThrow(CoexSnapshotDecodeError);
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
