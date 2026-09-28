import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import type { Rule, ServerConfig, UserConfig } from '@/contracts/types';
import { configBaseVersion } from '@/lib/staged-config';
import { meshNodeRouteBadge, reportForDisplayedMeshNode, reportForSavedConfig, ruleMeshRouteEvidence } from './mesh-route-badges';

const fixture = JSON.parse(readFileSync(fileURLToPath(new URL('../contracts/mesh-route-report.fixture.json', import.meta.url)), 'utf8')) as MeshRouteReport;
const clone = (): MeshRouteReport => structuredClone(fixture);
const rule = (cidr: string, enabled = true): Rule => ({
  id: 'route-1', type: 'ipCidr', values: [cidr], action: 'proxy', enabled,
} as Rule);
const applied = (): MeshRouteReport => {
  const report = clone();
  report.snapshot.scope = 'applied';
  report.snapshot.configSource = 'running';
  report.snapshot.loadEvidence = 'startupReady';
  report.snapshot.runGeneration = 7;
  return report;
};

describe('S2 badge projections preserve unknown without assigning ownership', () => {
  it('shows a confirmed blocker, a clear result, and opaque preferredBy separately', () => {
    const report = applied();
    expect(meshNodeRouteBadge(report, 'a')).toEqual({ kind: 'clear' });
    expect(meshNodeRouteBadge(report, 'b')).toEqual({
      kind: 'blocked', blocks: [{ cidr: '10.20.1.0/24', byServerId: 'a' }], incomplete: false,
    });
    expect(meshNodeRouteBadge(report, 'c')).toEqual({ kind: 'unknown' });
  });

  it('unconfirmed scope, missing report, omitted node, and unconfirmed blocker stay unknown', () => {
    expect(meshNodeRouteBadge(fixture, 'b')).toEqual({ kind: 'unknown' });
    expect(meshNodeRouteBadge(null, 'b')).toEqual({ kind: 'unknown' });
    const report = applied();
    report.totalCandidateCount += 1;
    expect(meshNodeRouteBadge(report, 'omitted')).toEqual({ kind: 'unknown' });
    report.results[1].blockedBy[0].confirmed = false;
    expect(meshNodeRouteBadge(report, 'b')).toEqual({ kind: 'unknown' });
  });

  it('D/R version and entity gates separate staged, saved-unapplied, and normal nonclaimants', () => {
    const report = applied();
    const node = {id:'a', protocol:'wireguard', name:'original'} as ServerConfig;
    const saved = {servers:[node]} as UserConfig;
    report.snapshot.configVersion = configBaseVersion(saved);
    expect(report.totalCandidateCount).toBe(report.snapshot.candidates.length);
    expect(report.unknownReasons).toEqual([]);
    const staged = {id:'new-staged-mesh', protocol:'wireguard', name:'staged'} as ServerConfig;
    expect(meshNodeRouteBadge(reportForDisplayedMeshNode(report, saved, staged), staged.id)).toEqual({ kind:'unknown' });
    const edited = {...node, meshRoutes:['10.0.0.0/8']} as ServerConfig;
    expect(meshNodeRouteBadge(reportForDisplayedMeshNode(report, saved, edited), edited.id)).toEqual({ kind:'unknown' });
    const savedNew = {servers:[node, staged]} as UserConfig;
    expect(meshNodeRouteBadge(reportForDisplayedMeshNode(report, savedNew, staged), staged.id)).toEqual({ kind:'unknown' });
    expect(ruleMeshRouteEvidence(reportForSavedConfig(report, savedNew), rule('10.20.1.7'), rule('10.20.1.7'))).toBe('unknown');
    // Same D/R version: a normal nonclaimant has no force-route claim, while
    // a candidate actually in R keeps its producer result.
    expect(meshNodeRouteBadge(reportForDisplayedMeshNode(report, saved, node), 'nonclaimant')).toEqual({ kind:'clear' });
    expect(meshNodeRouteBadge(reportForDisplayedMeshNode(report, saved, node), 'a')).toEqual({ kind:'clear' });
  });

  it('known blocked subset remains labelled incomplete when other ranges are unknown', () => {
    const report = applied();
    report.results[1].coverage = 'unknown';
    report.results[1].effective = null;
    expect(meshNodeRouteBadge(report, 'b')).toMatchObject({ kind: 'blocked', incomplete: true });
  });

  it('rule overlap uses reported effective ranges; disjoint rule stays unknown while another candidate is opaque', () => {
    const report = applied();
    expect(ruleMeshRouteEvidence(report, rule('10.20.1.7'), rule('10.20.1.7'))).toBe('overlap');
    expect(ruleMeshRouteEvidence(report, rule('203.0.113.0/24'), rule('203.0.113.0/24'))).toBe('unknown');
    expect(ruleMeshRouteEvidence(null, rule('10.20.1.7'), rule('10.20.1.7'))).toBe('unknown');
    expect(ruleMeshRouteEvidence(fixture, rule('10.20.1.7'), rule('10.20.1.7'))).toBe('unknown');
  });

  it('known disjoint, unsaved edit, disabled rule, and non-CIDR condition stay distinct', () => {
    const report = applied();
    report.snapshot.candidates.pop();
    report.results.pop();
    report.totalCandidateCount = 2;
    expect(ruleMeshRouteEvidence(report, rule('203.0.113.0/24'), rule('203.0.113.0/24'))).toBe('none');
    expect(ruleMeshRouteEvidence(report, rule('10.20.1.7'), rule('10.20.2.7'))).toBe('unknown');
    expect(ruleMeshRouteEvidence(report, rule('10.20.1.7', false), rule('10.20.1.7', false))).toBe('none');
    const domain = { ...rule('example.com'), type: 'domain' } as Rule;
    expect(ruleMeshRouteEvidence(report, domain, domain)).toBe('none');
  });
});
