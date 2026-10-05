import { runMobileSpeedTest, useMobileSpeedTestStore, mobileSpeedTestBusy } from '../use-mobile-speed-test';
/**
 * 移动端「节点」屏的**接线层**：读 store、发 IPC、把结果整形成 `view-model.ts` 的 props。
 * 呈现层（`NodesScreenView`）不认识 store，也不认识 `api` —— 这条分界是门能真渲染本屏的前提。
 *
 * # 判据一律复用桌面的既有纯函数，本文件不新造一条
 *
 * 分组（`groupServersBySubscription`）、筛选与排序（`projectVisibleServers`）、可测性与不可测理由
 * （`speedTestBlockReason` / `speedTestBlockedMessage`）、被覆盖网段（`shadowedCidrIndex`）、
 * 启动 gate 剔除理由（`invalidNodeIndex` + `invalidNodeReasonText`）、TS 出口警示
 * （`deriveTsExitWarning`）、组网隧道健康（`deriveMeshTunnelHealth`）、延迟陈旧（`isLatencyStale`）
 * —— 全部是桌面已有的单一真值。移动端另起一套判据，等于让同一个节点在两个客户端上得到两种解释。
 *
 * # 三个动作腿也是复用而不是重写
 *
 * 切出口走 `useSwitchNode`（**全仓唯一**的切换腿，桌面首页与节点页共用它，抄一份必然分叉）；
 * 测速走 `useNodeSpeedTest`；复制链接 / 批选 / 全选走 `useNodeActions`；
 * 删除三条（单删 / 批删 / 删订阅）走 `nodes/node-deletion.ts`，判据逐段复用桌面
 * `use-node-deletion.ts` 的那批纯函数。
 *
 * # 写失败必须可见（IA 裁定 #14）——**这条改变了「复用哪些腿」的答案**
 *
 * 立项时的根因是「移动外壳既没挂 `DialogHost`，也没有 toast 宿主」：`lib/error-handler` 的门面
 * 在无实现注入时静默落 console。桌面那两个宿主的外观全落在 `prototype.css` / `components.css`
 * = 桌面层叠链，import 即违约契约 A1。
 *
 * ⚠️ **这条根因 2026-09-06 两半都不成立了**：
 *  · toast 宿主 —— `mobile/MobileToaster.tsx`（批 1 / W-25），门面在移动端不再是 console；
 *  · 表单/弹层宿主 —— `mobile/forms/**`（批 2 / W-00）。它**不是**把 `DialogHost` 搬过来：
 *    字段规格与提交逻辑复用桌面的纯数据与纯函数（`field-spec.ts` / `node-spec.ts` /
 *    `proto-codec.ts` / `ts-settings-logic.ts` …），呈现层重写 ⇒ 契约 A1 一格没松。
 *
 * 处置不变，理由换成今天仍成立的那条：本屏用**行内错误**，一条 `notice` 槽位紧挨着控件，
 * 写失败当场出一行红字。**行内与全局 toast 不互斥**：一屏几十行节点，一条飘过去的 toast 说得清
 * 「发生了什么」，说不清「是哪一行」，而用户下一步要动的恰恰是那一行。
 *
 * ⚠️ 这条口径同时决定了**不复用** `useNodeSpeedTest` / `useNodeActions` 这两个 hook：
 * 它们的失败分支写死在 toast 门面上（`catch { toast.error(...) }`），今天不会再被吞掉，
 * 但落到的是**全局瞬态浮层**、不带行归属，且本屏的失败回显还要参与批选态的收敛。
 * （上一版这里写的是「包进 try/catch 只会得到一个永远不会触发的 catch」—— 那句话的前提是
 * 「移动端没有宿主」，宿主接上之后**已经反了**，故删掉，别照着它推理。）
 *
 * 处置：**判据全部复用，管道自己拥有**。复用的是会分叉的那部分 ——
 * 射程（`speedTestableIds` / `speedTestIdsForSelection`）、文案表
 * （`speedTestErrorMessage` / `notInPoolMessage` / `serverSwitchErrorText` / `subscriptionErrorDetail`）、
 * 后端切换收据与落库入口（`applyLatencyResults`）、
 * 唯一写入口（`useAppStore.switchServer` / `api.*`）。自己写的只有 try/catch 与「错误往哪显示」，
 * 而那**正是**两个客户端必然不同的那一格。
 *
 * 订阅刷新是例外：`refreshSubscriptionWithToast` **返回结构体**（`{ok}` / `{ok,detail}`），
 * 失败可观测且**带详情** ⇒ 整条复用，只补一行行内错误。
 * （2026-09-05 之前它只返回 `boolean`，detail 被吞在 toast 里；那条损失已消掉，见 `refreshSub`。）
 *
 * 所有写操作**必须**经 `runWrite` 这一个出口，门对着源码逐个对拍（见 `nodes-screen.test.tsx` ⑩）。
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { useDismissableLayer } from '../back-stack';
/* 本屏自己的样式面。外壳的 `mobile.css` 已定稿且不属本线射程，故另开一份并在
   `../mobile-entry.test.ts` 的契约 A1 允许集里登记它 —— 那道门是**集合恰等**，
   多一份少一份都红，所以「加一份 CSS」必须是一次显式登记，不能悄悄进链路。 */
import './nodes.css';
import { useTranslation } from 'react-i18next';
import { useAppStore, useEffectiveConfig, useEffectiveServers } from '@/store/app-store';
import type { ServerConfig } from '@/contracts/types';
import { useLatencyStore, isLatencyStale } from '@/store/use-latency-store';
import { useSubscriptionProgressStore } from '@/store/use-subscription-progress-store';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { withProxyStartClaim } from '@/lib/proxy-start-claim';
import { groupServersBySubscription } from '@/domain/server-grouping';
import {
  speedTestableIds,
  type SpeedTestCaps,
} from '@/domain/endpoint-routes';
import { isMeshNode, meshAllowsInternet } from '@/domain/endpoint-routes';
import { deriveTsExitWarning } from '@/domain/tailscale-exit-warning';
import { tsExitAction } from '../ts-exit-action';
import { loginAttemptActive } from '@/domain/tailscale-login-progress';
import { useTailscaleLoginProgressStore } from '@/store/use-tailscale-login-progress-store';
import { deriveMeshTunnelHealth } from '@/domain/mesh-tunnel-health';
import { invalidNodeReasonText } from '@/domain/invalid-node-reason';
import { subscriptionErrorDetail } from '@/domain/subscription-error-text';
import { refreshSubscriptionWithToast } from '@/domain/subscription-refresh';
import { stagedOnlyIds } from '@/lib/staged-config';
import { useVpnStatusStore } from '@/store/use-vpn-status-store';
import {
  invalidNodeIndex,
  protocolLabel,
  speedTestBlockReason,
  speedTestIdsForSelection,
  transferSummary,
} from '@/components/screens/nodes/nodes-logic';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import { asMeshRouteReport } from '../MobileMeshRouteEvidence';
import { meshRouteMatchesVisibleConfig } from '../mesh-route-context';
import { projectVisibleServers } from '@/components/screens/nodes/nodes-list-projection';
import {
  notInPoolMessage,
  speedTestBlockedMessage,
  speedTestErrorMessage,
} from '@/components/screens/shared/speedtest-feedback';
import { serverSwitchErrorText } from '@/domain/action-error-text';
import { mobileSwitchReceiptFeedback } from '../mobile-switch-feedback';
import { IpcError } from '@/ipc';
import { initialNodesTab } from '@/components/screens/nodes/initial-tab';
import { canCloneServer, meshSingletonConflict } from '@/domain/endpoint-routes';
import { editRoute } from '@/lib/staged-config';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { navigateMobile } from '../navigate';
import {
  closeMobileForm,
  mobileEditFormFor,
  openMobileForm,
  type MobileConfirmPayload,
} from '../forms/form-store';
import { useMobileNodeDeletion } from './node-deletion';
import { ROW_ACTIONS } from './absence-register';
import { NodesScreenView } from './NodesScreenView';
import { buildAddItems, buildRowItems, buildSubItems, buildSubSummary } from './view-model';
import type {
  EmptyKind,
  GroupTabVM,
  NodeRowVM,
  NodesListSortKey,
  SheetItem,
  SubSummaryVM,
} from './view-model';

/* 协议显示名与传输摘要：直接用桌面 `NodeCard` 的同一份（`nodes-logic#protocolLabel` /
   `#transferSummary`）。此前这里抄了一张表，MASQUE / Tailcat 上线时没跟上 —— 见
   `protocol-label-parity.test.ts`。 */

export function MobileNodesScreen(): ReactElement {
  const { t } = useTranslation();
  const config = useEffectiveConfig();
  const servers = useEffectiveServers();
  const diskServers = useAppStore((s) => s.servers);
  const selectedServerId = useAppStore((s) => s.selectedServerId);
  const proxyRunning = useAppStore((s) => !!s.proxyStatus?.running);
  const startProxy = useAppStore((s) => s.startProxy);
  const invalidNodes = useAppStore((s) => s.invalidNodes);
  const latencyMap = useLatencyStore((s) => s.latencyMap);
  const testedAt = useLatencyStore((s) => s.testedAt);
  const subscriptionProgress = useSubscriptionProgressStore((s) => s.progress);

  const subscriptions = useMemo(() => config?.subscriptions ?? [], [config?.subscriptions]);
  const groups = useMemo(
    () => groupServersBySubscription(servers, subscriptions, true),
    [servers, subscriptions],
  );

  const [tab, setTab] = useState<string | null>(null);
  /* 冷启动落点与桌面同一条判据（`initialNodesTab`），不另猜一个。`groups` 还没水合时它返回
     `null` —— 那时定位没有信息量，落成空串即「一个都不选中」，等 groups 到齐再自然命中。 */
  const activeTab = tab ?? initialNodesTab(groups, selectedServerId) ?? '';
  const activeGroup = groups.find((g) => g.id === activeTab);
  const activeSub = subscriptions.find((s) => s.id === activeTab);

  const [search, setSearch] = useState('');
  const [protoFilter, setProtoFilter] = useState('');
  const [sortKey, setSortKey] = useState<NodesListSortKey>('default');
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  /* 批量选择是一个**模式态**，系统返回键先退出它（分组段不吃返回：那是屏内视图，不是栈）。
     退出 = 关模式 + 清选中，与批量条上那颗退出按钮**同一个闭包**，不另写一份。 */
  const exitBatchMode = useCallback(() => {
    setBatchMode(false);
    setSelectedIds(new Set());
  }, []);
  useDismissableLayer(batchMode, exitBatchMode);
  const [notice, setNotice] = useState<
    { tone: 'ok' | 'info' | 'warn' | 'err'; text: string } | undefined
  >();
  const [tsState, setTsState] = useState<{ id: string; exists: boolean | null }>({ id: '', exists: null });
  const tsStateRequest = useRef(0);
  const onRowActionsOpen = useCallback((row: NodeRowVM) => {
    const request = ++tsStateRequest.current;
    if (row.server.protocol.toLowerCase() !== 'tailscale' || row.stagedOnly) return;
    const id = row.server.id;
    setTsState({ id, exists: null });
    void api.server.tailscaleStateExists([id], true).then((states) => {
      if (request !== tsStateRequest.current) return;
      setTsState({ id, exists: typeof states[id] === 'boolean' ? states[id] : null });
    }, () => {
      if (request === tsStateRequest.current) setTsState({ id, exists: null });
    });
  }, []);

  const visibleServers = useMemo(
    () => projectVisibleServers(activeGroup, search, protoFilter, sortKey, latencyMap),
    [activeGroup, search, protoFilter, sortKey, latencyMap],
  );

  const protoOptions = useMemo(() => {
    const set = new Set<string>();
    activeGroup?.servers.forEach((s) => set.add(s.protocol));
    return [...set].sort();
  }, [activeGroup]);

  const stagedOnly = useMemo(() => stagedOnlyIds(servers, diskServers), [servers, diskServers]);
  const speedTestCaps = useMemo<SpeedTestCaps>(() => ({ mainCorePool: proxyRunning }), [proxyRunning]);
  const invalidIndex = useMemo(() => invalidNodeIndex(invalidNodes), [invalidNodes]);
  /* 新报告带运行代和加载证据；旧 absorbed 仅精确字串去重，不能充当本层归属。 */
  const [meshRoute, setMeshRoute] = useState<{
    report: MeshRouteReport | null; previous: boolean; legacy: boolean;
  }>({ report: null, previous: false, legacy: false });
  const meshRouteEpoch = useRef(0);
  useEffect(() => {
    const epoch = ++meshRouteEpoch.current;
    setMeshRoute(previous => ({ ...previous, previous: previous.report !== null }));
    void api.config.meshRouteReport().then((raw: unknown) => {
      if (epoch !== meshRouteEpoch.current) return;
      const report = asMeshRouteReport(raw);
      setMeshRoute({ report, previous: false, legacy: report === null });
    }, () => {
      if (epoch !== meshRouteEpoch.current) return;
      setMeshRoute(previous => ({ ...previous, previous: previous.report !== null }));
    });
    return () => {
      meshRouteEpoch.current += 1;
    };
  }, [config, servers, selectedServerId, proxyRunning]);
  const serverNameById = useMemo(
    () => new Map(servers.map((s) => [s.id, s.name])),
    [servers],
  );
  const meshReportMatchesScreen = useMemo(
    () => meshRouteMatchesVisibleConfig(meshRoute.report, config, { servers }),
    [meshRoute.report, config, servers],
  );

  // ── 写操作：唯一出口 `runWrite`（IA 裁定 #14）─────────────────────────────
  const switchServer = useAppStore((s) => s.switchServer);
  const globalTesting = useMobileSpeedTestStore(mobileSpeedTestBusy);
  const [testing, setTesting] = useState(false);
  const [copyBusy, setCopyBusy] = useState(false);
  const copyInFlight = useRef(false);
  const [refreshBusy, setRefreshBusy] = useState(false);
  const refreshInFlight = useRef(false);

  /**
   * **全部**写操作的唯一出口。失败一律落成行内错误，绝不只把状态弹回原值。
   *
   * 判据不许只钉「有 catch」—— 有 catch 而 catch 里什么都不做，正是要抓的那个形态。
   * 故这里的 catch **必须**写进 `setNotice`，门对着这个函数体断言（`nodes-screen.test.tsx` ⑩）。
   */
  const runWrite = useCallback(
    async (op: () => Promise<void>, describe: (err: unknown) => string): Promise<void> => {
      try {
        await op();
      } catch (err) {
        console.error('[mobile-nodes] write failed:', err);
        const message = describe(err);
        setNotice({ tone: 'err', text: message });
      }
    },
    [],
  );

  /** 切出口共用 store 写腿；只有后端收据为 applied 才报告已生效。 */
  const onUseAsExit = useCallback(
    (row: NodeRowVM) => {
      if (row.server.id === selectedServerId) return;
      void runWrite(
        async () => {
          const receipt = await switchServer(row.server.id);
          const feedback = mobileSwitchReceiptFeedback(receipt, row.server.name, t);
          if (feedback?.tone === 'success') {
            setNotice(undefined);
            toast.success(feedback.text);
          } else if (feedback) setNotice({ tone: feedback.tone === 'warning' ? 'warn' : 'info', text: feedback.text });
          else if (receipt.status === 'notRunning') setNotice(undefined);
        },
        (err) => serverSwitchErrorText(err instanceof IpcError ? err.code : undefined, t),
      );
    },
    [runWrite, switchServer, selectedServerId, t],
  );

  /** 一轮测速。射程由调用方按复用的谓词算好；本函数只管发、落库、报结局。 */
  const runSpeedTest = useCallback(
    (ids: readonly string[]) => {
      if (ids.length === 0) {
        setNotice({ tone: 'info', text: t('nodes.noTestableNodes') });
        return;
      }
      setTesting(true);
      void runWrite(
        async () => {
          try {
            const result = await runMobileSpeedTest(ids, 'nodes');
            if (!result) return;
            /* 「有节点没进本轮」不是失败，是必须说出来的事实（后端 `zero_testable_envelope`
               的同一口径：每一类非零的数都报）。 */
            const skipped = notInPoolMessage(result, t);
            if (skipped !== null) setNotice({ tone: 'info', text: skipped });
          } finally {
            setTesting(false);
          }
        },
        (err) => speedTestErrorMessage(err, t),
      );
    },
    [runWrite, t],
  );

  /**
   * 复制分享链接。桌面把**两类失败分开报**，这里照搬：
   * 「该协议没有标准分享链接」与「链接生成好了但没写进剪贴板」是两回事 ——
   * 用同一个 catch 兜住会把后者谎报成前者，用户据此以为协议不支持，实际重试即可。
   */
  const copyLink = useCallback(
    (row: NodeRowVM) => {
      if (copyInFlight.current) return;
      copyInFlight.current = true;
      setCopyBusy(true);
      void runWrite(async () => {
        let url: string;
        try {
          url = await api.server.generateUrl(row.server);
        } catch (err) {
          console.error('[mobile-nodes] generate share url failed:', err);
          setNotice({ tone: 'err', text: t('nodes.copyLinkUnsupported') });
          return;
        }
        await navigator.clipboard.writeText(url);
        setNotice(undefined);
        toast.success(t('nodes.copyLinkOk'));
      }, () => t('nodes.copyLinksFailed')).finally(() => {
        copyInFlight.current = false;
        setCopyBusy(false);
      });
    },
    [runWrite, t],
  );

  /** 批量复制：`allSettled` 而非 `all` —— 混进一个无分享链接形态的协议，`all` 会整体 reject，
   *  本可成功的链接一条都进不了剪贴板（桌面同一条理由）。 */
  const copyLinksBatch = useCallback(
    (targets: readonly ServerConfig[]) => {
      if (targets.length === 0) return;
      if (copyInFlight.current) return;
      copyInFlight.current = true;
      setCopyBusy(true);
      void runWrite(async () => {
        const settled = await Promise.allSettled(targets.map((x) => api.server.generateUrl(x)));
        const urls = settled
          .filter((r): r is PromiseFulfilledResult<string> => r.status === 'fulfilled')
          .map((r) => r.value);
        const skipped = settled.length - urls.length;
        if (urls.length === 0) {
          setNotice({ tone: 'err', text: t('nodes.copyLinkUnsupported') });
          return;
        }
        await navigator.clipboard.writeText(urls.join('\n'));
        setNotice(undefined);
        toast.success(t(skipped > 0 ? 'nodes.copyLinksPartial' : 'nodes.copyLinksOk', { count: urls.length, skipped }));
      }, () => t('nodes.copyLinksFailed')).finally(() => {
        copyInFlight.current = false;
        setCopyBusy(false);
      });
    },
    [runWrite, t],
  );

  /**
   * 订阅刷新：`refreshSubscriptionWithToast` **返回结构体**（三态语义是它的存在理由，抄一份
   * 必然分叉）⇒ 整条复用，只补一行行内错误。
   *
   * 2026-09-05 之前它只返回 `boolean`，后端给的具体 detail 被吞在 toast 里，而移动外壳没有
   * toast 宿主 ⇒ 用户读到的永远是「刷新失败」四个字，为什么失败一个字都没有。现在详情随返回值
   * 一起过来，直接进屏级 `notice` —— **不另造全局宿主**（IA 裁定 #14），复用本屏唯一那条通道。
   */
  const refreshSub = useCallback(
    (subId: string) => {
      if (refreshInFlight.current) return;
      refreshInFlight.current = true;
      setRefreshBusy(true);
      void runWrite(async () => {
        const r = await refreshSubscriptionWithToast(subId, t);
        if (!r.ok) setNotice({ tone: 'err', text: r.detail });
      }, () => t('nodes.subRefreshFail')).finally(() => {
        refreshInFlight.current = false;
        setRefreshBusy(false);
      });
    },
    [runWrite, t],
  );

  // ── 删除腿 + 克隆（W-02 / W-03）─────────────────────────────────────────────
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);

  /** 叠一层确认面板；返回那一层的身份，供 `onConfirm` 关掉自己（不按栈顶猜）。 */
  const confirmLayer = useCallback(
    (payload: MobileConfirmPayload) => openMobileForm({ kind: 'confirm', payload }),
    [],
  );

  const { deleteNode, deleteBatch, deleteSubscription } = useMobileNodeDeletion({
    t,
    /* 删除腿走**本屏那个唯一写出口**（裁定 #14）：它自带 try/catch 会在同一个屏上开出
       第二条失败回显通道，而「唯一出口」正是本屏那道门判的形态。 */
    runWrite,
    clearNotice: () => setNotice(undefined),
    confirm: confirmLayer,
    dismiss: closeMobileForm,
    exitBatch: exitBatchMode,
  });

  /**
   * 克隆 —— **没有表单**，是一次直调 `server_add` 的动作（同桌面 `use-node-actions.ts:60`）。
   *
   * 剥 `id`（新建）+ `subscriptionId` / `providerName`（克隆体归自建，不随订阅刷新被当差集删掉）。
   * 单例拦截走**同一条纯函数** `meshSingletonConflict`：克隆是绕开表单直调 `server_add` 的一条
   * 造节点路径，后端无守卫 ⇒ 不在此拦就能造出第二个 TS/WARP 实例（TS 多实例互相顶掉 tailnet 地址；
   * WARP 抢内核 utun 致 `Connect: resource busy`）。**不传 editingId**：克隆语义恒为「再加一个」，
   * 源节点自身即占槽 ⇒ 必然被拦，与契约一致。
   */
  const cloneNode = useCallback(
    (server: ServerConfig) => {
      const slot = meshSingletonConflict(server, servers);
      if (!canCloneServer(server) || slot) {
        /* `meshSingletonConflict` 只剩 WARP 一支（Tailscale 那一支 2026-09-11 已撤：
           「多个 TS 互相顶掉 tailnet 地址」被真控制面实测推翻，自建 headscale 的 `prefixes.v4`
           可配成任意段）。随之删掉的 `nodes.cloneTsSingleton` 说的正是那句假话。
           真实相交由生成侧 `endpoint_force_route_report` 检出 —— 创建期判不了，
           新节点还没连上控制面，前缀不可知。 */
        setNotice({ tone: 'err', text: t('nodes.cloneWarpSingleton') });
        return;
      }
      const { id: _id, subscriptionId: _sub, providerName: _prov, ...rest } = server;
      const cloneName = t('nodes.cloneName', { name: server.name });
      void runWrite(
        async () => {
          // 暂存闸门与节点表单同一个 `editRoute`，不在这里另写一条判定。
          const staged = editRoute('servers', stagingEnabled) === 'staged';
          if (staged) {
            const entityId = crypto.randomUUID();
            stage({
              id: `server:${entityId}`,
              kind: 'server',
              label: `${t('node.addTitle')} ${cloneName}`,
              entityPath: ['servers', entityId],
              nextValue: { ...rest, id: entityId, name: cloneName },
            });
          } else {
            await api.server.add({ ...rest, name: cloneName });
            await useAppStore.getState().loadConfig(true);
          }
          /* 副本落在自建分组；直接写入完成后给短回执，暂存由底部待应用条持续说明。 */
          setNotice(undefined);
          if (!staged) toast.success(t('nodes.cloneSuccess'));
        },
        () => t('nodes.cloneFail'),
      );
    },
    [servers, t, runWrite, stagingEnabled, stage],
  );

  // ── 纯本地状态动作（不写盘，故不经 runWrite）────────────────────────────────
  const toggleSelect = useCallback((row: NodeRowVM) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(row.server.id)) next.delete(row.server.id);
      else next.add(row.server.id);
      return next;
    });
  }, []);

  const selectAll = useCallback(() => {
    const all = selectedIds.size === visibleServers.length && visibleServers.length > 0;
    setSelectedIds(all ? new Set() : new Set(visibleServers.map((x) => x.id)));
  }, [visibleServers, selectedIds.size]);

  /* 「全部测速」的射程是**全部**节点，不是当前分组里可见的那批 —— 拿可见数去禁用它，
     会在「当前 tab 恰好一个可测节点都没有」时把一个本来有目标的动作锁死。 */
  const testableTotal = useMemo(
    () => speedTestableIds(servers, speedTestCaps, stagedOnly).length,
    [servers, speedTestCaps, stagedOnly],
  );

  // ── 安全注脚：TS 出口警示（IA 裁定 #3）──────────────────────────────────────
  const selectedServer = servers.find((s) => s.id === selectedServerId);
  const tsId = selectedServer?.protocol?.toLowerCase() === 'tailscale' ? selectedServer.id : undefined;
  const tsLoggedIn = useAppStore((s) => (tsId ? !!s.tailscaleLoginStates[tsId] : false));
  const tsStatus = useAppStore((s) => (tsId ? s.tailscaleStatuses[tsId] : undefined));
  const tsAuthUrl = useAppStore((s) => (tsId ? s.tailscaleAuthUrls[tsId] : undefined));
  const tsLoginAttempt = useTailscaleLoginProgressStore((s) => (tsId ? s.attempts[tsId] : undefined));
  const tsExitWarning = deriveTsExitWarning({
    selectedServer,
    loggedIn: tsLoggedIn,
    proxyModeDirect: (config?.proxyMode || 'smart').toLowerCase() === 'direct',
    proxyRunning,
    status: tsStatus,
  });
  /** Match the action to the warning, not merely the presence of an auth URL. */
  const onTsExitAction = useCallback(() => {
    if (tsId === undefined) return;
    const storeUrl = !tsLoginAttempt || loginAttemptActive(tsLoginAttempt.phase) ? tsAuthUrl : null;
    const action = tsExitAction(tsExitWarning, tsStatus?.authURL, storeUrl);
    if (action.kind !== 'login-url') {
      openMobileForm({ kind: action.kind === 'login-panel' ? 'ts-login' : 'ts-exit', serverId: tsId });
      return;
    }
    void runWrite(() => api.system.openExternal(action.url), () => t('errors.operationFailed'));
  }, [tsExitWarning, tsStatus?.authURL, tsAuthUrl, tsLoginAttempt, tsId, runWrite, t, openMobileForm]);

  // ── 组网隧道健康：只在组网分组（IA 裁定 #3 后半）────────────────────────────
  const openVpnStatus = useVpnStatusStore((s) => (selectedServer ? s.openVpn[selectedServer.id] : undefined));
  const meshTunnelHealth =
    activeGroup?.isMesh === true
      ? deriveMeshTunnelHealth({ selectedServer, proxyRunning, tsStatus, openVpnStatus })
      : 'none';

  // ── 视图模型 ────────────────────────────────────────────────────────────────
  const groupTabs = useMemo<GroupTabVM[]>(
    () =>
      groups.map((g) => {
        const progress = subscriptionProgress[g.id];
        return {
          id: g.id,
          label: g.isManual ? t('nodes.tab.manual') : g.isMesh === true ? t('nodes.tab.mesh') : g.name,
          count: g.servers.length,
          failureDetail:
            progress?.phase === 'failed' ? subscriptionErrorDetail(progress, t) : undefined,
        };
      }),
    [groups, subscriptionProgress, t],
  );

  const rows = useMemo<NodeRowVM[]>(
    () =>
      visibleServers.map((server) => {
        const mesh = activeGroup?.isMesh === true || isMeshNode(server);
        const meshRouteContextMismatch = mesh && meshRoute.report?.snapshot.configSource === 'running' &&
          (!meshReportMatchesScreen || stagedOnly.has(server.id));
        const reason = speedTestBlockReason(server, speedTestCaps, stagedOnly.has(server.id));
        return {
          server,
          isCurrent: server.id === selectedServerId,
          isExit: mesh,
          lanOnly: mesh && !meshAllowsInternet(server),
          latencyMs: latencyMap[server.id],
          latencyStale: isLatencyStale(testedAt[server.id]),
          speedTestable: reason === null,
          speedTestBlockedHint: reason ? speedTestBlockedMessage(reason, t) : undefined,
          invalidReason:
            invalidNodeReasonText(invalidIndex[server.id], (k, f) => (f === undefined ? t(k) : t(k, f))) ??
            undefined,
          stagedOnly: stagedOnly.has(server.id),
          meshRouteReport: mesh ? meshRoute.report : undefined,
          meshRoutePrevious: meshRoute.previous,
          meshRouteLegacy: meshRoute.legacy,
          meshRouteContextMismatch,
          meshRouteNames: meshRouteContextMismatch ? undefined : serverNameById,
          deletable: server.subscriptionId === undefined,
          transport: transferSummary(server),
          protocolLabel: protocolLabel(server.protocol),
        };
      }),
    [
      visibleServers,
      activeGroup,
      speedTestCaps,
      stagedOnly,
      selectedServerId,
      latencyMap,
      testedAt,
      invalidIndex,
      meshRoute,
      meshReportMatchesScreen,
      serverNameById,
      t,
    ],
  );

  const sub = useMemo<SubSummaryVM | undefined>(() => {
    if (!activeSub) return undefined;
    /* 整块算值住在 `view-model.ts#buildSubSummary`（纯函数，判据够得着）——
       本屏只负责把入参取齐。抽出来的理由与三张动作表同一条，见那份文件里那段头注。 */
    return buildSubSummary({
      t,
      sub: activeSub,
      nodeCount: activeGroup?.servers.length ?? 0,
      progress: subscriptionProgress[activeSub.id],
      config: config ?? undefined,
    });
  }, [activeSub, activeGroup, subscriptionProgress, config, t]);

  const emptyKind: EmptyKind =
    search !== '' || protoFilter !== ''
      ? 'filtered'
      : activeSub
        ? 'sub'
        : activeGroup?.isMesh === true
          ? 'mesh'
          : 'all';

  const countSummary = t('nodes.stats', {
    total: servers.length,
    manual: groups.find((g) => g.isManual)?.servers.length ?? 0,
    mesh: groups.find((g) => g.isMesh === true)?.servers.length ?? 0,
    sub: servers.filter((s) => s.subscriptionId !== undefined).length,
  });

  /**
   * 「添加」的四条路径（W-01，2026-09-06 全部接通）。
   *
   * 后端四条腿全在且**无平台门控**：`server_add`（手动添加 / 组网隧道）·`server_add_bulk`（导入）
   * ·`subscription_create_start`（订阅）·`local_import_parse`（导入的解析那一步）。
   * 前端此前缺的只有**表单层** —— 桌面那条链住在 `components/dialogs/**`、外观在桌面层叠链上
   * （契约 A1 禁入），故本批另建了 `mobile/forms/**`：字段规格与提交逻辑复用桌面的纯数据/纯函数，
   * 只重写呈现层。
   *
   * 四条与登记表 `ADD_ACTIONS` 一一对应，门拿桌面 `NodesHeader` 的菜单项数与它对差
   * （桌面加第五项 ⇒ 红，逼一次显式处置决定）。
   *
   * # 🔴 四条**都**要把 tab 切到自己的产物落点（2026-09-06 复审 major）
   *
   * 分组由 `domain/server-grouping.ts` 算：手动添加与导入的产物落 `'manual'`（:53）、
   * 组网隧道落 `'mesh'`（:56）、新订阅落它自己的 id（:62）。当前 tab 停在别处时，新行不在
   * `visibleServers` 里 ⇒ 表单一关屏上**零变化、零文字**，用户读作「点了没反应」——
   * 那正是本屏立项要修的那件事。桌面四条逐条如此（`NodesHeader.tsx:81/:111/:125`、
   * `NodesScreen.tsx:491`），本仓自己也认过这条：`cloneNode` 里那句 `nodes.cloneSuccess`
   * 的理由逐字就是「当前若停在订阅 tab，新行不在本 tab 可见 ⇒ 点了完全没反应」。
   *
   * 切的**时机**与桌面逐条对齐，不统一：手动添加 / 组网接入是**开表之前**就切（用户在填表时
   * 背后那一屏已经是产物会出现的地方）；导入与订阅是**成功之后**才切（那两条都可能什么都没导进来，
   * 提前切等于为一次没发生的新增改变了用户的浏览位置）。
   */
  const addItems = useMemo<SheetItem[]>(
    () =>
      buildAddItems(t, {
        onManualAdd: () => {
          setTab('manual');
          openMobileForm({ kind: 'node' });
        },
        onMeshJoin: () => {
          setTab('mesh');
          openMobileForm({ kind: 'mesh-join' });
        },
        onImport: () => openMobileForm({ kind: 'import', onAdded: () => setTab('manual') }),
        /* 新增成功后切到那条订阅的分组：新订阅落在一个当前 tab 看不见的地方时，
           用户点完只会觉得什么都没发生（同桌面 `onAdded: setActiveTab`）。 */
        onAddSubscription: () => openMobileForm({ kind: 'sub', onAdded: (id) => setTab(id) }),
      }),
    [t],
  );

  /**
   * 订阅「更多」的五条（W-04，2026-09-06 全部接通）。逐条与 `SUB_MENU_ACTIONS` 对应。
   *
   * 「重命名」与「编辑」开**同一张**订阅表（Polaris 只有一个订阅表单），靠 `focus` 落点区分 ——
   * 不摆两个点下去完全一样的菜单项（口径逐字取自桌面 `use-node-subscription-actions.ts`）。
   *
   * 「更新间隔」是**全局**设置（Polaris 没有 per-sub 间隔字段，调度器只读
   * `config.subscriptionUpdateIntervalHours`）⇒ 跳到设置→更新那唯一一处真开关，
   * 不做一个假的「本订阅间隔」。跳不动时（没有导航器）**如实说**，不假装跳过去了。
   */
  const subItems = useMemo<SheetItem[]>(() => {
    if (!activeSub) return [];
    const sub = activeSub;
    return buildSubItems(t, {
      onRename: () => openMobileForm({ kind: 'sub', subId: sub.id, focus: 'name' }),
      onEditUrl: () => openMobileForm({ kind: 'sub', subId: sub.id, focus: 'url' }),
      onCopyUrl: () => {
        if (copyInFlight.current) return;
        copyInFlight.current = true;
        setCopyBusy(true);
        void runWrite(async () => {
          await navigator.clipboard.writeText(sub.url);
          setNotice(undefined);
          toast.success(t('nodes.subCopyUrlOk'));
        }, () => t('nodes.copyLinksFailed')).finally(() => {
          copyInFlight.current = false;
          setCopyBusy(false);
        });
      },
      /* 跳不动时（没有装载导航器）**如实说**，不假装跳过去了 —— `navigateMobile` 返回 false
         正是为此存在的。落回的那句话就是那一项自己的常驻说明（间隔是全局的）。 */
      onInterval: () => {
        if (!navigateMobile('settings', 'update')) {
          setNotice({ tone: 'warn', text: t('nodes.subIntervalHint') });
        }
      },
      onDelete: () => deleteSubscription(sub),
    });
  }, [activeSub, t, runWrite, deleteSubscription]);

  /**
   * 行动作面里的「连接」（K3-RV03 / IA §4.10 的第三件事：“plus an action that connects”）。
   *
   * 与首页那颗**同一条腿**：`withProxyStartClaim` 认领本次起核，否则提权门那两码
   * （`HELPER_NOT_INSTALLED` / `HELPER_GATE_ABORTED`）会被事件腿再报一遍（后端是双出口，
   * 理由逐字见 `home/MobileHomeScreen.tsx` 的 `onToggleConnect`）。
   *
   * **进行态回显**：动作面选中即关闭，本屏没有首页那颗按钮的 `connectBusy` 内联忙态 ⇒
   * 起核那几秒屏上什么都不动的话，用户会以为没点着。故这里落一条 `notice`，用的是已存在的
   * 五语种键 `home.statusStarting`（「启动中」），不新造键。起核完成后 `proxyRunning` 翻转，
   * 这颗动作从面板里消失、测速那条的置灰理由随之解除 —— 那是最终状态的回显。
   */
  const connectFromRow = useCallback(() => {
    setNotice({ tone: 'info', text: t('home.statusStarting') });
    void runWrite(
      () => withProxyStartClaim(() => startProxy()),
      (err) => (err instanceof Error ? err.message : t('mobileHome.actionFailed')),
    );
  }, [runWrite, startProxy, t]);

  const rowItems = useCallback(
    (row: NodeRowVM): SheetItem[] =>
      buildRowItems({
        t,
        row,
        coreRunning: proxyRunning,
        tsLoginState: tsState.id === row.server.id ? tsState.exists : null,
        disposition: (id) => ROW_ACTIONS.find((a) => a.id === id)?.disposition,
        handlers: {
          onSpeedTest: () => runSpeedTest([row.server.id]),
          onConnect: connectFromRow,
          onCopyLink: () => copyLink(row),
          onTsLogin: () => openMobileForm({ kind: 'ts-login', serverId: row.server.id }),
          onClone: () => cloneNode(row.server),
          /* 该开哪张表由**两端共用**的分流函数决定（`node-edit-routing#nodeEditForm` 的移动端
             名字翻译层）。批 3 起它对每一个协议都给得出一张表，不再有「编辑不动」那一档。 */
          onEdit: () => openMobileForm(mobileEditFormFor(row.server)),
          onDelete: () => deleteNode(row.server),
        },
      }),
    [t, proxyRunning, tsState, runSpeedTest, copyLink, connectFromRow, cloneNode, deleteNode],
  );

  return (
    <NodesScreenView
      t={t}
      groups={groupTabs}
      activeTab={activeTab}
      onSelectTab={(id) => {
        setTab(id);
        setSelectedIds(new Set());
        /* 切分组时清掉上一条反馈：「已切换到 X」留在另一个分组上读作当前事实，是陈旧信息。 */
        setNotice(undefined);
      }}
      sub={sub}
      onRefreshSub={() => {
        if (activeSub) refreshSub(activeSub.id);
      }}
      subItems={subItems}
      search={search}
      onSearch={setSearch}
      protoFilter={protoFilter}
      protoOptions={protoOptions}
      onProtoFilter={setProtoFilter}
      sortKey={sortKey}
      onSortKey={setSortKey}
      rows={rows}
      emptyKind={emptyKind}
      countSummary={countSummary}
      tsExitWarning={tsExitWarning}
      onTsExitAction={onTsExitAction}
      meshTunnelHealth={meshTunnelHealth}
      coreRunning={proxyRunning}
      testing={testing || globalTesting}
      copyBusy={copyBusy}
      refreshBusy={refreshBusy}
      onTestAll={() => runSpeedTest(speedTestableIds(servers, speedTestCaps, stagedOnly))}
      onTestVisible={() =>
        runSpeedTest(speedTestableIds(visibleServers, speedTestCaps, stagedOnly))
      }
      addItems={addItems}
      onUseAsExit={(row) => void onUseAsExit(row)}
      rowItems={rowItems}
      onRowActionsOpen={onRowActionsOpen}
      testableTotal={testableTotal}
      batchMode={batchMode}
      onToggleBatchMode={() => {
        if (batchMode) {
          exitBatchMode();
          return;
        }
        setBatchMode(true);
        setSelectedIds(new Set());
      }}
      selectedIds={selectedIds}
      onToggleSelect={toggleSelect}
      onSelectAll={selectAll}
      onBatchCopyLinks={() =>
        copyLinksBatch(visibleServers.filter((x) => selectedIds.has(x.id)))
      }
      onBatchSpeedTest={() =>
        runSpeedTest(speedTestIdsForSelection(visibleServers, selectedIds, speedTestCaps, stagedOnly))
      }
      onBatchDelete={() => deleteBatch(selectedIds, visibleServers)}
      notice={notice}
    />
  );
}

export default MobileNodesScreen;
