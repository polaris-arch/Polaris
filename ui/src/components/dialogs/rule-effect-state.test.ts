/**
 * `rule-effect-state.ts` 里那两个**纯**判据的单测。
 *
 * 两个函数都是 2026-09-13（批 10）从桌面 `.tsx` 里抽出来的 —— 抽出来之前它们藏在
 * `useState` 初始化器与一处 `onChange` 里，**任何测试都够不到**：
 * 桌面那两个组件要 jsdom + Csel + Modal 才渲染得起来，于是这两条判据从来没被单独验证过。
 * 抽出来是为了给移动端复用，而「能被测到」是同一次抽取的副产品 —— 本文件就是那份收据。
 *
 * 两条判据各自的失败后果都不是显示不对，是**存下去的数据不对**：
 *  · `ruleRouteTargetChoice` 是下拉与 `rule-submit.ts` 之间的**值编码协议**，两端读错一档，
 *    一条「代理到香港 01」的规则会被存成「默认代理」；
 *  · `dnsEffectLinkage` 少一步，会把「动作说返回 FakeIP、答案模式说给真实 IP」这种
 *    内部矛盾的效果交给引擎。
 */
import { describe, expect, it } from 'vitest';
import type { RuleDnsEffect, RuleRouteEffect } from '@/contracts/types';
import { dnsEffectLinkage, ruleRouteTargetChoice, useRuleDnsEffect } from './rule-effect-state';

describe('ruleRouteTargetChoice：存量 route 效果 → 目标出站的值编码', () => {
  it('没有 effect（新建）落在 `proxy`', () => {
    expect(ruleRouteTargetChoice(null)).toBe('proxy');
  });

  it('direct / block 逐字同名', () => {
    expect(ruleRouteTargetChoice({ action: 'direct' } as RuleRouteEffect)).toBe('direct');
    expect(ruleRouteTargetChoice({ action: 'block' } as RuleRouteEffect)).toBe('block');
  });

  it('proxy + 指定节点 → `node:<id>`（前缀与 id 都要在）', () => {
    expect(
      ruleRouteTargetChoice({ action: 'proxy', targetServerId: 'srv-hk-01' } as RuleRouteEffect),
    ).toBe('node:srv-hk-01');
  });

  /**
   * 🔴 `proxy` 有**两个**来源，它们落在同一档上：新建（没有 effect）与「代理但没指定节点」。
   * 这一条钉住后者 —— 桌面快选「默认代理」存下来的就是这个形态，读成 `node:undefined`
   * 会让下拉一个选项都勾不中，用户看到的是「我选的那个节点没了」。
   */
  it('proxy 但**没有**目标节点 → 仍是 `proxy`，不是 `node:undefined`', () => {
    expect(ruleRouteTargetChoice({ action: 'proxy' } as RuleRouteEffect)).toBe('proxy');
    expect(ruleRouteTargetChoice({ action: 'proxy', targetServerId: '' } as RuleRouteEffect)).toBe(
      'proxy',
    );
  });

  it('反向对照：四档真的互不相同（否则上面几条可以同时被一个常量满足）', () => {
    const all = [
      ruleRouteTargetChoice(null),
      ruleRouteTargetChoice({ action: 'direct' } as RuleRouteEffect),
      ruleRouteTargetChoice({ action: 'block' } as RuleRouteEffect),
      ruleRouteTargetChoice({ action: 'proxy', targetServerId: 'n1' } as RuleRouteEffect),
    ];
    expect(new Set(all).size).toBe(4);
  });
});

describe('dnsEffectLinkage：选了哪个动作 → answerMode / resolver 跟着变', () => {
  it('`fakeIp` ⇒ answerMode 也是 fakeIp', () => {
    expect(dnsEffectLinkage('fakeIp')).toEqual({ answerMode: 'fakeIp', resolver: 'direct' });
  });

  it('内置远程服务器 ⇒ resolver 走代理', () => {
    expect(dnsEffectLinkage('server:builtin-remote')).toEqual({
      answerMode: 'real',
      resolver: 'proxy',
    });
  });

  it('其余动作 ⇒ 真实 IP + 直连解析', () => {
    for (const choice of [
      'server:builtin-domestic',
      'group:g1',
      'hosts:h1',
      'reject',
      'predefined',
      'followRouteDefault',
    ]) {
      expect(dnsEffectLinkage(choice), `「${choice}」的联动不对`).toEqual({
        answerMode: 'real',
        resolver: 'direct',
      });
    }
  });

  /**
   * 反向对照：两格**都**跟着输入变，不是其中一格恒定。
   * 只验一格时，「resolver 永远是 direct」这种退化会整条溜过去（选了远程 DoH 却走直连解析
   * = 那台机器上的 DNS 查询根本没进隧道）。
   */
  it('反向对照：两格都随输入变，没有一格是恒定的', () => {
    const modes = new Set(
      ['fakeIp', 'server:builtin-remote', 'reject'].map((c) => dnsEffectLinkage(c).answerMode),
    );
    const resolvers = new Set(
      ['fakeIp', 'server:builtin-remote', 'reject'].map((c) => dnsEffectLinkage(c).resolver),
    );
    expect(modes.size, 'answerMode 恒定 ⇒ 它没在跟着动作走').toBeGreaterThan(1);
    expect(resolvers.size, 'resolver 恒定 ⇒ 它没在跟着动作走').toBeGreaterThan(1);
  });
});

describe('自检：两个符号与 hook 都真的从这份纯 `.ts` 导出（搬家没搬空）', () => {
  it('三个导出都在，且不是 undefined', () => {
    expect(typeof ruleRouteTargetChoice).toBe('function');
    expect(typeof dnsEffectLinkage).toBe('function');
    /* hook 本体不在这里跑（它要 React 渲染环境），只证明它确实住在这份文件里 ——
       桌面那两个 `.tsx` 现在是从这里再导出的，搬空了那边会静默变成 undefined。 */
    expect(typeof useRuleDnsEffect).toBe('function');
  });

  it('存量 DNS 效果的类型形状没被这次搬家改掉（编译期契约的运行期收据）', () => {
    const effect: RuleDnsEffect = { resolver: 'inherit', answerMode: 'real' };
    expect(effect.resolver).toBe('inherit');
  });
});
