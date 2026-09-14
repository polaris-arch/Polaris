import { describe, expect, it } from 'vitest';
import { DEFAULT_TUN_MTU, parseMtuInput, MTU_MAX, MTU_MIN } from './tun-mtu';
import {
  maskRustCommentsAndStrings,
  moduleSource,
} from '@/contracts/rust-source.test-support';

/** Rust 真值所在模块（模块路径，不带扩展名 —— 常量被挪进子文件时取材面照样跟着走）。 */
const RUST_MODULE = 'crates/config-engine/src/user_config/tun_config';

/**
 * 从**已净化**（注释 + 字符串都抹掉）的取材面抓 `const NAME: u32 = <数>;`，命中数必须恰好 1。
 *
 * 纪律同 `rustConstU64`（该 helper 只认 `u64`，本常量是 `u32`，故就地同构一份而不改共享 helper）：
 * 注释 / 字符串里的同形常量不算证据；0 次 = 改名或搬走、门已失去判据；>1 次 = 指向哪一处全凭书写顺序。
 */
function rustConstU32(masked: string, name: string): number {
  const hits = [...masked.matchAll(new RegExp(`const\\s+${name}\\s*:\\s*u32\\s*=\\s*([0-9_]+)\\s*;`, 'g'))];
  if (hits.length !== 1) {
    throw new Error(`Rust 常量 ${name} 在净化后的取材面上命中 ${hits.length} 次（应为 1）`);
  }
  return Number(hits[0][1].replace(/_/g, ''));
}

/// 🔴 **跨语言 parity 门**：渲染端占位符显示的「自动 = 多少」必须等于内核实际拿到的值。
///
/// 生成期真值在 Rust；两侧一旦分叉，用户看到的自动值就不是内核实际拿到的值 —— 那比不显示更糟。
/// 此前这道门是两张「栈 × 平台」表逐格镜像（靠人同步两边的用例）；TUN stack 移除后默认值收敛成
/// 单一常量，改成**直接读 Rust 源码里的字面量**对拍，任一侧改值而另一侧没跟即红。
describe('TUN 默认 MTU（与 Rust DEFAULT_TUN_MTU 同值）', () => {
  it('渲染端常量 == Rust `tun_config::DEFAULT_TUN_MTU`', () => {
    const rust = rustConstU32(maskRustCommentsAndStrings(moduleSource(RUST_MODULE)), 'DEFAULT_TUN_MTU');
    expect(DEFAULT_TUN_MTU).toBe(rust);
  });

  /// 判据待 2026-09-13 三平台实测回填；当前值 = sing-box 桌面上游默认。
  it('当前值为 65535，且落在可手填区间内（占位符显示的数用户也能原样填回去）', () => {
    expect(DEFAULT_TUN_MTU).toBe(65535);
    expect(parseMtuInput(String(DEFAULT_TUN_MTU))).toEqual({ mtu: DEFAULT_TUN_MTU });
  });

  /// 反向对照：抽取器必须对「注释里的同形常量」「字符串里的同形常量」「改名 / 重复定义」各自说话，
  /// 否则上面的 parity 可能读到的是一个假值而恒绿。
  it('抽取器有牙：只认净化面上的唯一真定义', () => {
    const real = 'pub const DEFAULT_TUN_MTU: u32 = 65535;';
    expect(rustConstU32(maskRustCommentsAndStrings(real), 'DEFAULT_TUN_MTU')).toBe(65535);

    // 文档注释里写一行同形常量 + 真常量值不同：只能读到真值。
    const decoy = `/// pub const DEFAULT_TUN_MTU: u32 = 9000;\n${real}`;
    expect(rustConstU32(maskRustCommentsAndStrings(decoy), 'DEFAULT_TUN_MTU')).toBe(65535);

    // 字符串里的同形常量 + 真常量改名：净化后 0 命中 ⇒ 抛，而不是读到字符串里的假值。
    const inString = 'const X: &str = "const DEFAULT_TUN_MTU: u32 = 9000;";\npub const RENAMED: u32 = 65535;';
    expect(() => rustConstU32(maskRustCommentsAndStrings(inString), 'DEFAULT_TUN_MTU')).toThrow(/命中 0 次/);

    // 两处真定义：判据指向哪一处全凭顺序 ⇒ 抛。
    expect(() => rustConstU32(`${real}\n${real}`, 'DEFAULT_TUN_MTU')).toThrow(/命中 2 次/);
  });
});

describe('MTU 输入解析', () => {
  it('空白 → 自动（mtu 缺席）', () => {
    expect(parseMtuInput('')).toEqual({ mtu: undefined });
    expect(parseMtuInput('   ')).toEqual({ mtu: undefined });
  });

  it('区间内整数原样收下', () => {
    expect(parseMtuInput('4064')).toEqual({ mtu: 4064 });
    expect(parseMtuInput(String(MTU_MIN))).toEqual({ mtu: MTU_MIN });
    expect(parseMtuInput(String(MTU_MAX))).toEqual({ mtu: MTU_MAX });
  });

  /// 🔴 越界**不钳制**。悄悄把 70000 改成 65535 = 框里是用户填的数、生效的是另一个，
  /// 与旧实现「填 9000 被静默改写成 1350」是同一类缺陷。宁可当场报错。
  it('越界与非数字一律判非法，不做钳制', () => {
    expect(parseMtuInput('70000')).toEqual({ invalid: true });
    expect(parseMtuInput('1279')).toEqual({ invalid: true });
    expect(parseMtuInput('0')).toEqual({ invalid: true });
    expect(parseMtuInput('abc')).toEqual({ invalid: true });
    expect(parseMtuInput('4064.5')).toEqual({ invalid: true });
    expect(parseMtuInput('-4064')).toEqual({ invalid: true });
  });
});
