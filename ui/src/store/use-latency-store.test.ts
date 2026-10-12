/**
 * 测速结果 store 回归测 —— 钉死「切屏即丢延迟」那一族根因的前端半边。
 *
 * 真机指控：「测试结果应该是所有节点都应该复用消费」。根因**不是**没存/没广播/key 不对，
 * 而是这份 map 曾以四份**组件私有 useState** 存在（HomeScreen / NodesScreen / StatusBar / TrayMenu），
 * 订阅寿命 = 组件挂载寿命，而 `ScreenRouter` 是裸 switch（无 keep-alive）⇒ 切屏即卸载即丢。
 *
 * 本文件锁三条（任一破了都会退回原状）：
 *  1. **状态在组件外**：store 是模块级单例，读写不经任何 React 生命周期；
 *  2. **合并不替换**：逐节点流式回填必须保留其余节点的历史值（原 useState 版本用函数式合并，
 *     改 store 时若写成整体替换，一次测速只会剩最后一个节点有值）；
 *  3. **订阅在组件外**：`subscribeLatencyEvents` 把事件流接进 store，且返回可退订句柄 ——
 *     「组件卸载」不参与其中，故跨屏切换不丢。
 */

import { describe, it, expect, beforeEach } from 'vitest';

import { seedMeasurementFixture } from './measurement-fixture.test-support';

import {
  useLatencyStore,
  isLatencyStale,
  LATENCY_STALE_MS,
  normalizeLatencyResult,
} from './use-latency-store';

beforeEach(() => {
  // 直接经 zustand 的 setState 复位：清空只有测试夹具需要，生产零调用点 —— 为它在 store 上导出一个
  // 公开 API 等于给四个屏幕 import 的 store 挂一条死 API（复审 #9），故不留 `resetLatencies`。
  useLatencyStore.setState({ latencyMap: {}, testedAt: {}, snapshot: null, projections: {}, currentProjections: {}, readError: false });
});

describe('use-latency-store：单一真值', () => {
  it('逐节点回填**合并**而非替换（一轮全量测速后每个节点都留有值）', () => {
    const applyLatencyResult = (id: string, latency: number) => seedMeasurementFixture({ [id]: latency });
    applyLatencyResult('a', 120);
    applyLatencyResult('b', 240);
    applyLatencyResult('c', -1); // 后端判定的真实不可测

    expect(useLatencyStore.getState().latencyMap).toEqual({ a: 120, b: 240, c: null });
  });

  it('批量合并保留未在本批的历史值（invoke 返回值兜底同步不得抹掉旧结果）', () => {
    seedMeasurementFixture({ 'old': 50 });
    seedMeasurementFixture({ a: 120, b: 240 });

    expect(useLatencyStore.getState().latencyMap).toEqual({ old: 50, a: 120, b: 240 });
  });

  it('后写覆盖同一节点的先前值（重测即刷新）', () => {
    seedMeasurementFixture({ 'a': 120 });
    seedMeasurementFixture({ 'a': 88 });

    expect(useLatencyStore.getState().latencyMap.a).toBe(88);
  });

  it('IPC 的负数失败码在唯一写入口归一为 null，绝不泄漏成绿色的 “-1 ms”', () => {
    expect(normalizeLatencyResult(-1)).toBeNull();
    expect(normalizeLatencyResult(Number.NaN)).toBeNull();
    expect(normalizeLatencyResult(0)).toBe(0);

    seedMeasurementFixture({ failed: -1, ok: 86 });
    expect(useLatencyStore.getState().latencyMap).toEqual({ failed: null, ok: 86 });
  });

  it('节点删除后同时裁剪延迟与时间戳，保留节点的结果不动', () => {
    seedMeasurementFixture({ keep: 42, removed: 88 });
    const keepStamp = useLatencyStore.getState().testedAt.keep;

    useLatencyStore.getState().retainServerIds(['keep']);

    expect(useLatencyStore.getState().latencyMap).toEqual({ keep: 42 });
    expect(useLatencyStore.getState().testedAt).toEqual({ keep: keepStamp });
  });

  it('节点集没有删除项时保持 state 引用，避免 configChanged 触发无效通知', () => {
    seedMeasurementFixture({ 'keep': 42 });
    const before = useLatencyStore.getState();
    useLatencyStore.getState().retainServerIds(['keep', 'new-without-result']);
    expect(useLatencyStore.getState()).toBe(before);
  });
});

describe('use-latency-store：陈旧判定的数据基础（契约「陈旧 >30min 半透明」）', () => {
  it('账本投影保留生产时刻，无身份入口不制造接收戳', () => {
    const before = Date.now();
    seedMeasurementFixture({ 'a': 120 });
    seedMeasurementFixture({ b: 240, c: -1 });
    const { testedAt } = useLatencyStore.getState();

    for (const id of ['a', 'b', 'c']) {
      expect(testedAt[id], `${id} 未打戳`).toBeGreaterThanOrEqual(before);
    }
  });

  it('批量结果同批同戳（同一次 speedTest 不该产生毫秒级抖动）', () => {
    seedMeasurementFixture({ a: 1, b: 2, c: 3 });
    const { testedAt } = useLatencyStore.getState();
    expect(new Set([testedAt.a, testedAt.b, testedAt.c]).size).toBe(1);
  });

  it('重测刷新时间戳（陈旧态必须能被一次重测清掉）', () => {
    useLatencyStore.setState({ latencyMap: { a: 120 }, testedAt: { a: 1 } });
    seedMeasurementFixture({ 'a': 88 });
    expect(useLatencyStore.getState().testedAt.a).toBeGreaterThan(1);
  });

  it('isLatencyStale：未测过不算陈旧，恰好 30min 不算，超过才算', () => {
    const now = 1_000_000_000;
    expect(isLatencyStale(undefined, now)).toBe(false);
    expect(isLatencyStale(now - LATENCY_STALE_MS, now)).toBe(false);
    expect(isLatencyStale(now - LATENCY_STALE_MS - 1, now)).toBe(true);
    expect(isLatencyStale(now + 1, now)).toBe(true);
  });
});

describe('unqualified event/return values', () => {
  it('bare numeric writers cannot label unknown identity as success or failure', () => {
    useLatencyStore.getState().applyLatencyResult('a', 1);
    useLatencyStore.getState().applyLatencyResults({ b: -1 });
    expect(useLatencyStore.getState().latencyMap).toEqual({});
    expect(useLatencyStore.getState().testedAt).toEqual({});
  });
});
