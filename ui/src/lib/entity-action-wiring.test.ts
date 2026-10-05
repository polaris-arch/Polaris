/**
 * **行内动作**接线守卫 —— 钉死「每一个把实体 id 交给后端的调用点，都被显式判过 staged-only 怎么办」。
 *
 * # 为什么需要这盏灯
 *
 * 列表现在渲染的是 `effectiveConfig`（否则用户看不见自己刚做的编辑），于是列表里会出现
 * **staged-only 实体**：在 effective 里、磁盘上还没有。后端一律按 id 在磁盘上找 —— 把这样一个 id
 * 交下去，轻则报错、重则静默缺席（`commands/speedtest::requested_server_configs` 对未知 id 是 `filter_map` 跳过）。
 *
 * 而这条风险**没有任何门会抓**：新增一个行内按钮、直接 `api.server.xxx(node.id)`，类型对、测试绿、构建过。
 * 三种落法（撤销 / 置灰 / 只数磁盘）写在 `lib/staged-config.ts` 的 `ENTITY_ACTION_TABLE`；
 * 本文件负责的是**另一半**：确保没有调用点从那张表旁边溜过去。
 *
 * # 判据面
 *
 * `api.(server|rules|subscription).<方法>` 的调用点，**限定在能渲染 staged-only 实体的文件里**
 * （= 文件里出现 `useEffectiveServers` / `useEffectiveRules`）。这三个命名空间是按实体 id 寻址的那三个；
 * 别的命名空间（`config` / `proxy` / `ruleResources`…）不按实体 id 下发，不在射程。
 *
 * 判据面**限定在这些文件**而不是全仓：别的文件渲染的是磁盘镜像，它们手上的 id 恒在盘上
 * （谁读哪一面由 `config-read-wiring.test.ts` 的 `MIRROR_SITES` 钉住）。两张表接力，缺一边都会漏。
 *
 * # 扫描要跨行
 *
 * `void api.server\n  .delete(...)` 是本仓真实存在的写法（`NodesScreen` 两处）。按行的正则会漏掉它们
 * —— 漏掉的恰好是本轮唯一走 `revert` 的那条腿。故先归一空白再匹配。
 *
 * # 守的是形态不是措辞
 *
 * 断言落在「哪个文件调了哪个后端方法、几次」这类结构事实上；改注释、改文案不会误伤，
 * 新增/挪走一个调用点则必然转红。
 */
import { describe, it, expect } from 'vitest';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { ENTITY_ACTION_TABLE, isBypassedOp, stagedOnlyStrategyOf } from './staged-config';

const SRC = fileURLToPath(new URL('..', import.meta.url));

function code(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/(^|[^:])\/\/.*$/gm, (m, p1: string) => p1 + ' '.repeat(m.length - p1.length));
}

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) sourceFiles(p, out);
    // 共享谓词（`contracts/test-only-modules.ts` 头注：三道门需要同一个概念，不许各留一份拷贝）。
    // `.test-support.` 同样不进产物，且产品代码不许 import 它们（`i18n-coverage` G0-b 锁着）——
    // 把它们留在产品面上，判据会被别的判据的**锚文本**喂饱（2026-09-06 在 app-wiring ⑩/⑫ 实测过一次假绿）。
    else if (/\.tsx?$/.test(e.name) && !IS_TEST_ONLY_MODULE.test(e.name)) out.push(p);
  }
  return out;
}

/** `\s*` 两处不可省：本仓有 `void api.server` 换行再 `.delete(` 的写法。 */
const CALL = /\bapi\s*\.\s*(server|rules|subscription)\s*\.\s*(\w+)/g;
/** 「这个文件能渲染 staged-only 实体」——判据面的边界。 */
// Extracted action owners are part of the same staged-only semantic surface even though
// the screen passes their effective/disk projections as explicit inputs.
const RENDERS_EFFECTIVE = /useEffectiveServers|useEffectiveRules|use-node-deletion|use-node-subscription-actions|use-node-speed-test/;
/** 同上，路径匹配腿（这些文件不 import useEffective*，靠调用方注入 effective/disk 投影）：
 *  use-node-deletion/use-node-subscription-actions/use-node-speed-test 是既有三家；
 *  use-node-actions.ts（5B）、rule-submit.ts（5C）与移动窗口 use-mobile-speed-test（2026-09-27）同样接 caller 的实体投影，同一条判据。 */
const EXTRACTED_OWNER_PATH =
  /components[\\/](?:screens[\\/]nodes[\\/]use-node-(?:deletion|subscription-actions|speed-test|actions)|dialogs[\\/]rule-submit)\.ts$|mobile[\\/]use-mobile-speed-test\.ts$/;

interface Call {
  readonly file: string;
  readonly callee: string;
}

const CALLS: Call[] = [];
let scannedFiles = 0;
let effectiveFiles = 0;
for (const p of sourceFiles(SRC)) {
  scannedFiles += 1;
  const src = code(readFileSync(p, 'utf8'));
  if (!RENDERS_EFFECTIVE.test(src) && !EXTRACTED_OWNER_PATH.test(p)) continue;
  effectiveFiles += 1;
  const rel = p.slice(SRC.length).split(/[\\/]/).join('/');
  for (const m of src.matchAll(CALL)) CALLS.push({ file: rel, callee: `api.${m[1]}.${m[2]}` });
}

/**
 * **自曝纪律**（模块加载期就抛，不留给断言）：扫空了 / 判据面失配 ⇒ 下面每一条断言都会空跑恒绿，
 * 而那正是本守卫要防的那种「没有任何门会转红」。抛出来比绿着更诚实。
 */
if (scannedFiles < 100) throw new Error(`行内动作守卫扫不到源码（只收到 ${scannedFiles} 个文件）`);
if (effectiveFiles < 8)
  throw new Error(`只有 ${effectiveFiles} 个文件渲染 effective 实体，判据面已失配`);
if (CALLS.length < 20) throw new Error(`只扫到 ${CALLS.length} 个后端调用点，判据面已失配`);
if (ENTITY_ACTION_TABLE.length === 0) throw new Error('ENTITY_ACTION_TABLE 空表，策略无从查起');

// ─────────────────────────────── 登记表 ───────────────────────────────

/**
 * - `ruled` —— 该调用点会拿到 staged-only 的 id，落法已裁定：`op` 必须在 `ENTITY_ACTION_TABLE` 里。
 * - `read-only` —— 可按 staged-only id 查询，但不写配置、进程或 state。
 * - `no-staged-only-id` —— 这个调用点**拿不到** staged-only 的 id。`why` 必须点名机械依据之一：
 *   `staged 分流后不可达`（同一函数里 `editRoute(...)==='staged'` 已先 return）/ `不传实体 id` /
 *   `该族恒不入暂存` / `仅取磁盘镜像或本次新建的 id`（有效集合只用于名称候选）。
 *
 * 上一轮的 `pending-ruling`（待裁定）已**清零**：组网单例节点那四条腿统一裁为 `block`。
 * 留着一条「还没定」的路就是守卫里一个公开的洞，故连同那条路一起删掉；日后再有待裁定项，照原样加回来。
 */
type ActionRoute = 'ruled' | 'read-only' | 'no-staged-only-id';

interface ActionSite {
  readonly file: string;
  readonly callee: string;
  /** 树上的调用点条数。同一 (文件, 方法) 的两处调用可以有不同处置 —— 计数逼人显式看一眼。 */
  readonly count: number;
  readonly route: ActionRoute;
  /** `ruled` 必须填 `ENTITY_ACTION_TABLE` 里的 op。 */
  readonly op?: string;
  readonly why: string;
}

const SITES: readonly ActionSite[] = [
  { file: 'components/dialogs/MeshJoinDialog.tsx', callee: 'api.server.tailscaleStateExists', count: 1,
    route: 'read-only', why: '按节点 id 只读本机 TS state 目录存在性；staged-only id 只会得到无目录，不执行登出' },
  { file: 'components/dialogs/TsSettingsDialog.tsx', callee: 'api.server.tailscaleStateExists', count: 1,
    route: 'read-only', why: '按节点 id 只读本机 TS state 目录存在性；结果只决定按钮可见性，不执行登出' },
  { file: 'mobile/forms/TsSettingsPanel.tsx', callee: 'api.server.tailscaleStateExists', count: 1,
    route: 'read-only', why: '按节点 id 只读本机 TS state 目录存在性；staged-only id 不会触发写操作' },
  { file: 'mobile/nodes/MobileNodesScreen.tsx', callee: 'api.server.tailscaleStateExists', count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达：打开行动作面时 row.stagedOnly 先返回；只读磁盘 TS state 目录的结果仅决定账号入口文案，未保存节点的入口置灰' },
  { file: 'mobile/forms/TsLoginPanel.tsx', callee: 'api.server.tailscaleLoginProgress', count: 1,
    route: 'read-only', why: '按已准备的 attempt id 只读原生登录回执，不保存节点或改变 TS state' },
  { file: 'mobile/use-mobile-speed-test.ts', callee: 'api.server.onSpeedTestProgress', count: 1, route: 'no-staged-only-id', why: '不传实体 id：窗口级事件订阅只收后端真实帧；结果/进度不是按 staged 实体寻址的命令' },
  { file: 'mobile/use-mobile-speed-test.ts', callee: 'api.server.onSpeedTestResult', count: 1, route: 'no-staged-only-id', why: '不传实体 id：窗口级事件订阅只收后端真实帧；结果/进度不是按 staged 实体寻址的命令' },
  { file: 'mobile/use-mobile-speed-test.ts', callee: 'api.server.onSpeedTestDone', count: 1, route: 'no-staged-only-id', why: '不传实体 id：窗口级事件订阅只收后端真实帧；结果/进度不是按 staged 实体寻址的命令' },

  // ── 已裁定 ──
  {
    file: 'components/screens/nodes/use-node-deletion.ts',
    callee: 'api.server.delete',
    count: 2,
    route: 'ruled',
    op: 'server.delete',
    why: '两处（单卡删除 + WARP 注销/重注册）都先过 splitStagedOnly：staged-only ⇒ 撤销条目；盘上节点统一暂存，Apply 才执行 TS/WARP 副作用',
  },
  {
    file: 'components/screens/nodes/use-node-deletion.ts',
    callee: 'api.server.deleteBatch',
    count: 1,
    route: 'ruled',
    op: 'server.deleteBatch',
    why: '批量：staged-only 走撤销；盘上节点统一暂存，暂存未启用或未知 id 才走后端即时腿',
  },
  {
    file: 'components/screens/nodes/use-node-speed-test.ts',
    callee: 'api.server.speedTest',
    count: 2,
    route: 'ruled',
    op: 'server.speedTest',
    why: 'staged-only 在卡上置灰（speedTestBlockReason 第三参）并从三个批量候选集里排掉（speedTestableIds 第三参）',
  },
  {
    file: 'mobile/use-mobile-speed-test.ts',
    callee: 'api.server.speedTest',
    count: 1,
    route: 'ruled',
    op: 'server.speedTest',
    why: '移动窗口 coordinator 统一首页当前/全部与节点单项/批量请求；调用方沿 speedTestBlockReason/stagedOnlyIds 与 speedTestableIds 排除未保存节点，首页 planAllHomeSpeedTest 也只交磁盘节点。真实 IPC 仅此一处，守卫由 nodes-speedtest-wiring/home-speedtest 目标覆盖',
  },
  // 2026-07-30 行内删除落地后**仍是 1 处**：规则列表的行内垃圾桶与规则弹窗 footer 那颗共用
  // `useRuleDelete` 这一条腿（本文件即那条腿），没有第二个 `api.rules.delete` 调用点。
  // 它留在本守卫射程内**不是巧合**：判据面 = 「文件里出现 useEffectiveRules」，而该 hook 自持
  // staged-only 差集的两个入参 ⇒ 抽函数没有把调用点抽到灯外（那才是绕开计数）。
  {
    file: 'lib/use-rule-delete.ts',
    callee: 'api.rules.delete',
    count: 1,
    route: 'ruled',
    op: 'rule.delete',
    why: 'staged-only ⇒ 撤销条目；盘上已有的那条才走「暂存一条 nextValue: null 的删除条目」或直落盘',
  },

  // ── 拿不到 staged-only 的 id ──
  {
    file: 'components/dialogs/ImportDialog.tsx',
    callee: 'api.server.addBulk',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（editRoute("servers") 命中即逐节点 stage 并跳过本调用）；且它建新节点、不按已有 id 寻址',
  },
  {
    file: 'components/dialogs/NodeDialog.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达；新建腿不传已有实体 id',
  },
  {
    file: 'components/dialogs/NodeDialog.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（同一 handleSubmit 里 editRoute 命中即 stage + return）',
  },
  {
    file: 'components/dialogs/NodeDialog.tsx',
    callee: 'api.server.tailcatKeypair',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id：只传表单里的私钥串（可空），后端纯计算密钥对，不读不写配置',
  },
  {
    file: 'components/dialogs/WgDialog.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达；新建腿不传已有实体 id',
  },
  {
    file: 'components/dialogs/WgDialog.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达',
  },
  {
    file: 'components/dialogs/TsSettingsDialog.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达',
  },
  {
    file: 'components/dialogs/TsSettingsDialog.tsx',
    callee: 'api.server.tailscaleGetStatus',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id（无参调用，拉的是整机 TS 状态快照）',
  },
  // TS 登录只用 effective 组网集合收集名称；既有身份来自磁盘 servers.find，
  // 新建身份由 planTsLoginSubmit mint 并先保存。下列调用不会收到 staged-only id。
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：add 只接收 planTsLoginSubmit 新建的节点，effective 仅用于名称候选',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：existingTs 从 servers.find(serverId) 取磁盘节点，effective 仅用于名称候选',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLoginPrepare',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：prepare 使用提交计划的 server.id，不读取 effective 节点身份',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleStateExists',
    count: 2,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：展示查询用 existingTs.id，提交校验用计划的 server.id',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLogout',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：退出旧身份使用计划的 server.id，effective 仅用于名称候选',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLogin',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：登录用既有磁盘节点或本次保存的节点，不传 effective 身份',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLoginCancel',
    count: 3,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：取消使用由提交计划生成的 activeRequest 或 server.id',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLoginProgress',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：焦点回读只使用提交前登记的 serverId+attemptId；节点已保存并通过刷新对账，不取 effective 暂存身份，旧请求回执不能覆盖新请求。',
  },
  // 移动端登录与桌面共用提交计划；既有身份从磁盘 servers 寻址，新身份先 mint 再保存。
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id：各平台共用真实登录 save 腿，只保存 planTsLoginSubmit 新建的节点；effective 集合只用于名称候选，不再有 iOS 仅保存配置的分支。',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：各平台共用真实登录 save 腿，既有节点从磁盘 servers.find(serverId) 取得并交给提交计划；请求授权仅使用 bound progress，不再轮询全局 TS STATUS。',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLoginPrepare',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：prepare 使用提交计划生成的 server.id',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleStateExists',
    count: 2,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：只校验提交计划生成的 server.id',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLogout',
    count: 1,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：清理提交计划中的节点身份',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLogin',
    count: 2,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：登录对象先保存再使用',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLoginCancel',
    count: 2,
    route: 'no-staged-only-id',
    why: '仅取磁盘镜像或本次新建的 id：取消只使用本次提交生成的 request.serverId 或 server.id',
  },

  // ── 移动端表单宿主与节点面（2026-09-06 批 2）。逐条与桌面同形腿的裁定**逐字一致** ──
  {
    file: 'mobile/nodes/node-deletion.ts',
    callee: 'api.server.delete',
    count: 1,
    route: 'ruled',
    op: 'server.delete',
    why: '与桌面 use-node-deletion 同一条：先过 splitStagedOnly（staged-only ⇒ 撤销条目），盘上节点按 partitionNodeDeleteRoutes 分流，直落盘那支才走本调用',
  },
  {
    file: 'mobile/nodes/node-deletion.ts',
    callee: 'api.server.deleteBatch',
    count: 1,
    route: 'ruled',
    op: 'server.deleteBatch',
    why: '批量：与桌面同一条，staged-only 走撤销、盘上节点按策略分流，只有 directIds 非空才发这一次',
  },
  {
    file: 'mobile/nodes/node-deletion.ts',
    callee: 'api.subscription.delete',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（editRoute("subscriptions") 命中即 stage 订阅 + 同组 stageServerDeletions 并 return）；订阅新增/更新是远端直写，不存在 staged-only 订阅 id',
  },
  {
    file: 'mobile/forms/NodeFormPanel.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达；新建腿不传已有实体 id（与 NodeDialog 逐字同形）',
  },
  {
    file: 'mobile/forms/NodeFormPanel.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（同一 submit 里 editRoute 命中即 stage + return）',
  },
  {
    file: 'mobile/forms/NodeFormPanel.tsx',
    callee: 'api.server.tailcatKeypair',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id：只传表单里的私钥串（可空），后端纯计算密钥对，不读不写配置（与 NodeDialog 同形）',
  },
  {
    file: 'mobile/forms/ImportFormPanel.tsx',
    callee: 'api.server.addBulk',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（命中即逐节点 stage 并跳过本调用）；且它建新节点、不按已有 id 寻址',
  },
  {
    file: 'mobile/forms/TsExitPanel.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（同一 save 里 editRoute 命中即 stage + return）',
  },
  {
    file: 'mobile/forms/TsExitPanel.tsx',
    callee: 'api.server.tailscaleGetStatus',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id（无参调用，拉的是整机 TS 状态快照，同 TsSettingsDialog）',
  },
  {
    file: 'mobile/nodes/MobileNodesScreen.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: '克隆腿：staged 分流后不可达（editRoute 命中即 stage 一条新 id 的条目并跳过）；克隆恒建新实体，剥掉了源节点的 id',
  },
  {
    // 提交逻辑（含这两个调用点）2026-08-30 随 5C 拆分外提到 rule-submit.ts，登记跟着落点走。
    file: 'components/dialogs/rule-submit.ts',
    callee: 'api.rules.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达；新建腿不传已有实体 id',
  },
  {
    file: 'components/dialogs/rule-submit.ts',
    callee: 'api.rules.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达',
  },
  {
    // 新建带场景的规则插到最前（spec §3.4-4）的直写腿：只在规则走直写时发生，此时暂存层不活跃
    // （无暂存条目）⇒ 序列里不可能有 staged-only 的 id；新 id 取自 rules.add 的落盘返回。
    file: 'components/dialogs/rule-submit.ts',
    callee: 'api.rules.reorder',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（暂存腿写顺序条目、不发 IPC）；直写腿时暂存层不活跃，序列里没有 staged-only id',
  },
  {
    file: 'components/RuleSubjectMenuItems.tsx',
    callee: 'api.rules.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（「加入已有规则」追加腿，editRoute 命中即 stage + 不发 IPC）；开关关着时暂存条目恒空 ⇒ 也不存在 staged-only 规则',
  },
  // ── 移动端连接屏 / 新建规则表（批 13）：与上面两条逐条同形，依据因此逐条相同 ──
  {
    file: 'mobile/connections/MobileConnectionsScreen.tsx',
    callee: 'api.rules.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（「加入已有规则」追加腿，editRoute 命中即 stage + 不发 IPC）；开关关着时暂存条目恒空 ⇒ 也不存在 staged-only 规则',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（行内复制腿，editRoute 命中即 stage + return）',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（开关切换腿）',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.reorder',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（顺序条目走 entityPath 单段，不下发 id）',
  },
  // ── 移动端首页「给这个主机加一条规则」面板：两条腿与桌面
  //    `ConnectionTopology#addSubjectRule` / `RuleSubjectMenuItems#append` 逐条同形 ──
  {
    file: 'mobile/home/MobileHomeScreen.tsx',
    callee: 'api.rules.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（editRoute 命中即 stage 一条自铸 id 的新条目并跳过 IPC）；新建腿不传已有实体 id',
  },
  {
    file: 'mobile/home/MobileHomeScreen.tsx',
    callee: 'api.rules.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（「合并进已有规则」追加腿，同桌面 RuleSubjectMenuItems）；开关关着时暂存条目恒空 ⇒ 也不存在 staged-only 规则',
  },
  // ── 移动端「规则」屏：三条腿与桌面 RulesScreen 逐条同形，依据因此逐条相同 ──
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（行内复制腿，editRoute 命中即 stage + return）',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（开关切换腿）',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.reorder',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（顺序条目走 entityPath 单段，不下发 id）',
  },
  {
    // cloneServer/copyLink/copyLinksBatch（含下面两条的调用点）2026-08-30 随 5B 拆分外提到
    // use-node-actions.ts，登记跟着落点走。
    file: 'components/screens/nodes/use-node-actions.ts',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（克隆腿）；新建腿不传已有实体 id',
  },
  {
    file: 'components/screens/nodes/use-node-actions.ts',
    callee: 'api.server.generateUrl',
    count: 2,
    route: 'no-staged-only-id',
    why: '不传实体 id：api-client 传的是整个 ServerConfig 对象（`{ server }`），后端不按 id 查盘',
  },
  {
    file: 'mobile/nodes/MobileNodesScreen.tsx',
    callee: 'api.server.generateUrl',
    count: 2,
    route: 'no-staged-only-id',
    why: '不传实体 id：传的是整个 ServerConfig 对象（`{ server }`），后端不按 id 查盘。单条 + 批量共两处',
  },
  /* `api.server.onSpeedTestProgress` 曾在此登记（NodesScreen 自订、渲染屏内进度行）。
     2026-07-31 进度改全局 sticky toast 后订阅上移到 `App.tsx` 的全局订阅层 —— 那个文件不在本门的
     判据面内（无 `useEffectiveServers`/`useEffectiveRules`，渲染不到 staged-only 实体），故整行删掉
     而不是改文件名。 */
  {
    file: 'components/screens/nodes/use-node-subscription-actions.ts',
    callee: 'api.subscription.delete',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达：运行期删除产生 subscription + child servers 同组条目；新增/更新订阅仍是远端直写，故不存在 staged-only 订阅 id',
  },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.registerWarp',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id（只传 license，返回一份 WireGuard 草稿）',
  },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: '新建腿：不传实体 id（后端落盘那一刻才发 id）。WARP 的写腿在写侧守卫里恒 direct/W-3，不经暂存',
  },
  {
    file: 'store/app-store.ts',
    callee: 'api.server.switch',
    count: 1,
    route: 'no-staged-only-id',
    why: '该族恒不入暂存：switchServer 在 BYPASS_TABLE 里（W-1），且出口选单读的是磁盘镜像（MIRROR_SITES 的 HomeScreen 行）⇒ 拿不到 staged-only id',
  },

  // ── 组网单例节点（TS / WARP）：不可逆动作一律 block ──
  // 这四条都触达**远端或不可逆状态**。即使 staged-only 的 TS/WARP 实体来自导入/手填，
  // 把一个盘上不存在的 id 发下去比挡住它糟得多，故显式登记而不是靠 `stagedOnlyStrategyOf` 的默认腿。
  {
    file: 'components/dialogs/TsSettingsDialog.tsx',
    callee: 'api.server.tailscaleLogout',
    count: 1,
    route: 'ruled',
    op: 'server.tailscaleLogout',
    why: '登出清的是磁盘上的 TS state 目录，盘上没有这个节点就没有作用对象；staged-only 节点必须挡住并提示先保存',
  },
  {
    file: 'components/screens/nodes/NodesScreen.tsx',
    callee: 'api.server.tailscaleLogout',
    count: 1,
    route: 'ruled',
    op: 'server.tailscaleLogout',
    why: '同 TsSettingsDialog（组网卡上的登出腿）；staged-only 节点必须挡住并提示先保存',
  },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.applyWarpLicense',
    count: 1,
    route: 'ruled',
    op: 'warp.edit',
    why: '向 Cloudflare 提交 license 当场改远端账户等级（W-3 不可逆）；staged-only 节点没有可更新的远端设备',
  },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'ruled',
    op: 'warp.edit',
    why: '更新的是一台**已注册**的远端设备（本文件无 editRoute，写侧四条腿恒 direct/W-3）；staged-only 节点必须挡住',
  },

  /* ── 移动端组网三张表（2026-09-06 批 3）。逐条与桌面同形腿的裁定**逐字一致** —— 两端对
        「这次调用会不会拿到一个盘上没有的 id」给出两种答案，就是两种可复现的失败。 ── */
  {
    file: 'mobile/forms/WgPanel.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达；新建腿不传已有实体 id（与 WgDialog 逐字同形）',
  },
  {
    file: 'mobile/forms/WgPanel.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（同一 submit 里 editRoute 命中即 stage + return）',
  },
  {
    file: 'mobile/forms/TsSettingsPanel.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'no-staged-only-id',
    why: 'staged 分流后不可达（同 TsSettingsDialog）',
  },
  {
    file: 'mobile/forms/TsSettingsPanel.tsx',
    callee: 'api.server.tailscaleGetStatus',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id（无参调用，拉的是整机 TS 状态快照，同 TsSettingsDialog）',
  },
  {
    file: 'mobile/forms/TsSettingsPanel.tsx',
    callee: 'api.server.tailscaleLogout',
    count: 2,
    route: 'ruled',
    op: 'server.tailscaleLogout',
    why: '正常登出与主核持有时用户确认断开后的第二次登出均清磁盘 TS state；staged-only 节点必须挡住并提示先保存，后者还复核节点仍在磁盘镜像中并由原生 writer gate 兜底',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.registerWarp',
    count: 1,
    route: 'no-staged-only-id',
    why: '不传实体 id（只传 license，返回一份 WireGuard 草稿），同 WarpDialog',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.add',
    count: 1,
    route: 'no-staged-only-id',
    why: '注册腿：不传实体 id（后端落盘那一刻才发 id）。WARP 的写腿恒 bypass/W-2，不经暂存（同 WarpDialog）',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.applyWarpLicense',
    count: 1,
    route: 'ruled',
    op: 'warp.edit',
    why: '同 WarpDialog：向 Cloudflare 提交 license 当场改远端账户等级（W-3 不可逆）；staged-only 节点没有可更新的远端设备',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.update',
    count: 1,
    route: 'ruled',
    op: 'warp.edit',
    why: '同 WarpDialog：更新的是一台**已注册**的远端设备；staged-only 节点由同一次提交里的 splitStagedOnly 前置挡住',
  },
];

// ─────────────────────────────── 断言 ───────────────────────────────

const key = (s: { file: string; callee: string }) => `${s.file} | ${s.callee}`;

describe('守卫自检：扫到的确实是源码（防扫空 → 断言恒真）', () => {
  it('测试文件被排除在扫描面外', () => {
    expect(sourceFiles(SRC).some((p) => /\.(test|spec)\.tsx?$/.test(p))).toBe(false);
  });

  it('跨行调用形态被扫到（锚点：`void api.server` 换行再 `.delete(`）', () => {
    // 按行的正则会漏掉它 —— 漏掉的恰好是本轮唯一走 revert 的那条腿。
    const found = new Set(CALLS.map(key));
    expect(found.has('components/screens/nodes/use-node-deletion.ts | api.server.delete')).toBe(true);
  });
});

describe('A1：调用点全登记（含条数）', () => {
  const actual = new Map<string, number>();
  for (const c of CALLS) actual.set(key(c), (actual.get(key(c)) ?? 0) + 1);

  it('树上扫到的每一个 (文件, 后端方法) 都在登记表里', () => {
    const registered = new Set(SITES.map(key));
    const unregistered = [...actual.keys()].filter((k) => !registered.has(k)).sort();
    expect(
      unregistered,
      '以下调用点未登记 —— 它会不会拿到 staged-only 的 id？拿得到就进 ENTITY_ACTION_TABLE，' +
        `拿不到就补一行说清依据：\n${unregistered.join('\n')}`
    ).toEqual([]);
  });

  it('登记表里没有陈旧行', () => {
    const stale = SITES.map(key)
      .filter((k) => !actual.has(k))
      .sort();
    expect(stale, `以下登记已陈旧（调用点被删/改名），删掉：\n${stale.join('\n')}`).toEqual([]);
  });

  it('登记的条数与树上一致', () => {
    const drift = SITES.filter((s) => actual.get(key(s)) !== s.count).map(
      (s) => `${key(s)}：登记 ${s.count}，实际 ${actual.get(key(s)) ?? 0}`
    );
    expect(drift, '同一 (文件, 方法) 新增了调用点 —— 新那处属于哪一路必须显式判一次').toEqual([]);
  });

  it('登记表自身不重复', () => {
    const seen = new Set<string>();
    const dup = SITES.map(key).filter((k) => (seen.has(k) ? true : (seen.add(k), false)));
    expect(dup).toEqual([]);
  });
});

describe('A2：每条登记说得出因由，且与策略表对得上', () => {
  it('ruled 的 op 必须在 ENTITY_ACTION_TABLE 里', () => {
    const ops = new Set(ENTITY_ACTION_TABLE.map((r) => r.op));
    for (const s of SITES.filter((x) => x.route === 'ruled')) {
      expect(s.op, `${key(s)} 标了 ruled 却没填 op`).toBeTruthy();
      expect(ops.has(s.op!), `${key(s)} 的 op「${s.op}」不在 ENTITY_ACTION_TABLE 里`).toBe(true);
    }
  });

  it('非 ruled 的行不许填 op（填了就是把没裁定的说成裁定过）', () => {
    for (const s of SITES.filter((x) => x.route !== 'ruled')) {
      expect(s.op, `${key(s)} 不是 ruled 却填了 op`).toBeUndefined();
    }
  });

  it('no-staged-only-id 必须点名机械依据之一', () => {
    for (const s of SITES.filter((x) => x.route === 'no-staged-only-id')) {
      expect(s.why, `${key(s)} 没点名依据`).toMatch(/staged 分流后不可达|不传实体 id|恒不入暂存|仅取磁盘镜像或本次新建的 id/);
    }
  });

  it('read-only 登记必须说明为何可接收 staged-only id', () => {
    for (const s of SITES.filter((x) => x.route === 'read-only')) {
      expect(s.why, `${key(s)} 没说明只读边界`).toMatch(/只读/);
    }
  });

  it('TS 登录只从 effective 取名称，不从中取后端操作 id', () => {
    const src = code(readFileSync(join(SRC, 'components/dialogs/TsLoginDialog.tsx'), 'utf8'));
    expect(src).toMatch(/const existingTs = servers\.find\([^;]+\) \?\? savedServer\.current;/);
    expect(src).toMatch(/const meshNames = groupServersBySubscription\(visibleServers, subscriptions\)[\s\S]*?\.servers\.map\(\(server\) => server\.name\)/);
    expect(src).toMatch(/planTsLoginSubmit\(\{[\s\S]*?existing: existingTs,/);
    expect(src).toMatch(/savedServer\.current = server;/);
  });

  it('组网远端动作的 block 理由必须明示覆盖 staged-only 实体', () => {
    // 普通节点删除已可暂存，因此 TS/WARP 实体也能够以 staged-only 形态存在。
    // 这几条远端登录/注销腿必须继续 block，且理由不得再借用「今天走不到」的旧前提。
    // `server.speedTest` 同样是 block，但它不属于远端账户动作，不在此列。
    //
    // 🔴 **2026-09-06（批 3）从「数个数」换成「逐条对差」**：移动端组网表落地后这一族从 4 条
    // 涨到 7 条，而个数式断言只会说「4 变成了 7」，说不出**多了哪条、少了哪条**。
    // 逐条清单严格更强：个数变了它必红，个数没变而某一条被换成另一条它同样红。
    const PREVENTIVE = new Set(['server.tailscaleLogout', 'warp.edit']);
    const rows = SITES.filter((x) => x.op !== undefined && PREVENTIVE.has(x.op));
    expect(rows.map(key).sort(), '组网单例那几条腿被悄悄改路由 / 删掉 / 换了一条').toEqual(
      [
        'components/dialogs/TsSettingsDialog.tsx | api.server.tailscaleLogout',
        'components/dialogs/WarpDialog.tsx | api.server.applyWarpLicense',
        'components/dialogs/WarpDialog.tsx | api.server.update',
        'components/screens/nodes/NodesScreen.tsx | api.server.tailscaleLogout',
        'mobile/forms/TsSettingsPanel.tsx | api.server.tailscaleLogout',
        'mobile/forms/WarpPanel.tsx | api.server.applyWarpLicense',
        'mobile/forms/WarpPanel.tsx | api.server.update',
      ].sort(),
    );
    for (const s of rows) {
      expect(stagedOnlyStrategyOf(s.op!), `${key(s)} 不再是 block`).toBe('block');
      expect(s.why, `${key(s)} 没有说明 staged-only 实体的处理`).toMatch(/staged-only/);
    }
  });

  it('每条登记都有非空理由', () => {
    for (const s of SITES) expect(s.why.length, key(s)).toBeGreaterThan(10);
  });
});

describe('A3：策略表自身（三种语义，一处定义）', () => {
  it('每条策略都有非空因由', () => {
    for (const r of ENTITY_ACTION_TABLE) expect(r.why.length, r.op).toBeGreaterThan(10);
  });

  it('op 不重复（同一个动作不得有两种策略）', () => {
    const seen = new Set<string>();
    const dup = ENTITY_ACTION_TABLE.map((r) => r.op).filter((o) =>
      seen.has(o) ? true : (seen.add(o), false)
    );
    expect(dup).toEqual([]);
  });

  it('未登记的 op 落最保守的一腿（不把可能不存在的 id 交给后端）', () => {
    // 变异对照：把 `?? 'block'` 改成 `?? 'revert'` 或让它抛，本条转红。
    expect(stagedOnlyStrategyOf('server.somethingBrandNew')).toBe('block');
  });

  it('表里登记过的 op 各自返回自己的策略（防 fallback 把整张表吃掉）', () => {
    for (const r of ENTITY_ACTION_TABLE) expect(stagedOnlyStrategyOf(r.op)).toBe(r.strategy);
  });
});

describe('A5：`disk-only` 那条没有函数出口，只能在源码上钉', () => {
  const NODES = code(
    readFileSync(join(SRC, 'components/screens/nodes/NodesScreen.tsx'), 'utf8')
  );

  it('切片非空（防下面两条在空串上恒真）', () => {
    expect(NODES.length).toBeGreaterThan(1000);
  });

  it('级联计数两处都数**磁盘镜像**', () => {
    // `disk-only` 不像 revert/block 有个函数可调，它就是「把哪一份集合传进去」。
    // 变异对照：把任一处换回 `subDeleteNodeCount(servers,`，本条转红 —— 那个数字会把
    // staged-only 节点也算进「后端将删除几个」，用户按一个虚高的数点确认。
    // SubInfoBar 的调用点（含下方计数）已随 5B 拆分外提到 NodesTabs.tsx，取材面须跟着落点走。
    const nodesTabs = code(readFileSync(join(SRC, 'components/screens/nodes/NodesTabs.tsx'), 'utf8'));
    expect(NODES + nodesTabs).not.toMatch(/subDeleteNodeCount\(\s*servers\b/);
    const subscriptionOwner = code(readFileSync(join(SRC, 'components/screens/nodes/use-node-subscription-actions.ts'), 'utf8'));
    const hits = [...(NODES + nodesTabs + subscriptionOwner).matchAll(/subDeleteNodeCount\(\s*diskServers\b/g)];
    expect(hits.length, '两处（确认弹窗文案 + 工具栏 SubInfoBar）都要数磁盘').toBe(2);
  });
});

/**
 * `ruled` 行的落法**不经** `splitStagedOnly` 的那几条 —— 各自点名它由谁钉。
 * 空着写「它有别的门」是不行的：得说出是哪一扇，否则下一个人无从核对。
 */
const ENFORCED_ELSEWHERE: Record<string, string> = {
  'server.speedTest':
    'nodes-speedtest-wiring.test.ts：卡上置灰走 speedTestBlockReason 第三参，三个批量入口走 speedTestableIds 的 excludeIds —— 不是一次 split 调用',
};

describe('A7：`ruled` 不是贴标签 —— 该调用点必须真的查过表', () => {
  const SOURCE = new Map(
    [...new Set(SITES.map((s) => s.file))].map((f) => [
      f,
      code(readFileSync(join(SRC, f), 'utf8')),
    ])
  );

  it('登记涉及的文件都读得到（读空 ⇒ 下面那条恒真）', () => {
    for (const [f, src] of SOURCE) expect(src.length, f).toBeGreaterThan(200);
  });

  it('每条 ruled 的 op 都在该文件里被真正传进 splitStagedOnly（另有门的除外，且要点名）', () => {
    // P4 实测教训：把 WarpDialog 的 block 腿整段删掉、登记原样留着 —— A1 只管「调用点登记没登记」，
    // 照样全绿。`ruled` 于是变成一张贴上去就算数的标签。本条把它钉成「查过表」这个结构事实。
    const missing: string[] = [];
    for (const s of SITES.filter((x) => x.route === 'ruled')) {
      if (ENFORCED_ELSEWHERE[s.op!] !== undefined) continue;
      const src = SOURCE.get(s.file)!;
      if (!new RegExp(`splitStagedOnly\\(\\s*'${s.op!.replace('.', '\\.')}'`).test(src)) {
        missing.push(`${key(s)} → op '${s.op}' 没在本文件里传给 splitStagedOnly`);
      }
    }
    expect(
      missing,
      '登记说它按表分流，源码里却没有那次查表 —— 要么补上调用，要么改登记并在 ENFORCED_ELSEWHERE 里点名由谁钉'
    ).toEqual([]);
  });

  it('ENFORCED_ELSEWHERE 里的每一条都点名了具体哪扇门（不许写「另有门」）', () => {
    for (const [op, whom] of Object.entries(ENFORCED_ELSEWHERE)) {
      expect(whom, `${op} 没点名具体的门`).toMatch(/\.test\.tsx?/);
    }
  });

  it('speedTest 那扇门还在（ENFORCED_ELSEWHERE 指着的那个文件）', () => {
    const guard = readFileSync(
      join(SRC, 'components/screens/nodes/nodes-speedtest-wiring.test.ts'),
      'utf8'
    );
    expect(guard.length).toBeGreaterThan(1000);
    expect(guard, 'speedTest 的置灰口径没门守了').toMatch(
      /speedTestBlockReason\\\(server,\\s\*speedTestCaps,\\s\*stagedOnly/
    );
  });
});

describe('A6：TS/WARP 的远端写腿仍由 W-3 门守着', () => {
  const WRITE_GUARD = readFileSync(join(SRC, 'lib/config-write-wiring.test.ts'), 'utf8');

  it('写侧守卫文件读得到（读空 ⇒ 下面那条恒假/恒真，两种都得有人看一眼）', () => {
    expect(WRITE_GUARD.length).toBeGreaterThan(2000);
  });

  it('写侧那条「TS / WARP 的远端写腿恒 direct」还在', () => {
    // 本条只锚远端注册/登录弹窗的 W-3 门；它不再被当作「staged-only 永远造不出」的前提。
    expect(
      WRITE_GUARD,
      '写侧的 T5 没了 —— TS / WARP 远端操作的 W-3 去向失去守卫'
    ).toMatch(/TS \/ WARP 的远端注册\/登录写腿恒 direct/);
  });
});

describe('A4：仍依赖「恒不入暂存」的前提本身有门', () => {
  it('switchServer 仍在绕过表里', () => {
    expect(
      isBypassedOp('switchServer'),
      '切节点不再绕过暂存 ⇒ app-store 那行登记要重判'
    ).toBe(true);
  });
});
