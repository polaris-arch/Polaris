// Register the oracle's actual-close/join hook while Vitest collects this file.
import { cssChain, measure } from '@/styles/css-oracle.test-support';
import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import type { ServerConfig, UserConfig } from '@/contracts/types';
import { DEMO_CONFIG } from '../../../../harness-fixture';
import { meshRouteEvidenceRows } from '@/domain/mesh-route-evidence';
import { createLatestReportLoader, type ReportLoadState } from './latest-report-loader';

const locale = JSON.parse(readFileSync(fileURLToPath(new URL('../../../i18n/locales/zh-CN.json', import.meta.url)), 'utf8')) as Record<string, unknown>;
function translate(key: string, vars?: Record<string, string>): string {
  let value: unknown = locale;
  for (const part of key.split('.')) value = (value as Record<string, unknown> | undefined)?.[part];
  if (typeof value !== 'string') throw new Error(`missing locale key: ${key}`);
  return value.replace(/\{\{(\w+)\}\}/g, (_, name: string) => vars?.[name] ?? '');
}

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: translate, i18n: { language: 'zh-CN' } }),
}));

const { MeshRouteEvidenceBlock } = await import('./MeshRouteEvidenceBlock');
const fixture = JSON.parse(readFileSync(fileURLToPath(new URL('../../../contracts/mesh-route-report.fixture.json', import.meta.url)), 'utf8')) as MeshRouteReport;
const settingsTunSource = readFileSync(fileURLToPath(new URL('./SettingsTun.tsx', import.meta.url)), 'utf8');
const servers = [
  { id: 'a', name: '家庭网络', protocol: 'tailscale' },
  { id: 'b', name: '公司网络', protocol: 'tailscale' },
  { id: 'c', name: '按目标选择', protocol: 'wireguard' },
] as ServerConfig[];
const render = (report: MeshRouteReport | null = fixture) =>
  renderToStaticMarkup(<MeshRouteEvidenceBlock report={report} servers={servers} />);
const clone = (): MeshRouteReport => structuredClone(fixture);

function runningReport(): MeshRouteReport {
  const report = clone();
  report.snapshot.scope = 'applied';
  report.snapshot.configSource = 'running';
  report.snapshot.configVersion = 'running-v1';
  report.snapshot.runGeneration = 7;
  report.snapshot.loadEvidence = 'startupReady';
  return report;
}

function incompleteRunningReport(): MeshRouteReport {
  const report = runningReport();
  const [a, b, opaque] = report.snapshot.candidates;
  const [aResult, bResult, opaqueResult] = report.results;
  // Producer-reachable order: A proves a range, then an opaque predecessor makes
  // the rest of B unknown without erasing B's already proved intersection with A.
  report.snapshot.candidates = [a, opaque, b];
  b.configuredCidrs.push('10.21.0.0/16');
  b.matchCidrs!.push({ cidr: '10.21.0.0/16', source: 'declared' });
  bResult.requested.push({ cidr: '10.21.0.0/16', source: 'declared' });
  bResult.coverage = 'unknown';
  bResult.effective = null;
  bResult.unknownReasons = ['earlierOpaqueMatchSet'];
  report.results = [aResult, opaqueResult, bResult];
  return report;
}

// This is the main surface even while both native details elements are closed.
const mainSurface = (report: MeshRouteReport) => render(report).split('<details')[0];

describe('P1 route evidence is a bounded report, not a live ownership claim', () => {
  it('the settings page consumes the actual report command and keeps the legacy estimate off this surface', () => {
    expect(settingsTunSource).toContain('api.config.meshRouteReport()');
    expect(settingsTunSource).toContain('<MeshRouteEvidenceBlock');
    expect(settingsTunSource).not.toContain('api.config.endpointForceRouteReport()');
  });

  it('provides runtime commit labels in both surfaces for all supported locales', () => {
    for (const language of ['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa']) {
      const text = JSON.parse(readFileSync(fileURLToPath(new URL(`../../../i18n/locales/${language}.json`, import.meta.url)), 'utf8'));
      expect(text.settings.tun.meshEvidence.load.runtimeCommitted).toEqual(expect.any(String));
      expect(text.mobileMeshRouteEvidence.load.runtimeCommitted).toEqual(expect.any(String));
      expect(Object.keys(text.settings.tun.meshEvidence.load).sort())
        .toEqual(Object.keys(text.mobileMeshRouteEvidence.load).sort());
    }
  });

  it('renders committed runtime loading as separate evidence from startup loading', () => {
    const report = clone();
    report.snapshot.scope = 'applied';
    report.snapshot.loadEvidence = 'runtimeCommitted';
    expect(render(report)).toContain('运行时加载已提交');
    expect(render(report)).not.toContain('启动加载已确认');
  });

  it('keeps producer order and distinguishes full, none, and unknown without CIDR calculation', () => {
    const rows = meshRouteEvidenceRows(fixture);
    expect(rows.map((row) => row.candidate.serverId)).toEqual(['a', 'b', 'c']);
    expect(rows.map((row) => row.resolution?.coverage)).toEqual(['full', 'none', 'unknown']);
    expect(rows[1].resolution?.effective).toEqual([]);
    expect(rows[2].resolution?.effective).toBeNull();
    const markup = render();
    expect(markup).toContain('家庭网络');
    expect(markup).toContain('公司网络');
    expect(markup).toContain('按目标选择');
    expect(markup).toContain('10.20.1.0/24');
    expect(markup).toContain('本层覆盖情况: 全部');
    expect(markup).toContain('本层覆盖情况: 未覆盖');
    expect(markup).toContain('本层覆盖情况: 未知');
    expect(markup).toContain('已确认空集');
    expect(markup).toContain('网段集合未知，不等于零覆盖。');
    expect(markup).not.toContain('preferredBy');
  });

  it('shows partial only when the report says partial, with no frontend subtraction', () => {
    const report = clone();
    report.results[1].coverage = 'partial';
    report.results[1].effective = ['10.20.1.0/25'];
    const markup = render(report);
    expect(markup).toContain('部分');
    expect(markup).toContain('10.20.1.0/25');
    expect(meshRouteEvidenceRows(report)[1].resolution?.effective).toEqual(['10.20.1.0/25']);
  });

  it('labels preview/source/load evidence, DNS handler, prior exceptions, and distinct range sources', () => {
    const markup = render();
    expect(markup).toContain('草稿预览');
    expect(markup).toContain('草稿配置');
    expect(markup).toContain('加载状态未知');
    expect(markup).toContain('家庭网络');
    expect(markup).toContain('自定义规则可能覆盖这些路由');
    expect(markup).toContain('应用规则可能覆盖这些路由');
    expect(markup).toContain('保留');
    expect(markup).toContain('观测到的主机地址');
    expect(markup).toContain('引用网段文件采样时间');
    expect(markup).toContain('节点声明的网段（不是已分配网段）');
    expect(markup).toContain('仅在这份报告中证实');
    expect(markup).toContain('本网段包含于先行节点的网段');
    expect(markup).toContain('始终路由所声明的子网');
    expect(markup).toContain('已选为出口节点');
    expect(markup).toContain('被规则指定');
    expect(markup).toContain('规则目标范围无法枚举');
    expect(markup).toContain('不证明实际流量归属或连通性');
  });

  it('does not turn missing results or unreadable files into a proved empty set', () => {
    const report = clone();
    report.results = report.results.filter((result) => result.serverId !== 'c');
    report.snapshot.candidates[0].referencedFile = {
      cidrs: null, readError: 'sensitive-local-path', sampledAtMs: null,
    };
    const markup = render(report);
    expect(meshRouteEvidenceRows(report)[2].resolution).toBeNull();
    expect(markup).toContain('引用文件内容未知或无法读取');
    expect(markup).toContain('网段集合未知，不等于零覆盖。');
    expect(markup).not.toContain('sensitive-local-path');
  });

  it('separates reported scope from load acknowledgement and marks stale snapshots', () => {
    const report = clone();
    report.snapshot.scope = 'persistedUnknown';
    report.snapshot.configSource = 'persisted';
    report.snapshot.loadEvidence = 'fileWrittenUnacknowledged';
    report.snapshot.snapshotStale = true;
    report.results[1].blockedBy[0].confirmed = false;
    const markup = render(report);
    expect(markup).toContain('配置快照，当前加载状态未确认');
    expect(markup).toContain('文件已写入，内核加载未确认');
    expect(markup).toContain('快照已过期');
    expect(markup).toContain('未证实为归属节点');
  });

  it('does not label a running snapshot file-read diagnostic as a core load failure', () => {
    const report = clone();
    report.snapshot.scope = 'persistedUnknown';
    report.snapshot.configSource = 'running';
    report.snapshot.loadEvidence = 'unknown';
    report.unknownReasons = ['fileReadFailed'];
    report.results[0].effective = null;
    report.results[0].coverage = 'unknown';
    const markup = render(report);
    expect(markup).toContain('配置快照，当前加载状态未确认');
    expect(markup).toContain('运行配置');
    expect(markup).toContain('加载状态未知');
    expect(markup).toContain('引用文件或加载证据不可用');
    expect(markup).toContain('网段集合未知，不等于零覆盖。');
    expect(markup).not.toContain('加载失败');
  });

  it('keeps unavailable and known-empty reports separate', () => {
    expect(render(null)).toContain('暂时无法取得路由证据');
    const report = clone();
    report.snapshot.candidates = [];
    report.results = [];
    expect(render(report)).not.toContain('暂时无法取得路由证据');
  });

  it('exposes report-level truncation and uncertainty without treating omitted nodes as conflict-free', () => {
    const report = clone();
    report.totalCandidateCount = 5;
    report.unknownReasons = ['resourceLimitExceeded'];
    const markup = render(report);
    expect(markup).toContain('已列出 / 总路由候选数: 3 / 5');
    expect(markup).toContain('缺席节点的冲突与覆盖情况未知');
    expect(markup).toContain('报告整体的不确定原因');
    expect(markup).toContain('诊断取材或结果超过限制');
  });

  it('keeps unresolved IDs inside technical details and localizes generation reasons', () => {
    const report = clone();
    report.results[1].blockedBy[0].serverId = 'unresolved-private-id';
    report.snapshot.candidates[2].generated = false;
    report.snapshot.candidates[2].engagedReason = 'generationFailed';
    report.snapshot.candidates[2].generationReason = 'control-url-ip';
    const markup = render(report);
    expect(markup).toContain('未知节点');
    expect(markup).toContain('unresolved-private-id');
    expect(markup.slice(0, markup.indexOf('技术详情'))).not.toContain('unresolved-private-id');
    expect(markup.slice(markup.indexOf('展开逐节点路由证据'))).not.toContain('unresolved-private-id');
    expect(markup).toContain('候选生成失败');
    expect(markup).not.toContain('control-url-ip');
    expect(markup).toContain('配置版本');
    expect(markup.indexOf('技术详情')).toBeLessThan(markup.indexOf('配置版本'));
  });

  it('a late preview cannot replace a newer applied report with another version', async () => {
    let finishPreview!: (report: MeshRouteReport) => void;
    let finishApplied!: (report: MeshRouteReport) => void;
    const preview = new Promise<MeshRouteReport>((resolve) => { finishPreview = resolve; });
    const applied = new Promise<MeshRouteReport>((resolve) => { finishApplied = resolve; });
    const jobs = [preview, applied];
    const published: ReportLoadState<MeshRouteReport>[] = [];
    const loader = createLatestReportLoader(() => jobs.shift()!, (state) => published.push(state));
    loader.refresh();
    loader.refresh();
    const newer = clone();
    newer.snapshot.scope = 'applied';
    newer.snapshot.configSource = 'running';
    newer.snapshot.configVersion = 'new-run';
    newer.snapshot.runGeneration = 2;
    newer.snapshot.loadEvidence = 'startupReady';
    finishApplied(newer);
    await applied;
    await Promise.resolve();
    finishPreview(fixture);
    await preview;
    await Promise.resolve();
    expect(published[published.length - 1]?.report?.snapshot).toMatchObject({
      scope: 'applied', configVersion: 'new-run', runGeneration: 2,
    });
    expect(published).toHaveLength(3);
  });
});


it('cross-platform mesh warnings are computed-visible without exposing Linux-only warnings', async () => {
  const { cssChain, measure } = await import('@/styles/css-oracle.test-support');
  const css = (await cssChain('desktop')).map(item => item.css);
  const report = clone();
  report.snapshot.scope = 'persistedUnknown';
  report.snapshot.configSource = 'persisted';
  report.snapshot.loadEvidence = 'fileWrittenUnacknowledged';
  report.snapshot.snapshotStale = true;
  report.totalCandidateCount = 5;
  report.unknownReasons = ['resourceLimitExceeded'];
  const status = '.set-block > .card-sub';
  const help = '.info-i[role="img"][tabindex="0"][aria-label][data-tip]';
  for (const os of ['win', 'mac', 'lin']) {
    const actual = await measure({ css, html: render(report) + '<div class="plat-warn control-linux-only">Linux only</div>', rootAttrs: { 'data-os': os } }, [
      { select: '.mesh-route-warning', props: ['display', 'visibility'], many: 'all' },
      { select: status, props: ['display', 'visibility'], many: 'all' },
      { select: help, props: ['display', 'visibility'] },
      { select: '.control-linux-only', props: ['display'] },
    ]);
    // Stale, omitted candidates and report-wide uncertainty remain on the main surface.
    expect(actual.count('.mesh-route-warning')).toBe(3);
    expect(actual.getAll('.mesh-route-warning', 'display').every(display => display !== 'none')).toBe(true);
    expect(actual.rectAll('.mesh-route-warning').every(rect => rect.height > 0)).toBe(true);
    // Combined scope/load/sample context, DNS owner and actual preceding exceptions stay visible; truncated count is in its warning.
    expect(actual.count(status)).toBe(3);
    expect(actual.getAll(status, 'visibility')).toEqual(Array(3).fill('visible'));
    expect(actual.rectAll(status).every(rect => rect.height > 0)).toBe(true);
    expect(actual.count(help)).toBe(1);
    expect(actual.rect(help).height).toBeGreaterThan(0);
    expect(actual.get('.control-linux-only', 'display')).toBe(os === 'lin' ? 'flex' : 'none');
  }
}, 30_000);

it('TUN preview identity changes for mesh routes, real server input and observed generation only', async () => {
  const { tunPreviewIdentity } = await import('./SettingsTun');
  const config: UserConfig = {
    ...DEMO_CONFIG,
    servers: [{ id: 'mesh', name: 'Mesh', protocol: 'openvpn-client', address: 'mesh.fixture.invalid', port: 1194, meshRoutes: ['10.1.0.0/16'] }],
  };
  const key = tunPreviewIdentity(config, '["100.64.0.1"]', 'running:1');
  expect(tunPreviewIdentity({ ...config, language: 'fa' }, '["100.64.0.1"]', 'running:1')).toBe(key);
  expect(tunPreviewIdentity({ ...config, servers: (config.servers ?? []).map(server => ({ ...server, meshRoutes: ['10.2.0.0/16'] })) }, '["100.64.0.1"]', 'running:1')).not.toBe(key);
  expect(tunPreviewIdentity(config, '["100.64.0.2"]', 'running:1')).not.toBe(key);
  expect(tunPreviewIdentity(config, '["100.64.0.1"]', 'running:2')).not.toBe(key);
  expect(settingsTunSource).toContain('tunPreviewIdentity(savedConfig, observationKey, previewGeneration)');
});

describe('closed desktop route details keep actionable summaries on the main surface', () => {
  it('names the affected node and earlier owner and distinguishes none from partial', () => {
    const report = runningReport();
    const main = mainSurface(report);
    expect(main).toContain('本报告：「公司网络」的目标网段均先由「家庭网络」匹配。');
    expect(main).toContain('data-mesh-route-summary="none"');
    expect(main).not.toContain('10.20.1.0/24');
    report.results[1].coverage = 'partial';
    report.results[1].effective = ['10.21.0.0/16'];
    expect(mainSurface(report)).toContain('本报告：「公司网络」部分网段先由「家庭网络」匹配。');
    expect(mainSurface(report)).toContain('data-mesh-route-summary="partial"');
    report.snapshot.loadEvidence = 'runtimeCommitted';
    expect(mainSurface(report)).toContain('data-mesh-route-summary="partial"');
  });

  it('retains proved blockers with remaining Unknown instead of claiming complete coverage', () => {
    for (const reasonAt of ['candidate', 'result', 'coverage', 'ranges'] as const) {
      const report = runningReport();
      if (reasonAt === 'candidate') report.snapshot.candidates[1].unknownReasons = ['fileReadFailed'];
      if (reasonAt === 'result') report.results[1].unknownReasons = ['earlierOpaqueMatchSet'];
      if (reasonAt === 'coverage') report.results[1].coverage = 'unknown';
      if (reasonAt === 'ranges') report.results[1].effective = null;
      const main = mainSurface(report);
      expect(main, reasonAt).toContain('本报告：「公司网络」部分网段先由「家庭网络」匹配，其余归属待确认。');
      expect(main, reasonAt).toContain('data-mesh-route-summary="incomplete"');
      expect(main, reasonAt).not.toContain('data-mesh-route-summary="none"');
      expect(main, reasonAt).not.toContain('data-mesh-route-summary="partial"');
    }
  });

  it('keeps purely unknown, unconfirmed-only and ungenerated results without a named owner', () => {
    for (const reasonAt of ['missing', 'unconfirmed', 'notGenerated'] as const) {
      const report = runningReport();
      if (reasonAt === 'missing') report.results.splice(1, 1);
      if (reasonAt === 'unconfirmed') report.results[1].blockedBy[0].confirmed = false;
      if (reasonAt === 'notGenerated') report.snapshot.candidates[1].generated = false;
      const main = mainSurface(report);
      expect(main, reasonAt).toContain('公司网络：本层路由归属待确认。');
      expect(main, reasonAt).not.toContain('先由「家庭网络」匹配');
      expect(main, reasonAt).not.toContain('data-mesh-route-summary="incomplete"');
    }
  });

  it('keeps the confirmed subset before an opaque predecessor and excludes unconfirmed blockers', () => {
    const report = incompleteRunningReport();
    expect(mainSurface(report)).toContain('本报告：「公司网络」部分网段先由「家庭网络」匹配，其余归属待确认。');
    const affected = report.results.find(result => result.serverId === 'b')!;
    affected.blockedBy.push({ serverId: 'c', cidr: '10.21.0.0/16', relation: 'equal', confirmed: false });
    for (const load of ['startupReady', 'runtimeCommitted'] as const) {
      report.snapshot.loadEvidence = load;
      const main = mainSurface(report);
      expect(main).toContain('本报告：「公司网络」部分网段先由「家庭网络」匹配，其余归属待确认。');
      expect(main).toContain('按目标选择：本层路由归属待确认。');
      expect(main).not.toContain('家庭网络 · 按目标选择');
      expect(main).not.toContain('data-mesh-route-summary="none"');
      expect(main).not.toContain('data-mesh-route-summary="partial"');
    }
  });

  it('labels preview results as preview even when the calculated report has confirmed blockers', () => {
    for (const report of [clone(), incompleteRunningReport()]) {
      report.snapshot.scope = 'preview';
      const main = mainSurface(report);
      expect(main).toContain('预览：「公司网络」的当前路由归属尚未确认。');
      expect(main).toContain('data-mesh-route-summary="preview"');
      expect(main).not.toContain('先由「家庭网络」匹配');
      expect(main).not.toContain('data-mesh-route-summary="none"');
      expect(main).not.toContain('data-mesh-route-summary="incomplete"');
    }
  });

  it('does not reuse a blocker as a current owner without a fresh acknowledged running identity', () => {
    const mutations: Array<(report: MeshRouteReport) => void> = [
      (report) => { report.snapshot.snapshotStale = true; },
      (report) => { report.snapshot.loadEvidence = 'fileWrittenUnacknowledged'; },
      (report) => { report.snapshot.loadEvidence = 'loadFailed'; },
      (report) => { report.snapshot.scope = 'persistedUnknown'; },
      (report) => { report.snapshot.configSource = 'persisted'; },
      (report) => { report.snapshot.configVersion = null; },
      (report) => { report.snapshot.runGeneration = null; },
    ];
    for (const mutate of mutations) {
      for (const report of [runningReport(), incompleteRunningReport()]) {
        mutate(report);
        const main = mainSurface(report);
        expect(main).toContain('公司网络：本层路由归属待确认。');
        expect(main).not.toContain('先由「家庭网络」匹配');
        expect(main).not.toContain('data-mesh-route-summary="none"');
        expect(main).not.toContain('data-mesh-route-summary="incomplete"');
      }
    }
  });

  it('keeps bootstrap overlap tentative instead of confirming an account or a current owner', () => {
    for (const report of [runningReport(), incompleteRunningReport()]) {
      report.results.find(result => result.serverId === 'b')!.requested = [{ cidr: '100.64.0.0/10', source: 'bootstrap' }];
      const main = mainSurface(report);
      expect(main).toContain('公司网络：默认范围重叠，真实网段待确认。');
      expect(main).toContain('data-mesh-route-summary="unknown"');
      expect(main).not.toContain('先由「家庭网络」匹配');
      expect(main).not.toContain('data-mesh-route-summary="incomplete"');
    }
  });

  it('adds no conflict or connectivity claim for a clear or empty report', () => {
    const report = runningReport();
    report.snapshot.candidates = report.snapshot.candidates.slice(0, 1);
    report.results = report.results.slice(0, 1);
    report.totalCandidateCount = 1;
    expect(mainSurface(report)).not.toContain('data-mesh-route-summary');
    report.snapshot.candidates = [];
    report.results = [];
    report.totalCandidateCount = 0;
    expect(mainSurface(report)).not.toContain('data-mesh-route-summary');
  });

  it('shows the known subset and remaining Unknown on macOS while route details remain closed', async () => {
    const css = (await cssChain('desktop')).map(item => item.css);
    const actual = await measure({ css, html: render(incompleteRunningReport()), rootAttrs: { 'data-os': 'mac' } }, [
      { select: '[data-mesh-route-summary="incomplete"]', props: ['display', 'visibility'] },
      { select: '[data-mesh-route-summary="unknown"]', props: ['display', 'visibility'] },
      { select: 'details:not([open]) > .mesh-route-nodes', props: ['display'] },
    ]);
    for (const kind of ['incomplete', 'unknown']) {
      const selector = `[data-mesh-route-summary="${kind}"]`;
      expect(actual.count(selector)).toBe(1);
      expect(actual.get(selector, 'visibility')).toBe('visible');
      expect(actual.rect(selector).height).toBeGreaterThan(0);
      expect(actual.ancestorPath(selector).some(ancestor => ancestor.startsWith('details'))).toBe(false);
    }
    expect(actual.count('details:not([open]) > .mesh-route-nodes')).toBe(1);
  }, 30_000);
});


it('keeps normal metadata in existing technical details while applied, preview and unsafe states remain explicit', () => {
  const report = clone();
  report.snapshot.scope = 'applied'; report.snapshot.configSource = 'running';
  report.snapshot.loadEvidence = 'startupReady'; report.snapshot.precedingExceptions = [];
  const html = render(report), main = html.slice(0, html.indexOf('<details'));
  expect(main).toContain('运行快照');
  expect(main).toContain('启动加载已确认');
  expect(main).toContain('报告采样时间');
  expect(main).not.toContain('运行配置');
  expect(main).not.toContain('已列出 / 总路由候选数');
  expect(html.slice(html.indexOf('技术详情'))).toContain('已列出 / 总路由候选数');
  expect(html.slice(html.indexOf('技术详情'))).toContain('运行配置');
  report.snapshot.scope = 'preview'; report.snapshot.configSource = 'draft'; report.snapshot.loadEvidence = 'unknown';
  const preview = render(report).split('<details')[0];
  expect(preview).toContain('草稿预览'); expect(preview).toContain('草稿配置'); expect(preview).toContain('加载状态未知');
  report.snapshot.scope = 'persistedUnknown'; report.snapshot.configSource = 'persisted';
  report.snapshot.loadEvidence = 'fileWrittenUnacknowledged'; report.snapshot.snapshotStale = true;
  report.snapshot.precedingExceptions = ['customRuleMayOverride'];
  const unsafe = render(report).split('<details')[0];
  expect(unsafe).toContain('已保存配置'); expect(unsafe).toContain('内核加载未确认');
  expect(unsafe).toContain('快照已过期'); expect(unsafe).toContain('自定义规则可能覆盖');
});
