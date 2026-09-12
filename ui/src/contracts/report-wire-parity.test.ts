/**
 * 两份只读报告契约的**跨语言对拍门**：
 * `tunnel-conflict-report.ts` / `endpoint-force-route-report.ts` ↔ 它们在 Rust 侧的线格式。
 *
 * # 这道门守的是什么
 *
 * 两份 TS 契约的字段是按 Rust serde 属性**逐条手工对出来**的，此前没有任何门。于是后端改一个
 * 字段名，两个编译器都不会说话 —— 前端读到的是 `undefined`，而 `undefined` 是**假值**：
 *
 *  - `TunnelConflictSnapshot` 的 `status` 一旦对不上，四态判别联合会整体落空 ⇒ 「没探成」
 *    （macOS / Windows 根本没有探测实现）被渲染成一句自信的「无冲突」；
 *  - `zeroCoverageServerIds` 一旦对不上，它会被读成空数组 ⇒ 「某个节点活着、engaged、
 *    流量一条都收不到」这件事在界面上彻底消失。
 *
 * 这正是本轮最要防的两种误读，故判据是**集合相等**（两个方向都说话），不是点名几个字段：
 * 点名清单由夹具定覆盖面，新加一个字段两边都不会红。
 *
 * # 判据从 Rust **源码**解析，不写第二份清单
 *
 * 照抄本仓既有范式（`mesh-predicates-parity.test.ts` / `update-popup-state-parity.test.ts` /
 * `warp-veto-parity.test.ts`）：把 Rust 源码当真值读进来。抄一份镜像常量只是把漂移面往后挪一格 ——
 * 「两处清单要一致」会变成「三处清单要一致」。
 *
 * # 读回形态 ≠ 写入形态（本门的全部技术含量都在这一条）
 *
 * Rust 侧的字段名不是线上的名字，中间隔着 serde：
 *
 *  - **结构体**：`#[serde(rename_all = "camelCase")]` 把 `by_server_id` 变成 `byServerId`；
 *    **没有**这行属性的结构体（`ForeignTunnelRoute` / `TunnelConflict`）字段原样上线 ——
 *    两种规则同时存在于本门的取材面上，一律按 camelCase 处理会把后两者对错。
 *  - **枚举**：同一个 `rename_all` 作用在**变体**上是另一条规则（只小写首字母），
 *    `FakeIpOverlap` → `fakeIpOverlap`、`AbsorbedEmpty` → `absorbedEmpty`。
 *  - **`TunnelConflictSnapshot` 根本不走 serde**：它的线格式是 `to_wire` 里手写的 `json!`，
 *    四支各写各的键。故那一半必须解析 `json!` 字面量，用结构体规则去套只会解析出空集。
 *
 * 而 `to_wire` 的**缺键**本身就是判据的一部分：只有 `probed` 那一支带 `conflicts`，
 * 其余三支缺键 —— 带一个空数组的话，渲染端最自然的 `conflicts.length === 0` 会把「没探成」
 * 读成「无冲突」。故本门逐支对键集，而不是对一个并集。
 *
 * # 自曝纪律
 *
 * 任何一处解析不出内容一律 **throw**，不走「读不到就跳过」—— 那样任一侧改名门就静默消失，
 * 「没检查」与「检查通过」的输出不可区分 = 没有这道门。解析器认不出的 serde 属性 / 带载荷的
 * 枚举变体同样当场抛：本门的射程如实登记为「unit 变体 + `rename_all="camelCase"` 或无规则」，
 * 超出这个面时必须有人来改门，而不是让它按错误的规则静默给出一个「相等」。
 */
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import * as ts from '@/test/ts-compiler';

import { maskRustComments, moduleSource } from './rust-source.test-support';

/* ══════════════════════════════════════════════════════════════════════════
 * 取材面
 * ══════════════════════════════════════════════════════════════════════════ */

/**
 * 三个 Rust 取材面。全部走 `moduleSource`（**模块路径**，不是文件路径）：
 * 模块源码天然分布在 `foo.rs` 与 `foo/` 两处，写死一个文件路径会静默失去另一半。
 * 注释掩成空格、**字符串保留** —— 本门的针有一半就是字符串字面量
 * （`rename = "…"` 与 `json!` 的键名），连字符串一起抹会让判据消失而不是变弱。
 */
const ENDPOINT_RS = maskRustComments(moduleSource('crates/config-engine/src/builder/endpoint_routes'));
const TUNNEL_LIB_RS = maskRustComments(moduleSource('crates/config-engine/src/builder/tunnel_conflict'));
const TUNNEL_RT_RS = maskRustComments(moduleSource('src-tauri/src/runtime/proxy/tunnel_conflict'));

const tsFile = (rel: string) => {
  const path = fileURLToPath(new URL(rel, import.meta.url));
  return ts.parseSourceFile(path, readFileSync(path, 'utf8'));
};
const FORCE_ROUTE_TS = tsFile('./endpoint-force-route-report.ts');
const TUNNEL_TS = tsFile('./tunnel-conflict-report.ts');

/* ══════════════════════════════════════════════════════════════════════════
 * Rust 侧：serde 线名解析
 * ══════════════════════════════════════════════════════════════════════════ */

/** 本门支持的 `rename_all` 规则。认不出的一律抛 —— 见文件头「自曝纪律」。 */
type RenameRule = 'camelCase' | null;

/** `pub struct|enum <name>` 的**属性块**与**大括号体**。命中数必须恰好 1。 */
function declaration(src: string, kind: 'struct' | 'enum', name: string): {
  rule: RenameRule;
  body: string;
} {
  const anchor = new RegExp(`\\bpub ${kind} ${name}\\b`, 'g');
  const hits = [...src.matchAll(anchor)];
  if (hits.length !== 1) {
    throw new Error(
      `[report-wire-parity] Rust 取材面上 \`pub ${kind} ${name}\` 命中 ${hits.length} 次（应为 1）` +
        ' —— 0 = 改名/搬走，本门已失去判据；>1 = 判据指向哪一处全凭书写顺序。',
    );
  }
  const at = hits[0].index;

  // 属性块：紧贴声明之上的连续 `#[...]` 行。文档注释已被掩成空白行，正好是这段的天然上界。
  const lines = src.slice(0, at).split('\n');
  const attrs: string[] = [];
  for (let i = lines.length - 2; i >= 0; i -= 1) {
    const line = lines[i].trim();
    if (!line.startsWith('#[')) break;
    attrs.unshift(line);
  }
  let rule: RenameRule = null;
  for (const attr of attrs) {
    const renameAll = /#\[serde\(rename_all\s*=\s*"([^"]+)"\)\]/.exec(attr);
    if (!renameAll) continue;
    if (renameAll[1] !== 'camelCase') {
      throw new Error(
        `[report-wire-parity] ${name} 用了本门不认识的 rename_all="${renameAll[1]}" —— ` +
          '射程之外，必须有人来改门，而不是让它按 camelCase 静默对错。',
      );
    }
    rule = 'camelCase';
  }

  // 大括号体：从声明后的第一个 `{` 起按配平取到匹配的 `}`。
  const open = src.indexOf('{', at);
  if (open < 0) throw new Error(`[report-wire-parity] ${name} 找不到左大括号`);
  let depth = 0;
  for (let i = open; i < src.length; i += 1) {
    if (src[i] === '{') depth += 1;
    else if (src[i] === '}') {
      depth -= 1;
      if (depth === 0) return { rule, body: src.slice(open + 1, i) };
    }
  }
  throw new Error(`[report-wire-parity] ${name} 的大括号不配平`);
}

/** serde `camelCase` 作用于**字段**（snake_case → camelCase）。 */
const camelField = (ident: string): string =>
  ident.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

/** serde `camelCase` 作用于**变体**（PascalCase → 仅小写首字母）。 */
const camelVariant = (ident: string): string => ident[0].toLowerCase() + ident.slice(1);

/** Rust 结构体的**线上字段名集合**（已按 serde 规则换算），排序。 */
function rustStructFields(src: string, name: string): string[] {
  const { rule, body } = declaration(src, 'struct', name);
  const out: string[] = [];
  let pendingRename: string | null = null;
  for (const raw of body.split('\n')) {
    const line = raw.trim();
    if (line === '') continue;
    if (line.startsWith('#[')) {
      const rename = /#\[serde\(rename\s*=\s*"([^"]+)"\)\]/.exec(line);
      if (rename) {
        pendingRename = rename[1];
        continue;
      }
      if (line.startsWith('#[serde(')) {
        throw new Error(
          `[report-wire-parity] ${name} 的字段带了本门不认识的 serde 属性 \`${line}\` —— ` +
            '它可能改变线格式（skip / flatten / default …），射程之外，必须有人来改门。',
        );
      }
      continue;
    }
    const field = /^pub\s+([a-z_][A-Za-z0-9_]*)\s*:/.exec(line);
    if (!field) continue;
    out.push(pendingRename ?? (rule === 'camelCase' ? camelField(field[1]) : field[1]));
    pendingRename = null;
  }
  if (out.length === 0) {
    throw new Error(`[report-wire-parity] 结构体 ${name} 解析出 0 个字段 —— 解析器与源码脱节。`);
  }
  return out.sort();
}

/** Rust 枚举的**线上变体名集合**（unit 变体；带载荷的当场抛），排序。 */
function rustEnumVariants(src: string, name: string): string[] {
  const { rule, body } = declaration(src, 'enum', name);
  const out: string[] = [];
  let pendingRename: string | null = null;
  for (const raw of body.split('\n')) {
    const line = raw.trim();
    if (line === '') continue;
    if (line.startsWith('#[')) {
      const rename = /#\[serde\(rename\s*=\s*"([^"]+)"\)\]/.exec(line);
      if (rename) {
        pendingRename = rename[1];
        continue;
      }
      if (line.startsWith('#[serde(')) {
        throw new Error(
          `[report-wire-parity] ${name} 的变体带了本门不认识的 serde 属性 \`${line}\` —— 射程之外。`,
        );
      }
      continue;
    }
    const unit = /^([A-Z][A-Za-z0-9]*)\s*,$/.exec(line);
    if (unit) {
      out.push(pendingRename ?? (rule === 'camelCase' ? camelVariant(unit[1]) : unit[1]));
      pendingRename = null;
      continue;
    }
    if (/^[A-Z][A-Za-z0-9]*\s*[({]/.test(line)) {
      throw new Error(
        `[report-wire-parity] ${name} 的变体 \`${line}\` 带载荷 —— 它的线格式不是一个裸字符串，` +
          '本门按 unit 变体解析会静默对错。射程之外，必须有人来改门。',
      );
    }
  }
  if (out.length === 0) {
    throw new Error(`[report-wire-parity] 枚举 ${name} 解析出 0 个变体 —— 解析器与源码脱节。`);
  }
  return out.sort();
}

/**
 * 手写 `to_wire` 的逐支键集：`status` 取值 → 该支 `json!` 里的全部键（含 `status` 自身）。
 *
 * `TunnelConflictSnapshot` 不派生 Serialize，线格式完全由这个函数决定 —— 用结构体规则去套它
 * 只会解析出空集，而空集上的相等断言恒真。
 */
function toWireArms(src: string): Map<string, string[]> {
  const at = src.indexOf('fn to_wire(&self) -> Value {');
  if (at < 0) {
    throw new Error(
      '[report-wire-parity] 找不到 `fn to_wire(&self) -> Value {` —— 改名/改签名了？本门已失去判据。',
    );
  }
  const open = src.indexOf('{', at);
  let depth = 0;
  let end = -1;
  for (let i = open; i < src.length; i += 1) {
    if (src[i] === '{') depth += 1;
    else if (src[i] === '}') {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) throw new Error('[report-wire-parity] to_wire 的大括号不配平');
  const body = src.slice(open, end);

  const arms = new Map<string, string[]>();
  for (const arm of body.matchAll(
    /Self::([A-Za-z0-9_]+)\s*(?:\{[^}]*\})?\s*=>\s*json!\(\{([\s\S]*?)\}\)/g,
  )) {
    const [, variant, literal] = arm;
    const status = /"status"\s*:\s*"([A-Za-z][A-Za-z0-9_]*)"/.exec(literal)?.[1];
    if (status === undefined) {
      throw new Error(
        `[report-wire-parity] to_wire 的 \`${variant}\` 支没有字面量 "status" —— ` +
          '判别联合失去判别键，渲染端无法分支。',
      );
    }
    const keys = [...literal.matchAll(/"([A-Za-z][A-Za-z0-9_]*)"\s*:/g)].map((m) => m[1]).sort();
    arms.set(status, keys);
  }
  if (arms.size === 0) {
    throw new Error('[report-wire-parity] to_wire 一支都没解析出来 —— 解析器与源码脱节。');
  }
  return arms;
}

/* ══════════════════════════════════════════════════════════════════════════
 * TS 侧：真 AST，不是正则
 * ══════════════════════════════════════════════════════════════════════════ */

function findDeclaration(sf: ts.SourceFile, name: string): ts.Node {
  let found: ts.Node | undefined;
  const visit = (n: ts.Node): void => {
    if (
      (ts.isInterfaceDeclaration(n) || ts.isTypeAliasDeclaration(n)) &&
      n.name.getText(sf) === name
    ) {
      found = n;
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  if (!found) {
    throw new Error(`[report-wire-parity] TS 契约里找不到 \`${name}\` —— 改名/删了？本门已失去判据。`);
  }
  return found;
}

/** TS 接口的字段名集合（含可选字段；`?` 不改变线上的键名），排序。 */
function tsInterfaceFields(sf: ts.SourceFile, name: string): string[] {
  const decl = findDeclaration(sf, name);
  if (!ts.isInterfaceDeclaration(decl)) {
    throw new Error(`[report-wire-parity] \`${name}\` 不是 interface`);
  }
  const out: string[] = [];
  for (const member of decl.members) {
    if (!ts.isPropertySignatureDeclaration(member)) continue;
    out.push(member.name.getText(sf).replace(/^['"]|['"]$/g, ''));
  }
  if (out.length === 0) {
    throw new Error(`[report-wire-parity] interface ${name} 解析出 0 个字段 —— 解析器与源码脱节。`);
  }
  return out.sort();
}

/** `type X = 'a' | 'b'` 的字面量集合（单成员亦可），排序。 */
function tsUnionLiterals(sf: ts.SourceFile, name: string): string[] {
  const decl = findDeclaration(sf, name);
  if (!ts.isTypeAliasDeclaration(decl)) {
    throw new Error(`[report-wire-parity] \`${name}\` 不是 type alias`);
  }
  const parts = ts.isUnionTypeNode(decl.type) ? [...decl.type.types] : [decl.type];
  const out = parts.map((p) => {
    if (!ts.isLiteralTypeNode(p) || !ts.isStringLiteral(p.literal)) {
      throw new Error(
        `[report-wire-parity] \`${name}\` 的成员 \`${p.getText(sf)}\` 不是字符串字面量 —— ` +
          '它对不上 Rust 的 unit 变体，本门无法对拍。',
      );
    }
    return p.literal.text;
  });
  if (out.length === 0) {
    throw new Error(`[report-wire-parity] 联合 ${name} 解析出 0 个成员 —— 解析器与源码脱节。`);
  }
  return out.sort();
}

/**
 * `type X = { status: 'a'; … } | { status: 'b'; … }` 的逐支键集：`status` 取值 → 该支全部键。
 *
 * **缺键是判据的一部分**：把它折成一个并集，就分不出「只有 `probed` 带 `conflicts`」这件事，
 * 而那正是这份契约存在的全部理由。
 */
function tsDiscriminatedUnion(sf: ts.SourceFile, name: string): Map<string, string[]> {
  const decl = findDeclaration(sf, name);
  if (!ts.isTypeAliasDeclaration(decl) || !ts.isUnionTypeNode(decl.type)) {
    throw new Error(`[report-wire-parity] \`${name}\` 不是判别联合 —— 契约形态被改了。`);
  }
  const out = new Map<string, string[]>();
  for (const branch of decl.type.types) {
    if (!ts.isTypeLiteralNode(branch)) {
      throw new Error(`[report-wire-parity] \`${name}\` 的一支不是对象字面量：${branch.getText(sf)}`);
    }
    const keys: string[] = [];
    let status: string | undefined;
    for (const member of branch.members) {
      if (!ts.isPropertySignatureDeclaration(member)) continue;
      const key = member.name.getText(sf).replace(/^['"]|['"]$/g, '');
      keys.push(key);
      if (key !== 'status') continue;
      const t = member.type;
      if (t && ts.isLiteralTypeNode(t) && ts.isStringLiteral(t.literal)) status = t.literal.text;
    }
    if (status === undefined) {
      throw new Error(`[report-wire-parity] \`${name}\` 有一支的 status 不是字符串字面量 —— 判别不了。`);
    }
    out.set(status, keys.sort());
  }
  return out;
}

/* ══════════════════════════════════════════════════════════════════════════
 * 门 0 · 解析器自检：两套 serde 规则、两种取材方式，都必须真的点过火
 * ══════════════════════════════════════════════════════════════════════════ */

describe('门 0 · 解析器自检（不点火的判据等于没有判据）', () => {
  it('取材面非空且是本门要的那三个模块', () => {
    expect(ENDPOINT_RS).toContain('pub struct EndpointForceRouteReport');
    expect(TUNNEL_LIB_RS).toContain('pub enum ConflictKind');
    expect(TUNNEL_RT_RS).toContain('fn to_wire(&self) -> Value');
  });

  it('字段规则与变体规则是**两条**规则，不能互相顶替', () => {
    // 同一个 `rename_all = "camelCase"`，作用在字段上是「按 `_` 分段首字母大写」，
    // 作用在变体上是「只小写首字母」。写成一个函数会把 `FakeIpOverlap` 对成 `fakeipoverlap`。
    expect(camelField('by_server_id')).toBe('byServerId');
    expect(camelField('zero_coverage_server_ids')).toBe('zeroCoverageServerIds');
    expect(camelField('fakeip_ranges')).toBe('fakeipRanges');
    expect(camelVariant('FakeIpOverlap')).toBe('fakeIpOverlap');
    expect(camelVariant('AbsorbedEmpty')).toBe('absorbedEmpty');
  });

  it('**没有** rename_all 的结构体不得被当成 camelCase 处理', () => {
    // `ForeignTunnelRoute` / `TunnelConflict` 两个结构体都没有那行属性，字段原样上线。
    // 解析器若无条件套 camelCase，这两个恰好全是单词字段、**照样相等** —— 故这里用一个合成
    // 样本把规则本身钉住，而不是靠真语料碰巧不出错。
    const sample = [
      '#[derive(serde::Serialize)]',
      'pub struct NoRule {',
      '    pub by_server_id: String,',
      '}',
      '#[derive(serde::Serialize)]',
      '#[serde(rename_all = "camelCase")]',
      'pub struct WithRule {',
      '    pub by_server_id: String,',
      '}',
    ].join('\n');
    expect(rustStructFields(sample, 'NoRule')).toEqual(['by_server_id']);
    expect(rustStructFields(sample, 'WithRule')).toEqual(['byServerId']);
  });

  it('per-variant / per-field `rename` 优先于 rename_all', () => {
    const sample = [
      '#[serde(rename_all = "camelCase")]',
      'pub enum Sample {',
      '    #[serde(rename = "openvpn-client")]',
      '    OpenvpnClient,',
      '    PlainOne,',
      '}',
    ].join('\n');
    expect(rustEnumVariants(sample, 'Sample')).toEqual(['openvpn-client', 'plainOne']);
  });

  it('射程之外的形态当场抛（带载荷的变体 / 认不出的 serde 属性 / 命中 0 或 2 次）', () => {
    const payload = ['pub enum Sample {', '    WithData { x: u8 },', '}'].join('\n');
    expect(() => rustEnumVariants(payload, 'Sample')).toThrow(/带载荷/);

    const weird = [
      '#[serde(rename_all = "camelCase")]',
      'pub struct Sample {',
      '    #[serde(skip_serializing_if = "Vec::is_empty")]',
      '    pub items: Vec<String>,',
      '}',
    ].join('\n');
    expect(() => rustStructFields(weird, 'Sample')).toThrow(/不认识的 serde 属性/);

    expect(() => rustStructFields('pub struct Gone {\n}', 'Missing')).toThrow(/命中 0 次/);
    const twice = 'pub struct Dup {\n    pub a: u8,\n}\npub struct Dup {\n    pub b: u8,\n}';
    expect(() => rustStructFields(twice, 'Dup')).toThrow(/命中 2 次/);
  });

  it('TS 侧解析器抓不到声明必须抛，而不是返回空集让断言恒真', () => {
    expect(() => tsInterfaceFields(FORCE_ROUTE_TS, 'NotThere')).toThrow(/找不到/);
    expect(() => tsUnionLiterals(TUNNEL_TS, 'NotThere')).toThrow(/找不到/);
  });
});

/* ══════════════════════════════════════════════════════════════════════════
 * 门 1 · endpoint-force-route-report.ts ↔ builder::endpoint_routes
 * ══════════════════════════════════════════════════════════════════════════ */

describe('门 1 · 组网段结算报告的线格式两侧相等', () => {
  it('ForceRouteLeg：三条腿逐字相等', () => {
    expect(tsUnionLiterals(FORCE_ROUTE_TS, 'ForceRouteLeg')).toEqual(
      rustEnumVariants(ENDPOINT_RS, 'ForceRouteLeg'),
    );
  });

  it('ForceRouteCoverage：三态逐字相等', () => {
    expect(tsUnionLiterals(FORCE_ROUTE_TS, 'ForceRouteCoverage')).toEqual(
      rustEnumVariants(ENDPOINT_RS, 'ForceRouteCoverage'),
    );
  });

  it('AbsorbedCidr：字段集相等（`byServerId` 少一个，用户就不知道被谁抢走）', () => {
    expect(tsInterfaceFields(FORCE_ROUTE_TS, 'AbsorbedCidr')).toEqual(
      rustStructFields(ENDPOINT_RS, 'AbsorbedCidr'),
    );
  });

  it('ServerForceRoute：字段集相等（`hasObservation` 是证据强度位，漏了就把猜测读成撞车）', () => {
    expect(tsInterfaceFields(FORCE_ROUTE_TS, 'ServerForceRoute')).toEqual(
      rustStructFields(ENDPOINT_RS, 'ServerForceRoute'),
    );
  });

  it('EndpointForceRouteReport：字段集相等（`zeroCoverageServerIds` 漏了 = 静默失效整条消失）', () => {
    expect(tsInterfaceFields(FORCE_ROUTE_TS, 'EndpointForceRouteReport')).toEqual(
      rustStructFields(ENDPOINT_RS, 'EndpointForceRouteReport'),
    );
  });

  it('正向对照：解析出来的确实是 camelCase 线名，不是原样的 snake_case', () => {
    // 上面五条都是「两侧相等」。若解析器两侧都读成 snake_case，它们照样全绿 ——
    // 这条钉住 Rust 那一侧真的过了 serde 换算。
    const report = rustStructFields(ENDPOINT_RS, 'EndpointForceRouteReport');
    expect(report).toContain('zeroCoverageServerIds');
    expect(report).not.toContain('zero_coverage_server_ids');
    expect(rustEnumVariants(ENDPOINT_RS, 'ForceRouteCoverage')).toContain('absorbedEmpty');
  });
});

/* ══════════════════════════════════════════════════════════════════════════
 * 门 2 · tunnel-conflict-report.ts ↔ runtime::proxy::tunnel_conflict + builder::tunnel_conflict
 * ══════════════════════════════════════════════════════════════════════════ */

describe('门 2 · 外来隧道冲突报告的线格式两侧相等', () => {
  const ARMS = toWireArms(TUNNEL_RT_RS);
  const BRANCHES = tsDiscriminatedUnion(TUNNEL_TS, 'TunnelConflictReport');

  it('四支齐全，且 status 取值逐字相等', () => {
    expect([...ARMS.keys()].sort()).toEqual(
      ['notProbed', 'probeFailed', 'probed', 'unsupported'].sort(),
    );
    expect([...BRANCHES.keys()].sort()).toEqual([...ARMS.keys()].sort());
  });

  it('**逐支**键集相等 —— 不是并集（缺键本身就是判据）', () => {
    for (const [status, keys] of ARMS) {
      expect(BRANCHES.get(status), `TS 缺 status='${status}' 这一支`).toEqual(keys);
    }
  });

  it('只有 `probed` 带 `conflicts`：其余三支两侧都必须缺这个键', () => {
    // 这条是上一条的**正向对照**：若解析器把某一侧读空，逐支相等会平凡通过（空 ≡ 空）。
    expect(ARMS.get('probed')).toContain('conflicts');
    expect(BRANCHES.get('probed')).toContain('conflicts');
    for (const status of ['notProbed', 'unsupported', 'probeFailed']) {
      expect(ARMS.get(status), `Rust 的 ${status} 支带上了 conflicts`).not.toContain('conflicts');
      expect(BRANCHES.get(status), `TS 的 ${status} 支带上了 conflicts`).not.toContain('conflicts');
    }
  });

  it('ConflictKind：三类冲突逐字相等', () => {
    expect(tsUnionLiterals(TUNNEL_TS, 'TunnelConflictKind')).toEqual(
      rustEnumVariants(TUNNEL_LIB_RS, 'ConflictKind'),
    );
  });

  it('ForeignTunnelRoute / TunnelConflict：无 rename_all 的两个结构体字段集相等', () => {
    expect(tsInterfaceFields(TUNNEL_TS, 'ForeignTunnelRoute')).toEqual(
      rustStructFields(TUNNEL_LIB_RS, 'ForeignTunnelRoute'),
    );
    expect(tsInterfaceFields(TUNNEL_TS, 'TunnelConflict')).toEqual(
      rustStructFields(TUNNEL_LIB_RS, 'TunnelConflict'),
    );
  });

  it('TunnelConflictCriteria ↔ Rust ConflictCriteria：字段集相等（两侧名字不同，故显式映射）', () => {
    expect(tsInterfaceFields(TUNNEL_TS, 'TunnelConflictCriteria')).toEqual(
      rustStructFields(TUNNEL_LIB_RS, 'ConflictCriteria'),
    );
  });

  it('正向对照：`probed` 支的载荷键真的被解析出来了（不是空集在自相等）', () => {
    expect(ARMS.get('probed')).toEqual(
      ['conflicts', 'criteria', 'foreignTunnels', 'status', 'suppressedRoutes'].sort(),
    );
    expect(rustStructFields(TUNNEL_LIB_RS, 'ConflictCriteria')).toContain('fakeipRanges');
  });
});
