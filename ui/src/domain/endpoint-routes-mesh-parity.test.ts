/**
 * `meshSystemSupportedOnPlatform` 的**跨语言**对拍门（TS ↔ Rust 字符串版）。
 *
 * # 守的是什么（真缺陷，2026-09-06 发现时它已经存在）
 *
 * 同一条谓词有三份实现：
 *
 * | # | 位置 | 形态 |
 * |---|---|---|
 * | A | `crates/mesh/src/exit_route.rs` | `Platform` 枚举穷举 `match` |
 * | B | `crates/config-engine/src/builder/endpoint_routes.rs` | 字符串**允许清单** |
 * | C | `ui/src/domain/endpoint-routes.ts`（本门守的那份） | 字符串谓词 |
 *
 * A↔B 由 `src-tauri/tests/platform_dispatch_exhaustive.rs` 的
 * `mesh_system_support_agrees_across_enum_and_string_faces` 逐名对拍。**C 在那道门的射程之外** ——
 * 于是 2026-09-05 把 A、B 从禁止清单改成允许清单那一次，C 被落下了，三份实现在
 * `'ios'` 与未经别名映射的 `'windows'` 上不同答（C 答 true，A/B 答 false），而**没有任何东西会说话**。
 *
 * 判错成 `true` 的代价是起核 FATAL（核拿 `system: true` 去开一张开不出来的内核接口），
 * 判错成 `false` 只是退 gVisor —— 代价不对称，故这条一致性值一道门。
 *
 * # 取材面是**派生的**，不是手写的
 *
 * 平台名清单从 `helper-proto` 的 `Platform::parse` 抠（那是字符串轴与枚举轴唯一的桥），
 * 期望答案从 B 的 `matches!` 允许清单抠。两边都从磁盘上的 Rust 源码现读 ——
 * 手写一份快照会在「Rust 侧改了、门没跟」时静默判绿，而那正是本门要防的形态。
 *
 * # 判据都带正面断言（不许「什么都没扫到」判绿）
 *
 * 纯相等断言会被「两边取材都塌成空集」骗过：`[].every(...)` 恒真。故每组先自检取材面规模，
 * 再断言支持面**非空**（若两份实现同时坏成全 false，逐名相等仍然成立）。
 *
 * # 必须剥 Rust 注释
 *
 * 被抠的两个函数的头注里逐字写满了 `"win32"` / `"android"` / `"darwin"` 这些字面量（它们正是在
 * 讲这件事）。不剥就会把注释里的举例抠成允许清单 —— 判据被自己的文档喂饱。
 * 同理，剥注释时**必须识别字符串**，否则一个含 `//` 的字面量会把后面整行代码吃掉。
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve as resolvePath } from 'node:path';

import { meshSystemSupportedOnPlatform } from './endpoint-routes';

/** …/ui/ */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
/** 仓根（`ui` 与 `crates` 的共同父目录）。 */
const REPO_ROOT = resolvePath(UI_ROOT, '..');

const readRust = (relToRepo: string) =>
  readFileSync(resolvePath(REPO_ROOT, relToRepo), 'utf8');

/**
 * 把 Rust 的行/块/doc 注释抹成空格，**保留字符串与字符字面量**（要抠的正是它们）。
 *
 * 逐字符扫而不是正则：正则版会被字符串里的 `//` 或 `/*` 骗到（`mask_code` 在 Rust 侧的门里
 * 也是同样的理由写成状态机）。原始字节长度保持不变，便于后续按下标做括号配平。
 */
function stripRustComments(src: string): string {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const next = src[i + 1];
    if (c === '/' && next === '*') {
      // Rust 的块注释可嵌套。
      let depth = 1;
      let j = i + 2;
      out += '  ';
      while (j < src.length && depth > 0) {
        if (src[j] === '/' && src[j + 1] === '*') {
          depth += 1;
          out += '  ';
          j += 2;
        } else if (src[j] === '*' && src[j + 1] === '/') {
          depth -= 1;
          out += '  ';
          j += 2;
        } else {
          out += src[j] === '\n' ? '\n' : ' ';
          j += 1;
        }
      }
      i = j;
    } else if (c === '/' && next === '/') {
      let j = i;
      while (j < src.length && src[j] !== '\n') {
        out += ' ';
        j += 1;
      }
      i = j;
    } else if (c === '"') {
      // 字符串字面量原样保留（含转义）。
      out += c;
      let j = i + 1;
      while (j < src.length) {
        out += src[j];
        if (src[j] === '\\') {
          if (j + 1 < src.length) {
            out += src[j + 1];
            j += 2;
            continue;
          }
        } else if (src[j] === '"') {
          j += 1;
          break;
        }
        j += 1;
      }
      i = j;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/** `open` 处 `{` 的配平右括号下标（含）。 */
function balancedEnd(code: string, open: number): number {
  let depth = 0;
  for (let i = open; i < code.length; i += 1) {
    if (code[i] === '{') depth += 1;
    else if (code[i] === '}') {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  throw new Error('括号不配平 —— 取材面塌了');
}

/** 抠出 `fn <name>` 的函数体（已剥注释的代码面上）。 */
function fnBody(code: string, signature: string): string {
  const occurrences = code.split(signature).length - 1;
  expect(
    occurrences,
    `锚点 ${signature} 在源码里出现 ${occurrences} 次，不唯一 —— 抠到的可能是别的函数体`
  ).toBe(1);
  const at = code.indexOf(signature);
  const open = code.indexOf('{', at);
  expect(open, `${signature} 之后找不到 '{'`).toBeGreaterThan(-1);
  return code.slice(open, balancedEnd(code, open) + 1);
}

/** 函数体里的**小写 ASCII** 字符串字面量（去重、保序）。 */
function lowercaseStringLiterals(body: string): string[] {
  const out: string[] = [];
  const re = /"([a-z0-9_]+)"/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(body)) !== null) {
    if (!out.includes(m[1])) out.push(m[1]);
  }
  return out;
}

// ── 取材（现读磁盘上的 Rust 源码）────────────────────────────────────────────

/** 字符串轴与枚举轴唯一的桥：`Platform::parse` 认得的全部平台名。 */
const PLATFORM_NAMES = lowercaseStringLiterals(
  fnBody(stripRustComments(readRust('crates/helper-proto/src/lib.rs')), 'fn parse')
);

/** Rust 字符串版（实现 B）的允许清单。 */
const RUST_ALLOWLIST = lowercaseStringLiterals(
  fnBody(
    stripRustComments(readRust('crates/config-engine/src/builder/endpoint_routes.rs')),
    'fn mesh_system_supported_on_platform'
  )
);

describe('meshSystemSupportedOnPlatform：TS 与 Rust 字符串版逐名同答', () => {
  it('① 取材面自检：两侧都真的抠到了东西', () => {
    // 下限不是「刚好等于今天的数」：抠取口径坏掉表现为 0 或 1，一定低于这里。
    expect(
      PLATFORM_NAMES.length,
      `只从 Platform::parse 抠出 ${PLATFORM_NAMES.length} 个平台名（${PLATFORM_NAMES.join(', ')}）—— 抠取口径坏了`
    ).toBeGreaterThanOrEqual(5);
    expect(
      RUST_ALLOWLIST.length,
      `只从 Rust 允许清单抠出 ${RUST_ALLOWLIST.length} 个字面量（${RUST_ALLOWLIST.join(', ')}）—— 抠取口径坏了`
    ).toBeGreaterThanOrEqual(2);

    // 允许清单必须是平台名的子集：抠到了别的串（如注释残渣、`to_ascii_lowercase` 里的标识符
    // 片段）会在这里红，而不是悄悄让下面的逐名比对多出几条平凡成立的项。
    for (const name of RUST_ALLOWLIST) {
      expect(
        PLATFORM_NAMES,
        `Rust 允许清单里的 ${name} 不被 Platform::parse 认得 —— 两侧取材已脱钩`
      ).toContain(name);
    }
  });

  it('② 逐名同答（含 ios / windows 这两个改前不同答的格）', () => {
    const diffs: string[] = [];
    for (const name of PLATFORM_NAMES) {
      const rust = RUST_ALLOWLIST.includes(name);
      const ts = meshSystemSupportedOnPlatform(name);
      if (ts !== rust) diffs.push(`${name}: Rust=${rust} / TS=${ts}`);
    }
    expect(
      diffs,
      `两份实现不同答：\n  ${diffs.join('\n  ')}\n\n` +
        '判错成 true 的代价是起核 FATAL（核拿 system:true 去开一张开不出来的内核接口）；' +
        '判错成 false 只是退 gVisor。三份实现必须逐名同答。'
    ).toEqual([]);
  });

  it('③ 正面断言：支持面非空（否则上一条会被「两边同时全 false」骗过）', () => {
    expect(meshSystemSupportedOnPlatform('darwin')).toBe(true);
    expect(meshSystemSupportedOnPlatform('linux')).toBe(true);
    expect(RUST_ALLOWLIST).toContain('darwin');
    expect(RUST_ALLOWLIST).toContain('linux');
  });

  it('④ 未点名的平台一律 false（禁止清单形态不许回来）', () => {
    // 'ios' 在 2026-09-06 已被 `Platform::parse` 认领成具名变体，故它不在这条"未知"清单里 ——
    // 它由 ② 逐名对拍覆盖。这里放的是真正没人答过题的那些。
    for (const unknown of ['freebsd', 'openbsd', 'solaris', 'haiku', '']) {
      expect(
        PLATFORM_NAMES,
        `本用例假定 ${unknown} 是未知平台；它已被 Platform::parse 认领，换一个`
      ).not.toContain(unknown);
      expect(
        meshSystemSupportedOnPlatform(unknown),
        `对未知平台串 ${JSON.stringify(unknown)} 答了「支持」—— 禁止清单式写法回来了`
      ).toBe(false);
    }
    expect(meshSystemSupportedOnPlatform(undefined)).toBe(false);
  });

  it('⑤ 大小写不敏感（Rust 侧 to_ascii_lowercase，两边契约必须一致）', () => {
    for (const name of PLATFORM_NAMES) {
      expect(
        meshSystemSupportedOnPlatform(name.toUpperCase()),
        `对 ${name} 的大小写敏感了`
      ).toBe(meshSystemSupportedOnPlatform(name));
    }
  });
});
