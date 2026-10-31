/**
 * SettingsDns 的纯判定单测 —— 三条都是**生产接线点本身**（组件直接调用这些导出函数，不是并行复刻），
 * 故删掉生产代码里的对应判定会让本文件转红，不会出现「测了个自造副本」的假绿。
 *
 *  1. `fakeIpTogglePatch`      —— 契约 L95「手改写 fakeIpTunAutoEnable:false」
 *  2. `needsFakeIpOffConfirm`  —— 契约 L95「TUN ON→OFF 一次性风险确认」
 *  3. `parseDnsServerSpec`     —— 契约 L94「非法标红不落盘」的判定源，须与后端 dns_spec.rs 同口径
 *  4. `normalizeDnsTimeoutInput` —— 与 store/sanitize.rs 的 1..60000 + round 同口径
 */
import { describe, it, expect } from 'vitest';
import {
  fakeIpTogglePatch,
  needsFakeIpOffConfirm,
  normalizeDnsTimeoutInput,
  nextRacePool,
  parseDnsServerSpec,
} from './SettingsDns';
import {
  dnsGroupReferences,
  dnsServerReferences,
} from '../rules/DnsPolicyWorkspace';
import {
  formatHostsPredefined,
  moveDnsGroupMember,
  parseHostsPredefined,
  validateDnsGroupForm,
  validateDnsServerForm,
} from '../../dialogs/DnsResourceDialog';
import {
  DNS_PRESET_CUSTOM,
  domesticPresets,
  remotePresets,
  canEnableRaceUpstream,
  editNodeResolverInventory,
  dnsUpstreamIdentity,
  isPureIpDnsSpec,
  nodeResolverTier1Count,
  trimNodeDnsSpec,
} from './settings-dns-logic';
import type { UserConfig } from '@/contracts/types';

describe('DNS 资源删除引用门', () => {
  const config = {
    dnsRules: [
      {
        id: 'policy-corp',
        remarks: 'Corp',
        enabled: false,
        effects: {
          dns: {
            resolver: 'direct',
            answerMode: 'real',
            action: {
              type: 'hostsFirst',
              hostsServerId: 'hosts-a',
              fallback: { type: 'group', groupId: 'group-a' },
            },
          },
        },
      },
    ],
    dnsServers: [
      { id: 'dns-a', name: 'A', enabled: true, type: 'udp', outbound: { type: 'direct' } },
      {
        id: 'dns-b',
        name: 'B',
        enabled: true,
        type: 'https',
        outbound: { type: 'direct' },
        bootstrapServerId: 'dns-a',
      },
    ],
    dnsServerGroups: [
      {
        id: 'group-a',
        name: 'Race A',
        enabled: true,
        mode: 'race',
        members: ['dns-a'],
      },
    ],
    dnsDefaults: {
      directServerId: 'dns-a',
      proxyServerId: 'dns-b',
      unmatchedAction: { type: 'group', groupId: 'group-a' },
    },
  } as unknown as UserConfig;

  it('Server 被 Group、bootstrap 和默认角色引用时全部列出且去重', () => {
    expect(dnsServerReferences(config, 'dns-a')).toEqual([
      { scope: 'group', name: 'Race A' },
      { scope: 'server', name: 'B' },
      { scope: 'defaults', name: 'direct' },
    ]);
  });

  it('Group 的嵌套 fallback 与默认动作引用都会阻止删除', () => {
    expect(dnsGroupReferences(config, 'group-a')).toEqual([
      { scope: 'policy', name: 'Corp' },
      { scope: 'defaults', name: 'unmatched' },
    ]);
  });
});

describe('Hosts 内联记录', () => {
  it('按行解析、去空值和重复值，并可稳定回显', () => {
    const records = parseHostsPredefined(`a.test = 1.1.1.1, 1.1.1.1\n坏行\nb.test=::1`);
    expect(records).toEqual({ 'a.test': ['1.1.1.1'], 'b.test': ['::1'] });
    expect(formatHostsPredefined(records)).toBe('a.test = 1.1.1.1\nb.test = ::1');
  });
});

describe('DNS 服务器组成员顺序', () => {
  it('上下移动共用稳定重排，越界保持原顺序', () => {
    expect(moveDnsGroupMember(['a', 'b', 'c'], 2, 0)).toEqual(['c', 'a', 'b']);
    expect(moveDnsGroupMember(['a', 'b'], 0, -1)).toEqual(['a', 'b']);
  });
});

describe('DNS 资源表单校验', () => {
  const base = {
    name: 'Resolver',
    type: 'https' as const,
    host: 'dns.example.com',
    port: '443',
    isBootstrap: false,
    bootstrapServerId: 'bootstrap-a',
    validBootstrapServerIds: new Set(['bootstrap-a']),
  };

  it('域名端点必须引用有效 Bootstrap，Bootstrap 自身只能使用 IP 端点', () => {
    expect(validateDnsServerForm(base)).toBeNull();
    expect(validateDnsServerForm({ ...base, bootstrapServerId: '' })).toBe('bootstrapMissing');
    expect(validateDnsServerForm({ ...base, isBootstrap: true })).toBe('bootstrapIp');
    expect(validateDnsServerForm({ ...base, isBootstrap: true, host: '1.1.1.1' })).toBeNull();
  });

  it('端口范围和服务器组必填成员在保存前拦截', () => {
    expect(validateDnsServerForm({ ...base, port: '65536' })).toBe('port');
    expect(validateDnsGroupForm({ name: 'Race', members: [] })).toBe('members');
    expect(validateDnsGroupForm({ name: 'Race', members: ['dns-a'] })).toBeNull();
  });
});

describe('DoH 配置库存与启用池解耦', () => {
  it('Tier1 最多启用 3 个，system 不占额度', () => {
    const custom = [{ id: 'doh-a', spec: 'https://8.8.8.8/q' }, { id: 'doh-b', spec: 'tls://1.1.1.1' }];
    expect(nextRacePool(['ali', 'dnspod', 'doh-a'], 'doh-b', true, custom)).toEqual(['ali', 'dnspod', 'doh-a']);
    expect(nextRacePool(['ali', 'dnspod', 'doh-a'], 'system', true, custom)).toEqual(['ali', 'dnspod', 'doh-a', 'system']);
    expect(nextRacePool(['ali', 'dnspod', 'doh-a'], 'dnspod', false)).toEqual(['ali', 'doh-a']);
  });

  it('explicit row IDs survive edits/reorder; added inventory stays disabled', () => {
    const previous = [{ id: 'a', spec: 'https://1.1.1.1/dns-query' }, { id: 'b', spec: 'https://8.8.8.8/dns-query' }];
    const result = editNodeResolverInventory(previous, ['a'], [previous[1], { id: 'a', spec: 'https://9.9.9.9/dns-query' }, { spec: 'tls://1.0.0.1:853' }], () => 'new-1');
    expect(result).toEqual({ ok: true, custom: [previous[1], { id: 'a', spec: 'https://9.9.9.9/dns-query' }, { id: 'new-1', spec: 'tls://1.0.0.1:853' }], pool: ['a'] });
    const inserted = editNodeResolverInventory(previous, [], [{ spec: 'https://9.9.9.9/dns-query' }, ...previous], () => 'new-head');
    expect(inserted.ok && inserted.custom.map(item => item.id)).toEqual(['new-head', 'a', 'b']);
  });
});

describe('fakeIpTogglePatch', () => {
  it('打开 FakeIP 时同写 fakeIpTunAutoEnable:false（消费一次性自动纠正资格）', () => {
    expect(fakeIpTogglePatch(true)).toEqual({
      enableFakeIp: true,
      fakeIpTunAutoEnable: false,
    });
  });

  it('关闭 FakeIP 时同写 fakeIpTunAutoEnable:false（这正是契约点名的场景：迁移用户手动关闭）', () => {
    expect(fakeIpTogglePatch(false)).toEqual({
      enableFakeIp: false,
      fakeIpTunAutoEnable: false,
    });
  });
});

describe('needsFakeIpOffConfirm', () => {
  it('TUN 下关闭 → 需要确认（节点将收真实 IP，机场可能拒连且客户端无法缓解）', () => {
    expect(needsFakeIpOffConfirm(false, 'tun')).toBe(true);
  });

  it('TUN 下开启 → 不确认（开启无风险）', () => {
    expect(needsFakeIpOffConfirm(true, 'tun')).toBe(false);
  });

  it.each(['systemProxy', 'manual', undefined])('非 TUN(%s) 关闭 → 不确认', (mode) => {
    expect(needsFakeIpOffConfirm(false, mode)).toBe(false);
  });
});

describe('parseDnsServerSpec（与后端 crates/config-engine/.../dns_spec.rs 同口径）', () => {
  it.each([
    ['https://1.1.1.1/dns-query', false],
    ['https://cloudflare-dns.com/dns-query', true],
    ['https://[2606:4700:4700::1111]/dns-query', false],
    ['tls://223.5.5.5:853', false],
    ['tls://dot.pub', true],
    ['udp://8.8.8.8', false],
    ['223.5.5.5', false],
    ['[2001:db8::1]', false],
    ['2001:db8::1', false],
  ])('%s 合法，isDomain=%s', (spec, isDomain) => {
    expect(parseDnsServerSpec(spec)).toEqual(expect.objectContaining({ isDomain }));
  });

  it.each([
    '',
    '   ',
    'doh.pub', // 裸域名：后端 parse_dns_server_spec 同样返回 None
    '8.8.8.8:53', // 无 scheme 的 IP:port —— 后端不接受，故不能在 UI 放行
    'https://1.1.1.1:70000/dns-query', // 端口越界
    'https://1.1.1.1:abc/dns-query', // 端口非数字
    'https:/1.1.1.1/dns-query', // 缺一个斜杠
    'https://', // 空 host
    'ftp://1.1.1.1',
  ])('%s 非法 → null（标红且不落盘）', (spec) => {
    expect(parseDnsServerSpec(spec)).toBeNull();
  });

  it('undefined / null 视为非法', () => {
    expect(parseDnsServerSpec(undefined)).toBeNull();
    expect(parseDnsServerSpec(null)).toBeNull();
  });

  it('IPv4 逐段 ≤255（256 判域名而非 IP，与后端 is_ipv4_segment 一致）', () => {
    expect(parseDnsServerSpec('https://256.1.1.1/dns-query')).toEqual(
      expect.objectContaining({ isDomain: true }),
    );
    expect(parseDnsServerSpec('256.1.1.1')).toBeNull(); // 裸形态既非 IP 也非可解析 spec
  });
});

describe('normalizeDnsTimeoutInput（与 crates/store/src/sanitize.rs:498-517 同口径）', () => {
  it('空 / 纯空格 → 删字段（用内核默认）', () => {
    expect(normalizeDnsTimeoutInput('')).toEqual({ value: undefined });
    expect(normalizeDnsTimeoutInput('   ')).toEqual({ value: undefined });
  });

  it.each([
    ['1', 1],
    ['5000', 5000],
    ['60000', 60000],
    ['1500.6', 1501], // 非整数四舍五入，对齐 sanitize 的 n.round()
  ])('%s → %s', (raw, value) => {
    expect(normalizeDnsTimeoutInput(raw)).toEqual({ value });
  });

  it.each(['0', '-1', '60001', 'abc', 'NaN', 'Infinity'])(
    '%s 越界/非数值 → null（标红且不落盘，避免后端静默删字段）',
    (raw) => {
      expect(normalizeDnsTimeoutInput(raw)).toBeNull();
    },
  );
});

/**
 * 上游预设表的**内容门**（2026-09-06 新增）。
 *
 * # 补的是哪条缝
 *
 * `remotePresets` / `domesticPresets` 2026-09-06 从 `SettingsDns.tsx` 的私有函数提升成
 * `settings-dns-logic.ts` 的**共享面**（移动端 `mobile/settings/DnsPage.tsx` 读同一张表）。
 * 但当时仓里判这张表的三条判据**全都随表一起漂**：
 *  · 移动端渲染断言写的是 `for (const preset of remotePresets(...)) expect(html).toContain(preset.value)`
 *    —— 读的是表自己，改表则期望同步改；
 *  · 「表只有一份」那条扫描只认两条针字面量，且只数**几个文件持有**，不判值对不对；
 *  · 桌面侧一条都没有。
 * 实测：把 `settings-dns-logic.ts:215` 的 `value: 'https://8.8.8.8/dns-query'` 改成
 * `'https://8.8.4.4/dns-query'`（label 仍写着 `Google 8.8.8.8`，即值与标签当场互相矛盾），
 * `npx vitest run` 全量 **全绿**。⇒ 一个改错的上游地址会同时发到两端而没有任何门转红，
 * 用户在两端选中「Google 8.8.8.8」，落进 `dnsConfig.foreignDns` 的却是另一个地址。
 *
 * # 判据的取材：**逐字手写字面量**，不从 `remotePresets()` 反读
 *
 * 从被判对象自己身上取期望值就是自证。故下面两张表是手抄的，改表必须同步改这里 ——
 * 那正是「共享面要有人签字」这件事的机器形态。
 */
describe('DNS 上游预设表：内容门（共享面，改表必须同时过两端的门）', () => {
  /** i18n 桩：返回 key 本身 ⇒ label 的前缀就是那两个类型词的 key，可直接判类型归属。 */
  const T = (key: string): string => key;

  it('远程预设：四条、顺序与地址逐字不变', () => {
    expect(remotePresets(T).map((p) => p.value)).toEqual([
      'https://1.1.1.1/dns-query',
      'https://8.8.8.8/dns-query',
      'https://cloudflare-dns.com/dns-query',
      'https://dns.google/dns-query',
    ]);
  });

  it('国内预设：四条、顺序与地址逐字不变', () => {
    expect(domesticPresets(T).map((p) => p.value)).toEqual([
      'https://223.5.5.5/dns-query',
      'https://1.12.12.12/dns-query',
      'https://doh.pub/dns-query',
      'https://dns.alidns.com/dns-query',
    ]);
  });

  /**
   * label 与 value 不许各走各的。两条不变量：
   *  ① label 里印的那个地址，必须**就是** value 里的那个 host（`parseDnsServerSpec` 解出来的，
   *     不是这里再写一遍正则）——这条直接抓「值改了、标签没改」；
   *  ② label 开头那个类型词（IP DoH / 域名 DoH）必须与 value 的真实形态一致
   *     ——这条抓「把一条域名 DoH 归进 IP DoH 那一组」，那会让 bootstrap 那一段判错。
   */
  function labelAgreesWithValue(preset: { value: string; label: string }): boolean {
    const parsed = parseDnsServerSpec(preset.value);
    if (!parsed) return false;
    if (!preset.label.includes(parsed.server)) return false;
    const claimsIp = preset.label.startsWith('settings.dns.dohByIp');
    const claimsDomain = preset.label.startsWith('settings.dns.dohByDomain');
    if (claimsIp === claimsDomain) return false; // 两个都不是（或都是）⇒ 类型词丢了
    return claimsIp === !parsed.isDomain;
  }

  it('每一条 label 印的地址就是它 value 里的那个，且类型词与 value 的真实形态一致', () => {
    for (const preset of [...remotePresets(T), ...domesticPresets(T)]) {
      expect(labelAgreesWithValue(preset), `预设「${preset.label}」的标签与落库值对不上`).toBe(
        true,
      );
    }
  });

  it('反向对照：谓词认得出「值改了、标签没改」与「类型词归错组」', () => {
    // 这就是实测过的那条静默变异：标签仍写 8.8.8.8，落库的是 8.8.4.4。
    expect(
      labelAgreesWithValue({
        value: 'https://8.8.4.4/dns-query',
        label: 'settings.dns.dohByIp · Google 8.8.8.8',
      }),
      '值与标签矛盾都没抓出来 ⇒ 上一条是恒 true 的谓词',
    ).toBe(false);
    // 域名 DoH 被归进 IP DoH 那一组。
    expect(
      labelAgreesWithValue({
        value: 'https://dns.google/dns-query',
        label: 'settings.dns.dohByIp · dns.google',
      }),
    ).toBe(false);
    // 正向对照：一条真的对得上的预设必须判 true（否则上面两条可能只是「它对谁都 false」）。
    expect(
      labelAgreesWithValue({
        value: 'https://dns.google/dns-query',
        label: 'settings.dns.dohByDomain · dns.google',
      }),
    ).toBe(true);
  });

  it('哨兵不许混进表里（`__custom__` 是显示态，不是可落库的上游）', () => {
    for (const preset of [...remotePresets(T), ...domesticPresets(T)]) {
      expect(preset.value).not.toBe(DNS_PRESET_CUSTOM);
      // 且每条都必须是这个应用真的解析得了的规格，否则选中即静默回落到兜底上游。
      expect(parseDnsServerSpec(preset.value), `「${preset.value}」解析不出来`).not.toBeNull();
    }
  });

  it('两张表不许有交集（同一个上游同时挂在国内与国外两侧 = 选哪边都一样）', () => {
    const remote = new Set(remotePresets(T).map((p) => p.value));
    const overlap = domesticPresets(T)
      .map((p) => p.value)
      .filter((v) => remote.has(v));
    expect(overlap).toEqual([]);
  });

  it('语言切换后重算：表是**函数**不是模块级常量（常量在 import 期求值，那时语言还没校正）', () => {
    const zh = remotePresets(() => 'zh');
    const en = remotePresets(() => 'en');
    expect(zh[0].label).not.toBe(en[0].label);
    // 但 value 不随语言变（落库的是地址，不是文案）。
    expect(zh.map((p) => p.value)).toEqual(en.map((p) => p.value));
  });
});


describe('N1 case-sensitive DNS upstream identity', () => {
  it('canonicalizes authority without changing path/query data', () => {
    expect(dnsUpstreamIdentity('https://DNS.EXAMPLE:443/Query?Token=A')).toBe(dnsUpstreamIdentity('https://dns.example/Query?Token=A'));
    expect(new Set(['https://dns.example/Query?Token=A', 'https://dns.example/query?Token=A', 'https://dns.example/Query?Token=a'].map(dnsUpstreamIdentity)).size).toBe(3);
  });
});

describe('pure-IP native DoT inventory and encrypted quota', () => {
  it.each(['tls://1.1.1.1', 'tls://1.1.1.1:65535', 'tls://[2001:0DB8::1]:853'])('admits %s', spec => {
    expect(isPureIpDnsSpec(spec)).toBe(true);
  });
  it.each(['tls://resolver.example', 'tls://1.1.1.1:0', 'tls://1.1.1.1:65536', 'tls://1.1.1.1/path', 'tls://1.1.1.1?sni=x', 'tls://user@1.1.1.1', 'tls://[::1]tail', 'tls://[::1]:', 'tls://[1.1.1.1]', 'tls://[:::]'])('rejects %s', spec => {
    expect(isPureIpDnsSpec(spec)).toBe(false);
  });
  it('deduplicates default ports and normalized IPs but preserves protocol/path/query data', () => {
    expect(dnsUpstreamIdentity('tls://[2001:0db8:0:0:0:0:0:1]')).toBe(dnsUpstreamIdentity('tls://[2001:db8::1]:0853'));
    expect(dnsUpstreamIdentity('tls://1.1.1.1')).toBe(dnsUpstreamIdentity('tls://1.1.1.1:853'));
    expect(dnsUpstreamIdentity('tls://1.1.1.1:853')).not.toBe(dnsUpstreamIdentity('https://1.1.1.1:853/q'));
    expect(dnsUpstreamIdentity('https://1.1.1.1/Query?Token=A')).not.toBe(dnsUpstreamIdentity('https://1.1.1.1/query?Token=A'));
    expect(dnsUpstreamIdentity('https://1.1.1.1/q?Token=A')).not.toBe(dnsUpstreamIdentity('https://1.1.1.1/q?Token=a'));
  });
  it('shares the quota across DoH/DoT while UDP/system and equivalent entries are free', () => {
    const custom = [
      { id: 'dot', spec: 'tls://1.1.1.1' }, { id: 'dup', spec: 'tls://1.1.1.1:853' },
      { id: 'udp', spec: 'udp://9.9.9.9:5353' }, { id: 'extra', spec: 'tls://8.8.8.8' },
      { id: 'bad', spec: 'tls://resolver.example' },
    ];
    const pool = ['ali', 'dnspod', 'dot'];
    expect(nodeResolverTier1Count(pool, custom)).toBe(3);
    expect(canEnableRaceUpstream(pool, 'extra', custom)).toBe(false);
    for (const id of ['dup', 'udp', 'system']) expect(canEnableRaceUpstream(pool, id, custom)).toBe(true);
    expect(nextRacePool(pool, 'extra', true, custom)).toEqual(pool);
    expect(nextRacePool(pool, 'udp', true, custom)).toEqual([...pool, 'udp']);
    expect(nextRacePool(pool, 'bad', true, custom)).toEqual(pool);
    expect(nextRacePool([...pool, 'bad'], 'bad', false, custom)).toEqual(pool);
  });
});

describe('identified node resolver inventory save transaction', () => {
  it('deletes the selected source without enabling pre-existing duplicate targets', () => {
    const previous = [{ id: 'a', spec: 'tls://1.1.1.1' }, { id: 'b', spec: 'tls://9.9.9.9' }, { id: 'c', spec: 'tls://9.9.9.9:853' }];
    expect(editNodeResolverInventory(previous, ['a'], previous.slice(1), () => 'new')).toEqual({ ok: true, custom: [previous[1]], pool: [] });
  });
  it.each([
    ['tls://1.1.1.1', 'tls://1.1.1.1'],
    [' tls://1.1.1.1 ', 'tls://1.1.1.1'],
    ['tls://1.1.1.1', 'tls://1.1.1.1:853'],
    ['tls://[2001:0db8::1]', 'tls://[2001:db8::1]:853'],
  ])('preserves either explicitly edited duplicate row: %s / %s', (left, right) => {
    const previous = [{ id: 'a', spec: left }, { id: 'b', spec: right }];
    for (const selected of ['a', 'b']) {
      const rows = previous.map(item => ({ ...item, spec: item.id === selected ? 'tls://9.9.9.9' : item.spec }));
      const edited = editNodeResolverInventory(previous, [selected], rows, () => 'new');
      expect(edited).toMatchObject({ ok: true, pool: [selected] });
      if (!edited.ok) throw new Error('valid edit rejected');
      expect(edited.custom.find(item => item.id === selected)?.spec).toBe('tls://9.9.9.9');
      const saved = JSON.parse(JSON.stringify(edited));
      expect(editNodeResolverInventory(saved.custom, saved.pool, saved.custom, () => 'new')).toEqual(edited);
      const deleted = editNodeResolverInventory(previous, [selected], previous.filter(item => item.id !== selected), () => 'new');
      expect(deleted).toMatchObject({ ok: true, pool: [] });
    }
  });
  it('maps only surviving aliases and retains selected order through reorder and unrelated edits', () => {
    const previous = [{ id: 'a', spec: 'https://1.1.1.1' }, { id: 'b', spec: 'https://1.1.1.1/dns-query' }, { id: 'u', spec: 'udp://9.9.9.9' }];
    const result = editNodeResolverInventory(previous, ['a', 'u', 'b', 'ali'], [previous[2], previous[1], previous[0], { spec: 'udp://8.8.8.8' }], () => 'new');
    expect(result).toEqual({ ok: true, custom: [previous[2], previous[1], { id: 'new', spec: 'udp://8.8.8.8' }], pool: ['b', 'u', 'ali'] });
    const changed = editNodeResolverInventory(previous, ['b'], [previous[0], previous[1], { id: 'u', spec: 'udp://9.9.9.9:5353' }], () => 'new');
    expect(changed).toMatchObject({ ok: true, pool: ['a'] });
  });
  it('explicit edits into earlier/later existing endpoints differ from deletion', () => {
    const previous = [{ id: 'a', spec: 'tls://1.1.1.1' }, { id: 'b', spec: 'tls://9.9.9.9' }];
    for (const selected of ['a', 'b']) {
      const target = previous.find(item => item.id !== selected)!;
      expect(editNodeResolverInventory(previous, [selected], previous.map(item => ({ ...item, spec: target.spec })), () => 'new')).toMatchObject({ ok: true, pool: ['a'] });
      expect(editNodeResolverInventory(previous, [selected], [target], () => 'new')).toMatchObject({ ok: true, pool: [] });
    }
  });
  it('validates the complete edited quota before save and never mutates old data on rejection', () => {
    for (const spec of ['udp://9.9.9.9', 'tls://1.1.1.1:853']) {
      const previous = [{ id: 'dot', spec: 'tls://1.1.1.1' }, { id: 'u', spec }];
      const pool = ['ali', 'dnspod', 'dot', 'u'];
      const before = JSON.stringify({ previous, pool });
      const rows = [previous[0], { id: 'u', spec: 'tls://9.9.9.9' }];
      expect(editNodeResolverInventory(previous, pool, rows, () => 'new')).toEqual({ ok: false, error: 'quota' });
      expect(JSON.stringify({ previous, pool })).toBe(before);
      expect(editNodeResolverInventory(previous, ['ali', 'dnspod', 'dot'], rows, () => 'new')).toMatchObject({ ok: true, pool: ['ali', 'dnspod', 'dot'] });
      expect(editNodeResolverInventory(previous, pool, [previous[0]], () => 'new')).toMatchObject({ ok: true, pool: ['ali', 'dnspod', 'dot'] });
    }
  });
  it('keeps free Tier2/equivalent edits and inactive malformed inventory editable', () => {
    const previous = [{ id: 'dot', spec: 'tls://1.1.1.1' }, { id: 'u', spec: 'udp://9.9.9.9' }, { id: 'bad', spec: 'tls://resolver.example' }];
    const pool = ['ali', 'dnspod', 'dot', 'u', 'system'];
    expect(editNodeResolverInventory(previous, pool, [{ id: 'dot', spec: 'tls://1.1.1.1:853' }, previous[1], previous[2], { spec: 'tls://8.8.8.8' }], () => 'new')).toMatchObject({ ok: true, pool });
    expect(editNodeResolverInventory(previous, [...pool, 'bad'], previous, () => 'new')).toEqual({ ok: false, error: 'invalid' });
    expect(editNodeResolverInventory([], [], [{ spec: 'tls://resolver.example' }], () => 'new')).toEqual({ ok: false, error: 'invalid' });
    expect(editNodeResolverInventory(previous, pool, [previous[0], previous[1], { id: 'bad', spec: 'tls://8.8.8.8' }], () => 'new')).toMatchObject({ ok: true, pool });
  });
  it('uses builtin-first effective lookup for shadowed malformed and valid custom rows', () => {
    const shadowed = [{ id: 'ali', spec: 'tls://resolver.example' }];
    expect(editNodeResolverInventory(shadowed, ['ali'], [...shadowed, { spec: 'udp://9.9.9.9' }], () => 'new')).toEqual({ ok: true, custom: [...shadowed, { id: 'new', spec: 'udp://9.9.9.9' }], pool: ['ali'] });
    expect(editNodeResolverInventory(shadowed, ['ali'], [], () => 'new')).toEqual({ ok: true, custom: [], pool: ['ali'] });
    const valid = [{ id: 'ali', spec: 'tls://1.1.1.1' }, { id: 'custom', spec: 'tls://1.1.1.1:853' }];
    expect(editNodeResolverInventory(valid, ['custom', 'ali'], valid, () => 'new')).toEqual({ ok: true, custom: valid, pool: ['custom', 'ali'] });
  });
  it('rejects stale or duplicate supplied IDs rather than guessing a source row', () => {
    const previous = [{ id: 'a', spec: 'tls://1.1.1.1' }];
    expect(editNodeResolverInventory(previous, ['a'], [{ id: 'unknown', spec: previous[0].spec }], () => 'new')).toEqual({ ok: false, error: 'invalid' });
    expect(editNodeResolverInventory(previous, ['a'], [previous[0], previous[0]], () => 'new')).toEqual({ ok: false, error: 'invalid' });
  });
});

describe('raw node authority and common Unicode edge admission', () => {
  it.each(['https', 'tls', 'udp'])('rejects internal whitespace/control in %s authority', scheme => {
    for (const authority of ['1.1.1.1\t:853', '1.1.1.1\n:853', '1.1.1.1\r:853', '1.1.1.1 :853', '1.1.1.1\u00a0:853', '1.1.1.1\u007f:853', '1.1.1.1\u0085:853', '1.1.1.1\ufeff:853', '[2001:db8::1\t]:853', '1.1.1.1:8\t53']) {
      const spec = `${scheme}://${authority}${scheme === 'https' ? '/Query?Token=A%20B' : ''}`;
      expect(isPureIpDnsSpec(spec)).toBe(false);
      expect(editNodeResolverInventory([], [], [{ spec }], () => 'new')).toEqual({ ok: false, error: 'invalid' });
    }
  });
  it.each([0x9,0xa,0xb,0xc,0xd,0x20,0x85,0xa0,0x1680,0x2000,0x2001,0x2002,0x2003,0x2004,0x2005,0x2006,0x2007,0x2008,0x2009,0x200a,0x2028,0x2029,0x202f,0x205f,0x3000,0xfeff].map(value => String.fromCodePoint(value)))('normalizes the same edge set across saved inventory toggle/reload: %j', edge => {
    for (const spec of ['https://1.1.1.1/Query?Token=A%20B', 'tls://1.1.1.1', 'udp://1.1.1.1', '1.1.1.1', '[2001:db8::1]']) {
      for (const raw of [edge + spec, spec + edge, edge + spec + edge]) {
        expect(trimNodeDnsSpec(raw)).toBe(spec);
        expect(isPureIpDnsSpec(raw)).toBe(true);
        const previous = [{ id: 'saved', spec: raw }];
        const pool = nextRacePool([], 'saved', true, previous);
        expect(pool).toEqual(['saved']); // Toggle keeps raw bytes; native accepts the same edges.
        const result = editNodeResolverInventory(previous, pool, previous, () => 'new');
        expect(result).toEqual({ ok: true, custom: [{ id: 'saved', spec }], pool });
      }
    }
  });
});
