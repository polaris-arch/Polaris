/**
 * 🔴 **声明的采样面 == 实际采样面**（2026-09-05 第六轮 B1）。
 *
 * # 这条门解决什么
 *
 * 第六轮验收给出一条**组合爆炸型**的缺陷：裁判在条件空间里**只采样一个点**（一个视口、一个 `dir`、
 * 一个主题），而静态模型按定义只看无条件档 —— 两者中间是一整块**没人看的面**。实测三条全绿：
 * `@media (max-width:380px)`、`dir=rtl`、`data-*` 档。
 *
 * 视口 × 方向 × 主题 × `data-*` × `prefers-*` 是组合面，**追它没有终点**。所以本轮的口径不是「把它做全」，
 * 而是把「本裁判在哪几个点上采过样」写成一张表（`css-oracle.test-support.ts` 的 [`SAMPLED_FACE`]），
 * 两个方向各有一段自检。
 *
 * 🔴 **这两段的强度不一样，先说清楚再往下读**（2026-09-05 第七轮 T1 / T2）：
 *
 *  · **实际 ⊆ 声明**：`measureOnce` 在发命令之前过 `assertPointDeclared`。这是一条**运行期自检**，
 *    **它自己没有判据**：本文件「有牙」那条测的是那个**函数**，而「`measureOnce` 真的调用了它」
 *    这条接线一条判据都没有 —— 本轮实测把生产路径上那一行注掉，全量 239 文件 / 4019 条照样 rc=0 全绿。
 *  · **声明 ⊆ 实际**：本文件。判的是**调用点源码里有没有那个字面量**，**不是运行期有没有真采过**：
 *    从源码里把 `viewport:` / `media:` / `rootAttrs:` / `bodyAttrs:` 后面的对象字面量提出来与表对拍。
 *    采样者被 `skip` 掉、或那条 `measure(...)` 之后的断言被删掉，只要这几行字面量还在，本门照旧全绿。
 *
 * 所以这里**不写**「两边合起来这句话就不再是没人管的话」—— 上一版就是这么写的，
 * 而两扇门之间的那道缝正是生产路径。两条收窄都进了裁判模块头注的「抓不到的形态」表。
 *
 * 🔴 [`SAMPLED_FACE`] 按**轴**声明，不按（链 × 轴）：轴上有 `dark` 不等于**每条链**都采过 `dark`。
 * 第七轮 T6 抓到的正是这一条：桌面那条链一次都没采过 `prefers-color-scheme: dark`，
 * 而按轴读起来像是覆盖了。处置是给桌面门补了一条 dark 腿，并在本文件加一条判据钉住它还在
 * （见「（链 × 轴）：桌面链的 dark 腿还在」）。**其余（链 × 轴）格子仍然没有判据。**
 *
 * # 取材面（这条判据自己的射程）
 *
 * 调用点 = `src/**` 里**同时**满足两条的文件：引用了 `css-oracle.test-support`，且真的调用了度量。
 * 本文件被显式排除（见 [`SELF`]）：它自己带着一批**合成夹具**，那些夹具里的取值不是真采样面。
 * 排除是一句写出来的话，并且下面有一条判据钉住「本文件确实不是调用点」。
 *
 * 提取器的两个已知收窄，方向都是**偏红**（漏提 ⇒ 声明里那一档找不到 ⇒ 红），不会偏绿：
 *  · 只认 `viewport:` / `media:` / `rootAttrs:` / `bodyAttrs:` 四个**键名后紧跟对象字面量**的写法；
 *    把取值绕一层函数参数再传进去（`at({ 'prefers-color-scheme': 'dark' })`）提不出来；
 *  · 标识符只按两种写法回溯：同文件里的 `for (const X of ['a','b'] as const)` 与形参标注
 *    `(X: 'a' | 'b')`。两种都对不上 ⇒ **抛**，不是静默跳过。
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve as resolvePath } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  DEFAULT_VIEWPORT,
  MEDIA_DEFAULTS,
  SAMPLED_FACE,
  assertPointDeclared,
  hostSkeleton,
  viewportKey,
} from './css-oracle.test-support';
import type { CtxName } from './css-cascade.test-support';

const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
const SRC = resolvePath(UI_ROOT, 'src');
const rel = (absPath: string) => relative(UI_ROOT, absPath).split('\\').join('/');

/** 本文件自己 —— 带着合成夹具，不是调用点。排除有判据看着（见「取材面自检」那条）。 */
const SELF = 'src/styles/css-oracle-sampling.test.ts';
/** 裁判模块自己 —— 它的默认取值由 `DEFAULT_VIEWPORT` / `MEDIA_DEFAULTS` 直接读，不走源码提取。 */
const ORACLE = 'src/styles/css-oracle.test-support.ts';

// ════════════════════════════════════════════════════════════════════════════
// ① 扫描器：注释抹平、字符串标位（**保长度**，位置即索引）
// ════════════════════════════════════════════════════════════════════════════

interface Scanned {
  /** 与原文等长：注释字符换成空格，其余原样。 */
  text: string;
  /** 与原文等长：该位置是否在字符串 / 模板字面量**内部**（含定界符）。 */
  inStr: Uint8Array;
}

/**
 * 一遍扫出「哪些位置是注释」「哪些位置在字符串里」。
 *
 * 为什么不用 `css-cascade.test-support` 的 `stripComments`：它只剥 `/* *\/`，不剥 `//`，
 * 也不认模板字面量 —— 而这份判据的取材面是 **TS 源码**，两样都必须认，
 * 否则注释里/字符串里的一句引文就会被当成真的采样点（判据被自己的取材面污染）。
 */
export function scanSource(src: string): Scanned {
  const out = new Array<string>(src.length);
  const inStr = new Uint8Array(src.length);
  let i = 0;
  const blank = (from: number, to: number): void => {
    for (let k = from; k < to; k += 1) out[k] = src[k] === '\n' ? '\n' : ' ';
  };
  while (i < src.length) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      blank(i, stop);
      i = stop;
      continue;
    }
    if (c === '/' && src[i + 1] === '/') {
      let stop = src.indexOf('\n', i);
      if (stop === -1) stop = src.length;
      blank(i, stop);
      i = stop;
      continue;
    }
    if (c === "'" || c === '"' || c === '`') {
      let j = i + 1;
      while (j < src.length) {
        if (src[j] === '\\') {
          j += 2;
          continue;
        }
        if (src[j] === c) {
          j += 1;
          break;
        }
        if (c !== '`' && src[j] === '\n') break;
        j += 1;
      }
      for (let k = i; k < j; k += 1) {
        out[k] = src[k];
        inStr[k] = 1;
      }
      i = j;
      continue;
    }
    out[i] = c;
    i += 1;
  }
  for (let k = 0; k < src.length; k += 1) out[k] ??= src[k];
  return { text: out.join(''), inStr };
}

/** 代码区（不在注释、不在字符串里）出现过这段字面文本吗。 */
function hasCodeToken(sc: Scanned, token: string): boolean {
  let at = sc.text.indexOf(token);
  while (at >= 0) {
    if (sc.inStr[at] === 0) return true;
    at = sc.text.indexOf(token, at + 1);
  }
  return false;
}

// ════════════════════════════════════════════════════════════════════════════
// ② 对象字面量切片 + 取值解析
// ════════════════════════════════════════════════════════════════════════════

/** `key: { … }` 的**内容**（不含两端花括号），按出现序。花括号配平时跳过字符串里的括号。 */
export function objectLiterals(sc: Scanned, key: string): string[] {
  const out: string[] = [];
  const head = new RegExp(`(?:^|[^\\w$.])${key}\\s*:\\s*\\{`, 'g');
  let m: RegExpExecArray | null;
  while ((m = head.exec(sc.text)) !== null) {
    const open = m.index + m[0].length - 1;
    if (sc.inStr[open] === 1) continue;
    let depth = 0;
    let end = -1;
    for (let k = open; k < sc.text.length; k += 1) {
      if (sc.inStr[k] === 1) continue;
      if (sc.text[k] === '{') depth += 1;
      else if (sc.text[k] === '}') {
        depth -= 1;
        if (depth === 0) {
          end = k;
          break;
        }
      }
    }
    if (end === -1) throw new Error(`\`${key}: {\` 的花括号没配平 —— 切片器读不下去，别往下推。`);
    out.push(sc.text.slice(open + 1, end));
    head.lastIndex = end;
  }
  return out;
}

/** 按**顶层**逗号切（括号里、字符串里的逗号不切）。 */
function topLevelParts(body: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let quote = '';
  let buf = '';
  for (let i = 0; i < body.length; i += 1) {
    const c = body[i];
    if (quote !== '') {
      buf += c;
      if (c === '\\') {
        buf += body[i + 1] ?? '';
        i += 1;
      } else if (c === quote) quote = '';
      continue;
    }
    if (c === "'" || c === '"' || c === '`') {
      quote = c;
      buf += c;
      continue;
    }
    if ('{[('.includes(c)) depth += 1;
    if ('}])'.includes(c)) depth -= 1;
    if (c === ',' && depth === 0) {
      parts.push(buf);
      buf = '';
      continue;
    }
    buf += c;
  }
  parts.push(buf);
  return parts.map((p) => p.trim()).filter((p) => p !== '');
}

/** 同文件里能把一个标识符解析成哪几个字面量。两种写法，别的一律抛。 */
export function identifierDomain(sc: Scanned, name: string): string[] {
  const found = new Set<string>();
  const literals = (blob: string): string[] => [...blob.matchAll(/'([^']*)'|"([^"]*)"/g)].map((m) => m[1] ?? m[2]);
  for (const m of sc.text.matchAll(new RegExp(`for\\s*\\(\\s*const\\s+${name}\\s+of\\s*\\[([^\\]]*)\\]`, 'g')))
    for (const v of literals(m[1])) found.add(v);
  for (const m of sc.text.matchAll(new RegExp(`[(,]\\s*${name}\\s*:\\s*((?:\\s*'[^']*'\\s*\\|)*\\s*'[^']*')`, 'g')))
    for (const v of literals(m[1])) found.add(v);
  return [...found].sort();
}

/** 一个属性名 → 它在这个文件里可能取到的全部字面量。解析不出来 ⇒ 抛。 */
function propValues(sc: Scanned, file: string, key: string, body: string): { name: string; values: string[] }[] {
  const out: { name: string; values: string[] }[] = [];
  for (const part of topLevelParts(body)) {
    const at = part.indexOf(':');
    if (at < 0) throw new Error(`${file} 的 \`${key}\` 里有一项不是 \`名: 值\`：\`${part}\``);
    const rawName = part.slice(0, at).trim();
    const name = /^['"]/.test(rawName) ? rawName.slice(1, -1) : rawName;
    const raw = part.slice(at + 1).trim();
    const str = /^'([^']*)'$/.exec(raw) ?? /^"([^"]*)"$/.exec(raw);
    if (str !== null) {
      out.push({ name, values: [str[1]] });
      continue;
    }
    if (/^-?\d+(?:\.\d+)?$/.test(raw)) {
      out.push({ name, values: [raw] });
      continue;
    }
    if (/^[A-Za-z_$][\w$]*$/.test(raw)) {
      const domain = identifierDomain(sc, raw);
      if (domain.length === 0)
        throw new Error(
          `${file} 的 \`${key}.${name}\` 传的是标识符 \`${raw}\`，而本文件里既没有 ` +
            `\`for (const ${raw} of [...] as const)\` 也没有形参标注 \`${raw}: 'a' | 'b'\` —— ` +
            '取值集提不出来。静默跳过它就等于让声明的采样面对这一档说不出话（第六轮 B1）。',
        );
      out.push({ name, values: domain });
      continue;
    }
    throw new Error(`${file} 的 \`${key}.${name}\` 的取值 \`${raw}\` 不是字面量也不是标识符 —— 本判据不猜。`);
  }
  return out;
}

// ════════════════════════════════════════════════════════════════════════════
// ③ 实际采样面
// ════════════════════════════════════════════════════════════════════════════

export interface Face {
  viewport: string[];
  media: Record<string, string[]>;
  rootAttrs: Record<string, string[]>;
  bodyAttrs: Record<string, string[]>;
}

const emptyFace = (): Face => ({ viewport: [], media: {}, rootAttrs: {}, bodyAttrs: {} });
const add = (bag: Record<string, string[]>, name: string, values: readonly string[]): void => {
  const seen = (bag[name] ??= []);
  for (const v of values) if (!seen.includes(v)) seen.push(v);
};
const normalise = (f: Face): Face => ({
  viewport: [...new Set(f.viewport)].sort(),
  media: Object.fromEntries(Object.entries(f.media).map(([k, v]) => [k, [...new Set(v)].sort()])),
  rootAttrs: Object.fromEntries(Object.entries(f.rootAttrs).map(([k, v]) => [k, [...new Set(v)].sort()])),
  bodyAttrs: Object.fromEntries(Object.entries(f.bodyAttrs).map(([k, v]) => [k, [...new Set(v)].sort()])),
});

/** 从**一个文件**的源码里提取采样点，并报出它用到了哪几个层叠上下文。 */
export function faceOf(src: string, file: string): { face: Face; contexts: string[] } {
  const sc = scanSource(src);
  const face = emptyFace();
  for (const body of objectLiterals(sc, 'viewport')) {
    const props = new Map(propValues(sc, file, 'viewport', body).map((p) => [p.name, p.values]));
    const w = props.get('width');
    const h = props.get('height');
    if (w === undefined || h === undefined)
      throw new Error(`${file} 的 \`viewport\` 里没有 width/height —— 采样点算不出来。`);
    for (const ww of w)
      for (const hh of h)
        for (const dd of props.get('dpr') ?? [String(DEFAULT_VIEWPORT.dpr)])
          face.viewport.push(`${ww}x${hh}@${dd}`);
  }
  for (const [key, bag] of [
    ['media', face.media],
    ['rootAttrs', face.rootAttrs],
    ['bodyAttrs', face.bodyAttrs],
  ] as const)
    for (const body of objectLiterals(sc, key))
      for (const p of propValues(sc, file, key, body)) add(bag, p.name, p.values);
  const contexts = [
    ...new Set(
      [...sc.text.matchAll(/(?:^|[^\w$.])ctx\s*:\s*'(\w+)'/g)]
        .filter((m) => sc.inStr[m.index + m[0].length - 2] === 1)
        .map((m) => m[1]),
    ),
  ];
  return { face, contexts };
}

/** `src/**` 下的全部 `.ts` / `.tsx`。 */
function sourceFiles(dir = SRC): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) out.push(...sourceFiles(full));
    else if (/\.tsx?$/.test(entry)) out.push(full);
  }
  return out.sort();
}

/** 调用点：引用了裁判模块、且真的调用了度量。 */
function callSites(): { file: string; src: string }[] {
  const out: { file: string; src: string }[] = [];
  for (const abs of sourceFiles()) {
    const file = rel(abs);
    if (file === SELF || file === ORACLE) continue;
    const src = readFileSync(abs, 'utf8');
    const sc = scanSource(src);
    if (!hasCodeToken(sc, 'css-oracle.test-support') && !src.includes('css-oracle.test-support')) continue;
    if (!hasCodeToken(sc, 'measure(')) continue;
    out.push({ file, src });
  }
  return out;
}

/** 全部调用点 + 裁判自己的默认取值 + 宿主 `.html` 的 `<html>`/`<body>` 属性。 */
function actualFace(): { face: Face; files: string[]; contexts: string[] } {
  const face = emptyFace();
  const files: string[] = [];
  const contexts = new Set<string>();
  // 裁判自己的默认档：不给 `viewport` / `media` 时**实际发出去**的就是这两张表。
  face.viewport.push(viewportKey(DEFAULT_VIEWPORT));
  for (const [name, value] of Object.entries(MEDIA_DEFAULTS)) add(face.media, name, [value]);
  for (const { file, src } of callSites()) {
    files.push(file);
    const one = faceOf(src, file);
    face.viewport.push(...one.face.viewport);
    for (const [n, v] of Object.entries(one.face.media)) add(face.media, n, v);
    for (const [n, v] of Object.entries(one.face.rootAttrs)) add(face.rootAttrs, n, v);
    for (const [n, v] of Object.entries(one.face.bodyAttrs)) add(face.bodyAttrs, n, v);
    for (const c of one.contexts) contexts.add(c);
  }
  // 宿主段的 `<html lang dir>` / `<body>` 属性来自产品那份 `.html`，不是调用点写的。
  for (const ctx of contexts) {
    const skel = hostSkeleton(ctx as CtxName);
    for (const [n, v] of Object.entries(skel.rootAttrs)) add(face.rootAttrs, n, [v]);
    for (const [n, v] of Object.entries(skel.bodyAttrs)) add(face.bodyAttrs, n, [v]);
  }
  return { face: normalise(face), files, contexts: [...contexts].sort() };
}

// ════════════════════════════════════════════════════════════════════════════
// ④ 判据
// ════════════════════════════════════════════════════════════════════════════

const declaredFace = (): Face =>
  normalise({
    viewport: [...SAMPLED_FACE.viewport],
    media: Object.fromEntries(Object.entries(SAMPLED_FACE.media).map(([k, v]) => [k, [...v]])),
    rootAttrs: Object.fromEntries(Object.entries(SAMPLED_FACE.rootAttrs).map(([k, v]) => [k, [...v]])),
    bodyAttrs: Object.fromEntries(Object.entries(SAMPLED_FACE.bodyAttrs).map(([k, v]) => [k, [...v]])),
  });

describe('声明的采样面 == 实际采样面', () => {
  const actual = actualFace();

  it('取材面自检：调用点找得到，五道生产门一个不少，且每条轴都提到了东西', () => {
    for (const gate of [
      'src/components/layout/window-drag-region.test.ts',
      'src/mobile/connections/connections-screen.test.tsx',
      'src/mobile/home/home-screen.test.tsx',
      'src/mobile/nodes/nodes-screen.test.tsx',
      'src/mobile/screens/rules/rules-screen.test.tsx',
    ])
      expect(actual.files, `${gate} 没被当成调用点 —— 它的采样点整段没进对拍`).toContain(gate);
    expect(actual.files.length, '调用点扫出来太少，这条判据在空跑').toBeGreaterThanOrEqual(6);
    expect(actual.contexts, '一个层叠上下文都没提到 ⇒ 宿主段的 lang/dir 整段缺席').not.toEqual([]);
    expect(actual.face.viewport.length, '视口一个都没提出来').toBeGreaterThan(1);
    expect(Object.keys(actual.face.media).length, '媒体特性一个都没提出来').toBeGreaterThan(0);
    expect(Object.keys(actual.face.rootAttrs).length, '`<html>` 属性一个都没提出来').toBeGreaterThan(0);
  });

  it('本文件不是调用点 —— 排除它是一句写出来的话，不是巧合', () => {
    const self = readFileSync(resolvePath(UI_ROOT, SELF), 'utf8');
    expect(
      hasCodeToken(scanSource(self), 'measure('),
      '本文件开始调用度量了 —— 它就成了调用点，而它带着合成夹具，' +
        '要么把夹具挪走，要么把本文件写进调用点集合',
    ).toBe(false);
  });

  it('视口 / 媒体特性 / `<html>` 属性 / `<body>` 属性四条轴逐值相等', () => {
    const declared = declaredFace();
    expect(actual.face.viewport, '声明的视口取值集与代码里实际传的对不上').toEqual(declared.viewport);
    expect(actual.face.media, '声明的媒体特性取值集与代码里实际传的对不上').toEqual(declared.media);
    expect(actual.face.rootAttrs, '声明的 `<html>` 属性取值集与代码里实际传的对不上').toEqual(
      declared.rootAttrs,
    );
    expect(actual.face.bodyAttrs, '声明的 `<body>` 属性取值集与代码里实际传的对不上').toEqual(
      declared.bodyAttrs,
    );
  });

  /**
   * 🔴 第七轮 T6：`SAMPLED_FACE` 按**轴**声明，答不出「哪条链采了哪个取值」。
   *
   * 实测出来的那一格：桌面那条链此前一次都没采过 `prefers-color-scheme: dark`
   * （`window-drag-region.test.ts` 问的全是几何，`assertColorAsksArePinned` 不响，
   * `media` 落在 `MEDIA_DEFAULTS` 的 light 上），而轴上那个 `dark` 来自移动端的调用点 ——
   * 于是「轴上有 dark」被读成「dark 档有人看着」。处置是给桌面门补了一条 dark 腿（那条腿
   * 真的量了两次），本条把「那条腿还在」钉住。
   *
   * 🔴 **本条读的是那个文件的源码**：它保证那几行取值字面量还在，
   * 保证不了那条 `it` 真的跑过（`skip` 掉它本条照样绿）—— 与本文件另一半是同一个收窄，见头注。
   * 🔴 **其余（链 × 轴）格子仍然没有判据**，本条只点名这一格。
   */
  it('（链 × 轴）：桌面链的 dark 腿还在 —— 按轴声明说不出的那一格，这里点名钉住', () => {
    const DESKTOP_GATE = 'src/components/layout/window-drag-region.test.ts';
    expect(actual.files, `${DESKTOP_GATE} 不在调用点集合里 ⇒ 下面两条恒真`).toContain(DESKTOP_GATE);
    const { face, contexts } = faceOf(readFileSync(resolvePath(UI_ROOT, DESKTOP_GATE), 'utf8'), DESKTOP_GATE);
    expect(contexts, '这个文件量的不再是桌面链 —— 本条钉的那一格已经换了对象').toEqual(['desktop']);
    expect(
      face.media['prefers-color-scheme'] ?? [],
      '桌面链上一次都没采过 `prefers-color-scheme: dark`：`SAMPLED_FACE` 按轴声明，' +
        '轴上那个 dark 来自移动端的调用点，读起来却像桌面这一档也有人看着（第七轮 T6）',
    ).toContain('dark');
    // 正向对照：提取器不是「什么都提得出来」—— 同一条读法在一份没有 dark 腿的合成输入上提不到它。
    expect(
      faceOf("paint({ ctx: 'desktop', media: { 'prefers-color-scheme': 'light' } });", '<fixture>').face
        .media['prefers-color-scheme'],
    ).toEqual(['light']);
  });

  it('实际 ⊆ 声明那一半有牙：采样面之外的点当场抛', () => {
    const inside = {
      viewport: { width: 390, height: 844, dpr: 1 },
      media: { 'prefers-color-scheme': 'light' },
      rootAttrs: { 'data-theme': 'dark' },
      bodyAttrs: {},
    };
    expect(() => assertPointDeclared(inside)).not.toThrow();
    expect(() => assertPointDeclared({ ...inside, viewport: { width: 380, height: 844, dpr: 1 } })).toThrow(
      /380x844@1/,
    );
    expect(() => assertPointDeclared({ ...inside, rootAttrs: { dir: 'rtl' } })).toThrow(/rtl/);
    expect(() => assertPointDeclared({ ...inside, rootAttrs: { 'data-density': 'compact' } })).toThrow(
      /整条轴都没声明/,
    );
    expect(() => assertPointDeclared({ ...inside, media: { 'prefers-contrast': 'more' } })).toThrow(
      /整条轴都没声明/,
    );
  });

  describe('提取器自己有门', () => {
    /** 合成夹具**故意**带上真实写法，验的是提取器，不是产品。 */
    const FIXTURE = [
      'const注释 = 1;',
      '/* viewport: { width: 111, height: 222 } */',
      '// rootAttrs: { "data-theme": "sepia" }',
      'const s = `media: { "prefers-contrast": "more" }`;',
      "const t = 'viewport: { width: 333, height: 444 }';",
      "for (const th of ['dark', 'light'] as const) {",
      "  paint({ viewport: { width: 390, height: 844 }, rootAttrs: { 'data-theme': th }, ctx: 'mobile' });",
      '}',
      "const q = (os: 'win' | 'mac') => ({ rootAttrs: { 'data-os': os }, media: { 'forced-colors': 'none' } });",
    ].join('\n');

    it('正向对照：真写法提得出来，标识符按 `for…of` 与形参标注回溯', () => {
      const { face, contexts } = faceOf(FIXTURE, '<fixture>');
      expect(face.viewport).toEqual(['390x844@1']);
      expect(normalise(face).rootAttrs).toEqual({ 'data-theme': ['dark', 'light'], 'data-os': ['mac', 'win'] });
      expect(face.media).toEqual({ 'forced-colors': ['none'] });
      expect(contexts).toEqual(['mobile']);
    });

    it('反向对照：注释里、字符串里、模板里的那几句一个都不算数', () => {
      const { face } = faceOf(FIXTURE, '<fixture>');
      expect(face.viewport, '注释/字符串里的视口被当成真采样点了').not.toContain('111x222@1');
      expect(face.viewport).not.toContain('333x444@1');
      expect(face.rootAttrs['data-theme'] ?? [], '注释里的取值被算进来了').not.toContain('sepia');
      expect(Object.keys(face.media), '模板字面量里的那句被算进来了').not.toContain('prefers-contrast');
    });

    it('反向对照：标识符回溯不出来 ⇒ 抛（不是静默跳过）', () => {
      expect(() => faceOf("paint({ rootAttrs: { 'data-theme': whoKnows } });", '<fixture>')).toThrow(
        /取值集提不出来/,
      );
      expect(() => faceOf("paint({ viewport: { width: w(), height: 2 } });", '<fixture>')).toThrow(
        /不是字面量也不是标识符/,
      );
    });

    it('正向对照：提取器不是「什么都提不出来」—— 新加一档它立刻报出来', () => {
      const { face } = faceOf("paint({ viewport: { width: 320, height: 568, dpr: 2 } });", '<fixture>');
      expect(face.viewport).toEqual(['320x568@2']);
    });
  });
});
