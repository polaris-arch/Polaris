/**
 * 节点编辑器**桌面 ↔ 移动端能力对差**的取材与登记（节点表单那一格的 B 面数据源）。
 *
 * # 为什么要这一张（主门头注「射程自曝」第 2 条落在节点表单上的那一格）
 *
 * 移动端节点表单 `NodeFormPanel.tsx` 与桌面 `NodeDialog.tsx` 共用字段表（`ND_SPEC`）与编解码层
 * （`protoCodec`），于是**字段级**的新增（MASQUE / Tailcat 的字段、TLS 证书固定两格、modeledOf）
 * 天然两端同时到达。**表单级**的能力却各写一份：无地址协议隐藏地址行、Tailcat 保存前的
 * `tailcat_emit_check` 同宽门、`tailcat_keypair` 生成密钥对、DERP 前置代理提示、编解码失败时把用户
 * 带到出错分组、custom 协议的内核兼容性探测 —— 这些住在 `NodeDialog.tsx` 的组件体里，
 * 桌面加一条、移动端一行没写，此前**没有任何一面看得见**（节点屏那张登记表只管行动作与菜单）。
 *
 * # 取材口径：从桌面组件**源码**派生，不手抄名单
 *
 * 对 `NodeDialog.tsx` 走真 TypeScript AST，取四类「能力指纹」：
 *  · `import:<名>` —— 从仓内模块（`@/…` 或相对路径）按名 import 的**值**（`type` 导入不算）。
 *    判据 / 纯逻辑 / 组件都在这一格：桌面接一个新的共享判据，这里就多一条。
 *  · `api:<链>` —— `api.x.y(…)` 形态的 IPC 调用（`api.server.tailcatKeypair` 就是这一格）。
 *  · `i18n:<键>` —— `t('字面量')` 渲染的文案键（新提示、新按钮的文案都在这一格）。
 *  · `proto:<协议>` —— `proto === '<协议>'` / `.protocol === '<协议>'` 的协议分支
 *    （表单级的「某协议专属行为」几乎都长这样）。
 * 同样的四类从移动端 `NodeFormPanel.tsx` 取一遍。**桌面有、移动端没有**的每一条 = 一项。
 *
 * 一项要么被移动端**真的消费**（同名指纹出现在移动端源码里 ⇒ 自动销掉），要么在下面的
 * `NODE_FORM_PARITY` 里登记一个处置；两样都没有的，进 `wiring-completeness.test.ts` 的 B 面成为债。
 *
 * # 处置码沿用四屏对差表那一套（`parity-registry.test-support.ts`），不新造
 *
 * 本表里只有 `ported` 一种：**桌面的呈现原语在移动端被重写成了等价物**（契约 A1 —— 桌面渲染器
 * 的类名全落在桌面层叠链，移动端必须重写呈现层）。每条的锚指向**移动端**那处等价物，门会打开
 * 那个文件、剥掉注释之后核对那段文本还在。「我们还没做」一律不许进这张表 —— 那是债，老实留在
 * B 面红着。
 *
 * # 射程自曝（守不住什么）
 *
 * 1. **守的是「移动端有没有消费同一个能力指纹」，不是「行为是否等价」**。移动端 import 了
 *    `tailcatSettingsError` 却从不调用它，本面会判「已消费」。行为那一半由
 *    `node-form-parity.test.ts` 的渲染 / 提交门成对补上（字段真的画出来、提交袋真的过闸）。
 * 2. **只取 `NodeDialog.tsx` 一个文件**。桌面把某个能力挪进一个新的子组件文件，本面会少一条 ——
 *    `node-form-parity.test.ts` ① 组对此有一条正面断言（桌面指纹里必须含已知的几条锚点能力），
 *    挪走时它先红。
 * 3. 字段级能力（`ND_SPEC` 里的字段）**不在本面**：两端同调 `nodeFormGroups(proto)`，
 *    是否真的画出来由同一份测试文件的渲染门逐协议核对。
 */
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { blankComments } from '@/contracts/comment-blanking.test-support';
import * as tsc from '@/test/ts-compiler';
import type { Anchor, ParityDisposition, ParityEntry } from '../parity-registry.test-support';

/** 仓根（锚的 `file` 相对它）。 */
export const REPO_DIR = fileURLToPath(new URL('../../../../', import.meta.url));

/** 桌面真值源 —— 节点编辑器组件本体。 */
export const DESKTOP_NODE_FORM = 'ui/src/components/dialogs/NodeDialog.tsx';
/** 移动端对应面板。 */
export const MOBILE_NODE_FORM = 'ui/src/mobile/forms/NodeFormPanel.tsx';

/** 子树里的全部节点（含自身）。 */
function under(root: tsc.Node): tsc.Node[] {
  const out: tsc.Node[] = [];
  const walk = (n: tsc.Node): void => {
    out.push(n);
    tsc.forEachChild(n, walk);
  };
  walk(root);
  return out;
}

/** 节点在（已剥注释的）原文里的那一段，去首尾空白。 */
const textOf = (sf: tsc.SourceFile, n: tsc.Node): string => sf.text.slice(n.pos, n.end).trim();

/** 协议分支：`proto === 'x'` / `x.protocol === 'x'`（两侧任意顺序）。 */
const PROTO_BRANCH = /\b(?:proto|protocol)\s*===\s*'([a-z0-9-]+)'|'([a-z0-9-]+)'\s*===\s*(?:[\w.]*\.)?(?:proto|protocol)\b/g;

/**
 * 一份源码的能力指纹集合。`fileName` 只用来让 AST 解析器认得扩展名；`text` 是被取材的原文
 * （合成样本也走这里 —— 反向对照要能喂一段不在仓里的源码）。
 *
 * 注释先抹掉（行列不变）：注释里写着 `api.server.tailcatKeypair` 不算消费它。
 */
export function capabilityFingerprints(fileName: string, text: string): Set<string> {
  const blanked = blankComments(text);
  /* 抹过注释的文本与盘上不同 ⇒ 走虚拟解析（`parseSourceFile` 对「项目里有、但文本不同」会直接抛）。
     扩展名保留，JSX 才解析得对。 */
  const sf = tsc.parseSourceFile(`/virtual/${fileName.replace(/^.*\//, '')}`, blanked);
  const out = new Set<string>();

  for (const s of sf.statements) {
    if (!tsc.isImportDeclaration(s)) continue;
    const spec = s.moduleSpecifier;
    if (!tsc.isStringLiteral(spec)) continue;
    if (!spec.text.startsWith('@/') && !spec.text.startsWith('.')) continue;
    if (/^import\s+type\b/.test(textOf(sf, s))) continue;
    const clause = s.importClause;
    if (!clause) continue;
    if (clause.name) out.add(`import:${clause.name.text}`);
    const named = clause.namedBindings;
    if (named && tsc.isNamedImports(named)) {
      for (const e of named.elements) {
        if (/^type\s/.test(textOf(sf, e))) continue;
        out.add(`import:${e.name.text}`);
      }
    }
  }

  for (const n of under(sf)) {
    if (!tsc.isCallExpression(n)) continue;
    const callee = textOf(sf, n.expression);
    if (/^api(?:\.\w+)+$/.test(callee)) out.add(`api:${callee}`);
    if (callee === 't') {
      const a = n.arguments[0];
      if (a !== undefined && (tsc.isStringLiteral(a) || tsc.isNoSubstitutionTemplateLiteral(a))) {
        out.add(`i18n:${a.text}`);
      }
    }
  }

  for (const m of blanked.matchAll(PROTO_BRANCH)) out.add(`proto:${m[1] ?? m[2]}`);
  return out;
}

const readRepo = (file: string): string => readFileSync(join(REPO_DIR, file), 'utf8');

export function desktopNodeFormFingerprints(): Set<string> {
  return capabilityFingerprints(DESKTOP_NODE_FORM, readRepo(DESKTOP_NODE_FORM));
}

export function mobileNodeFormFingerprints(): Set<string> {
  return capabilityFingerprints(MOBILE_NODE_FORM, readRepo(MOBILE_NODE_FORM));
}

const mobile = (mustContain: string): ParityDisposition => ({
  kind: 'ported',
  mobile: { file: MOBILE_NODE_FORM, mustContain },
});

/**
 * 登记表：桌面指纹里、移动端**以不同形态**承接的那几条。只收呈现原语的等价重写（`ported`），
 * 逐条的锚指向移动端那处等价物。
 *
 * `id` 就是指纹本身（由门从桌面源码算出，不是人起的名字 —— 桌面那处改名，这里当场变僵尸、门红）。
 */
export const NODE_FORM_PARITY: readonly ParityEntry[] = [
  /* 外壳：桌面模态框 → 移动端底部表单壳（同一个关闭意图出口、同一个提交 / 取消）。 */
  { id: 'import:Modal', disposition: mobile('<FormSheet'), note: '模态框 → 移动端表单壳' },
  /* 弹窗栈：桌面 `useDialogStore` → 移动端 `useMobileFormStore`（同一套 open / closeInstance 语义）。 */
  { id: 'import:useDialogStore', disposition: mobile('useMobileFormStore('), note: '弹窗栈 → 移动端表单栈' },
  /* 下拉：`Csel`（Portal 自绘浮层）→ 原生 `<select>`（Android 系统选择器，见 FormFields 头注第 2 条）。 */
  { id: 'import:Csel', disposition: mobile('<MobileSelect'), note: '自绘下拉 → 应用风格选择面，原生 select 保留 change 语义' },
  /* 字段渲染器：桌面 `FormFields` / `FormSection` → 移动端 `MobileFields`（同一张 `FieldSpec` 表）。 */
  { id: 'import:FormFields', disposition: mobile('<MobileFields'), note: '字段渲染器重写' },
  { id: 'import:FormSection', disposition: mobile('<MobileFields'), note: '分节渲染器重写' },
  /* 页签 → 逐组折叠段（NodeFormPanel 头注「唯一一处刻意的形态偏离」；分区真值同一个 `nodeFormGroups`）。 */
  { id: 'import:FormTabs', disposition: mobile('<FormGroup'), note: '页签 → 折叠段' },
  { id: 'import:MeshInboundPolicyEditor', disposition: mobile('<MeshInboundPolicyFields'), note: '桌面策略编辑器 → 移动端同一份归一化与校验判据、紧凑折叠表单字段' },
  { id: 'import:nodeFormUsesTabs', disposition: mobile('<FormGroup'), note: '页签开关 → 折叠段恒用' },
  /* 页签自身的两条文案（页签组的无障碍名 / basic+transport 合并页签名）：折叠段逐组取组名，
     没有「页签组」这个对象，也没有合并页签。 */
  { id: 'i18n:node.formGroup.aria', disposition: mobile('<FormGroup'), note: '页签组无障碍名 → 逐组组头' },
  { id: 'i18n:node.formGroup.connection', disposition: mobile("t('node.formGroup.basic')"), note: '合并页签 → 逐组组名' },
  /* 悬停信息图标 → 控件下方常驻一行说明（触屏没有 hover，FormFields 头注第 1 条）。 */
  { id: 'import:InfoIcon', disposition: mobile('m-form-hint'), note: '悬停提示 → 常驻说明行' },
  /* 全局 toast → 表单壳内联通知（错误与出错字段在同一根滚动轴上）。 */
  { id: 'import:toast', disposition: mobile('setNotice('), note: 'toast → 表单内联通知' },
];

/** 一条对差项（进 `wiring-completeness.test.ts` B 面的形状）。 */
export interface NodeFormParityItem {
  readonly id: string;
  readonly where: string;
  readonly detail: string;
  readonly work: string;
}

/**
 * 桌面有、移动端没消费、也没登记的每一条。`wiring-completeness.test.ts` 逐条收进 B 面。
 * 参数可注入（反向对照要喂合成的两侧指纹）。
 */
export function nodeFormParityDebt(
  desktop: ReadonlySet<string> = desktopNodeFormFingerprints(),
  mobileSide: ReadonlySet<string> = mobileNodeFormFingerprints(),
  register: readonly ParityEntry[] = NODE_FORM_PARITY,
): NodeFormParityItem[] {
  const registered = new Set(register.map((e) => e.id));
  return [...desktop]
    .filter((id) => !mobileSide.has(id) && !registered.has(id))
    .sort()
    .map((id) => ({
      id: `parity:node-form:${id}`,
      where: `${DESKTOP_NODE_FORM} → ${MOBILE_NODE_FORM}`,
      detail: `桌面节点编辑器消费了 ${id}，移动端节点表单没有 · 界面上一个字都没提`,
      // 不并成一件工作：界面上一个字都没提的缺口各自是一件（同 `scanParityRegisterFace` 的口径）。
      work: `parity:node-form:${id}`,
    }));
}

/** 登记表里每条锚的核对结果（剥注释后命中的行号；-1 = 找不到）。 */
export function resolveNodeFormAnchor(anchor: Anchor): { exists: boolean; hit: number } {
  const abs = join(REPO_DIR, anchor.file);
  if (!existsSync(abs)) return { exists: false, hit: -1 };
  const text = blankComments(readFileSync(abs, 'utf8'));
  const at = text.indexOf(anchor.mustContain);
  return { exists: true, hit: at === -1 ? -1 : text.slice(0, at).split('\n').length };
}
