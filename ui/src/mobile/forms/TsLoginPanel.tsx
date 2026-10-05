import { MobileInfo } from '../MobileInfo';
/**
 * 移动 Tailscale 接入：先保存配置，再发起授权。保存成功后的重试复用同一节点。
 * URL 自动打开归 app-wiring；本面板保留复制/重开入口。瞬态进度按 attempt 接受后端
 * 授权结局与 URL；全局 STATUS 只展示账号状态，不证明本次请求成功。
 */
import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import {
  TS_LOGIN_TIMEOUT_MS,
  supportsTsAccountActions,
  executeTsLogin,
  nextTsNodeName,
  planTsLoginSubmit,
  supportsTsLoginActions,
  type TsLoginMode,
} from '@/components/dialogs/ts-login-server';
import type { ServerConfig } from '@/contracts/types';
import { controlUrlReject } from '@/domain/control-url';
import { groupServersBySubscription } from '@/domain/server-grouping';
import { INVALID_NODE_REASON_KEY } from '@/domain/invalid-node-reason';
import { validatedTailscaleAuthUrl } from '@/domain/tailscale-auth-url';
import { copyLoginUrl, loginAttemptActive, loginFailureReasonKey, openLoginUrl, progressForLoginRequest } from '@/domain/tailscale-login-progress';
import { toast } from '@/lib/error-handler';
import { useAppStore, useEffectiveConfig, useEffectiveServers } from '@/store/app-store';
import { useTailscaleLoginProgressStore } from '@/store/use-tailscale-login-progress-store';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

export function TsLoginPanel({
  instanceId,
  serverId,
}: {
  instanceId: string;
  serverId?: string;
}): ReactElement {
  const { t } = useTranslation();
  const accountActionsSupported = supportsTsAccountActions();
  const loginActionsSupported = supportsTsLoginActions();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  /* 落盘决策读**磁盘镜像**（同桌面）：`planTsLoginSubmit` 要回答的是「后端此刻按 id 找不找得到
     这个节点」，那一栏读 disk，不读 effective。 */
  const servers = useAppStore((s) => s.servers);
  const visibleServers = useEffectiveServers();
  const subscriptions = useEffectiveConfig((config) => config?.subscriptions);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const setTailscaleAuthUrl = useAppStore((s) => s.setTailscaleAuthUrl);
  const setTailscaleLoginInitiated = useAppStore((s) => s.setTailscaleLoginInitiated);
  const setTailscaleLoginState = useAppStore((s) => s.setTailscaleLoginState);

  /* `serverId` 是本面板身兼「新建」与「编辑既有节点」的判据：带 id = 给该节点换 key / 换控制面，
     不带 = 新建。**不回落成按协议 `.find()`** —— Tailscale 已不是单例
     （`meshSingletonConflict` 只剩 WARP 支），按协议取「任意一个」会把这次登录写进任意一个
     既有节点，正是 `node-edit-routing.ts` 记的那条缺陷。 */
  const existingTs = serverId ? servers.find((s) => s.id === serverId) : undefined;
  const meshNames = groupServersBySubscription(visibleServers, subscriptions)
    .find((group) => group.id === 'mesh')?.servers.map((server) => server.name) ?? [];

  const [savedServer, setSavedServer] = useState<ServerConfig>();
  const [name, setName] = useState(() => existingTs?.name ?? nextTsNodeName(meshNames));
  const [nameEdited, setNameEdited] = useState(false);
  const [errName, setErrName] = useState(false);
  const [mode, setMode] = useState<TsLoginMode>('browser');
  const [authKey, setAuthKey] = useState('');
  const [errKey, setErrKey] = useState(false);
  /* 自建控制面地址。**登录表必须有这一格**：登录核确实透传它
     （`crates/mesh/src/tailscale_login.rs`），但此前只有「TS 设置」表有控件，而那张表要求节点
     已存在 ⇒ 自建控制面用户的第一次登录必然打向官方 controlplane。 */
  const [controlUrl, setControlUrl] = useState('');
  const [errControl, setErrControl] = useState<string | null>(null);
  /* 该节点是否已有登录 state。切 auth_key 时必须先清掉它 —— tsnet 手上只要有有效 node key
     就不会去用 `auth_key`，于是「填了新 key、提交成功、身份一动不动」。 */
  const [hasState, setHasState] = useState<boolean | null>(accountActionsSupported && existingTs ? null : false);
  const [submitting, setSubmitting] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const [progressReadFailedAttempt, setProgressReadFailedAttempt] = useState<string | null>(null);
  type PendingLogin = { serverId: string; attemptId: string; source: 'pending' | 'transient' | 'main'; persisted: boolean };
  const [pending, setPending] = useState<PendingLogin | null>(null);
  const pendingRef = useRef<PendingLogin | null>(null);
  const submissionRef = useRef(0);
  const editedAfterSaveRef = useRef(false);
  const editRevisionRef = useRef(0);
  const progress = useTailscaleLoginProgressStore((s) => progressForLoginRequest(
    pending ? s.attempts[pending.serverId] : undefined,
    pending,
  ));

  /* 回显既有控制面地址；保存回包不能覆盖提交期间新输入的草稿。 */
  useEffect(() => {
    if (dirty) return;
    setControlUrl(existingTs?.tailscaleSettings?.controlUrl ?? '');
  }, [dirty, existingTs?.id, existingTs?.tailscaleSettings?.controlUrl]);

  /* 首次查询只驱动旧会话提示；提交 Auth Key 前会重新读取，读取失败中断，不操作旧会话。 */
  useEffect(() => {
    const id = savedServer?.id ?? existingTs?.id;
    if (!id) {
      setHasState(false);
      return;
    }
    let cancelled = false;
    setHasState(null);
    api.server
      .tailscaleStateExists([id], true)
      .then((map) => {
        if (!cancelled) setHasState(typeof map[id] === 'boolean' ? map[id] : null);
      })
      .catch(() => {
        if (!cancelled) setHasState(null);
      });
    return () => {
      cancelled = true;
    };
  }, [savedServer?.id, existingTs?.id]);

  const authUrl = validatedTailscaleAuthUrl(progress &&
    (progress.phase === 'awaitingAuth' || progress.phase === 'mainCore') ? progress.url : null);

  // Native progress can fail after start has handed off; that path has no pending submit
  // promise left to report it. The save claim belongs to this attempt, not an older node.
  useEffect(() => {
    if (!pending || !progress || !['failed', 'timedOut', 'cancelled'].includes(progress.phase)) return;
    setSubmitting(false);
    const reason = t(loginFailureReasonKey(progress.reason));
    setNotice({ tone: 'err', text: pending.persisted
      ? `${t('ts.loginSavedAttemptIncomplete')} ${reason}` : reason });
  }, [pending?.attemptId, pending?.persisted, progress?.phase, progress?.reason, t]);

  // Chrome can own the foreground while native login finishes. An event sent during WebView
  // suspension is not a durable receipt, so reconcile this exact request on return. A missing
  // receipt stays unknown; old or cancelled requests cannot complete a successor's panel.
  useEffect(() => {
    if (!pending) return;
    const { serverId, attemptId } = pending;
    let disposed = false;
    let revision = 0;
    const reconcile = async (): Promise<void> => {
      const request = ++revision;
      try {
        const receipt = await api.server.tailscaleLoginProgress(serverId, attemptId);
        if (disposed || request !== revision || pendingRef.current?.attemptId !== attemptId
          || !hasInstance(instanceId)) return;
        setProgressReadFailedAttempt(null);
        if (!receipt || (receipt.url && !validatedTailscaleAuthUrl(receipt.url))) return;
        const current = useTailscaleLoginProgressStore.getState().attempts[serverId];
        if (current?.attemptId !== attemptId) return;
        if (!useTailscaleLoginProgressStore.getState().apply(receipt)) return;
        setTailscaleAuthUrl(serverId, loginAttemptActive(receipt.phase) ? receipt.url ?? null : null);
        setTailscaleLoginInitiated(serverId, loginAttemptActive(receipt.phase));
        if (receipt.phase === 'authorized') setTailscaleLoginState(serverId, true);
      } catch {
        // A failed read proves nothing about authorization. Keep the event path live and
        // show that this attempt's result could not be checked on return from the browser.
        const current = useTailscaleLoginProgressStore.getState().attempts[serverId];
        if (!disposed && request === revision && pendingRef.current?.attemptId === attemptId
          && hasInstance(instanceId) && current?.attemptId === attemptId
          && loginAttemptActive(current.phase)) setProgressReadFailedAttempt(attemptId);
      }
    };
    const onVisible = (): void => { if (!document.hidden) void reconcile(); };
    window.addEventListener('focus', reconcile);
    document.addEventListener('visibilitychange', onVisible);
    void reconcile();
    return () => {
      disposed = true;
      window.removeEventListener('focus', reconcile);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [pending?.serverId, pending?.attemptId, hasInstance, instanceId,
    setTailscaleAuthUrl, setTailscaleLoginInitiated, setTailscaleLoginState]);

  // Authorization is a terminal receipt for this exact attempt. Detach before closing so the
  // unmount cleanup cannot send a late native cancel for an already authorized session.
  useEffect(() => {
    if (!pending?.persisted || progress?.phase !== 'authorized' || !hasInstance(instanceId)) return;
    const active = pendingRef.current;
    const current = useTailscaleLoginProgressStore.getState().attempts[pending.serverId];
    if (active?.serverId !== pending.serverId || active.attemptId !== pending.attemptId
      || current?.attemptId !== pending.attemptId || current.phase !== 'authorized') return;
    pendingRef.current = null;
    setPending(null);
    setSubmitting(false);
    setTailscaleAuthUrl(pending.serverId, null);
    setTailscaleLoginInitiated(pending.serverId, false);
    setNotice(undefined);
    toast.success(t('ts.authorizationComplete'));
    if (!editedAfterSaveRef.current) closeInstance(instanceId);
  }, [pending, progress?.phase, hasInstance, instanceId, closeInstance,
    setTailscaleAuthUrl, setTailscaleLoginInitiated, t]);

  const revokePendingLogin = (request: PendingLogin): void => {
    const current = useTailscaleLoginProgressStore.getState().attempts[request.serverId];
    if (current?.attemptId !== request.attemptId) return;
    // Revoke URL ownership before awaiting native close: a late event cannot reopen it.
    if (loginAttemptActive(current.phase))
      useTailscaleLoginProgressStore.getState().apply({ ...current, phase: 'cancelled', url: null });
    setTailscaleAuthUrl(request.serverId, null);
    setTailscaleLoginInitiated(request.serverId, false);
  };

  const cancelPendingLogin = async (request: PendingLogin, detached = false): Promise<boolean> => {
    const current = useTailscaleLoginProgressStore.getState().attempts[request.serverId];
    if (current?.attemptId === request.attemptId && current.phase === 'authorized') return true;
    revokePendingLogin(request);
    try {
      await api.server.tailscaleLoginCancel(request.serverId, request.attemptId);
      return true;
    } catch {
      if (detached) toast.error(t('ts.loginCancelFailed'));
      else setNotice({ tone: 'err', text: t('ts.loginCancelFailed') });
      return false;
    }
  };

  // 只计时本面板启动、仍在等待 URL 的瞬态核，绝不因等待主核 STATUS 去停止主核。
  useEffect(() => {
    if (mode !== 'browser' || pending?.source !== 'transient' || progress?.phase !== 'starting' || authUrl !== null) return;
    const timer = setTimeout(() => {
      const current = useTailscaleLoginProgressStore.getState().attempts[pending.serverId];
      if (current?.attemptId !== pending.attemptId || current.phase !== 'starting') return;
      useTailscaleLoginProgressStore.getState().apply({ ...current, phase: 'timedOut', reason: 'authorizationTimedOut', url: null });
      setTailscaleLoginInitiated(pending.serverId, false);
      void cancelPendingLogin(pending);
    }, TS_LOGIN_TIMEOUT_MS);
    return () => clearTimeout(timer);
  }, [mode, pending, progress?.phase, authUrl, setTailscaleLoginInitiated]);

  useEffect(() => () => {
    const active = pendingRef.current;
    if (!active) return;
    pendingRef.current = null;
    // Revoke locally synchronously; only this attempt owns its URL and initiated flag.
    void cancelPendingLogin(active, true);
  }, []);

  // 重试先完成旧 attempt 的取消，避免旧 cleanup 在新核启动后把它误杀。
  const discardPendingLogin = async (): Promise<boolean> => {
    const active = pendingRef.current;
    const current = active && useTailscaleLoginProgressStore.getState().attempts[active.serverId];
    if (active && current?.attemptId === active.attemptId) {
      if (!await cancelPendingLogin(active)) return false;
    }
    pendingRef.current = null;
    setPending(null);
    return true;
  };

  const openAuthUrl = async (url: string): Promise<void> => {
    await openLoginUrl(url, api.system.openExternal,
      () => setNotice({ tone: 'err', text: t('mobileSettings.about.openLinkFailed') }));
  };

  const copyAuthUrl = async (url: string): Promise<void> => {
    try {
      await copyLoginUrl(url, navigator.clipboard);
      setNotice(undefined);
      toast.success(t('ts.authUrlCopied'));
    } catch {
      setNotice({ tone: 'err', text: t('connections.copyFailed') });
    }
  };

  const closeAfterCancel = async (): Promise<void> => {
    const active = pendingRef.current;
    if (active && !await cancelPendingLogin(active)) return;
    pendingRef.current = null;
    setPending(null);
    closeInstance(instanceId);
  };

  const requestClose = (): void => {
    if (!dirty) {
      void closeAfterCancel();
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('ts.loginDiscardTitle'),
        message: t('ts.loginDiscardMsg'),
        confirmLabel: t('ts.loginDiscard'),
        danger: true,
        onConfirm: async () => {
          closeInstance(confirmId);
          await closeAfterCancel();
        },
      },
    });
  };

  const submit = async (): Promise<void> => {
    if (submitting) return;
    const submissionBase = savedServer ?? existingTs;
    const submittedName = !nameEdited && !submissionBase ? nextTsNodeName(meshNames) : name.trim();
    if (!submittedName) {
      setErrName(true);
      return;
    }
    setName(submittedName);
    if (mode === 'authkey' && authKey.trim() === '') {
      setErrKey(true);
      return;
    }
    /* 与「TS 设置」表同一判据（`domain/control-url.ts`，与 Rust 侧同源）——拦在提交前，
       光标还停在输入框旁边；后端那道 fail-closed 是下发前的兜底。 */
    const badControl = controlUrlReject(controlUrl);
    if (badControl) {
      setErrControl(badControl);
      return;
    }
    setErrControl(null);
    const submissionRevision = editRevisionRef.current;
    const submission = ++submissionRef.current;
    setSubmitting(true);
    setNotice(undefined);
    let persisted = false;
    try {
      if (!await discardPendingLogin()) return;
      const { server, persist } = planTsLoginSubmit({
        // The fresh state query occurs after prepare and before save. This preview flag does
        // not decide logout; executeTsLogin consumes only the fresh result below.
        existing: submissionBase, name: submittedName, mode, authKey, controlUrl, hasState: false,
        mintId: () => crypto.randomUUID(),
      });
      persisted = persist === 'none';
      const request: PendingLogin = { serverId: server.id, attemptId: crypto.randomUUID(), source: 'pending', persisted };
      pendingRef.current = request;
      setPending(request);
      useTailscaleLoginProgressStore.getState().begin(server.id, request.attemptId);
      setTailscaleAuthUrl(server.id, null);
      setTailscaleLoginInitiated(server.id, true);
      const stillActive = () => {
        const phase = useTailscaleLoginProgressStore.getState().attempts[server.id];
        return pendingRef.current?.attemptId === request.attemptId && hasInstance(instanceId)
          && phase?.attemptId === request.attemptId
          && phase.phase !== 'timedOut' && phase.phase !== 'failed' && phase.phase !== 'cancelled';
      };
      let startResult: Awaited<ReturnType<typeof api.server.tailscaleLogin>> | undefined;
      const outcome = await executeTsLogin({
        isActive: stillActive,
        prepare: () => api.server.tailscaleLoginPrepare(server.id, request.attemptId),
        verifyState: mode === 'authkey' && accountActionsSupported ? async () => {
          const states = await api.server.tailscaleStateExists([server.id]);
          if (typeof states[server.id] !== 'boolean') throw new Error('STATE_QUERY_UNAVAILABLE');
          if (stillActive()) setHasState(states[server.id]);
          return states[server.id];
        } : undefined,
        logout: () => api.server.tailscaleLogout(server.id, request.attemptId),
        save: async () => {
          if (persist === 'add') await api.server.add(server);
          else if (persist === 'update') await api.server.update(server);
        },
        onSaved: () => {
          // A successful write is retained across a later refresh or login failure.
          persisted = true;
          setSavedServer(server);
          if (pendingRef.current?.attemptId === request.attemptId) {
            const savedRequest = { ...pendingRef.current, persisted: true };
            pendingRef.current = savedRequest;
            setPending(savedRequest);
          }
          editedAfterSaveRef.current = editRevisionRef.current !== submissionRevision;
          if (stillActive() && !editedAfterSaveRef.current) setDirty(false);
        },
        refresh: async () => {
          if (persist === 'none') return;
          await loadConfig(true);
          const mirrored = useAppStore.getState().servers.find((s) => s.id === server.id);
          if (!mirrored || mirrored.name !== server.name
            || mirrored.tailscaleSettings?.controlUrl !== server.tailscaleSettings?.controlUrl
            || mirrored.tailscaleSettings?.authKey !== server.tailscaleSettings?.authKey
            || mirrored.tailscaleSettings?.sourceTag !== server.tailscaleSettings?.sourceTag) {
            throw new Error('TS_CONFIG_REFRESH_FAILED');
          }
        },
        start: async () => {
          startResult = await api.server.tailscaleLogin(server, { attemptId: request.attemptId, mode });
          return startResult;
        },
        cancel: async () => {
          const latest = useTailscaleLoginProgressStore.getState().attempts[server.id];
          if (latest?.attemptId === request.attemptId && latest.phase === 'authorized') return;
          await api.server.tailscaleLoginCancel(server.id, request.attemptId);
        },
      });
      if (!stillActive()) return;
      const current = useTailscaleLoginProgressStore.getState().attempts[server.id];
      if (outcome.phase === 'failed') {
        console.warn('[mobile-ts-login] authorization flow failed');
        if (current?.attemptId === request.attemptId && loginAttemptActive(current.phase)) {
          useTailscaleLoginProgressStore.getState().apply({ ...current, phase: 'failed', reason: outcome.reason, url: null });
        }
        setTailscaleLoginInitiated(server.id, false);
        const reason = outcome.reason === 'stateQueryFailed' ? t('ts.loginSessionReadFailed')
          : outcome.reason === 'saveFailed' ? t('ts.loginSaveFailed')
          : outcome.reason === 'configurationRefreshFailed' ? t('ts.loginRefreshFailed')
          : outcome.reason === 'logoutFailed' ? t('ts.loginLogoutFailed')
          : t(loginFailureReasonKey(outcome.reason));
        setNotice({ tone: 'err', text: persisted
          ? `${t('ts.loginSavedAttemptIncomplete')} ${reason}` : reason });
        return;
      }
      if (outcome.phase === 'cancelled' || !startResult) {
        setTailscaleLoginInitiated(server.id, false);
        if (persisted) setNotice({ tone: 'info', text: t('ts.loginSavedAttemptIncomplete') });
        return;
      }
      const next = { ...(pendingRef.current ?? request), source: startResult.reason === 'inMainCore' ? 'main' as const : 'transient' as const };
      pendingRef.current = next;
      setPending(next);
      // Producer progress can be ahead of this invocation receipt; never downgrade it.
      if (current?.attemptId === request.attemptId && current.phase === 'starting') {
        setNotice({ tone: 'info', text: t('ts.loginSubmitted') });
      }
    } catch {
      // Validation above has explicit inline errors. Unexpected IPC failures only expose a
      // stable localized stage, never raw URLs, auth keys, or filesystem paths.
      console.error('[mobile-ts-login] unexpected UI failure');
      setNotice({ tone: 'err', text: t('ts.loginAttemptFailed') });
    } finally {
      if (hasInstance(instanceId) && submissionRef.current === submission) setSubmitting(false);
    }
  };

  return (
    <FormSheet
      title={t('ts.loginTitle')}
      onRequestClose={requestClose}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={mode === 'browser' ? t('ts.openLogin') : t('ts.signIn')}
      submitDisabled={submitting || !loginActionsSupported}
      onSubmit={() => void submit()}
      notice={progressReadFailedAttempt === pending?.attemptId && progress
        && loginAttemptActive(progress.phase)
        ? { tone: 'err', text: t('ts.loginProgressReadFailed') } : notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mts-name">
          {t('ts.nodeName')}<span className="m-form-req" aria-hidden>*</span>
        </label>
        <input
          id="mts-name"
          className="m-form-input"
          value={name}
          disabled={submitting}
          onChange={(e) => {
            setName(e.target.value);
            setNameEdited(true);
            setErrName(false);
            setDirty(true);
            editedAfterSaveRef.current = true;
            editRevisionRef.current++;
          }}
        />
        {errName && <p className="m-form-err">{t('ts.errName')}</p>}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mts-control-url">
          {t('ts.controlUrl')}
        </label>
        <input
          id="mts-control-url"
          className="m-form-input mono"
          value={controlUrl}
          placeholder="https://controlplane.tailscale.com"
          onChange={(e) => {
            setControlUrl(e.target.value);
            setErrControl(null);
            setDirty(true);
            editedAfterSaveRef.current = true;
            editRevisionRef.current++;
          }}
        />
        <div className="m-form-hint"><MobileInfo title={t('ts.controlUrl')} summary={t('mobileHelp.tsControlUrl')} details={t('ts.controlUrlLoginHint')} /></div>
        {errControl && (
          <p className="m-form-err">
            {t('ts.errControlUrl')}
            {INVALID_NODE_REASON_KEY[errControl] ? ` — ${t(INVALID_NODE_REASON_KEY[errControl])}` : ''}
          </p>
        )}
      </div>

      <div className="m-form-row">
        <span className="m-form-label" id="mts-method">
          {t('ts.method')}
        </span>
        <div className="m-form-seg" role="group" aria-labelledby="mts-method">
          <button
            type="button"
            className={mode === 'browser' ? 'on' : ''}
            aria-pressed={mode === 'browser'}
            disabled={submitting}
            onClick={() => {
              void discardPendingLogin().then((ok) => { if (ok) { setMode('browser'); setDirty(true); editedAfterSaveRef.current = true; editRevisionRef.current++; } });
            }}
          >
            {t('ts.browserLogin')}
          </button>
          <button
            type="button"
            className={mode === 'authkey' ? 'on' : ''}
            aria-pressed={mode === 'authkey'}
            disabled={submitting}
            onClick={() => {
              void discardPendingLogin().then((ok) => { if (ok) { setMode('authkey'); setDirty(true); editedAfterSaveRef.current = true; editRevisionRef.current++; } });
            }}
          >
            {t('ts.authKey')}
          </button>
        </div>
      </div>

      {mode === 'authkey' && (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor="mts-authkey">
            {t('ts.authKeyLabel')}
            <span className="m-form-req" aria-hidden>
              *
            </span>
          </label>
          {/* Auth Key 是**凭据**：与私钥同一条纪律 —— 输入框遮住，且它绝不进任何日志或错误消息
              （错误提示只用阶段/稳定错误码，不渲染原始错误）。 */}
          <input
            id="mts-authkey"
            type="password"
            className="m-form-input mono"
            value={authKey}
            placeholder="YOUR_TAILSCALE_AUTH_KEY"
            onChange={(e) => {
              setAuthKey(e.target.value);
              setErrKey(false);
              setDirty(true);
              editedAfterSaveRef.current = true;
              editRevisionRef.current++;
            }}
          />
          <p className="m-form-hint">{t('ts.authkeyHint')}</p>
          {errKey && <p className="m-form-err">{t('ts.errKey')}</p>}
          {hasState === true && <div className="m-form-hint"><MobileInfo title={t('ts.authKeyLabel')} summary={t('mobileHelp.tsAuthKeySwitch')} details={t(accountActionsSupported ? 'ts.authKeySwitchLogoutNote' : 'ts.identityRetirementRequired')} /></div>}
          {hasState === null && <p className="m-form-hint">{t('ts.loginStateUnknown')}</p>}
        </div>
      )}

      {(mode === 'browser' || progress !== undefined) && (progress?.phase === 'authorized' ? (
        <p className="m-form-hint">{t('ts.loginMainCoreAuthorized')}</p>
      ) : authUrl !== null ? (
        <div className="m-form-row">
          <span className="m-form-label" id="mts-url">
            {t('ts.authUrlLabel')}
          </span>
          <p className="m-form-code" aria-labelledby="mts-url">
            {authUrl}
          </p>
          <div className="m-form-inline">
            <button
              type="button"
              className="m-form-btn"
              onClick={() => void copyAuthUrl(authUrl)}
            >
              {t('common.copy')}
            </button>
            {/* 自动打开已由 app-wiring 那条全局订阅做过一次；这颗是**重试入口**（见文件头）。 */}
            <button type="button" className="m-form-btn" onClick={() => void openAuthUrl(authUrl)}>
              {t('ts.openUrl')}
            </button>
          </div>
          <p className="m-form-hint">{t('ts.authUrlHint')}</p>
        </div>
      ) : progress?.phase === 'timedOut' || progress?.phase === 'failed' ? (
        <div className="m-form-row">
          {progress.phase === 'timedOut' && <p className="m-form-err">{t('ts.loginUrlTimeout')}</p>}
          <button
            type="button"
            className="m-form-btn"
            disabled={submitting}
            onClick={() => void submit()}
          >
            {t('ts.retryLogin')}
          </button>
        </div>
      ) : progress?.phase === 'preparingConnection' || progress?.phase === 'waitingForReady' ? (
        <p className="m-form-hint" role="status">{t(progress.phase === 'preparingConnection' ? 'prerequisite.preparingConnection' : 'prerequisite.waitingForReady')}</p>
      ) : progress?.phase === 'starting' ? (
        <p className="m-form-hint">{t('ts.awaitingUrl')}</p>
      ) : progress?.phase === 'mainCore' && !progress.reason ? (
        <p className="m-form-hint">{t('ts.loginMainCoreAwaiting')}</p>
      ) : (
        <p className="m-form-hint">{t('ts.browserHint')}</p>
      ))}
    </FormSheet>
  );
}
