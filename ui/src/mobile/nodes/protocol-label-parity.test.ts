/**
 * 节点协议名 / 传输摘要的**两端同源**门。
 *
 * # 守的是什么
 *
 * 移动端节点屏与首页各自抄过一张协议名表（注释还写着「与桌面同表」）。MASQUE / Tailcat 上线时
 * 只有桌面 `NodeCard` 那份加了条目，移动端于是在角标上渲染 `masque-client`（首页大写成
 * `MASQUE-CLIENT`）、传输摘要一格空着 —— 「与桌面同表」这句注释没有任何判据在核。
 *
 * # 取材面（不手抄名单）
 *
 *  P1 协议轴 = `domain/server-completeness#ALL_PROTOCOLS`（与 `contracts/types` 的 `Protocol` 联合派生）：
 *     共用的 `protocolLabel` 对每一个协议都必须给出登记过的显示名（不是回落原始枚举）。
 *  P2 消费方轴 = `ui/src/mobile/` 下全部生产源码里**在对象字面量上赋值** `protocolLabel:` 的文件
 *     （行 VM 的构造点）：每一个都必须从 `components/screens/nodes/nodes-logic` 引入共用函数，
 *     且不得自带一张 `PROTOCOL_LABEL` 表或本地 `protocolLabel` 定义。
 *  P3 同一批文件里赋值 `transport:` 的，必须经共用的 `transferSummary(`。
 *
 * # 射程外（如实登记）
 *
 * 源码级：证明构造点引用了共用函数，不证明真机上那一格画出来了（后者由各屏渲染门与真机验收管）。
 * 托盘 `tray-node-select.ts` 的短写表是**刻意**不同的一张（窄菜单用 WG/TS/SS），不在本门面内。
 */
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { blankComments } from '@/contracts/comment-blanking.test-support';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { protocolLabel, transferSummary } from '@/components/screens/nodes/nodes-logic';
import { ALL_PROTOCOLS } from '@/domain/server-completeness';
import type { ServerConfig } from '@/contracts/types';

const MOBILE = fileURLToPath(new URL('..', import.meta.url));

function prodFiles(dir: string): string[] {
  const out: string[] = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...prodFiles(p));
    else if (/\.tsx?$/.test(e.name) && !IS_TEST_ONLY_MODULE.test(e.name)) out.push(p);
  }
  return out;
}

const SRC = new Map(prodFiles(MOBILE).map((f) => [relative(MOBILE, f).split('\\').join('/'), blankComments(readFileSync(f, 'utf8'))]));

/** 对象字面量上的赋值（排除 `readonly protocolLabel: string` 这类类型声明）。 */
const assigns = (src: string, field: string): boolean =>
  new RegExp(`\\b${field}\\s*:\\s*(?!string\\b|number\\b)[\\w(\\[]`).test(src.replace(/\breadonly\s+\w+\??\s*:[^;\n]*/g, ''));

const CONSUMERS = [...SRC].filter(([, s]) => assigns(s, 'protocolLabel')).map(([f]) => f).sort();

const SHARED_IMPORT = /import\s*\{[^}]*\bprotocolLabel\b[^}]*\}\s*from\s*'@\/components\/screens\/nodes\/nodes-logic'/;

describe('节点协议名 / 传输摘要：桌面与移动端同源', () => {
  it('P1 共用 `protocolLabel` 覆盖 ALL_PROTOCOLS 每一个协议（不回落原始枚举）', () => {
    expect(ALL_PROTOCOLS.length, 'ALL_PROTOCOLS 塌了').toBeGreaterThan(10);
    const unlabeled = ALL_PROTOCOLS.filter((p) => protocolLabel(p) === p);
    expect(unlabeled, '这些协议没有显示名，角标会渲染原始枚举').toEqual([]);
    // 正面：本批点名的两个协议逐字钉住。
    expect(protocolLabel('masque-client')).toBe('MASQUE');
    expect(protocolLabel('tailcat')).toBe('Tailcat');
    expect(transferSummary({ protocol: 'tailcat' } as ServerConfig)).toBe('derp · wg');
    expect(transferSummary({ protocol: 'masque-client' } as ServerConfig)).toBe('h3 · masque');
  });

  it('Tailscale 只保留协议标签，静态传输摘要缺席；其它端点摘要照常提供', () => {
    expect(protocolLabel('tailscale')).toBe('Tailscale');
    expect(transferSummary({ protocol: 'tailscale' } as ServerConfig)).toBe('');
    expect(transferSummary({ protocol: 'wireguard' } as ServerConfig)).toBe('udp · wg');
    expect(transferSummary({ protocol: 'tailcat' } as ServerConfig)).toBe('derp · wg');
    expect(transferSummary({ protocol: 'masque-client' } as ServerConfig)).toBe('h3 · masque');
  });

  it('P2 移动端每个构造 `protocolLabel` 的点都引用共用函数、不自带表', () => {
    // 取材自检（正面）：节点屏与首页今天都构造这一格，扫描面塌成空集时这里先红。
    expect(CONSUMERS.length, `一个构造点都没扫到：${JSON.stringify(CONSUMERS)}`).toBeGreaterThanOrEqual(2);
    const bad = CONSUMERS.filter((f) => {
      const s = SRC.get(f)!;
      return (
        !SHARED_IMPORT.test(s) ||
        /\bPROTOCOL_LABEL\b/.test(s) ||
        /\b(?:function|const|let)\s+protocolLabel\b/.test(s)
      );
    });
    expect(bad, '这些移动端文件没用共用的 protocolLabel（或自带了一张会漂的副本表）').toEqual([]);
  });

  it('P3 构造 `transport:` 的点经共用 `transferSummary(`', () => {
    const withTransport = CONSUMERS.filter((f) => assigns(SRC.get(f)!, 'transport'));
    expect(withTransport.length, '节点屏的传输摘要构造点没扫到').toBeGreaterThanOrEqual(1);
    const bad = withTransport.filter((f) => !/\btransferSummary\(/.test(SRC.get(f)!));
    expect(bad, '这些文件自己拼传输摘要（MASQUE / Tailcat 那几格会空着）').toEqual([]);
  });
});
