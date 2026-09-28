/**
 * 移动端**表单宿主**的门（批 2 / W-00）。
 *
 * 三块判据面，逐块都有自检：
 *  · **纯函数** —— 栈语义（`form-store.ts`）与「这一行编辑得动吗」（`mobileEditFormFor`），
 *    不碰 DOM、不碰 React，直接驱动；
 *  · **源码扫描** —— union 的分支集合与宿主 `switch` 的 case 集合**恰等**。tsc 的 `never` 兜底
 *    已经在编译期挡住「加一支而不加 case」，但它挡不住**反过来**那一手：把兜底改成
 *    `default: return null`，从此每加一支 union 都静默渲染成空（点了什么也不发生）。
 *    本组钉的正是那个形态。
 *  · **跨屏跳转**（`navigate.ts`）—— 装载器不在时必须返回 `false`，让调用方如实告知。
 *
 * # 射程自曝
 *
 * 六个面板自己**渲染不动**：它们内部 `useTranslation()`，而本仓 vitest 是 `environment:'node'`、
 * 刻意不装 jsdom，也不在测试里搭 i18n。故「表单在真机上长什么样、键盘弹起时挡不挡得住提交键」
 * 归真机验收；这里覆盖的是栈、分派、与两条纯判据。
 * 「弹层吃不吃系统返回键」由 `../back-navigation.test.tsx` 的遮罩恰等快照与登记点快照守着。
 */

import { describe, expect, it, beforeEach } from 'vitest';
import * as tsc from '@/test/ts-compiler';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { ServerConfig } from '@/contracts/types';
import { PROTO_OPTIONS } from '@/components/dialogs/node-spec';
import { installMobileNavigator, navigateMobile } from '../navigate';
import { setPushedPage, usePushedPage } from '../MobileShell';
import {
  closeMobileForm,
  mobileEditFormFor,
  openMobileForm,
  useMobileFormStore,
} from './form-store';

const read = (rel: string): string =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');

/** 剥注释（保长度）：头注里写满了 `case '…'` 这类示例，不剥会把说明当成实现。 */
function strip(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/\/\/[^\n]*/g, (m) => m.replace(/[^\n]/g, ' '));
}

/**
 * `protocol` 收 `string` 而不是 `Protocol`：本判据**故意**要喂 `'VLESS'` / `'WIREGUARD'` 这类
 * 大小写变体 —— 订阅与存量配置里的协议大小写不归前端管，而 `mobileEditFormFor` 的判据正是
 * 「小写化之后是不是那两个」。用类型把这批输入挡在门外，等于把这条判据要验的东西删掉。
 */
const server = (protocol = 'vless', over: Partial<ServerConfig> = {}): ServerConfig =>
  ({
    id: 's1',
    name: 'n',
    protocol,
    address: 'a',
    port: 443,
    ...over,
  }) as unknown as ServerConfig;

beforeEach(() => {
  useMobileFormStore.getState().closeAll();
});

describe('① 栈语义（与桌面 dialog-store 同形：push / pop / 按身份关闭）', () => {
  it('自检：起手是空栈，且 `closeAll` 真的排得空（否则下面每条都在别人的残留上跑）', () => {
    expect(useMobileFormStore.getState().stack).toEqual([]);
  });

  it('open 返回稳定身份，叠上去的在栈顶', () => {
    const a = openMobileForm({ kind: 'node' });
    const b = openMobileForm({ kind: 'import' });
    const stack = useMobileFormStore.getState().stack;
    expect(stack.map((e) => e.kind)).toEqual(['node', 'import']);
    expect(stack.map((e) => e.instanceId)).toEqual([a, b]);
    expect(a, '两次 open 拿到了同一个身份 —— 异步回调会关错层').not.toBe(b);
  });

  it('`closeInstance` 关的是**自己那一层**，不是栈顶（异步 continuation 的唯一出口）', () => {
    const a = openMobileForm({ kind: 'node' });
    openMobileForm({ kind: 'confirm', payload: { title: 't', message: 'm', onConfirm: () => {} } });
    closeMobileForm(a); // 关的是**底下**那一层
    expect(useMobileFormStore.getState().stack.map((e) => e.kind)).toEqual(['confirm']);
    // 反向对照：`close()` 关的确实是栈顶（两条语义不同，用它证明上一条不是巧合）。
    const c = openMobileForm({ kind: 'import' });
    useMobileFormStore.getState().close();
    expect(useMobileFormStore.getState().stack.some((e) => e.instanceId === c)).toBe(false);
  });

  it('`hasInstance` 认得出在场与已关（异步回调据它决定还要不要 setState）', () => {
    const a = openMobileForm({ kind: 'mesh-join' });
    expect(useMobileFormStore.getState().hasInstance(a)).toBe(true);
    closeMobileForm(a);
    expect(useMobileFormStore.getState().hasInstance(a)).toBe(false);
    // 幂等：再关一次不炸、也不误伤别人。
    const b = openMobileForm({ kind: 'mesh-join' });
    closeMobileForm(a);
    expect(useMobileFormStore.getState().hasInstance(b)).toBe(true);
  });

  it('载荷原样带过（`kind` 之外的字段不许在入栈时被吃掉）', () => {
    openMobileForm({ kind: 'sub', subId: 'sub-9', focus: 'url' });
    const stack = useMobileFormStore.getState().stack;
    const top = stack[stack.length - 1];
    expect(top).toMatchObject({ kind: 'sub', subId: 'sub-9', focus: 'url' });
  });
});

describe('② union 的每一支在宿主里都有 case（`never` 兜底不许被改成静默 return null）', () => {
  const STORE_SRC = strip(read('./form-store.ts'));
  const HOST_SRC = strip(read('./MobileFormHost.tsx'));

  /** union 分支：`| { kind: 'x'` 的字面量。 */
  const unionKinds = [...STORE_SRC.matchAll(/\|\s*\{\s*kind:\s*'([a-z-]+)'/g)].map((m) => m[1]).sort();
  /** 宿主的 case 标签。 */
  const hostCases = [...HOST_SRC.matchAll(/case\s+'([a-z-]+)':/g)].map((m) => m[1]).sort();

  it('自检：两个取材面都解析出来了，且量级合理（任一为空会让下面那条恒绿）', () => {
    expect(unionKinds.length, 'union 一支都没解析出来 —— 写法变了？').toBeGreaterThan(3);
    expect(hostCases.length, '宿主一个 case 都没解析出来 —— 写法变了？').toBeGreaterThan(3);
  });

  it('union 分支集合 == 宿主 case 集合（多一支少一个 case 都红）', () => {
    expect(hostCases, 'union 与宿主的 switch 对不上').toEqual(unionKinds);
  });

  it('兜底是 `never`，不是 `return null`（后者会让新增的 kind 静默渲染成空）', () => {
    expect(HOST_SRC, '宿主的穷尽兜底不见了 —— 加一支 union 将不再编译不过').toContain(
      'const never: never = entry',
    );
    expect(HOST_SRC, '兜底退化成了静默返回').not.toMatch(/default:\s*\n?\s*return null;/);
  });

  /**
   * 恰等快照。**不是「包含」** —— 包含式断言会被「顺手删掉一支」骗过，而删掉的那一支恰恰是
   * 某条入口点开之后什么都不发生的形态。批 3 新增四支（组网三张表 + Tailscale 设置表）；
   * 批 13 新增 `rule`（连接行的「新建规则」，`RuleFormPanel`）。
   */
  it('落地的十九个 kind 一个不少一个不多（少一个 = 那条入口点开是空的）', () => {
    /* 2026-09-13（批 10 / 规则屏）加了六支：规则表、DNS 服务器 / 分组两张表、
       添加自定义应用、资源目录、按 URL 下载。加一支而不补 case ⇒ 宿主的 `never` 兜底编译不过，
       这条断言守的是另一半：**加了 case 却忘了在这里登记**（那时没有任何东西会红）。
       2026-09-13（批 16）加了 `taildrop`：Taildrop 收件箱，入口在 `TsSettingsPanel` 的
       账号级动作那一行，**必须带 `serverId`**（Tailscale 已不是单例）。
       2026-09-25（ζ 批 A8）加了 `sub-create-task`：订阅创建操作的恢复面，只由应用级水合
       （`app-wiring.ts`）与订阅表单的发布失败交接打开，**必须带 `operationId`**。
       2026-09-25（网络场景移动端批）加了 `network-profile`：场景表单（新建 / 编辑），入口在规则屏
       「网络场景」二级页与规则表单「生效网络」的「新建场景…」（后者带 `onSaved` 回填）。 */
    expect(unionKinds).toEqual(
      [
        'app-add',
        'confirm',
        'dns-group',
        'dns-server',
        'import',
        'mesh-join',
        'network-profile',
        'node',
        'res-catalog',
        'res-url',
        'rule',
        'sub',
        'sub-create-task',
        'taildrop',
        'ts-exit',
        'ts-login',
        'ts-settings',
        'warp',
        'wg',
      ].sort(),
    );
  });
});

/**
 * ③ `mobileEditFormFor`：这一行该开哪张表。
 *
 * 🔴 **批 3 换了形态**：上一版断言 wireguard / tailscale（含 WARP）返回 `null`（= 那三张表还没移植）。
 * 三张表本批落地之后 `null` 那一档不存在了，判据随之从「返回 null」换成**更强的正面等式**：
 * 每一族都必须落到**确定的那一支**上，且那一支宿主真的有 case。
 *
 * ⚠️ 只断言「不为 null」是**更弱**的写法：把 wireguard 错分到通用节点表也满足它，
 * 而那一条恰恰是 `node-edit-routing.ts` 头注里写着会**丢数据**的路径
 * （codecBase 退化 ⇒ `wireguardSettings` 被整体丢弃后覆盖写回）。
 */
describe('③ `mobileEditFormFor`：这一行该开哪张表', () => {
  const HOST_SRC = strip(read('./MobileFormHost.tsx'));
  const hostCases = new Set(
    [...HOST_SRC.matchAll(/case\s+'([a-z-]+)':/g)].map((m) => m[1]),
  );

  it('`ND_SPEC` 建模的每一个协议都落节点表单（正面断言，不是「没出现 X」）', () => {
    for (const [proto] of PROTO_OPTIONS) {
      expect(
        mobileEditFormFor(server(proto)),
        `${proto} 在 ND_SPEC 里，却没落到通用节点表`,
      ).toEqual({ kind: 'node', serverId: 's1' });
    }
    // 自检：协议表非空且有量级（空表会让上面那个循环恒真）。
    expect(PROTO_OPTIONS.length).toBeGreaterThan(10);
  });

  it('三个组网族各落各的专属表（错分到通用节点表 = 丢 wireguardSettings/tailscaleSettings）', () => {
    expect(mobileEditFormFor(server('wireguard'))).toEqual({ kind: 'wg', serverId: 's1' });
    /* Tailscale 已不是单例 ⇒ 设置表必须携 id（不携时面板只会自查到任意一个节点）。 */
    expect(mobileEditFormFor(server('tailscale'))).toEqual({ kind: 'ts-settings', serverId: 's1' });
    expect(
      mobileEditFormFor({
        ...server('wireguard'),
        wireguardSettings: {
          privateKey: 'k',
          localAddress: ['10.0.0.2/32'],
          peerPublicKey: 'p',
          warpDevice: { deviceId: 'd', token: 't' },
        },
      }),
      'WARP 的 protocol 也是 wireguard —— 它必须先被判出来，否则会落进通用 WG 表',
    ).toEqual({ kind: 'warp', edit: true });
  });

  it('分流出来的每一支宿主都真有 case（否则按钮亮着、点下去 never 兜底在运行期抛异常）', () => {
    expect(hostCases.size, '宿主一个 case 都没解析出来 ⇒ 下面恒真').toBeGreaterThan(5);
    for (const proto of ['vless', 'wireguard', 'tailscale', 'openconnect']) {
      const kind = mobileEditFormFor(server(proto)).kind;
      expect(hostCases.has(kind), `${proto} 落到了宿主没有 case 的 kind: ${kind}`).toBe(true);
    }
  });

  it('大小写不敏感（订阅/存量节点的 protocol 大小写不归前端管）', () => {
    expect(mobileEditFormFor(server('VLESS')).kind).toBe('node');
    expect(mobileEditFormFor(server('WIREGUARD')).kind).toBe('wg');
    expect(mobileEditFormFor(server('TailScale')).kind).toBe('ts-settings');
  });
});

describe('④ 跨屏跳转：没有装载器时必须如实返回 false', () => {
  it('没装载器 ⇒ false（调用方据此落一句说明，而不是假装跳过去了）', () => {
    expect(navigateMobile('settings', 'update')).toBe(false);
  });

  it('装上之后 ⇒ true，且目的地与二级页都真的写出去了', () => {
    setPushedPage('settings', null);
    const seen: string[] = [];
    const off = installMobileNavigator((d) => seen.push(d));
    try {
      expect(navigateMobile('settings', 'update')).toBe(true);
      expect(seen, '导航器没被调到').toEqual(['settings']);
      // 二级页**先设再切**：反过来会让切过去那一帧读到上一次的页，闪一下根页。
      expect(readPushed('settings')).toBe('update');
    } finally {
      off();
      setPushedPage('settings', null);
    }
  });

  it('卸载之后回到 false（装载器泄漏会让一个已经卸载的屏继续吃跳转）', () => {
    const off = installMobileNavigator(() => {});
    off();
    expect(navigateMobile('nodes')).toBe(false);
  });

  it('不带二级页时不动那张表（只切目的地）', () => {
    setPushedPage('settings', 'update');
    const off = installMobileNavigator(() => {});
    try {
      expect(navigateMobile('settings')).toBe(true);
      expect(readPushed('settings'), '不带 page 的跳转顺手清了二级页').toBe('update');
    } finally {
      off();
      setPushedPage('settings', null);
    }
  });
});

/**
 * 读那张模块级的二级页表。
 *
 * `usePushedPage` 是 hook，而它的 server snapshot 与 live 读法指向**同一个函数**
 *（理由见 `MobileShell` 头注）⇒ 在 `renderToStaticMarkup` 里挂一个只读它的探针，
 * 读到的就是生产读到的那一份。不另导出一个「测试专用」的读函数：那会造出第二条读法，
 * 而两条读法哪天不一致，这道门量的就不再是生产那一条。
 */
function readPushed(destination: 'settings' | 'nodes'): string | null {
  let seen: string | null = null;
  function Probe(): null {
    seen = usePushedPage(destination);
    return null;
  }
  renderToStaticMarkup(createElement(Probe));
  return seen;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑤ 成功回执不许落在**会随面板一起卸载**的通道上。
 *
 * 🔴 本组 2026-09-06 补（批 2 复审，三条同时点名的 major）。形态：
 * `ImportFormPanel.doImport` 里 `setNotice({ tone: 'ok', … })` 与下一行的
 * `closeInstance(instanceId)` 落在**同一个块**里 —— React 18 自动批处理，`MobileFormHost`
 * 下一次提交时本实例已不在栈上、面板当帧卸载 ⇒ 那条回执一帧都没画过。
 * 丢掉的不是客套话：`nodes.importSingletonSkipped` 里「跳过了 M 条」是只有这里说得出的事实。
 * 桌面 `ImportDialog.tsx:299-316` 走的是全局 toast，且那段注释逐字记着它当初修的正是
 * 「导入完全静默」这个形态 —— 换成面板本地 `notice` 等于原样复发。
 *
 * 判据两条**成对**：
 *  ① 否定腿 —— `forms/**` 里没有任何一处「同块内既 `setNotice({tone:'ok'` 又 `closeInstance(`」；
 *  ② 正面腿 —— 导入成功路径上**真的**有一条走全局通道的回执（只写①会被「回执整条删掉」骗过）。
 * 失败回执**不在射程内**：表还开着，错误该跟着表活着（`write-failure-visibility.test.ts`
 * 那道门要求的正是失败落在 catch 里的 `setNotice`，两者不冲突）。
 * ═════════════════════════════════════════════════════════════════════════ */

/** `forms/` 下的生产面板（`.tsx`，去掉测试与外壳）。 */
const PANEL_FILES = [
  'ConfirmPanel.tsx',
  'ImportFormPanel.tsx',
  'MeshJoinPanel.tsx',
  'NodeFormPanel.tsx',
  'SubFormPanel.tsx',
  'TsExitPanel.tsx',
] as const;

/**
 * 源码里「同一个块内既报了成功、又关了这一层」的位置。
 *
 * 按**块**判而不是按文件判：一个面板在别处（还开着的时候）落一条 `tone:'ok'` 的 notice
 * 是完全正当的（`SubFormPanel` 的预检结果就是），按文件判会把它一起误伤。
 */
function receiptsLostOnUnmount(source: string): string[] {
  const sf = tsc.parseSourceFile('mobile-form-panel.tsx', source);
  const hits: string[] = [];
  const collect = (root: tsc.Node, pred: (n: tsc.Node) => boolean): tsc.Node[] => {
    const out: tsc.Node[] = [];
    const walk = (n: tsc.Node): void => {
      if (pred(n)) out.push(n);
      tsc.forEachChild(n, walk);
    };
    walk(root);
    return out;
  };
  const calleeText = (c: tsc.Node): string => {
    const e = (c as tsc.CallExpression).expression;
    return tsc.isIdentifier(e) ? e.text : tsc.isPropertyAccessExpression(e) ? e.name.text : '';
  };
  const okNotice = (collect(sf, tsc.isCallExpression) as tsc.CallExpression[]).filter((c) => {
    if (calleeText(c) !== 'setNotice') return false;
    const arg = c.arguments[0];
    return (
      arg !== undefined &&
      tsc.isObjectLiteralExpression(arg) &&
      arg.properties.some(
        (p) =>
          tsc.isPropertyAssignment(p) &&
          tsc.isIdentifier(p.name) &&
          p.name.text === 'tone' &&
          tsc.isStringLiteral(p.initializer) &&
          p.initializer.text === 'ok',
      )
    );
  });
  for (const call of okNotice) {
    let block: tsc.Node | undefined = call;
    while (block !== undefined && !tsc.isBlock(block)) block = block.parent;
    if (block === undefined) continue;
    const closes = (collect(block, tsc.isCallExpression) as tsc.CallExpression[]).filter(
      (c) => calleeText(c) === 'closeInstance',
    );
    if (closes.length > 0) hits.push(`setNotice({tone:'ok'}) 与 closeInstance( 同块`);
  }
  return hits;
}

describe('⑤ 成功回执的通道：不许落在随面板卸载而消失的地方', () => {
  it('⓪ 谓词自检：坏形态认得出、好形态不误伤（否则下面那条否定断言是空话）', () => {
    const bad = `
function P() {
  const doIt = async () => {
    setNotice({ tone: 'ok', text: t('x') });
    closeInstance(instanceId);
  };
}
`;
    expect(receiptsLostOnUnmount(bad).length, '坏形态没被认出来').toBe(1);
    // ① 走全局 toast 再关 —— 正确形态，不许误伤。
    const good = `
function P() {
  const doIt = async () => {
    toast.success(t('x'));
    closeInstance(instanceId);
  };
}
`;
    expect(receiptsLostOnUnmount(good)).toEqual([]);
    // ② 报了成功但**不关**这一层（预检结果）—— 正当，不许误伤。
    const stays = `
function P() {
  const preview = async () => {
    setNotice({ tone: 'ok', text: t('x') });
  };
}
`;
    expect(receiptsLostOnUnmount(stays)).toEqual([]);
    // ③ 失败回执与关闭同块 —— 不在本门射程内（那条腿由 write-failure-visibility 守）。
    const failure = `
function P() {
  const doIt = async () => {
    setNotice({ tone: 'err', text: t('x') });
    closeInstance(instanceId);
  };
}
`;
    expect(receiptsLostOnUnmount(failure)).toEqual([]);
  });

  it.each(PANEL_FILES)('%s：没有一条成功回执与关闭这一层同块', (file) => {
    const src = read(`./${file}`);
    expect(src.length, `${file} 源码读取失败 ⇒ 下面恒绿`).toBeGreaterThan(200);
    expect(
      receiptsLostOnUnmount(src),
      `${file} 里有一条 \`tone: 'ok'\` 的回执与 \`closeInstance(\` 落在同一个块里 —— ` +
        'React 18 自动批处理，面板当帧卸载，那条回执一帧都没画过（用户只看到弹层消失）。' +
        '改走全局 toast，或交给调用方落屏级 notice',
    ).toEqual([]);
  });

  it('🔴 正面腿：导入成功路径上真的有一条走全局通道的回执（否则①会被「回执删光」骗过）', () => {
    const src = strip(read('./ImportFormPanel.tsx'));
    for (const key of ['nodes.importSingletonSkipped', 'nodes.importOk', 'nodes.importStagedOk']) {
      expect(src, `导入回执少了 ${key} —— 那不是修通道，是把话删了`).toContain(key);
    }
    expect(src, '导入成功没有落到全局 toast（面板马上就卸载了，本地 notice 画不出来）').toMatch(
      /toast\.(?:success|info)\(/,
    );
    // 「跳过了 M 条」这条只有这里说得出的事实必须带上两个计数，不能退化成一句「已导入」。
    expect(src).toMatch(/importSingletonSkipped[\s\S]{0,120}skipped:/);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑥ 「判据只有一处」与「结构上不可能生效的控件」—— 批 2 复审的三条 minor 各一条门。
 * ═════════════════════════════════════════════════════════════════════════ */
describe('⑥ 复用与死控件：三条各一条判据', () => {
  it('select 的两条判据走共用的 `normalizeSelectOptions`，移动端不再抄一份', () => {
    const fields = strip(read('./FormFields.tsx'));
    expect(fields, '移动端字段渲染器没有消费共用的选项归一化').toContain('normalizeSelectOptions(');
    /* 复发形态就是那条逐字抄过来的正则与「并入首位」的合并逻辑。 */
    expect(
      fields,
      '移动端又手写了一份「点分键才翻译」的判据 —— 桌面哪天放宽判据，同一张 `ND_SPEC` ' +
        '在两个客户端上会给出两种标签，而这类分叉没有任何门',
    ).not.toContain('[A-Za-z0-9_]+(?:\\.[A-Za-z0-9_]+)+');
    expect(fields, '移动端又手写了一份「当前值不在表里就并入首位」').not.toContain(
      'spec.options.some(',
    );
    // 桌面那一侧同样只剩一层薄壳（两端都指向同一份，才叫「一处」）。
    expect(strip(read('../../components/dialogs/FieldSpec.tsx'))).toContain(
      'return normalizeSelectOptions(options, current, translate);',
    );
    // 反向自检：同一条否定断言对着旧写法确实报得出来。
    expect(
      "/^[A-Za-z0-9_]+(?:\\.[A-Za-z0-9_]+)+$/.test(l)".includes('[A-Za-z0-9_]+(?:\\.[A-Za-z0-9_]+)+'),
    ).toBe(true);
  });

  it('「经代理更新」被全局策略覆盖时置灰并换说明（一颗必然无效的开关比没有更坏）', () => {
    const src = strip(read('./SubFormPanel.tsx'));
    /* `domain/subscription-proxy.ts` 在 policy 非 follow 时直接按 policy 返回、忽略 per-sub ⇒
       那颗开关结构上不可能生效。移动端设置页自己就能把 policy 拨成 proxy/direct，路径闭合。 */
    expect(src, '没有读全局策略 —— 开关恒可点').toContain('config?.subscriptionProxyPolicy');
    expect(src, '覆盖时没有置灰那颗开关').toMatch(/disabled=\{proxyOverridden\}/);
    for (const key of ['sub.viaProxyOverrideProxy', 'sub.viaProxyOverrideDirect']) {
      expect(src, `覆盖态没有换成 ${key} —— 屏上没有一句话解释为什么拨了不生效`).toContain(key);
    }
    // 反面：通用那句只许留在 follow 那一档（三元的兜底），不许成为恒定说明。
    expect(src).toMatch(/proxyPolicy === 'proxy'[\s\S]{0,220}sub\.viaProxyHint/);
  });

  it('订阅创建的终态：先校验真的发布到 store，`clearTerminal` 排在 `hasInstance` 之后', () => {
    const src = strip(read('./SubFormPanel.tsx'));
    /* 后端原子提交成功而强刷没把它发布进 store 时：照旧关表 + `onAdded(createdId)` 会把 tab
       切到一个 `groups` 里不存在的分组（空列表、零错误提示），而 terminal 已被清掉、
       移动端又没有应用级恢复腿 ⇒ 这次创建再也捞不回来。桌面为这一格专门留了 completionFailed 分支。 */
    expect(src, '强刷之后没有校验这条订阅真的发布进 store 了').toMatch(
      /config\?\.subscriptions\?\.some\(\(s\) => s\.id === createdId\)/,
    );
    const body = src.slice(src.indexOf('const createdId'));
    const published = body.indexOf('published !== true');
    const hasInst = body.indexOf('!hasInstance(instanceId)');
    const cleared = body.indexOf('clearTerminal(snapshot.operationId)', published);
    expect(published, '发布校验那一支没了').toBeGreaterThan(-1);
    expect(hasInst, '`hasInstance` 那一跳没了').toBeGreaterThan(-1);
    expect(
      cleared > hasInst,
      '`clearTerminal` 仍排在 `hasInstance` 之前 —— 表单不在了就把 terminal 永久丢掉，' +
        '下一次渲染器水合再也捞不回这次成功的创建',
    ).toBe(true);
  });
});
