import { describe, expect, it } from 'vitest';
import { parseMtuInput, MTU_MAX, MTU_MIN } from './tun-mtu';

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
