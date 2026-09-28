import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端**规则表单**（新建 / 编辑；流量面与 DNS 面共用一张表，由 `initialPlane` 分叉）。
 *
 * # 复用的是判据，重写的只有呈现
 *
 * 桌面 `RuleDialog.tsx` 那 516 行里，真正是判据的部分早就住在纯 `.ts` 里，本面板原样吃它们：
 *  · 草稿编辑 —— `rule-cond.ts` 的 `setCondTypeAt` / `toggleCondValueAt` / `splitVals` /
 *    `invalidCondValues` / `computeTestMatch`，以及候选面的 `matchRuleValueOptions` /
 *    `offPoolSelectedOptions`；
 *  · 候选池 —— `use-rule-pools.ts` 的 `useRulePools`（惰性取数 + 排序快照，零 DOM 依赖）；
 *  · 类型物料 —— `domain/rules.ts` 的 `RULE_TYPES` / `RULE_TYPE_IDS` / `RULE_CATEGORY_ORDER` /
 *    `findAddableRuleType` / `isRuleTypePlatformSupported` / `isRuleTypeDnsEffectSupported`；
 *  · 效果两格 —— `rule-effect-state.ts` 的 `ruleRouteTargetChoice` / `useRuleDnsEffect` /
 *    `dnsEffectLinkage`（本批从两个桌面 `.tsx` 下沉，判据一行未改）；
 *  · 提交与删除 —— `rule-submit.ts#submitRule` 与 `lib/use-rule-delete.ts#useRuleDelete`。
 *    **提交腿必须复用**：暂存分流、前端自铸 id、`RULE_INVALID` 的分档处置都在那里，
 *    照抄一份等于让「改一条规则会不会落盘」在两端各有一份实现。
 *
 * 🔴 **本文件不许出现任何 `RuleType` 字面量**（逐字沿用桌面 `RuleDialog.tsx` 的硬约束）：
 * 一切逐类型差异从描述符结构字段读（`RULE_TYPES[t].source` 的 `kind` / `pool` / `addressing` /
 * `groupBy` / `scale` / `allowFreeInput`）。加第 16 个规则类型时这里一行都不用改。
 *
 * # 三处刻意与桌面不同的形态（不是排版偏好）
 *
 * 1. **页签 → 折叠段**（与本仓另外四张表同一条）：一张有条件区 + 效果区 + 测试区的表在手机上
 *    本来就要纵向滚，切成页签等于让校验失败时先跳页签再滚动。故用 [`FormGroup`]，
 *    「基本 / 条件 / 执行效果」默认展开，只有辅助性的匹配测试默认收起。
 * 2. **下拉 → `select-sheet`**（IA §3.3）。类型选择器与目标出站都走
 *    `SelectSheetPanel`，后者带**可折叠分组组头** —— 桌面把几十上百个节点按订阅折进组里，
 *    那一维在 390 宽的屏上比桌面更要紧。分组与默认展开集复用
 *    `domain/server-grouping` 的 `groupServersBySubscription` / `defaultOpenGroupIds`
 *    （三处选择器共用的单一判据），只是不画国旗 —— 移动端没有 `FlagImg` 那条渲染链。
 * 3. **条件值的候选面常驻、手填折叠**：桌面靠 hover 与宽屏并排摆两者，触屏上并排会把两边都压到
 *    够不着；故候选在上、手填收进 `Fold`（有候选时默认折叠，同桌面）。
 *
 * # 写失败
 *
 * 走表单宿主那条机制（`FormSheet` 的 `notice` 槽 + `setNotice`，登记在
 * `mobile/write-failure-visibility.test.ts` 的 `FEEDBACK['表单宿主']`）。
 * 提交腿本身的失败由 `submitRule` 落 toast（移动端 toast 宿主 2026-09-06 已在，
 * `mobile/MobileToaster.tsx`），**且失败时它不调 `close`** ⇒ 表单留在原地、草稿不丢。
 * 删除腿的错误由本面板接住并落 notice —— `useRuleDelete` 明确把错误抛给调用点。
 */

import { useMemo, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { Rule, RuleType } from '@/contracts/types';
import {
  DEFAULT_RULE_TYPE,
  RULE_CATEGORY_ORDER,
  RULE_TYPES,
  RULE_TYPE_IDS,
  findAddableRuleType,
  isRuleTypeDnsEffectSupported,
  isRuleTypePlatformSupported,
  ruleCategoryLabelKey,
  ruleConditions,
  ruleDnsEffect,
  ruleRouteEffect,
  ruleTypeHintKey,
  ruleTypeNameKey,
  ruleTypePlaceholderKey,
  type RulePreset,
} from '@/domain/rules';
import { defaultOpenGroupIds, groupServersBySubscription } from '@/domain/server-grouping';
import type { CselGroup } from '@/components/dialogs/csel-logic';
import {
  computeTestMatch,
  matchRuleValueOptions,
  offPoolSelectedOptions,
  selectedValueSet,
  setCondTypeAt,
  splitVals,
  toggleCondValueAt,
  type Cond,
} from '@/components/dialogs/rule-cond';
import { EMPTY_SNAP, useRulePools } from '@/components/dialogs/use-rule-pools';
import { ruleSetPickState } from '@/components/dialogs/rule-set-pick';
import {
  dnsEffectLinkage,
  ruleRouteTargetChoice,
  useRuleDnsEffect,
} from '@/components/dialogs/rule-effect-state';
import { submitRule } from '@/components/dialogs/rule-submit';
import { NEW_PROFILE_CHOICE, networkProfileOptions } from '@/components/dialogs/network-profile-options';
import { BUILTIN_NETENV_DHCP_ID, probeReasonKey } from '@/domain/network-profile';
import { mobileReasonKey } from '../screens/rules/network-profile-copy';
import { useRuleDelete } from '@/lib/use-rule-delete';
import { revealElement, useRevealAfterCommit } from '@/components/reveal';
import { useAppStore, useEffectiveConfig, useEffectiveRules, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import {
  CheckIcon,
  Choice,
  Fold,
  SearchField,
  SelectSheetPanel,
  SelectSheetTrigger,
} from '../screens/rules/Primitives';
/* 本面板复用规则屏那套 `select-sheet` / `fold` / `search` 原语的**外观**。
   CSS 集合恰等那道门（`mobile-entry.test.ts` ③）数的是模块图上的 CSS 文件集合，
   而这一份早已在集合里（规则屏自己 import 它）⇒ 再 import 一次不改变集合，只保证
   「表单从任何入口打开时这套类都在」，不靠「规则屏一定先渲染过」这种时序巧合。 */
import '../screens/rules/rules-screen.css';
import { FormGroup } from './FormGroup';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

/** 名称、匹配条件和执行效果共同定义一条规则；测试匹配是可选辅助操作。 */
type Section = 'basic' | 'cond' | 'effect' | 'test';
const REQUIRED_SECTIONS: readonly Section[] = ['basic', 'cond', 'effect'];

/** 三个快速策略在目标出站里的值编码（`rule-submit.ts` 的反解协议，不是本地约定）。 */
const TARGET_POLICY_VALUES = ['proxy', 'direct', 'block'] as const;

export function RuleFormPanel({
  instanceId,
  ruleId,
  preset,
  initialPlane,
}: {
  instanceId: string;
  ruleId?: string;
  preset?: RulePreset;
  initialPlane?: 'route' | 'dns';
}): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);

  const plane: 'route' | 'dns' = initialPlane ?? 'route';
  /* 读 **effective** 不读盘：暂存过的规则打开时要显示暂存后的值（同桌面 `RuleDialog.tsx:501`）。 */
  const planeRules = useEffectiveRules(plane);
  const base = ruleId === undefined ? undefined : planeRules.find((rule) => rule.id === ruleId);
  const isEdit = base !== undefined;

  const config = useEffectiveConfig();
  const servers = useEffectiveServers();
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const deleteRule = useRuleDelete(plane);

  const routeEnabled = plane === 'route';
  const dnsEnabled = plane === 'dns';

  const baseRouteEffect = base ? ruleRouteEffect(base) : null;
  const baseDnsEffect = base ? ruleDnsEffect(base) : null;

  /* ── 草稿 ─────────────────────────────────────────────────────────────── */

  const [conds, setConds] = useState<Cond[]>(() => {
    if (base) {
      const existing = ruleConditions(base).map((c) => ({ t: c.type, v: c.values.join(', ') }));
      return existing.length > 0 ? existing : [{ t: DEFAULT_RULE_TYPE, v: '' }];
    }
    return [{ t: preset?.type ?? DEFAULT_RULE_TYPE, v: preset?.value ?? '' }];
  });
  /** 多条件的合成方式。**默认 `or`** —— 契约、Rust 侧与两端详情卡三处同一个缺省。 */
  const [logic, setLogic] = useState<'and' | 'or'>(base?.combineMode === 'and' ? 'and' : 'or');
  const [name, setName] = useState(base?.remarks ?? preset?.value ?? '');
  /** 生效网络：'' = 任何网络（`networkProfileId` 缺省），同桌面 `RuleDialog`。 */
  const [networkProfileId, setNetworkProfileId] = useState(base?.networkProfileId ?? '');
  const [test, setTest] = useState('');
  const [errName, setErrName] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const [sections, setSections] = useState<ReadonlySet<Section>>(() => new Set(REQUIRED_SECTIONS));
  const scheduleReveal = useRevealAfterCommit();
  /** 打开着的选择器面板（`type:<下标>` / `target` / `network-profile` / `dns-action` / `dns-fallback`）。 */
  const [sheet, setSheet] = useState<string | null>(null);

  const touch = (): void => setDirty(true);
  const toggleSection = (id: Section): void =>
    setSections((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const revealValidationSection = (section: 'basic' | 'cond'): void => {
    setSections((previous) => new Set([...previous, section]));
    scheduleReveal(() => {
      const key = `${instanceId}:rule:${section}`;
      const group = [...document.querySelectorAll<HTMLElement>('[data-form-group]')]
        .find((element) => element.dataset.formGroup === key);
      if (!group) return;
      const body = group.closest<HTMLElement>('.m-form-body');
      const header = group.querySelector<HTMLElement>('.m-form-group-h');
      if (body && header && header.getBoundingClientRect().top < body.getBoundingClientRect().top) {
        body.scrollBy({ top: header.getBoundingClientRect().top - body.getBoundingClientRect().top, behavior: 'auto' });
      } else {
        revealElement(group);
      }
    });
  };

  /* ── 候选池（惰性、按类型键存，排序走快照）───────────────────────────── */

  const pools = useRulePools(conds, t);

  /**
   * 切类型：值一律清空（`setCondTypeAt`），**并重建该类型的排序快照**。
   *
   * 🔴 `setPoolSnap` 在桌面全文件只许出现一处、且必须在这条腿里（`rule-cond.test.ts` 的门
   * 按源码字符串钉着）。这里同形：快照只在「打开」与「切类型」两个时刻重建 ——
   * 跟着实时勾选态走的话，勾一个候选它就跳到顶部，列表在手指底下自己重排。
   */
  const setCondType = (index: number, next: RuleType): void => {
    setConds((prev) => setCondTypeAt(prev, index, next));
    pools.setPoolSnap((prev) => ({ ...prev, [next]: EMPTY_SNAP }));
    touch();
  };

  const setCondValue = (index: number, value: string): void => {
    setConds((prev) => prev.map((c, i) => (i === index ? { ...c, v: value } : c)));
    touch();
  };

  const toggleCondValue = (index: number, value: string): void => {
    setConds((prev) => toggleCondValueAt(prev, index, value));
    touch();
  };

  const usedTypes = useMemo(() => new Set(conds.map((c) => c.t)), [conds]);
  /**
   * 平台判据：device 类（MAC / 主机名）只有 Linux / macOS 的内核支持。
   *
   * 移动端拿不到 `<html data-os>`（那是桌面外壳写的），传 `undefined` ⇒ `isRuleTypePlatformSupported`
   * 走 **fail-closed** 分支（不支持）。这与「猜一个 android 字符串」的差别是：猜错时会放行一条
   * 在这台设备上永远不命中的规则，而它在界面上看不出任何异常。
   */
  const nodePlatform = undefined;
  const addableType = findAddableRuleType(usedTypes, nodePlatform);

  const addCond = (): void => {
    if (addableType === undefined) return;
    setConds((prev) => [...prev, { t: addableType, v: '' }]);
    touch();
  };
  const removeCond = (index: number): void => {
    setConds((prev) => (prev.length > 1 ? prev.filter((_, i) => i !== index) : prev));
    touch();
  };

  /**
   * 类型选择器的 5 组。**不带 `id` ⇒ 不可折叠恒展开**，与桌面 `RuleDialog.tsx#typeGroups` 同：
   * 只有 15 项，折起来只会多一次点击。禁用条件三条逐字同源（已被本规则别的条件占用 /
   * 平台不支持 / DNS 面不支持），`id !== currentType` 那条豁免保证跨平台打开的存量规则
   * 仍看得见自己的类型。
   */
  const typeGroups = (currentType: RuleType): CselGroup[] =>
    RULE_CATEGORY_ORDER.map((cat) => ({
      label: tr(ruleCategoryLabelKey(cat)),
      options: RULE_TYPE_IDS.filter((id) => RULE_TYPES[id].category === cat).map((id) => ({
        value: id,
        label: tr(ruleTypeNameKey(id)),
        disabled:
          id !== currentType
          && (usedTypes.has(id)
            || !isRuleTypePlatformSupported(id, nodePlatform)
            || (dnsEnabled && !isRuleTypeDnsEffectSupported(id))),
      })),
    }));

  /* ── 效果：目标出站（route 面）────────────────────────────────────────── */

  const [target, setTarget] = useState<string>(() => ruleRouteTargetChoice(baseRouteEffect));
  const nodeGroups = useMemo(
    () => groupServersBySubscription(servers, config?.subscriptions ?? []),
    [servers, config?.subscriptions],
  );
  const targetNodeId = target.startsWith('node:') ? target.slice(5) : undefined;
  /**
   * 目标出站候选：三档策略（不可折叠，主路径不许被折进去）+ 按订阅/分组折叠的节点组。
   * 与桌面同一份分组判据，差别只有「不画国旗」—— 移动端没有 `FlagImg` 那条渲染链，
   * 而国旗是装饰性的第二判据（名称派生），缺它不改变这份列表回答的问题。
   */
  const targetGroups: CselGroup[] = useMemo(
    () => [
      {
        label: tr('appPolicy.policy'),
        options: [
          { value: TARGET_POLICY_VALUES[0], label: tr('rules.targetDefaultProxy') },
          { value: TARGET_POLICY_VALUES[1], label: tr('rules.targetDirect') },
          /* 动作标签轴：「阻断」恒 `--err` 且常驻，走 `danger` 通道而不是在本页刷一层红。 */
          { value: TARGET_POLICY_VALUES[2], label: tr('rules.targetBlock'), danger: true },
        ],
      },
      ...nodeGroups.map((group) => ({
        id: group.id,
        /* 自建 / 组网组的 `name` 是占位符，按 isManual/isMesh 本地化（ServerGroup 契约的明文要求）。 */
        label: group.isManual
          ? tr('nodes.tab.manual')
          : group.isMesh
            ? tr('nodes.tab.mesh')
            : group.name,
        options: group.servers.map((server) => ({
          value: `node:${server.id}`,
          label: `${tr('rules.targetProxyTo')} ${server.name}`,
        })),
      })),
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [nodeGroups, t],
  );
  const targetOpenGroups = useMemo(
    () => defaultOpenGroupIds(nodeGroups, targetNodeId),
    [nodeGroups, targetNodeId],
  );
  const targetLabel = useMemo(() => {
    for (const group of targetGroups) {
      const hit = group.options.find((option) => option.value === target);
      if (hit) return hit.label;
    }
    return target;
  }, [targetGroups, target]);

  /* ── 效果：DNS（dns 面）──────────────────────────────────────────────── */

  const dns = useRuleDnsEffect(
    baseDnsEffect,
    initialPlane,
    config?.dnsServers ?? [],
    config?.dnsServerGroups ?? [],
    servers,
    config?.dnsDefaults,
    t,
    /* 内置 DHCP 解析器不可用原因换成手机上的那一句（原因码不变，见 `network-profile-copy`）。 */
    mobileReasonKey,
  );
  const labelOfGroups = (groups: readonly CselGroup[], value: string): string => {
    for (const group of groups) {
      const hit = group.options.find((option) => option.value === value);
      if (hit) return hit.label;
    }
    return value;
  };

  /* ── 生效网络（spec §6.1）──────────────────────────────────────────── */

  /* 候选口径与桌面同一个函数（任何网络 / 各场景，停用的标注 / 已删除补一项 / 新建场景…）；
     场景表取**展示面**：暂存中新建的场景也要立刻可选（同桌面 `RuleDialog`）。 */
  const profileOptions = useMemo(
    () =>
      networkProfileOptions(config?.networkProfiles ?? [], networkProfileId, t).map((option) => ({
        id: option.value,
        label: option.label,
        description: option.description,
        disabled: option.disabled,
      })),
    [config?.networkProfiles, networkProfileId, t],
  );
  const profileMissing = profileOptions.some((option) => option.disabled === true);

  /* ── 测试匹配 ───────────────────────────────────────────────────────── */

  const testResult = useMemo(() => computeTestMatch(conds, logic, test), [conds, logic, test]);

  /* ── 提交 / 删除 / 关闭 ─────────────────────────────────────────────── */

  const requestClose = (): void => {
    if (submitting) return;
    if (!dirty) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: tr('rules.discardTitle'),
        message: tr('rules.discardMsg'),
        confirmLabel: tr('rules.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  const submit = (): void => {
    if (submitting) return;
    setNotice(undefined);
    void submitRule({
      t,
      conds,
      name,
      setErrName,
      onValidationError: revealValidationSection,
      networkProfileId,
      /* 新建**带场景**的规则插到最前（spec §3.4-4）：序列由共享的 `orderWithNewRuleFirst` 算，
         现序口径同规则列表（本平面 effective 规则 + 持久化顺序）。 */
      planeOrder: {
        ruleIds: planeRules.map((rule) => rule.id),
        persistedOrder: (plane === 'dns' ? config?.dnsRuleOrder : config?.routeRuleOrder) ?? [],
      },
      logic,
      target,
      dnsAction: dns.dnsAction,
      dnsFallbackAction: dns.dnsFallbackAction,
      dnsResolver: dns.dnsResolver,
      dnsAnswerMode: dns.dnsAnswerMode,
      dnsPredefinedRcode: dns.dnsPredefinedRcode,
      dnsPredefinedAnswer: dns.dnsPredefinedAnswer,
      dnsPredefinedNs: dns.dnsPredefinedNs,
      dnsPredefinedExtra: dns.dnsPredefinedExtra,
      isEdit,
      base,
      initialPlane: plane,
      stagingEnabled,
      stage,
      close: () => closeInstance(instanceId),
      loadConfig,
      setSubmitting,
    });
  };

  const requestDelete = (): void => {
    if (base === undefined) return;
    const target1: Rule = base;
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: tr('rules.deleteRule'),
        /* 二次确认那句话用桌面同一个键（`rules.deleteConfirmAgain`），不新起一条：
           说的是同一件事，新键还要五语各补一遍、且两端措辞会各自漂。 */
        message: tr('rules.deleteConfirmAgain'),
        confirmLabel: tr('common.delete'),
        danger: true,
        onConfirm: () => {
          void (async () => {
            try {
              await deleteRule(target1);
              closeInstance(confirmId);
              closeInstance(instanceId);
            } catch (err) {
              console.error('[mobile-rule-form] delete failed:', err);
              closeInstance(confirmId);
              /* 表单可能已经被用户关掉（异步 continuation）⇒ 先确认宿主还在，再落回显。 */
              if (hasInstance(instanceId)) setNotice({ tone: 'err', text: tr('rules.deleteFail') });
            }
          })();
        },
      },
    });
  };

  /* ── 渲染 ───────────────────────────────────────────────────────────── */

  return (
    <FormSheet
      title={isEdit ? tr('rules.editTitle') : tr('rules.newTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      submitLabel={isEdit ? tr('common.save') : tr('rules.add')}
      submitDisabled={submitting}
      onSubmit={submit}
      notice={notice}
    >
      <FormGroup
        title={tr('rules.newTitle')}
        groupId={`${instanceId}:rule:basic`}
        open={sections.has('basic')}
        onToggle={() => toggleSection('basic')}
      >
        <div className="m-form-row">
          <label className="m-form-label" htmlFor="mrf-name">
            {tr('rules.name')}
            <span className="m-form-req" aria-hidden>
              *
            </span>
          </label>
          <input
            id="mrf-name"
            className="m-form-input"
            value={name}
            placeholder={tr('rules.namePh')}
            onChange={(e) => {
              setName(e.target.value);
              setErrName(false);
              touch();
            }}
          />
          {errName && <p className="m-form-err">{tr('rules.errName')}</p>}
        </div>
        {/* 生效网络放「基本」段，和名称一起作为规则主属性；桌面挂在 InfoIcon 上的说明在这里常驻（§4.12）。 */}
        <div className="m-form-row">
          <span className="m-form-label">{tr('rules.networkProfile.ruleField')}</span>
          <p className="m-form-hint">{tr('rules.networkProfile.ruleFieldHint')}</p>
          <SelectSheetTrigger
            label={tr('rules.networkProfile.ruleField')}
            options={profileOptions}
            value={networkProfileId}
            open={sheet === 'network-profile'}
            onOpen={() => setSheet('network-profile')}
          />
          {profileMissing && <p className="m-form-err">{tr('rules.networkProfile.badgeMissingTip')}</p>}
        </div>
      </FormGroup>

      <FormGroup
        title={tr('rules.conditions')}
        groupId={`${instanceId}:rule:cond`}
        open={sections.has('cond')}
        onToggle={() => toggleSection('cond')}
      >
        {/* 多条件的合成方式。单条件时整块不渲染 —— 一条条件谈不上「全部满足」。 */}
        {conds.length > 1 && (
          <div className="m-form-row">
            <span className="m-form-label">{tr('rules.combineMode')}</span>
            <Choice
              label={tr('rules.combineMode')}
              active={logic}
              onSelect={(next) => {
                setLogic(next);
                touch();
              }}
              items={[
                { id: 'or' as const, label: tr('rules.combineOr') },
                { id: 'and' as const, label: tr('rules.combineAnd') },
              ]}
            />
          </div>
        )}

        {conds.map((cond, index) => (
          <CondEditor
            key={`${cond.t}-${index}`}
            t={tr}
            cond={cond}
            index={index}
            canRemove={conds.length > 1}
            typeGroups={typeGroups(cond.t)}
            sheet={sheet}
            setSheet={setSheet}
            pools={pools}
            onTypeChange={(next) => setCondType(index, next)}
            onValueChange={(value) => setCondValue(index, value)}
            onToggleValue={(value) => toggleCondValue(index, value)}
            onRemove={() => removeCond(index)}
          />
        ))}

        {/* 「添加条件」的显隐与取值**共用** `findAddableRuleType`：写成 `conds.length < 15`
            会在 device 类不可用的平台上留一颗按下去什么都不发生的按钮。 */}
        {addableType !== undefined && (
          <button type="button" className="m-form-btn" onClick={addCond}>
            {tr('rules.addCondition')}
          </button>
        )}
      </FormGroup>

      <FormGroup
        title={routeEnabled ? tr('rules.routeEffect') : tr('rules.dnsEffect')}
        groupId={`${instanceId}:rule:effect`}
        open={sections.has('effect')}
        onToggle={() => toggleSection('effect')}
      >
        {routeEnabled && (
          <div className="m-form-row">
            <span className="m-form-label">{tr('rules.target')}</span>
            <p className="m-form-hint">{tr('rules.routeEffectHint')}</p>
            <SelectSheetTrigger
              label={tr('rules.target')}
              options={[]}
              value={target}
              valueLabel={targetLabel}
              open={sheet === 'target'}
              onOpen={() => setSheet('target')}
            />
          </div>
        )}
        {dnsEnabled && (
          <>
            <div className="m-form-row">
              <span className="m-form-label">{tr('rules.dnsAction')}</span>
              <p className="m-form-hint">{tr('rules.dnsEffectHint')}</p>
              <SelectSheetTrigger
                label={tr('rules.dnsAction')}
                options={[]}
                value={dns.dnsAction}
                valueLabel={labelOfGroups(dns.dnsActionGroups, dns.dnsAction)}
                open={sheet === 'dns-action'}
                onOpen={() => setSheet('dns-action')}
              />
              {/* 已选中内置 DHCP 解析器、而它在本机不可用（Android 恒如此）：值照常回显，原因写在下面，
                  不悄悄清空（同桌面 `RuleDnsEffect`）。 */}
              {dns.dnsAction === `server:${BUILTIN_NETENV_DHCP_ID}` && dns.netenvStatus?.available === false && (
                <p className="m-form-err">
                  {tr('rules.networkProfile.probeUnavailable', {
                    reason: tr(mobileReasonKey(probeReasonKey(dns.netenvStatus.reason))),
                  })}
                </p>
              )}
            </div>
            {dns.dnsAction.startsWith('hosts:') && (
              <div className="m-form-row">
                <span className="m-form-label">{tr('rules.dnsHostsFallback')}</span>
                <SelectSheetTrigger
                  label={tr('rules.dnsHostsFallback')}
                  options={[]}
                  value={dns.dnsFallbackAction}
                  valueLabel={labelOfGroups(dns.dnsFallbackGroups, dns.dnsFallbackAction)}
                  open={sheet === 'dns-fallback'}
                  onOpen={() => setSheet('dns-fallback')}
                />
              </div>
            )}
            {dns.dnsAction === 'predefined' && (
              <>
                <div className="m-form-row">
                  <label className="m-form-label" htmlFor="mrf-rcode">
                    {tr('rules.dnsPredefinedRcode')}
                  </label>
                  {/* 六档固定枚举，用原生 select：Android 会把它渲染成系统全屏选择器，
                      那正是触屏该有的形态，且免费拿到无障碍与旋转适配（同 `MobileField`）。 */}
                  <MobileSelect
                    id="mrf-rcode"
                    className="m-form-select"
                    value={dns.dnsPredefinedRcode}
                    onChange={(e) => {
                      dns.setDnsPredefinedRcode(e.target.value);
                      touch();
                    }}
                  >
                    {['NOERROR', 'FORMERR', 'SERVFAIL', 'NXDOMAIN', 'NOTIMP', 'REFUSED'].map((code) => (
                      <option key={code} value={code}>
                        {code}
                      </option>
                    ))}
                  </MobileSelect>
                </div>
                {(
                  [
                    ['answer', tr('rules.dnsPredefinedAnswer'), dns.dnsPredefinedAnswer, dns.setDnsPredefinedAnswer],
                    ['ns', tr('rules.dnsPredefinedNs'), dns.dnsPredefinedNs, dns.setDnsPredefinedNs],
                    ['extra', tr('rules.dnsPredefinedExtra'), dns.dnsPredefinedExtra, dns.setDnsPredefinedExtra],
                  ] as const
                ).map(([field, label, value, setValue]) => (
                  <div key={field} className="m-form-row">
                    <label className="m-form-label" htmlFor={`mrf-${field}`}>
                      {label}
                    </label>
                    <textarea
                      id={`mrf-${field}`}
                      className="m-form-input m-form-area mono"
                      rows={2}
                      value={value}
                      placeholder={tr('rules.dnsPredefinedRecordsHint')}
                      onChange={(e) => {
                        setValue(e.target.value);
                        touch();
                      }}
                    />
                  </div>
                ))}
              </>
            )}
          </>
        )}
      </FormGroup>

      <FormGroup
        title={tr('rules.testMatch')}
        groupId={`${instanceId}:rule:test`}
        open={sections.has('test')}
        onToggle={() => toggleSection('test')}
      >
        <div className="m-form-row">
          <input
            className="m-form-input mono"
            value={test}
            placeholder={tr('rules.testPh')}
            onChange={(e) => setTest(e.target.value)}
          />
          {/* `'empty'` 态一个字都不印（同桌面）：还没输入就报「不匹配」是假信号。 */}
          {testResult !== 'empty' && (
            <p className="m-form-hint">
              {testResult === 'hit'
                ? tr('rules.testHit')
                : testResult === 'miss'
                  ? tr('rules.testMiss')
                  : tr('rules.testUntestable')}
            </p>
          )}
        </div>
      </FormGroup>

      {/* 删除只在编辑态在场（桌面同）。走表单栈上叠一层确认 —— 触屏没有 hover，
          原地二次确认的翻红那一下在拇指底下被手指自己挡住。 */}
      {isEdit && (
        <button type="button" className="m-form-btn danger" onClick={requestDelete}>
          {tr('rules.deleteRule')}
        </button>
      )}

      {/* 选择器面板一律渲染在表单体内、按 `sheet` 开合（单点持有：同屏两个面板同时开是误触面）。 */}
      <SelectSheetPanel
        label={tr('rules.networkProfile.ruleField')}
        /* 置灰那一项只有「场景已删除」（当前值指向一个不在了的场景）—— 点不动的理由写成可见文字。 */
        note={profileMissing ? tr('rules.networkProfile.badgeMissingTip') : undefined}
        options={profileOptions}
        value={networkProfileId}
        open={sheet === 'network-profile'}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          setSheet(null);
          if (next === NEW_PROFILE_CHOICE) {
            /* 建完直接选中：新场景表单叠在本表单之上，`onSaved` 回传新 id（同桌面）。 */
            open({
              kind: 'network-profile',
              onSaved: (id) => {
                setNetworkProfileId(id);
                touch();
              },
            });
            return;
          }
          setNetworkProfileId(next);
          touch();
        }}
        closeLabel={tr('common.close')}
      />
      <SelectSheetPanel
        label={tr('rules.target')}
        groups={targetGroups}
        value={target}
        open={sheet === 'target'}
        openGroupIds={targetOpenGroups}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          setTarget(next);
          setSheet(null);
          touch();
        }}
        closeLabel={tr('common.close')}
      />
      <SelectSheetPanel
        label={tr('rules.dnsAction')}
        groups={dns.dnsActionGroups}
        value={dns.dnsAction}
        open={sheet === 'dns-action'}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          dns.setDnsAction(next);
          /* 联动判据共用 `dnsEffectLinkage`，不在这里再写一遍条件表达式。 */
          const linked = dnsEffectLinkage(next);
          dns.setDnsAnswerMode(linked.answerMode);
          dns.setDnsResolver(linked.resolver);
          setSheet(null);
          touch();
        }}
        closeLabel={tr('common.close')}
      />
      <SelectSheetPanel
        label={tr('rules.dnsHostsFallback')}
        groups={dns.dnsFallbackGroups}
        value={dns.dnsFallbackAction}
        open={sheet === 'dns-fallback'}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          dns.setDnsFallbackAction(next);
          setSheet(null);
          touch();
        }}
        closeLabel={tr('common.close')}
      />
    </FormSheet>
  );
}

/**
 * 一条匹配条件的编辑器。
 *
 * 整段判定链逐行同桌面 `RuleCondRow.tsx:133-196`（那段是行内表达式，不是可 import 的函数）——
 * **注释里逐条注明它对应桌面哪一格**，因为这是本文件里唯一一处「照着写」而不是「调用」的地方。
 * 变量名刻意与桌面同名，便于对差。
 */
function CondEditor({
  t,
  cond,
  index,
  canRemove,
  typeGroups,
  sheet,
  setSheet,
  pools,
  onTypeChange,
  onValueChange,
  onToggleValue,
  onRemove,
}: {
  t: (key: string, vars?: Record<string, unknown>) => string;
  cond: Cond;
  index: number;
  canRemove: boolean;
  typeGroups: CselGroup[];
  sheet: string | null;
  setSheet: (next: string | null) => void;
  pools: ReturnType<typeof useRulePools>;
  onTypeChange: (next: RuleType) => void;
  onValueChange: (value: string) => void;
  onToggleValue: (value: string) => void;
  onRemove: () => void;
}): ReactElement {
  const desc = RULE_TYPES[cond.t];
  const src = desc.source;
  const isResRef = src.kind === 'pool' && src.addressing === 'res-id';
  const sheetId = `type:${index}`;

  const query = pools.poolQuery[cond.t] ?? '';
  const group = pools.poolGroup[cond.t] ?? 'all';
  const onlySel = pools.poolOnlySel[cond.t] === true;
  const phase = src.kind === 'pool' ? pools.poolPhase(src.pool) : null;
  const poolLoading = phase?.loading === true;
  const poolFailed = phase?.failed === true;

  /** `free` 类型没有候选面 ⇒ `all` 恒 `null`，右侧一个控件都不渲染（同桌面）。 */
  const all = src.kind === 'pool' ? (pools.poolOptions.get(cond.t) ?? []) : null;
  const matched = all ? matchRuleValueOptions(all, query) : null;
  const selected = selectedValueSet(cond.v);
  const groupsPresent =
    all && src.kind === 'pool' && src.groupBy !== null
      ? (['builtin', 'external'] as const).filter((g) => all.some((o) => o.group === g))
      : [];
  /* 加载中一律不判「池外已选」：那一刻池是空的，会把**每一个**已选值都标成「本地没有」。 */
  const offHint = poolFailed
    ? 'rules.candidatesFailed'
    : src.kind === 'pool' && src.pool === 'process'
      ? 'rules.candidateNotRunning'
      : 'rules.candidateNotLocal';
  const offPool = all && !poolLoading ? offPoolSelectedOptions(cond.v, all, t(offHint)) : null;
  const grouped =
    matched && groupsPresent.length > 1 && group !== 'all'
      ? matched.filter((o) => o.group === group)
      : matched;
  const poolShown = grouped
    ? onlySel
      ? grouped.filter((o) => selected.has(o.value.toLowerCase()))
      : grouped
    : null;
  /* 池外已选恒排最前、且不受分类切换影响 —— 它们是「你选了但这里没有」，藏起来等于让用户
     以为自己没选过。 */
  const shownOpts = poolShown
    ? [...(offPool ? matchRuleValueOptions(offPool, query) : []), ...poolShown]
    : null;
  const emptyText = pools.poolEmptyText(poolLoading, poolFailed, shownOpts?.length ?? 0);
  const rsMissing = isResRef ? pools.ruleSetMissing(cond.v) : [];
  const rsPick = isResRef ? ruleSetPickState(pools.resItems, query) : 'ok';

  const typeLabel = t(ruleTypeNameKey(cond.t));
  const valueCount = splitVals(cond.v).length;

  return (
    <section className="m-form-group">
      <div className="m-form-row">
        <span className="m-form-label">{t('rules.matchType')}</span>
        <SelectSheetTrigger
          label={t('rules.matchType')}
          options={[]}
          value={cond.t}
          valueLabel={typeLabel}
          open={sheet === sheetId}
          onOpen={() => setSheet(sheetId)}
        />
        <p className="m-form-hint">{t(ruleTypeHintKey(cond.t))}</p>
      </div>

      {shownOpts !== null && (
        <>
          <SearchField
            value={query}
            onChange={(next) => pools.setPoolQuery((prev) => ({ ...prev, [cond.t]: next }))}
            placeholder={t('common.search')}
            clearLabel={t('common.close')}
          />
          {/* 来源筛选只在**两组都有货**时出现：只有一组时它是一颗恒等于「全部」的控件。 */}
          {groupsPresent.length > 1 && (
            <Choice
              label={t('rules.candidateGroup')}
              active={group}
              onSelect={(next) => pools.setPoolGroup((prev) => ({ ...prev, [cond.t]: next }))}
              items={[
                { id: 'all' as const, label: t('common.all') },
                { id: 'builtin' as const, label: t('resCatalog.builtin') },
                { id: 'external' as const, label: t('resCatalog.external') },
              ]}
            />
          )}
          <button
            type="button"
            className={onlySel ? 'm-form-btn primary' : 'm-form-btn'}
            aria-pressed={onlySel}
            onClick={() => pools.setPoolOnlySel((prev) => ({ ...prev, [cond.t]: !onlySel }))}
          >
            {t('rules.candidateSelected', { n: selected.size })}
          </button>
          <div className="mr-sel-list" role="listbox" aria-label={typeLabel} aria-multiselectable>
            {shownOpts.length === 0 ? (
              <p className="m-form-hint">{emptyText}</p>
            ) : (
              shownOpts.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className="mr-sel-opt"
                  role="option"
                  aria-selected={selected.has(option.value.toLowerCase())}
                  onClick={() => onToggleValue(option.value)}
                >
                  <span className="mr-sel-opt-tx">
                    <span>{option.label}</span>
                    {option.hint != null && <span className="mr-sheet-item-sub">{option.hint}</span>}
                  </span>
                  <span className="mr-sel-ck" aria-hidden>
                    <CheckIcon />
                  </span>
                </button>
              ))
            )}
          </div>
          {isResRef && rsPick !== 'ok' && (
            <p className="m-form-hint">
              {pools.poolEmptyText(rsPick === 'loading', rsPick === 'failed', 0)}
            </p>
          )}
          {rsMissing.length > 0 && (
            <p className="m-form-err">{t('rules.ruleSetMissingHint', { n: rsMissing.length })}</p>
          )}
        </>
      )}

      {/* 手填腿**恒在**（五个池类型的 `allowFreeInput` 全为真，理由逐条写在描述符表里）。
          有候选面时收进折叠段，同桌面：它是补充通道，不是主路径。 */}
      {shownOpts !== null ? (
        <Fold title={t('rules.manualInput')} count={valueCount}>
          <textarea
            className="m-form-input m-form-area mono"
            rows={3}
            value={cond.v}
            placeholder={t(ruleTypePlaceholderKey(cond.t))}
            onChange={(e) => onValueChange(e.target.value)}
          />
        </Fold>
      ) : (
        <div className="m-form-row">
          <span className="m-form-label">{t('rules.condValues')}</span>
          <textarea
            className="m-form-input m-form-area mono"
            rows={3}
            value={cond.v}
            placeholder={t(ruleTypePlaceholderKey(cond.t))}
            onChange={(e) => onValueChange(e.target.value)}
          />
        </div>
      )}

      {canRemove && (
        <button type="button" className="m-form-btn danger" onClick={onRemove}>
          {t('rules.removeCondition')}
        </button>
      )}

      <SelectSheetPanel
        label={t('rules.matchType')}
        groups={typeGroups}
        value={cond.t}
        open={sheet === sheetId}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          onTypeChange(next as RuleType);
          setSheet(null);
        }}
        closeLabel={t('common.close')}
      />
    </section>
  );
}
