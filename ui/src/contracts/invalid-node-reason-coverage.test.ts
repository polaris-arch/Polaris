/**
 * 启动 gate 剔除原因（`InvalidNodeInfo.reason`）的**跨语言对账门**：
 * Rust config-engine 的 reason token ↔ `domain/invalid-node-reason.ts` 登记表 ↔ 五份 locale。
 *
 * # 守的是什么
 *
 * token 未登记时 `NodeCard` 只渲染通用前半句「节点配置无效，已在启动时跳过」——唯一说明「为什么」
 * 的那半句静默消失。TypeScript 抓不到（reason 运行期就是个 string），纯前端测试也抓不到
 * （登记表自己跟自己比永远一致）。实例：MASQUE 两个 token 上线时没登记，门缺席所以没人发现。
 *
 * # 取材面（从 Rust 源码抽，不在这里手抄名单）
 *
 * `crates/config-engine/src` 全部**生产** `.rs`（剔除 `tests/`），剥注释后按两种定义形态抽：
 *  R1 `const INVALID_REASON_<X>: &str = "<token>";` —— detour 级联 / custom 畸形 / MASQUE / Tailcat；
 *  R2 `user_config/control_url.rs` 的 `reject_token` match 臂 `=> "<token>"` —— control_url 三个成因。
 *
 * # 判据
 *
 *  G1 token 集与登记表键集**双向相等**：正向抓「后端新 token 没登记」，反向抓「token 改名/删了，前端留死项」。
 *  G2 登记表每个 i18n 键在五语种都是非空串（缺了 i18next 会把键名渲染给用户）。
 *  G0 取材自检：两种形态各自抽到内容，且包含已知 token（防正则失配后空跑恒绿）。
 *  G3 **消费方轴**：凡是把 `invalidNodeIndex(…)` 算出来的剔除索引拿去渲染的界面（桌面节点屏、
 *     移动端节点屏……），它的**屏内相对 import 闭包**里必须有 `invalidNodeReasonText(` 这条翻译腿。
 *     消费方集合从 `ui/src` 全部生产源码里**现场扫**（不手抄名单），故新开第三个节点列表也自动进面；
 *     自检要求两端（`components/` 与 `mobile/`）各至少一个消费方 —— 同一 reason 在两端同文案的前提
 *     是两端过的是同一个函数、同一张表。
 *
 * # 射程外（如实登记）
 *
 * 第三种定义形态（例如某个新函数直接 `return "some-token"` 再塞进 `gate_invalid_nodes`）抽不到。
 * 新增 reason 请沿用 R1 的 `INVALID_REASON_*` 常量形态，本门自动覆盖。
 */
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { blankComments } from './comment-blanking.test-support';
import { IS_TEST_ONLY_MODULE } from './test-only-modules';
import { SUPPORTED_LANGUAGES } from '../domain/language';
import { INVALID_NODE_REASON_KEY } from '../domain/invalid-node-reason';
import {
  maskRustComments,
  moduleSource,
  productionRsFilesUnder,
} from './rust-source.test-support';

const LOCALES_DIR = fileURLToPath(new URL('../i18n/locales', import.meta.url));
const UI_SRC = fileURLToPath(new URL('..', import.meta.url));
/** 翻译函数的定义文件：它自己含 `invalidNodeReasonText(`，不能算作任何消费方的翻译腿。 */
const REASON_MODULE = join(UI_SRC, 'domain', 'invalid-node-reason.ts');

/** R1：全部生产源码里的 `INVALID_REASON_*` 常量值。剥注释（保留字符串——针本身就是字面量）。 */
function constTokens(): string[] {
  const re = /\bconst\s+INVALID_REASON_\w+\s*:\s*&(?:'static\s+)?str\s*=\s*"([^"]+)"\s*;/g;
  return productionRsFilesUnder('crates/config-engine/src').flatMap((file) =>
    [...maskRustComments(readFileSync(file, 'utf8')).matchAll(re)].map((m) => m[1]),
  );
}

/** R2：`control_url::reject_token` 的 match 臂字面量。函数找不到即抛（改名/改形 = 判据失效）。 */
function controlUrlTokens(): string[] {
  const src = maskRustComments(moduleSource('crates/config-engine/src/user_config/control_url'));
  const body = /pub fn reject_token\([^)]*\)\s*->\s*&'static str\s*\{\s*match\s+\w+\s*\{([\s\S]*?)\}\s*\}/.exec(
    src,
  );
  if (!body) {
    throw new Error('control_url.rs 里找不到 `reject_token` 的 match —— 改名或改形了，本门已失去 R2 取材面');
  }
  return [...body[1].matchAll(/=>\s*"([^"]+)"/g)].map((m) => m[1]);
}

/** `ui/src` 下全部生产 `.ts/.tsx`（剔除测试 / test-support）。 */
function productionTsFiles(dir: string = UI_SRC): string[] {
  const out: string[] = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...productionTsFiles(p));
    else if (/\.tsx?$/.test(e.name) && !/\.d\.ts$/.test(e.name) && !IS_TEST_ONLY_MODULE.test(e.name)) out.push(p);
  }
  return out;
}

/** 剥注释后的源码（行号不变）。 */
const code = (file: string): string => blankComments(readFileSync(file, 'utf8'));

/**
 * 屏内**相对** import（`./` / `../`）的传递闭包。刻意不跟 `@/…`：那会把整个应用拖进来，
 * 随便哪个远处模块碰巧调一次翻译函数就能喂饱判据。桌面的真实链路是
 * `NodesScreen → ./NodesGrid → ./NodeCard`（两跳），故必须是闭包而不是一跳。
 */
function relativeClosure(root: string): Set<string> {
  const seen = new Set<string>([root]);
  const queue = [root];
  while (queue.length > 0) {
    const file = queue.pop()!;
    for (const m of code(file).matchAll(/(?:from|import)\s*\(?\s*['"](\.{1,2}\/[^'"]+)['"]/g)) {
      const base = resolve(dirname(file), m[1]);
      const hit = [base, `${base}.ts`, `${base}.tsx`, join(base, 'index.ts'), join(base, 'index.tsx')].find(
        (c) => existsSync(c) && statSync(c).isFile() && /\.tsx?$/.test(c),
      );
      if (hit !== undefined && !IS_TEST_ONLY_MODULE.test(hit) && !seen.has(hit)) {
        seen.add(hit);
        queue.push(hit);
      }
    }
  }
  return seen;
}

/** G3 取材：调用（不是定义）`invalidNodeIndex(` 的生产文件 = 剔除索引的渲染消费方。 */
const CONSUMERS = productionTsFiles()
  .filter((f) => {
    const src = code(f);
    return /\binvalidNodeIndex\(/.test(src.replace(/\bfunction\s+invalidNodeIndex\(/g, ''));
  })
  .map((f) => relative(UI_SRC, f).split('\\').join('/'))
  .sort();

const R1 = constTokens();
const R2 = controlUrlTokens();
const RUST_TOKENS = [...new Set([...R1, ...R2])].sort();

function lookup(tree: unknown, key: string): unknown {
  return key
    .split('.')
    .reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], tree);
}

describe('invalid-node reason token 跨语言对账', () => {
  it('G0 取材自检：两种形态都抽到内容，且含已知 token', () => {
    expect(R1.length, 'R1（INVALID_REASON_* 常量）一个都没抽到').toBeGreaterThan(0);
    expect(R2.length, 'R2（reject_token 臂）一个都没抽到').toBeGreaterThan(0);
    for (const known of ['tailcat-key-invalid', 'tailcat-derp-invalid', 'detour-cascade']) {
      expect(R1, `R1 没抽到已知 token ${known}`).toContain(known);
    }
    expect(R2, 'R2 没抽到已知 token control-url-ip').toContain('control-url-ip');
  });

  it('G1 Rust token 集 = 登记表键集（双向）', () => {
    const registered = Object.keys(INVALID_NODE_REASON_KEY).sort();
    const unregistered = RUST_TOKENS.filter((t) => !registered.includes(t));
    const dead = registered.filter((t) => !RUST_TOKENS.includes(t));
    expect(unregistered, 'Rust 有、登记表缺（NodeCard 只剩通用前半句）').toEqual([]);
    expect(dead, '登记表有、Rust 已无（死项）').toEqual([]);
  });

  it('G2 登记的 i18n 键在五语种都是非空串', () => {
    const missing: string[] = [];
    for (const l of SUPPORTED_LANGUAGES) {
      const tree: unknown = JSON.parse(readFileSync(join(LOCALES_DIR, `${l}.json`), 'utf8'));
      for (const [token, key] of Object.entries(INVALID_NODE_REASON_KEY)) {
        const v = lookup(tree, key);
        if (typeof v !== 'string' || v.trim() === '') missing.push(`${l}: ${key} (← ${token})`);
      }
    }
    expect(missing).toEqual([]);
  });

  it('G3 消费方轴：每个渲染剔除索引的屏都经 `invalidNodeReasonText` 翻译（桌面 + 移动端）', () => {
    // 取材自检（正面）：两端各至少一个消费方 —— 扫描面塌成空集时这里先红，而不是下面空跑恒绿。
    expect(
      CONSUMERS.some((f) => f.startsWith('components/')),
      `桌面侧一个消费方都没扫到：${JSON.stringify(CONSUMERS)}`,
    ).toBe(true);
    expect(
      CONSUMERS.some((f) => f.startsWith('mobile/')),
      `移动端一个消费方都没扫到：${JSON.stringify(CONSUMERS)}`,
    ).toBe(true);
    const untranslated = CONSUMERS.filter((rel) => {
      const face = [...relativeClosure(join(UI_SRC, rel))].filter((f) => f !== REASON_MODULE);
      return !face.some((f) => /\binvalidNodeReasonText\(/.test(code(f)));
    });
    expect(
      untranslated,
      '这些屏渲染了剔除索引，却没有经 `invalidNodeReasonText` 翻译 —— 用户会看到机器 token 或看不到「为什么」',
    ).toEqual([]);
  });
});
