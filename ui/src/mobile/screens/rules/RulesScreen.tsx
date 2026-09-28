/**
 * 移动端「规则」屏 —— 四合一入口（IA spec §1.2 / §2.2）。
 *
 * 它收拢桌面的四个屏：`rules`(流量) + `dnsrules` + `apppolicy` + `resources`。
 *
 * # 为什么是这四个，而不是「主题相近」
 *
 * 四者两两之间存在**双向的、已实现的**引用关系（收敛文档 §1.3）：
 *  · 流量与 DNS **是同一个组件文件**：`RulesScreen.tsx:69` 收 `plane` prop，桌面拆成两个侧栏项
 *    只是因为竖侧栏放得下。折成两个分段是**还原**，不是合并。
 *  · 应用分流写 `config.appRules`，而节点屏把 `customRules` 与 `appRules` **合起来**算
 *    「哪些节点被规则指名」；后端 route builder 也是并排消费。
 *  · 资源与规则是同一条引用关系的两端：规则行的「资源缺失」角标由资源集算，
 *    资源行的「被引用」徽章由规则集反算。分在两个一级 tab，等于让用户为查一条规则缺哪个资源跨 tab。
 *
 * # 容器 / 呈现分离
 *
 * 本文件**只做接线**：读 store、算派生量、发 IPC、持有屏级状态。四个分段是纯呈现组件，
 * 门可以拿夹具直接渲染它们 —— 「测方法体」与「测接线」是两件事，分开才各自测得动。
 *
 * # 复用的是逻辑，不是组件
 *
 * `domain/*` 的纯函数、`store/*` 的 hook、`lib/*` 的工具**全部原样复用**（`useRuleDelete`、
 * `duplicateRulePayload`、`editRoute`、`stagedOnlyIds`、`missingResourceRuleIds`、
 * `meshOverlapRuleIds` …）。不能复用的只有桌面**组件**：它们的外观全落在 `prototype.css` /
 * `components.css` 上，那是契约 A1 禁止进入移动端的层叠链。
 *
 * ⚠️ **登记一条真实的重复**：下面的 `commitOrder` / `handleToggle` / `handleDuplicate` /
 * `handleRegionChange` 与桌面 `RulesScreen.tsx` 的同名函数是**逐条同义的第二份实现**。
 * 正确的收口是把它们抽进 `lib/`，两端共用 —— 但那是一次桌面侧改动，而本批的硬约束是
 * 「桌面屏只读不改」。抽取登记为后续线（设计文档 §7），在那之前这里每一条都刻意与桌面逐字同形，
 * 便于对差。
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { useDismissableLayer } from '../../back-stack';
import { setPushedPage, usePushedPage } from '../../MobileShell';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import type {
  AppRule,
  RegionRoutingConfig,
  Rule,
  RuleResourceListItem,
  RuleResourceProgress,
} from '@/contracts/types';
import { effectiveRegionRouting } from '@/domain/region-routing';
import { availableResourceTagSet, missingResourceRuleIds } from '@/domain/rule-resource-refs';
import { duplicateRulePayload } from '@/domain/rule-duplicate';
import { ruleDnsEffect, ruleRouteEffect } from '@/domain/rules';
import {
  BUILTIN_NETENV_DHCP_ID,
  criteriaSummary,
  PROFILE_MATCH_KEYS,
  profileRefCounts,
  profileRowStatus,
  ruleProfileBadge,
} from '@/domain/network-profile';
import { netenvDnsDisplayName } from '@/components/dialogs/dns-action-options';
import { probeDisplayText, useResolvedProbes } from '@/components/screens/rules/network-profile-probes';
import type { MeshRouteReport } from '@/contracts/mesh-route-report';
import { asMeshRouteReport, meshRouteSummaryKey, MobileMeshRouteEvidence } from '../../MobileMeshRouteEvidence';
import { meshRouteRuleHints } from '../../mesh-route-context';
import { MobileInfo } from '../../MobileInfo';
import { isMeshNode } from '@/domain/endpoint-routes';
import { mergeAppPresets, type AppPreset } from '@/domain/app-rules-preset';
import { categoryLabel } from '@/domain/rule-resource-catalog';
import { resourceUpdateFeedback, resourceUpdateOutcome } from '@/domain/resource-update-outcome';
import { defaultOpenGroupIds, groupServersBySubscription } from '@/domain/server-grouping';
import type { CselGroup } from '@/components/dialogs/csel-logic';
import { useAppStore, useEffectiveConfig, useEffectiveRules, useEffectiveServers } from '@/store/app-store';
import { useAppPresetsStore } from '@/store/use-app-presets-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { editRoute, stagedOnlyIds } from '@/lib/staged-config';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { useRuleDelete } from '@/lib/use-rule-delete';
import { toast } from '@/lib/error-handler';
import { appPresetLabel } from '@/components/hover-cards/AppRuleHoverCard';
import { resourceRefs } from '@/components/hover-cards/ResourceRefsHoverCard';
import { APP_CATEGORY_ALL, compareLabel, sortAppCategories } from '@/components/screens/app-policy/app-policy-logic';
import { useConfig } from '@/components/screens/settings/use-config';
import { ActionSheet, AddIcon, BackIcon, MoreIcon, NetworkProfileIcon, SegmentedTabs, Switch } from './Primitives';
import { TrafficSegment, type RuleRowBinding } from './TrafficSegment';
import { DnsResourcePage, DnsSegment, type DnsResourceRow } from './DnsSegment';
import { AppsSegment, type AppRowModel, type AppView } from './AppsSegment';
import { appRuleForPick, type AppPolicyWritablePick } from './app-policy-write';
import { closeMobileForm, openMobileForm } from '../../forms/form-store';
import {
  dnsGroupReferences,
  dnsResourceReferenceText,
  dnsServerReferences,
  isProtectedDnsServer,
} from '@/components/dialogs/dns-resource-logic';
import { ResourcesSegment, type ResourceRowModel, type ResourceSource } from './ResourcesSegment';
import { NetworkProfilesPage, type NetworkProfileRowModel } from './NetworkProfilesPage';
import { mobileReasonKey } from './network-profile-copy';
import './rules-screen.css';

type Segment = 'traffic' | 'dns' | 'apps' | 'resources';
/**
 * 二级页（IA §1.2：DNS 服务器与分组从屏头溢出进入，不做第二层分段条）。
 * `network-profiles`：网络场景（spec §6.1 D13）—— 流量与 DNS 两个平面**同一个入口**、同一页。
 */
const PUSHED_PAGES = ['dns-servers', 'dns-groups', 'network-profiles'] as const;
type PushedPage = (typeof PUSHED_PAGES)[number];
const isPushedPage = (raw: string): raw is PushedPage =>
  (PUSHED_PAGES as readonly string[]).includes(raw);

/**
 * 本屏的栈 = 外壳那张表里 `rules` 那一格，与设置屏走同一条通道
 * （`MobileShell` 的 `usePushedPage` / `setPushedPage`）。
 *
 * 🔴 **此前是屏内 `useState`，那本身就是一个缺陷**：外壳读不到本屏压了栈 ⇒
 * `MobileShell.tsx` 里的 `pushed` 恒 `false` ⇒ 两个 DNS 二级页上照常画着底部导航，
 * 违反 `mobile-screen-shell.md#Variants`（pushed detail screen … **no duplicate bottom
 * navigation**）与 IA §2.4。设置屏是做对的那条腿 ⇒ 这不是刻意取舍，是漏接。
 * 顺带还修掉「在二级页上点导航，栈位连同页面一起丢」那半条。
 *
 * 收窄在读回处做：外壳那一格存的是裸 `string`（它不认识、也不该认识本屏的页 id 枚举），
 * 不在表里的值一律当「停在根页」，而不是渲染出一页空白。
 */
function useRulesStack(): {
  page: PushedPage | null;
  setPage: (next: PushedPage | null) => void;
} {
  const raw = usePushedPage('rules');
  const setPage = useCallback((next: PushedPage | null) => {
    setPushedPage('rules', next);
  }, []);
  const page = raw !== null && isPushedPage(raw) ? raw : null;
  /* 系统返回键在二级页上 = 回根页，与屏头那颗返回按钮同一个动作、同一个闭包。 */
  useDismissableLayer(page !== null, () => setPage(null));
  return { page, setPage };
}

const RULE_DEL_PREFIX = 'rule-del:';
const RES_DEL_PREFIX = 'res-del:';
/** DNS 服务器 / 分组删除的二次确认 key（与桌面 `DnsPolicyWorkspace` 同前缀）。 */
const DNS_SRV_DEL_PREFIX = 'dns-server:';
const DNS_GRP_DEL_PREFIX = 'dns-group:';
/** 自定义应用删除的二次确认 key（与桌面 `AppPolicyScreen` 同前缀）。 */
const APP_REMOVE_PREFIX = 'app-remove:';
/** 「重置内置资源」的二次确认 key（与桌面同名，全仓 confirm 站点登记表按它对账）。 */
const GEO_RESET_KEY = 'geo-reset';

/** 应用分类表（桌面 `AppPolicyScreen.tsx:86-93` 那张，逐值同）。 */
const APP_CATEGORY_KEYS = [APP_CATEGORY_ALL, 'video', 'social', 'ai', 'tools', 'game'] as const;

export function MobileRulesScreen(): ReactElement {
  const { t, i18n } = useTranslation();
  /** 薄封装：分段是纯呈现组件，不该各自持有 i18next 实例。 */
  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string,
    [t],
  );

  const [segment, setSegment] = useState<Segment>('traffic');
  const { page, setPage } = useRulesStack();
  const [sheetRuleId, setSheetRuleId] = useState<string | null>(null);
  const [sheetResId, setSheetResId] = useState<string | null>(null);
  /* 屏头的溢出面板（IA §1.2 + §3.1 #10）：DNS 段 = 服务器 / 分组 / 网络场景三个二级页的入口，
     流量段 = 网络场景。两个平面进的是同一页（spec §6.1：场景同时被两个平面引用）。 */
  const [headSheet, setHeadSheet] = useState(false);

  const config = useEffectiveConfig();
  const servers = useEffectiveServers();
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const loadConfig = useAppStore((s) => s.loadConfig);
  /* 核起停会改变引擎算出来的 force-route 段集（未运行时报告是空的），故列进下面那条拉取的依赖。 */
  const proxyRunning = useAppStore((s) => !!s.proxyStatus?.running);
  const { armed, confirmTwice } = useConfirmTwice();

  /**
   * 写失败的**行内**回显（键 → 已翻译的文案）。
   *
   * 立项时移动端没有 toast 宿主：`lib/error-handler.ts:62-71` 的默认实现只 `console.*`，真实实现
   * 由桌面 `Toaster.tsx` 注入而它只挂在 `AppShell` 上 ⇒ 这一屏每一次写失败都是「开关弹回去、
   * 什么都不说」——W10/W14 那批真机缺陷的原样复现。
   * （宿主 2026-09-06 已补上，见 `mobile/MobileToaster.tsx`；本表照旧，理由见 `Primitives.tsx`
   * 的 `InlineError` 头注：这一屏一次可以有好几处并发的写，行内说「是哪一颗」。）
   *
   * 键就是「哪个控件失败了」：`rule:<id>` / `order:<plane>` / `regionRouting` / `proxyMode` /
   * `appRoutingEnabled` / `appRule:<appId>` / `res:<id>` / `res:all` / `dnsres:<id>` / `np:<场景 id>`。
   * 落到那个控件旁边，不是屏顶。
   * **每次重试先清**，否则上一次的红字会挂在一个已经成功的控件旁边。
   */
  const [writeErrors, setWriteErrors] = useState<Record<string, string>>({});
  const reportWriteError = useCallback((key: string, text: string) => {
    setWriteErrors((prev) => ({ ...prev, [key]: text }));
  }, []);
  const clearWriteError = useCallback((key: string) => {
    setWriteErrors((prev) => {
      if (!(key in prev)) return prev;
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }, []);
  const errorOf = useCallback((key: string): string | undefined => writeErrors[key], [writeErrors]);

  const proxyMode = config?.proxyMode ?? 'smart';
  const isSmartMode = proxyMode === 'smart';
  const modeInactive = !isSmartMode;

  /* ═══════════ 规则（流量 / DNS 两个平面共用一条派生链）═══════════ */

  const routeRules = useEffectiveRules('route');
  const dnsRules = useEffectiveRules('dns');
  const diskRouteRules = useAppStore((s) => s.rules);
  const diskDnsRules = useAppStore((s) => s.dnsRules);
  const deleteRouteRule = useRuleDelete('route');
  const deleteDnsRule = useRuleDelete('dns');

  /** 乐观重排：先落本地，失败回拉。不落就会「拖完瞬间弹回原位」，表现成拖拽完全无效。 */
  const [optimisticOrder, setOptimisticOrder] = useState<Partial<Record<'route' | 'dns', string[]>>>({});

  const orderedOf = useCallback(
    (plane: 'route' | 'dns'): Rule[] => {
      const all = plane === 'dns' ? dnsRules : routeRules;
      const planeRules = all.filter((rule) =>
        plane === 'dns' ? ruleDnsEffect(rule) !== null : ruleRouteEffect(rule) !== null,
      );
      const persisted = config?.[plane === 'dns' ? 'dnsRuleOrder' : 'routeRuleOrder'] ?? [];
      const byId = new Map(planeRules.map((rule) => [rule.id, rule]));
      const out: Rule[] = [];
      for (const id of optimisticOrder[plane] ?? persisted) {
        const rule = byId.get(id);
        if (rule) {
          out.push(rule);
          byId.delete(id);
        }
      }
      out.push(...planeRules.filter((rule) => byId.has(rule.id)));
      return out;
    },
    [routeRules, dnsRules, config, optimisticOrder],
  );

  /**
   * 持久顺序或规则集变了 ⇒ 丢掉乐观顺序（桌面 `RulesScreen.tsx:87` 同一条）。
   * 不丢的话它会**永久**遮住持久顺序：暂存条目被撤销、或另一处改了顺序，列表都不会跟着回来。
   */
  const persistedRouteOrder = (config?.routeRuleOrder ?? []).join('\u0000');
  const persistedDnsOrder = (config?.dnsRuleOrder ?? []).join('\u0000');
  useEffect(
    () => setOptimisticOrder({}),
    [persistedRouteOrder, persistedDnsOrder, routeRules, dnsRules],
  );

  const routeOrdered = useMemo(() => orderedOf('route'), [orderedOf]);
  const dnsOrdered = useMemo(() => orderedOf('dns'), [orderedOf]);

  const serverNameById = useMemo(() => {
    const m = new Map<string, string>();
    servers.forEach((s) => m.set(s.id, s.name));
    return m;
  }, [servers]);

  const dnsServerNameById = useMemo(() => {
    const names = new Map<string, string>();
    for (const server of config?.dnsServers ?? []) {
      const name =
        server.id === 'builtin-domestic'
          ? tr('settings.dns.builtinDomesticName')
          : server.id === 'builtin-remote'
            ? tr('settings.dns.builtinRemoteName')
            : server.id === 'builtin-bootstrap'
              ? tr('settings.dns.builtinBootstrapName')
              : server.name;
      names.set(server.id, name);
    }
    // 内置「当前网络 DHCP 下发的 DNS」不在 dnsServers 资源表里（生成器按需产出，spec D5）——
    // 不登记名字的话，选了它的 DNS 规则行上会印出保留 id `builtin-netenv-dhcp`（同桌面 `RulesScreen`）。
    names.set(BUILTIN_NETENV_DHCP_ID, netenvDnsDisplayName(t));
    return names;
  }, [config?.dnsServers, tr, t]);

  const dnsGroupNameById = useMemo(
    () => new Map((config?.dnsServerGroups ?? []).map((group) => [group.id, group.name])),
    [config?.dnsServerGroups],
  );

  /* ═══════════ 规则资源（角标输入 + 资源分段的列表本体，一次拉取两处用）═══════════ */

  const [resItems, setResItems] = useState<RuleResourceListItem[]>([]);
  const [resLoading, setResLoading] = useState(true);
  const [resError, setResError] = useState(false);
  const [resProgress, setResProgress] = useState<Record<string, RuleResourceProgress>>({});
  const [resUpdatingAll, setResUpdatingAll] = useState(false);
  const [resUpdatingIds, setResUpdatingIds] = useState<ReadonlySet<string>>(() => new Set());

  const reloadResources = useCallback(async () => {
    try {
      const list = await api.ruleResources.list();
      // IPC 边界不可信 TS 的类型承诺（异常路径可能下发 null）：非数组一律当空集。
      setResItems(Array.isArray(list) ? list : []);
      setResError(false);
    } catch {
      setResError(true);
    } finally {
      setResLoading(false);
    }
  }, []);

  // 随 config 变化重拉：在资源分段删/恢复资源会改 config，规则行的「资源缺失」角标要即时反映。
  useEffect(() => {
    void reloadResources();
  }, [reloadResources, config]);

  useEffect(() => {
    let disposed = false;
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const off = api.ruleResources.onProgress((p: RuleResourceProgress) => {
      if (disposed) return;
      setResProgress((prev) => ({ ...prev, [p.id]: p }));
      /*
       * `cancelled` 与 `done` / `error` 同属**终态**：三者都要清进度 + 重拉列表。
       * 漏掉清理 → 取消后那行永远停在进度条上（后端已经中止，界面却还在动 = 假进行中）。
       * 与桌面 `ResourcesScreen.tsx:114-142` 同一条腿、同一个 300ms。
       */
      if (p.status === 'done' || p.status === 'error' || p.status === 'cancelled') {
        const timer = setTimeout(() => {
          timers.delete(timer);
          if (disposed) return;
          setResProgress((prev) => {
            const next = { ...prev };
            delete next[p.id];
            return next;
          });
          void reloadResources();
        }, 300);
        timers.add(timer);
      }
    });
    return () => {
      disposed = true;
      off();
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
    };
  }, [reloadResources]);

  const availableResTags = useMemo(() => availableResourceTagSet(resItems), [resItems]);

  const missingResRoute = useMemo(
    () => missingResourceRuleIds(routeOrdered, availableResTags),
    [routeOrdered, availableResTags],
  );
  const missingResDns = useMemo(
    () => missingResourceRuleIds(dnsOrdered, availableResTags),
    [dnsOrdered, availableResTags],
  );

  /* Use backend candidates, never the old report's ExactInline absorbed/covered claim. */
  const [meshRoute, setMeshRoute] = useState<{ report: MeshRouteReport | null; previous: boolean; legacy: boolean }>({
    report: null, previous: false, legacy: false,
  });
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
      setMeshRoute(previous => ({ ...previous, previous: true }));
    });
    return () => {
      meshRouteEpoch.current += 1;
    };
  }, [config, routeRules, proxyRunning]);

  const meshRuleHints = useMemo(() => {
    return meshRouteRuleHints(meshRoute.report, config, servers, routeRules, meshRoute.previous, isSmartMode);
  }, [isSmartMode, routeRules, meshRoute, config, servers]);

  const stagedOnlyRoute = useMemo(
    () => stagedOnlyIds(routeRules, diskRouteRules),
    [routeRules, diskRouteRules],
  );
  const stagedOnlyDns = useMemo(() => stagedOnlyIds(dnsRules, diskDnsRules), [dnsRules, diskDnsRules]);

  /* ═══════════ 写路径（与桌面逐条同形，见文件头注的重复登记）═══════════ */

  const commitOrder = useCallback(
    async (plane: 'route' | 'dns', ordered: Rule[]) => {
      const orderKey = plane === 'dns' ? 'dnsRuleOrder' : 'routeRuleOrder';
      clearWriteError(`order:${plane}`);
      setOptimisticOrder((prev) => ({ ...prev, [plane]: ordered.map((r) => r.id) }));
      // 顺序在条目模型里是**整集合的主键序列**，不是某个实体的字段；让它绕过暂存是错的 ——
      // 同一页面里「改规则」进暂存、「拖排序」直落盘，条上的「N 项待保存」就说不清列表现在是什么顺序，
      // 而顺序决定命中优先级。
      /*
       * 闸门的第一个实参必须是**键字面量**（`config-write-wiring.test.ts` 的 T3 形态判据）：
       * 传变量的话，静态分析说不出这次改的是哪个 config 键，去向判定就没人能核。
       * 故这里按平面分支写死两个键，而不是把 `orderKey` 传进去。
       */
      const orderStaged =
        plane === 'dns'
          ? editRoute('dnsRuleOrder', stagingEnabled) === 'staged'
          : editRoute('routeRuleOrder', stagingEnabled) === 'staged';
      if (orderStaged) {
        stage({
          id: `order:${orderKey}`,
          kind: 'rule',
          label: tr('home.stagedRuleOrder'),
          entityPath: [orderKey],
          nextValue: ordered.map((r) => r.id),
        });
        return;
      }
      try {
        await api.rules.reorder(
          ordered.map((r) => r.id),
          plane,
        );
        toast.success(tr('rules.reorderOk'));
      } catch (err) {
        // 乐观更新的回滚是**静默**的：行弹回原位与「拖拽功能坏了」完全同形。
        // 排序决定命中优先级，用户以为改了实际没改 = 分流按旧优先级跑。必须报。
        console.error('[MobileRulesScreen] reorder failed:', err);
        setOptimisticOrder((prev) => ({ ...prev, [plane]: undefined }));
        toast.error(tr('rules.reorderFail'));
        reportWriteError(`order:${plane}`, tr('rules.reorderFail'));
      }
    },
    [stagingEnabled, stage, tr, clearWriteError, reportWriteError],
  );

  const handleMove = useCallback(
    (plane: 'route' | 'dns', rule: Rule, to: 'up' | 'down' | 'top' | 'bottom') => {
      const list = plane === 'dns' ? dnsOrdered : routeOrdered;
      const from = list.findIndex((r) => r.id === rule.id);
      if (from === -1) return;
      const target =
        to === 'up' ? from - 1 : to === 'down' ? from + 1 : to === 'top' ? 0 : list.length - 1;
      // 边界外 = 空操作（面板里那两颗此时本就 disabled，这里是旁路防御，不发 IPC）。
      if (target < 0 || target >= list.length || target === from) return;
      const ordered = [...list];
      const [moved] = ordered.splice(from, 1);
      ordered.splice(target, 0, moved);
      setSheetRuleId(null);
      void commitOrder(plane, ordered);
    },
    [routeOrdered, dnsOrdered, commitOrder],
  );

  /**
   * 长按拖拽落点的提交（IA §4.13）。与 `handleMove` 是同一条写路径的两个入口 ——
   * 都算出完整的新顺序再交给 `commitOrder`，暂存 / 直落盘 / 乐观回滚三件事只有一份实现。
   */
  const handleReorder = useCallback(
    (plane: 'route' | 'dns', rule: Rule, toIndex: number) => {
      const list = plane === 'dns' ? dnsOrdered : routeOrdered;
      const from = list.findIndex((r) => r.id === rule.id);
      if (from === -1) return;
      const target = Math.min(Math.max(toIndex, 0), list.length - 1);
      if (target === from) return;
      const ordered = [...list];
      const [moved] = ordered.splice(from, 1);
      ordered.splice(target, 0, moved);
      void commitOrder(plane, ordered);
    },
    [routeOrdered, dnsOrdered, commitOrder],
  );

  const handleToggle = useCallback(
    async (plane: 'route' | 'dns', rule: Rule) => {
      const collectionKey = plane === 'dns' ? 'dnsRules' : 'trafficRules';
      const currentEffect = plane === 'dns' ? ruleDnsEffect(rule) : ruleRouteEffect(rule);
      if (!currentEffect) return;
      clearWriteError(`rule:${rule.id}`);
      const next: Rule = {
        ...rule,
        effects: {
          ...rule.effects,
          [plane]: { ...currentEffect, enabled: currentEffect.enabled === false },
        },
      };
      const prev = plane === 'dns' ? useAppStore.getState().dnsRules : useAppStore.getState().rules;
      const optimistic = prev.map((r) => (r.id === next.id ? next : r));
      useAppStore.setState(plane === 'dns' ? { dnsRules: optimistic } : { rules: optimistic });
      const stageRule =
        plane === 'dns'
          ? editRoute('dnsRules', stagingEnabled) === 'staged'
          : editRoute('trafficRules', stagingEnabled) === 'staged';
      if (stageRule) {
        stage({
          id: `rule:${next.id}`,
          kind: 'rule',
          label: `${tr('rules.editTitle')} ${next.remarks || next.type}`,
          entityPath: [collectionKey, next.id],
          nextValue: next,
        });
        return;
      }
      try {
        await api.rules.update(next, plane);
      } catch (err) {
        // 开关弹回原位是静默的。停用了以为停用了、其实规则还在生效 → 必须报。
        console.error('[MobileRulesScreen] toggle failed:', err);
        useAppStore.setState(plane === 'dns' ? { dnsRules: prev } : { rules: prev });
        toast.error(tr('rules.saveFailed'));
        // 只回滚不报，开关就是「弹回去了、什么都没说」——与「功能坏了」完全同形。
        reportWriteError(`rule:${rule.id}`, tr('rules.saveFailed'));
      }
    },
    [stagingEnabled, stage, tr, clearWriteError, reportWriteError],
  );

  const handleDuplicate = useCallback(
    async (plane: 'route' | 'dns', rule: Rule) => {
      const collectionKey = plane === 'dns' ? 'dnsRules' : 'trafficRules';
      setSheetRuleId(null);
      clearWriteError(`rule:${rule.id}`);
      try {
        const payload = duplicateRulePayload(rule, tr('rules.copySuffix'));
        const stageRule =
          plane === 'dns'
            ? editRoute('dnsRules', stagingEnabled) === 'staged'
            : editRoute('trafficRules', stagingEnabled) === 'staged';
        if (stageRule) {
          // 前端自铸 id（后端只在落盘那一刻发 id，而条目现在就要一个稳定的实体寻址键）。
          const entityId = crypto.randomUUID();
          stage({
            id: `rule:${entityId}`,
            kind: 'rule',
            label: `${tr('rules.newTitle')} ${payload.remarks ?? payload.type}`,
            entityPath: [collectionKey, entityId],
            nextValue: { ...payload, id: entityId },
          });
          toast.success(tr('rules.duplicated'));
          return;
        }
        await api.rules.add(payload, plane);
        void loadConfig(true);
        toast.success(tr('rules.duplicated'));
      } catch (err) {
        console.error('[MobileRulesScreen] duplicate failed:', err);
        toast.error(tr('rules.saveFailed'));
        reportWriteError(`rule:${rule.id}`, tr('rules.saveFailed'));
      }
    },
    [stagingEnabled, stage, loadConfig, tr, clearWriteError, reportWriteError],
  );

  const requestDeleteRule = useCallback(
    (plane: 'route' | 'dns', rule: Rule) => {
      clearWriteError(`rule:${rule.id}`);
      confirmTwice(`${RULE_DEL_PREFIX}${rule.id}`, () => {
        void (async () => {
          try {
            await (plane === 'dns' ? deleteDnsRule : deleteRouteRule)(rule);
            setSheetRuleId(null);
          } catch (err) {
            console.error('[MobileRulesScreen] delete failed:', err);
            toast.error(tr('rules.deleteFail'));
            // 删除的可见结果就是「这行消失」；静默失败与「按钮失灵」完全同形。
            reportWriteError(`rule:${rule.id}`, tr('rules.deleteFail'));
          }
        })();
      });
    },
    [confirmTwice, deleteDnsRule, deleteRouteRule, tr, clearWriteError, reportWriteError],
  );

  const handleRegionChange = useCallback(
    async (next: RegionRoutingConfig) => {
      const cur = useAppStore.getState().config;
      if (!cur) return;
      clearWriteError('regionRouting');
      const prevRouting = effectiveRegionRouting(cur);
      const regionChanged = prevRouting.region !== next.region;
      const reverseChanged = prevRouting.reverse !== next.reverse;
      if (editRoute('regionRouting', stagingEnabled) === 'staged') {
        stage({
          id: 'setting:regionRouting',
          kind: 'setting',
          label: tr('home.stagedSetting', { key: 'regionRouting' }),
          entityPath: ['regionRouting'],
          nextValue: next,
        });
      } else {
        try {
          await useAppStore.getState().saveConfig({ regionRouting: next });
        } catch (err) {
          // 保存失败**不回滚**（受控组件，按钮态跟着 store 走）：表现为「点了没反应」，必须报。
          console.error('[MobileRulesScreen] regionRouting save failed:', err);
          toast.error(tr('rules.saveFailed'));
          // 受控组件写失败不回滚 ⇒ 表现就是「点了没反应」，必须在控件旁边说出来。
          reportWriteError('regionRouting', tr('rules.saveFailed'));
          return;
        }
      }
      // 按钮的选中态只说「选中了」，说不出「选中意味着什么」——这两条 toast 才是语义。
      if (regionChanged) {
        const regionName =
          next.region === 'cn'
            ? tr('rules.region.cn')
            : next.region === 'ir'
              ? tr('rules.region.ir')
              : tr('rules.region.ru');
        toast.info(tr('rules.regionPicked', { region: regionName }));
      }
      if (reverseChanged) {
        if (next.reverse) toast.success(tr('rules.reverseOn'));
        else toast.info(tr('rules.reverseOff'));
      }
    },
    [stagingEnabled, stage, tr, clearWriteError, reportWriteError],
  );

  const handleBackToSmart = useCallback(async () => {
    clearWriteError('proxyMode');
    try {
      await useAppStore.getState().updateProxyMode('smart');
      toast.success(tr('rules.backToSmartOk'));
    } catch (err) {
      // 失败时那条模式警告仍留在原地 —— 不报的话与「按钮失灵」同形。
      console.error('[MobileRulesScreen] backToSmart failed:', err);
      toast.error(tr('rules.backToSmartFail'));
      reportWriteError('proxyMode', tr('rules.backToSmartFail'));
    }
  }, [tr, clearWriteError, reportWriteError]);

  /* ═══════════ 分段：应用 ═══════════ */

  const presets = useAppPresetsStore((s) => s.presets);
  const presetsFailed = useAppPresetsStore((s) => s.failed);
  const presetsLoading = useAppPresetsStore((s) => s.loading);
  const presetsLoaded = useAppPresetsStore((s) => s.loaded);
  const loadPresets = useAppPresetsStore((s) => s.loadPresets);
  useEffect(() => {
    void loadPresets();
  }, [loadPresets]);

  const [appCategory, setAppCategory] = useState<string>(APP_CATEGORY_ALL);
  /* 排布档（卡片 / 列表）。与桌面同：纯组件状态，不落盘 —— 它是一个观看偏好，不是配置。 */
  const [appView, setAppView] = useState<AppView>('cards');
  const [appSearch, setAppSearch] = useState('');
  /* 分类 `select-sheet` 的开合态。与本屏其余面板同一套：状态在容器，分段是纯呈现。 */
  const [appCategorySheet, setAppCategorySheet] = useState(false);
  /* 打开了策略面板的那一行。单点持有 —— 同屏两个面板同时开是误触面（与资源行同口径）。 */
  const [appPolicySheetId, setAppPolicySheetId] = useState<string | null>(null);

  const allPresets = useMemo(
    () => mergeAppPresets(presets ?? [], config?.customAppPresets),
    [presets, config?.customAppPresets],
  );
  /** 内置预设的 id 集合。自定义应用的 `labelKey` 就是用户填的名字本身，内置的才走 `rules.apps.*`。 */
  const builtinPresetIds = useMemo(() => new Set((presets ?? []).map((p) => p.id)), [presets]);
  const appRuleById = useMemo(() => {
    const m = new Map<string, AppRule>();
    for (const rule of config?.appRules ?? []) m.set(rule.appId, rule);
    return m;
  }, [config?.appRules]);

  /**
   * 应用策略的显示文案与配色 —— 与桌面 `appPolicyView`（`AppPolicyScreen.tsx:787-804`）**逐条同义**。
   * 两处易漂且都会显示错值：
   *  · `action==='proxy'` 但**没指定节点** = 快选「默认代理」的落盘形态 ⇒ 显示「跟随全局」，不是「默认代理」；
   *  · 指定了节点但节点**已删** ⇒ 显示「指定节点」，不是回落成别的词。
   */
  const appPolicyOf = useCallback(
    (preset: AppPreset): { text: string; tone: AppRowModel['policyTone']; value: string } => {
      const rule = appRuleById.get(preset.id);
      if (!rule) return { text: tr('appPolicy.followGlobal'), tone: 'follow', value: 'follow' };
      if (rule.action === 'direct') {
        return { text: tr('appPolicy.action.direct'), tone: 'direct', value: 'direct' };
      }
      if (rule.action === 'block') {
        return { text: tr('appPolicy.action.block'), tone: 'block', value: 'block' };
      }
      if (rule.targetServerId) {
        const name = serverNameById.get(rule.targetServerId);
        /* 节点已删时文案退回「指定节点」，但**值仍指向那个 id** —— 选择器据此在列表里勾它，
           勾不到就一个都不勾（那正是「这个节点没了」该有的表现），而不是错勾成跟随全局。 */
        return {
          text: name ?? tr('appPolicy.summary.node'),
          tone: 'proxy',
          value: `node:${rule.targetServerId}`,
        };
      }
      return { text: tr('appPolicy.followGlobal'), tone: 'proxy', value: 'follow' };
    },
    [appRuleById, serverNameById, tr],
  );

  const appCategories = useMemo(
    () =>
      sortAppCategories(
        APP_CATEGORY_KEYS.map((key) => ({ key, label: tr(`appPolicy.cat.${key}`) })),
        i18n.language,
      ),
    [tr, i18n.language],
  );

  const appGroups = useMemo(() => {
    const needle = appSearch.trim().toLowerCase();
    const rows: AppRowModel[] = [];
    for (const preset of allPresets) {
      if (appCategory !== APP_CATEGORY_ALL && preset.category !== appCategory) continue;
      const name = appPresetLabel(t, preset.labelKey, preset.id, !builtinPresetIds.has(preset.id));
      const processes = preset.processNames?.join(' · ');
      if (
        needle !== '' &&
        !name.toLowerCase().includes(needle) &&
        !(processes ?? '').toLowerCase().includes(needle)
      ) {
        continue;
      }
      const policy = appPolicyOf(preset);
      rows.push({
        id: preset.id,
        name,
        emoji: preset.emoji,
        processes,
        policyText: policy.text,
        policyTone: policy.tone,
        policyValue: policy.value,
        /* 「是不是自定义」与显示名走**同一个判据**（`builtinPresetIds`）：另起一条会在
           内置表加载失败那一刻分叉 —— 那时每一条都会被判成自定义，于是每一条都长出删除键。 */
        isCustom: !builtinPresetIds.has(preset.id),
        category: preset.category,
      });
    }
    const byCat = new Map<string, AppRowModel[]>();
    for (const row of rows) {
      const bucket = byCat.get(row.category);
      if (bucket) bucket.push(row);
      else byCat.set(row.category, [row]);
    }
    return [...byCat.entries()]
      .map(([key, list]) => ({
        key,
        label: tr(`appPolicy.cat.${key}`),
        rows: [...list].sort((a, b) => compareLabel(a.name, b.name, i18n.language)),
      }))
      .sort((a, b) => compareLabel(a.label, b.label, i18n.language));
  }, [allPresets, appCategory, appSearch, appPolicyOf, builtinPresetIds, t, tr, i18n.language]);

  const appSummary = useMemo(() => {
    let follow = 0;
    let node = 0;
    let direct = 0;
    let block = 0;
    // 与桌面 `AppPolicyScreen.tsx:203-217` 逐条同义：**无显式规则 / 规则未启用 /
    // action=proxy 但未指定节点** 三种都归「跟随全局」（即视觉默认态），五路恒和为 total。
    for (const preset of allPresets) {
      const rule = appRuleById.get(preset.id);
      const action = rule?.enabled ? rule.action : undefined;
      if (action === 'direct') direct += 1;
      else if (action === 'block') block += 1;
      else if (action === 'proxy' && rule?.targetServerId) node += 1;
      else follow += 1;
    }
    return { total: allPresets.length, follow, node, direct, block };
  }, [allPresets, appRuleById]);

  /** `undefined` = 开（兼容老配置）。与桌面 `AppPolicyScreen.tsx:117` 逐字同义 —— 写成 `=== true`
   *  会让老配置在移动端显示「总开关关闭」，而分流其实正在生效。 */
  const appMasterEnabled = config?.appRoutingEnabled !== false;
  const toggleAppMaster = useCallback(async () => {
    const next = !appMasterEnabled;
    clearWriteError('appRoutingEnabled');
    if (editRoute('appRoutingEnabled', stagingEnabled) === 'staged') {
      stage({
        id: 'setting:appRoutingEnabled',
        kind: 'setting',
        label: tr('appPolicy.masterTip'),
        entityPath: ['appRoutingEnabled'],
        nextValue: next,
      });
      return;
    }
    try {
      await useAppStore.getState().saveConfig({ appRoutingEnabled: next });
    } catch (err) {
      console.error('[MobileRulesScreen] appRoutingEnabled save failed:', err);
      toast.error(tr('appPolicy.masterToggleFail'));
      reportWriteError('appRoutingEnabled', tr('appPolicy.masterToggleFail'));
    }
  }, [appMasterEnabled, stagingEnabled, stage, tr, clearWriteError, reportWriteError]);

  /**
   * 策略选择器的候选：**一组策略 + 若干组按订阅折叠的节点**，与桌面 `PolicySelector` 同形
   * （`AppPolicyScreen.tsx:924-994`：`mm-lbl` 策略三项 + `mm-sep` + `ns-grp` 分组 + `mi ns-node`）。
   *
   * 分组与默认展开集复用 `domain/server-grouping` 的两个函数 —— 与规则表单的「目标出站」、
   * 桌面三处选择器读的是**同一份**判据。自建 / 组网组的 `name` 是占位符，按布尔本地化。
   */
  const appNodeGroups = useMemo(
    () => groupServersBySubscription(servers, config?.subscriptions ?? []),
    [servers, config?.subscriptions],
  );
  const appPolicyGroups: CselGroup[] = useMemo(
    () => [
      {
        label: tr('appPolicy.policy'),
        options: [
          { value: 'follow', label: tr('appPolicy.followGlobal') },
          { value: 'direct', label: tr('appPolicy.action.direct') },
          /* 动作标签轴：「阻断」恒 `--err`，走 `danger` 通道而不是在本屏刷一层红。 */
          { value: 'block', label: tr('appPolicy.action.block'), danger: true },
        ],
      },
      ...appNodeGroups.map((group) => ({
        id: group.id,
        label: group.isManual
          ? tr('nodes.tab.manual')
          : group.isMesh
            ? tr('nodes.tab.mesh')
            : group.name,
        options: group.servers.map((server) => ({
          value: `node:${server.id}`,
          label: server.name,
        })),
      })),
    ],
    [appNodeGroups, tr],
  );
  /** 面板打开的那一行当前选中的节点（非节点档为 undefined ⇒ 全折叠，**不猜第一组**）。 */
  const appPolicyOpenGroups = useMemo(() => {
    const row = appGroups.flatMap((group) => group.rows).find((r) => r.id === appPolicySheetId);
    const nodeId = row?.policyValue.startsWith('node:') === true
      ? row.policyValue.slice(5)
      : undefined;
    return defaultOpenGroupIds(appNodeGroups, nodeId);
  }, [appGroups, appPolicySheetId, appNodeGroups]);

  /**
   * 落一档应用策略 —— 与桌面 `AppPolicyScreen.tsx:225 applyRule` / `:259 setAppPolicy` 同一条写路径
   * （`mutateConfigEntities` + `appRules` 暂存闸门）。
   *
   * 🔴 **落盘形态不在这里拼**，由 `./app-policy-write` 的 `appRuleForPick` 给：那是一条跨语言契约的
   * UI 端（`inbounds.rs:717` 是另一端），拼在这里就只剩「正则去认一段对象字面量」这一种判法。
   * 两处刻意的差别（`follow` 写成 proxy+无节点、`enabled` 恒 true）连同理由都写在那个模块里。
   *
   * 失败走**行内**回显而不是 toast：移动端没有 toast 宿主（见 `writeErrors` 那段头注），
   * 且 `half-truth-facts.test.ts` 的静音 toast 棘轮正是为了不让这批回声继续增长。
   */
  const handleAppPolicyPick = useCallback(
    async (appId: string, pick: AppPolicyWritablePick, targetServerId?: string) => {
      setAppPolicySheetId(null);
      clearWriteError(`appRule:${appId}`);
      const next: AppRule = appRuleForPick(appId, pick, targetServerId);
      const preset = allPresets.find((p) => p.id === appId);
      const name =
        preset != null
          ? appPresetLabel(t, preset.labelKey, preset.id, !builtinPresetIds.has(preset.id))
          : appId;
      /* 配置暂存闸门：`appRules` 是 UserConfig 字段（Class B），条目按 **appId** 寻址。 */
      if (editRoute('appRules', stagingEnabled) === 'staged') {
        stage({
          id: `appRule:${appId}`,
          kind: 'appRule',
          label: tr('home.stagedAppPolicy', { name }),
          entityPath: ['appRules', appId],
          nextValue: next,
        });
        return;
      }
      try {
        await useAppStore
          .getState()
          .mutateConfigEntities([{ collection: 'appRules', entityId: appId, value: next }]);
      } catch (err) {
        // 写失败时 store 未 patch ⇒ 触发器停在旧档，与「面板点了没反应」同形，必须报。
        console.error('[MobileRulesScreen] appRule save failed:', err);
        reportWriteError(`appRule:${appId}`, tr('appPolicy.policyUpdateFail'));
      }
    },
    [
      allPresets,
      builtinPresetIds,
      stagingEnabled,
      stage,
      t,
      tr,
      clearWriteError,
      reportWriteError,
    ],
  );

  /**
   * 删除一个自定义应用。
   *
   * 🔴 **预设与它的 appRule 必须在同一个事务里删**（同桌面 `removeCustomApp`）：
   * 只删预设会在盘上留一条指向不存在应用的孤儿规则 —— 它不会报错、不会显示，
   * 而 route builder 照样按它发一条 `rule_set` 规则。暂存腿同理，两条条目共用一个 `groupId`。
   */
  const requestRemoveCustomApp = useCallback(
    (appId: string) => {
      clearWriteError(`appRule:${appId}`);
      const preset = allPresets.find((candidate) => candidate.id === appId);
      const name =
        preset != null
          ? appPresetLabel(t, preset.labelKey, preset.id, !builtinPresetIds.has(preset.id))
          : appId;
      const hasRule = appRuleById.has(appId);
      confirmTwice(`${APP_REMOVE_PREFIX}${appId}`, () => {
        if (editRoute('customAppPresets', stagingEnabled) === 'staged') {
          stage({
            id: `appPreset:${appId}`,
            kind: 'appPreset',
            label: `${tr('common.delete')} ${name}`,
            entityPath: ['customAppPresets', appId],
            nextValue: null,
          });
          if (hasRule) {
            stage({
              id: `appRule:${appId}`,
              kind: 'appRule',
              label: `${tr('common.delete')} ${name}`,
              entityPath: ['appRules', appId],
              nextValue: null,
            });
          }
          return;
        }
        void (async () => {
          try {
            await useAppStore.getState().mutateConfigEntities([
              { collection: 'customAppPresets', entityId: appId, value: null },
              ...(hasRule
                ? [{ collection: 'appRules' as const, entityId: appId, value: null }]
                : []),
            ]);
          } catch (err) {
            console.error('[MobileRulesScreen] remove custom app failed:', err);
            reportWriteError(`appRule:${appId}`, tr('appPolicy.removeFail'));
          }
        })();
      });
    },
    [
      allPresets,
      appRuleById,
      builtinPresetIds,
      confirmTwice,
      stagingEnabled,
      stage,
      t,
      tr,
      clearWriteError,
      reportWriteError,
    ],
  );

  /* ═══════════ 分段：资源 ═══════════ */

  const [resSource, setResSource] = useState<ResourceSource>('all');

  /**
   * 暂存里已被删除的资源要从展示面消失。后端列表反映的是**磁盘**与文件状态，而暂存删除还没落盘 ——
   * 不过滤的话，用户点了删、条上记了一笔「待保存」，行却还在，读作「删除没生效」。
   * 内置 geo 不在 `config.ruleResources` 里，恒保留（与桌面 `ResourcesScreen.tsx:148-157` 同）。
   */
  const effectiveResourceIds = useMemo(
    () => (config == null ? null : new Set((config.ruleResources ?? []).map((r) => r.id))),
    [config],
  );

  const resourceGroups = useMemo(() => {
    const byCat = new Map<string, ResourceRowModel[]>();
    for (const item of resItems) {
      if (
        effectiveResourceIds !== null &&
        item.builtin !== true &&
        !effectiveResourceIds.has(item.id)
      ) {
        continue;
      }
      if (resSource === 'builtin' && item.builtin !== true) continue;
      if (resSource === 'external' && item.builtin === true) continue;
      const progress = resProgress[item.id];
      const refs = resourceRefs(
        item.id,
        routeRules,
        config?.appRules ?? [],
        presets ?? [],
        config?.customAppPresets,
      );
      const row: ResourceRowModel = {
        id: item.id,
        name: item.name,
        builtin: item.builtin === true,
        references: refs.length,
        progress:
          progress?.status === 'downloading' || progress?.status === 'queued'
            ? (progress.percent ?? 0) / 100
            : undefined,
        failed: progress?.status === 'error',
        cancelled: progress?.status === 'cancelled',
        /* `data-contract.json#sources.rule-resources`（2026-09-06 登记）。原样透传，
           不在这里做「没有就填 0」那种补全 —— 格式化与缺席兜底都在分段里，只有一处。 */
        size: item.size,
        updatedAt: item.downloadedAt,
      };
      const key = categoryLabel(item.category, tr('resources.categoryCustom'));
      const bucket = byCat.get(key);
      if (bucket) bucket.push(row);
      else byCat.set(key, [row]);
    }
    return [...byCat.entries()].map(([key, rows]) => ({ key, label: key, rows }));
  }, [resItems, resSource, resProgress, routeRules, config, presets, tr, effectiveResourceIds]);

  const handleUpdateAll = useCallback(async () => {
    if (resUpdatingAll || resUpdatingIds.size > 0) return;
    setResUpdatingAll(true);
    clearWriteError('res:all');
    toast.info(tr('resources.updateAllStarted'), { key: 'resource-update-all', sticky: true });
    try {
      const feedback = resourceUpdateFeedback(
        resourceUpdateOutcome(await api.ruleResources.updateAll(), new Set(resItems.map((item) => item.id)).size), tr,
      );
      if (feedback.tone === 'error') {
        toast.error(feedback.text, undefined, { key: 'resource-update-all' });
        reportWriteError('res:all', feedback.text);
      } else toast[feedback.tone](feedback.text, { key: 'resource-update-all' });
    } catch (err) {
      console.error('[MobileRulesScreen] updateAll failed:', err);
      toast.error(tr('resources.updateAllFailed'), undefined, { key: 'resource-update-all' });
      reportWriteError('res:all', tr('resources.updateAllFailed'));
    } finally {
      setResUpdatingAll(false);
      void reloadResources();
    }
  }, [reloadResources, tr, clearWriteError, reportWriteError, resItems, resUpdatingAll, resUpdatingIds]);

  const handleUpdateOne = useCallback(
    async (id: string) => {
      const item = resItems.find((r) => r.id === id);
      if (!item) return;
      if (resUpdatingAll || resUpdatingIds.has(id)) return;
      setResUpdatingIds((prev) => new Set(prev).add(id));
      setSheetResId(null);
      clearWriteError(`res:${id}`);
      try {
        // 内置与外置是**两条不同的更新腿**（内置按 name 走出厂源，外置按 id 重下）。
        const result = item.builtin === true
          ? await api.ruleResources.updateBuiltin(item.name)
          : await api.ruleResources.redownload(item.id);
        const feedback = resourceUpdateFeedback(resourceUpdateOutcome([result], 1), tr);
        toast[feedback.tone](feedback.text);
        if (feedback.tone === 'error') reportWriteError(`res:${id}`, feedback.text);
      } catch (err) {
        console.error('[MobileRulesScreen] update failed:', err);
        toast.error(tr('resources.updateFailed'));
        reportWriteError(`res:${id}`, tr('resources.updateFailed'));
      } finally {
        setResUpdatingIds((prev) => { const next = new Set(prev); next.delete(id); return next; });
        void reloadResources();
      }
    },
    [resItems, reloadResources, tr, clearWriteError, reportWriteError, resUpdatingAll, resUpdatingIds],
  );

  const handleCancelRes = useCallback(
    async (id: string) => {
      setSheetResId(null);
      clearWriteError(`res:${id}`);
      try {
        // 后端返回被真正中断的传输数；0 = 下载在这次点击之前就已经结束（不是失败）。
        // **必须容忍 null 信封**（同桌面 `ResourcesScreen.tsx:259`）：不守的话正常回包解构就抛，
        // 一路走到 catch 报「取消失败」——报了个假失败。
        const result = await api.ruleResources.cancel(id);
        if (result?.cancelled === 0) toast.info(tr('resources.cancelAlreadyDone'));
      } catch (err) {
        console.error('[MobileRulesScreen] cancel failed:', err);
        toast.error(tr('resources.cancelFailed'));
        reportWriteError(`res:${id}`, tr('resources.cancelFailed'));
      } finally {
        void reloadResources();
      }
    },
    [reloadResources, tr, clearWriteError, reportWriteError],
  );

  const handleDeleteRes = useCallback(
    (id: string) => {
      clearWriteError(`res:${id}`);
      confirmTwice(`${RES_DEL_PREFIX}${id}`, () => {
        /*
         * **暂存分流不能少**（桌面 `ResourcesScreen.tsx:220-230`）：`ruleResources` 是 Class B，
         * 命中暂存腿时只记一条「删除意图」（`nextValue: null`），真正的 unlink 留到 Apply。
         * 少了这一支 = 暂存开着也直接把盘上的规则文件删了，而条上一个字都没有 —— 而且
         * `config-write-wiring` 那道门是**按文件**查 `editRoute(...)==='staged'` 的，
         * 同文件别处有它就绿，抓不到这一处。
         */
        const item = resItems.find((r) => r.id === id);
        if (editRoute('ruleResources', stagingEnabled) === 'staged') {
          stage({
            id: `resource:${id}`,
            kind: 'resource',
            label: `${tr('common.delete')} ${item?.name ?? id}`,
            entityPath: ['ruleResources', id],
            nextValue: null,
          });
          toast.info(tr('resources.deleteDone'));
          setSheetResId(null);
          return;
        }
        void (async () => {
          try {
            await api.ruleResources.delete(id, true);
            toast.info(tr('resources.deleteDone'));
            setSheetResId(null);
          } catch (err) {
            console.error('[MobileRulesScreen] delete resource failed:', err);
            toast.error(tr('resources.deleteFail'));
            reportWriteError(`res:${id}`, tr('resources.deleteFail'));
          } finally {
            void reloadResources();
          }
        })();
      });
    },
    [
      confirmTwice,
      reloadResources,
      tr,
      clearWriteError,
      reportWriteError,
      resItems,
      stagingEnabled,
      stage,
    ],
  );

  /**
   * 重置内置资源 —— 一键覆盖 geosite + geoip **两类全部**内置资源，故走原地二次确认。
   * 传 `'all'`：后端对无法识别的 tag 走两类全重置，传 `'geosite'` 会静默漏掉内置 GeoIP。
   */
  const handleResetBuiltin = useCallback(() => {
    clearWriteError('res:reset');
    confirmTwice(GEO_RESET_KEY, () => {
      void (async () => {
        try {
          await api.ruleResources.resetBuiltin('all');
          toast.success(tr('resources.resetBuiltinDone'));
        } catch (err) {
          console.error('[MobileRulesScreen] resetBuiltin failed:', err);
          toast.error(tr('resources.resetBuiltinFailed'));
          reportWriteError('res:reset', tr('resources.resetBuiltinFailed'));
        } finally {
          void reloadResources();
        }
      })();
    });
  }, [confirmTwice, reloadResources, tr, clearWriteError, reportWriteError]);

  /* ═══════════ 二级页：DNS 服务器 / 分组 ═══════════ */

  /*
   * DNS 资源的读与写**都走设置页那条 `useConfig()` 漏斗**（与桌面 `DnsPolicyWorkspace` 1:1）：
   * 暂存 / 写盘 / 失败回滚只有一条路径，不在规则屏另造一套保存协议。
   *
   * ⚠️ 读也走它，不只是写：patch 的入参是「在**基准**上改一个字段的整份数组」。
   * 展示读一个源、写的基准取另一个源，就等于让基准跟着展示口径漂 —— 两个源今天恰好同值，
   * 不代表明天还同值（它们各自的合成时机不同）。同源是这条腿的前提，不是风格。
   */
  const {
    config: dnsConfig,
    update: updateConfig,
    loading: configLoading,
    error: configError,
    reload: reloadConfig,
  } = useConfig();

  /* 引用图与**写腿读同一个源**（`dnsConfig`）：展示读一个源、判据取另一个，
     两个源今天恰好同值不代表明天还同值（它们各自的合成时机不同）。 */
  const dnsServerRows = useMemo<DnsResourceRow[]>(
    () =>
      (dnsConfig?.dnsServers ?? []).map((server) => ({
        id: server.id,
        name: dnsServerNameById.get(server.id) ?? server.name,
        description: tr(`settings.dns.serverType_${server.type}`),
        enabled: server.enabled !== false,
        /* 内置判据走共享的 `isProtectedDnsServer`，不用 `id.startsWith('builtin-')` 猜：
           那个前缀只是命名习惯，而「哪三个不许删」是一张白名单。 */
        builtin: isProtectedDnsServer(server.id),
        references: dnsConfig == null ? 0 : dnsServerReferences(dnsConfig, server.id).length,
      })),
    [dnsConfig, dnsServerNameById, tr],
  );

  const dnsGroupRows = useMemo<DnsResourceRow[]>(
    () =>
      (dnsConfig?.dnsServerGroups ?? []).map((group) => ({
        id: group.id,
        name: group.name,
        description: tr(
          group.mode === 'race' ? 'settings.dns.groupRace' : 'settings.dns.groupFallback',
        ),
        enabled: group.enabled !== false,
        references: dnsConfig == null ? 0 : dnsGroupReferences(dnsConfig, group.id).length,
      })),
    [dnsConfig, tr],
  );

  /*
   * ⚠️ 两个开关都必须 `await` + `catch`，且要 `throwOnError: true`。
   * `useConfig().update` 的缺省行为是**自己把失败吞进 toast 并回滚**（`use-config.ts:356-374`）——
   * 而移动端的 toast 是个 console ⇒ 不 opt-in 抛错的话，开关会自己弹回去、一个字都没有。
   */
  const toggleDnsServer = useCallback(
    (id: string, next: boolean) => {
      clearWriteError(`dnsres:${id}`);
      void (async () => {
        try {
          await updateConfig(
            {
              dnsServers: (dnsConfig?.dnsServers ?? []).map((s) =>
                s.id === id ? { ...s, enabled: next } : s,
              ),
            },
            { throwOnError: true },
          );
        } catch (err) {
          console.error('[MobileRulesScreen] dns server toggle failed:', err);
          reportWriteError(`dnsres:${id}`, tr('common.saveFailed'));
        }
      })();
    },
    [updateConfig, dnsConfig?.dnsServers, clearWriteError, reportWriteError, tr],
  );

  const toggleDnsGroup = useCallback(
    (id: string, next: boolean) => {
      clearWriteError(`dnsres:${id}`);
      void (async () => {
        try {
          await updateConfig(
            {
              dnsServerGroups: (dnsConfig?.dnsServerGroups ?? []).map((g) =>
                g.id === id ? { ...g, enabled: next } : g,
              ),
            },
            { throwOnError: true },
          );
        } catch (err) {
          console.error('[MobileRulesScreen] dns group toggle failed:', err);
          reportWriteError(`dnsres:${id}`, tr('common.saveFailed'));
        }
      })();
    },
    [updateConfig, dnsConfig?.dnsServerGroups, clearWriteError, reportWriteError, tr],
  );

  /**
   * 删除一条 DNS 服务器 / 分组。**三道闸，顺序不可调**（逐条同桌面
   * `DnsPolicyWorkspace.tsx#requestDeleteDnsServer`）：
   *
   *  ① **内置保护** —— 三个内置服务器整条不许删（它们是引导链与两条默认解析腿的载体）。
   *     UI 上那一行本来就不画删除键，这里是旁路防御：「UI 拦住了」不等于「写路径拦住了」。
   *  ② **被引用则拒绝** —— 不是级联删、也不是弹窗问，而是**不删并说清被谁占着**。
   *     引用图走共享的 `dnsServerReferences` / `dnsGroupReferences`（`dns-resource-logic`），
   *     与行上那个引用数是同一份判据 ⇒ 角标显示 0 的行必然删得掉，显示 N 的必然删不掉。
   *     理由落**行内**（本屏机制：`reportWriteError`），不是屏顶飘一条。
   *  ③ **二次确认** —— `confirmTwice` + `.confirming` 类，与规则行 / 资源行同一条契约。
   */
  const requestDeleteDnsServer = useCallback(
    (id: string) => {
      if (isProtectedDnsServer(id)) return;
      clearWriteError(`dnsres:${id}`);
      const refs = dnsConfig == null ? [] : dnsServerReferences(dnsConfig, id);
      if (refs.length > 0) {
        reportWriteError(
          `dnsres:${id}`,
          tr('settings.dns.resourceInUseDesc', {
            refs: refs
              .map((reference) => dnsResourceReferenceText(reference, tr))
              .join(tr('common.listSeparator')),
          }),
        );
        return;
      }
      confirmTwice(`${DNS_SRV_DEL_PREFIX}${id}`, () => {
        void (async () => {
          try {
            await updateConfig(
              { dnsServers: (dnsConfig?.dnsServers ?? []).filter((server) => server.id !== id) },
              { throwOnError: true },
            );
          } catch (err) {
            console.error('[MobileRulesScreen] dns server delete failed:', err);
            reportWriteError(`dnsres:${id}`, tr('common.saveFailed'));
          }
        })();
      });
    },
    [dnsConfig, updateConfig, confirmTwice, tr, clearWriteError, reportWriteError],
  );

  const requestDeleteDnsGroup = useCallback(
    (id: string) => {
      clearWriteError(`dnsres:${id}`);
      const refs = dnsConfig == null ? [] : dnsGroupReferences(dnsConfig, id);
      if (refs.length > 0) {
        reportWriteError(
          `dnsres:${id}`,
          tr('settings.dns.resourceInUseDesc', {
            refs: refs
              .map((reference) => dnsResourceReferenceText(reference, tr))
              .join(tr('common.listSeparator')),
          }),
        );
        return;
      }
      confirmTwice(`${DNS_GRP_DEL_PREFIX}${id}`, () => {
        void (async () => {
          try {
            await updateConfig(
              {
                dnsServerGroups: (dnsConfig?.dnsServerGroups ?? []).filter(
                  (group) => group.id !== id,
                ),
              },
              { throwOnError: true },
            );
          } catch (err) {
            console.error('[MobileRulesScreen] dns group delete failed:', err);
            reportWriteError(`dnsres:${id}`, tr('common.saveFailed'));
          }
        })();
      });
    },
    [dnsConfig, updateConfig, confirmTwice, tr, clearWriteError, reportWriteError],
  );

  /* ═══════════ 二级页：网络场景（spec §6.1 / §6.2）═══════════ */

  /*
   * 场景集合的读与写同 DNS 资源那一条腿：`useConfig()` 漏斗（桌面 `NetworkProfilePanel` 同一条），
   * 展示与 patch 的基准同源。「本机将使用哪种探测、可不可用」由后端与生成器同一个函数算好
   * （`useResolvedProbes`），这里只翻成文案 —— 渲染端重算必然漂移（spec §4.4 末段）。
   */
  const networkProfiles = useMemo(() => dnsConfig?.networkProfiles ?? [], [dnsConfig?.networkProfiles]);
  const resolvedProbes = useResolvedProbes(dnsConfig);

  const networkProfileRows = useMemo<NetworkProfileRowModel[]>(
    () =>
      networkProfiles.map((profile) => {
        const refs = profileRefCounts(profile.id, routeRules, dnsRules);
        const summary = criteriaSummary(profile);
        const status = profileRowStatus(profile, resolvedProbes);
        /* 摘要拼法逐段同桌面 `ProfileList`（地址段 ×n · 搜索域 ×n (样例)）。 */
        const parts = [
          summary.cidrs > 0 ? `${tr('rules.networkProfile.summaryCidrs')} ×${summary.cidrs}` : '',
          summary.domains > 0 ? `${tr('rules.networkProfile.summaryDomains')} ×${summary.domains}` : '',
        ].filter(Boolean);
        return {
          id: profile.id,
          name: profile.name,
          enabled: profile.enabled,
          summary: `${parts.join(' · ')}${summary.sample ? ` (${summary.sample})` : ''}`,
          refs: tr('rules.networkProfile.refs', { route: refs.route, dns: refs.dns }),
          probe:
            status.kind === 'probe'
              ? {
                  text: probeDisplayText(status.display, t, mobileReasonKey),
                  unavailable: status.display.kind === 'unavailable',
                }
              : undefined,
          warning:
            status.kind === 'probe' && status.warningKey !== null ? tr(status.warningKey) : undefined,
          match:
            status.kind === 'probe' && status.match !== null
              ? { state: status.match, text: tr(PROFILE_MATCH_KEYS[status.match]) }
              : undefined,
        };
      }),
    [networkProfiles, routeRules, dnsRules, resolvedProbes, tr, t],
  );

  /** 启停一个场景。与 DNS 资源开关同一条理由：`await` + `throwOnError`，失败落在这一行上。 */
  const toggleNetworkProfile = useCallback(
    (id: string, next: boolean) => {
      clearWriteError(`np:${id}`);
      void (async () => {
        try {
          await updateConfig(
            { networkProfiles: networkProfiles.map((p) => (p.id === id ? { ...p, enabled: next } : p)) },
            { throwOnError: true },
          );
        } catch (err) {
          console.error('[MobileRulesScreen] network profile toggle failed:', err);
          reportWriteError(`np:${id}`, tr('common.saveFailed'));
        }
      })();
    },
    [updateConfig, networkProfiles, clearWriteError, reportWriteError, tr],
  );

  /**
   * 删除一个场景。叠一层确认面板，正文说清后果：有规则在用 ⇒「N 条规则将停止生效」
   * （spec §6.2；规则本身不动，悬空引用按 fail-closed 不生成，§3.4-2）；没人用 ⇒ 引用计数那一句（0 / 0）。
   */
  const requestDeleteNetworkProfile = useCallback(
    (id: string) => {
      const profile = networkProfiles.find((p) => p.id === id);
      if (profile === undefined) return;
      clearWriteError(`np:${id}`);
      const refs = profileRefCounts(id, routeRules, dnsRules);
      const inUse = refs.route + refs.dns;
      const confirmId = openMobileForm({
        kind: 'confirm',
        payload: {
          title: profile.name,
          message:
            inUse > 0
              ? tr('rules.networkProfile.deleteRefsWarn', { count: inUse })
              : tr('rules.networkProfile.refs', { route: refs.route, dns: refs.dns }),
          confirmLabel: tr('common.delete'),
          danger: true,
          onConfirm: () => {
            closeMobileForm(confirmId);
            void (async () => {
              try {
                await updateConfig(
                  { networkProfiles: networkProfiles.filter((p) => p.id !== id) },
                  { throwOnError: true },
                );
              } catch (err) {
                console.error('[MobileRulesScreen] network profile delete failed:', err);
                reportWriteError(`np:${id}`, tr('common.saveFailed'));
              }
            })();
          },
        },
      });
    },
    [networkProfiles, routeRules, dnsRules, updateConfig, clearWriteError, reportWriteError, tr],
  );

  /** 未命中默认动作的当前值（只读展示；改它要 `select-sheet` + 完整选项集，归后续线）。 */
  const dnsDefaultActionText = useMemo(() => {
    /*
     * `dnsDefaults` 缺席时**不能**显示「跟随流量路由」：未命中默认动作没有「继承」可言，
     * 那是一个错的当前值。桌面 `DnsPolicyWorkspace.tsx:124-131` 按 `enableFakeIp` 推缺省 ——
     * 开着 = 返回 FakeIP，关着 = 走内置国内服务器。照抄。
     */
    const action =
      config?.dnsDefaults?.unmatchedAction ??
      (config?.dnsConfig?.enableFakeIp === false
        ? ({ type: 'server', serverId: 'builtin-domestic' } as const)
        : ({ type: 'fakeIp' } as const));
    if (action.type === 'server')
      return tr('rules.dnsActionServer', {
        name: dnsServerNameById.get(action.serverId) ?? action.serverId,
      });
    if (action.type === 'group')
      return tr('rules.dnsActionGroup', {
        name: dnsGroupNameById.get(action.groupId) ?? action.groupId,
      });
    if (action.type === 'hostsFirst')
      return tr('rules.dnsActionHosts', {
        name: dnsServerNameById.get(action.hostsServerId) ?? action.hostsServerId,
      });
    if (action.type === 'reject') return tr('rules.dnsActionReject');
    if (action.type === 'predefined') return tr('rules.dnsActionPredefined');
    if (action.type === 'fakeIp') return tr('rules.dnsActionFakeIp');
    return tr('rules.dnsResolverInherit');
  }, [config?.dnsDefaults, config?.dnsConfig, dnsServerNameById, dnsGroupNameById, tr]);

  /** `hostsFirst` 的「未命中后的服务器」那一行（桌面 `DnsPolicyWorkspace.tsx:256-267`）。 */
  const dnsDefaultFallbackText = useMemo(() => {
    const action = config?.dnsDefaults?.unmatchedAction;
    if (action?.type !== 'hostsFirst') return undefined;
    const fb = action.fallback;
    if (!fb) return tr('rules.dnsResolverInherit');
    if (fb.type === 'server')
      return tr('rules.dnsActionServer', {
        name: dnsServerNameById.get(fb.serverId) ?? fb.serverId,
      });
    if (fb.type === 'group')
      return tr('rules.dnsActionGroup', { name: dnsGroupNameById.get(fb.groupId) ?? fb.groupId });
    if (fb.type === 'reject') return tr('rules.dnsActionReject');
    if (fb.type === 'predefined') return tr('rules.dnsActionPredefined');
    if (fb.type === 'fakeIp') return tr('rules.dnsActionFakeIp');
    return tr('rules.dnsResolverInherit');
  }, [config?.dnsDefaults, dnsServerNameById, dnsGroupNameById, tr]);

  /* ═══════════ 规则行绑定 ═══════════ */

  /* 规则行徽标的场景表取**展示面**（暂存中新建的场景也要认得，同桌面 `RulesScreen`）；
     探测源取后端解析结果（与二级页同一份拉取）。 */
  const badgeProfiles = useMemo(() => config?.networkProfiles ?? [], [config?.networkProfiles]);

  const bindRows = useCallback(
    (plane: 'route' | 'dns', list: Rule[]): RuleRowBinding[] =>
      list.map((rule) => {
        const route = ruleRouteEffect(rule);
        const dnsEffect = ruleDnsEffect(rule);
        const targetServerId = route?.action === 'proxy' ? route.targetServerId : undefined;
        const dnsAction = dnsEffect?.action;
        const dnsActionName =
          dnsAction?.type === 'server'
            ? dnsServerNameById.get(dnsAction.serverId)
            : dnsAction?.type === 'group'
              ? dnsGroupNameById.get(dnsAction.groupId)
              : dnsAction?.type === 'hostsFirst'
                ? dnsServerNameById.get(dnsAction.hostsServerId)
                : undefined;
        return {
          rule,
          enabled: (plane === 'dns' ? dnsEffect?.enabled : route?.enabled) !== false,
          targetNodeName: targetServerId ? serverNameById.get(targetServerId) : undefined,
          dnsActionName,
          targetMissing: !!targetServerId && !serverNameById.has(targetServerId),
          stagedOnly: (plane === 'dns' ? stagedOnlyDns : stagedOnlyRoute).has(rule.id),
          hasMissingResource: (plane === 'dns' ? missingResDns : missingResRoute).has(rule.id),
          hasMeshOverlap: plane === 'route' && meshRuleHints.ids.has(rule.id),
          meshOverlapUnknown: plane === 'route' && meshRuleHints.unknown,
          meshOverlapPrevious: plane === 'route' && meshRoute.previous,
          routeInactive: plane === 'route' && modeInactive && route !== null,
          networkProfileBadge: ruleProfileBadge(rule, badgeProfiles, resolvedProbes),
          deleteConfirming: armed === `${RULE_DEL_PREFIX}${rule.id}`,
          sheetOpen: sheetRuleId === rule.id,
          onOpenSheet: (r) => setSheetRuleId(r ? r.id : null),
          onToggle: (r) => void handleToggle(plane, r),
          onMove: (r, to) => handleMove(plane, r, to),
          onReorder: (r, toIndex) => handleReorder(plane, r, toIndex),
          onDuplicate: (r) => void handleDuplicate(plane, r),
          onDelete: (r) => requestDeleteRule(plane, r),
          /* 编辑开表单宿主那一层（`forms/RuleFormPanel`）。表单读 **effective** 规则、
             提交走共享的 `submitRule` ⇒ 暂存分流与校验只有一份实现。 */
          onEdit: (r) => openMobileForm({ kind: 'rule', ruleId: r.id, initialPlane: plane }),
        };
      }),
    [
      serverNameById,
      dnsServerNameById,
      dnsGroupNameById,
      stagedOnlyRoute,
      stagedOnlyDns,
      missingResRoute,
      missingResDns,
      meshRuleHints,
      modeInactive,
      badgeProfiles,
      resolvedProbes,
      armed,
      sheetRuleId,
      handleToggle,
      handleMove,
      handleReorder,
      handleDuplicate,
      requestDeleteRule,
      tr,
    ],
  );

  const trafficRows = useMemo(() => bindRows('route', routeOrdered), [bindRows, routeOrdered]);
  const dnsRows = useMemo(() => bindRows('dns', dnsOrdered), [bindRows, dnsOrdered]);

  /* ═══════════ 渲染 ═══════════ */

  const segments = [
    { id: 'traffic' as const, label: tr('mobileRules.seg.traffic'), count: trafficRows.length },
    { id: 'dns' as const, label: tr('mobileRules.seg.dns'), count: dnsRows.length },
    { id: 'apps' as const, label: tr('mobileRules.seg.apps'), count: appSummary.total },
    { id: 'resources' as const, label: tr('mobileRules.seg.resources'), count: resItems.length },
  ];

  if (page === 'dns-servers' || page === 'dns-groups') {
    const isServers = page === 'dns-servers';
    return (
      <section className="mr-screen" aria-label={tr('mobileNav.rules')}>
        <div className="mr-top">
          <div className="mr-head">
            <button
              type="button"
              className="mr-btn ghost icon"
              aria-label={tr('mobileRules.back')}
              onClick={() => setPage(null)}
            >
              <BackIcon />
            </button>
            <div className="mr-head-tx">
              <h1 className="mr-title">
                {tr(isServers ? 'rules.dnsWorkspace.serversTab' : 'rules.dnsWorkspace.groupsTab')}
              </h1>
            </div>
            <div className="mr-acts">
              {/* 新建。桌面是页头那颗 `+` 按 `dnsView` 分派（`RulesScreen.tsx:538-547`）；
                  移动端把服务器 / 分组做成了二级页 ⇒ 分派落在「你现在在哪一页」上，语义同。 */}
              <button
                type="button"
                className="mr-btn flow icon"
                aria-label={tr(isServers ? 'settings.dns.addServer' : 'settings.dns.addGroup')}
                onClick={() =>
                  openMobileForm(isServers ? { kind: 'dns-server' } : { kind: 'dns-group' })
                }
              >
                <AddIcon />
              </button>
            </div>
          </div>
        </div>
        <DnsResourcePage
          t={tr}
          rows={isServers ? dnsServerRows : dnsGroupRows}
          emptyText={tr(
            isServers ? 'mobileRules.dnsServersEmpty' : 'rules.dnsWorkspace.groupsEmpty',
          )}
          onToggle={isServers ? toggleDnsServer : toggleDnsGroup}
          onEdit={(id) =>
            openMobileForm(isServers ? { kind: 'dns-server', serverId: id } : { kind: 'dns-group', groupId: id })
          }
          onDelete={isServers ? requestDeleteDnsServer : requestDeleteDnsGroup}
          deleteConfirmingId={
            armed?.startsWith(isServers ? DNS_SRV_DEL_PREFIX : DNS_GRP_DEL_PREFIX) === true
              ? armed.slice((isServers ? DNS_SRV_DEL_PREFIX : DNS_GRP_DEL_PREFIX).length)
              : null
          }
          errorOf={errorOf}
        />
      </section>
    );
  }

  if (page === 'network-profiles') {
    return (
      <section className="mr-screen" aria-label={tr('mobileNav.rules')}>
        <div className="mr-top">
          <div className="mr-head">
            <button
              type="button"
              className="mr-btn ghost icon"
              aria-label={tr('mobileRules.back')}
              onClick={() => setPage(null)}
            >
              <BackIcon />
            </button>
            <div className="mr-head-tx">
              <h1 className="mr-title">{tr('rules.networkProfile.title')}</h1>
            </div>
            <div className="mr-acts">
              {/* 新建场景（桌面列表弹窗脚上那颗「新建场景」）。编辑表单是表单宿主的一层。 */}
              <button
                type="button"
                className="mr-btn flow icon"
                aria-label={tr('rules.networkProfile.add')}
                onClick={() => openMobileForm({ kind: 'network-profile' })}
              >
                <AddIcon />
              </button>
            </div>
          </div>
        </div>
        <NetworkProfilesPage
          t={tr}
          loadState={dnsConfig != null ? 'ready' : configError != null ? 'error' : configLoading ? 'loading' : 'error'}
          rows={networkProfileRows}
          onRetry={() => void reloadConfig()}
          onToggle={toggleNetworkProfile}
          onEdit={(id) => openMobileForm({ kind: 'network-profile', profileId: id })}
          onDelete={requestDeleteNetworkProfile}
          errorOf={errorOf}
        />
      </section>
    );
  }

  return (
    <section className="mr-screen" aria-label={tr('mobileNav.rules')}>
      {/* 固定：屏头 + 吸在它下面的四路分段条（分段条是范围指示器，滚走了下面就读不出属于哪一段）。 */}
      <div className="mr-top">
        <div className="mr-head">
          <div className="mr-head-tx">
            <h1 className="mr-title">{tr('mobileNav.rules')}</h1>
          </div>
          <div className="mr-acts">
            {/* 应用分流总开关：桌面也在屏头动作里，位置不变。 */}
            {segment === 'apps' && (
              <Switch
                id="mr-app-master"
                checked={appMasterEnabled}
                onChange={() => void toggleAppMaster()}
                label={tr('appPolicy.masterTip')}
              />
            )}
            {/*
              DNS 分段的溢出：服务器 / 分组两个二级页（IA §1.2「规则 → DNS → **header overflow**」，
              内容序那节 §2.1 又说了一遍）。

              为什么是一颗 `⋯` 而不是两颗文字按钮：§3.1 #10 给屏头定了上限 ——
              「Desktop `.acts` carries up to three buttons; **mobile caps at two plus an overflow
              that opens `action-sheet`**」。摆两颗文字按钮 + 那颗「添加规则」= 三颗，超限；
              而且 390 宽的屏上，两颗中文文字按钮与标题「规则」挤在一行，读起来像是和标题并列的
              二级导航，桌面把三个视图收在同一个分段条里的层级关系就丢了。
            */}
            {segment === 'dns' && (
              <button
                type="button"
                className="mr-btn ghost icon"
                aria-haspopup="dialog"
                aria-expanded={headSheet}
                aria-label={tr('mobileRules.more')}
                onClick={() => setHeadSheet(true)}
              >
                <MoreIcon />
              </button>
            )}
            {/* 流量段的溢出：只有「网络场景」一项。与 DNS 段那颗是同一个面板状态、进同一页 ——
                桌面把入口放在两个平面共用的页头（spec §6.1），这里同理不分平面。 */}
            {segment === 'traffic' && (
              <button
                type="button"
                className="mr-btn ghost icon"
                aria-haspopup="dialog"
                aria-expanded={headSheet}
                aria-label={tr('mobileRules.more')}
                onClick={() => setHeadSheet(true)}
              >
                <MoreIcon />
              </button>
            )}
            {/* 添加规则。按当前分段决定这条规则落在哪个平面 —— 两个平面**不共存**
                （`rule-submit.ts` 从 `initialPlane` 派生 route/dns，刻意不收一个布尔以免自相矛盾）。 */}
            {(segment === 'traffic' || segment === 'dns') && (
              <button
                type="button"
                className="mr-btn flow icon"
                aria-label={tr('rules.add')}
                onClick={() =>
                  openMobileForm({ kind: 'rule', initialPlane: segment === 'dns' ? 'dns' : 'route' })
                }
              >
                <AddIcon />
              </button>
            )}
          </div>
        </div>
        <SegmentedTabs
          items={segments}
          active={segment}
          onSelect={(id) => {
            setSegment(id);
            setSheetRuleId(null);
            setSheetResId(null);
          }}
          label={tr('mobileNav.rules')}
        />
      </div>

      {segment === 'traffic' && (
        <TrafficSegment
          t={tr}
          modeInactive={modeInactive}
          regionRouting={effectiveRegionRouting(config ?? {})}
          onRegionChange={(next) => void handleRegionChange(next)}
          isSmartMode={isSmartMode}
          onBackToSmart={() => void handleBackToSmart()}
          rows={trafficRows}
          meshInfo={servers.some(isMeshNode) ? <MobileInfo
            title={tr('mobileMeshRouteEvidence.title')}
            summary={tr(meshRouteSummaryKey(meshRoute.report, undefined, meshRoute.previous, meshRoute.legacy,
              meshRuleHints.contextMismatch))}
            details={<MobileMeshRouteEvidence report={meshRoute.report} previous={meshRoute.previous}
              legacy={meshRoute.legacy} contextMismatch={meshRuleHints.contextMismatch}
              nameOf={(id) => meshRuleHints.contextMismatch ? id : serverNameById.get(id) ?? id} />}
          /> : null}
          errorOf={errorOf}
        />
      )}

      {segment === 'dns' && (
        <DnsSegment
          t={tr}
          rows={dnsRows}
          defaultActionText={dnsDefaultActionText}
          defaultFallbackText={dnsDefaultFallbackText}
          systemPaneAbsentReason={tr('mobileRules.dnsSystemPaneAbsent')}
          errorOf={errorOf}
        />
      )}

      {segment === 'apps' && (
        <AppsSegment
          t={tr}
          summary={appSummary}
          presetsPending={presetsLoading || (!presetsLoaded && !presetsFailed)}
          presetsFailed={presetsFailed}
          masterEnabled={appMasterEnabled}
          isSmartMode={isSmartMode}
          onBackToSmart={() => void handleBackToSmart()}
          categories={appCategories}
          category={appCategory}
          onCategoryChange={setAppCategory}
          search={appSearch}
          onSearchChange={setAppSearch}
          groups={appGroups}
          platformNote={tr('mobileRules.appRoutingMatchNote')}
          view={appView}
          onViewChange={setAppView}
          masterError={errorOf('appRoutingEnabled')}
          categorySheetOpen={appCategorySheet}
          onCategorySheetOpen={() => setAppCategorySheet(true)}
          onCategorySheetClose={() => setAppCategorySheet(false)}
          policySheetAppId={appPolicySheetId}
          onPolicySheetOpen={setAppPolicySheetId}
          onPolicySheetClose={() => setAppPolicySheetId(null)}
          policyGroups={appPolicyGroups}
          policyOpenGroups={appPolicyOpenGroups}
          onPolicyPick={(appId, pick, targetServerId) =>
            void handleAppPolicyPick(appId, pick, targetServerId)
          }
          onAddCustom={() => openMobileForm({ kind: 'app-add' })}
          onRemoveCustom={requestRemoveCustomApp}
          removeConfirmingId={
            armed?.startsWith(APP_REMOVE_PREFIX) === true
              ? armed.slice(APP_REMOVE_PREFIX.length)
              : null
          }
          errorOf={errorOf}
        />
      )}

      {segment === 'resources' && (
        <ResourcesSegment
          t={tr}
          source={resSource}
          onSourceChange={setResSource}
          loading={resLoading}
          error={resError}
          groups={resourceGroups}
          updatingAll={resUpdatingAll || resUpdatingIds.size > 0}
          updatingIds={resUpdatingIds}
          onUpdateAll={() => void handleUpdateAll()}
          onResetBuiltin={handleResetBuiltin}
          resetConfirming={armed === GEO_RESET_KEY}
          errorOf={errorOf}
          onUpdateOne={(id) => void handleUpdateOne(id)}
          onCancel={(id) => void handleCancelRes(id)}
          onDelete={handleDeleteRes}
          deleteConfirmingId={
            armed?.startsWith(RES_DEL_PREFIX) === true ? armed.slice(RES_DEL_PREFIX.length) : null
          }
          sheetId={sheetResId}
          onOpenSheet={setSheetResId}
          onCatalog={() => openMobileForm({ kind: 'res-catalog' })}
          onUrlDownload={() => openMobileForm({ kind: 'res-url' })}
        />
      )}

      {/* 屏头溢出的底部面板：DNS 段三行（服务器 / 分组 / 网络场景），流量段一行（网络场景）。 */}
      {headSheet && (
        <ActionSheet
          title={tr(segment === 'dns' ? 'rules.dnsWorkspace.ariaLabel' : 'mobileRules.more')}
          actions={[
            ...(segment === 'dns'
              ? [
                  {
                    id: 'dns-servers',
                    label: tr('rules.dnsWorkspace.serversTab'),
                    onSelect: () => {
                      setHeadSheet(false);
                      setPage('dns-servers');
                    },
                  },
                  {
                    id: 'dns-groups',
                    label: tr('rules.dnsWorkspace.groupsTab'),
                    onSelect: () => {
                      setHeadSheet(false);
                      setPage('dns-groups');
                    },
                  },
                ]
              : []),
            {
              id: 'network-profiles',
              label: tr('rules.networkProfile.entry'),
              icon: <NetworkProfileIcon />,
              onSelect: () => {
                setHeadSheet(false);
                setPage('network-profiles');
              },
            },
          ]}
          onClose={() => setHeadSheet(false)}
          closeLabel={tr('common.close')}
        />
      )}
    </section>
  );
}
