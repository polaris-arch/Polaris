import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import fixture from '@/contracts/mesh-route-report.fixture.json';
import type { ServerConfig } from '@/contracts/types';

type Dict = Record<string, unknown>;
const langs = ['zh-CN', 'en-US'] as const;
const dictionaries = Object.fromEntries(langs.map((lang) => [lang, JSON.parse(readFileSync(
  fileURLToPath(new URL(`../../i18n/locales/${lang}.json`, import.meta.url)), 'utf8',
)) as Dict]));
const h = vi.hoisted(() => ({ lang: 'zh-CN' }));
function translate(key: string, vars?: Record<string, unknown>): string {
  let value: unknown = dictionaries[h.lang];
  for (const part of key.split('.')) value = (value as Dict | undefined)?.[part];
  if (typeof value !== 'string') throw new Error(`${h.lang} missing ${key}`);
  return value.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars?.[name] ?? ''));
}
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: translate }) }));

const { MobileMeshRouteBlock } = await import('./TunReports');
const { MobileMeshRouteEvidence, asMeshRouteReport, meshRouteState } = await import('../MobileMeshRouteEvidence');
const servers: ServerConfig[] = [
  { id: 'a', name: '家里', protocol: 'tailscale', address: '', port: 0 },
  { id: 'b', name: '公司', protocol: 'tailscale', address: '', port: 0 },
  { id: 'c', name: '出口', protocol: 'wireguard', address: '', port: 0 },
];
const nameOf = (id: string): string => servers.find((s) => s.id === id)?.name ?? id;
const clone = (): MeshRouteReport => {
  const report = asMeshRouteReport(structuredClone(fixture));
  if (!report) throw new Error('Shared wire fixture is no longer accepted');
  return report;
};
const applied = (): MeshRouteReport => {
  const report = clone();
  report.snapshot.scope = 'applied';
  report.snapshot.configSource = 'running';
  report.snapshot.configVersion = 'running-v1';
  report.snapshot.runGeneration = 1;
  report.snapshot.loadEvidence = 'startupReady';
  return report;
};
function block(report: MeshRouteReport | null, options: { previous?: boolean; legacy?: boolean; loading?: boolean; error?: boolean } = {}): string {
  return renderToStaticMarkup(<MobileMeshRouteBlock report={report} loading={!!options.loading} error={!!options.error}
    previous={!!options.previous} legacy={!!options.legacy} onRefresh={() => {}} servers={servers} />);
}
function details(report: MeshRouteReport | null, serverId?: string): string {
  return renderToStaticMarkup(<MobileMeshRouteEvidence report={report} serverId={serverId} nameOf={nameOf} />);
}

describe('mobile mesh route evidence', () => {
  it('uses the mobile route command and preserves the mobile platform boundary', () => {
    const backend = readFileSync(fileURLToPath(new URL('../../../../crates/system-integration/src/route_probe.rs', import.meta.url)), 'utf8');
    const page = readFileSync(fileURLToPath(new URL('./TunPage.tsx', import.meta.url)), 'utf8');
    expect(backend).toMatch(/Platform::Android\s*\|\s*Platform::Ios\s*=>\s*Ok\(TunnelProbeOutcome::Unsupported/);
    expect(page).not.toContain('.tunnelConflictReport(');
    expect(page).toContain('.meshRouteReport(');
    expect(page).toContain('<MobileMeshRouteBlock');
  });

  for (const lang of langs) {
    it(`${lang}: preview and unacknowledged reports cannot claim loaded ownership`, () => {
      h.lang = lang;
      const preview = clone();
      expect(meshRouteState(preview, 'b')).toBe('preview');
      expect(block(preview)).toContain(translate('mobileMeshRouteEvidence.summary.preview'));
      expect(details(preview, 'b')).toContain(translate('mobileMeshRouteEvidence.calculatedRanges'));
      expect(details(preview, 'b')).toContain(translate('mobileMeshRouteEvidence.zeroCalculated'));
      expect(details(preview, 'b')).not.toContain(translate('mobileMeshRouteEvidence.zeroEffective'));
      expect(details(preview, 'b')).not.toContain(translate('mobileMeshRouteEvidence.loadedRanges'));
      preview.snapshot.scope = 'applied';
      preview.snapshot.configSource = 'running';
      preview.snapshot.runGeneration = 1;
      preview.snapshot.loadEvidence = 'fileWrittenUnacknowledged';
      expect(meshRouteState(preview, 'b')).toBe('pending');
      expect(block(preview)).toContain(translate('mobileMeshRouteEvidence.summary.pending'));
    });

    it(`${lang}: applied overlap shows source, earlier owner, DNS, and preceding rules`, () => {
      h.lang = lang;
      const report = applied();
      expect(meshRouteState(report, 'b')).toBe('none');
      expect(meshRouteState(report)).toBe('unknown');
      const markup = details(report, 'b');
      expect(markup).not.toContain('>0<'); // no React-rendered length 0 beside closed evidence
      for (const value of ['10.20.1.0/24', translate('mobileMeshRouteEvidence.scope.applied'),
        translate('mobileMeshRouteEvidence.source.declared'), translate('mobileMeshRouteEvidence.zeroEffective'),
        translate('mobileMeshRouteEvidence.blockedConfirmed', { node: '家里' }),
        translate('mobileMeshRouteEvidence.dnsOwner', { node: '家里' }),
        translate('mobileMeshRouteEvidence.customRule'), translate('mobileMeshRouteEvidence.appRule'),
        translate('mobileMeshRouteEvidence.layerBoundary')]) expect(markup).toContain(value);
    });

    it(`${lang}: null effective, empty requested, legacy, previous, and global omission stay distinct`, () => {
      h.lang = lang;
      const report = applied();
      expect(meshRouteState(report, 'c')).toBe('unknown');
      expect(details(report, 'c')).toContain(translate('mobileMeshRouteEvidence.effectiveUnknown'));
      report.results[2].effective = [];
      report.results[2].coverage = 'full';
      report.results[2].unknownReasons = [];
      expect(meshRouteState(report, 'c')).toBe('noRequested');
      expect(details(report, 'c')).toContain(translate('mobileMeshRouteEvidence.noRequested'));
      expect(details(report, 'c')).not.toContain(translate('mobileMeshRouteEvidence.zeroEffective'));
      expect(details(report, 'c')).not.toContain(translate('mobileMeshRouteEvidence.coverage.none'));
      expect(asMeshRouteReport({ servers: [], zeroCoverageServerIds: [], absorbedCount: 0 })).toBeNull();
      expect(block(null, { legacy: true })).toContain(translate('mobileMeshRouteEvidence.summary.legacy'));
      expect(block(report, { previous: true, error: true })).toContain(translate('mobileMeshRouteEvidence.summary.previous'));
      expect(block(report, { previous: true, error: true })).toContain('role="alert"');
      expect(block(report, { loading: true })).toContain('disabled');
      expect(meshRouteState({ ...report, unknownReasons: ['resourceLimitExceeded'] }, 'a')).toBe('unknown');
      expect(meshRouteState({ ...report, totalCandidateCount: report.snapshot.candidates.length + 1 }, 'a')).toBe('unknown');
      expect(details({ ...report, totalCandidateCount: report.snapshot.candidates.length + 1 }, 'a'))
        .toContain(translate('mobileMeshRouteEvidence.coverageCaveat'));
      expect(details({ ...report, totalCandidateCount: report.snapshot.candidates.length + 1 }, 'a'))
        .not.toContain(translate('mobileMeshRouteEvidence.loadedRanges'));
    });
  }

  it('bootstrap overlap is tentative and does not identify a duplicate account', () => {
    h.lang = 'zh-CN';
    const report = applied();
    report.results[1].requested = [{ cidr: '100.64.0.0/10', source: 'bootstrap' }];
    expect(meshRouteState(report, 'b')).toBe('bootstrapOverlap');
    expect(details(report, 'b')).toContain(translate('mobileMeshRouteEvidence.blockedCandidate', { node: '家里' }));
    expect(details(report, 'b')).not.toContain(translate('mobileMeshRouteEvidence.blockedConfirmed', { node: '家里' }));
  });

  it('aggregates active requested ranges: a public-only exit is neutral, full plus none is partial', () => {
    const report = applied();
    report.results[2].effective = [];
    report.results[2].coverage = 'full';
    report.results[2].unknownReasons = [];
    expect(meshRouteState(report, 'c')).toBe('noRequested');
    expect(meshRouteState(report)).toBe('partial'); // a is full, b is fully overlapped
    report.results[1].requested = [];
    report.results[1].effective = [];
    report.results[1].blockedBy = [];
    report.results[1].coverage = 'full';
    expect(meshRouteState(report)).toBe('full'); // a plus two neutral exits
    report.results[0].effective = [];
    report.results[0].coverage = 'none';
    expect(meshRouteState(report)).toBe('none');
    report.results[0].effective = null;
    report.results[0].coverage = 'unknown';
    expect(meshRouteState(report)).toBe('unknown');
  });
});
