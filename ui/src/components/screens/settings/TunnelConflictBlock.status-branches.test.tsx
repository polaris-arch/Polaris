/**
 * 「本机其它隧道」报告块的**四态分支门**。
 *
 * # 守什么（本批最重要的一条）
 *
 * 后端刻意只在 `probed` 那一支带 `conflicts` 键，其余三支缺键 —— 因为带一个空数组的话，渲染端
 * 最自然的 `(conflicts ?? []).length === 0` 会把「没探成」读成一句自信的「无冲突」。而
 * **macOS / Windows 根本没有探测实现**（只有 Linux 有），2026-09-08 报障那台正是 macOS：
 * 用户会看着一句「无冲突」把真病因排除掉。
 *
 * 于是本门逐态断言，两个方向都说话：
 *  - **否定**：`notProbed` / `unsupported` / `probeFailed` / `probed+有冲突` 四态里，界面文本
 *    **不含任何等价于「无冲突」的说法**（针眼见 `NO_CONFLICT_NEEDLES`）；
 *  - **正面**：同样四态里，各自那句「为什么没看成 / 看到了什么」必须真的出现 —— 只写否定断言的门
 *    会被「整个块根本没渲染」骗成绿的，也会被「四态塌成一句空话」骗成绿的；
 *  - **正向对照**：`probed` 且 `conflicts` 为空时，针眼**必须**出现。它证明针眼是活的：
 *    哪天文案改得不再含这几个词，这一条会先红，而不是让上面那批否定断言静默失去牙齿。
 *
 * # 为什么读真 locale 而不是桩 `t`
 *
 * 判据是「**界面文本**里没有等价于无冲突的说法」，不是「没用到某个 key」。桩 `t` 只能证明后者：
 * 把 `tunnelConflictUnsupported` 的译文改成「本平台无冲突」，桩 `t` 那版照样全绿。
 * 故 `t` 直接查 `locales/*.json`，中英两份都查（针眼分别是中文与英文）。
 *
 * # 变异对照（判据 6）
 *
 * 把组件里的 `status` 分支换成 `(report.conflicts ?? []).length === 0 ? 无冲突 : 列表`：
 * `unsupported` / `notProbed` 会落进「无冲突」⇒ 否定断言转红；
 * 换成 `report.conflicts?.length === 0 ? … : 列表`（不带 `??`）：三态落进「列表」分支 ⇒
 * 各自那句解释消失 ⇒ 正面断言转红。两种写法都红，正是这门要的。
 */
import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';

import type { TunnelConflictReport } from '@/contracts/tunnel-conflict-report';

type Dict = Record<string, unknown>;

const locale = (name: string): Dict =>
  JSON.parse(
    readFileSync(fileURLToPath(new URL(`../../../i18n/locales/${name}.json`, import.meta.url)), 'utf8'),
  ) as Dict;

const DICTS: Record<string, Dict> = { 'zh-CN': locale('zh-CN'), 'en-US': locale('en-US') };

/** 当前语种（`vi.mock` 工厂被提升，只能经 `vi.hoisted` 共享）。 */
const h = vi.hoisted(() => ({ lang: 'zh-CN' }));

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) => translate(h.lang, key, vars),
    i18n: { language: h.lang },
  }),
}));

/** 点分寻址 + `{{x}}` 插值。取不到就抛 —— 缺键静默回落成 key 会让下面的否定断言全部空转变绿。 */
function translate(lang: string, key: string, vars?: Record<string, unknown>): string {
  let node: unknown = DICTS[lang];
  for (const seg of key.split('.')) {
    node = (node as Dict | undefined)?.[seg];
  }
  if (typeof node !== 'string') throw new Error(`[tunnel-conflict] ${lang} 缺键 ${key}`);
  return node.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars?.[name] ?? ''));
}

const { TunnelConflictBlock } = await import('./TunnelConflictBlock');

/**
 * 「无冲突」的针眼 —— 中英各一。它们**逐字出现在** `tunnelConflictNone` 那句译文里，
 * 由本文件末尾的正向对照钉住；其余任何一态出现它们即是本门要抓的那种谎。
 */
const NO_CONFLICT_NEEDLES: Record<string, string> = { 'zh-CN': '无冲突', 'en-US': 'no conflict' };

const render = (report: TunnelConflictReport | null, lang: string): string => {
  h.lang = lang;
  return renderToStaticMarkup(<TunnelConflictBlock report={report} />);
};

const LANGS = ['zh-CN', 'en-US'] as const;

const NOT_PROBED: TunnelConflictReport = { status: 'notProbed' };
const UNSUPPORTED: TunnelConflictReport = { status: 'unsupported', platform: 'darwin' };
const PROBE_FAILED: TunnelConflictReport = { status: 'probeFailed', error: 'ip: command not found' };
const PROBED_CLEAN: TunnelConflictReport = {
  status: 'probed',
  foreignTunnels: [{ interface: 'utun4', prefix: '198.18.0.0/16' }],
  conflicts: [],
  criteria: { fakeipRanges: [], meshCidrs: [], tunAddresses: [] },
};
const PROBED_CONFLICT: TunnelConflictReport = {
  status: 'probed',
  foreignTunnels: [{ interface: 'utun4', prefix: '32.0.0.0/24' }],
  conflicts: [{ interface: 'utun4', prefix: '32.0.0.0/24', kind: 'meshOverlap' }],
  criteria: { fakeipRanges: [], meshCidrs: ['32.0.0.0/24'], tunAddresses: [] },
};

describe('判据 3 —— 未探测 ≠ 无冲突（逐态断言，中英双语）', () => {
  for (const lang of LANGS) {
    const needle = NO_CONFLICT_NEEDLES[lang];

    it(`[${lang}] 正向对照：探过了且真的没冲突时，针眼「${needle}」确实出现（否定断言有牙）`, () => {
      expect(render(PROBED_CLEAN, lang)).toContain(needle);
    });

    it(`[${lang}] notProbed：说清「本次没探」，且不含「${needle}」`, () => {
      const markup = render(NOT_PROBED, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictNotProbed'));
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] unsupported：点名平台 + 判定未进行，且不含「${needle}」`, () => {
      const markup = render(UNSUPPORTED, lang);
      // 平台名换成用户认得的产品名（darwin → macOS），不把 Node 约定名直接摆给用户。
      expect(markup).toContain(
        translate(lang, 'settings.tun.tunnelConflictUnsupported', { platform: 'macOS' }),
      );
      expect(markup).toContain('macOS');
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] probeFailed：说清失败 + 带后端诊断串，且不含「${needle}」`, () => {
      const markup = render(PROBE_FAILED, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictFailed'));
      expect(markup).toContain('ip: command not found');
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] probed 且有冲突：逐条列出，且不含「${needle}」`, () => {
      const markup = render(PROBED_CONFLICT, lang);
      expect(markup).toContain('utun4');
      expect(markup).toContain('32.0.0.0/24');
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictKindMesh'));
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] 报告还没拿到（null）：显示读不到，且不含「${needle}」`, () => {
      const markup = render(null, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictUnavailable'));
      expect(markup).not.toContain(needle);
    });
  }
});

describe('守卫自检：四态真的走的是四条不同的分支', () => {
  it('四态两两渲染结果互不相同（塌成一句空话时本条转红）', () => {
    const bodies = [NOT_PROBED, UNSUPPORTED, PROBE_FAILED, PROBED_CLEAN, PROBED_CONFLICT].map((r) =>
      render(r, 'zh-CN'),
    );
    expect(new Set(bodies).size).toBe(bodies.length);
  });

  it('针眼确实住在 `tunnelConflictNone` 那句译文里（针眼失效先红在这里）', () => {
    for (const lang of LANGS) {
      expect(translate(lang, 'settings.tun.tunnelConflictNone', { count: 1 })).toContain(
        NO_CONFLICT_NEEDLES[lang],
      );
    }
  });

  it('其余四句译文里一个针眼都没有（文案层面的同一条不变量，与渲染无关）', () => {
    for (const lang of LANGS) {
      for (const key of [
        'settings.tun.tunnelConflictHint',
        'settings.tun.tunnelConflictNotProbed',
        'settings.tun.tunnelConflictUnsupported',
        'settings.tun.tunnelConflictFailed',
        'settings.tun.tunnelConflictFound',
      ]) {
        expect(translate(lang, key, { platform: 'macOS', count: 1 })).not.toContain(
          NO_CONFLICT_NEEDLES[lang],
        );
      }
    }
  });
});
