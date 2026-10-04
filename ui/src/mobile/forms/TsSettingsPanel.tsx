import { supportsTsAccountActions } from '@/components/dialogs/ts-login-server';
/**
 * 移动端 **Tailscale 设置表** —— 批 3。节点行上「编辑」一个 tailscale 节点落到这里
 * （`form-store#mobileEditFormFor`），组网接入面上点 Tailscale 且已有节点时也落到这里
 * （与桌面 `MeshJoinDialog` 的 `go({ kind: tsNode ? 'ts-settings' : 'ts-login' })` 逐字同形）。
 *
 * # 复用的是什么（判据一条都不新造）
 *
 *  · **字段表** —— `components/dialogs/ts-spec#tsMainSpec` + `#TS_ADV_SPEC`（本批从
 *    `TsSettingsDialog.tsx` 拆进零 React 的 `.ts`）。那张表同时是覆盖门的判据面：
 *    Rust `TailscaleSettings` 加一个键而表里没有对应控件 ⇒ 门红，两端一起红。
 *  · **分组** —— `mesh-form-layout#groupTsFields`（桌面页签 / 这里折叠段，同一份分区）。
 *  · **草稿 ⇄ 设置** —— `ts-settings-logic#initTsDraft` / `#buildTsSettings`
 *    （「缺省即默认」：不把默认值写成显式值，否则磁盘成了第二个默认值真值源；
 *    并**保全未建模字段** —— `authKey` 由登录腿写入，本面绝不能覆写掉）。
 *  · **两条提交前校验** —— `#invalidTsCidrs`（后端对非法 CIDR 是静默丢弃）与
 *    `#invalidControlUrl`（IP 形式的 control_url 会让 sing-box 初始化 tailscale endpoint 时
 *    直接 panic）。两条都必须拦在**保存这一刻**，光标还在那个输入框旁边。
 *  · **出口候选** —— `#exitNodeOptions`（原样收下全部 peer，筛/排/去重/禁用注记都在那支纯函数里）。
 *  · **登出** —— `api.server.tailscaleLogout` + `lib/staged-config#splitStagedOnly`
 *    （登出清的是磁盘上的 TS state 目录，盘上没有这个节点就没有作用对象）。
 *
 * # 与 `TsExitPanel` 的分工
 *
 * `TsExitPanel`（W-05）只搬**出口设备那一格** —— 它是 TsExitWarning 那条安全注脚唯一能解除的
 * 东西，入口在节点行上、要一跳到位。本面板是**整张设置表**，出口那一格在它的「基础」组里。
 * 两者共用同一份 `ts-settings-logic`，不会给「出口选了什么」造两个答案。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
/* 「展开即露出」的全仓不变量（`components/reveal.ts`）：`.m-form-body` 是 `overflow-y:auto` 的
   单一滚动容器，底部那个分组展开时新长出来的字段整段落在视区之外。与桌面四个分组菜单**同一形状**，
   故走同一条腿，不在这里另写一个滚动。 */
import { revealElement, useRevealAfterCommit } from '@/components/reveal';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import type { MeshInboundPolicy } from '@/contracts/types';
import type { TailscaleStatusPeer } from '@/contracts/tailscale-status';
import type { FormValue, FormValues, SelectOption } from '@/components/dialogs/field-spec';
import { TS_ADV_SPEC, tsMainSpec } from '@/components/dialogs/ts-spec';
import { groupTsFields, type TsFormGroup } from '@/components/dialogs/mesh-form-layout';
import {
  buildTsSettings,
  exitNodeOptions,
  initTsDraft,
  invalidControlUrl,
  invalidTsCidrs,
  peersForTsNode,
} from '@/components/dialogs/ts-settings-logic';
import { applyDetour, endpointDetourOptions } from '@/components/dialogs/detour-options';
import { applyMeshInboundPolicy, meshInboundPolicyError, normalizeMeshInboundPolicy } from '@/components/dialogs/mesh-inbound-policy';
import { INVALID_NODE_REASON_KEY } from '@/domain/invalid-node-reason';
import { taildropBadgeCount } from '@/domain/taildrop';
import { editRoute, splitStagedOnly, stagedOnlyIds } from '@/lib/staged-config';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { MobileFields } from './FormFields';
import { MeshInboundPolicyFields } from './MeshInboundPolicyFields';
import { FormSheet } from './FormSheet';
import { FormGroup } from './FormGroup';
import { useMobileFormStore } from './form-store';
import { isMainCoreLogoutError, sameRunningCore, stopOwnedCoreThenLogout } from './ts-logout-flow';

/** 分组的呈现顺序与标题键（分区本身来自 `groupTsFields`）。 */
const TS_GROUPS: ReadonlyArray<readonly [TsFormGroup, string]> = [
  ['basic', 'node.formGroup.basic'],
  ['routing', 'node.formGroup.routing'],
  ['advanced', 'node.formGroup.advanced'],
];

export function TsSettingsPanel({
  instanceId,
  serverId,
}: {
  instanceId: string;
  serverId: string;
}): ReactElement {
  const { t } = useTranslation();
  const accountActionsSupported = supportsTsAccountActions();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const isTop = useMobileFormStore((s) => s.stack[s.stack.length - 1]?.instanceId === instanceId);
  const servers = useEffectiveServers();
  const diskServers = useAppStore((s) => s.servers);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const tailscaleStatuses = useAppStore((s) => s.tailscaleStatuses);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const interfaces = useNetworkInterfaces();

  /* 单例：编辑目标由**面板自查**（同桌面 `TsSettingsDialog`），故那一支不带 id。 */
  /* 按 `serverId` 寻址，**不按协议 `.find()`**：Tailscale 已不是单例
     （`meshSingletonConflict` 只剩 WARP 支），按协议取「任意一个」会让编辑第二个节点
     打开/写坏第一个。桌面 `TsSettingsDialog` 同一处同一条改动。 */
  const node = servers.find((s) => s.id === serverId);

  const [peers, setPeers] = useState<readonly TailscaleStatusPeer[]>([]);
  const [connected, setConnected] = useState<boolean | null>(null);
  // A saved node can exist after logout. Only native state existence may offer Logout.
  const [hasLoginState, setHasLoginState] = useState<boolean | null>(null);
  const stateReadRevision = useRef(0);
  const [draft, setDraft] = useState<FormValues>(() => ({
    ...initTsDraft(node),
    bindInterface: node?.bindInterface ?? '',
  }));
  const [name, setName] = useState(node?.name ?? '');
  const [errName, setErrName] = useState(false);
  const [meshPolicy, setMeshPolicy] = useState<MeshInboundPolicy | undefined>(node?.meshInboundPolicy);
  const [meshPolicyError, setMeshPolicyError] = useState<string | null>(null);
  const [meshPolicyErrorVersion, setMeshPolicyErrorVersion] = useState(0);
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [openGroups, setOpenGroups] = useState<ReadonlySet<TsFormGroup>>(new Set(['basic']));
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  /* 「本次提交之后再露出」：点的那一刻字段还没渲染，当场量到的是旧 DOM。 */
  const scheduleReveal = useRevealAfterCommit();

  /* 出口候选：拉状态快照（核未跑 / 无节点时为空 ⇒ 降级成手填，不是报错；同桌面那条 effect）。 */
  useEffect(() => {
    let cancelled = false;
    setPeers([]);
    setConnected(null);
    api.server
      .tailscaleGetStatus()
      .then((snap) => {
        if (cancelled) return;
        setConnected(snap.connected && snap.statuses.some((status) => status.serverId === serverId));
        setPeers(peersForTsNode(snap, serverId));
      })
      .catch(() => {
        if (cancelled) return;
        setPeers([]); // 非 Tauri / 核未跑 / 失败 → 手动填写降级。
      });
    return () => {
      cancelled = true;
    };
  }, [serverId]);

  // Re-read when this sheet becomes visible again after a login sheet closes, and when
  // Android returns from system UI. A failed read is unknown, never proof of a session.
  useEffect(() => {
    if (!isTop || !accountActionsSupported) return;
    let alive = true;
    const readState = (): void => {
      const revision = ++stateReadRevision.current;
      setHasLoginState(null);
      void api.server.tailscaleStateExists([serverId]).then(
        (states) => {
          if (alive && revision === stateReadRevision.current)
            setHasLoginState(typeof states[serverId] === 'boolean' ? states[serverId] : null);
        },
        () => {
          if (alive && revision === stateReadRevision.current) setHasLoginState(null);
        },
      );
    };
    readState();
    window.addEventListener('focus', readState);
    return () => {
      alive = false;
      window.removeEventListener('focus', readState);
    };
  }, [serverId, isTop]);

  const setField = (k: string, v: FormValue): void => {
    setDraft((d) => ({ ...d, [k]: v }));
    setDirty(true);
  };

  /* Taildrop 未读角标。判据与桌面同一函数（`domain/taildrop#taildropBadgeCount`）：
     取 `unreadFileCount` 而非 `waitingFileCount`（读过但没删的文件仍在 waiting 里，
     拿它当角标会让角标永远消不掉），且不可用时恒 0。 */
  const unread = taildropBadgeCount(tailscaleStatuses[serverId]);

  /* 判据取**已保存值**而非草稿值（禁用豁免须在整个面板生命期内稳定，见 exitNodeOptions 头注）。 */
  const savedExit = node?.tailscaleSettings?.exitNode ?? '';
  const exitOpts = exitNodeOptions(peers, savedExit, {
    none: t('ts.exitNone'),
    custom: t('common.customEllipsis'),
    inUse: t('ts.exitInUse'),
    offline: t('ts.exitOffline'),
    notAdvertised: t('ts.exitNotAdvertised'),
  });
  const detourOpts = endpointDetourOptions(servers, node?.id, t('node.detourDirect'));
  const interfaceOpts: SelectOption[] = buildNetworkInterfaceChoices(
    interfaces.items,
    typeof draft.bindInterface === 'string' ? draft.bindInterface : '',
    {
      defaultLabel: t('node.bindInterfaceInherit'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    },
  ).map(({ value, label, disabled }) => [value, label, disabled] as SelectOption);
  const groups = groupTsFields([...tsMainSpec(exitOpts, detourOpts, interfaceOpts), ...TS_ADV_SPEC]);

  const requestClose = (): void => {
    if (!dirty) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('ts.discardTitle'),
        message: t('ts.discardMsg'),
        confirmLabel: t('ts.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  const save = async (): Promise<void> => {
    if (node === undefined) return;
    const trimmedName = name.trim();
    if (!trimmedName) {
      setErrName(true);
      return;
    }
    const policy = normalizeMeshInboundPolicy(meshPolicy);
    const policyError = meshInboundPolicyError(policy, 'tailscale');
    setMeshPolicyError(policyError);
    if (policyError) {
      setMeshPolicyErrorVersion((version) => version + 1);
      setNotice({ tone: 'err', text: t(policyError) });
      return;
    }
    /* 非法 CIDR 必须前端拦：后端 sanitize 对非法项是**静默丢弃**，不拦就成了「界面收下了、盘上没有」。 */
    const badCidr = invalidTsCidrs(draft);
    if (badCidr.length > 0) {
      setOpenGroups((prev) => new Set([...prev, 'routing']));
      setNotice({ tone: 'err', text: t('ts.errCidr', { list: badCidr.join(', ') }) });
      return;
    }
    /* control_url 必须拦在**保存**这一刻：IP 形式会让 sing-box 初始化 tailscale endpoint 时直接
       panic。后端虽有 fail-closed 兜底，但那是**事后**告知 —— 用户那时已经离开本面板。 */
    const badControl = invalidControlUrl(draft);
    if (badControl !== null) {
      setOpenGroups((prev) => new Set([...prev, 'advanced']));
      setNotice({
        tone: 'err',
        text: `${t('ts.errControlUrl')}${t('common.colon')}${t(INVALID_NODE_REASON_KEY[badControl])}`,
      });
      return;
    }
    setBusy(true);
    try {
      // detour 在顶层，`buildTsSettings` 够不着 —— 单独写回（哨兵 ⇒ 删键）。
      const next = applyDetour(
        { ...node, name: trimmedName, tailscaleSettings: buildTsSettings(node.tailscaleSettings, draft) },
        draft.detour,
      );
      const bindInterface = String(draft.bindInterface ?? '').trim();
      if (bindInterface) next.bindInterface = bindInterface;
      else delete next.bindInterface;
      applyMeshInboundPolicy(next, policy);
      // 暂存灰度的**唯一**闸门（与桌面同一个 `editRoute`）。
      if (editRoute('servers', stagingEnabled) === 'staged') {
        stage({
          id: `server:${node.id}`,
          kind: 'server',
          label: `${t('ts.settingsTitle')} ${next.name}`,
          entityPath: ['servers', node.id],
          nextValue: next,
        });
        closeInstance(instanceId);
        return; // 零 IPC 写、零磁盘写
      }
      await api.server.update(next);
      await loadConfig(true);
      closeInstance(instanceId);
      toast.success(t('common.saved'));
    } catch (e) {
      console.error('[mobile-ts-settings] save failed:', e);
      setNotice({ tone: 'err', text: t('common.saveFailed') });
    } finally {
      if (hasInstance(instanceId)) setBusy(false);
    }
  };

  const completeLogout = async (serverId: string): Promise<void> => {
    // Invalidate an older state query issued while the confirmation was closing.
    ++stateReadRevision.current;
    setHasLoginState(false);
    const store = useAppStore.getState();
    store.setTailscaleLoginState(serverId, false);
    store.setTailscaleAuthUrl(serverId, null);
    store.setTailscaleLoginInitiated(serverId, false);
    store.clearTailscaleStatus(serverId);
    try {
      await loadConfig(true);
      closeInstance(instanceId);
      toast.success(t('nodes.meshTsLogoutOk'));
    } catch {
      setNotice({ tone: 'info', text: t('ts.logoutRefreshFailed') });
    }
  };

  /** 登出：主核持有该节点时另问一次是否断开，绝不从通用错误推断可以停核。 */
  const requestLogout = (): void => {
    if (!accountActionsSupported) return;
    if (node === undefined) return;
    const serverId = node.id;
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('ts.logout'),
        message: t('meshJoin.logout'),
        confirmLabel: t('ts.logout'),
        danger: true,
        onConfirm: async () => {
          closeInstance(confirmId);
          const stagedOnly = stagedOnlyIds(servers, diskServers);
          const split = splitStagedOnly(
            'server.tailscaleLogout',
            [serverId],
            stagedOnly,
            stagedEntries,
            'servers',
          );
          if (split.blocked.length > 0) {
            /* 「还不能做」而不是错误：盘上没有这个节点，就没有 state 目录可清。 */
            setNotice({ tone: 'info', text: t('home.stagedOnlyBlocked') });
            return;
          }
          setBusy(true);
          try {
            await api.server.tailscaleLogout(serverId);
            await completeLogout(serverId);
          } catch (e) {
            if (!isMainCoreLogoutError(e)) {
              setNotice({ tone: 'err', text: t('nodes.meshTsLogoutFail') });
              return;
            }
            // The native writer gate refused this exact node. Capture the live core before asking
            // to stop it; a later confirmation must not stop a replacement/user-selected core.
            const ownerStatus = await api.proxy.getStatus().catch(() => null);
            if (!ownerStatus || !sameRunningCore(ownerStatus, ownerStatus)) {
              setNotice({ tone: 'info', text: t('ts.logoutStopChanged') });
              return;
            }
            const owner = { serverId, selectedId: useAppStore.getState().selectedServerId,
              status: ownerStatus };
            const stopConfirmId = open({
              kind: 'confirm',
              payload: {
                title: t('ts.logoutStopTitle'),
                message: t('ts.logoutStopMessage'),
                confirmLabel: t('ts.logoutStopConfirm'),
                danger: true,
                onConfirm: async () => {
                  closeInstance(stopConfirmId);
                  if (!hasInstance(instanceId)) return;
                  setBusy(true);
                  try {
                    const result = await stopOwnedCoreThenLogout(owner, {
                      selectedId: () => useAppStore.getState().selectedServerId,
                      serverPresent: (id) => useAppStore.getState().servers.some((s) => s.id === id),
                      status: () => api.proxy.getStatus(),
                      stop: () => useAppStore.getState().stopProxy(),
                      logout: async (id) => { await api.server.tailscaleLogout(id); },
                    });
                    if (result.kind === 'loggedOut') await completeLogout(serverId);
                    else if (result.kind === 'changed') setNotice({ tone: 'info', text: t('ts.logoutStopChanged') });
                    else if (result.kind === 'stopFailed') setNotice({ tone: 'err', text: t('ts.logoutStopFailed') });
                    else setNotice({ tone: 'err', text: t(result.code === 'TAILSCALE_LOGOUT_MAIN_CORE'
                      ? 'ts.reasonMainCoreInUse' : 'nodes.meshTsLogoutFail') });
                  } catch (error) {
                    // Delegate or local-state failures are not one of the helper's structured outcomes.
                    // Keep this form open and surface them instead of dropping an async confirmation rejection.
                    console.error('[mobile-ts-settings] confirmed logout failed:', error);
                    if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t('nodes.meshTsLogoutFail') });
                  } finally {
                    if (hasInstance(instanceId)) setBusy(false);
                  }
                },
              },
            });
          } finally {
            if (hasInstance(instanceId)) setBusy(false);
          }
        },
      },
    });
  };

  return (
    <FormSheet
      title={t('ts.settingsTitle')}
      onRequestClose={requestClose}
      closeLocked={busy}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={t('common.save')}
      submitDisabled={busy || node === undefined}
      onSubmit={() => void save()}
      notice={notice}
    >
      {node === undefined ? (
        <p className="m-form-hint">{t('ts.noNode')}</p>
      ) : (
        <>
          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mts-settings-name">
              {t('ts.nodeName')}<span className="m-form-req" aria-hidden>*</span>
            </label>
            <input id="mts-settings-name" className="m-form-input" value={name} disabled={busy}
              onChange={(e) => { setName(e.target.value); setErrName(false); setDirty(true); }} />
            {errName && <p className="m-form-err">{t('ts.errName')}</p>}
          </div>

          {/* 核没跑 ⇒ 出口候选恒空，只剩「无 / 自定义…」。如实说明而不是留一个空下拉让人猜。 */}
          {connected === false && <p className="m-form-hint">{t('ts.exitEmptyHint')}</p>}

          {TS_GROUPS.map(([id, titleKey]) => (
            <FormGroup
              key={id}
              title={t(titleKey)}
              open={openGroups.has(id)}
              onToggle={(section) => {
                const willOpen = !openGroups.has(id);
                setOpenGroups((prev) => {
                  const next = new Set(prev);
                  if (next.has(id)) next.delete(id);
                  else next.add(id);
                  return next;
                });
                /* 只在**展开**时滚：折叠只会让内容变短，此时滚动等于凭空把用户挪走。 */
                scheduleReveal(willOpen && section !== null ? () => revealElement(section) : null);
              }}
            >
              <MobileFields fields={groups[id]} values={draft} onChange={setField} t={t} />
            </FormGroup>
          ))}

          <MeshInboundPolicyFields idPrefix="mts" protocol="tailscale"
            value={meshPolicy} errorKey={meshPolicyError} errorVersion={meshPolicyErrorVersion}
            onChange={(next) => { setMeshPolicy(next); setMeshPolicyError(null); setDirty(true); }} />

          {/* 账号级动作。桌面把它们摆在组网接入面的 Tailscale 卡片上（`MeshJoinDialog` 的
              `actions`）；移动端那张接入面是一列纵向选择，塞不下三颗次动作，故收进这张表的末尾 ——
              Taildrop 与登录入口常驻；只有 native state 存在才给登出动作。
              处置逐条登记在 `nodes/absence-register.ts#MESH_JOIN_ACTIONS`。

              🔴 Taildrop 那颗 2026-09-13（批 16）补进来。它收在这里而不是接入面上，恰恰是因为
              桌面本轮把那张卡改成多节点分行的**那条理由**：收件箱必须按 `serverId` 寻址。
              本面板的 union 那一支**必须带 id**（`form-store.ts`），于是「绑在第一个节点上」
              这件事在移动端结构上就不会发生；而每个 Tailscale 节点在节点屏上各有一行、
              「编辑」都落到这张表 ⇒ 每一个节点的收件箱都到得了。完整依据见 `TaildropPanel` 头注。 */}
          {!accountActionsSupported && <p className="m-form-hint">{t('ts.iosAccountActionsUnavailable')}</p>}
          <div className="m-form-row">
            <span className="m-form-label">{t('ts.method')}</span>
            <div className="m-form-inline">
              <button
                type="button"
                className="m-form-btn"
                disabled={busy}
                onClick={() => open({ kind: 'taildrop', serverId })}
              >
                {t('meshJoin.taildrop')}
                {/* 未读数。`taildropBadgeCount` 在「连不上」的三态里恒返 0（与桌面同一函数）——
                    一个进不去的收件箱不该在界面上顶着数字。 */}
                {unread > 0 && ` (${unread})`}
              </button>
              <button
                type="button"
                className="m-form-btn"
                disabled={busy}
                onClick={() => open({ kind: 'ts-login', serverId })}
              >
                {t(!accountActionsSupported ? 'common.edit' : hasLoginState === false ? 'ts.signIn' : 'meshJoin.switchAccount')}
              </button>
              {(hasLoginState === true || !accountActionsSupported) && <button
                type="button"
                className="m-form-btn danger"
                disabled={busy || !accountActionsSupported}
                onClick={requestLogout}
              >
                {t('ts.logout')}
              </button>}
            </div>
          </div>
        </>
      )}
    </FormSheet>
  );
}
