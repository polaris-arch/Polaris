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
 *
 * # 射程外（如实登记）
 *
 * 第三种定义形态（例如某个新函数直接 `return "some-token"` 再塞进 `gate_invalid_nodes`）抽不到。
 * 新增 reason 请沿用 R1 的 `INVALID_REASON_*` 常量形态，本门自动覆盖。
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { SUPPORTED_LANGUAGES } from '../domain/language';
import { INVALID_NODE_REASON_KEY } from '../domain/invalid-node-reason';
import {
  maskRustComments,
  moduleSource,
  productionRsFilesUnder,
} from './rust-source.test-support';

const LOCALES_DIR = fileURLToPath(new URL('../i18n/locales', import.meta.url));

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
});
