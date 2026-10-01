/**
 * 剔除原因 token → 用户可见句。Tailcat 的两个 token 直接从 Rust 常量读（`protocol_settings.rs`），
 * 防止后端改名而这张表静默失联（失联时 NodeCard 只剩通用前半句，「为什么」那半句没了）。
 */
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { INVALID_NODE_REASON_KEY, invalidNodeReasonText } from './invalid-node-reason';
import zhCN from '@/i18n/locales/zh-CN.json';

const RUST = readFileSync(
  fileURLToPath(new URL('../../../crates/config-engine/src/user_config/protocol_settings.rs', import.meta.url)),
  'utf8'
);

describe('Tailcat 剔除原因登记', () => {
  it('Rust 的 INVALID_REASON_TAILCAT_* 全部登记，且映到存在的 i18n 键', () => {
    const tokens = [...RUST.matchAll(/pub const INVALID_REASON_TAILCAT_\w+: &str = "([^"]+)";/g)].map((m) => m[1]);
    expect(tokens.sort()).toEqual(['tailcat-derp-invalid', 'tailcat-key-invalid']);
    const lookup = (key: string) =>
      key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], zhCN);
    for (const token of tokens) {
      const key = INVALID_NODE_REASON_KEY[token];
      expect(key, `${token} 没登记`).toBeDefined();
      expect(typeof lookup(key), `${key} 不在 locale 里`).toBe('string');
    }
  });

  it('tooltip 拼的是人话，不是 token', () => {
    const t = (k: string) => `<${k}>`;
    expect(invalidNodeReasonText('tailcat-key-invalid', t)).toBe(
      '<nodes.nodeInvalid><nodes.nodeInvalidSep><nodes.invalidTailcatKey>'
    );
    expect(invalidNodeReasonText('tailcat-derp-invalid', t)).not.toContain('tailcat-derp-invalid');
  });
});
