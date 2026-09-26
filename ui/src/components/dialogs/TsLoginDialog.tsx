/** Tailscale nodes are saved before authorization. Request-scoped progress confirms Running,
 * cancellation awaits process reap, and retries retain the same saved node identity. */

import { useEffect, useRef, useState } from 'react';
import type { ServerConfig } from '@/contracts/types';
import { useTailscaleLoginProgressStore } from '@/store/use-tailscale-login-progress-store';
import { copyLoginUrl, loginAttemptActive, loginFailureReasonKey, openLoginUrl } from '@/domain/tailscale-login-progress';
import { useTranslation } from 'react-i18next';
import { useAppStore } from '@/store/app-store';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { Modal } from './Modal';
import { useDialogStore } from './dialog-store';
import { executeTsLogin, planTsLoginSubmit } from './ts-login-server';
import { TsLoginModeSwitch } from './TsLoginModeSwitch';
import { controlUrlReject } from '@/domain/control-url';
import { INVALID_NODE_REASON_KEY } from '@/domain/invalid-node-reason';
import { InfoIcon } from '@/components/InfoIcon';
import { validatedTailscaleAuthUrl } from '@/domain/tailscale-auth-url';

function TsIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M9 15l6-6M8 8a3 3 0 10-3 3M16 16a3 3 0 103 3" />
    </svg>
  );
}

export function TsLoginDialog({ serverId }: { serverId?: string }) {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const close = useDialogStore((s) => s.close);
  const servers = useAppStore((s) => s.servers);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const setTailscaleAuthUrl = useAppStore((s) => s.setTailscaleAuthUrl);
  const setTailscaleLoginInitiated = useAppStore((s) => s.setTailscaleLoginInitiated);

  // serverId 是本弹窗身兼「新建」与「编辑既有节点」的判据：带 id = 给该节点换 key / 换控制面，
  // 不带 = 新建（Tailscale 不再是单例，没有 id 时不猜、直接走新建路径——不回落 `.find(protocol===...)`，
  // 否则多节点时会把登录写进任意一个既有节点，重犯 node-edit-routing 那条缺陷）。
  const savedServer = useRef<ServerConfig | undefined>(undefined);
  const activeRequest = useRef<{ serverId: string; attemptId: string } | null>(null);
  const [saved, setSaved] = useState(Boolean(serverId));
  const existingTs = servers.find((s) => s.id === (serverId ?? savedServer.current?.id)) ?? savedServer.current;

  // 回显既有控制面地址（再次进入本弹窗时不该看起来像"没配过"）。
  useEffect(() => {
    setControlUrl(existingTs?.tailscaleSettings?.controlUrl ?? '');
  }, [existingTs?.id, existingTs?.tailscaleSettings?.controlUrl]);

  // 此查询只用于展示提示；AuthKey 提交会重新查询，失败时停止，不使用这里的缓存决定登出。
  useEffect(() => {
    const id = existingTs?.id;
    if (!id) {
      setHasState(false);
      return;
    }
    let cancelled = false;
    api.server
      .tailscaleStateExists([id])
      .then((map) => {
        if (!cancelled) setHasState(map[id] === true);
      })
      .catch(() => {
        if (!cancelled) setHasState(false);
      });
    return () => {
      cancelled = true;
    };
  }, [existingTs?.id]);

  const [mode, setMode] = useState<'browser' | 'authkey'>('browser');
  const [authKey, setAuthKey] = useState('');
  const [errKey, setErrKey] = useState(false);
  // 自建控制面地址。**登录弹窗必须有这个字段**：登录核确实透传它
  // （`crates/mesh/src/tailscale_login.rs`），但此前只有「TS 设置」弹窗有控件，
  // 而那个弹窗要求节点已存在 ⇒ 自建控制面用户的第一次登录必然打向官方 controlplane。
  const [controlUrl, setControlUrl] = useState('');
  const [errControl, setErrControl] = useState<string | null>(null);
  // 该节点是否已有登录 state。切 auth_key 时必须先清掉它 —— tsnet 手上只要有有效 node key
  // 就不会去用 `auth_key`，于是「填了新 key、提交成功、身份一动不动」。
  const [hasState, setHasState] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [pendingServerId, setPendingServerId] = useState<string | null>(null);
  const progress = useTailscaleLoginProgressStore((s) => pendingServerId ? s.attempts[pendingServerId] : undefined);
  const cachedAuthUrl = useAppStore((s) => pendingServerId ? s.tailscaleAuthUrls[pendingServerId] : undefined);
  const authUrl = validatedTailscaleAuthUrl( progress?.phase === 'awaitingAuth' ? progress.url :
    progress?.phase === 'mainCore' && !progress.reason ? cachedAuthUrl : null);
  const loginTimedOut = progress?.phase === 'timedOut' || progress?.phase === 'failed';
  const awaitingUrl = progress?.phase === 'starting';

  const discardPendingLogin = () => {
    const request = activeRequest.current;
    activeRequest.current = null;
    if (request) {
      const current = useTailscaleLoginProgressStore.getState().attempts[request.serverId];
      if (current?.attemptId === request.attemptId && loginAttemptActive(current.phase)) {
        useTailscaleLoginProgressStore.getState().apply({ ...current, phase: 'cancelled', url: null });
        setTailscaleLoginInitiated(request.serverId, false);
        setTailscaleAuthUrl(request.serverId, null);
        void api.server.tailscaleLoginCancel(request.serverId, request.attemptId).catch(() => {});
      }
    }
    setPendingServerId(null);
  };

  useEffect(() => () => {
    const request = activeRequest.current;
    activeRequest.current = null;
    if (!request) return;
    const s = useAppStore.getState();
    const current = useTailscaleLoginProgressStore.getState().attempts[request.serverId];
    if (current?.attemptId === request.attemptId && loginAttemptActive(current.phase)) {
      useTailscaleLoginProgressStore.getState().apply({ ...current, phase: 'cancelled', url: null });
      s.setTailscaleLoginInitiated(request.serverId, false);
      s.setTailscaleAuthUrl(request.serverId, null);
      void api.server.tailscaleLoginCancel(request.serverId, request.attemptId).catch(() => {});
    }
  }, []);

  const requestClose = () => {
    if (!dirty) {
      close();
      return;
    }
    open({
      kind: 'confirm',
      payload: {
        title: t('ts.loginDiscardTitle'),
        message: t('ts.loginDiscardMsg'),
        confirmLabel: t('ts.loginDiscard'),
        danger: true,
        onConfirm: () => {
          close();
          close();
        },
      },
    });
  };

  const handleSubmit = async () => {
    if (mode === 'authkey' && !authKey.trim()) {
      setErrKey(true);
      return;
    }
    // 与「TS 设置」弹窗同一判据（`domain/control-url.ts`，与 Rust 侧同源）——
    // 拦在保存前，光标还停在输入框旁边；后端那道 fail-closed 是下发前的兜底。
    const badControl = controlUrlReject(controlUrl);
    if (badControl) {
      setErrControl(badControl);
      return;
    }
    setErrControl(null);
    const { server, persist } = planTsLoginSubmit({
      existing: existingTs,
      mode,
      authKey,
      controlUrl,
      hasState,
      mintId: () => crypto.randomUUID(),
    });

    discardPendingLogin();
    const request = { serverId: server.id, attemptId: crypto.randomUUID() };
    activeRequest.current = request;
    useTailscaleLoginProgressStore.getState().begin(server.id, request.attemptId);
    setPendingServerId(server.id);
    setTailscaleAuthUrl(server.id, null);
    setTailscaleLoginInitiated(server.id, true);
    setSubmitting(true);
    const stillActive = () => activeRequest.current?.attemptId === request.attemptId;
    const outcome = await executeTsLogin({
      isActive: stillActive,
      prepare: () => api.server.tailscaleLoginPrepare(server.id, request.attemptId),
      verifyState: mode === 'authkey' ? async () => {
        const states = await api.server.tailscaleStateExists([server.id]);
        if (typeof states[server.id] !== 'boolean') throw new Error('STATE_QUERY_UNAVAILABLE');
        return states[server.id];
      } : undefined,
      logout: () => api.server.tailscaleLogout(server.id, request.attemptId),
      save: async () => {
        if (persist === 'add') await api.server.add(server);
        else if (persist === 'update') await api.server.update(server);
      },
      onSaved: () => {
        savedServer.current = server;
        if (stillActive()) { setSaved(true); setDirty(false); }
      },
      refresh: () => persist !== 'none' ? loadConfig(true) : Promise.resolve(),
      start: () => api.server.tailscaleLogin(server, { attemptId: request.attemptId, mode }),
      cancel: () => api.server.tailscaleLoginCancel(server.id, request.attemptId),
    });
    if (stillActive()) {
      if (outcome.phase === 'failed') {
        const current = useTailscaleLoginProgressStore.getState().attempts[server.id];
        if (current?.attemptId === request.attemptId && loginAttemptActive(current.phase)) {
          useTailscaleLoginProgressStore.getState().apply({ ...current, phase: 'failed', reason: outcome.reason, url: null });
        }
      }
      setSubmitting(false);
    }
  };

  const copyAuthUrl = async () => {
    try {
      if (!authUrl) throw new Error('AUTH_URL_UNAVAILABLE');
      await copyLoginUrl(authUrl, navigator.clipboard);
      toast.success(t('ts.authUrlCopied'));
    } catch {
      toast.error(t('ts.authUrlCopyFailed'));
    }
  };

  return (
    <Modal
      titleId="ts-dlg-title"
      title={t('ts.loginTitle')}
      onClose={requestClose}
      icon={<TsIcon />}
      className="entry-form-dlg"
      footer={
        <>
          {/* 提交中**不锁**「取消」：原型 `:2545` 的 ghost 钮无 disabled，且本仓此前四个弹窗锁、
              两个不锁（NodeDialog/SubDialog）—— 不是与原型的差，是实现自己两套。统一为不锁：
              提交卡住（IPC 无应答）时用户必须还能退出，否则弹窗成了死窗。 */}
          <button type="button" className="btn ghost" onClick={requestClose}>
            {t('common.cancel')}
          </button>
          <button type="button" className="btn flow" onClick={() => void handleSubmit()} disabled={submitting}>
            {submitting && <span className="spinner spin-inline" style={{ marginRight: 6 }} />}
            {mode === 'browser' ? t('ts.openLogin') : t('ts.signIn')}
          </button>
        </>
      }
    >
      <div className="fld">
        <label className="fld-l fld-l-info" htmlFor="ts-login-control-url">
          <span>{t('ts.controlUrl')}</span>
          <InfoIcon tip={t('ts.controlUrlLoginHint')} />
        </label>
        <input
          id="ts-login-control-url"
          className="input mono"
          value={controlUrl}
          onChange={(e) => {
            setControlUrl(e.target.value);
            setErrControl(null);
            setDirty(true);
          }}
          placeholder="https://controlplane.tailscale.com"
        />
        {errControl && (
          <div className="err-line">
            {t('ts.errControlUrl')}
            {INVALID_NODE_REASON_KEY[errControl]
              ? ` — ${t(INVALID_NODE_REASON_KEY[errControl])}`
              : ''}
          </div>
        )}
      </div>

      <div className="fld">
        <label className="fld-l">{t('ts.method')}</label>
        <TsLoginModeSwitch mode={mode} submitting={submitting} label={t('ts.method')}
          browserLabel={t('ts.browserLogin')} authkeyLabel={t('ts.authKey')}
          onChange={(next) => { setMode(next); discardPendingLogin(); setDirty(true); }} />
      </div>

      {saved && <div className="card-sub" role="status">{t('ts.nodeSaved')}</div>}
      {progress?.phase === 'authorized' && <div className="card-sub" role="status">{t('ts.authorizationComplete')}</div>}
      {progress?.phase === 'mainCore' && <div className="card-sub" role="status">{t(progress.reason === 'configurationPending' ? 'ts.loginInMainCoreNeedsRestart' : 'ts.mainCoreAwaitingAuthorization')}</div>}
      {mode === 'authkey' && progress && loginAttemptActive(progress.phase) && <div className="card-sub" role="status">{t('ts.authorizing')}</div>}
      {mode === 'authkey' && loginTimedOut && <div className="dlg-err" role="alert">{t(saved ? 'ts.authorizationIncomplete' : 'ts.nodeSaveFailed')} {t(loginFailureReasonKey(progress?.reason))}</div>}
      {mode === 'browser' ? (
        authUrl ? (
          <div className="fld">
            <label className="fld-l" htmlFor="ts-auth-url">
              {t('ts.authUrlLabel')}
            </label>
            <div style={{ display: 'flex', gap: 8 }}>
              <input
                id="ts-auth-url"
                className="input mono"
                value={authUrl}
                readOnly
                style={{ flex: 1 }}
                onFocus={(e) => e.currentTarget.select()}
              />
              <button
                type="button"
                className="btn ghost sm"
                onClick={() => void copyAuthUrl()}
              >
                {t('common.copy')}
              </button>
              <button
                type="button"
                className="btn ghost sm"
                onClick={() => void openLoginUrl(authUrl, api.system.openExternal, () => toast.error(t('ts.browserOpenFailed')))}
              >
                {t('ts.openUrl')}
              </button>
            </div>
            <div className="card-sub" style={{ marginTop: 6 }}>
              {t('ts.authUrlHint')}
            </div>
          </div>
        ) : progress?.phase === 'authorized' ? null : loginTimedOut ? (
          <div className="fld">
            <div className="dlg-err">
              {t(saved ? 'ts.authorizationIncomplete' : 'ts.nodeSaveFailed')} {t(loginFailureReasonKey(progress?.reason))}
            </div>
            <button
              type="button"
              className="btn ghost sm"
              style={{ marginTop: 8 }}
              onClick={() => void handleSubmit()}
              disabled={submitting}
            >
              {t('ts.retryLogin')}
            </button>
          </div>
        ) : awaitingUrl ? (
          <div className="card-sub" style={{ lineHeight: 1.6, display: 'flex', alignItems: 'center', gap: 8 }}>
            <span className="spinner spin-inline" />
            {t('ts.awaitingUrl')}
          </div>
        ) : (
          <div className="card-sub" style={{ lineHeight: 1.6 }}>
            {t('ts.browserHint')}
          </div>
        )
      ) : (
        <>
          <div className="fld">
            <label className="fld-l fld-l-info" htmlFor="ts-authkey">
              <span>{t('ts.authKeyLabel')}</span>
              <InfoIcon tip={t('ts.authkeyHint')} />
            </label>
            <input
              id="ts-authkey"
              className="input mono"
              value={authKey}
              onChange={(e) => {
                setAuthKey(e.target.value);
                setErrKey(false);
                setDirty(true);
              }}
              placeholder="YOUR_TAILSCALE_AUTH_KEY"
            />
            {errKey && <div className="err-line">{t('ts.errKey')}</div>}
            <div className="card-sub" style={{ marginTop: 6 }}>
              {t('ts.authKeyEphemeralHint')}
            </div>
            {hasState && (
              <div className="card-sub" style={{ marginTop: 6 }}>
                {t('ts.authKeySwitchLogoutNote')}
              </div>
            )}
          </div>
        </>
      )}
    </Modal>
  );
}

export default TsLoginDialog;
