import { beforeEach, describe, expect, it, vi } from 'vitest';

const ipc = vi.hoisted(() => ({
  add: vi.fn(),
  reorder: vi.fn(),
}));
vi.mock('@/ipc', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/ipc')>();
  return { ...real, api: { ...real.api, rules: { ...real.api.rules, add: ipc.add, reorder: ipc.reorder } } };
});
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn(), success: vi.fn(), info: vi.fn() } }));
import type { TFunction } from 'i18next';
import type { Rule } from '@/contracts/types';
import type { StagedEntry } from '@/lib/staged-config';
import { submitRule, type RuleSubmitArgs } from './rule-submit';

/**
 * RuleDialog「生效网络」的写侧（spec §8.3「RuleDialog 生效网络读写」）：走暂存腿（零 IPC），
 * 断言落进条目的 `networkProfileId`。读侧（编辑态从 base 预填）是 `useState(base?.networkProfileId ?? '')`
 * 一行，由下面的编辑用例间接覆盖：base 带什么、不改就原样写回。
 */
const t = ((key: string) => key) as TFunction;

function args(over: Partial<RuleSubmitArgs>): { a: RuleSubmitArgs; staged: StagedEntry[] } {
  const staged: StagedEntry[] = [];
  const a: RuleSubmitArgs = {
    t,
    conds: [{ t: 'domainSuffix', v: 'corp.example' }],
    name: '公司域名',
    setErrName: () => {},
    networkProfileId: '',
    logic: 'or',
    target: 'direct',
    dnsAction: 'server:builtin-domestic',
    dnsFallbackAction: 'server:builtin-domestic',
    dnsResolver: 'direct',
    dnsAnswerMode: 'real',
    dnsPredefinedRcode: 'NOERROR',
    dnsPredefinedAnswer: '',
    dnsPredefinedNs: '',
    dnsPredefinedExtra: '',
    isEdit: false,
    initialPlane: 'route',
    stagingEnabled: true,
    stage: (entry) => staged.push(entry),
    close: () => {},
    loadConfig: () => {},
    setSubmitting: () => {},
    ...over,
  };
  return { a, staged };
}

const base: Rule = {
  id: 'r-1',
  type: 'domainSuffix',
  values: ['corp.example'],
  action: 'direct',
  enabled: true,
  remarks: '公司域名',
  networkProfileId: 'np-corp',
  effects: { route: { action: 'direct' } },
};

describe('submitRule · networkProfileId', () => {
  it('新建：选了场景 ⇒ 写入；任何网络 ⇒ 不写这个键', async () => {
    const withProfile = args({ networkProfileId: 'np-corp' });
    await submitRule(withProfile.a);
    expect((withProfile.staged[0].nextValue as Rule).networkProfileId).toBe('np-corp');

    const any = args({});
    await submitRule(any.a);
    expect('networkProfileId' in (any.staged[0].nextValue as Rule)).toBe(false);
  });

  it('编辑：保持原场景原样写回；改成任何网络 ⇒ 清掉（序列化后键消失）', async () => {
    const keep = args({ isEdit: true, base, networkProfileId: 'np-corp' });
    await submitRule(keep.a);
    expect((keep.staged[0].nextValue as Rule).networkProfileId).toBe('np-corp');

    const clear = args({ isEdit: true, base, networkProfileId: '' });
    await submitRule(clear.a);
    const next = JSON.parse(JSON.stringify(clear.staged[0].nextValue)) as Rule;
    expect('networkProfileId' in next).toBe(false);
  });

  it('DNS 平面同一字段（一个字段覆盖两个平面，spec §3.2 方案 A）', async () => {
    const dns = args({ initialPlane: 'dns', networkProfileId: 'np-corp', dnsAction: 'server:builtin-netenv-dhcp' });
    await submitRule(dns.a);
    const next = dns.staged[0].nextValue as Rule;
    expect(next.networkProfileId).toBe('np-corp');
    expect(next.effects?.dns?.action).toEqual({ type: 'server', serverId: 'builtin-netenv-dhcp' });
    expect(dns.staged[0].entityPath).toEqual(['dnsRules', dns.staged[0].entityPath[1]]);
  });
});

describe('新建带场景的规则插到最前（spec §3.4-4，不改排序协议）', () => {
  const planeOrder = { ruleIds: ['a', 'b', 'c'], persistedOrder: ['c', 'a', 'b'] };
  beforeEach(() => {
    ipc.add.mockReset();
    ipc.reorder.mockReset();
  });

  it('暂存腿：规则条目之后追加整序列顺序条目（与拖拽排序同形），新 id 在首位', async () => {
    const x = args({ networkProfileId: 'np-corp', planeOrder });
    await submitRule(x.a);
    expect(x.staged).toHaveLength(2);
    const newId = (x.staged[0].nextValue as Rule).id;
    expect(x.staged[1]).toMatchObject({ id: 'order:routeRuleOrder', entityPath: ['routeRuleOrder'] });
    expect(x.staged[1].nextValue).toEqual([newId, 'c', 'a', 'b']);
  });

  it('直写腿：rules_add 之后用既有 rules_reorder 把后端发的 id 排到最前', async () => {
    ipc.add.mockResolvedValue({ id: 'rule_new' });
    ipc.reorder.mockResolvedValue(undefined);
    const x = args({ networkProfileId: 'np-corp', planeOrder, stagingEnabled: false, initialPlane: 'dns', dnsAction: 'server:builtin-domestic' });
    await submitRule(x.a);
    expect(ipc.reorder).toHaveBeenCalledWith(['rule_new', 'c', 'a', 'b'], 'dns');
  });

  it('反向对照：不挂场景 / 编辑已有规则 ⇒ 不动顺序（沿用追加到末尾 / 原位置）', async () => {
    const plain = args({ planeOrder });
    await submitRule(plain.a);
    expect(plain.staged).toHaveLength(1);

    ipc.add.mockResolvedValue({ id: 'rule_new' });
    await submitRule(args({ planeOrder, stagingEnabled: false }).a);
    expect(ipc.reorder).not.toHaveBeenCalled();

    const edit = args({ isEdit: true, base, networkProfileId: 'np-corp', planeOrder });
    await submitRule(edit.a);
    expect(edit.staged).toHaveLength(1);
  });

  it('重排失败不当作保存失败：弹窗照常关闭', async () => {
    ipc.add.mockResolvedValue({ id: 'rule_new' });
    ipc.reorder.mockRejectedValue(new Error('boom'));
    const close = vi.fn();
    await submitRule(args({ networkProfileId: 'np-corp', planeOrder, stagingEnabled: false, close }).a);
    expect(close).toHaveBeenCalledTimes(1);
  });
});
