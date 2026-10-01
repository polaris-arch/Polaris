/**
 * endpoint 前置代理（detour）—— 纯逻辑测试 ＋ **两道源码结构门**。
 *
 * # ⚠️ 门 A / 门 B 是**结构门**（如实标注，别当它们是行为门）
 *
 * 本仓 vitest 是 `environment: 'node'`（无 jsdom），三个弹窗**渲染不了** ⇒ 「表单里真有这个控件、
 * 拨了它值真能存下来」这件事没法在这一层直测。门 A 判据是「源码里存在一条 `k: 'detour'` 的
 * FieldSpec 表项、且带 hint 键」，门 B 判据是「五份 locale JSON 里那几个键存在且非空」。
 * 它们抓的是**回归**（有人把控件删了 / 把 hint 摘了 / 加了键但少补一门语言），
 * 不是「控件真的接对了」。真接线由下面的纯函数测试（`endpointDetourOptions` / `applyDetour` /
 * `draftFromServer` ⇄ `buildWgServer` 往返）与 Rust 侧
 * `builder/generate.rs` 的三条产物门（detour 落进 JSON / endpoint 目标被排除 / 悬空被剪）覆盖。
 *
 * # 抓不到什么
 *
 *  - 控件**摆在哪个区**（主区/高级折叠）、顺序、样式 —— 结构门不看这些。
 *  - 译文**质量**：只验「键在、非空、含 UDP/TCP 那个判别词」，不验句子对不对。
 *  - **真核行为**：前置代理不支持 UDP 时 WG 到底表现成什么样，本层测不到（语义已在
 *    2026-07-31 的本机 loopback A/B 里实测，结论记在 `crates/config-engine/src/singbox/endpoint.rs`）。
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { ServerConfig } from '@/contracts/types';
import {
  DETOUR_NONE,
  applyDetour,
  detourDraftValue,
  endpointDetourOptions,
} from './detour-options';
import { draftFromServer, buildWgServer, emptyWgDraft } from './wg-logic';
import { initTsDraft } from './ts-settings-logic';

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
}
function locale(name: string): Record<string, Record<string, string>> {
  return JSON.parse(read(`../../i18n/locales/${name}.json`)) as Record<
    string,
    Record<string, string>
  >;
}

const srv = (id: string, extra: Partial<ServerConfig> = {}): ServerConfig =>
  ({ id, name: id.toUpperCase(), protocol: 'vless', address: 'a', port: 443, ...extra }) as ServerConfig;

describe('endpointDetourOptions —— 候选集', () => {
  it('首项恒是「不串联」哨兵，其后是可作 detour 目标的节点', () => {
    const opts = endpointDetourOptions([srv('a'), srv('b')], undefined, '直连');
    expect(opts[0]).toEqual([DETOUR_NONE, '直连']);
    expect(opts.map(([v]) => v)).toEqual([DETOUR_NONE, 'a', 'b']);
  });

  it('排除自身（自指是环，生成侧的环检测会整条丢掉）', () => {
    const opts = endpointDetourOptions([srv('a'), srv('b')], 'a', '直连');
    expect(opts.map(([v]) => v)).toEqual([DETOUR_NONE, 'b']);
  });

  it('排除 endpoint 类协议 —— 判据与 Rust `is_mesh_protocol` 同一口径（wireguard / tailscale）', () => {
    const servers = [
      srv('proxy'),
      srv('wg', { protocol: 'wireguard' }),
      srv('ts', { protocol: 'tailscale' }),
    ];
    const opts = endpointDetourOptions(servers, undefined, '直连');
    expect(opts.map(([v]) => v)).toEqual([DETOUR_NONE, 'proxy']);
  });
});

describe('applyDetour / detourDraftValue —— 提交与回显成对', () => {
  it('哨兵 ⇒ 删键（不写 `direct` 字面量：盘上不留假的 id 引用）', () => {
    const s = srv('a', { detour: 'old' });
    applyDetour(s, DETOUR_NONE);
    expect('detour' in s).toBe(false);
  });

  it('空值 / 非字符串 ⇒ 同样删键', () => {
    for (const v of ['', '   ', undefined, 42]) {
      const s = srv('a', { detour: 'old' });
      applyDetour(s, v);
      expect('detour' in s, `值 ${String(v)} 应删键`).toBe(false);
    }
  });

  it('选了节点 ⇒ 写进 detour；回显取得回同一个值', () => {
    const s = srv('a');
    applyDetour(s, 'front');
    expect(s.detour).toBe('front');
    expect(detourDraftValue(s)).toBe('front');
  });

  it('缺席回显 = 哨兵（缺省即默认）', () => {
    expect(detourDraftValue(srv('a'))).toBe(DETOUR_NONE);
    expect(detourDraftValue(undefined)).toBe(DETOUR_NONE);
  });
});

describe('WG / TS 草稿 ⇄ ServerConfig 往返带上 detour', () => {
  const wgNode = srv('w1', {
    protocol: 'wireguard',
    detour: 'front',
    wireguardSettings: {
      privateKey: 'priv',
      peerPublicKey: 'pub',
      localAddress: ['10.0.0.2/32'],
    },
  });

  it('draftFromServer 取顶层 detour（不是 wireguardSettings 里的）', () => {
    expect(draftFromServer(wgNode).detour).toBe('front');
  });

  it('buildWgServer 由草稿定夺 —— 改成哨兵能真正清掉存量值', () => {
    const kept = buildWgServer('WG', draftFromServer(wgNode), wgNode);
    expect(kept.detour).toBe('front');
    const cleared = buildWgServer(
      'WG',
      { ...draftFromServer(wgNode), detour: DETOUR_NONE },
      wgNode
    );
    expect('detour' in cleared).toBe(false);
  });

  it('新增态默认不串联', () => {
    expect(emptyWgDraft().detour).toBe(DETOUR_NONE);
  });

  it('initTsDraft 同样取顶层 detour', () => {
    expect(initTsDraft(srv('t1', { protocol: 'tailscale', detour: 'front' })).detour).toBe('front');
    expect(initTsDraft(srv('t1', { protocol: 'tailscale' })).detour).toBe(DETOUR_NONE);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// 门 A（结构门）：三个 endpoint 弹窗都得有这个控件，且带 hint 键 + 走共用候选函数
// ════════════════════════════════════════════════════════════════════════════

/*
 * 🔴 **取材面 2026-09-06 随字段表搬家一起挪**：三张 FieldSpec 表从弹窗 `.tsx` 里拆进了零 React 的
 * `.ts`（`wg-spec` / `ts-spec` / `warp-spec`，理由见各自头注：移动端按契约 A1 不许 import 桌面 `.tsx`，
 * 两端要共用同一张表）。本门要断言的是**表项**，故读表所在的那个文件；读回弹窗会恒红，
 * 而那是判据量错了地方，不是控件没了。
 *
 * 两端因此共用同一条判据：这三个 detour 表项一旦缺失，桌面与移动端**同时**失去那个控件。
 */
const FORMS: ReadonlyArray<{
  file: string;
  src: string;
  ns: string;
  /** 消费这张表的**两个客户端**（候选构造与提交侧写回都住在它们里面）。 */
  consumers: ReadonlyArray<readonly [string, string]>;
}> = [
  {
    file: 'wg-spec.ts',
    src: read('./wg-spec.ts'),
    ns: 'wg',
    consumers: [
      ['WgDialog.tsx', read('./WgDialog.tsx')],
      ['mobile/forms/WgPanel.tsx', read('../../mobile/forms/WgPanel.tsx')],
    ],
  },
  {
    file: 'ts-spec.ts',
    src: read('./ts-spec.ts'),
    ns: 'ts',
    consumers: [
      ['TsSettingsDialog.tsx', read('./TsSettingsDialog.tsx')],
      ['mobile/forms/TsSettingsPanel.tsx', read('../../mobile/forms/TsSettingsPanel.tsx')],
    ],
  },
  {
    file: 'warp-spec.ts',
    src: read('./warp-spec.ts'),
    ns: 'warp',
    consumers: [
      ['WarpDialog.tsx', read('./WarpDialog.tsx')],
      ['mobile/forms/WarpPanel.tsx', read('../../mobile/forms/WarpPanel.tsx')],
    ],
  },
];

describe('门 A（源码结构门）：三个 endpoint 表单的 detour 控件', () => {
  it('自检：三份源码都读得到（读不到必须转红，不得 0 断言空转）', () => {
    for (const f of FORMS) {
      expect(f.src.length, `${f.file} 源码读取失败`).toBeGreaterThan(800);
    }
  });

  for (const f of FORMS) {
    it(`${f.file}：有一条 k: 'detour' 的 select 表项，label/hint 指向 ${f.ns}.detour*`, () => {
      // 取那一行整体判断，避免「文件里别处提过 detour」把门盖绿。
      const line = f.src
        .split('\n')
        .find((l) => /\{\s*t:\s*'select',\s*k:\s*'detour'/.test(l));
      expect(line, `${f.file} 缺少 detour 的 select 表项`).toBeTruthy();
      expect(line).toContain(`label: '${f.ns}.detour'`);
      expect(line).toContain(`hint: '${f.ns}.detourHint'`);
      expect(line).toContain('options: detourOpts');
    });

    /* 候选构造与提交侧写回住在**消费者**里（表本身只收一个 `detourOpts` 形参）。
       六个消费者逐个判：桌面漏一处或移动端漏一处，都是那一端的用户选了个不生效的前置代理。 */
    it(`${f.file} 的两个客户端：候选集都走共用的 endpointDetourOptions（排除判据一处定义）`, () => {
      for (const [name, src] of f.consumers) {
        expect(src.length, `${name} 源码读取失败`).toBeGreaterThan(1000);
        expect(src, `${name} 没走共用的候选构造`).toContain('endpointDetourOptions(');
        // 各自手拼一份 `servers.filter(...)` 就是这道门要防的复发形态。
        expect(src, `${name} 自己拼了一份候选`).not.toMatch(/detourOpts[^\n]*=\s*\[/);
      }
    });
  }

  it('提交侧统一走 applyDetour（哨兵 ⇒ 删键，六个消费者不各写一份）', () => {
    for (const f of FORMS) {
      for (const [name, src] of f.consumers) {
        const wired =
          src.includes('applyDetour(') ||
          // WG 那两个的写回下沉在 `wg-logic.ts#buildWgServer` 里（同一函数）。
          (name.endsWith('WgDialog.tsx') || name.endsWith('WgPanel.tsx')) &&
            read('./wg-logic.ts').includes('applyDetour(');
        expect(wired, `${name} 的 detour 未接到 applyDetour`).toBe(true);
      }
    }
  });
});

// ════════════════════════════════════════════════════════════════════════════
// 门 B：提示文案五语齐，且 WG/WARP 那两句必须说到 UDP、TS 那句必须说到 TCP
// ════════════════════════════════════════════════════════════════════════════

const LOCALES = ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa'] as const;

describe('门 B：detour 提示文案五语齐 + UDP/TCP 判别词', () => {
  it('自检：五份 locale 都读得到且非空', () => {
    for (const l of LOCALES) {
      expect(Object.keys(locale(l)).length, `${l} 读取失败`).toBeGreaterThan(10);
    }
  });

  for (const l of LOCALES) {
    it(`${l}：wg / ts / warp 的 detour + detourHint 都在且非空`, () => {
      const d = locale(l);
      for (const ns of ['wg', 'ts', 'warp'] as const) {
        expect(d[ns]?.detour?.trim(), `${l} ${ns}.detour 缺失`).toBeTruthy();
        expect(d[ns]?.detourHint?.trim(), `${l} ${ns}.detourHint 缺失`).toBeTruthy();
      }
    });

    it(`${l}：WG/WARP 提示必须写明前置代理要支持 UDP 转发`, () => {
      // 判据取 `UDP ASSOCIATE` 这个**跨语言不翻译**的协议术语 —— 五种语言的译文里都是这串拉丁字母，
      // 于是这条断言与语种无关，却仍然咬得住「有人把 UDP 那句删了/改成泛泛的一句话」。
      const d = locale(l);
      expect(d.wg.detourHint, `${l} wg.detourHint 未提 UDP ASSOCIATE`).toContain('UDP ASSOCIATE');
      expect(d.warp.detourHint, `${l} warp.detourHint 未提 UDP ASSOCIATE`).toContain(
        'UDP ASSOCIATE'
      );
    });

    it(`${l}：TS 提示是 TCP 那条（与 WG/WARP 刻意不同，抄错会让用户白换代理）`, () => {
      const d = locale(l);
      expect(d.ts.detourHint, `${l} ts.detourHint 未提 TCP`).toContain('TCP');
      expect(
        d.ts.detourHint.includes('UDP ASSOCIATE'),
        `${l} ts.detourHint 不得照抄 WG 的 UDP 硬约束`
      ).toBe(false);
    });
  }
});

describe('前置代理候选：endpoint 腿的 VPN 客户端同样排除', () => {
  it('openconnect / openvpn-client 不在候选里', () => {
    // 它们的 tag 落 `endpoints[]`，不在 `outbounds[]` ⇒ 指向它们的 detour 是悬空引用，
    // 生成侧会把**引用方整个节点**剪掉并上报 invalid（不是「多一个没用的选项」那么轻）。
    // 判据从只认 WG/TS 的 `isMeshProtocol` 换成 `landsInEndpoints` 才覆盖到这两个。
    const servers = [
      { id: 'oc', name: 'OC', protocol: 'openconnect' },
      { id: 'ov', name: 'OV', protocol: 'openvpn-client' },
      { id: 'mq', name: 'MQ', protocol: 'masque-client' },
      { id: 'v', name: 'V', protocol: 'vless' },
    ] as ServerConfig[];
    const ids = endpointDetourOptions(servers, undefined, '不串联').map(([v]) => v);
    expect(ids).not.toContain('oc');
    expect(ids).not.toContain('ov');
    expect(ids).not.toContain('mq');
    expect(ids).toContain('v');
  });
});

// ════════════════════════════════════════════════════════════════════════════
/**
 * 门 C：**凡是写回 `detour` 的表单，提交侧一律走 `applyDetour`** —— 清空必须真的清得掉。
 *
 * 2026-09-06 复审抓到的上游缺陷（两处，两端同批修）：`NodeDialog` / 移动端 `NodeFormPanel` 的
 * 提交侧手写成 `if (detour) meta.detour = detour;` —— 选了「不串联」时**不写键**，而
 * `codecBase = { ...base, ...meta }` 让存量 `base.detour` 从 spread 里活下来，
 * `protoCodec[proto].toConfig` 全程不碰 detour（`proto-codec.ts` 里除一条注释外零 `detour`）
 * ⇒ 写回的 `full.detour` 仍是旧值：界面显示「不串联」而流量照走前置代理，且用户没有任何办法清掉它。
 * 紧邻的 `bindInterface` 有显式 `delete full.bindInterface`，detour 漏了这一手。
 *
 * 门 A 的 `FORMS` 只覆盖三个 endpoint 弹窗（它们的 detour 是 FieldSpec 表项），
 * 而这两处是**手写控件**，落在那张表之外 —— 「没有任何门」正是这条缺陷活到今天的原因。
 * 本门按「提交侧」而不是「有没有控件」取材，故两种控件形态都在射程内。
 */
const DETOUR_SUBMITTERS: ReadonlyArray<{ file: string; src: string; via?: string }> = [
  { file: 'NodeDialog.tsx', src: read('./NodeDialog.tsx') },
  { file: 'TsSettingsDialog.tsx', src: read('./TsSettingsDialog.tsx') },
  { file: 'WarpDialog.tsx', src: read('./WarpDialog.tsx') },
  // WgDialog 的写回下沉在 `wg-logic.ts#buildWgServer`（同一函数）。
  { file: 'WgDialog.tsx', src: read('./WgDialog.tsx'), via: read('./wg-logic.ts') },
  { file: 'mobile/forms/NodeFormPanel.tsx', src: read('../../mobile/forms/NodeFormPanel.tsx') },
  /* 批 3 的三张组网表（移动端）。WG 那条的写回同样下沉在 `wg-logic.ts#buildWgServer`。 */
  { file: 'mobile/forms/WarpPanel.tsx', src: read('../../mobile/forms/WarpPanel.tsx') },
  { file: 'mobile/forms/TsSettingsPanel.tsx', src: read('../../mobile/forms/TsSettingsPanel.tsx') },
  {
    file: 'mobile/forms/WgPanel.tsx',
    src: read('../../mobile/forms/WgPanel.tsx'),
    via: read('./wg-logic.ts'),
  },
];

describe('门 C：写 detour 的表单，提交侧全部走 applyDetour（含手写控件的那两处）', () => {
  it('自检：八份源码都读得到且有量级（读不到必须转红，不得 0 断言空转）', () => {
    for (const f of DETOUR_SUBMITTERS) {
      expect(f.src.length, `${f.file} 源码读取失败`).toBeGreaterThan(1000);
    }
  });

  it('取材面自检：清册没有漏掉任何一处「持有 detour 草稿并提交」的表单', () => {
    /* 反过来数一遍：全仓（桌面 dialogs + 移动 forms）里凡是 `useState(base?.detour` /
       `detour` 草稿字段并调 `api.server.add|update` 的文件，都必须在清册里。
       这一条防的是「新加了第六个表单、它自己手写一份」——那正是本次缺陷的复发形态。 */
    const holders = ['NodeDialog.tsx', 'WgDialog.tsx', 'WarpDialog.tsx', 'TsSettingsDialog.tsx'];
    for (const h of holders) {
      expect(
        DETOUR_SUBMITTERS.some((f) => f.file === h),
        `${h} 持有 detour 却不在清册里`,
      ).toBe(true);
    }
    expect(read('../../mobile/forms/NodeFormPanel.tsx')).toContain('base?.detour');
  });

  for (const f of DETOUR_SUBMITTERS) {
    it(`${f.file}：提交侧调了 applyDetour（哨兵/空值 ⇒ 删键）`, () => {
      const wired = f.src.includes('applyDetour(') || (f.via ?? '').includes('applyDetour(');
      expect(
        wired,
        `${f.file} 的 detour 没接到 applyDetour —— 选「不串联」清不掉存量值：` +
          '界面写着不串联，流量照走前置代理',
      ).toBe(true);
    });
  }

  it('🔴 行为腿：`{...base, ...meta}` 那条起底路径上，清空真的删得掉键（不是留下旧值）', () => {
    /* 逐字复刻两处提交侧的形状：base 带存量 detour，meta 是「选了不串联所以不写这个键」的覆盖层。
       没有 `applyDetour` 这一手时，spread 会让 `base.detour` 活下来 —— 下面第一条断言就是它。 */
    const base = srv('n1', { detour: 'front' });
    const meta = { id: 'n1', name: 'N1', protocol: 'vless', address: 'a', port: 443 } as ServerConfig;
    const naive = { ...base, ...meta } as ServerConfig;
    expect(naive.detour, '对照组自己塌了：spread 之后旧值本就没了，那本门无从说话').toBe('front');
    // 生产的那一手：空值（= 移动端/桌面的「不串联」状态值）⇒ 删键。
    const fixed = applyDetour({ ...base, ...meta } as ServerConfig, '');
    expect('detour' in fixed, '选了「不串联」而磁盘上的串联关系没被清掉').toBe(false);
    // 哨兵字面量同样删键（两端的下拉值是 `direct`，state 里落成 `''`，两种都要成立）。
    expect('detour' in applyDetour({ ...base, ...meta } as ServerConfig, DETOUR_NONE)).toBe(false);
    // 正面对照：选了一个真节点时值要留住（否则「删得掉」可能只是「什么都不写」）。
    expect(applyDetour({ ...base, ...meta } as ServerConfig, 'other').detour).toBe('other');
  });
});
