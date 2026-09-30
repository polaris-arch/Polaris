/**
 * 「展开即露出」的两道门。
 *
 * # 判据取自「这个交互必须成立什么」，不是「当初改了哪几个文件」
 *
 * 用户反馈的是节点表单的某一处折叠，但同一交互还分布在共享 `Fold`、三处自带样式的
 * `<details>` 与非 details 的内联导入面板。若把门写成「NodeDialog 里那两处用了 Fold」，
 * 下一个人新加一个折叠仍然默认是坏的，而门照绿。故门写成**全仓扫描**：
 * 每一个 `<details>` 都必须挂上 `onToggle` —— 这是「展开时有人管」的唯一结构证据。
 *
 * # 为什么不把门写成「都必须用 `<Fold>`」
 *
 * `rule-test-det` / `tun-details` / `us-notes` 三处有各自的类名与内联样式，逼它们改形去换取
 * 这个行为，是用视觉回归的风险买一致性。行为（露出）与形态（markup）是两件事，门只锁前者。
 */
import { describe, it, expect } from 'vitest';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { computeRevealDelta } from './reveal';

// ---------------------------------------------------------------------------
// 1. 滚动量：纯几何，脱离 DOM 直接算
// ---------------------------------------------------------------------------

describe('computeRevealDelta', () => {
  const container = { top: 100, bottom: 500 };

  it('已经全看得见 → 不滚（无谓滚动比不滚更让人失去位置感）', () => {
    expect(computeRevealDelta({ top: 200, bottom: 400 }, container)).toBe(0);
  });

  it('底部恰好贴着容器下沿 → 不滚（边界不该触发）', () => {
    expect(computeRevealDelta({ top: 200, bottom: 500 }, container)).toBe(0);
  });

  it('溢出一点点 → 只滚溢出量', () => {
    expect(computeRevealDelta({ top: 200, bottom: 560 }, container)).toBe(60);
  });

  it('内容比容器还高 → 滚动量以 summary 顶部封顶，标题不会被顶出视口', () => {
    // 溢出 700，但顶部距容器顶只有 50 ⇒ 最多滚 50
    expect(computeRevealDelta({ top: 150, bottom: 1200 }, container)).toBe(50);
  });

  it('已经贴顶且仍然溢出 → 不滚（再滚就是把标题甩出去）', () => {
    expect(computeRevealDelta({ top: 100, bottom: 1200 }, container)).toBe(0);
  });

  it('元素整体在容器上方（异常输入）→ 不返回负数', () => {
    expect(computeRevealDelta({ top: -300, bottom: -100 }, container)).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// 2. 全仓扫描：没有一个折叠是「展开了没人管」的
// ---------------------------------------------------------------------------

const SRC = fileURLToPath(new URL('..', import.meta.url));

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      sourceFiles(full, out);
    } else if (name.endsWith('.tsx') && !name.includes('.test.')) {
      // 测试文件自己会在断言字符串里写 `<details class=…`，扫进来是假红。
      out.push(full);
    }
  }
  return out;
}

/**
 * 注释必须先剥掉：本门第一版扫到的唯一「违规」是 ListEditor 里一句**解释这道门的注释**
 * （「内联导入面板不是 <details>…」）—— 判据被自己写的散文污染，红得毫无信息量。
 */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

/**
 * 从 `<details` 截到它的 `<summary`：不能截到第一个 `>`，因为属性里的
 * `onToggle={(e) => …}` 自带 `>`，naive 截断会把 onToggle 切掉一半、判成没挂。
 */
function detailsTags(raw: string): string[] {
  const src = stripComments(raw);
  const tags: string[] = [];
  let i = src.indexOf('<details');
  while (i !== -1) {
    const end = src.indexOf('<summary', i);
    tags.push(src.slice(i, end === -1 ? i + 400 : end));
    i = src.indexOf('<details', i + 1);
  }
  return tags;
}

describe('展开即露出 —— 全仓不变量', () => {
  const files = sourceFiles(SRC);

  it('扫到的源文件数量合理（扫描器本身没瞎）', () => {
    expect(files.length).toBeGreaterThan(20);
  });

  it('每一个 <details> 都挂了 onToggle', () => {
    const offenders: string[] = [];
    for (const f of files) {
      for (const tag of detailsTags(readFileSync(f, 'utf8'))) {
        if (!tag.includes('onToggle')) offenders.push(`${f.slice(SRC.length)}: ${tag.trim().slice(0, 80)}`);
      }
    }
    expect(offenders, '这些 <details> 展开后不会把内容滚进视区，用户会以为点了没反应').toEqual([]);
  });

  it('确有 <details> 被扫到（防止上一条因为扫了个空集合而假绿）', () => {
    const total = files.reduce((n, f) => n + detailsTags(readFileSync(f, 'utf8')).length, 0);
    expect(total).toBeGreaterThan(0);
  });

  it('扫描器会剥注释 —— 注释里提到 <details> 不该被算成违规（本门第一版就栽在这）', () => {
    expect(detailsTags('// 说明：<details> 是折叠元素\nconst a = 1;')).toEqual([]);
    expect(detailsTags('/* <details> 举例 */')).toEqual([]);
    // 但真标签仍要被扫到，剥注释不能顺手把射程剥没了
    expect(detailsTags('<details onToggle={x}><summary>t</summary></details>')).toHaveLength(1);
  });

  it('Fold 的 onToggle 里真的调了 revealOnToggle（不只是同步 state）', () => {
    const src = readFileSync(join(SRC, 'components/Fold.tsx'), 'utf8');
    expect(src).toMatch(/onToggle=\{[\s\S]*revealOnToggle\(e\)[\s\S]*\}/);
  });

  /**
   * 「露出」的两条腿 —— 判据认哪一条，取决于**分组的 DOM 形状**，不取决于谁先写的。
   *
   * · `revealSiblingGroup` —— **扁平兄弟**（组头与项目并列，没有分组容器）：四个桌面菜单
   *   （`.ns-grp` / `.csel-grp` / `.tray-group-h`）都是这一形，只露出组头等于没露出。
   * · `revealElement` —— **容器**（`<section>` 里装着组头与内容）：移动端节点表单的可折叠段
   *   （`mobile/forms/NodeFormPanel.tsx`）是这一形，容器本身就是要滚进视区的那一段。
   *
   * 🔴 2026-09-06 从「只认 `revealSiblingGroup`」放宽到这两条。放宽的**边界**写清楚：
   * 一个既不调 A 也不调 B 的分组菜单**仍然**是违规（下面那条反向对照钉着这一点）。
   * 两条腿同住 `reveal.ts`、同走 `computeRevealDelta`，没有第二套滚动逻辑。
   * 只认 A 会把容器形的分组逼去用一个对它不成立的函数（`marker` 取组头的第一个类名、
   * 沿**兄弟**扫段尾 —— 容器形里组头没有兄弟，那条腿量出来的段恒等于组头自己）。
   */
  const REVEAL_LEGS = ['revealSiblingGroup', 'revealElement('] as const;
  const revealsSomething = (src: string): boolean => REVEAL_LEGS.some((leg) => src.includes(leg));

  it('可滚菜单里的分组展开全部露出（.ns-grp / .csel-grp / .tray-group-h 同一形状）', () => {
    // 判据取「用了 openGroups 这个惯用法的组件」，不是「本次改了哪四个文件」：
    // 四个菜单都带 max-height + overflow-y:auto（.node-menu 430 / .mini-menu 360 /
    // .csel-menu 300 / .tray-menu 600），组头在底部时展开，新项目整段落在菜单视区之外。
    // 移动端表单面板同理（`.m-form-body` 是 overflow-y:auto 的单一滚动容器）。
    // 第五个人再照抄这个惯用法加一个分组菜单，忘了露出 → 本条转红。
    const offenders = files
      .filter((f) => readFileSync(f, 'utf8').includes('openGroups'))
      .filter((f) => !revealsSomething(readFileSync(f, 'utf8')))
      .map((f) => f.slice(SRC.length));
    expect(offenders, '这些分组菜单展开后新项目落在视区外').toEqual([]);
  });

  it('反向对照：放宽之后「两条腿都没调」仍然算违规（否则上一条恒绿）', () => {
    expect(revealsSomething('const [openGroups, setOpenGroups] = useState(new Set());')).toBe(false);
    expect(revealsSomething('scheduleReveal(() => revealSiblingGroup(header));')).toBe(true);
    expect(revealsSomething('scheduleReveal(() => revealElement(section));')).toBe(true);
    // 「只是提到了那个名字的一部分」不算数：`revealElement` 必须是**调用**（带左括号）。
    expect(revealsSomething('import type { revealElementProps } from "./x";')).toBe(false);
  });

  it('确有分组菜单被扫到（防止上一条在空集合上恒绿）', () => {
    const n = files.filter((f) => readFileSync(f, 'utf8').includes('openGroups')).length;
    expect(n).toBeGreaterThanOrEqual(4);
  });

  it('ListEditor 的内联导入面板也走同一条路（它不是 details，拿不到 toggle）', () => {
    const src = readFileSync(join(SRC, 'components/screens/settings/ListEditor.tsx'), 'utf8');
    expect(src).toContain('revealElement(importPanelRef.current)');
    expect(src).toMatch(/\}, \[importOpen\]\);/);
  });
});
