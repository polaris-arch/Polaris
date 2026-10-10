/**
 * 移动端「首页」的**接线层**：读 store、订阅事件流、发 IPC，把结果整形成 `view-model.ts` 的 props。
 * 呈现层（`HomeScreenView`）不认识 store，也不认识 `api` —— 这条分界是门能真渲染本屏的前提。
 *
 * 设计文档：`~/docs/polaris/design/polaris-mobile-screen-home-2026-09-04.md`。
 *
 * # 判据一律复用既有单一真值，本文件不新造一条
 *
 * 连接三态（`deriveTakeoverConnState`）、哨兵判定（`isDirectSelection` /
 * `isBlockSelection`）、延迟陈旧（`isLatencyStale`）、出口地区（`resolveExitRegion` /
 * `resolveExitNodeFlagCode`）、TS 出口警示（`deriveTsExitWarning`）、订阅生命周期
 * （`createTopicSubscription`）—— 全是桌面已有的腿。移动端另起一套，等于让同一份状态在两个客户端上
 * 得到两种解释。
 *
 * # 事件流：**桌面与 Android 共用同一份 relay 与同一批事件**
 *
 * 本屏订两个 topic：`stats`（`TrafficStats`，喂流量图与诊断行）与 `detail`
 * （`ConnectionsDetailUpdate`，喂三张分析卡）。**不订 `aggregate`** —— 那是连接页 Top-N 排名专用，
 * 且它的 `count` 是**连接条数不是字节数**，按字节排名的卡一个都不能用它
 * （`data-contract.json#sources.connections-aggregate.traps`）。
 *
 * `detail` 是增量流，两条协议不变量必须由消费方兑现，漏一条就会把两份数据集合并：
 *  · `reset=true` 帧携带完整基线 ⇒ 整表替换；
 *  · `generation` 变了 ⇒ 前一代的一切作废（重连 / reset / 离线之后就是这样）；
 *    忽略 `generation` 的消费方会在一次重连之后把两个数据集混在一起。
 *
 * # 写失败必须可见（IA 裁定 #14）
 *
 * 所有写操作**必须**经 `runWrite` 这一个出口（`write-errors.ts` 的纯工厂），门对着源码逐个对拍。
 * 移动端没有 toast / 弹窗宿主，且本批不许往外壳里造 ⇒ 失败落成**贴着那颗控件**的行内红字。
 *
 * # 桌面屏只读不改
 *
 * 从 `components/screens/**` 引入的全部是 `.ts` 的纯函数 / 纯逻辑（`connection-state.ts`、
 * `format.ts`）。桌面 `.tsx` 一个都不引 —— 那是契约 A1 与节点屏那道门共同钉死的边界。
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
/* 本屏自己的样式面。外壳的 `mobile.css` 已定稿且不属本线射程，故另开一份并在
   `../mobile-entry.test.ts` 的契约 A1 允许集里登记 —— 那道门是**集合恰等**，
   多一份少一份都红，所以「加一份 CSS」必须是一次显式登记，不能悄悄进链路。 */
import './home.css';
import { useTranslation } from 'react-i18next';
import { api, unlockApi } from '@/ipc';
import { manualNetworkContextMatches } from '@/contracts/manual-network-check';
import { manualSpeedTestCaps } from '@/domain/endpoint-routes';
import type {
  ConnectionsDetailUpdate,
  ProxyMode,
  ProxyModeType,
  RuleAction,
  ServerConfig,
  TrafficStats,
} from '@/contracts/types';
import {
  getEffectiveConfig,
  useAppStore,
  useEffectiveConfig,
  useEffectiveRules,
  useEffectiveServers,
} from '@/store/app-store';
import { isLatencyStale, useLatencyStore } from '@/store/use-latency-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import {
  BLOCK_SERVER_ID,
  DIRECT_SERVER_ID,
  isBlockSelection,
  isDirectSelection,
  isSentinelSelection,
} from '@/domain/direct-selection';
import { ruleSubjectForValue, ruleTypeNameKey, type RuleSubject } from '@/domain/rules';
import {
  analyzeRuleCoverage,
  appendSubjectToRule,
  isShadowedTarget,
  matchAppendTargets,
  ruleAppendTargets,
  sortAppendTargets,
  type RuleAppendTarget,
} from '@/components/dialogs/rule-append';
import { isReverseRegionRouting } from '@/domain/region-routing';
import { countryCodeToFlagAsset } from '@/domain/flag-assets';
import { localizeRegion, resolveExitNodeFlagCode, resolveExitRegion } from '@/domain/exit-flag';
import { deriveTsExitWarning } from '@/domain/tailscale-exit-warning';
import { tsExitAction } from '../ts-exit-action';
import { useTailscaleLoginProgressStore } from '@/store/use-tailscale-login-progress-store';
import { createTopicSubscription } from '@/lib/topic-subscription';
import { withProxyStartClaim } from '@/lib/proxy-start-claim';
import { editRoute, stagedOnlyIds } from '@/lib/staged-config';
import { deriveTakeoverConnState } from '@/components/screens/home/connection-state';
import { useConfig } from '@/components/screens/settings/use-config';
import type { ServiceId } from '@/contracts/unlock-detection';
import { UNLOCK_SERVICES, unlockBadgeSrc } from './unlock-services';
import { createRunWrite, writeErrorsFromProxyStatus } from './write-errors';
import { EMPTY_TRAFFIC_BUFFER, pushTotals, type TrafficBuffer } from './traffic-buffer';
import {
  applyDetail,
  compositionOf,
  EMPTY_ACTIVE_TABLE,
  filterByHostQuery,
  hostTopOf,
  ruleHitsOf,
  totalBytesOf,
  type ActiveTable,
} from './aggregate';
import { openMobileForm } from '../forms/form-store';
import { protocolLabel } from '@/components/screens/nodes/nodes-logic';
import { HomeScreenView, RULE_ACTION_KEY } from './HomeScreenView';
import { useHomeSpeedTest } from './use-home-speed-test';
import { mobileSwitchReceiptFeedback } from '../mobile-switch-feedback';
import { toast } from '@/lib/error-handler';
import { navigateMobile } from '../navigate';
import type {
  AppendTargetVM,
  CoreLifecycle,
  NodeIdentity,
  NodePickRow,
  RuleSubjectVM,
  RuleSubjectView,
  UnlockEntryVM,
  WriteErrors,
} from './view-model';

/** 协议显示名：与节点卡同一份（`nodes-logic#protocolLabel`），不在本屏另抄一张会漂的表。 */
const protocolLabelOf = (p: string | undefined): string => (p === undefined ? '' : protocolLabel(p));

/**
 * 移动端的接管方式：**恒为 TUN，不是一个可选项，也不读 `config.proxyModeType`**。
 *
 * 三档接管里另外两档（`systemProxy` / `manual`）全靠 `mixed-in` 本地代理入站，而
 * `crates/config-engine/src/builder/inbounds.rs` 在 `platform == "android"` 时不发它 ⇒
 * 那两档在 Android 上产出的配置**一个用户流量入站都没有**。配置里那个字段的默认值恰恰是
 * `systemProxy`（`user_config/app_config.rs`），所以**照读它反而是错的**：全新安装会拿到
 * 一个盘上写着 systemProxy、实际跑着 TUN 的配置，连接态判定会走进「系统代理未生效」那条
 * 移动端根本不存在的分支。故这里把平台事实写成常量，让读点自己说清楚它读的是平台不是配置。
 *
 * 配置生成侧（Android 无条件发 TUN 入站、不看 `proxy_mode_type`）由另一条线归口，本文件不碰。
 *
 * ⚠️ 连带结论，登记在此：`deriveTakeoverConnState` 的 `proxy-degraded`（「系统代理未生效」）
 * 这一态在移动端**结构上不可达** —— 那条分支的前提就是 `mode === 'systemProxy'`。呈现层仍照
 * 三态渲染，是因为 `TakeoverConnState` 是与桌面共用的契约类型：把它在这一屏收窄，等于让一个
 * 纯组件渲染不出自己入参类型允许的取值。它不是一颗控件，不会诱导用户去做任何事，只是永不出现。
 */
const MOBILE_TAKEOVER: ProxyModeType = 'tun';

/** 「上次检测」的相对时间。分钟级即可 —— 这张卡的时效性预期本就在十分钟量级。 */
function checkedLabel(t: (k: string, o?: Record<string, unknown>) => string, at: number | null): string | null {
  if (at === null) return null;
  const mins = Math.floor((Date.now() - at) / 60_000);
  if (mins < 1) return t('mobileHome.checkedJustNow');
  if (mins < 60) return t('mobileHome.checkedMinutesAgo', { n: mins });
  return t('mobileHome.checkedHoursAgo', { n: Math.floor(mins / 60) });
}

export function MobileHomeScreen(): ReactElement {
  const { t, i18n } = useTranslation();
  const [writeErrors, setWriteErrors] = useState<WriteErrors>({});
  /** 连接/断开在飞（后端只投影 `starting`，「停止中」没有对应字段，由这里补）。 */
  const [connectBusy, setConnectBusy] = useState<'starting' | 'stopping' | null>(null);
  const [manualCheckBusy, setManualCheckBusy] = useState(false);
  const checkRun = useRef({ generation: 0, busy: false, alive: true, requestId: null as string | null });
  const normalMainRequired = manualSpeedTestCaps(false).normalMainRequired === true;
  const runWrite = useMemo(() => createRunWrite(setWriteErrors, t), [t]);

  /* ── store ───────────────────────────────────────────────────────────────── */
  const servers = useEffectiveServers();
  /* **磁盘侧**镜像，只用来算 staged-only 差集（`stagedOnlyIds` 的第二个入参），与节点屏同一条腿。 */
  const diskServers = useAppStore((s) => s.servers);
  const selectedServerId = useAppStore((s) => s.selectedServerId);
  const proxyStatus = useAppStore((s) => s.proxyStatus);
  const ipInfo = useAppStore((s) => s.ipInfo);
  const unlock = useAppStore((s) => s.unlock);
  const privacyMode = useAppStore((s) => s.privacyMode);
  const startProxy = useAppStore((s) => s.startProxy);
  const stopProxy = useAppStore((s) => s.stopProxy);
  const switchServer = useAppStore((s) => s.switchServer);
  const beginUnlockCheck = useAppStore((s) => s.beginUnlockCheck);
  const applyUnlockSnapshot = useAppStore((s) => s.applyUnlockSnapshot);
  const setIpInfo = useAppStore((s) => s.setIpInfo);
  const latencyMap = useLatencyStore((s) => s.latencyMap);
  const testedAt = useLatencyStore((s) => s.testedAt);
  const config = useEffectiveConfig();
  const { update } = useConfig();
  /* 规则写腿的三个依赖，与桌面 `RuleSubjectMenuItems` / `ConnectionTopology` 逐字同一批：
     有效规则表（含暂存重放）、暂存总开关、入条目。两端对「这次写该不该进暂存」必须同一个答案。 */
  const rules = useEffectiveRules();
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const loadConfig = useAppStore((s) => s.loadConfig);

  const proxyRunning = proxyStatus?.running === true;
  const routing = (config?.proxyMode ?? 'smart') as ProxyMode;
  const latencyCheck = useHomeSpeedTest({
    servers,
    diskServers,
    selectedId: selectedServerId,
    running: proxyRunning,
    routing,
  });
  useEffect(() => {
    checkRun.current.alive = true;
    checkRun.current.generation += 1;
    // A normal-main preparation may change running/startTime itself. Only independent
    // checks use that old connection fence; iOS consumes the backend ready context below.
    return () => {
      checkRun.current.generation += 1;
      checkRun.current.alive = false;
      const requestId = checkRun.current.requestId;
      if (requestId) void runWrite('network-check', async () => {
        try {
          await unlockApi.cancelManualCheck(requestId);
        } catch {
          const message = i18n.t('prerequisite.cancelFailed');
          toast.error(message);
          throw new Error(message);
        }
      });
    };
  }, [normalMainRequired, normalMainRequired ? null : proxyRunning,
    normalMainRequired ? null : proxyStatus?.startTime, selectedServerId, routing]);

  /* ── 事件流：stats + detail ─────────────────────────────────────────────── */
  const [stats, setStats] = useState<TrafficStats | null>(null);
  const [buffer, setBuffer] = useState<TrafficBuffer>(EMPTY_TRAFFIC_BUFFER);
  const [active, setActive] = useState<ActiveTable>(EMPTY_ACTIVE_TABLE);
  /* `pushTotals` 要读上一拍的缓冲，而 stats 帧到得比 React 提交快 —— 用 ref 串起来，
     避免闭包捕获到一个已经过期的 buffer（那会让每一帧都基于同一个基线差分）。 */
  const bufRef = useRef(EMPTY_TRAFFIC_BUFFER);

  useEffect(() => {
    const sub = createTopicSubscription<TrafficStats>(
      {
        onFrame: (cb) => api.stats.onStatsUpdated(cb),
        subscribe: () => api.stats.subscribe('stats'),
        unsubscribe: () => api.stats.unsubscribe('stats'),
      },
      (s) => {
        setStats(s);
        bufRef.current = pushTotals(bufRef.current, s.totalUpload, s.totalDownload, Date.now());
        setBuffer(bufRef.current);
      },
    );
    sub.setWanted(true);
    return () => sub.dispose();
  }, []);

  useEffect(() => {
    const sub = createTopicSubscription<ConnectionsDetailUpdate>(
      {
        onFrame: (cb) => api.stats.onConnectionsDetail(cb),
        subscribe: () => api.stats.subscribe('detail'),
        unsubscribe: () => api.stats.unsubscribe('detail'),
      },
      (u) => setActive((prev) => applyDetail(prev, u)),
    );
    sub.setWanted(true);
    return () => sub.dispose();
  }, []);

  /* 核停了就把两张表清掉：留着旧帧会让「核已停」这一屏还在画上一秒的流量。 */
  useEffect(() => {
    if (proxyRunning) return;
    bufRef.current = EMPTY_TRAFFIC_BUFFER;
    setBuffer(EMPTY_TRAFFIC_BUFFER);
    setActive(EMPTY_ACTIVE_TABLE);
  }, [proxyRunning]);

  /* ── 派生 ────────────────────────────────────────────────────────────────── */
  const currentServer = useMemo(
    () => servers.find((s) => s.id === selectedServerId),
    [servers, selectedServerId],
  );

  /* 复用桌面那条腿而不是就地写 `running ? …`：连接三态的定义只该有一份。TUN 下它的口径是
     「核在跑 = 流量在经核」，故系统代理活态（`useSystemProxyLive`，驱动只挂在桌面 `App.tsx`）
     在这条分支上读不读都一样 —— 本屏因此不再订它。 */
  const connState = deriveTakeoverConnState({
    running: proxyRunning,
    proxyModeType: MOBILE_TAKEOVER,
    errorCode: proxyStatus?.errorCode,
  });

  /**
   * 核心生命周期五态。
   *
   * `proxyStatus.starting` 是**后端投影的「有起核腿在飞」**，而 `running:false` 期间也可能正在起核
   * （起核重试预算内一轮可达数十秒）。不读它，连接按钮在整个起核窗口里都是可点的 ⇒
   * 用户再点一下就**在已有起核腿之上再叠一次 start**（那句话是 `ProxyStatus.starting` 的字段注释
   * 逐字写的，托盘浮层为此专门读它）。
   *
   * 「停止中」后端没有对应字段，故由本屏自己的在飞标记补上 —— 断开同样不是瞬时的。
   * `errorCode` 排在 `running` 之后：成功起核会清空它，跑着的时候那个码是上一轮的陈旧记录。
   */
  const core: CoreLifecycle =
    proxyStatus?.starting === true || connectBusy === 'starting'
      ? 'starting'
      : connectBusy === 'stopping'
        ? 'stopping'
        : proxyRunning
          ? 'running'
          : proxyStatus?.errorCode !== undefined
            ? 'error'
            : 'stopped';

  /** 哨兵选择**不是节点** —— 必须在碰 `currentServer` 之前分支，否则卡片会渲染出一个空名字。 */
  const node: NodeIdentity = useMemo(() => {
    if (isDirectSelection(selectedServerId)) return { kind: 'direct' };
    if (isBlockSelection(selectedServerId)) return { kind: 'block' };
    if (currentServer === undefined) return { kind: 'none' };
    const code = resolveExitNodeFlagCode(proxyRunning, ipInfo?.proxy?.countryCode);
    return {
      kind: 'node',
      name: currentServer.name,
      protocolLabel: protocolLabelOf(currentServer.protocol),
      flagSrc: code === null ? null : (countryCodeToFlagAsset(code)?.src ?? null),
      latencyMs: latencyMap[currentServer.id],
      latencyStale: isLatencyStale(testedAt[currentServer.id]),
    };
  }, [selectedServerId, currentServer, latencyMap, testedAt, proxyRunning, ipInfo]);

  const tsId = currentServer?.protocol?.toLowerCase() === 'tailscale' ? currentServer.id : undefined;
  const tsLoggedIn = useAppStore((s) => (tsId ? !!s.tailscaleLoginStates[tsId] : false));
  const tsStatus = useAppStore((s) => (tsId ? s.tailscaleStatuses[tsId] : undefined));
  const tsAuthUrl = useAppStore((s) => (tsId ? s.tailscaleAuthUrls[tsId] : undefined));
  const tsLoginAttempt = useTailscaleLoginProgressStore((s) => (tsId ? s.attempts[tsId] : undefined));
  const tsExitWarning = deriveTsExitWarning({
    selectedServer: currentServer,
    loggedIn: tsLoggedIn,
    proxyModeDirect: routing === 'direct',
    proxyRunning,
    status: tsStatus,
  });

  const entries = useMemo(() => [...active.byId.values()], [active]);
  const composition = useMemo(() => compositionOf(entries), [entries]);
  const ruleHits = useMemo(() => ruleHitsOf(entries), [entries]);
  /* 主机 Top 卡的筛选词。声明在这里而不是下面那块「屏级 UI 状态」里：它是紧跟着的那条 `useMemo`
     的依赖，`const` 的 TDZ 让「先用后声明」直接是一个运行期错误，不是风格问题。 */
  const [hostQuery, setHostQuery] = useState('');
  /* 主机 Top：**先按筛选词过滤整个窗口，再排名取前 5**（判据 `filterByHostQuery`，与后端
     `project_connections_topology_iter` 逐字同源）。顺序反过来就只能在已经看得见的 5 行里筛，
     那不解决「在大集合里定位」这个问题 —— 桌面那条注释说的是同一件事。
     筛选词不参与另外两张卡：`traffic-composition` / `rule-hits` 与本卡共用同一个窗口这条口径
     （`data-contract.json#globalRules` 第 2 条）说的是**数据窗口**，而筛选是本卡自己的一次取景；
     把它渗进另外两张卡会让卡头上的合计与用户没动过的那两张卡对不上。 */
  const hosts = useMemo(
    () => hostTopOf(filterByHostQuery(entries, hostQuery)),
    [entries, hostQuery],
  );
  const windowBytes = useMemo(() => totalBytesOf(entries), [entries]);

  /**
   * 出口地区。`text` 分支产出的 `region` **未本地化**（`resolveExitRegion` 刻意保持 i18n 无关，
   * 以便 node 直测），调用方必须再过一道 `localizeRegion` —— 少这一道，境外直连出口会把一个裸
   * `US` 直接呈现给用户，而不是「美国」。
   */
  const directRoute = routing === 'direct' || isDirectSelection(selectedServerId);
  const exitRegionRaw = resolveExitRegion(proxyRunning && !directRoute, ipInfo?.proxy, ipInfo?.direct);
  const exitRegion =
    exitRegionRaw.kind === 'text'
      ? {
          kind: 'text' as const,
          region: localizeRegion(exitRegionRaw.region, i18n.language) ?? exitRegionRaw.region,
        }
      : exitRegionRaw;

  /*
   * ⚠️ `checking` 必须从 `unlock.running` 派生，不能指望 `results` 里出现它：
   * `beginUnlockCheck`（`store/app-store.ts`）在开测那一刻把 `results` **清空**，
   * 于是检测在飞的整段时间里每个服务都会回落到 `idle` —— 七颗徽章一起退回灰度，
   * 规范里那一档 animated 的 `checking`（`unlock-detection.md:69`）在移动端根本渲染不出来。
   * 桌面同一张卡走的就是这条派生（`components/screens/home/HomeScreen.tsx`）。
   */
  const unlockVms: readonly UnlockEntryVM[] = useMemo(
    () =>
      UNLOCK_SERVICES.map((s) => ({
        id: s.id,
        name: s.name,
        badgeSrc: unlockBadgeSrc(s.id),
        result: unlock.running
          ? ({ status: 'checking' } as const)
          : (unlock.results[s.id] ?? { status: 'idle' }),
      })),
    [unlock.results, unlock.running],
  );
  // A route switch can precede the next unlock frame. Do not label the old proxy's
  // service results as observations of the direct exit.
  const directUnlockCurrent = !!ipInfo?.direct?.ip && unlock.egress?.ip === ipInfo.direct.ip;
  const visibleUnlockVms = proxyRunning && directRoute && !directUnlockCurrent && !unlock.running
    ? unlockVms.map((service) => ({ ...service, result: { status: 'idle' as const } }))
    : unlockVms;

  /* ── 屏级 UI 状态 ────────────────────────────────────────────────────────── */
  /* 只在暂存里、盘上还没有的节点。选它做出口会连带一次整核重启（差集要落盘才生效），
     故必须**标出来**而不是混在里面 —— 与节点屏逐字同一条口径。 */
  const stagedOnly = useMemo(() => stagedOnlyIds(servers, diskServers), [servers, diskServers]);

  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerQuery, setPickerQuery] = useState('');
  /* 「给这个主机加一条规则」面板挂着的观测对象；`null` = 面板没开。存对象而不是存 host 字符串 ——
     判定（是 IP 还是域名）在开面板那一刻做一次就够，存字符串会让每个消费点各判一遍。 */
  const [ruleSubject, setRuleSubject] = useState<RuleSubject | null>(null);
  const [ruleSubjectView, setRuleSubjectView] = useState<RuleSubjectView>('menu');
  /* 「新建规则」视图的两格草稿。备注留空 = 用默认那句（`ruleRemarksHint`），
     故这里不预填 —— 预填之后「用户改没改过」就再也分不出来了，换动作时要么覆盖用户输入，
     要么留着一句与所选动作对不上的备注。 */
  const [newRuleAction, setNewRuleAction] = useState<RuleAction>('proxy');
  const [newRuleRemarks, setNewRuleRemarks] = useState('');
  const [appendQuery, setAppendQuery] = useState('');
  /* 网络检测卡里点开详情的那个服务（`unlock-detection.md`「Interaction」）。存 id 不存快照 ——
     结果对象会在下一轮检测里被整体替换，存快照的详情会停在旧结论上。这里**不写任何东西**，
     故不进 `WRITE_CONTROLS`：它是一次纯读的展开。 */
  const [unlockDetailId, setUnlockDetailId] = useState<ServiceId | null>(null);

  const pickRows: readonly NodePickRow[] = useMemo(() => {
    return servers
      .map((s) => ({
        server: s,
        isCurrent: s.id === selectedServerId,
        stagedOnly: stagedOnly.has(s.id),
        latencyMs: latencyMap[s.id],
        latencyStale: isLatencyStale(testedAt[s.id]),
        protocolLabel: protocolLabelOf(s.protocol),
      }));
  }, [servers, latencyMap, testedAt, selectedServerId, stagedOnly]);

  /**
   * 「阻断」在**直连模式**下不可选，并把原因写在旁边。判据与桌面 `HomeScreen.tsx` 的
   * `blockDisabledReason` 逐字同源：`route.final` 恒 = `direct` 时没有一条流量经过 proxy-selector，
   * 选中阻断是一次静默 no-op —— 用户会以为自己断网了。
   */
  const blockDisabledReason = routing === 'direct' ? t('home.blockExitUnavailableInDirect') : null;

  /**
   * 空态：一个节点都没有、**且**没选哨兵出口。后半句不能省（桌面同一条判据）——
   * 直连 / 阻断都是有效的出口配置，选了它的人不缺节点。
   */
  const noServers = servers.length === 0 && !isSentinelSelection(selectedServerId);

  /* ── ⑦ 主机 Top 行上的规则面板：判据全部复用桌面那批纯函数 ─────────────── */

  /** 客户端启发式覆盖提示（`analyzeRuleCoverage`）。只影响提示与排序，不禁用任何动作。 */
  const coverage = useMemo(
    () => (ruleSubject === null ? null : analyzeRuleCoverage(rules, ruleSubject)),
    [rules, ruleSubject],
  );

  const ruleSubjectVM: RuleSubjectVM | null = useMemo(() => {
    if (ruleSubject === null || coverage === null) return null;
    const covering =
      coverage.firstId === null ? null : (rules.find((r) => r.id === coverage.firstId) ?? null);
    return {
      subject: ruleSubject,
      coveringName:
        covering === null ? null : covering.remarks?.trim() || t(ruleTypeNameKey(covering.type)),
    };
  }, [ruleSubject, coverage, rules, t]);

  /**
   * 「合并进已有规则」的候选。三个纯函数的顺序**不能换**（与桌面 `RulePickDialog` 逐字相同）：
   * 先按规则序展开全部目标，再按「可追加 → 已包含 → 置灰」重排，最后才按搜索词过滤 ——
   * 反过来（先过滤再排序）会让排名在每次击键之间跳动。
   */
  const appendTargets: readonly AppendTargetVM[] = useMemo(() => {
    if (ruleSubject === null || coverage === null) return [];
    return matchAppendTargets(
      sortAppendTargets(ruleAppendTargets(rules, ruleSubject)),
      appendQuery,
    ).map((target) => ({ target, shadowed: isShadowedTarget(coverage, target) }));
  }, [rules, ruleSubject, coverage, appendQuery]);

  /**
   * 默认备注。`remarks` 在这条腿上是**必填**而不是可选：规则列表的标题在无备注时回落成裸类型名
   * （`RuleItem.tsx::ruleTitle`），同类型的快速规则会完全无法区分，而顺序直接决定命中优先级。
   * 这一条逐字取自桌面 `ConnectionTopology#addSubjectRule` 的注释。
   */
  const ruleRemarksHint =
    ruleSubject === null
      ? ''
      : t('home.ruleRemarks', {
          action: t(RULE_ACTION_KEY[newRuleAction]),
          value: ruleSubject.value,
        });

  /* ── 写腿。每一处写调用都在 `runWrite(` 里，门逐个对拍 ─────────────────── */

  /**
   * 连接开关。
   *
   * `withProxyStartClaim` 认领本次起核：提权门那两码（`HELPER_NOT_INSTALLED` /
   * `HELPER_GATE_ABORTED`）后端是**双出口** —— 既 emit `event:proxyError`，又让本次 start reject。
   * 本屏的 `runWrite` 就是发起方的 await 腿；不认领则事件腿会把同一次失败**再报一遍**。
   * 认领期内事件腿让位，而托盘 / 开机自动连接这些无人 await 的入口不经此处、未认领，照常由事件腿报。
   * **删掉这层包裹即回归双报**（`lib/proxy-start-claim.test.ts` 的消费面守卫钉着它）。
   */
  const onToggleConnect = useCallback(() => {
    const stopping = proxyRunning;
    setConnectBusy(stopping ? 'stopping' : 'starting');
    void runWrite('connect', () =>
      stopping ? stopProxy() : withProxyStartClaim(() => startProxy()),
    ).finally(() => setConnectBusy(null));
  }, [runWrite, proxyRunning, startProxy, stopProxy]);

  /**
   * 网络检测。**必须消费返回值**：后端有多条早退路径把终态直接塞在返回值里、不经事件，
   * 只监听事件会在那些路径上永远停在「检测中」（这条逐字取自桌面 `HomeScreen.tsx:774`）。
   */
  const runNetworkCheck = useCallback(() => {
    if (checkRun.current.busy) return;
    checkRun.current.busy = true;
    const generation = ++checkRun.current.generation;
    const startedRunning = useAppStore.getState().proxyStatus?.running === true;
    const startedAt = useAppStore.getState().proxyStatus?.startTime;
    const startedServerId = useAppStore.getState().selectedServerId;
    const startedRouting = getEffectiveConfig()?.proxyMode ?? 'smart';
    const startedConfig = JSON.stringify(getEffectiveConfig());
    const requestId = normalMainRequired ? crypto.randomUUID() : null;
    checkRun.current.requestId = requestId;
    setManualCheckBusy(true);
    void runWrite('network-check', async () => {
      if (requestId) {
        const result = await unlockApi.manualCheck(requestId, true);
        const previous = useAppStore.getState().proxyStatus;
        const current = await api.proxy.getStatus();
        const state = useAppStore.getState();
        if (checkRun.current.generation !== generation || !checkRun.current.alive
          || state.selectedServerId !== startedServerId
          || JSON.stringify(getEffectiveConfig()) !== startedConfig
          || !manualNetworkContextMatches(result.context, requestId, current)
          || (state.proxyStatus !== previous && !manualNetworkContextMatches(result.context, requestId, state.proxyStatus))) return;
        state.setProxyStatus(current);
        if (result.ipInfo.data) setIpInfo(result.ipInfo.data);
        if (result.unlock.data) applyUnlockSnapshot(result.unlock.data);
        state.setUnlock({ running: false });
        // The same explicit network action also refreshes the current RTT, after ready.
        await latencyCheck.runCurrent();
        const failures = [result.ipInfo.error && t('prerequisite.ipInfoFailed'),
          result.unlock.error && t('prerequisite.unlockFailed')].filter(Boolean);
        if (failures.length) throw new Error(failures.join(' '));
        return;
      }
      beginUnlockCheck();
      const [ip, services] = await Promise.allSettled([
        api.ipInfo.get(true, true),
        unlockApi.run(true),
      ] as const);
      const currentState = useAppStore.getState();
      const current = currentState.proxyStatus;
      if (checkRun.current.generation !== generation ||
          (current?.running === true) !== startedRunning || current?.startTime !== startedAt ||
          currentState.selectedServerId !== startedServerId ||
          (getEffectiveConfig()?.proxyMode ?? 'smart') !== startedRouting) return;
      if (ip.status === 'fulfilled') setIpInfo(ip.value);
      if (services.status === 'fulfilled') applyUnlockSnapshot(services.value);
      useAppStore.getState().setUnlock({ running: false });
      if (ip.status === 'rejected') throw ip.reason;
      if (services.status === 'rejected') throw services.reason;
    }).finally(() => {
      checkRun.current.busy = false;
      checkRun.current.requestId = null;
      if (requestId && checkRun.current.generation === generation) useAppStore.getState().setUnlock({ running: false });
      if (checkRun.current.alive) setManualCheckBusy(false);
    });
  }, [runWrite, beginUnlockCheck, applyUnlockSnapshot, setIpInfo, normalMainRequired, latencyCheck, t]);

  const onUseAsExit = useCallback(
    (server: ServerConfig) => {
      // Only a known manual selection may skip the authoritative switch/readback.
      const config = useAppStore.getState().config;
      if (config && config.selectionIntent === undefined && server.id === config.selectedServerId) {
        setPickerOpen(false);
        return;
      }
      void runWrite('switch-node', async () => {
        const receipt = await switchServer(server.id);
        const feedback = mobileSwitchReceiptFeedback(receipt, server.name, t);
        if (feedback?.tone === 'success') toast.success(feedback.text);
        else if (feedback?.tone === 'warning') toast.warning(feedback.text);
        else if (feedback) toast.info(feedback.text);
      }).then(() => {
        /* 失败也关：选择器是一层覆盖，错误留在它下面等于看不见。关掉之后行内错误就贴在
           动作行的「切换节点」那颗控件下面 —— 那是用户刚刚动过的地方，且是可见的。 */
        setPickerOpen(false);
      });
    },
    [runWrite, switchServer, t],
  );

  /** Sentinel exits use the same authoritative switch receipt as real nodes. */
  const onPickSentinel = useCallback(
    (kind: 'direct' | 'block') => {
      if (kind === 'block' && blockDisabledReason !== null) return;
      const id = kind === 'direct' ? DIRECT_SERVER_ID : BLOCK_SERVER_ID;
      const config = useAppStore.getState().config;
      if (config && config.selectionIntent === undefined && id === config.selectedServerId) {
        setPickerOpen(false);
        return;
      }
      void runWrite('switch-node', async () => {
        const receipt = await switchServer(id);
        const label = t(kind === 'direct' ? 'home.routingDirect' : 'home.routingBlock');
        const feedback = mobileSwitchReceiptFeedback(receipt, label, t);
        if (feedback?.tone === 'success') toast.success(feedback.text);
        else if (feedback?.tone === 'warning') toast.warning(feedback.text);
        else if (feedback) toast.info(feedback.text);
      }).then(() => {
        /* 失败也关，理由同 `onUseAsExit`：选择器是一层覆盖，错误留在它下面等于看不见。 */
        setPickerOpen(false);
      });
    },
    [blockDisabledReason, runWrite, switchServer, t],
  );

  const onSetRouting = useCallback(
    (v: ProxyMode) => {
      void runWrite('routing', () => update({ proxyMode: v }));
    },
    [runWrite, update],
  );


  /** Match the action to the warning: missing auth URL must not open exit setup. */
  const onTsExitAction = useCallback(() => {
    if (tsId === undefined) return;
    const action = tsExitAction(tsExitWarning, {
      liveUrl: tsStatus?.authURL, storeUrl: tsAuthUrl, attempt: tsLoginAttempt, normalMainRequired,
    });
    if (action.kind === 'login-url') {
      void runWrite('switch-node', () => api.system.openExternal(action.url));
    } else {
      openMobileForm({ kind: action.kind === 'login-panel' ? 'ts-login' : 'ts-exit', serverId: tsId });
    }
  }, [tsExitWarning, tsStatus?.authURL, tsAuthUrl, tsLoginAttempt, tsId, runWrite, openMobileForm, normalMainRequired]);

  /* ── ⑦ 规则写腿。三个入口（快速两颗 / 新建 / 合并）共用一个控件 id 与一条暂存闸门 ──────── */

  /**
   * 打开「给这个主机加一条规则」面板。
   *
   * 判不出观测对象（`hostNameOf` 的第三级回落是 `rule` 名，那是一条规则的名字不是一个目的地）
   * ⇒ **不开面板**。呈现层对这类行也不画按钮，故这里只是二次守门。
   */
  const onOpenRuleSubject = useCallback((host: string) => {
    const subject = ruleSubjectForValue(host);
    if (subject === null) return;
    setRuleSubject(subject);
    setRuleSubjectView('menu');
    setNewRuleAction('proxy');
    setNewRuleRemarks('');
    setAppendQuery('');
  }, []);

  /**
   * 新建一条只含该对象的规则 —— 快速两颗与「新建规则」视图共用这一条腿。
   *
   * 暂存闸门与桌面 `ConnectionTopology#addSubjectRule` 逐字同形：`trafficRules` 是 Class B、
   * 无副作用 ⇒ 默认腿（进暂存）。新增时**前端自铸 id**：条目现在就要一个稳定的实体寻址键，
   * 而后端只在落盘那一刻才发 id。
   */
  const addSubjectRule = useCallback(
    (action: RuleAction, remarks: string) => {
      const subject = ruleSubject;
      if (subject === null) return;
      void runWrite('rule-add', async () => {
        const rule = {
          type: subject.type,
          values: [subject.value],
          action,
          enabled: true,
          remarks,
        };
        if (editRoute('trafficRules', stagingEnabled) === 'staged') {
          const entityId = crypto.randomUUID();
          stage({
            id: `rule:${entityId}`,
            kind: 'rule',
            label: `${t('rules.newTitle')} ${subject.value}`,
            entityPath: ['trafficRules', entityId],
            nextValue: { ...rule, id: entityId },
          });
        } else {
          await api.rules.add(rule, 'route');
          void loadConfig(true);
        }
      }).then(() => {
        /* 成功失败都关：面板是一层覆盖，`rule-add` 的行内错误贴在它下面那张卡上。 */
        setRuleSubject(null);
      });
    },
    [ruleSubject, runWrite, stagingEnabled, stage, t, loadConfig],
  );

  const onQuickRule = useCallback(
    (action: 'proxy' | 'direct') => {
      const subject = ruleSubject;
      if (subject === null) return;
      addSubjectRule(
        action,
        t('home.ruleRemarks', { action: t(RULE_ACTION_KEY[action]), value: subject.value }),
      );
    },
    [ruleSubject, addSubjectRule, t],
  );

  const onCreateRule = useCallback(() => {
    addSubjectRule(newRuleAction, newRuleRemarks.trim() || ruleRemarksHint);
  }, [addSubjectRule, newRuleAction, newRuleRemarks, ruleRemarksHint]);

  /**
   * 把该对象追加进一条已有规则。判据与变换全部是 `rule-append.ts` 的纯函数
   * （桌面 `RuleSubjectMenuItems#append` 是同一条腿，那里是全仓唯一的 `api.rules.update` 追加点）。
   *
   * `appendSubjectToRule` 返回 `null` = 目标漂移（规则在面板开着的这段时间被别处改过）⇒ 如实报失败，
   * **不静默当成功**。「已包含」那一档在呈现层就是禁用的，点不到这里。
   */
  const onAppendToRule = useCallback(
    (target: RuleAppendTarget) => {
      const subject = ruleSubject;
      if (subject === null) return;
      void runWrite('rule-add', async () => {
        const base = rules.find((rule) => rule.id === target.ruleId) ?? null;
        const next = base === null ? null : appendSubjectToRule(base, target, subject);
        if (next === null) throw new Error(t('rules.appendFail'));
        const label = next.remarks?.trim() || t(ruleTypeNameKey(next.type));
        if (editRoute('trafficRules', stagingEnabled) === 'staged') {
          stage({
            id: `rule:${next.id}`,
            kind: 'rule',
            label: `${t('rules.editTitle')} ${label}`,
            entityPath: ['trafficRules', next.id],
            nextValue: next,
          });
        } else {
          await api.rules.update(next, 'route');
          void loadConfig(true);
        }
      }).then(() => {
        setRuleSubject(null);
      });
    },
    [ruleSubject, runWrite, rules, t, stagingEnabled, stage, loadConfig],
  );

  return (
    <HomeScreenView
      t={t}
      writeErrors={writeErrorsFromProxyStatus(writeErrors, proxyStatus, t)}
      core={core}
      connState={connState}
      node={node}
      onToggleConnect={onToggleConnect}
      onNetworkCheck={() => {
        runNetworkCheck();
        if (!normalMainRequired) void latencyCheck.runCurrent();
      }}
      latencyCheck={latencyCheck.view}
      tsExitWarning={tsExitWarning}
      onTsExitAction={onTsExitAction}
      pickerOpen={pickerOpen}
      onOpenPicker={() => { setPickerQuery(''); setPickerOpen(true); }}
      onClosePicker={() => setPickerOpen(false)}
      pickerQuery={pickerQuery}
      onPickerQuery={setPickerQuery}
      pickRows={pickRows}
      pickerSubscriptions={config?.subscriptions ?? []}
      onUseAsExit={onUseAsExit}
      onPickSentinel={onPickSentinel}
      blockDisabledReason={blockDisabledReason}
      noServers={noServers}
      /* 两条直达入口落到移动端自己的表单宿主（批 2 接的那一层），不是桌面弹窗。 */
      onAddServer={() => openMobileForm({ kind: 'node' })}
      onAddSubscription={() => openMobileForm({ kind: 'sub' })}
      routing={routing}
      /* 真机 2026-07-20 §1.4 的移植：反向（回国）分流在首页必须是**常驻可见**的。
         判据复用 `domain/region-routing.ts`，不在本屏就地写 `config?.regionRouting?.reverse`
         —— 那会漏掉 `enabled` 这个前置条件（关掉地区分流后 `reverse` 是死数据，提示即噪音）。 */
      reverseRouting={isReverseRegionRouting(config)}
      exitRegion={exitRegion}
      exitFlagSrc={
        exitRegion.kind === 'flag' ? (countryCodeToFlagAsset(exitRegion.code)?.src ?? null) : null
      }
      exitIsDirect={!proxyRunning || directRoute}
      exitProbing={ipInfo?.loading === true || manualCheckBusy}
      exitIp={privacyMode ? '••••' : (proxyRunning && !directRoute ? ipInfo?.proxy?.ip : ipInfo?.direct?.ip) ?? null}
      onSetRouting={onSetRouting}
      unlock={visibleUnlockVms}
      unlockCheckedLabel={checkedLabel(t, unlock.checkedAt)}
      unlockRunning={unlock.running}
      unlockDetailId={unlockDetailId}
      onOpenUnlockDetail={setUnlockDetailId}
      onCloseUnlockDetail={() => setUnlockDetailId(null)}
      samples={buffer.samples}
      composition={composition}
      ruleHits={ruleHits}
      onOpenRuleHit={(ruleGroup) => { void navigateMobile('connections', undefined, { ruleGroup }); }}
      hosts={hosts}
      hostsMasked={privacyMode}
      hostQuery={hostQuery}
      onHostQuery={setHostQuery}
      ruleSubject={ruleSubjectVM}
      ruleSubjectView={ruleSubjectView}
      onOpenRuleSubject={onOpenRuleSubject}
      onCloseRuleSubject={() => setRuleSubject(null)}
      onRuleSubjectView={setRuleSubjectView}
      onQuickRule={onQuickRule}
      newRuleAction={newRuleAction}
      onNewRuleAction={setNewRuleAction}
      newRuleRemarks={newRuleRemarks}
      onNewRuleRemarks={setNewRuleRemarks}
      ruleRemarksHint={ruleRemarksHint}
      onCreateRule={onCreateRule}
      appendQuery={appendQuery}
      onAppendQuery={setAppendQuery}
      appendTargets={appendTargets}
      onAppendToRule={onAppendToRule}
      windowBytes={windowBytes}
      windowConnections={entries.length}
      kernelConnections={stats?.activeConnections ?? null}
    />
  );
}
