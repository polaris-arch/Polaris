/**
 * 移动端**订阅表单**（新增 / 编辑；「重命名」与「编辑」按 `focus` 落到同一张表的不同字段）。
 *
 * # 新增走的是 backend-owned create operation，不是一次 `subscription.update`
 *
 * 复用 `store/subscription-create-operation-store` 的 `start(operationId, input)` —— 那一条腿
 * 处理的是**响应丢失**：Rust 侧可能已经登记并 spawn 了操作而回执没回来，此时按调用方自持的 id
 * 重挂（`createStatus` → `createList`），只有「状态/列表都查无此 id」才算确证没登记、可以另起一次。
 * 抄一份 `api.subscription.createStart` 等于把这条重挂逻辑扔掉，表现是**重复创建同一个订阅**。
 *
 * # 进度事件订阅在**应用级**，恢复面是本文件的 `SubCreateTaskPanel`（2026-09-25 ζ 批 A8）
 *
 * 🔴 **这一段此前写的是「进度事件订阅落在本组件上」**：移动端没有应用级那条腿，表单在自己的
 * 生命期内订阅 `onCreateProgressReady`；表单关掉 / 应用被系统回收之后那次创建仍在后端跑，而
 * 移动端**没有恢复面**。Android 上进程回收、Activity 重建远比桌面频繁，这一格不是边角。
 *
 * 现在与桌面同形：`mobile/app-wiring.ts` 在应用级挂 `subscribeAndHydrateSubscriptionCreateOperations()`
 * （先登记事件再 list 水合，顺序由 store 那个函数自持），本表单只读 store。水合找回来的「本机
 * 发起过的」操作交给 `SubCreateTaskPanel` —— 进度文案与本表单是**同一份**
 * （`subscriptionCreateProgressText`），不另写一套。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import type { SubscriptionConfig } from '@/contracts/types';
import {
  subscriptionCreateIsCancellable,
  type SubscriptionCreatePhase,
  type SubscriptionCreateSnapshot,
} from '@/contracts/subscription-create-operation';
import { SUBSCRIPTION_ERROR_I18N_KEY } from '@/contracts/subscription-preview';
import { subscriptionErrorDetail } from '@/domain/subscription-error-text';
import { resolveSubscriptionViaProxy } from '@/domain/subscription-proxy';
import { subAutoUpdateNoticeKey, subAutoUpdateNoticeMode, subEffectiveIntervalHours } from '@/domain/subscription-auto-update';
import { useAppStore, useEffectiveConfig } from '@/store/app-store';
import { toast } from '@/lib/error-handler';
import {
  subscriptionCreateTerminalNeedsAnnouncement,
  useSubscriptionCreateOperationStore,
} from '@/store/subscription-create-operation-store';
import { isSubscriptionUrl } from '@/components/dialogs/sub-url';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { MobileSelect } from '../MobileSelect';
import { FormSheet } from './FormSheet';
import { openMobileSubscriptionCreateRecovery, useMobileFormStore } from './form-store';

/** 各阶段的文案键 —— 逐字同桌面 `SubscriptionCreateTaskDialog#PHASE_KEY`。 */
const CREATE_PHASE_KEY: Readonly<Record<SubscriptionCreatePhase, string>> = {
  queued: 'sub.createTaskPhaseQueued',
  fetching: 'sub.createTaskPhaseFetching',
  parsing: 'sub.createTaskPhaseParsing',
  committing: 'sub.createTaskPhaseCommitting',
  succeeded: 'common.done',
  failed: 'sub.createTaskFailed',
  cancelled: 'common.cancel',
};

/**
 * 创建操作的**进度视图**（一句话）：有提供者扇出计数时报计数，否则报阶段。
 * 表单（`SubFormPanel`）与恢复面（`SubCreateTaskPanel`）读同一份，口径同桌面恢复弹窗。
 */
export function subscriptionCreateProgressText(
  snapshot: SubscriptionCreateSnapshot,
  t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  if (!snapshot.terminal && snapshot.providers?.total != null) {
    return t('sub.createTaskProviders', {
      done: snapshot.providers.done ?? 0,
      total: snapshot.providers.total,
    });
  }
  return t(CREATE_PHASE_KEY[snapshot.phase]);
}

export function SubFormPanel({
  instanceId,
  subId,
  focus,
  onAdded,
}: {
  instanceId: string;
  subId?: string;
  focus?: 'name' | 'url';
  onAdded?: (subId: string) => void;
}): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const config = useEffectiveConfig();
  const loadConfig = useAppStore((s) => s.loadConfig);
  const startCreate = useSubscriptionCreateOperationStore((s) => s.start);
  const clearTerminal = useSubscriptionCreateOperationStore((s) => s.clearTerminal);
  const markTerminalHandled = useSubscriptionCreateOperationStore((s) => s.markTerminalHandled);

  const base = subId === undefined ? undefined : config?.subscriptions?.find((s) => s.id === subId);
  const isEdit = base !== undefined;
  /* 全局订阅代理策略：非 `follow` 时**覆盖** per-sub 那一位（`resolveSubscriptionViaProxy` 的
     语义，下面预检那一格用的就是它）⇒ 开关置灰并换说明，同桌面 `SubDialog.tsx:71/:76`。 */
  const proxyPolicy = config?.subscriptionProxyPolicy ?? 'follow';
  const proxyOverridden = proxyPolicy !== 'follow';

  const [name, setName] = useState(base?.name ?? '');
  const [url, setUrl] = useState(base?.url ?? '');
  const [ua, setUa] = useState(base?.userAgent ?? '');
  const [autoUpdate, setAutoUpdate] = useState(base?.autoUpdate ?? false);
  const autoUpdateNoticeKey = subAutoUpdateNoticeKey(
    subAutoUpdateNoticeMode({ autoUpdate }, config, config?.restartOnNodeChange === true),
  );
  const [viaProxy, setViaProxy] = useState(base?.updateViaProxy ?? false);
  const [proxyBindInterface, setProxyBindInterface] = useState(base?.proxyBindInterface ?? '');
  const interfaces = useNetworkInterfaces();
  const interfaceOptions = buildNetworkInterfaceChoices(interfaces.items, proxyBindInterface, {
    defaultLabel: t('sub.bindInterfaceInherit'),
    unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
    down: t('settings.network.interfaceDown'),
  });
  const [dirty, setDirty] = useState(false);
  const [errName, setErrName] = useState(false);
  const [errUrl, setErrUrl] = useState(false);
  const [busy, setBusy] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [operationId, setOperationId] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  const snapshot = useSubscriptionCreateOperationStore((s) =>
    operationId === null ? null : (s.snapshots[operationId] ?? null),
  );
  const operationBusy = snapshot !== null && !snapshot.terminal;

  /* 终态处置。成功 ⇒ 刷 store 后关表并回调（回调让节点屏切到新订阅的分组）。 */
  useEffect(() => {
    if (snapshot === null || !snapshot.terminal) return;
    if (snapshot.phase === 'failed') {
      clearTerminal(snapshot.operationId);
      setOperationId(null);
      setNotice({
        tone: 'err',
        text: subscriptionErrorDetail(snapshot.error ?? {}, t, 'sub.previewFail'),
      });
      return;
    }
    if (snapshot.phase !== 'succeeded' || !snapshot.result) return;
    const createdId = snapshot.result.subscription.id;
    const partial = snapshot.result.partial === true;
    void (async () => {
      await loadConfig(true);
      /* 🔴 **强刷之后要问一句「真的发布到 store 了吗」**（桌面
         `use-subscription-create-dialog-operation.ts:108-118` 逐字如此，此前这里漏了）。
         后端原子提交成功而这一趟 `loadConfig` 没把它发布进 store 时，若照旧关表 + 回调，
         `onAdded(createdId)` 会把 tab 切到一个 `groups` 里不存在的分组 ⇒ 空列表、零错误提示。
         不成立就**不关表、不报成功**，理由与桌面同一条。 */
      const published = useAppStore
        .getState()
        .config?.subscriptions?.some((s) => s.id === createdId);
      if (published !== true) {
        console.error('[mobile-sub-form] committed subscription was not published to app store');
        setNotice({ tone: 'err', text: t('common.configLoadFail') });
        return;
      }
      /* 表单已经不在了（用户切走 / 被系统回收）⇒ **不清 terminal**：留着它，下一次渲染器
         水合时这次成功的创建才捞得回来。清在 `hasInstance` 之前等于把它永久丢掉（桌面
         同一处的注释逐字写着这条）。 */
      if (!hasInstance(instanceId)) return;
      const shouldToast = subscriptionCreateTerminalNeedsAnnouncement(
        useSubscriptionCreateOperationStore.getState().handledTerminalRevisions[snapshot.operationId], snapshot,
      );
      if (shouldToast) markTerminalHandled(snapshot.operationId, snapshot.revision);
      clearTerminal(snapshot.operationId);
      onAdded?.(createdId);
      closeInstance(instanceId);
      if (shouldToast) toast.success(t(partial ? 'sub.addedPartial' : 'sub.added'));
    })().catch((err: unknown) => {
      console.error('[mobile-sub-form] completion refresh failed:', err);
      setNotice({ tone: 'err', text: t('common.configLoadFail') });
    });
  }, [snapshot, clearTerminal, markTerminalHandled, loadConfig, hasInstance, instanceId, onAdded, closeInstance, t]);

  const requestClose = (): void => {
    /* 创建进行中不许关：终态之后的发布（强刷 + 切 tab）要回到这一层来做。 */
    if (busy || operationBusy) return;
    /* 已提交但发布没成（上面那条 effect 的 `configLoadFail` 腿）：表单可以走，但必须把这次
       操作交给一个**当场可见**的重试面，而不是等下次冷启动水合才被捞回来（同桌面
       `use-subscription-create-dialog-operation.ts#requestClose` 那一格）。 */
    if (snapshot?.phase === 'succeeded') {
      openMobileSubscriptionCreateRecovery(snapshot.operationId);
      closeInstance(instanceId);
      return;
    }
    if (!dirty) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('sub.discardTitle'),
        message: t('sub.discardMsg'),
        confirmLabel: t('sub.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  /** 预检：拉取并解析，**不写 config**。失败按后端分类取当前语种详情，不塞 IPC 原文。 */
  const runPreview = async (): Promise<void> => {
    if (!isSubscriptionUrl(url)) {
      setErrUrl(true);
      return;
    }
    setPreviewing(true);
    setNotice(undefined);
    try {
      const r = await api.subscription.preview(url.trim(), {
        viaProxy: resolveSubscriptionViaProxy(config?.subscriptionProxyPolicy, viaProxy),
        userAgent: ua.trim() === '' ? undefined : ua.trim(),
      });
      if (r.ok) {
        setNotice({ tone: 'ok', text: t('sub.previewOk', { n: r.nodeCount ?? 0 }) });
      } else {
        const key = r.errorKind === undefined ? undefined : SUBSCRIPTION_ERROR_I18N_KEY[r.errorKind];
        setNotice({
          tone: 'err',
          text:
            key !== undefined ? subscriptionErrorDetail(r, t, 'sub.previewFail') : t('sub.previewFail'),
        });
      }
    } catch (e) {
      // IPC 原始诊断只进日志：提示必须跨语言稳定。
      console.error('[mobile-sub-form] preview failed:', e);
      setNotice({ tone: 'err', text: t('sub.previewFail') });
    } finally {
      if (hasInstance(instanceId)) setPreviewing(false);
    }
  };

  const submit = async (): Promise<void> => {
    if (busy || operationBusy) return;
    const nameEmpty = name.trim() === '';
    const urlBad = !isSubscriptionUrl(url);
    setErrName(nameEmpty);
    setErrUrl(urlBad);
    if (nameEmpty || urlBad) return;

    setBusy(true);
    try {
      if (isEdit && base !== undefined) {
        const next: SubscriptionConfig = {
          ...base,
          name: name.trim(),
          url: url.trim(),
          autoUpdate,
          userAgent: ua.trim() === '' ? undefined : ua.trim(),
          updateViaProxy: viaProxy,
          proxyBindInterface: proxyBindInterface || undefined,
        };
        // 编辑只写 config（对齐桌面：edit 不自动拉取）。
        await api.subscription.update(next);
        await loadConfig(true);
        closeInstance(instanceId);
        toast.success(t('sub.updated'));
        return;
      }
      /* 新增：调用方自持 operationId（响应丢失后按同一个 id 重挂，见头注）。 */
      const nextId = operationId ?? crypto.randomUUID();
      setOperationId(nextId);
      await startCreate(nextId, {
        name: name.trim(),
        url: url.trim(),
        autoUpdate,
        userAgent: ua.trim() === '' ? undefined : ua.trim(),
        updateViaProxy: viaProxy,
        proxyBindInterface: proxyBindInterface || undefined,
      });
      /* 不在这里关表：终态由上面那条 effect 处置（成功才关，失败留在原地显详情）。 */
    } catch (e) {
      console.error('[mobile-sub-form] save failed:', e);
      setNotice({ tone: 'err', text: t('common.saveFailed') });
    } finally {
      if (hasInstance(instanceId)) setBusy(false);
    }
  };

  const progress =
    snapshot === null || snapshot.terminal ? undefined : subscriptionCreateProgressText(snapshot, t);
  return (
    <FormSheet
      title={isEdit ? t('sub.editTitle') : t('sub.addTitle')}
      onRequestClose={requestClose}
      closeLocked={busy || operationBusy}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={isEdit ? t('common.save') : t('sub.add')}
      submitDisabled={busy || previewing || operationBusy}
      onSubmit={() => void submit()}
      notice={progress !== undefined ? { tone: 'info', text: progress } : notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="msf-name">
          {t('sub.name')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="msf-name"
          className="m-form-input"
          value={name}
          placeholder={t('sub.namePh')}
          /* 「重命名」与「编辑」是同一张表的两个落点（Polaris 只有一个订阅表单）——
             靠 autoFocus 区分，而不是摆两个点下去完全一样的菜单项。 */
          autoFocus={focus === 'name'}
          onChange={(e) => {
            setName(e.target.value);
            setDirty(true);
            setErrName(false);
          }}
        />
        {errName && <p className="m-form-err">{t('sub.errName')}</p>}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="msf-url">
          {t('sub.url')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="msf-url"
          className="m-form-input mono"
          value={url}
          placeholder="https://example.com/sub?token=…"
          autoFocus={focus === 'url'}
          onChange={(e) => {
            setUrl(e.target.value);
            setDirty(true);
            setErrUrl(false);
          }}
        />
        {errUrl && <p className="m-form-err">{t('sub.errUrl')}</p>}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="msf-ua">
          {t('sub.ua')}
          <span className="m-form-opt">{t('common.optional')}</span>
        </label>
        <input
          id="msf-ua"
          className="m-form-input mono"
          value={ua}
          placeholder={t('sub.uaPh')}
          onChange={(e) => {
            setUa(e.target.value);
            setDirty(true);
          }}
        />
        <p className="m-form-hint">{t('sub.uaHint')}</p>
      </div>

      <div className="m-form-row m-form-switch">
        <div className="m-form-switch-tx">
          <span className="m-form-label" id="msf-auto-l">
            {t('sub.autoUpdate')}
          </span>
          <p className="m-form-hint">{t('sub.autoUpdateHint')}</p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={autoUpdate}
          aria-labelledby="msf-auto-l"
          className={`m-form-swt${autoUpdate ? ' on' : ''}`}
          onClick={() => {
            setAutoUpdate((v) => !v);
            setDirty(true);
          }}
        />
      </div>

      {autoUpdateNoticeKey !== '' && (
        <p className="m-form-hint m-form-auto-update-notice">
          {t(autoUpdateNoticeKey, { h: subEffectiveIntervalHours(config) })}
        </p>
      )}

      {/* 🔴 全局 `subscriptionProxyPolicy` 非 `follow` 时这颗开关**结构上不可能生效**
          （`domain/subscription-proxy.ts:20-22` 直接按 policy 返回，忽略 per-sub），故置灰并
          把说明换成两句已有的 override 文案 —— 同桌面 `SubDialog.tsx:382-386/:396`。
          此前这里无条件可点、说明恒为通用那句：用户拨动它、保存成功、`updateViaProxy` 也确实
          写进了 config，而更新行为一点不变，屏上没有一个字解释为什么（§4.12 用常驻行，不用 tooltip）。 */}
      <div className="m-form-row m-form-switch">
        <div className="m-form-switch-tx">
          <span className="m-form-label" id="msf-proxy-l">
            {t('sub.viaProxy')}
          </span>
          <p className="m-form-hint">
            {proxyPolicy === 'proxy'
              ? t('sub.viaProxyOverrideProxy')
              : proxyPolicy === 'direct'
                ? t('sub.viaProxyOverrideDirect')
                : t('sub.viaProxyHint')}
          </p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={viaProxy}
          aria-labelledby="msf-proxy-l"
          className={`m-form-swt${viaProxy ? ' on' : ''}`}
          disabled={proxyOverridden}
          onClick={() => {
            setViaProxy((v) => !v);
            setDirty(true);
          }}
        />
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="msf-interface">
          {t('sub.bindInterface')}
        </label>
        <MobileSelect
          id="msf-interface"
          className="m-form-select"
          aria-describedby="msf-interface-hint"
          value={proxyBindInterface}
          disabled={interfaces.loading && interfaces.items.length === 0}
          onChange={(e) => {
            setProxyBindInterface(e.currentTarget.value);
            setDirty(true);
          }}
        >
          {interfaceOptions.map((option) => (
            <option key={option.value} value={option.value} disabled={option.disabled === true}>
              {option.label}
            </option>
          ))}
        </MobileSelect>
        <p className="m-form-hint" id="msf-interface-hint">{t('sub.bindInterfaceHint')}</p>
        {interfaces.failed && <p className="m-form-err">{t('settings.network.interfaceListFailed')}</p>}
      </div>

      <button
        type="button"
        className="m-form-btn"
        disabled={previewing || busy || operationBusy}
        onClick={() => void runPreview()}
      >
        {t('sub.previewBtn')}
      </button>
    </FormSheet>
  );
}

/**
 * 订阅**创建操作恢复面**（2026-09-25 ζ 批 A8）。桌面对位：`SubscriptionCreateTaskDialog`。
 *
 * 只为「本机发起过」的操作打开（`mobile/app-wiring.ts` 水合后的 `recovered`，或上面表单在
 * 发布失败时交接过来）—— 后端 list 里只有历史终态的那些**永远不开**，否则每次 WebView 重建都会
 * 把旧的成功重播一遍。三种终局：
 *  · 进行中 —— 显示阶段；关闭 = 「取消添加」（叠一层确认，同桌面：后端操作不许变成看不见的）；
 *  · 失败 —— 原地显示本地化原因，关闭即清掉这条追踪；
 *  · 成功 —— 强刷配置、**确认真的发布进 store** 之后才清掉并关闭，回执走全局 toast
 *    （面板当帧卸载，面板内 notice 一帧都画不出来，同 `ImportFormPanel` 那条 🔴）；
 *    没发布成就不关、给「重试」。
 *
 * 判据逐字取自桌面那个组件（关闭锁 / 「面板没了就别清终态」/ 成功回执键），就地写成三行，
 * 不 import 桌面那个 `.tsx`：它连着桌面 `Modal` 与 `dialog-store`，不该进移动端的模块图。
 */
export function SubCreateTaskPanel({
  instanceId,
  operationId,
}: {
  instanceId: string;
  operationId: string;
}): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const snapshot = useSubscriptionCreateOperationStore((s) => s.snapshots[operationId] ?? null);
  const cancel = useSubscriptionCreateOperationStore((s) => s.cancel);
  const clearTerminal = useSubscriptionCreateOperationStore((s) => s.clearTerminal);
  const markTerminalHandled = useSubscriptionCreateOperationStore((s) => s.markTerminalHandled);
  const terminalHandledRevision = useSubscriptionCreateOperationStore(
    (s) => s.handledTerminalRevisions[operationId],
  );
  const loadConfig = useAppStore((s) => s.loadConfig);
  const [cancelling, setCancelling] = useState(false);
  const [settling, setSettling] = useState(false);
  const [completionFailed, setCompletionFailed] = useState(false);
  const [publishAttempt, setPublishAttempt] = useState(0);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const cancelInFlight = useRef(false);
  const handledAttempt = useRef<string | null>(null);

  /* 关闭锁，同桌面 `subscriptionCreateTaskCloseLocked`：已提交（committing / succeeded）的操作
     在发布确认之前不许被关掉 —— 关掉就是把一次已落盘的创建变成看不见的。 */
  const closeLocked =
    cancelling || settling || snapshot?.phase === 'committing' || snapshot?.phase === 'succeeded';

  useEffect(() => {
    if (!snapshot?.terminal) return;
    const attemptKey =
      snapshot.phase === 'succeeded' ? `${snapshot.revision}:${publishAttempt}` : `${snapshot.revision}`;
    if (handledAttempt.current === attemptKey) return;
    handledAttempt.current = attemptKey;

    if (snapshot.phase === 'cancelled') {
      clearTerminal(operationId);
      closeInstance(instanceId);
      return;
    }
    if (snapshot.phase !== 'succeeded' || !snapshot.result) return; // failed：原因常驻在面板里
    const createdId = snapshot.result.subscription.id;
    const partial = snapshot.result.partial === true;
    const shouldToast = subscriptionCreateTerminalNeedsAnnouncement(terminalHandledRevision, snapshot);
    setSettling(true);
    void (async () => {
      await loadConfig(true);
      const published = useAppStore
        .getState()
        .config?.subscriptions?.some((sub) => sub.id === createdId);
      if (published !== true) {
        console.error('[mobile-sub-create-task] committed subscription was not published to app store');
        if (hasInstance(instanceId)) {
          setSettling(false);
          setCompletionFailed(true);
          setNotice({ tone: 'err', text: t('common.configLoadFail') });
        }
        return;
      }
      /* 面板已经不在了 ⇒ **不清终态**：留给下一次水合重试发布（同桌面
         `subscriptionCreatePublicationCanFinalize`）。少一条成功回执可以接受，少一次发布不行。 */
      if (!hasInstance(instanceId)) return;
      if (shouldToast) markTerminalHandled(operationId, snapshot.revision);
      clearTerminal(operationId);
      closeInstance(instanceId);
      if (shouldToast) toast.success(t(partial ? 'sub.addedPartial' : 'sub.added'));
    })().catch((err: unknown) => {
      console.error('[mobile-sub-create-task] completion refresh failed:', err);
      if (hasInstance(instanceId)) {
        setSettling(false);
        setCompletionFailed(true);
        setNotice({ tone: 'err', text: t('common.configLoadFail') });
      }
    });
  }, [
    snapshot,
    publishAttempt,
    terminalHandledRevision,
    clearTerminal,
    closeInstance,
    hasInstance,
    instanceId,
    loadConfig,
    markTerminalHandled,
    operationId,
    t,
  ]);

  const retryPublication = (): void => {
    if (snapshot?.phase !== 'succeeded' || settling) return;
    setCompletionFailed(false);
    setNotice(undefined);
    setPublishAttempt((n) => n + 1);
  };

  const requestClose = (): void => {
    if (closeLocked) return;
    if (snapshot === null || snapshot.terminal) {
      if (snapshot?.terminal) clearTerminal(operationId);
      closeInstance(instanceId);
      return;
    }
    if (!subscriptionCreateIsCancellable(snapshot) || cancelInFlight.current) return;
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('sub.cancelCreateTitle'),
        message: t('sub.cancelCreateMsg'),
        confirmLabel: t('sub.cancelCreate'),
        danger: true,
        onConfirm: async () => {
          if (cancelInFlight.current) return;
          cancelInFlight.current = true;
          setCancelling(true);
          setNotice(undefined);
          closeInstance(confirmId);
          try {
            const next = await cancel(operationId);
            /* 只有终态 cancelled 帧才关面板（上面那条 effect）；非终态回执 ⇒ 解锁，允许再点。 */
            if (!next.terminal && hasInstance(instanceId)) {
              cancelInFlight.current = false;
              setCancelling(false);
            }
          } catch (err) {
            console.error('[mobile-sub-create-task] cancel failed:', err);
            cancelInFlight.current = false;
            if (hasInstance(instanceId)) {
              setCancelling(false);
              setNotice({ tone: 'err', text: t('sub.previewFail') });
            }
          }
        },
      },
    });
  };

  const running = snapshot !== null && !snapshot.terminal;
  const view =
    notice ??
    (snapshot === null
      ? { tone: 'info' as const, text: t('common.loading') }
      : snapshot.phase === 'failed'
        ? {
            tone: 'err' as const,
            text: `${subscriptionCreateProgressText(snapshot, t)} · ${subscriptionErrorDetail(
              snapshot.error ?? {},
              t,
              'sub.previewFail',
            )}`,
          }
        : { tone: 'info' as const, text: subscriptionCreateProgressText(snapshot, t) });
  const retry = snapshot?.phase === 'succeeded' && completionFailed;

  return (
    <FormSheet
      title={t('sub.createTaskTitle')}
      onRequestClose={requestClose}
      closeLocked={closeLocked}
      closeLabel={t('common.close')}
      /* 进行中时「关闭」就是「取消添加」（叠确认），所以按钮直说；终态才叫「关闭」。 */
      cancelLabel={running ? t('sub.cancelCreate') : t('common.close')}
      submitLabel={retry ? t('common.retry') : undefined}
      submitDisabled={settling}
      onSubmit={retryPublication}
      notice={view}
    >
      <p className="m-form-hint">{t('sub.createTaskRecoveredHint')}</p>
    </FormSheet>
  );
}
