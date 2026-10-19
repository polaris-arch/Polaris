import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const ipc = vi.hoisted(() => ({
  add: vi.fn(),
  update: vi.fn(),
  reorder: vi.fn(),
  validateRegex: vi.fn(),
}));
vi.mock('@/ipc', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/ipc')>();
  return { ...real, api: { ...real.api, rules: { ...real.api.rules, add: ipc.add, update: ipc.update, reorder: ipc.reorder, validateRegex: ipc.validateRegex } } };
});
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn(), success: vi.fn(), info: vi.fn() } }));
import type { TFunction } from 'i18next';
import type { Rule } from '@/contracts/types';
import type { StagedEntry } from '@/lib/staged-config';
import { submitRule, type RuleSubmitArgs } from './rule-submit';
import { formatConditionValues } from './rule-cond';

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

beforeEach(() => { ipc.validateRegex.mockReset().mockImplementation(async (patterns: string[]) => patterns.map(() => true)); useAppStore.setState({ rules: [base], dnsRules: [base], proxyStarting: true, proxyStopping: false }); useStagedConfigStore.setState({ enabled: true, entries: [] }); });

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

describe('submitRule · validation location', () => {
  it('reports one deterministic destination when both name and conditions are empty', async () => {
    const onValidationError = vi.fn();
    const setErrName = vi.fn();
    const x = args({ name: '', conds: [{ t: 'domainSuffix', v: '' }], onValidationError, setErrName });
    await submitRule(x.a);
    expect(setErrName).toHaveBeenCalledWith(true);
    expect(onValidationError).toHaveBeenCalledOnce();
    expect(onValidationError).toHaveBeenCalledWith('basic');
    expect(x.staged).toHaveLength(0);
  });

  it('sends missing or malformed conditions to the condition group without staging a rule', async () => {
    for (const conds of [
      [{ t: 'domainSuffix' as const, v: '' }],
      [{ t: 'ipCidr' as const, v: '10.0.0.0/40' }],
    ]) {
      const onValidationError = vi.fn();
      const x = args({ conds, onValidationError });
      await submitRule(x.a);
      expect(onValidationError).toHaveBeenCalledOnce();
      expect(onValidationError).toHaveBeenCalledWith('cond');
      expect(x.staged).toHaveLength(0);
    }
  });

  it('does not invoke presentation callback on successful submission', async () => {
    const onValidationError = vi.fn();
    await submitRule(args({ onValidationError }).a);
    expect(onValidationError).not.toHaveBeenCalled();
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


describe('submitRule typed regex values', () => {
  it.each(['route', 'dns'] as const)('submits and re-edits comma-bearing expressions without splitting in %s', async (initialPlane) => {
    const values = ['^[a,b]\\.example$', '^a{1,3}\\.example$', '^\\D+$'];
    const create = args({ initialPlane, conds: [{ t: 'domainRegex', v: values.join('\n') }] });
    await submitRule(create.a);
    const saved = create.staged[0].nextValue as Rule;
    expect(saved.values).toEqual(values);
    useAppStore.setState({ rules: [saved], dnsRules: [saved] });
    const edit = args({ initialPlane, isEdit: true, base: saved, conds: [{ t: 'domainRegex', v: formatConditionValues(saved.type, saved.values) }] });
    await submitRule(edit.a);
    expect((edit.staged[0].nextValue as Rule).values).toEqual(values);
  });
  it('keeps ordinary domain comma and newline behavior', async () => {
    const create = args({ conds: [{ t: 'domain', v: 'A.example, b.example\nc.example' }] });
    await submitRule(create.a);
    expect((create.staged[0].nextValue as Rule).values).toEqual(['A.example', 'b.example', 'c.example']);
  });
});

describe('submitRule · independent native Auto references', () => {
  it.each(['global', 'manual', 'mesh'])('persists %s without a synthetic node id', async (scope) => {
    const { a, staged } = args({ target: `auto:${scope}` });
    await submitRule(a);
    const rule = staged[0].nextValue as Rule;
    expect(rule.targetAutoSelect).toEqual({ scope });
    expect(rule.effects?.route?.targetAutoSelect).toEqual({ scope });
    expect(rule.targetServerId).toBeUndefined();
    expect(rule.effects?.route?.targetServerId).toBeUndefined();
  });
  it('keeps each subscription reference separate and restores a manual target on edit', async () => {
    for (const subscriptionId of ['A / 香港', 'D']) {
      const { a, staged } = args({ target: `auto:subscription:${encodeURIComponent(subscriptionId)}` });
      await submitRule(a);
      expect((staged[0].nextValue as Rule).targetAutoSelect).toEqual({ scope: 'subscription', subscriptionId });
    }
    const { a, staged } = args({ target: 'node:manual-node', isEdit: true, base: { ...base, targetAutoSelect: { scope: 'global' }, effects: { route: { action: 'proxy', targetAutoSelect: { scope: 'global' } } } } });
    useAppStore.setState({ rules: [a.base!] });
    await submitRule(a);
    const rule = staged[0].nextValue as Rule;
    expect(rule.targetServerId).toBe('manual-node');
    expect(rule.targetAutoSelect).toBeUndefined();
    expect(rule.effects?.route?.targetAutoSelect).toBeUndefined();
  });
});

describe('rule editor original entity admission', () => {
  it.each([true, false])('refuses changed or removed edits without creating/replacing (staged=%s)', async stagingEnabled => {
    for (const rules of [[], [{ ...base, remarks: 'changed elsewhere' }]]) {
      useAppStore.setState({ rules });
      ipc.add.mockClear();
      const x = args({ isEdit: true, base, stagingEnabled });
      await submitRule(x.a);
      expect(x.staged).toEqual([]);
      expect(ipc.add).not.toHaveBeenCalled();
    }
  });
  it('missing initial edit never turns into creation', async () => {
    const x = args({ isEdit: true, base: undefined });
    ipc.add.mockClear();
    await submitRule(x.a);
    expect(x.staged).toEqual([]);
    expect(ipc.add).not.toHaveBeenCalled();
  });
  it('unrelated entity changes do not invalidate this edit', async () => {
    useAppStore.setState({ rules: [base, { ...base, id: 'other', remarks: 'changed' }] });
    const x = args({ isEdit: true, base });
    await submitRule(x.a);
    expect(x.staged).toHaveLength(1);
  });
});

 it('direct edit passes the original entity to native CAS', async () => {
   ipc.update.mockResolvedValue(undefined);
   const { a } = args({ isEdit: true, base, stagingEnabled: false });
   await submitRule(a);
   expect(ipc.update).toHaveBeenLastCalledWith(expect.objectContaining({ id: base.id }), 'route', base);
 });


describe('authoritative regex admission before staged or direct save', () => {
  it.each(['[', '(', '*', '\\'])('rejects native invalid syntax verdict for %s before staging', async pattern => {
    ipc.validateRegex.mockResolvedValueOnce([false]);
    const request = args({ conds: [{ t: 'domainRegex', v: pattern }] });
    await submitRule(request.a);
    expect(ipc.validateRegex).toHaveBeenLastCalledWith([pattern]);
    expect(request.staged).toHaveLength(0);
  });
  it('preserves a valid Go expression even when matching preview would be unknown', async () => {
    const pattern = String.raw`\Q[a]+\E`;
    ipc.validateRegex.mockResolvedValueOnce([true]);
    const request = args({ conds: [{ t: 'domainRegex', v: pattern }] });
    await submitRule(request.a);
    expect((request.staged[0].nextValue as Rule).values).toEqual([pattern]);
  });
  it('cannot stage after input ownership expires during native admission', async () => {
    let resolve!: (values: boolean[]) => void;
    ipc.validateRegex.mockImplementationOnce(() => new Promise<boolean[]>(done => { resolve = done; }));
    let current = true;
    const request = args({ conds: [{ t: 'domainRegex', v: 'example' }], isCurrent: () => current });
    const operation = submitRule(request.a);
    current = false;
    resolve([true]);
    await operation;
    expect(request.staged).toHaveLength(0);
  });
  it('rejects a missing native verdict instead of treating it as success', async () => {
    ipc.validateRegex.mockResolvedValueOnce([]);
    const request = args({ conds: [{ t: 'domainRegex', v: 'example' }] });
    await submitRule(request.a);
    expect(request.staged).toHaveLength(0);
  });
});

it('independent: reports a dispatched write failure even after input ownership changes', async () => {
  const { toast } = await import('@/lib/error-handler');
  vi.mocked(toast.error).mockClear();
  let reject!: (e: Error) => void;
  ipc.add.mockImplementationOnce(() => new Promise((_resolve, no) => { reject = no; }));
  let current = true;
  const request = args({ stagingEnabled: false, isCurrent: () => current });
  const operation = submitRule(request.a);
  expect(ipc.add).toHaveBeenCalled();
  current = false; // still-mounted editable form changed while native write is in flight
  reject(new Error('DISK_WRITE_FAILED'));
  await operation;
  expect(toast.error).toHaveBeenCalled();
});

it('independent: does not use pre-await direct route after the core starts', async () => {
  let resolve!: (v: boolean[]) => void;
  ipc.validateRegex.mockImplementationOnce(() => new Promise<boolean[]>(yes => { resolve = yes; }));
  ipc.add.mockClear().mockResolvedValue({ id: 'new' });
  useStagedConfigStore.setState({ enabled: true, entries: [] });
  useAppStore.setState({ proxyStarting: false, proxyStopping: false, proxyStatus: undefined });
  const request = args({ stagingEnabled: false, conds: [{ t: 'domainRegex', v: 'example' }] });
  const operation = submitRule(request.a);
  useAppStore.setState({ proxyStarting: true });
  resolve([true]);
  await operation;
  expect(ipc.add).not.toHaveBeenCalled();
});

it.each(['stopping', 'entries'] as const)('rechecks staging after %s changes during admission', async change => {
  let resolve!: (v: boolean[]) => void;
  ipc.validateRegex.mockImplementationOnce(() => new Promise<boolean[]>(yes => { resolve = yes; }));
  ipc.add.mockClear();
  useStagedConfigStore.setState({ enabled: true, entries: [] });
  useAppStore.setState({ proxyStarting: false, proxyStopping: false, proxyStatus: undefined });
  const request = args({ stagingEnabled: false, conds: [{ t: 'domainRegex', v: 'example' }] });
  const operation = submitRule(request.a);
  if (change === 'stopping') useAppStore.setState({ proxyStopping: true });
  else useStagedConfigStore.setState({ entries: [{ id: 'other', kind: 'rule', label: 'other', entityPath: ['trafficRules', 'other'], nextValue: base }] });
  resolve([true]);
  await operation;
  expect(ipc.add).not.toHaveBeenCalled();
  expect(request.staged).toHaveLength(1);
});
it('unavailable syntax is actionable and distinct from invalid', async () => {
  const { toast } = await import('@/lib/error-handler');
  vi.mocked(toast.error).mockClear();
  ipc.validateRegex.mockResolvedValueOnce([null]);
  const request = args({ conds: [{ t: 'domainRegex', v: 'example' }] });
  await submitRule(request.a);
  expect(request.staged).toHaveLength(0);
  expect(toast.error).toHaveBeenCalledWith('rules.syntaxUnavailable');
});
it('unchanged legacy expression does not require a checker for metadata editing', async () => {
  const saved = { ...base, type: 'domainRegex' as const, values: [String.raw`\p{ASCII}`] };
  useAppStore.setState({ rules: [saved] });
  ipc.validateRegex.mockClear();
  const request = args({ isEdit: true, base: saved, name: 'new label', conds: [{ t: 'domainRegex', v: saved.values[0] }] });
  await submitRule(request.a);
  expect(ipc.validateRegex).not.toHaveBeenCalled();
  expect((request.staged[0].nextValue as Rule).values).toEqual(saved.values);
});

it('preserves an unchanged 1025-byte legacy expression when renaming', async () => {
  const saved = { ...base, type: 'domainRegex' as const, values: ['a'.repeat(1025)] };
  useAppStore.setState({ rules: [saved] });
  ipc.validateRegex.mockClear();
  const request = args({ isEdit: true, base: saved, name: 'new label', conds: [{ t: 'domainRegex', v: saved.values[0] }] });
  await submitRule(request.a);
  expect(ipc.validateRegex).not.toHaveBeenCalled();
  expect(request.staged).toHaveLength(1);
  expect((request.staged[0].nextValue as Rule).values).toEqual(saved.values);
});
it.each([false, true])('rejects a new oversized expression before checking (edit=%s)', async isEdit => {
  ipc.validateRegex.mockClear();
  const request = args({ isEdit, base: isEdit ? base : undefined, conds: [{ t: 'domainRegex', v: 'a'.repeat(1025) }] });
  await submitRule(request.a);
  expect(ipc.validateRegex).not.toHaveBeenCalled();
  expect(request.staged).toHaveLength(0);
});
