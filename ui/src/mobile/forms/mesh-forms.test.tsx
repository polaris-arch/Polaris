/**
 * 组网三张表（WARP / Tailscale / WireGuard）在移动端**真的在**的门 —— 批 3。
 *
 * # 本门与既有那几道的分工（不重复实现任何一道）
 *
 *  · `form-host.test.ts` ② 守「union ↔ case 恰等」与 `mobileEditFormFor` 的分流；
 *  · `nodes-screen.test.tsx` ⑥-c 守「桌面五个选择 ↔ 登记表 ↔ 生产面板的分流」；
 *  · `wiring-completeness.test.ts` 数「还剩几条没接」。
 * 三道都是**源码/登记**面。它们全绿而这三张表其实画不出一个输入框，是完全可能的 ——
 * 本门补的正是那一格：**把面板真渲染出来，看那些字段在不在**。
 *
 * # 为什么这里能真渲染（而不是又一道 grep 门）
 *
 * 本仓 vitest 是 `environment:'node'`、刻意不装 jsdom，但 `renderToStaticMarkup` 只要 React
 * 本身（既有先例：`FieldSpec.switch-disabled.test.tsx` / `back-navigation.test.tsx`）。
 * 于是「字段渲染出来没有」「私钥那格是不是 password」「分组标题在不在」这三类是**能真测的**。
 *
 * ⚠️ **测不到什么，如实记**：
 *  · `useEffect` 不执行 ⇒ Tailscale 的「等地址超时」「卸载杀核」、TS 设置表的出口候选拉取，
 *    这三条只有源码级取证，行为归真机验收；
 *  · 交互（点开分组 / 切换分段 / 提交）不发生 ⇒ 默认收起的分组里那些字段量不到，
 *    故下面对折叠段只断言**组头在**，对展开的那一组才断言字段在；
 *  · 几何与 CSS 不在射程内。
 *
 * # 🔴 一条都不 mock
 *
 * 面板真身、真的 `wgSpec()` / `tsMainSpec()` / `warpAdvancedSpec()`、真的 store。
 * 前几批复审累计抓到六条 major 都是同一形态：判据把要验的组件 mock 成桩，于是
 * 「组件挂着但它不再调那条腿」这一档全绿。这里把两条判据**成对**交：
 * 规格函数的返回值（本体）× 面板渲染出来的 HTML（生产在用它）。
 */

import { afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { ReactElement } from 'react';
import * as tsc from '@/test/ts-compiler';

import type { ServerConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';
import { wgSpec } from '@/components/dialogs/wg-spec';
import { TS_ADV_SPEC, tsMainSpec } from '@/components/dialogs/ts-spec';
import {
  planWarpSubmit,
  parseHostPort,
  warpAdvancedSpec,
  warpDraftFromNode,
} from '@/components/dialogs/warp-spec';
import { tsLoginBrowserView } from '@/components/dialogs/ts-login-server';
import { endpointDetourOptions } from '@/components/dialogs/detour-options';
import { buildWgServer, draftFromServer, parseConfToDraft } from '@/components/dialogs/wg-logic';
import { groupTsFields, groupWgFields } from '@/components/dialogs/mesh-form-layout';
import { exitNodeOptions, initTsDraft } from '@/components/dialogs/ts-settings-logic';
import { meshSingletonConflict } from '@/domain/endpoint-routes';
import { MESH_JOIN_CHOICES } from '../nodes/absence-register';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';

import { MobileFields } from './FormFields';
import { MeshJoinPanel, meshJoinFormFor } from './MeshJoinPanel';
import { WgPanel } from './WgPanel';
import { WarpPanel } from './WarpPanel';
import { TsLoginPanel } from './TsLoginPanel';
import { TsSettingsPanel } from './TsSettingsPanel';
import { useMobileFormStore } from './form-store';

const read = (rel: string): string => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
/** 剥注释：本文件与被读的源码都逐字引用了要断言的标识符，不剥就是拿注释当证据。 */
const strip = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/.*$/gm, '$1');

const html = (el: ReactElement): string => renderToStaticMarkup(el);

/**
 * zustand v5 在 `react-dom/server` 下读的是**创建那一刻**的状态对象（`api.getInitialState()`），
 * 故渲染前必须把当下实时状态整体镜像进去 —— 否则每次渲染都拿空 store 跑，
 * 「存量节点」那一档在门里根本不存在。
 *
 * 🔴 本文件此前把这件事记成了**本仓不可逾越的限制**（「渲染不到有节点的那一档」），
 * 于是三张表的**编辑态一次都没被渲染过**：五个渲染点全部只传 `instanceId`，`serverId` / `edit`
 * 一次都没传。那句话是错的 —— `mobile-chrome.test.tsx:104` 与 `config-sync.test.tsx:134`
 * 早就用同一条镜像把 store 喂进 `renderToStaticMarkup` 并断言渲染输出随之改变。
 * 2026-09-06 复审 major 已订正，编辑态改由下面 ②-f 真渲染。
 */
function mirrorLiveStateIntoSsrSnapshot(): void {
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
}

/* ── 源码级取材：走真 TypeScript AST，不用正则 ────────────────────────────────
 *
 * 本批复审抓到的三条 major 是同一个形状：判据只覆盖**函数体**，不覆盖「把参数喂给它」那一跳
 * （`<MobileFields fields={spec}/>` / `open(target)` / `planWarpSubmit({ …, isEdit })`）。
 * 实测变异：把 `open(target)` 换成 `open({ kind: 'node' })` —— 三张表全废，全仓门一条不红。
 * 用 AST 而不是子串：`toContain('open(')` 连注释都能满足，而实参是**结构**，只有结构判得了。
 */
const absOf = (rel: string): string => fileURLToPath(new URL(rel, import.meta.url));
const parse = (rel: string): tsc.SourceFile => tsc.parseSourceFile(absOf(rel), read(rel));

/** 子树里的全部节点（含自身）。 */
const under = (root: tsc.Node): tsc.Node[] => {
  const out: tsc.Node[] = [];
  const walk = (n: tsc.Node): void => {
    out.push(n);
    tsc.forEachChild(n, walk);
  };
  walk(root);
  return out;
};

const calleeName = (c: tsc.CallExpression): string | null => {
  const e = c.expression;
  if (tsc.isIdentifier(e)) return e.text;
  if (tsc.isPropertyAccessExpression(e) && tsc.isIdentifier(e.name)) return e.name.text;
  return null;
};

/** 某棵子树（整份文件或一个回调）里对 `name(…)` 的全部调用。 */
const callsTo = (root: tsc.Node, name: string): tsc.CallExpression[] =>
  under(root).filter(
    (n): n is tsc.CallExpression => tsc.isCallExpression(n) && calleeName(n) === name,
  );

/** 标识符实参 ⇒ 它的名字；其它形态（字面量 / 数组 / 调用…）⇒ `null`。 */
const argIdent = (call: tsc.CallExpression, index: number): string | null => {
  const a = call.arguments[index];
  return a !== undefined && tsc.isIdentifier(a) ? a.text : null;
};

/** `<Tag attr={…}>` 里那个表达式（同一文件里同名标签的全部实例）。 */
const jsxAttrExprs = (sf: tsc.SourceFile, tag: string, attr: string): tsc.Node[] => {
  const tagOf = (name: tsc.Node): string => (tsc.isIdentifier(name) ? name.text : '');
  const out: tsc.Node[] = [];
  for (const n of under(sf)) {
    const opening = tsc.isJsxSelfClosingElement(n)
      ? n
      : tsc.isJsxElement(n)
        ? n.openingElement
        : null;
    if (opening === null || tagOf(opening.tagName) !== tag) continue;
    for (const p of opening.attributes.properties) {
      if (!tsc.isJsxAttribute(p) || !tsc.isIdentifier(p.name) || p.name.text !== attr) continue;
      const init = p.initializer;
      if (init !== undefined && tsc.isJsxExpression(init) && init.expression !== undefined) {
        out.push(init.expression);
      }
    }
  }
  return out;
};

/** `const <name> = f(…)` 的那次调用（同名多处则取第一处）。 */
const initCallOf = (sf: tsc.SourceFile, name: string): tsc.CallExpression | null => {
  for (const n of under(sf)) {
    if (!tsc.isVariableDeclaration(n) || !tsc.isIdentifier(n.name) || n.name.text !== name) continue;
    const init = n.initializer;
    if (init !== undefined && tsc.isCallExpression(init)) return init;
  }
  return null;
};

/** 对象字面量实参里某个键的值节点（`{ isEdit }` 这种简写也认）。 */
const objectArgProp = (
  call: tsc.CallExpression,
  index: number,
  key: string,
): { kind: 'shorthand' | 'value'; node: tsc.Node } | null => {
  const a = call.arguments[index];
  if (a === undefined || !tsc.isObjectLiteralExpression(a)) return null;
  for (const p of a.properties) {
    const named =
      (tsc.isPropertyAssignment(p) || tsc.isShorthandPropertyAssignment(p)) &&
      (tsc.isIdentifier(p.name) || tsc.isStringLiteral(p.name)) &&
      p.name.text === key;
    if (!named) continue;
    if (tsc.isShorthandPropertyAssignment(p)) return { kind: 'shorthand', node: p.name };
    if (tsc.isPropertyAssignment(p)) return { kind: 'value', node: p.initializer };
  }
  return null;
};

/**
 * 键 → **渲染出来会长成什么样**（已本地化 + HTML 转义）。
 *
 * 🔴 断言必须走这一层，不许写死英文串：i18n 在本套件里是**真初始化**的（面板用的是真
 * `useTranslation`），HTML 里出现的是译文而不是键。写死英文 = 换一句文案就红一次，
 * 而那不是缺陷；走 `i18n.t(键)` 则判据仍然钉在**键**上 —— 键没接上时它当场红。
 */
const tr = (key: string): string =>
  i18n
    .t(key)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');

beforeAll(async () => {
  /* 语言包是异步加载的（`i18n/index.ts` 的按需 loader）。不等它，`t()` 会回退成键，
     下面每条 `toContain(tr(...))` 就变成「键 == 键」的空断言。 */
  await i18nReady;
});

const wgNode = (over: Partial<ServerConfig> = {}): ServerConfig => ({
  id: 'wg-1',
  name: 'wg node',
  protocol: 'wireguard',
  address: '203.0.113.7',
  port: 51820,
  wireguardSettings: {
    privateKey: 'PRIVATE-KEY-DO-NOT-LEAK=',
    localAddress: ['10.0.0.2/32'],
    peerPublicKey: 'PEER-PUB=',
  },
  ...over,
});

const tsNode = (): ServerConfig => ({
  id: 'ts-1',
  name: 'Tailscale',
  protocol: 'tailscale',
  address: '',
  port: 0,
  tailscaleSettings: { hostname: 'sway-phone', authKey: 'AUTH-KEY-DO-NOT-LEAK' },
});

/**
 * 干净起手：store 与表单栈都清空（残留会让下面每条都在别人的状态上跑）。
 * 清完**同步镜像一次**：`mirrorLiveStateIntoSsrSnapshot` 是直接改 `getInitialState()` 那个对象，
 * 不在这里把清空态也镜像回去的话，上一条用例塞进去的节点会留在下一条的渲染快照里。
 */
function resetStores(): void {
  useMobileFormStore.getState().closeAll();
  useStagedConfigStore.setState({ entries: [] });
  useAppStore.setState({ servers: [], config: null });
  mirrorLiveStateIntoSsrSnapshot();
}

beforeEach(resetStores);
afterEach(resetStores);

/* ════════════════════════════════════════════════════════════════════════════
 * ① 拆分的形态：三份规格是**零 React** 的 `.ts`，且两端真的都在用它
 *
 * 契约 A1（`mobile-entry.test.ts` / `nodes-screen.test.tsx` ② 的 A 腿边界）的**意图**是
 * 「不许把会渲染的控件引进移动端」。规格拆成零 import 的 `.ts` 之后两侧都成立 ——
 * 但那句话只有在规格文件**真的没有 React / 没有 CSS** 时才成立，故这里正面钉住。
 *
 * 变异靶：往 `wg-spec.ts` 里加一行 `import { useState } from 'react';` → ①-a 红。
 * ══════════════════════════════════════════════════════════════════════════ */
describe('① 三份规格拆成了零 React 的 `.ts`，且两端真的共用它', () => {
  const SPECS = [
    ['wg-spec.ts', read('../../components/dialogs/wg-spec.ts')],
    ['ts-spec.ts', read('../../components/dialogs/ts-spec.ts')],
    ['warp-spec.ts', read('../../components/dialogs/warp-spec.ts')],
  ] as const;

  /** 「这份源码碰 React 或 CSS 了吗」。返回命中的说明，没有则 `null`。 */
  const touchesRenderer = (src: string): string | null => {
    const body = strip(src);
    if (/from\s+'react[^']*'/.test(body)) return 'import react';
    if (/from\s+'react-i18next'/.test(body)) return 'import react-i18next';
    if (/\.css'/.test(body)) return 'import css';
    if (/<[A-Za-z]/.test(body)) return 'jsx';
    return null;
  };

  it('⓪ 自检：谓词认得出坏形态，也不误伤好形态（否则下面那条否定断言是空话）', () => {
    expect(touchesRenderer("import { useState } from 'react';\n")).toBe('import react');
    expect(touchesRenderer("import './x.css';\n")).toBe('import css');
    expect(touchesRenderer('const a = <div />;\n')).toBe('jsx');
    expect(touchesRenderer("import type { X } from '@/contracts/types';\nexport const a = 1;\n")).toBeNull();
  });

  it('①-a 三份规格都不碰 React / CSS（碰了就等于把桌面渲染链拖进移动包）', () => {
    for (const [name, src] of SPECS) {
      expect(src.length, `${name} 读空了 —— 被改名/移走了？`).toBeGreaterThan(800);
      expect(touchesRenderer(src), `${name} 碰了渲染层`).toBeNull();
    }
  });

  it('①-b 桌面调用点没改：三个弹窗 import 的是拆出来的那一份，`wgSpec` 仍从 WgDialog 再导出', () => {
    const wgDialog = strip(read('../../components/dialogs/WgDialog.tsx'));
    expect(wgDialog, 'WgDialog 没有再导出 wgSpec —— 既有调用点会断').toContain(
      "export { wgSpec } from './wg-spec'",
    );
    expect(wgDialog, 'WgDialog 自己也要用它（否则桌面表单是空的）').toContain("from './wg-spec'");
    expect(strip(read('../../components/dialogs/TsSettingsDialog.tsx')), 'TsSettingsDialog 没接上 ts-spec').toContain(
      "from './ts-spec'",
    );
    expect(strip(read('../../components/dialogs/WarpDialog.tsx')), 'WarpDialog 没接上 warp-spec').toContain(
      "from './warp-spec'",
    );
  });

  it('①-c 移动端四张面板 import 的也是同一份规格（不是自己抄了一张表）', () => {
    const cases: ReadonlyArray<readonly [string, string]> = [
      ['WgPanel.tsx', '@/components/dialogs/wg-spec'],
      ['TsSettingsPanel.tsx', '@/components/dialogs/ts-spec'],
      ['WarpPanel.tsx', '@/components/dialogs/warp-spec'],
      ['TsLoginPanel.tsx', '@/components/dialogs/ts-login-server'],
    ];
    for (const [file, spec] of cases) {
      expect(strip(read(`./${file}`)), `${file} 没有引用共用规格 ${spec}`).toContain(spec);
    }
  });
});

/* ════════════════════════════════════════════════════════════════════════════
 * ② 真渲染：字段不是「表里有」，是**画出来了**
 *
 * 这一组是本门存在的主要理由。判据成对交：
 *  · **本体**：`wgSpec()` / `tsMainSpec()` / `warpAdvancedSpec()` 的返回值里有那些 `k`；
 *  · **生产在用它**：面板渲染出来的 HTML 里有那些 `label`。
 * 只测前者 = 表对了但没人渲染；只测后者 = 画出来了但可能是面板自己抄的一张表。
 *
 * 变异靶（每条都实测过会红）：
 *  · 把 `WgPanel` 里 `groups[id]` 换成 `[]`（字段一个不渲染）→ ②-a 红；
 *  · 把 `wgSpec` 里 `k: 'privateKey'` 那一行的 `secret: true` 删掉 → ②-c 红；
 *
 * ⚠️ 这里曾经还写着一条**没验证过**的收据：「把 `WarpPanel` 的 `warpAdvancedSpec(...)` 换成 `[]`
 * → ②-b 红」。实测是假的：WARP 的「高级」组默认收起（同桌面 `FormSection collapsible`），
 * `FormGroup` 收起时 `MobileFields` 根本不渲染 ⇒ ②-b 对 WARP 只量得到组头。那一跳
 * （`fields={spec}`）与高级组里那五个控件的渲染面，2026-09-06 复审后改由 ⑤-c 成对钉住。
 * ══════════════════════════════════════════════════════════════════════════ */
describe('② 真渲染：三张表的字段在移动端画得出来', () => {
  it('⓪ 自检：面板真的渲染出了表单外壳，且 i18n 真的初始化了（两者任一塌掉都会让下面恒假/恒真）', () => {
    const out = html(<WgPanel instanceId="i1" />);
    expect(out.length, 'WgPanel 渲染出来是空的').toBeGreaterThan(500);
    expect(out, '渲染的不是表单外壳').toContain('class="m-form-panel"');
    expect(out, '底部动作区不见了').toContain('class="m-form-foot"');
    expect(tr('common.cancel'), 'i18n 没初始化 —— 译文回退成了键，下面每条断言都会退化成「键==键」').not.toBe(
      'common.cancel',
    );
  });

  it('②-a WireGuard：`wgSpec` 的基础组字段逐个画出来了（含私钥与对端公钥）', () => {
    const spec = wgSpec({}, undefined, [], []);
    const basicKeys = groupWgFields(spec).basic.map((f) => f.k);
    // 本体自检：分组非空且含那几个承重字段（空表会让下面的循环恒真）。
    expect(basicKeys, 'wgSpec 的基础组里没有私钥').toContain('privateKey');
    expect(basicKeys, 'wgSpec 的基础组里没有对端公钥').toContain('peerPublicKey');
    expect(spec.length, 'wgSpec 返回的字段太少 —— 表塌了').toBeGreaterThan(10);

    const out = html(<WgPanel instanceId="i1" />);
    /* `basic` 组默认展开 ⇒ 它那几格必须真的在 HTML 里。其余两组默认收起（见文件头射程自曝），
       只断言组头在。 */
    for (const key of basicKeys) {
      const label = spec.find((f) => f.k === key)!.label;
      expect(out, `WgPanel 没有画出 ${key}（label 键 ${label}）`).toContain(tr(label));
    }
    for (const title of ['node.formGroup.connection', 'node.formGroup.routing', 'node.formGroup.advanced']) {
      expect(out, `WgPanel 少了分组 ${title}`).toContain(tr(title));
    }
    // 两条来源（手填 / 粘贴 .conf）都在，且默认停在手填。
    expect(out, '少了「粘贴 .conf」那一支').toContain(tr('wg.paste'));
    /* 反向对照：收起的那两组里的字段**不该**出现 —— 否则「画出来了」这句话没有区分力
       （整份源码都塞进 HTML 时上面那批也会全过）。 */
    expect(out, '折叠的分组把字段也渲染出来了').not.toContain(tr('wg.reserved'));
  });

  it('②-b WARP 注册表与 Tailscale 登录表：档位 / 方式 / 主按钮都画出来了', () => {
    const warp = html(<WarpPanel instanceId="i1" />);
    expect(warp, 'WARP 少了名称字段').toContain(tr('warp.name'));
    expect(warp, 'WARP 少了计费档分段').toContain(tr('warp.plan'));
    expect(warp, 'WARP 的主按钮不是「注册」').toContain(tr('warp.register'));
    expect(warp, 'WARP 少了高级折叠段（`warpAdvancedSpec` 的落点）').toContain(tr('node.formGroup.advanced'));
    // 反向对照：注册态不该出现「已接入」那一屏（`done` 分支），否则上面几条可能只是「串在源码里」。
    expect(warp, '一进来就画成了已完成').not.toContain(tr('warp.doneTitle'));
    // 本体：高级组那张表真的有内容（空表时上面那条只证明了组头在）。
    expect(warpAdvancedSpec([], 'ph', []).map((f) => f.k)).toContain('endpoint');

    const login = html(<TsLoginPanel instanceId="i2" />);
    expect(login, 'TS 登录少了方式分段').toContain(tr('ts.method'));
    expect(login, 'TS 登录少了浏览器登录那一支').toContain(tr('ts.browserLogin'));
    expect(login, 'TS 登录少了 Auth Key 那一支').toContain(tr('ts.authKey'));
    expect(login, '初始态没有给出浏览器登录的说明').toContain(tr('ts.browserHint'));
    // 反向对照：还没提交就画出「等地址 / 超时」两态之一，说明那几个分支的条件写反了。
    expect(login, '一进来就在等登录地址').not.toContain(tr('ts.awaitingUrl'));
    expect(login, '一进来就报了超时').not.toContain(tr('ts.awaitingUrlTimeout'));

    /* TS 设置表的**空态**：一个 tailscale 节点都没有时如实说明，且提交键置灰
       （没有可写的目标；点得动才是缺陷）。这是本仓渲染得到的唯一一档 TS 设置态 ——
       「有节点」那一档见下面 ②-c 的说明。 */
    const settings = html(<TsSettingsPanel instanceId="i3" serverId="ts-1" />);
    expect(settings, 'TS 设置在没有节点时没给出说明').toContain(tr('ts.noNode'));
    expect(settings, 'TS 设置空态没有把保存键置灰 —— 那颗按钮没有作用对象').toMatch(
      /class="m-form-btn primary"[^>]*disabled/,
    );
    // 反向对照：同一次渲染里「取消」不该被置灰（否则上一条只证明了「有个 disabled 字样」）。
    expect(settings, '取消键被一起锁掉了').toMatch(/class="m-form-btn">/);
  });

  /**
   * 编辑态的**回填链**（规格 → 渲染器这一段；面板整条链见下面 ②-f）。
   *
   * ⚠️ 这里曾经写着一句错的话：「本仓能渲染，但渲染不到『有节点』的那一档」。它给的理由是
   * zustand v5 在 `renderToStaticMarkup` 下取 `api.getInitialState()`、`setState` 影响不了渲染
   * —— 前半句是事实，后半句的结论不成立：把实时状态**镜像**进那个初始状态对象即可
   * （`mirrorLiveStateIntoSsrSnapshot`，仓里两处先例 `mobile-chrome.test.tsx:104` /
   * `config-sync.test.tsx:134`）。那句话的后果是三张表的编辑态**一次都没被渲染过**：五个渲染点
   * 全部只传 `instanceId`，`serverId` / `edit` 一次都没传，于是「面板找不到编辑基准 ⇒ 画成空白
   * 新建表 ⇒ 保存时走 add、盘上多一个空节点」这一整档缺陷一条门都拦不住
   * （2026-09-06 复审 major，已订正）。
   *
   * 本条仍然留着，射程也没变：它测**规格 → 渲染器**（不经面板），与 ②-f 的「面板整条链」成对交
   * —— 只有本条 = 函数对了但没人调，只有 ②-f = 画出来了但可能是面板自己抄的一张表。
   */
  it('②-c 编辑态的回填：存量值经生产的规格 + 生产的渲染器真的画进了输入框', () => {
    const node = wgNode();
    const draft = draftFromServer(node);
    const fields = wgSpec(draft, node, [], []);
    const out = html(<MobileFields fields={groupWgFields(fields).basic} values={draft} onChange={() => {}} t={(k) => i18n.t(k)} />);
    expect(out, '编辑态没回填对端地址').toContain('value="203.0.113.7"');
    expect(out, '编辑态没回填端口').toContain('value="51820"');
    expect(out, '编辑态没回填接口地址').toContain('value="10.0.0.2/32"');
    expect(out, '编辑态没回填对端公钥').toContain('value="PEER-PUB="');
    // 反向对照：空草稿下这些值不该出现（否则上一条可能只是「串在规格的 placeholder 里」）。
    const empty = html(
      <MobileFields fields={groupWgFields(wgSpec({}, undefined, [], [])).basic} values={{}} onChange={() => {}} t={(k) => i18n.t(k)} />,
    );
    expect(empty, '空草稿凭空带出了存量值').not.toContain('value="203.0.113.7"');

    // Tailscale 同形：`initTsDraft` 起底 + `tsMainSpec` 的基础组真的画得出主机名与出口。
    const tsDraft = initTsDraft(tsNode());
    const tsFields = groupTsFields([...tsMainSpec([['', tr('ts.exitNone')]], [], []), ...TS_ADV_SPEC]);
    const tsOut = html(<MobileFields fields={tsFields.basic} values={tsDraft} onChange={() => {}} t={(k) => i18n.t(k)} />);
    expect(tsOut, 'TS 编辑态没回填主机名').toContain('value="sway-phone"');
    expect(tsOut, 'TS 基础组没画出出口设备').toContain(i18n.t('ts.exitNode'));
    expect(TS_ADV_SPEC.map((f) => f.k), 'TS_ADV_SPEC 里没有 controlUrl').toContain('controlUrl');

    /* 🔴 `authKey` 是登录腿写进去的**凭据**，设置表既不展示也不能覆写它
       （`buildTsSettings` 以 base 起底保全未建模字段）。它绝不许出现在这张表的任何一格里。 */
    expect(tsOut, 'Tailscale 的 authKey 被画进了设置表').not.toContain('AUTH-KEY-DO-NOT-LEAK');
  });

  it('②-d 私钥那一格渲染成 `type="password"` —— 喂的是**生产那张表**，不是自造夹具', () => {
    const spec = wgSpec({}, undefined, [], []);
    const secretKeys = spec.filter((f) => f.t === 'text' && f.secret === true).map((f) => f.k);
    expect(secretKeys, '`wgSpec` 里私钥不再是 secret —— 输入框会明文回显').toContain('privateKey');
    expect(secretKeys, '`wgSpec` 里预共享密钥不再是 secret').toContain('preSharedKey');

    const out = html(
      <MobileFields
        fields={spec.filter((f) => f.k === 'privateKey' || f.k === 'address')}
        values={{ privateKey: 'PRIVATE-KEY-DO-NOT-LEAK=', address: '203.0.113.7' }}
        onChange={() => {}}
        t={(k) => i18n.t(k)}
      />,
    );
    expect(out, '私钥输入框不是 password —— 肩窥就能读走').toContain('type="password"');
    // 正向对照：同一次渲染里非 secret 的那一格是 text（否则「全都是 password」也能骗过上一条）。
    expect(out, '普通字段被误判成 secret').toContain('type="text"');
    // 显隐键在场（44px 触控目标那颗；桌面那颗 20px 字形在触屏上不成立）。
    expect(out, '没有显隐切换 —— 用户没法核对自己粘对了没有').toContain(i18n.t('common.showSecret'));
    /* 私钥只许作为那一个 password 输入框的 `value` 出现**一次**：出现两次意味着别处也回显了它
       （预览块、调试块、aria-label…）。桌面 `WgDialog` 有一块 `.wg-preview`，移动端刻意没搬。 */
    const hits = [...out.matchAll(/PRIVATE-KEY-DO-NOT-LEAK=/g)].length;
    expect(hits, `私钥在 HTML 里出现了 ${hits} 次 —— 它只该是那一个 password 输入框的值`).toBe(1);
  });

  it('②-e 接线面：面板真的用生产的回填函数起底，且凭据不进日志', () => {
    const wgSrc = strip(read('./WgPanel.tsx'));
    expect(wgSrc, 'WgPanel 编辑态不是用 `draftFromServer` 起底 —— 存量字段会被清空后覆盖写回').toContain(
      'draftFromServer(base)',
    );
    expect(wgSrc, 'WgPanel 新增态不是用 `emptyWgDraft` 起底').toContain('emptyWgDraft()');
    const tsSrc = strip(read('./TsSettingsPanel.tsx'));
    expect(tsSrc, 'TsSettingsPanel 不是用 `initTsDraft` 起底 —— 缺键会被 `buildTsSettings` 当成「用户清空了」').toContain(
      'initTsDraft(node)',
    );
    expect(tsSrc, 'TsSettingsPanel 没把分组喂给渲染器').toContain('groups[id]');

    /* 三张表的 `console.*` 只吐错误对象，不吐草稿 / 组装好的 server / 凭据。
       变异靶：把 `console.error('[mobile-wg-form] save failed:', e)` 改成 `..., e, draft)` → 红。 */
    for (const file of ['WgPanel.tsx', 'WarpPanel.tsx', 'TsLoginPanel.tsx', 'TsSettingsPanel.tsx']) {
      const body = strip(read(`./${file}`));
      const calls = [...body.matchAll(/console\.\w+\(([^;]*?)\);/g)].map((m) => m[1]);
      expect(calls.length, `${file} 里一处 console 都没有 —— 诊断腿没了`).toBeGreaterThan(0);
      for (const args of calls) {
        for (const leak of ['draft', 'server', 'authKey', 'license', 'privateKey', 'next']) {
          expect(
            new RegExp(`\\b${leak}\\b`).test(args),
            `${file} 的 console 里带上了 ${leak} —— 凭据会进日志`,
          ).toBe(false);
        }
      }
    }
  });

  /**
   * ②-f **编辑态真渲染**（2026-09-06 复审 major 新增）—— 面板整条链，不是拆开的两半。
   *
   * 这一档此前在本仓「测不到」，理由写在 ②-c 头上、而那句话是错的（见那里的订正）。
   * 现在把实时状态镜像进 SSR 快照即可驱动，于是下面这些形态第一次有门：
   *  · `WgPanel` 拿 `serverId` 找不到基准 ⇒ 画成空白新建表 ⇒ 保存走 `add`，盘上多一个空节点；
   *  · `WarpPanel` 的 `edit` 没接到 `findWarpNode` ⇒ 主按钮仍是「注册」，编辑变成再注册一台；
   *  · `TsSettingsPanel` 找不到 TS 节点 ⇒ 恒画空态、保存键恒灰。
   * 每条都配一条**反向对照**：空 store 下同一断言必须落到相反的一边。
   */
  it('②-f 编辑态：面板拿 serverId / edit 真的找到了基准，并把存量值画进了输入框', () => {
    /* ⓪ 自检：镜像这条腿真的在起作用（不起作用的话下面三组全是在空 store 上跑）。 */
    useAppStore.setState({ servers: [wgNode()] });
    mirrorLiveStateIntoSsrSnapshot();
    expect(
      useAppStore.getInitialState().servers.map((s) => s.id),
      'SSR 快照没跟上实时状态 —— 下面每条都在空 store 上渲染，恒绿',
    ).toEqual(['wg-1']);

    /* ① WireGuard 编辑态：存量值真的回填进了输入框，标题与主按钮也切到编辑那一档。 */
    const wgEdit = html(<WgPanel instanceId="i1" serverId="wg-1" />);
    expect(wgEdit, '编辑态没回填对端地址 —— 面板没找到基准，这是一张空白新建表').toContain(
      'value="203.0.113.7"',
    );
    expect(wgEdit, '编辑态没回填端口').toContain('value="51820"');
    expect(wgEdit, '编辑态没回填对端公钥').toContain('value="PEER-PUB="');
    expect(wgEdit, '编辑态的主按钮不是「保存」').toContain(tr('common.save'));
    /* 反向对照：同一个 `serverId` 在空 store 上必须画不出这些值（证明上面几条量的是 store）。 */
    useAppStore.setState({ servers: [] });
    mirrorLiveStateIntoSsrSnapshot();
    const wgOrphan = html(<WgPanel instanceId="i1" serverId="wg-1" />);
    expect(wgOrphan, '基准不在时凭空带出了存量值').not.toContain('value="203.0.113.7"');
    expect(wgOrphan, '基准不在时主按钮仍是「保存」—— 那会走 update 打一个不存在的 id').toContain(
      tr('wg.add'),
    );

    /* ② WARP 编辑态：`edit` 经 `findWarpNode` 落到那台已注册设备上。 */
    const warpNode = wgNode({
      id: 'warp-0',
      name: 'WARP',
      wireguardSettings: {
        privateKey: 'k',
        localAddress: ['172.16.0.2/32'],
        peerPublicKey: 'p',
        warpDevice: { deviceId: 'd', token: 't' },
      },
    });
    useAppStore.setState({ servers: [warpNode] });
    mirrorLiveStateIntoSsrSnapshot();
    const warpEdit = html(<WarpPanel instanceId="i2" edit />);
    expect(warpEdit, 'WARP 编辑态的标题不是「编辑」').toContain(tr('warp.editTitle'));
    expect(warpEdit, 'WARP 编辑态的主按钮不是「保存」—— 按下去会再注册一台远端设备').toContain(
      tr('common.save'),
    );
    expect(warpEdit, 'WARP 编辑态没回填名称').toContain('value="WARP"');
    expect(warpEdit, 'WARP 编辑态还在画新建那句说明').not.toContain(tr('warp.note'));
    // 反向对照：`edit` 为假时同一份 store 上必须是注册态。
    const warpNew = html(<WarpPanel instanceId="i2" />);
    expect(warpNew, '没传 edit 却画成了编辑态').toContain(tr('warp.register'));

    /* ③ Tailscale 设置表：有节点时不再画空态，且保存键不再置灰。 */
    useAppStore.setState({ servers: [tsNode()] });
    mirrorLiveStateIntoSsrSnapshot();
    const tsEdit = html(<TsSettingsPanel instanceId="i3" serverId="ts-1" />);
    expect(tsEdit, '有 TS 节点却仍画空态 —— 面板没找到它').not.toContain(tr('ts.noNode'));
    expect(tsEdit, 'TS 设置表没回填主机名').toContain('value="sway-phone"');
    expect(tsEdit, '有节点时保存键仍被置灰').not.toMatch(/class="m-form-btn primary"[^>]*disabled/);
    /* 🔴 `authKey` 是登录腿写进去的凭据：整张表（不只是那一格）都不许回显它。
       这一条此前只在「规格 → 渲染器」那一半上量过，现在量的是面板真身。 */
    expect(tsEdit, 'Tailscale 的 authKey 被画进了设置表').not.toContain('AUTH-KEY-DO-NOT-LEAK');
    // 反向对照：空 store 下同一断言必须落回空态那一边。
    useAppStore.setState({ servers: [] });
    mirrorLiveStateIntoSsrSnapshot();
    expect(html(<TsSettingsPanel instanceId="i3" serverId="ts-1" />), '空 store 下没画空态').toContain(
      tr('ts.noNode'),
    );
  });

});

/* ════════════════════════════════════════════════════════════════════════════
 * ③ 单例拦截与提交校验：**共用同一条谓词**，不是两端各写一份
 *
 * 上一批栽过一次：订阅 URL 校验被在移动端重写成正则，注释却声称「与桌面同一条」，
 * 实测四组输入两侧结论相反。本组因此不只断言「两边都有拦」，而是断言**判据只有一份**，
 * 并直接拿那一份跑输入表。
 * ══════════════════════════════════════════════════════════════════════════ */
describe('③ 单例闸与提交校验：一份判据，两个客户端', () => {
  it('③-a WARP 的三档拒绝住在 `planWarpSubmit`，两端都调它、且都不再自己写 if', () => {
    const rows: ReadonlyArray<readonly [string, Parameters<typeof planWarpSubmit>[0], string | null]> = [
      ['名字空 ⇒ 拒', { name: '  ', plan: 'free', license: '', endpointRaw: '', isEdit: false }, 'name'],
      [
        'WARP+ 没填许可证 ⇒ 拒',
        { name: 'WARP', plan: 'plus', license: ' ', endpointRaw: '', isEdit: false },
        'license',
      ],
      [
        '端点填了但解析不出 ⇒ 拒',
        { name: 'WARP', plan: 'free', license: '', endpointRaw: 'not-an-endpoint', isEdit: false },
        'endpoint',
      ],
      [
        '编辑态端点必须解析得出',
        { name: 'WARP', plan: 'free', license: '', endpointRaw: '', isEdit: true },
        'endpoint',
      ],
      ['新建态端点留空 ⇒ 放行（由注册应答给）', { name: 'WARP', plan: 'free', license: '', endpointRaw: '', isEdit: false }, null],
      [
        '合法端点 ⇒ 放行并带出解析结果',
        { name: 'WARP', plan: 'free', license: '', endpointRaw: '[2606:4700::1]:2408', isEdit: true },
        null,
      ],
    ];
    for (const [what, input, expected] of rows) {
      expect(planWarpSubmit(input).reject, what).toBe(expected);
    }
    expect(planWarpSubmit({ name: 'W', plan: 'free', license: '', endpointRaw: '[2606:4700::1]:2408', isEdit: true }).endpoint)
      .toEqual({ host: '2606:4700::1', port: 2408 });
    expect(parseHostPort('engage.cloudflareclient.com:2408')).toEqual({
      host: 'engage.cloudflareclient.com',
      port: 2408,
    });

    /* 两端都消费它，且**都没有**自己那份「名字空就拒」的 if —— 后者才是分叉的入口。 */
    for (const file of ['../../components/dialogs/WarpDialog.tsx', './WarpPanel.tsx']) {
      const body = strip(read(file));
      expect(body, `${file} 没有调用共用判据 planWarpSubmit`).toContain('planWarpSubmit(');
      expect(
        /if\s*\(\s*!name\.trim\(\)/.test(body) || /name\.trim\(\)\s*===\s*''/.test(body),
        `${file} 又自己写了一份名字校验 —— 两端必然在某天对同一份输入给出不同结论`,
      ).toBe(false);
    }
  });

  it('③-b 粘贴 Cloudflare 的 wg-quick `.conf` 会撞上 WARP 单例槽（本面板最真实的旁路腿）', () => {
    /* 这条不是假设：`isWarpServer` 有一条端点域名兜底，粘贴 CF 的 .conf 造出来的就是一个 WARP。
       已有 WARP 时放行 ⇒ 两者抢内核 utun ⇒ `Connect: resource busy` FATAL。 */
    const conf = [
      '[Interface]',
      'PrivateKey = wOEabc=',
      'Address = 172.16.0.2/32',
      '[Peer]',
      'PublicKey = HIgabc=',
      'Endpoint = engage.cloudflareclient.com:2408',
      'AllowedIPs = 0.0.0.0/0, ::/0',
    ].join('\n');
    const draft = parseConfToDraft(conf);
    expect(draft, '解析器没认出这份 .conf —— 下面那条判据会在空气上跑').not.toBeNull();
    const candidate = buildWgServer('WARP-clone', draft!, undefined);
    const existing = wgNode({
      id: 'warp-0',
      wireguardSettings: {
        privateKey: 'k',
        localAddress: ['172.16.0.2/32'],
        peerPublicKey: 'p',
        warpDevice: { deviceId: 'd', token: 't' },
      },
    });
    expect(
      meshSingletonConflict(candidate, [existing]),
      '粘贴 CF 的 .conf 没有被判成 WARP —— 单例槽形同虚设',
    ).toBe('warp');
    // 反向对照：槽位空着时放行（不是「一律拒绝」）。
    expect(meshSingletonConflict(candidate, [])).toBeNull();
    /* 生产面板真的过这道闸（判据在那儿，面板没调 = 闸装在别人家门口）。
       ⚠️ 这两条是**词法**的，管不住喂进去的实参 —— 复审实测把 `(servers, …)` 换成 `([], …)`
       即两道闸恒放行，而这两条照旧全绿。实参那一跳由 ⑤-f 钉住，两条成对读。 */
    expect(strip(read('./WgPanel.tsx')), 'WgPanel 没过单例闸').toContain('blockedByMeshSingleton(');
    expect(strip(read('./WarpPanel.tsx')), 'WARP 注册没走「先过闸再打远端」那条腿').toContain(
      'registerWarpIfSlotFree(',
    );
  });

  it('③-c 四张表都走**同一个**暂存闸门 `editRoute`，没有第二个 if', () => {
    for (const file of ['WgPanel.tsx', 'WarpPanel.tsx', 'TsSettingsPanel.tsx']) {
      const body = strip(read(`./${file}`));
      if (file === 'WarpPanel.tsx') {
        /* WARP 注册/编辑有远端副作用，走 `splitStagedOnly` 的 staged-only 拦截而不是暂存分流
           （逐字同桌面 `WarpDialog#doEdit`）—— 形态不同，但同样是那一份共用判据。 */
        expect(body, 'WARP 编辑没有 staged-only 拦截').toContain('splitStagedOnly(');
        continue;
      }
      expect(body, `${file} 没走共用的暂存闸门`).toContain("editRoute('servers'");
    }
  });
});

/* ════════════════════════════════════════════════════════════════════════════
 * ④ Tailscale 登录：判决门照抄不重写，订阅不重复挂
 * ══════════════════════════════════════════════════════════════════════════ */
describe('④ Tailscale 登录：URL 与登录态都只**读** store，不在面板里重判', () => {
  const panel = strip(read('./TsLoginPanel.tsx'));

  it('④-a 面板不自己订阅 `onTailscaleAuth`（那条在应用级，且它还要开浏览器 + 发通知）', () => {
    expect(panel, '面板又挂了一条 URL 订阅 —— 面板一卸载它就没了，而登录多半发生在别的屏').not.toContain(
      'onTailscaleAuth(',
    );
    expect(panel, '面板没有从 store 读 URL').toContain('tailscaleAuthUrls[');
    // 应用级那条必须还在（它才是真正的写入方；被摘掉的话面板永远等不到地址）。
    expect(strip(read('../app-wiring.ts')), '应用级的 onTailscaleAuth 订阅不见了').toContain(
      'api.proxy.onTailscaleAuth(',
    );
  });

  it('④-b 登录态的判决门只有一处（在 app-wiring），面板里不许出现第二次判决', () => {
    expect(
      panel,
      '面板自己判了一次 definitive 帧 —— 那是给同一个问题造第二个答案（判据在 app-wiring）',
    ).not.toContain('isDefinitiveTsLoginFrame');
    const wiring = strip(read('../app-wiring.ts'));
    expect(wiring, '应用级那条判决门没了 —— 早期 NoState 帧会把「已登录」写穿进 localStorage 缓存').toContain(
      'isDefinitiveTsLoginFrame(data)',
    );
    // 顺序也要对：先整帧落 store，再过门折叠登录态（桌面同一条，照抄不重写）。
    expect(wiring.indexOf('setTailscaleStatus(data)')).toBeLessThan(
      wiring.indexOf('isDefinitiveTsLoginFrame(data)'),
    );
  });

  /**
   * ④-d **本次登录走到哪一步 ≠ 这个节点历史上登录过没有**（2026-09-06 复审 major）。
   *
   * 面板此前在浏览器登录那一支的最前面加了一档 `loggedIn`
   * （`tailscaleLoginStates[pendingServerId] === true` ⇒ 画「已提交，正在连接…」）。那张表是
   * **跨会话持久**的：初值来自 localStorage 缓存，`reset` 时刻意不清。于是对一个此前登录过的
   * Tailscale 节点重新登录（设置表末尾的「切换账号」，`planTsLoginSubmit` 复用既有节点 ⇒ id 不变），
   * `setPendingServerId(server.id)` 那一刻这一档当场命中，登录地址、复制/打开两颗重试键、
   * 超时那一臂**全部不可达**，而且没有任何东西会把它翻回 false。
   *
   * 🔴 上一版的 ④-b 里有一条 `expect(panel).toContain('tailscaleLoginStates[')` —— 它是在**给缺陷
   * 上锁**：删掉那一档反而会红。这里换成成对的两条：判据本体（真值表）× 两端都消费它。
   */
  it('④-d 旧四档判据不看历史登录态；两端显示只等待本次 starting phase', () => {
    /* 真值表。第三列是本次登录的四个状态，逐行对应面板/弹窗画的那一档。 */
    const rows: ReadonlyArray<
      readonly [string, Parameters<typeof tsLoginBrowserView>, ReturnType<typeof tsLoginBrowserView>]
    > = [
      ['还没提交 ⇒ 说明文案', [null, null, false], 'hint'],
      ['提交了、地址还没到 ⇒ 等待', ['ts-1', null, false], 'awaiting'],
      ['地址到了 ⇒ 给地址（含那两颗重试键）', ['ts-1', 'https://login.tailscale.com/a/abc', false], 'url'],
      ['等地址超时 ⇒ 超时臂', ['ts-1', null, true], 'timeout'],
      /* 🔴 回归钉子：超时之后地址仍到达 ⇒ 仍然先给地址（用户还能去授权），
         这正是「地址优先」那一句的可执行形态。 */
      ['超时后地址仍到达 ⇒ 地址优先', ['ts-1', 'https://login.tailscale.com/a/abc', true], 'url'],
    ];
    for (const [what, args, expected] of rows) {
      expect(tsLoginBrowserView(...args), what).toBe(expected);
    }

    /* 新 attempt 协议还会报告 mainCore / cancelled：不能把任一 pending id
       直接翻成“仍在等 URL”，只在 starting phase 画等待。 */
    for (const file of ['./TsLoginPanel.tsx', '../../components/dialogs/TsLoginDialog.tsx']) {
      const body = strip(read(file));
      expect(body, `${file} 没有消费本次 attempt 进度`).toContain('useTailscaleLoginProgressStore');
      expect(body, `${file} 把主核/取消态误画成等待 URL`).toContain("progress?.phase === 'starting'");
      expect(
        body,
        `${file} 仍在读 \`tailscaleLoginStates\` —— 那张表答的是「历史上登录过没有」，` +
          '拿它挡在本次登录的四档前面，重新登录一个既有节点时登录地址与重试键会全部不可达',
      ).not.toContain('tailscaleLoginStates');
    }

    /* 反向对照：这条判据认得出「顺序被写反了」。把地址那一档挪到超时之后 ——
       即上一版缺陷的等价形态 —— 真值表必须判否。 */
    const wrongOrder = (
      pending: string | null,
      url: string | null,
      timedOut: boolean,
    ): ReturnType<typeof tsLoginBrowserView> => {
      if (timedOut) return 'timeout';
      if (url !== null) return 'url';
      return pending !== null ? 'awaiting' : 'hint';
    };
    expect(
      rows.every(([, args, expected]) => wrongOrder(...args) === expected),
      '把「超时」排到「地址」前面之后真值表仍然全绿 —— 它认不出顺序被写反',
    ).toBe(false);
  });

  it('④-c 移动瞬态 URL 等待用共用时限；桌面消费后端 attempt 进度', () => {
    expect(panel, '面板没走共用的提交计划').toContain('planTsLoginSubmit(');
    expect(panel, '面板自己写了一个超时数').not.toMatch(/setTimeout\([^)]*,\s*\d{4,}/);
    expect(panel, '面板没用共用的超时常量').toContain('TS_LOGIN_TIMEOUT_MS');
    expect(strip(read('../../components/dialogs/TsLoginDialog.tsx'))).toContain("progress?.phase === 'timedOut'");
  });
});

/* ════════════════════════════════════════════════════════════════════════════
 * ⑤ **喂参数那一跳**（2026-09-06 三条复审共同点名的形态）
 *
 * 上面每一组都只覆盖到「函数体」为止：规格函数返回什么、判据函数怎么判、面板里出现过哪个名字。
 * 三条复审各自实测的变异全部落在**调用点把什么喂给它**这一跳，而那一跳一条门都没有：
 *  · `MeshJoinPanel` 的 `open(target)` → `open({ kind: 'node' })`：五个组网选择全开成通用节点表，
 *    三张表全废 —— tsc rc=0、全量 vitest 0 红、`report-wiring.sh` 数字一字不变；
 *  · `WarpPanel` 的 `fields={spec}` → `fields={spec.filter(…)}`：WARP 表静默丢掉前置代理与出口
 *    网卡两个控件 —— 九个门 418 条全绿；
 *  · `TsSettingsPanel` 的 `exitNodeOptions(peers, …)` → `(peers.slice(0, 0), …)`：出口设备下拉
 *    永远只剩「无 / 自定义…」—— 七个门 349 条全绿；
 *  · `WarpDialog` / `WarpPanel` 喂给 `planWarpSubmit` 的 `isEdit` → `isEdit: false`：编辑态清空
 *    端点会被放行，`doEdit(ep!)` 里抛 TypeError 被吞成一句「保存失败」，M6 不变量破了而全量绿。
 *
 * 判据走**真 TypeScript AST**（`@/test/ts-compiler`），不是子串：实参是结构，`toContain('open(')`
 * 连一句注释都能满足。每组配一条合成源码的反向对照，证明它认得出「换成字面量/空数组」。
 * ══════════════════════════════════════════════════════════════════════════ */
describe('⑤ 喂参数那一跳：调用点交给函数的就是那条真腿', () => {
  const MESH_JOIN = parse('./MeshJoinPanel.tsx');
  const WARP_PANEL = parse('./WarpPanel.tsx');
  const WARP_DIALOG = parse('../../components/dialogs/WarpDialog.tsx');
  const TS_SETTINGS = parse('./TsSettingsPanel.tsx');
  const WG_PANEL = parse('./WgPanel.tsx');

  const syn = (name: string, text: string): tsc.SourceFile => tsc.parseSourceFile(name, text);

  it('⓪ 谓词自检：AST 工具对正反例都判得对（否则下面每条都是空话）', () => {
    const good = syn(
      'syn-good.tsx',
      "const spec = warpAdvancedSpec(a, b, c);\nconst x = <MobileFields fields={spec} />;\n",
    );
    const bad = syn(
      'syn-bad.tsx',
      "const spec = warpAdvancedSpec(a, b, c);\nconst x = <MobileFields fields={[]} />;\n",
    );
    expect(jsxAttrExprs(good, 'MobileFields', 'fields').map((n) => tsc.isIdentifier(n))).toEqual([
      true,
    ]);
    expect(jsxAttrExprs(bad, 'MobileFields', 'fields').map((n) => tsc.isIdentifier(n))).toEqual([
      false,
    ]);
    expect(jsxAttrExprs(good, 'MobileFields', 'nope'), '不存在的属性应当切不出东西').toEqual([]);
    expect(initCallOf(good, 'spec') !== null, '`const spec = f(…)` 应当认得出来').toBe(true);
    expect(initCallOf(good, 'nope'), '不存在的绑定应当返回 null').toBeNull();

    const calls = syn('syn-call.ts', 'f(a, [], { isEdit }, { isEdit: false });');
    const call = callsTo(calls, 'f')[0]!;
    expect(argIdent(call, 0), '标识符实参应当报出名字').toBe('a');
    expect(argIdent(call, 1), '数组字面量不是标识符').toBeNull();
    expect(objectArgProp(call, 2, 'isEdit')?.kind, '简写属性应当认出来').toBe('shorthand');
    expect(objectArgProp(call, 3, 'isEdit')?.kind, '显式赋值应当认出来').toBe('value');
    expect(objectArgProp(call, 2, 'nope'), '不存在的键应当返回 null').toBeNull();
  });

  /* ── ⑤-a 组网接入面的分流：真夹具驱动生产函数，逐个 id 对结果恰等 ───────────── */
  it('⑤-a WARP按槽位分流，已有多个 Tailscale 时添加入口仍新建', () => {
    const warp = wgNode({
      id: 'warp-0',
      wireguardSettings: {
        privateKey: 'k',
        localAddress: ['172.16.0.2/32'],
        peerPublicKey: 'p',
        warpDevice: { deviceId: 'd', token: 't' },
      },
    });
    const ts = tsNode();

    // WARP 空槽注册，TS 无论已有多少节点都能新建独立 userspace 身份。
    expect(meshJoinFormFor('warp', []), 'WARP 槽空着却落到编辑态').toEqual({ kind: 'warp', edit: false });
    expect(meshJoinFormFor('tailscale', []), 'TS 槽空着却落到设置表').toEqual({ kind: 'ts-login' });
    // WARP 槽位被占：落编辑。TS 添加入口不能任选第一个既有 id。
    expect(meshJoinFormFor('warp', [warp]), '已有 WARP 却仍落注册态 —— 按下去会再烧一台远端设备').toEqual({
      kind: 'warp',
      edit: true,
    });
    expect(meshJoinFormFor('tailscale', [ts, { ...ts, id: 'ts-second' }])).toEqual({ kind: 'ts-login' });
    // 三条隧道落通用节点表的对应协议（`wireguard` 有自己那张表）。
    expect(meshJoinFormFor('wireguard', [])).toEqual({ kind: 'wg' });
    expect(meshJoinFormFor('openconnect', [])).toEqual({ kind: 'node', initialProto: 'openconnect' });
    expect(meshJoinFormFor('openvpn', [])).toEqual({ kind: 'node', initialProto: 'openvpn-client' });
    // 反向对照：登记表以外的 id 落空（`blocked` 那一支的前提）。
    expect(meshJoinFormFor('__nope__', []), '未知 id 竟然也开出了一张表').toBeNull();

    /* 五支与登记表恰等：桌面加第六个选择而这里没跟上，`MESH_JOIN_CHOICES` 那条会先红，
       但「登记了却映射不出来」只有这一条看得见。 */
    for (const choice of MESH_JOIN_CHOICES) {
      if (choice.disposition.kind !== 'ported') continue;
      expect(meshJoinFormFor(choice.id, []), `${choice.id} 登记成 ported 却映射不出任何表`).not.toBeNull();
    }

    /* 渲染面：五个选择都画得出来且都点得动；两个托管服务已接入时说明文案跟着换
       （不换的话，接入过的用户会以为点进去要再建一个）。 */
    useAppStore.setState({ servers: [] });
    mirrorLiveStateIntoSsrSnapshot();
    const fresh = html(<MeshJoinPanel instanceId="i9" />);
    for (const choice of MESH_JOIN_CHOICES) {
      expect(fresh, `接入面少了「${choice.label}」这一项`).toContain(choice.label);
    }
    expect(fresh, '槽位都空着却有选择被置灰').not.toContain('disabled=""');
    expect(fresh, '槽位空着却画成了「已接入」').not.toContain(tr('meshJoin.warpConfigured'));

    useAppStore.setState({ servers: [warp, ts] });
    mirrorLiveStateIntoSsrSnapshot();
    const taken = html(<MeshJoinPanel instanceId="i9" />);
    expect(taken, '已有 WARP 却没换成「已接入」的说明').toContain(tr('meshJoin.warpConfigured'));
    expect(taken, '添加入口不得把新 TS 登录送到任意既有节点设置').toContain(tr('meshJoin.tsNew'));
    expect(taken).not.toContain(tr('meshJoin.tsConfigured'));
  });

  it('⑤-b 接入面的 `onClick` 把 `meshJoinFormFor` 的返回值交给了 `open`（不是一张写死的表）', () => {
    /* 生产源码面：`open(…)` 的实参必须是那个由 `meshJoinFormFor(choice.id, servers)` 绑出来的
       标识符，而不是任何对象字面量。复审实测的变异 `open({ kind: 'node' })` 在这里当场红。 */
    const opens = callsTo(MESH_JOIN, 'open');
    expect(opens.length, '接入面里一处 `open(` 都没有 —— 取材面塌了').toBeGreaterThan(0);
    for (const call of opens) {
      const name = argIdent(call, 0);
      expect(
        name,
        '接入面把一个**字面量**交给了 `open` —— 五个选择会开出同一张表，而账本仍宣称三张表已接线',
      ).not.toBeNull();
      const init = initCallOf(MESH_JOIN, name!);
      expect(init !== null && calleeName(init) === 'meshJoinFormFor', `\`${name}\` 不是 meshJoinFormFor 的产物`).toBe(
        true,
      );
      expect(argIdent(init!, 1), 'meshJoinFormFor 的节点列表实参不是那个 effective 列表').toBe('servers');
    }
    // `servers` 取的是展示面（含暂存节点）——单例分流必须看得见暂存里的那一个。
    const serversInit = initCallOf(MESH_JOIN, 'servers');
    expect(serversInit !== null && calleeName(serversInit) === 'useEffectiveServers', '接入面读的不是 effective 列表').toBe(
      true,
    );

    // 反向对照：合成一份「恒开通用节点表」的源码，同一套判据必须判否。
    const gutted = syn(
      'syn-mesh.tsx',
      "const target = meshJoinFormFor(choice.id, servers);\nconst h = () => open({ kind: 'node' });\n",
    );
    expect(
      callsTo(gutted, 'open').every((c) => argIdent(c, 0) !== null),
      '把 `open(target)` 换成字面量之后判据仍然全绿 —— 它认不出三张表被整片废掉',
    ).toBe(false);
  });

  /* ── ⑤-c WARP：规格 → 渲染器这一跳 ────────────────────────────────────────── */
  it('⑤-c WARP 把 `warpAdvancedSpec` 的产物整份喂给了 `MobileFields`（不是过滤剩几项）', () => {
    const fields = jsxAttrExprs(WARP_PANEL, 'MobileFields', 'fields');
    expect(fields.length, 'WarpPanel 里没有 `<MobileFields fields={…}>` —— 取材面塌了').toBe(1);
    const name = tsc.isIdentifier(fields[0]!) ? fields[0]!.text : null;
    expect(name, 'WARP 喂给渲染器的不是一个绑定 —— 极可能是就地过滤/裁剪过的表').not.toBeNull();
    const init = initCallOf(WARP_PANEL, name!);
    expect(
      init !== null && calleeName(init) === 'warpAdvancedSpec',
      `\`${name}\` 不是 \`warpAdvancedSpec\` 的产物 —— 高级组里那五个控件可能已经不是共用那张表`,
    ).toBe(true);
    // 前置代理候选也要是真算出来的那一份（本仓对上游的有意偏离，另有 endpoint-detour 专门守它）。
    expect(argIdent(init!, 0), 'WARP 的前置代理候选不是 `endpointDetourOptions` 的产物').toBe('detourOpts');
    const detourInit = initCallOf(WARP_PANEL, 'detourOpts');
    expect(detourInit !== null && calleeName(detourInit) === 'endpointDetourOptions', 'detourOpts 不是算出来的').toBe(
      true,
    );

    /* 渲染面（成对交的另一半）：把生产的 `warpAdvancedSpec` 产物喂给生产的 `MobileFields`，
       断言那六个控件真的画得出来。WARP 的「高级」组默认收起（同桌面 `FormSection collapsible`），
       面板整体渲染量不到它，故这一半单独量。 */
    const spec = warpAdvancedSpec(
      endpointDetourOptions([], undefined, tr('node.detourDirect')),
      'ph',
      [['', 'inherit']],
    );
    expect(spec.map((f) => f.k), 'warpAdvancedSpec 的表塌了').toEqual([
      'endpoint',
      'mtu',
      'workers',
      'keepalive',
      'detour',
      'bindInterface',
      /* 按需连接（桌面「多 VPN 兼容」批加进三张组网表的同一格，定义在 `on-demand-field.ts`）。 */
      'onDemand',
    ]);
    const advHtml = html(
      <MobileFields fields={spec} values={warpDraftFromNode(undefined)} onChange={() => {}} t={(k) => i18n.t(k)} />,
    );
    for (const f of spec) {
      expect(advHtml, `WARP 高级组画不出 ${f.k}（label 键 ${f.label}）`).toContain(tr(f.label));
    }
    // 反向对照：空表时画不出这些（证明上面那批量的是 spec 而不是别处的串）。
    const emptyHtml = html(<MobileFields fields={[]} values={{}} onChange={() => {}} t={(k) => i18n.t(k)} />);
    expect(emptyHtml, '空表也画出了控件').not.toContain(tr('warp.endpoint'));

    // 反向对照：合成一份「就地过滤」的源码，判据必须判否。
    const gutted = syn(
      'syn-warp.tsx',
      "const spec = warpAdvancedSpec(a, b, c);\nconst x = <MobileFields fields={spec.filter((f) => f.k !== 'detour')} />;\n",
    );
    expect(
      jsxAttrExprs(gutted, 'MobileFields', 'fields').every((n) => tsc.isIdentifier(n)),
      '把 `fields={spec}` 换成就地过滤之后判据仍然全绿 —— 控件会静默消失',
    ).toBe(false);
  });

  /* ── ⑤-d Tailscale：peers → 出口候选 → 规格 这两跳 ────────────────────────── */
  it('⑤-d TS 设置表把拉回来的 peers 真的喂给了出口候选，再喂给 `tsMainSpec`', () => {
    const main = callsTo(TS_SETTINGS, 'tsMainSpec');
    expect(main.length, 'TsSettingsPanel 里没有 `tsMainSpec(` —— 取材面塌了').toBe(1);
    const optsName = argIdent(main[0]!, 0);
    expect(optsName, 'tsMainSpec 的出口候选实参是个字面量 —— 下拉里永远只剩「无 / 自定义…」').not.toBeNull();
    const optsInit = initCallOf(TS_SETTINGS, optsName!);
    expect(
      optsInit !== null && calleeName(optsInit) === 'exitNodeOptions',
      `\`${optsName}\` 不是 \`exitNodeOptions\` 的产物`,
    ).toBe(true);
    expect(
      argIdent(optsInit!, 0),
      '出口候选不是拿拉回来的 peers 算的 —— 那条 `tailscaleGetStatus` 白拉了',
    ).toBe('peers');
    // 已保存值取的是**已保存**那一份而不是草稿（禁用豁免须在整个面板生命期内稳定）。
    expect(argIdent(optsInit!, 1), '出口候选的已保存值实参变了').toBe('savedExit');

    /* 渲染面：生产的 `exitNodeOptions` 拿一份非空 peers 算出来的候选，经生产的渲染器真的画成了
       `<option>`。effect 在本仓不执行 ⇒ 面板里的 `peers` 恒空，这一半只能这么量。 */
    const peers = [
      { hostName: 'gate-router', ip: '100.64.0.9', online: true, exitNode: false, exitNodeOption: true, active: true },
    ];
    const opts = exitNodeOptions(peers, '', {
      none: tr('ts.exitNone'),
      custom: tr('common.customEllipsis'),
      inUse: tr('ts.exitInUse'),
      offline: tr('ts.exitOffline'),
      notAdvertised: tr('ts.exitNotAdvertised'),
    });
    expect(opts.map((o) => o[0]), '出口候选里没有那台 peer —— 下面那条会在空气上跑').toContain('gate-router');
    const tsHtml = html(
      <MobileFields
        fields={groupTsFields([...tsMainSpec(opts, [], []), ...TS_ADV_SPEC]).basic}
        values={initTsDraft(tsNode())}
        onChange={() => {}}
        t={(k) => i18n.t(k)}
      />,
    );
    expect(tsHtml, 'peer 没有被画成出口候选').toContain('gate-router');
    // 反向对照：空 peers 时画不出它（证明上一条量的是候选而不是别处的串）。
    const emptyTs = html(
      <MobileFields
        fields={groupTsFields([...tsMainSpec(exitNodeOptions([], '', {
          none: tr('ts.exitNone'),
          custom: tr('common.customEllipsis'),
          inUse: tr('ts.exitInUse'),
          offline: tr('ts.exitOffline'),
          notAdvertised: tr('ts.exitNotAdvertised'),
        }), [], []), ...TS_ADV_SPEC]).basic}
        values={initTsDraft(tsNode())}
        onChange={() => {}}
        t={(k) => i18n.t(k)}
      />,
    );
    expect(emptyTs, '空候选里凭空出现了 peer').not.toContain('gate-router');

    // 反向对照：合成一份 `exitNodeOptions(peers.slice(0, 0), …)`，判据必须判否。
    const gutted = syn(
      'syn-ts.tsx',
      'const exitOpts = exitNodeOptions(peers.slice(0, 0), savedExit, labels);\nconst g = tsMainSpec(exitOpts, d, i);\n',
    );
    const guttedInit = initCallOf(gutted, 'exitOpts');
    expect(
      argIdent(guttedInit!, 0),
      '把 peers 换成 `peers.slice(0, 0)` 之后判据仍然全绿 —— 出口下拉会永远空着',
    ).toBeNull();
  });

  /* ── ⑤-e 两端喂给 `planWarpSubmit` 的三个实参 ───────────────────────────── */
  it('⑤-e `planWarpSubmit` 的 `isEdit` / `endpointRaw` 在两端都是真值，不是字面量', () => {
    for (const [what, sf] of [
      ['桌面 WarpDialog', WARP_DIALOG],
      ['移动 WarpPanel', WARP_PANEL],
    ] as const) {
      const calls = callsTo(sf, 'planWarpSubmit');
      expect(calls.length, `${what} 没有调用共用判据 planWarpSubmit`).toBe(1);
      const call = calls[0]!;
      const isEdit = objectArgProp(call, 0, 'isEdit');
      expect(isEdit, `${what} 没有把 isEdit 喂进去`).not.toBeNull();
      /* 简写（`isEdit,`）或引用同名绑定都可以；**字面量不行** —— 喂 `false` 会让编辑态把端点
         清空的提交放行，随后 `ep.host` 抛 TypeError 被吞成一句「保存失败」（M6 不变量破）。 */
      const ok =
        isEdit!.kind === 'shorthand' ||
        (tsc.isIdentifier(isEdit!.node) && isEdit!.node.text === 'isEdit');
      expect(ok, `${what} 喂给 planWarpSubmit 的 isEdit 是个字面量/别的表达式 —— M6 不变量会静默失守`).toBe(
        true,
      );

      const raw = objectArgProp(call, 0, 'endpointRaw');
      expect(raw, `${what} 没有把 endpointRaw 喂进去`).not.toBeNull();
      expect(
        under(raw!.node).some(
          (n) =>
            tsc.isPropertyAccessExpression(n) &&
            tsc.isIdentifier(n.expression) &&
            n.expression.text === 'draft' &&
            tsc.isIdentifier(n.name) &&
            n.name.text === 'endpoint',
        ),
        `${what} 喂给 planWarpSubmit 的端点不是表单里那一格（\`draft.endpoint\`）`,
      ).toBe(true);
    }

    // 反向对照：合成 `isEdit: false`，判据必须判否。
    const gutted = syn('syn-plan.ts', "const p = planWarpSubmit({ name, plan, license, endpointRaw: '', isEdit: false });");
    const g = objectArgProp(callsTo(gutted, 'planWarpSubmit')[0]!, 0, 'isEdit')!;
    expect(
      g.kind === 'shorthand' || (tsc.isIdentifier(g.node) && g.node.text === 'isEdit'),
      '把 `isEdit,` 换成 `isEdit: false` 之后判据仍然全绿',
    ).toBe(false);
  });

  /* ── ⑤-f 两道单例闸喂进去的两个列表 ──────────────────────────────────────── */
  it('⑤-f 两道单例闸吃的是真节点列表（喂空数组 = 恒放行，此前全仓无门）', () => {
    const reg = callsTo(WARP_PANEL, 'registerWarpIfSlotFree');
    expect(reg.length, 'WARP 注册没走「先过闸再打远端」那条腿').toBe(1);
    expect(
      argIdent(reg[0]!, 0),
      'WARP 注册闸吃的不是那个 effective 列表 —— 已有 WARP 时会白烧一台 Cloudflare 孤儿设备',
    ).toBe('servers');

    const blocked = callsTo(WG_PANEL, 'blockedByMeshSingleton');
    expect(blocked.length, 'WgPanel 没过单例闸').toBe(1);
    expect(argIdent(blocked[0]!, 0), '单例闸判的不是这次要造的那个节点').toBe('server');
    expect(
      argIdent(blocked[0]!, 1),
      '单例闸吃的不是那个 effective 列表 —— 粘贴 CF 的 .conf 会造出第二个 WARP，两者抢内核 utun',
    ).toBe('servers');

    // 两个面板的 `servers` 都必须取展示面（含暂存节点）。
    for (const [what, sf] of [
      ['WarpPanel', WARP_PANEL],
      ['WgPanel', WG_PANEL],
    ] as const) {
      const init = initCallOf(sf, 'servers');
      expect(init !== null && calleeName(init) === 'useEffectiveServers', `${what} 的 servers 不是 effective 列表`).toBe(
        true,
      );
    }

    // 反向对照：合成 `([], …)` / `(server, [], …)`，判据必须判否。
    const gutted = syn(
      'syn-guard.ts',
      'const a = registerWarpIfSlotFree([], t, r);\nconst b = blockedByMeshSingleton(server, [], t, id);\n',
    );
    expect(argIdent(callsTo(gutted, 'registerWarpIfSlotFree')[0]!, 0), '喂空数组仍被判为真列表').toBeNull();
    expect(argIdent(callsTo(gutted, 'blockedByMeshSingleton')[0]!, 1), '喂空数组仍被判为真列表').toBeNull();
  });

  /* ── ⑤-g 闸不过那一档的可见性：面板内也有回执，不只押在跨层 toast 上 ────────── */
  it('⑤-g 单例闸被拦时两张表都把理由落进面板内的 `notice`', () => {
    for (const [what, sf, fn] of [
      ['WarpPanel', WARP_PANEL, 'registerWarpIfSlotFree'],
      ['WgPanel', WG_PANEL, 'blockedByMeshSingleton'],
    ] as const) {
      const call = callsTo(sf, fn)[0]!;
      const cb = call.arguments[call.arguments.length - 1]!;
      expect(
        tsc.isArrowFunction(cb) || tsc.isFunctionExpression(cb),
        `${what} 没有给单例闸传回执回调 —— 那一档的唯一反馈是一条跨层 toast`,
      ).toBe(true);
      expect(
        callsTo(cb, 'setNotice').length,
        `${what} 的回执回调里没有 setNotice —— 用户按下主按钮后面板上什么都不会变`,
      ).toBeGreaterThan(0);
    }
    /* 文案仍只有一份：两张表都不许自己再写一句「槽位被占」的文案。 */
    for (const file of ['WarpPanel.tsx', 'WgPanel.tsx']) {
      const body = strip(read(`./${file}`));
      expect(body, `${file} 自己抄了一份单例槽文案 —— 与共用那句必然分叉`).not.toContain(
        'nodes.warpSlotTaken',
      );
      expect(body, `${file} 自己抄了一份单例槽文案`).not.toContain('nodes.tsSlotTaken');
    }
  });

  /* ── ⑤-h 校验失败要**掰开**装着出错字段的那一组 ─────────────────────────────
   *
   * WARP 的端点住在默认收起的「高级」组里。只落一条 notice 的话，用户看到「端点格式应为
   * host:port」却看不到那个输入框 —— 同批另外两张表都做了这件事（`WgPanel#revealGroup`、
   * `TsSettingsPanel` 的两条校验腿），WARP 这条漏了（2026-09-06 复审 minor）。 */
  it('⑤-h WARP 端点被拒时掰开了「高级」组（否则出错的字段在屏幕上根本不在）', () => {
    /** `if (… === '<档>')` 这一档的 then 分支里调了哪些函数。 */
    const rejectArm = (sf: tsc.SourceFile, kind: string): string[] => {
      for (const n of under(sf)) {
        if (!tsc.isIfStatement(n)) continue;
        const hit = under(n.expression).some(
          (x) => tsc.isStringLiteral(x) && x.text === kind,
        );
        if (!hit) continue;
        return under(n.thenStatement)
          .filter((x): x is tsc.CallExpression => tsc.isCallExpression(x))
          .map((c) => calleeName(c) ?? '');
      }
      return [];
    };
    const arm = rejectArm(WARP_PANEL, 'endpoint');
    expect(arm.length, "WarpPanel 里找不到 `reject === 'endpoint'` 那一档 —— 取材面塌了").toBeGreaterThan(0);
    expect(arm, '端点被拒时没有掰开「高级」组 —— 用户看得到错误、看不到那个字段').toContain(
      'revealAdvanced',
    );
    expect(arm, '端点被拒时没有回显错误文案').toContain('setNotice');
    // 反向对照：合成一份只落 notice 的源码，同一套切法必须判否。
    const gutted = syn(
      'syn-reject.ts',
      "if (verdict.reject === 'endpoint') { setNotice({ tone: 'err', text: t('warp.errEndpoint') }); return; }",
    );
    expect(rejectArm(gutted, 'endpoint'), '只落 notice 的形态没被判否').not.toContain('revealAdvanced');
    // 姊妹腿：另外两张表的同类分支照旧在掰组（同根因，一处漏了就该顺手核一遍）。
    expect(strip(read('./WgPanel.tsx')), 'WgPanel 的必填校验不再掰组').toContain("revealGroup('basic')");
    expect(strip(read('./WgPanel.tsx')), 'WgPanel 的 Reserved 校验不再掰组').toContain(
      "revealGroup('advanced')",
    );
    expect(strip(read('./TsSettingsPanel.tsx')), 'TS 设置表的 CIDR 校验不再掰组').toContain("'routing'");
  });

  /* ── ⑤-i 落盘之后不许再问「放弃登录？」 ──────────────────────────────────── */
  it('⑤-i TS 登录腿落盘成功即清脏态（否则关闭时会问一句事实错误的「放弃登录？」）', () => {
    const sf = parse('./TsLoginPanel.tsx');
    const order = under(sf);
    const at = (pred: (n: tsc.Node) => boolean): number => order.findIndex(pred);
    const clearDirty = at(
      (n) =>
        tsc.isCallExpression(n) &&
        calleeName(n) === 'setDirty' &&
        n.arguments[0] !== undefined &&
        n.arguments[0]!.kind === tsc.SyntaxKind.FalseKeyword,
    );
    expect(
      clearDirty,
      '登录腿从不清脏态 —— Auth Key 早已随 add/update 落盘，关闭时却被问「已填写的内容将不会保存」',
    ).toBeGreaterThan(-1);
    const login = at((n) => tsc.isCallExpression(n) && calleeName(n) === 'tailscaleLogin');
    expect(login, '取材面塌了：面板里没有 `tailscaleLogin(`').toBeGreaterThan(-1);
    expect(
      clearDirty,
      '清脏态排在起登录核之后 —— `!res.started` / authKey 两支 return 之前就该清干净了',
    ).toBeLessThan(login);
    /* 关闭路径确实按 `dirty` 分流（否则上面两条量了个不影响任何东西的布尔）。 */
    expect(strip(read('./TsLoginPanel.tsx')), '关闭路径不再看 dirty').toContain('if (!dirty)');
  });

  /* ── ⑤-j 同一块里不许把 label 的键当说明再用一遍 ──────────────────────────── */
  it('⑤-j WgPanel 的「粘贴 .conf」块：标签与说明是两个键（重复标签比没有更坏）', () => {
    const body = strip(read('./WgPanel.tsx'));
    expect(
      (body.match(/wg\.pasteLabel/g) ?? []).length,
      '`wg.pasteLabel` 在 WgPanel 里出现了不止一次 —— 渲染出来是同一句话重复两遍',
    ).toBe(1);
    expect(body, '粘贴块没有说明文案').toContain('wg.pasteHint');
    // 渲染面：两句真的都画出来了，且不是同一句。
    const out = html(<WgPanel instanceId="i1" />);
    expect(out, '默认停在手填页，粘贴块不该渲染').not.toContain(tr('wg.pasteHint'));
    expect(tr('wg.pasteHint'), '说明与标签是同一句译文 —— 那等于没改').not.toBe(tr('wg.pasteLabel'));
  });
});
