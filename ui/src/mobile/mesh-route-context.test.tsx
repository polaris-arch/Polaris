import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import type { Rule, ServerConfig, UserConfig } from '@/contracts/types';
import fixture from '@/contracts/mesh-route-report.fixture.json';
import zhCN from '@/i18n/locales/zh-CN.json';
import { configBaseVersion } from '@/lib/staged-config';

const translate = (key: string, vars?: Record<string, unknown>): string => {
  let value: unknown = zhCN;
  for (const part of key.split('.')) value = (value as Record<string, unknown>)[part];
  if (typeof value !== 'string') throw new Error(`missing ${key}`);
  return value.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars?.[name] ?? ''));
};
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: translate }) }));

const { asMeshRouteReport, meshRouteState, meshRouteSummaryKey, MobileMeshRouteEvidence } =
  await import('./MobileMeshRouteEvidence');
const { meshRouteMatchesVisibleConfig, meshRouteRuleHints } = await import('./mesh-route-context');

const node = (id: string, protocol = 'tailscale'): ServerConfig =>
  ({ id, name: id, protocol, address: `${id}.example`, port: 443 }) as ServerConfig;
const rule = (id: string, cidr: string): Rule =>
  ({ id, type: 'ipCidr', values: [cidr], action: 'proxy', enabled: true }) as Rule;
const base = (rules: Rule[] = []): UserConfig => ({
  servers: [node('a'), node('b'), node('plain', 'vless')],
  trafficRules: rules,
  selectedServerId: 'a',
  proxyMode: 'smart',
} as UserConfig);

function applied(config: UserConfig): MeshRouteReport {
  const report = asMeshRouteReport(structuredClone(fixture));
  if (!report) throw new Error('shared fixture rejected');
  report.snapshot.scope = 'applied';
  report.snapshot.configSource = 'running';
  report.snapshot.configVersion = configBaseVersion(config);
  report.snapshot.runGeneration = 1;
  report.snapshot.loadEvidence = 'startupReady';
  report.snapshot.candidates[2].matchCidrs = [];
  report.results[2].effective = [];
  report.results[2].coverage = 'full';
  report.results[2].unknownReasons = [];
  return report;
}

function details(report: MeshRouteReport, serverId: string, contextMismatch: boolean): string {
  return renderToStaticMarkup(<MobileMeshRouteEvidence report={report} serverId={serverId}
    contextMismatch={contextMismatch} nameOf={(id) => id} />);
}

describe('mobile running report versus visible edit', () => {
  it('keeps a staged-only node and a saved but unapplied edit out of the running conclusion', () => {
    const running = base();
    const report = applied(running);
    expect(meshRouteMatchesVisibleConfig(report, running, { servers: running.servers })).toBe(true);
    expect(meshRouteState(report, 'a')).toBe('full');

    const staged = { ...running, servers: [...running.servers, node('new')] };
    expect(meshRouteMatchesVisibleConfig(report, staged, { servers: staged.servers })).toBe(false);
    expect(meshRouteState(report, 'new', false, false, true)).toBe('unknown');
    expect(meshRouteSummaryKey(report, 'a', false, false, true))
      .toBe('mobileMeshRouteEvidence.summary.unknown');
    const saved = { ...running, enableIPv6: true };
    expect(meshRouteMatchesVisibleConfig(report, saved, { servers: saved.servers })).toBe(false);
    const markup = details(report, 'b', true);
    expect(markup).toContain(translate('mobileMeshRouteEvidence.scope.unmatchedConfig'));
    expect(markup).toContain(translate('mobileMeshRouteEvidence.runningResult', {
      result: translate('mobileMeshRouteEvidence.coverage.none'),
    }));
    expect(markup).not.toContain(translate('mobileMeshRouteEvidence.loadedRanges'));
    expect(markup).not.toContain(translate('mobileMeshRouteEvidence.zeroEffective'));
    expect(markup).toContain(translate('mobileMeshRouteEvidence.zeroCalculated'));
  });

  it('guards a same-ID edit and an optimistic mirror, then realigns after the new run', () => {
    const running = base();
    const oldReport = applied(running);
    const editedNode = { ...running.servers[0], address: 'changed.example' };
    const edited = { ...running, servers: [editedNode, ...running.servers.slice(1)] };
    expect(meshRouteMatchesVisibleConfig(oldReport, edited, { servers: edited.servers })).toBe(false);
    expect(meshRouteMatchesVisibleConfig(oldReport, running, { servers: edited.servers })).toBe(false);
    const nextReport = applied(edited);
    expect(meshRouteMatchesVisibleConfig(nextReport, edited, { servers: edited.servers })).toBe(true);
    expect(meshRouteState(nextReport, 'a')).toBe('full');
    expect(details(nextReport, 'b', false)).toContain(translate('mobileMeshRouteEvidence.loadedRanges'));
  });

  it('distinguishes an empty global report and a valid non-force node without calling either overlap', () => {
    const running = base();
    const report = applied(running);
    expect(meshRouteState(report, 'plain')).toBe('absent');
    report.results = [];
    report.snapshot.candidates = [];
    report.totalCandidateCount = 0;
    expect(meshRouteState(report)).toBe('empty');
    expect(meshRouteState(report, 'plain')).toBe('absent');
    expect(meshRouteState(report, 'a')).toBe('absent');
  });

  it('does not label a persisted unknown sample as a running report', () => {
    const config = base();
    const report = applied(config);
    report.snapshot.scope = 'persistedUnknown';
    report.snapshot.configSource = 'persisted';
    report.snapshot.configVersion = null;
    expect(meshRouteMatchesVisibleConfig(report, config, { servers: config.servers })).toBe(false);
    expect(meshRouteRuleHints(report, config, config.servers, [], false, true).contextMismatch).toBe(false);
    expect(meshRouteState(report, 'a')).toBe('pending');
    expect(details(report, 'a', false)).not.toContain(translate('mobileMeshRouteEvidence.scope.unmatchedConfig'));
  });

  it('never badges a current rule with requested CIDRs from a different running version', () => {
    const originalRule = rule('same-id', '192.168.1.0/24');
    const running = base([originalRule]);
    const report = applied(running);
    const editedRule = rule('same-id', '10.20.0.5/32');
    const draft = { ...running, trafficRules: [editedRule] };
    const stale = meshRouteRuleHints(report, draft, draft.servers, draft.trafficRules, false, true);
    expect(stale.contextMismatch).toBe(true);
    expect(stale.unknown).toBe(true);
    expect(stale.ids).toEqual(new Set());
    const optimistic = meshRouteRuleHints(report, running, running.servers, [editedRule], false, true);
    expect(optimistic.ids).toEqual(new Set());
    expect(optimistic.contextMismatch).toBe(true);

    const newReport = applied(draft);
    const aligned = meshRouteRuleHints(newReport, draft, draft.servers, draft.trafficRules, false, true);
    expect(aligned.contextMismatch).toBe(false);
    expect(aligned.unknown).toBe(false);
    expect(aligned.ids).toEqual(new Set(['same-id']));
  });
});
