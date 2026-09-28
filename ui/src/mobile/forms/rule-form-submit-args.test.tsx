/**
 * 规则表单**装袋那一跳**的门：交给 `submitRule` 的实参袋，逐格等于表单当下的取值。
 *
 * # 为什么必须单开这一条（本仓栽过的固定形态）
 *
 * `rule-submit.ts` 自己有判据、`rule-cond.ts` / `rule-effect-state.ts` 各有单测、
 * 接线完成度门数「还剩几条没接」—— 三类**全绿而这张表把用户选的档位丢掉**，是完全可能的：
 * 它们覆盖的是**函数体**，覆盖不到「把参数喂给它」那一跳。
 * `mesh-forms.test.tsx` 的头注逐字记着同一形态的三条 major（`open(target)` 换成
 * `open({ kind:'node' })`，三张表全废而全仓门一条不红）。本门补的就是这一格，判据面**就是那一跳**。
 *
 * 后果不是显示不对，是**存下去的数据不对**：`target` / `dnsAction` / `dnsFallbackAction`
 * 三格里任何一格被换成字面量，表单照样提交成功、照样关掉，而落盘的是另一条规则。
 *
 * # 怎么在 node 环境里拿到「一次真实提交」
 *
 * 本仓 vitest 是 `environment:'node'`（刻意不装 jsdom）⇒ 点不了按钮。但提交闭包是**渲染期**
 * 就交给外壳的一个 prop：把 `FormSheet` 换成一枚探针、抓住它收到的 `onSubmit`，再直接调它 ——
 * 走的是面板里那个真的 `submit`，不是复制品。
 *
 * 🔴 **只换两样，且两样都不是被验对象**：
 *  · `FormSheet` —— 外壳（它渲染什么本门一个字都不判，另有 `mesh-forms` 与各屏门在看）；
 *  · `submitRule` —— 被验的是「**交给它什么**」，不是它拿到之后做什么（那边自有判据）。
 * 面板本体、`useRuleDnsEffect` 的初值反解、`ruleRouteTargetChoice` 的值编码、
 * 条件草稿的读回，全是真身。为防「探针抓了个 undefined 而断言空跑」，⓪ 组先证明这两样探针是活的。
 */
import { beforeAll, beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ReactElement } from 'react';
import type { Rule, UserConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useMobileFormStore } from './form-store';

/** 探针的落点。`vi.hoisted` —— `vi.mock` 的工厂是提升的，引用普通模块变量会拿到 TDZ。 */
const probe = vi.hoisted(() => ({
  onSubmit: null as null | (() => void),
  args: null as Record<string, unknown> | null,
  calls: 0,
}));

vi.mock('./FormSheet', () => ({
  FormSheet: (props: { onSubmit?: () => void }): null => {
    probe.onSubmit = props.onSubmit ?? null;
    return null;
  },
}));

vi.mock('@/components/dialogs/rule-submit', () => ({
  submitRule: (args: Record<string, unknown>): Promise<void> => {
    probe.args = args;
    probe.calls += 1;
    return Promise.resolve();
  },
}));

const { RuleFormPanel } = await import('./RuleFormPanel');

beforeAll(async () => {
  await i18nReady;
});

/** SSR 渲染读的是 `getInitialState()` 那个对象，清完要同步镜像（同 `mesh-forms.test.tsx`）。 */
function mirror(): void {
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
}

function reset(): void {
  probe.onSubmit = null;
  probe.args = null;
  probe.calls = 0;
  useMobileFormStore.getState().closeAll();
  useStagedConfigStore.setState({ entries: [] });
  useAppStore.setState({ rules: [], dnsRules: [], servers: [], config: null });
  mirror();
}

beforeEach(reset);
afterEach(reset);

const NODE = { id: 'srv-hk-01', name: 'HK-01', protocol: 'vless', address: '1.1.1.1', port: 443 };

const CONFIG = {
  dnsServers: [
    { id: 'dns-x', name: 'X DoH', type: 'https', enabled: true, outbound: { type: 'direct' }, endpoint: { host: '9.9.9.9' } },
    { id: 'dns-hosts', name: 'Hosts', type: 'hosts', enabled: true, outbound: { type: 'direct' } },
  ],
  dnsServerGroups: [{ id: 'grp-1', name: 'G1', enabled: true, mode: 'race', members: ['dns-x'] }],
  servers: [NODE],
} as unknown as UserConfig;

/** 一条存量规则。表单读它当编辑基准 ⇒ 实参袋里的每一格都该等于从这里反解出来的值。 */
const ruleOf = (over: Partial<Rule>): Rule => ({
  id: 'r1',
  type: 'domain',
  values: ['a.example', 'b.example'],
  action: 'direct',
  enabled: true,
  remarks: '存量规则',
  ...over,
});

/** 把规则与配置喂进 store，渲染一次表单，返回捕获到的提交闭包。 */
function mountAndSubmit(
  element: ReactElement,
  state: { rules?: Rule[]; dnsRules?: Rule[] },
): Record<string, unknown> {
  useAppStore.setState({
    rules: state.rules ?? [],
    dnsRules: state.dnsRules ?? [],
    servers: [NODE] as never,
    config: CONFIG,
  });
  mirror();
  renderToStaticMarkup(element);
  expect(probe.onSubmit, '外壳没拿到提交闭包 —— 探针抓空了，下面每条都会在 null 上断言').not.toBeNull();
  probe.onSubmit?.();
  expect(probe.calls, '提交闭包跑了，但没有调用 `submitRule`').toBe(1);
  expect(probe.args, '实参袋是空的').not.toBeNull();
  return probe.args as Record<string, unknown>;
}

describe('⓪ 自检：两枚探针都是活的（否则下面每条都是空跑）', () => {
  it('i18n 真的加载了（回退成键会让文案类断言变成「键 == 键」）', () => {
    expect(i18n.t('rules.newTitle')).not.toBe('rules.newTitle');
  });

  it('外壳探针抓得到 `onSubmit`，且它真的通到 `submitRule`', () => {
    const args = mountAndSubmit(<RuleFormPanel instanceId="i0" initialPlane="route" />, {});
    expect(typeof args).toBe('object');
    /* 新建态也要有个可提交的袋子：`isEdit` 假、`base` 缺席，其余格照样齐。 */
    expect(args.isEdit).toBe(false);
    expect(args.base).toBeUndefined();
  });
});

describe('① 装袋：实参袋逐格等于表单当下的取值', () => {
  it('🔴 `target` —— 存量规则指着某个节点时，袋里必须是 `node:<id>`，不是写死的档位', () => {
    const rule = ruleOf({
      action: 'proxy',
      targetServerId: 'srv-hk-01',
      effects: { route: { action: 'proxy', targetServerId: 'srv-hk-01' } },
    });
    const args = mountAndSubmit(
      <RuleFormPanel instanceId="i1" ruleId="r1" initialPlane="route" />,
      { rules: [rule] },
    );
    expect(args.target, '目标出站丢了 —— 落库的会是「默认代理」而不是那个节点').toBe(
      'node:srv-hk-01',
    );
    expect(args.initialPlane).toBe('route');
    expect(args.isEdit).toBe(true);
  });

  it('🔴 `dnsAction` —— 指着某个 DNS 服务器时，袋里必须是 `server:<id>`', () => {
    const rule = ruleOf({
      effects: {
        dns: {
          enabled: true,
          action: { type: 'server', serverId: 'dns-x' },
          resolver: 'direct',
          answerMode: 'real',
        },
      },
    });
    const args = mountAndSubmit(
      <RuleFormPanel instanceId="i2" ruleId="r1" initialPlane="dns" />,
      { dnsRules: [rule] },
    );
    expect(args.dnsAction, 'DNS 动作丢了 —— 这条规则会被存成解析到另一个服务器').toBe(
      'server:dns-x',
    );
    expect(args.initialPlane).toBe('dns');
  });

  it('🔴 `dnsFallbackAction` —— hostsFirst 的兜底那一格单独反解，不许跟主动作串味', () => {
    const rule = ruleOf({
      effects: {
        dns: {
          enabled: true,
          action: {
            type: 'hostsFirst',
            hostsServerId: 'dns-hosts',
            fallback: { type: 'group', groupId: 'grp-1' },
          },
          resolver: 'direct',
          answerMode: 'real',
        },
      },
    });
    const args = mountAndSubmit(
      <RuleFormPanel instanceId="i3" ruleId="r1" initialPlane="dns" />,
      { dnsRules: [rule] },
    );
    expect(args.dnsAction).toBe('hosts:dns-hosts');
    expect(args.dnsFallbackAction, 'hosts 的兜底丢了 —— 没命中 hosts 的域名会去到别处').toBe(
      'group:grp-1',
    );
  });

  it('条件草稿与名字也逐格透传（袋里其余格不是摆设）', () => {
    const rule = ruleOf({
      remarks: '看这条',
      conditions: [
        { type: 'domain', values: ['a.example', 'b.example'] },
        { type: 'ipCidr', values: ['10.0.0.0/8'] },
      ],
      combineMode: 'and',
      effects: { route: { action: 'block' } },
    });
    const args = mountAndSubmit(
      <RuleFormPanel instanceId="i4" ruleId="r1" initialPlane="route" />,
      { rules: [rule] },
    );
    expect(args.name).toBe('看这条');
    expect(args.logic, '与/或丢了 —— 两条件规则的命中面会从「都满足」变成「满足任一」').toBe('and');
    expect(args.conds).toEqual([
      { t: 'domain', v: 'a.example, b.example' },
      { t: 'ipCidr', v: '10.0.0.0/8' },
    ]);
    expect(args.target).toBe('block');
  });

  it('🔴 `networkProfileId` —— 存量规则挂着场景时原样透传（传 \'\' 会在编辑时把场景悄悄清掉）', () => {
    const rule = ruleOf({ networkProfileId: 'np-office', effects: { route: { action: 'direct' } } });
    const args = mountAndSubmit(
      <RuleFormPanel instanceId="i7" ruleId="r1" initialPlane="route" />,
      { rules: [rule] },
    );
    expect(args.networkProfileId, '生效网络丢了 —— 这条规则会变成在任何网络都生效').toBe('np-office');
    /* 新建带场景的规则插到最前（spec §3.4-4）：`planeOrder` 取本平面 effective 规则现序 + 持久化顺序。 */
    expect(args.planeOrder).toEqual({ ruleIds: ['r1'], persistedOrder: [] });
  });

  it('新建态「任何网络」= \'\'（不写键），反向对照：不是画死的', () => {
    const fresh = mountAndSubmit(<RuleFormPanel instanceId="i8" initialPlane="route" />, {});
    expect(fresh.networkProfileId).toBe('');
    reset();
    const other = mountAndSubmit(
      <RuleFormPanel instanceId="i9" ruleId="r1" initialPlane="route" />,
      { rules: [ruleOf({ networkProfileId: 'np-home', effects: { route: { action: 'direct' } } })] },
    );
    expect(other.networkProfileId).toBe('np-home');
  });

  /**
   * 反向对照：三格**互不相同**，且都随输入变。
   * 少了这一条，上面三条可以被「袋里每一格都恰好等于某个常量」同时满足。
   */
  it('反向对照：换一条存量规则，三格跟着换（不是画死的）', () => {
    const first = mountAndSubmit(
      <RuleFormPanel instanceId="i5" ruleId="r1" initialPlane="dns" />,
      {
        dnsRules: [
          ruleOf({
            effects: {
              dns: { enabled: true, action: { type: 'server', serverId: 'dns-x' }, resolver: 'direct', answerMode: 'real' },
            },
          }),
        ],
      },
    );
    reset();
    const second = mountAndSubmit(
      <RuleFormPanel instanceId="i6" ruleId="r1" initialPlane="dns" />,
      {
        dnsRules: [
          ruleOf({
            effects: {
              dns: { enabled: true, action: { type: 'fakeIp' }, resolver: 'inherit', answerMode: 'fakeIp' },
            },
          }),
        ],
      },
    );
    expect(first.dnsAction).toBe('server:dns-x');
    expect(second.dnsAction).toBe('fakeIp');
    expect(second.dnsAnswerMode).toBe('fakeIp');
  });

  /**
   * 袋子的**形状**也要钉住：`rule-submit.ts` 的 `RuleSubmitArgs` 有 22 个格，
   * 少喂一个的后果是 `undefined` 一路走到落盘（TS 在 `as` 或可选参数下拦不住）。
   * 这条只判「格都在」，各格的值由上面几条逐条判。
   */
  it('袋子的格一个不少（漏喂一格 = 那一格以 undefined 落盘）', () => {
    const args = mountAndSubmit(<RuleFormPanel instanceId="i7" initialPlane="dns" />, {});
    for (const key of [
      't',
      'conds',
      'name',
      'setErrName',
      'logic',
      'target',
      'dnsAction',
      'dnsFallbackAction',
      'dnsResolver',
      'dnsAnswerMode',
      'dnsPredefinedRcode',
      'dnsPredefinedAnswer',
      'dnsPredefinedNs',
      'dnsPredefinedExtra',
      'isEdit',
      'initialPlane',
      'stagingEnabled',
      'stage',
      'close',
      'loadConfig',
      'setSubmitting',
    ]) {
      expect(Object.keys(args), `实参袋缺一格：${key}`).toContain(key);
      expect(args[key], `实参袋的 ${key} 是 undefined`).not.toBeUndefined();
    }
  });
});
