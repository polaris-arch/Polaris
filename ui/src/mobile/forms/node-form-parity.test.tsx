import { MOBILE_FIELD_HINT_SUMMARIES } from './FormFields';
/**
 * 移动端节点表单与桌面 `NodeDialog` 的**能力对差门** + **行为门**（移动端 P1 批）。
 *
 * # 两条判据成对交（缺一半都留着绕行路）
 *
 *  · **源码面**（① ~ ④）：`node-form-parity.test-support.ts` 从桌面 `NodeDialog.tsx` 的 AST 取能力指纹
 *    （共享判据 import / IPC 调用 / 文案键 / 协议分支），移动端 `NodeFormPanel.tsx` 同样取一遍，
 *    差集减登记表 = 债。那一面同时喂进 `wiring-completeness.test.ts` 的 B 面（`report-wiring.sh` 数它）。
 *    它守得住「移动端有没有消费同一个能力」，守不住「消费了但没接对」。
 *  · **行为面**（⑤ ⑥）：把面板**真渲染**出来看字段在不在（逐协议、逐字段，含 MASQUE / Tailcat /
 *    TLS 证书固定），再抓住提交闭包**真提交**一次，看写出去的 `ServerConfig` 对不对
 *    （Tailcat 同宽门拦得住、无地址协议写空地址、extraJson 删掉的键不复活）。
 *
 * # 只换四样，且四样都不是被验对象
 *
 *  · `FormSheet` —— 外壳：换成探针，抓 `onSubmit`，并把 children 原样画出来；
 *  · `FormGroup` —— 折叠段：恒展开（本仓 vitest 是 `environment:'node'`、点不了组头；
 *    被验的是组里的字段，不是折叠这件事 —— 折叠另有 `reveal.test.ts` 与各屏门）；
 *  · `@/ipc` 的 `api.server.add/update/tailcatKeypair` 与 `api.proxy.probeOutbound` —— 被验的是
 *    「**交给它什么**」；
 *  · `protoCodec[*].fromConfig` —— **只在 ⑥-c 一条里**叠一层草稿补丁，模拟「用户在扩展 JSON 里删掉了
 *    一个键」（SSR 下无法真的输入）。`toConfig` 与面板的装配逻辑全是真身；⑥-c 还带一条正面对照
 *    证明补丁真的打上了（不打补丁时那个键**应当**被保留）。
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ReactElement, ReactNode } from 'react';
import { readFileSync } from 'node:fs';

import type { ServerConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';
import { PROTO_OPTIONS, allFields, nodeFormGroups, type NodeProto } from '@/components/dialogs/node-spec';
import { draftFromSpecs } from '@/components/dialogs/field-spec';
import { isAddresslessProtocol } from '@/domain/server-completeness';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';

import {
  DESKTOP_NODE_FORM,
  MOBILE_NODE_FORM,
  NODE_FORM_PARITY,
  capabilityFingerprints,
  desktopNodeFormFingerprints,
  mobileNodeFormFingerprints,
  nodeFormParityDebt,
  resolveNodeFormAnchor,
} from './node-form-parity.test-support';
import { useMobileFormStore } from './form-store';

const probe = vi.hoisted(() => ({
  onSubmit: null as null | (() => void),
  draftPatch: null as null | Record<string, unknown>,
  add: [] as unknown[],
  update: [] as unknown[],
}));

vi.mock('./FormSheet', () => ({
  FormSheet: (props: { onSubmit?: () => void; children?: ReactNode }): ReactElement => {
    probe.onSubmit = props.onSubmit ?? null;
    return <div data-probe="sheet">{props.children}</div>;
  },
}));

vi.mock('./FormGroup', () => ({
  FormGroup: (props: { title: string; groupId?: string; open?: boolean; children?: ReactNode }): ReactElement => (
    <section data-probe="group" data-group-id={props.groupId?.split(':').slice(-1)[0]} data-open={String(props.open)}>
      <h3>{props.title}</h3>
      {props.children}
    </section>
  ),
}));

vi.mock('@/ipc', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/ipc')>();
  return {
    ...real,
    api: {
      ...real.api,
      server: {
        ...real.api.server,
        add: (s: unknown): Promise<void> => {
          probe.add.push(s);
          return Promise.resolve();
        },
        update: (s: unknown): Promise<void> => {
          probe.update.push(s);
          return Promise.resolve();
        },
      },
    },
  };
});

vi.mock('@/components/dialogs/proto-codec', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/components/dialogs/proto-codec')>();
  const wrapped = Object.fromEntries(
    Object.entries(real.protoCodec).map(([proto, codec]) => [
      proto,
      {
        ...codec,
        fromConfig: (s: ServerConfig) => ({ ...codec.fromConfig(s), ...(probe.draftPatch ?? {}) }),
      },
    ]),
  );
  return { ...real, protoCodec: wrapped };
});

const { NodeFormPanel } = await import('./NodeFormPanel');

beforeAll(async () => {
  await i18nReady;
});

/** SSR 读的是 `getInitialState()` 那个对象，改完 store 要同步镜像（同 `mesh-forms.test.tsx`）。 */
function mirror(): void {
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
}

function reset(): void {
  probe.onSubmit = null;
  probe.draftPatch = null;
  probe.add = [];
  probe.update = [];
  useMobileFormStore.getState().closeAll();
  useStagedConfigStore.setState({ entries: [] });
  useAppStore.setState({ servers: [], config: null });
  mirror();
}

beforeEach(reset);
afterEach(reset);

/** 与 renderToStaticMarkup 同一套转义，文案里带引号 / 尖括号时断言才对得上。 */
const esc = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#x27;');
const tx = (key: string): string => {
  const v = i18n.t(key);
  expect(v, `i18n 键 ${key} 没加载（回退成键会让下面的断言变成「键 == 键」）`).not.toBe(key);
  return esc(v);
};

const renderNew = (proto: NodeProto): string =>
  renderToStaticMarkup(<NodeFormPanel instanceId="nf-new" initialProto={proto} />);

const renderEdit = (base: ServerConfig): string => {
  useAppStore.setState({ servers: [base] });
  mirror();
  return renderToStaticMarkup(<NodeFormPanel instanceId="nf-edit" serverId={base.id} />);
};

/** 渲染一次编辑态、调一次真的提交闭包，等它落地。 */
async function submitEdit(base: ServerConfig): Promise<void> {
  renderEdit(base);
  expect(probe.onSubmit, '外壳探针没抓到提交闭包 —— 下面的断言会全部空跑').not.toBeNull();
  probe.onSubmit?.();
  await new Promise((r) => setTimeout(r, 0));
}

/** 合法的 Tailcat 公钥形态（44 字符标准 base64，恰一个 `=`）。 */
const KEY = (c: string): string => `${c.repeat(43)}=`;

// ─────────────────────────────── 源码面 ───────────────────────────────

describe('① 取材面自检（指纹取材不许排成空集，也不许被注释 / type 导入污染）', () => {
  it('桌面指纹含已知的锚点能力 —— 能力被挪出 NodeDialog.tsx 时这里先红', () => {
    const desktop = desktopNodeFormFingerprints();
    for (const id of [
      'api:api.server.update',
      'api:api.server.add',
      'api:api.server.tailcatKeypair',
      'api:api.proxy.probeOutbound',
      'import:protoCodec',
      'import:isAddresslessProtocol',
      'import:tailcatSettingsError',
      'import:nodeFieldGroup',
      'i18n:node.tcChainHint',
      'proto:tailcat',
      'proto:custom',
    ]) {
      expect(desktop.has(id), `${DESKTOP_NODE_FORM} 的指纹里没有 ${id}`).toBe(true);
    }
  });

  it('移动端指纹同样取得到（切点自检：不是恒空）', () => {
    const mobile = mobileNodeFormFingerprints();
    for (const id of ['api:api.server.update', 'import:protoCodec', 'import:nodeFormGroups']) {
      expect(mobile.has(id), `${MOBILE_NODE_FORM} 的指纹里没有 ${id}`).toBe(true);
    }
  });

  it('注释里提到的调用不算消费；type 导入不算能力；协议分支两种写法都认', () => {
    const src = [
      "import type { Foo } from '@/x';",
      "import { type Bar, baz } from './y';",
      "import { useState } from 'react';",
      '// api.server.tailcatKeypair(k)',
      "/* t('node.tcGenerate') */",
      'export function F() {',
      "  if ('tailcat' === proto) api.server.add({});",
      "  return t('node.real');",
      '}',
    ].join('\n');
    const fp = capabilityFingerprints('synthetic.tsx', src);
    expect([...fp].sort()).toEqual(
      ['api:api.server.add', 'i18n:node.real', 'import:baz', 'proto:tailcat'].sort(),
    );
  });
});

describe('② 反向对照（差集判据真的报得出债）', () => {
  it('桌面多一条 ⇒ 报；移动端消费了 ⇒ 不报；登记了 ⇒ 不报', () => {
    const desktop = new Set(['api:api.a', 'api:api.b', 'import:Modal']);
    const mobile = new Set(['api:api.b']);
    expect(nodeFormParityDebt(desktop, mobile).map((d) => d.id)).toEqual([
      'parity:node-form:api:api.a',
    ]);
    expect(nodeFormParityDebt(desktop, mobile, []).map((d) => d.id)).toEqual([
      'parity:node-form:api:api.a',
      'parity:node-form:import:Modal',
    ]);
  });
});

describe('③ 登记表纪律', () => {
  const desktop = desktopNodeFormFingerprints();
  const mobile = mobileNodeFormFingerprints();

  it('每条登记都对得上桌面的一条真实指纹（无僵尸），且移动端并没有同名消费（无冗余）', () => {
    expect(NODE_FORM_PARITY.filter((e) => !desktop.has(e.id)).map((e) => e.id), '僵尸登记').toEqual([]);
    expect(NODE_FORM_PARITY.filter((e) => mobile.has(e.id)).map((e) => e.id), '冗余登记').toEqual([]);
  });

  it('只收 ported，锚落在 ui/src/mobile/ 下、剥注释后真的命中', () => {
    for (const e of NODE_FORM_PARITY) {
      expect(e.disposition.kind, `${e.id} 的处置不是 ported —— 债务不许进这张表`).toBe('ported');
      if (e.disposition.kind !== 'ported') continue;
      const a = e.disposition.mobile;
      expect(a.file.startsWith('ui/src/mobile/'), `${e.id} 的锚不在移动端`).toBe(true);
      const r = resolveNodeFormAnchor(a);
      expect(r.exists, `${a.file} 不存在`).toBe(true);
      expect(r.hit, `${a.file} 里找不到 «${a.mustContain}»（剥注释后）`).toBeGreaterThan(0);
    }
    // 正面对照：一段只在注释里出现的文本不算命中。
    expect(
      resolveNodeFormAnchor({ file: MOBILE_NODE_FORM, mustContain: '唯一一处刻意的形态偏离' }).hit,
    ).toBe(-1);
  });
});

describe('④ 当下对差为空（逐条列出，便于直接定位；`report-wiring.sh` 数的是同一份）', () => {
  it('桌面节点编辑器的每条能力指纹，移动端都消费了或登记了等价物', () => {
    expect(nodeFormParityDebt().map((d) => d.id)).toEqual([]);
  });
});

// ─────────────────────────────── 行为面 ───────────────────────────────

describe('⑤ 渲染：逐协议、逐字段真的画出来（字段表与分组同源 `nodeFormGroups`）', () => {
  const groupOpen = (html: string, id: string): boolean | null => {
    const match = html.match(new RegExp(`data-group-id="${id}" data-open="(true|false)"`));
    return match ? match[1] === 'true' : null;
  };

  it('新增的连接路径默认可见，选配折叠；编辑已启用高级/路由自动展开', () => {
    const vless = renderNew('vless');
    expect(groupOpen(vless, 'basic')).toBe(true);
    expect(groupOpen(vless, 'transport')).toBe(true);
    expect(groupOpen(vless, 'advanced')).toBe(false);

    const openvpn = renderNew('openvpn-client');
    expect(groupOpen(openvpn, 'basic')).toBe(true);
    expect(groupOpen(openvpn, 'routing')).toBe(false);
    expect(groupOpen(openvpn, 'advanced')).toBe(false);

    probe.draftPatch = { meshRoutes: '10.10.0.0/16', extraJson: '{"x":1}' };
    const edited = renderEdit({ id: 'ovpn-existing', name: 'work', protocol: 'openvpn-client', address: 'vpn.example', port: 443 });
    expect(groupOpen(edited, 'routing')).toBe(true);
    expect(groupOpen(edited, 'advanced')).toBe(true);
  });

  it('共享选配控件在高级组内部；已有前置代理会打开该组', () => {
    const html = renderNew('vless');
    const advanced = html.indexOf('data-group-id="advanced"');
    const groupEnd = html.indexOf('</section>', advanced);
    expect(advanced).toBeGreaterThan(-1);
    for (const id of ['mnf-detour', 'mnf-bind']) {
      const field = html.indexOf(`id="${id}"`);
      expect(field, id).toBeGreaterThan(advanced);
      expect(field, id).toBeLessThan(groupEnd);
    }
    const edited = renderEdit({
      id: 'via-existing', name: 'via', protocol: 'vless', address: 'proxy.example', port: 443,
      detour: 'other-node',
    });
    expect(groupOpen(edited, 'advanced')).toBe(true);
  });

  for (const [proto] of PROTO_OPTIONS) {
    it(`${proto}：可见字段的标签与关键说明全部在场；地址行按「无地址协议」取舍`, () => {
      const html = renderNew(proto);
      const draft = draftFromSpecs(allFields(proto));
      const fields = nodeFormGroups(proto).flatMap((g) => g.fields);
      expect(fields.length, `${proto} 的分组里一个字段都没有`).toBeGreaterThan(0);
      for (const f of fields) {
        if (f.when !== undefined && !f.when(draft)) continue;
        expect(html, `${proto}.${f.k} 的标签没渲染`).toContain(tx(f.label));
        if (f.hint !== undefined) expect(html, `${proto}.${f.k} 的说明没渲染`).toContain(tx(MOBILE_FIELD_HINT_SUMMARIES[f.hint] ?? f.hint));
      }
      expect(html.includes('id="mnf-addr"'), `${proto} 的地址行取舍不对`).toBe(!isAddresslessProtocol(proto));
    });
  }

  it('Tailcat：公钥行 + 生成密钥对 + 前置代理专属提示在场，通用链式提示不在', () => {
    const html = renderNew('tailcat');
    expect(html).toContain(tx('node.tcPublicKey'));
    expect(html).toContain(tx('node.tcGenerate'));
    expect(html).toContain(tx('node.tcPublicKeyHint'));
    expect(html).toContain(tx('mobileHelp.tcChain'));
    expect(html).not.toContain(tx('node.chainHint'));
    // DERP 服务器模式的「主机名须与 DERP 证书一致」提示随模式显隐：切到 servers 模式后在场。
    const edit = renderEdit({
      id: 'tc1', name: 'tc', protocol: 'tailcat', address: '', port: 0,
      tailcatSettings: { serverPublicKey: KEY('A'), serverDiscoKey: KEY('B'), derpServers: ['derp.example'] },
    } as ServerConfig);
    expect(edit).toContain(tx('mobileHelp.derpServers'));
    // 私钥已填 ⇒ 按钮文案换成「显示公钥」。
    const withKey = renderEdit({
      id: 'tc2', name: 'tc', protocol: 'tailcat', address: '', port: 0,
      tailcatSettings: { serverPublicKey: KEY('A'), serverDiscoKey: KEY('B'), privateKey: KEY('C'), derpRegion: 1 },
    } as ServerConfig);
    expect(withKey).toContain(tx('node.tcDerivePublic'));
  });

  it('非 Tailcat 协议不画公钥行，链式提示是通用那条', () => {
    const html = renderNew('vless');
    expect(html).not.toContain(tx('node.tcPublicKey'));
    expect(html).toContain(tx('node.chainHint'));
  });

  it('TLS 证书固定：两格与「只比对服务器证书」提示在场（QUIC 协议恒开 TLS，无门）', () => {
    for (const proto of ['hysteria2', 'tuic'] as const) {
      const html = renderNew(proto);
      expect(html, proto).toContain(tx('node.field.certSha256'));
      expect(html, proto).toContain(tx('node.field.certPkSha256'));
      expect(html, proto).toContain(tx('mobileHelp.certPin'));
    }
    // 渲染出来的就是 locale 里这一句（上面逐字比对）；这里钉住 14b4800 加的那半句还在源文案里。
    const zh = JSON.parse(readFileSync(new URL('../../i18n/locales/zh-CN.json', import.meta.url), 'utf8')) as {
      node: { field: { certPinHint: string } };
    };
    expect(zh.node.field.certPinHint, '提示文案里「只比对服务器证书」那一句不见了').toContain('只比对服务器证书');
  });

  it('custom：内核兼容性探测按钮在场，其它协议不画', () => {
    expect(renderNew('custom')).toContain(tx('node.customProbe.test'));
    expect(renderNew('vless')).not.toContain(tx('node.customProbe.test'));
  });
});

describe('⑥ 提交：抓住真的提交闭包，看写出去的 ServerConfig', () => {
  it('a. Tailcat 键形态不合法 ⇒ 同宽门拦下，一次写都不发', async () => {
    await submitEdit({
      id: 'tc-bad', name: 'tc', protocol: 'tailcat', address: '', port: 0,
      tailcatSettings: { serverPublicKey: 'not-base64', serverDiscoKey: KEY('B'), derpRegion: 1 },
    } as ServerConfig);
    expect(probe.update, '不合法的 Tailcat 节点被写出去了 —— store 会把它静默丢掉').toEqual([]);
    expect(probe.add).toEqual([]);
  });

  it('a′. 正面对照：合法 Tailcat 节点照常写出；无地址协议写空地址 + 0 端口', async () => {
    await submitEdit({
      id: 'tc-ok', name: 'tc', protocol: 'tailcat', address: 'stale.example', port: 443,
      tailcatSettings: { serverPublicKey: KEY('A'), serverDiscoKey: KEY('B'), derpRegion: 1 },
    } as ServerConfig);
    expect(probe.update).toHaveLength(1);
    const out = probe.update[0] as ServerConfig;
    expect(out.address).toBe('');
    expect(out.port).toBe(0);
    expect(out.tailcatSettings?.serverPublicKey).toBe(KEY('A'));
  });

  const TOR_BASE = {
    id: 'tor-1', name: 'tor', protocol: 'tor', address: '', port: 0,
    torSettings: { executablePath: '/usr/bin/tor', drop_me: 'd', keep_me: 'k', username: 'x' },
  } as unknown as ServerConfig;
  const torOut = (): Record<string, unknown> =>
    (probe.update[0] as unknown as { torSettings: Record<string, unknown> }).torSettings;

  it('b. 正面对照：不动扩展 JSON 时，袋键原样保留（证明下一条的补丁是唯一变量）', async () => {
    await submitEdit(TOR_BASE);
    expect(probe.update).toHaveLength(1);
    expect(torOut()).toMatchObject({ drop_me: 'd', keep_me: 'k', username: 'x' });
  });

  it('c. 🔴 modeledOf：扩展 JSON 里删掉的键，保存后不从已存节点复活', async () => {
    probe.draftPatch = { extraJson: JSON.stringify({ keep_me: 'k' }) };
    await submitEdit(TOR_BASE);
    expect(probe.update).toHaveLength(1);
    const out = torOut();
    expect(out, '删掉的袋键从已存节点复活了').not.toHaveProperty('drop_me');
    expect(out.keep_me).toBe('k');
    // 两边都不收的建模键（表单不映射、袋外）照常从 base 带过来。
    expect(out.username).toBe('x');
    expect(out.executablePath).toBe('/usr/bin/tor');
  });
});
