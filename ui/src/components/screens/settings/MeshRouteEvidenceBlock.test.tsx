import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import type { ServerConfig } from '@/contracts/types';
import { meshRouteEvidenceRows } from '@/domain/mesh-route-evidence';
import { createLatestReportLoader, type ReportLoadState } from './latest-report-loader';

const locale = JSON.parse(readFileSync(fileURLToPath(new URL('../../../i18n/locales/zh-CN.json', import.meta.url)), 'utf8')) as Record<string, unknown>;
function translate(key: string): string {
  let value: unknown = locale;
  for (const part of key.split('.')) value = (value as Record<string, unknown> | undefined)?.[part];
  if (typeof value !== 'string') throw new Error(`missing locale key: ${key}`);
  return value;
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

describe('P1 route evidence is a bounded report, not a live ownership claim', () => {
  it('the settings page consumes the actual report command and keeps the legacy estimate off this surface', () => {
    expect(settingsTunSource).toContain('api.config.meshRouteReport()');
    expect(settingsTunSource).toContain('<MeshRouteEvidenceBlock');
    expect(settingsTunSource).not.toContain('api.config.endpointForceRouteReport()');
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
    expect(markup).toContain('不证明当前流量承载、连通或稳定归属');
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
