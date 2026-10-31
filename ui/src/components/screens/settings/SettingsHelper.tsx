/**
 * SettingsHelper —— 提权助手子页（原型 [data-sec="helper"] L2371-2420）。
 *
 * 两块：
 *  1. helper-card：原型定义 5 态（installed / none / installing / needs-btm / needs-upgrade）+
 *     真实运行时另需的 3 态（checking / unsupported / needs-repair，原型没有对应模板——按同一视觉语言
 *     借用 none/needs-upgrade 的 markup 骨架续写，非另起设计）
 *  2. 回退方案：异常自动恢复（纯说明行，无状态徽章——理由见该段注释）
 *
 * 接 helperApi.getStatus / install / uninstall + helperApi.onUpgradeable。
 * 原型 .hc-plat 平台切换 tab（mac/win/lin）是静态 demo 用来一次性展示三平台外观的道具——
 * 真实 app 只有一个当前平台，不需要「切换看别的平台」，故不搬。
 */

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { revealOnToggle } from '@/components/reveal';
import { helperApi, type HelperActionErrorCode, type HelperInstallMode } from '@/ipc/api-client';
import { useNavStore } from '@/store/nav-store';
import { toast } from '@/lib/error-handler';
import type { HelperStatus } from '@/contracts/types/runtime';
import { Phead, SetBlock, SetRow, Pill, Dot, Button, Spinner, FeedbackNotice } from './Primitives';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { helperActionErrorText } from '@/domain/action-error-text';
import { coreAdmissionIsDamage, coreAdmissionMessageKey } from '@/domain/core-admission';
import { useDialogStore } from '@/components/dialogs/dialog-store';
import { shellPlatformFromDataOs } from './settings-logic';
import { beginHelperInstall, clearHelperInstallFeedback, completeHelperInstall, helperInstallSnapshot, subscribeHelperInstall } from './helper-install-session';

/** 本页唯一的原地二次确认项（原型 :4234 `helper-uninstall`）。 */
const UNINSTALL_KEY = 'helper-uninstall';

/** Helper 卡片派生态（消费 HelperStatus）。 */
type HelperState =
  | 'checking'
  | 'unsupported'
  | 'installed'
  | 'none'
  | 'cleanup-pending'
  | 'installing'
  | 'needs-upgrade'
  | 'needs-btm'
  | 'needs-repair'
  | 'admission-unavailable';

/**
 * `status === null` 必须自成一态（'checking'），**不能并进 'unsupported'**：
 * null 出现在「首帧还没拉到」与「getStatus 失败」两种情况，把它们说成「本平台不支持」是编造事实
 * ——每个用户进这一页的第一帧都会看到一句关于自己系统的假话。
 */
function deriveState(s: HelperStatus | null, cleanupPending = false): HelperState {
  if (!s) return 'checking';
  if (!s.supported) return 'unsupported';
  if (cleanupPending) return 'cleanup-pending';
  if (s.selectedCore?.admission && !coreAdmissionIsDamage(s.selectedCore.admission)) return 'admission-unavailable';
  if (s.upgradeable) return 'needs-upgrade';
  // backgroundDisabled（macOS BTM 登录项被禁）比 needsRepair 更具体，需先判（contract: runtime.ts:167）。
  if (s.backgroundDisabled) return 'needs-btm';
  if (s.needsRepair) return 'needs-repair';
  if (s.ready) return s.installed ? 'installed' : 'needs-repair';
  if (s.installed) return 'needs-repair';
  return 'none';
}

export default function SettingsHelper() {
  // 卡片正文已全部走 `helper.*`（此前是硬编码中文，en/ru/fa 用户看到的也是中文）：状态标题类接既有键，
  // 步骤/说明/按钮等本页独有的句子本批新立键，五语种齐备。
  const { t } = useTranslation();
  const [status, setStatus] = useState<HelperStatus | null>(null);
  const [state, setState] = useState<HelperState>('checking');
  const [busy, setBusy] = useState(helperInstallSnapshot().busy);
  const [installFailure, setInstallFailure] = useState<HelperActionErrorCode | null>(helperInstallSnapshot().failure);
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const [recoveryNotice, setRecoveryNotice] = useState<'partial' | 'unsupported' | 'pending' | null>(null);
  const cleanupPending = useRef(false);
  const installInFlight = useRef(false);
  const migrationConfirmation = useRef<string | null>(null);
  const currentStatus = useRef<HelperStatus | null>(null);
  const { armed, confirmTwice } = useConfirmTwice();
  const confirmingUninstall = armed === UNINSTALL_KEY;

  // Saved click/confirmation closures must consult the latest accepted
  // readback, not the status captured when a dialog was opened.
  function migrationAdmissionAvailable() {
    // Cleanup debt is a presentation priority, not permission to bypass a
    // newly observed admission refusal.
    const latest = deriveState(currentStatus.current);
    return latest !== 'checking' && latest !== 'unsupported' && latest !== 'admission-unavailable';
  }

  function publishStatus(s: HelperStatus) {
    currentStatus.current = s;
    setStatus(s);
    setState(deriveState(s, cleanupPending.current));
    if (!migrationAdmissionAvailable() && migrationConfirmation.current) {
      useDialogStore.getState().closeInstance(migrationConfirmation.current);
      migrationConfirmation.current = null;
    }
  }

  useEffect(() => {
    let alive = true;
    let readGeneration = 0;
    let wasInstalling = false;
    const readStatus = (force = false) => {
      const generation = ++readGeneration;
      void helperApi.getStatus(force).then((s) => {
        if (!alive || generation !== readGeneration) return;
        publishStatus(s);
      }).catch((err) => {
        if (alive && generation === readGeneration) console.error('[SettingsHelper] getStatus failed:', err);
      });
    };
    const offInstall = subscribeHelperInstall(({ busy: installing, failure }) => {
      const settled = wasInstalling && !installing;
      wasInstalling = installing;
      setBusy(installing);
      setInstallFailure(failure);
      // The request may have started in a now-unmounted page. Read current status
      // for this surviving page; never infer ready from success or retain diagnostics.
      if (settled) readStatus(true);
    });
    // Failure stays checking, not a fabricated unsupported/absent status.
    readStatus();
    const off = helperApi.onUpgradeable(() => readStatus());
    return () => {
      alive = false;
      readGeneration++;
      offInstall();
      off();
      if (migrationConfirmation.current) {
        useDialogStore.getState().closeInstance(migrationConfirmation.current);
        migrationConfirmation.current = null;
      }
    };
  }, []);

  /**
   * 以下三个动作都经 `onClick={install}` 这类形式挂到按钮上 —— React **丢弃**返回的 promise，
   * 故每个都必须自带 catch，否则 helperApi 一旦 reject 就是没人能接的未捕获 rejection：
   * 用户只看到按钮转一圈回到原样，不知道失败了、更不知道为什么。
   */
  function reportFailure(title: string) {
    toast.error(title, helperActionErrorText('failed', t));
  }

  /** 安装/升级/修复共用（三态按钮都指向它）。 */
  function install() {
    return runInstall();
  }

  async function runInstall(mode?: HelperInstallMode) {
    if (mode === 'stopAndUpgrade' && !migrationAdmissionAvailable()) return;
    if (installInFlight.current) return;
    const request = beginHelperInstall();
    if (request === null) return;
    installInFlight.current = true;
    if (migrationConfirmation.current) {
      useDialogStore.getState().closeInstance(migrationConfirmation.current);
      migrationConfirmation.current = null;
    }
    let failure: HelperActionErrorCode | null = 'failed';
    try {
      const r = mode === undefined ? await helperApi.install() : await helperApi.install(mode);
      setRecoveryNotice(previous => !r.recovery ? (r.success ? null : previous)
        : r.errorCode === 'recoveryRequired' || r.recovery.user === 'pending' ? 'pending'
        : r.recovery.user === 'unsupported' ? 'unsupported'
        : !r.recovery.helperComplete ? 'partial' : null);
      failure = r.success ? null : r.errorCode ?? 'failed';
      if (r.success) cleanupPending.current = false;
      publishStatus(r.status);
      // W10：Rust 侧把所有失败（用户取消/脚本失败/二进制缺失）都装进 success:false 的 ok 应答——
      // 不在这里读出来，任何失败都是零反馈的「点击安装无反应」。信封不 reject，catch 永远等不到它。
      if (!r.success) {
        toast.error(t('helper.installFail'), helperActionErrorText(r.errorCode, t));
      }
    } catch (err) {
      console.error('[SettingsHelper] install failed:', err);
      reportFailure(t('helper.installFail'));
    } finally {
      installInFlight.current = false;
      completeHelperInstall(request, failure);
    }
  }

  // This is a separate explicit action. An ordinary install failure never selects it.
  function stopAndUpgrade() {
    if (!migrationAdmissionAvailable() || busy || installInFlight.current || helperInstallSnapshot().busy || !['mac', 'lin'].includes(shellPlatformFromDataOs() ?? '') || installFailure !== 'upgradeDeferred' || helperInstallSnapshot().failure !== 'upgradeDeferred') return;
    const confirmedPlatform = shellPlatformFromDataOs();
    const dialogs = useDialogStore.getState();
    if (migrationConfirmation.current && dialogs.hasInstance(migrationConfirmation.current)) return;
    const instanceId = dialogs.open({
      kind: 'confirm',
      payload: {
        title: t(confirmedPlatform === 'lin' ? 'helper.linuxStopAndUpgradeTitle' : 'helper.stopAndUpgradeTitle'),
        message: t(confirmedPlatform === 'lin' ? 'helper.linuxStopAndUpgradeWarning' : 'helper.stopAndUpgradeWarning'),
        confirmLabel: t('helper.stopAndUpgradeConfirm'),
        danger: true,
        onCancel: () => {
          if (migrationConfirmation.current === instanceId) migrationConfirmation.current = null;
        },
        onConfirm: () => {
          if (migrationConfirmation.current !== instanceId || !useDialogStore.getState().hasInstance(instanceId)) return;
          useDialogStore.getState().closeInstance(instanceId);
          migrationConfirmation.current = null;
          if (!migrationAdmissionAvailable() || shellPlatformFromDataOs() !== confirmedPlatform || !['mac', 'lin'].includes(confirmedPlatform ?? '') || installInFlight.current || helperInstallSnapshot().busy || helperInstallSnapshot().failure !== 'upgradeDeferred') return;
          void runInstall('stopAndUpgrade');
        },
      },
    });
    migrationConfirmation.current = instanceId;
  }

  /**
   * 卸载提权助手 —— **破坏性操作**，确认闸门必须真生效。
   *
   * 最早写成 `if (!window.confirm(...)) return;`：Tauri 下 confirm 返 Promise（恒 truthy），
   * 取消与确定等价 ⇒ 实为「无确认直接卸载」。后改自绘 `ConfirmDialog`，2026-07-29 再统一到
   * 原型的原地二次点击（:4234 `helper-uninstall` → `confirmTwice(t, '卸载提权助手？', …)`）。
   */
  function uninstall() {
    confirmTwice(UNINSTALL_KEY, () => {
      void (async () => {
        setBusy(true);
        try {
          const r = await helperApi.uninstall();
          if (r.success) { cleanupPending.current = false; clearHelperInstallFeedback(); }
          else if (r.errorCode === 'partialCleanup') cleanupPending.current = true;
          publishStatus(r.status);
          // W10 同族：卸载的失败/取消同样装在信封里，不读就是「点了没反应」。
          if (!r.success) {
            toast.error(t('helper.uninstallFail'), helperActionErrorText(r.errorCode, t));
          }
        } catch (err) {
          // `onClick={uninstall}` 丢弃返回值 ⇒ 不在这里 catch 就是没人能接的 rejection，用户零反馈。
          console.error('[SettingsHelper] uninstall failed:', err);
          reportFailure(t('helper.uninstallFail'));
        } finally {
          setBusy(helperInstallSnapshot().busy);
        }
      })();
    });
  }

  async function recheck() {
    setBusy(true);
    try {
      const s = await helperApi.getStatus(true);
      publishStatus(s);
    } catch (err) {
      console.error('[SettingsHelper] recheck failed:', err);
      reportFailure(t('helper.statusCheckFail'));
    } finally {
      setBusy(helperInstallSnapshot().busy);
    }
  }

  /**
   * 无「打开登录项设置」IPC 命令：best-effort 复制设置路径，供用户手动前往。
   *
   * 路径本身也走 i18n（`helper.loginItemsPath`）：它要与用户那台 macOS「系统设置」里**实际显示的**
   * 路径逐字对上才有用，照抄中文给英文系统的用户等于给了一串对不上的字。界面语言通常跟随系统语言，
   * 故按界面语言给是当前能做到的最接近的口径。同一串也用在上面的 `helper.btmDesc` 里。
   */
  async function copyLoginItemsHint() {
    try {
      await navigator.clipboard.writeText(t('helper.loginItemsPath'));
    } catch {
      // 剪贴板 API 不可用（非安全上下文）：静默兜底
    }
  }

  // These are observations about service readiness and selected installed content.
  // The status API supplies no attestation of the currently running core identity.
  const observedBoolean = (value: unknown) => typeof value === 'boolean' ? value : null;
  const protocol = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
  const buildId = (value: unknown) => typeof value === 'string' && /^[A-Za-z0-9._+-]{1,96}$/.test(value) ? value : null;
  const digest = (value: unknown) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value) ? value : null;
  const coreState = status?.selectedCore?.state;
  const observedCoreState = coreState === 'present' || coreState === 'absent' || coreState === 'invalid' || coreState === 'unavailable' ? coreState : null;
  const diagnostics = status ? {
    helper: {
      supported: observedBoolean(status.supported), installed: observedBoolean(status.installed),
      ready: observedBoolean(status.ready), loaded: observedBoolean(status.loaded),
      upgradeable: observedBoolean(status.upgradeable), needsRepair: observedBoolean(status.needsRepair),
      backgroundDisabled: observedBoolean(status.backgroundDisabled), pathMismatch: observedBoolean(status.pathMismatch),
      version: protocol(status.version), expectedProtocolVersion: protocol(status.expectedProtocolVersion),
      helperBuildId: buildId(status.helperBuildId), expectedBuildId: buildId(status.expectedBuildId),
    },
    selectedCore: status.selectedCore ? {
      id: digest(status.selectedCore.id), providerFingerprint: digest(status.selectedCore.providerFingerprint),
      state: observedCoreState,
    } : null,
  } : null;
  const booleanText = (value: unknown) => t(value === true ? 'helper.checkYes' : value === false ? 'helper.checkNo' : 'helper.checkUnknown');
  async function copyDiagnostics() {
    if (!diagnostics) return;
    setCopyState('idle');
    try {
      await navigator.clipboard.writeText(JSON.stringify(diagnostics, null, 2));
      setCopyState('copied');
    } catch {
      setCopyState('failed');
    }
  }

  // A typed deferral changes the existing primary action in place. It never
  // retries ordinary install or selects migration without the explicit confirmation.
  const migrationAvailable = migrationAdmissionAvailable() && ['mac', 'lin'].includes(shellPlatformFromDataOs() ?? '') && installFailure === 'upgradeDeferred';

  // A disappeared SCM service does not prove the protected support tree is clean.
  // Keep the same explicit-confirmation cleanup action in both recovery and none
  // states, including after reopening this page; status refresh cannot erase it.
  const uninstallButton = (
    <Button
      variant="ghost"
      size="sm"
      className={confirmingUninstall ? 'confirming' : undefined}
      onClick={uninstall}
      disabled={busy}
      style={{ color: 'hsl(var(--err))' }}
    >
      <span>
        {confirmingUninstall ? t('helper.uninstallConfirmAgain') : t('helper.uninstall')}
      </span>
    </Button>
  );

  return (
    <section className="screen" data-sec="helper">
      <Phead title={t('helper.title')} sub={t('helper.pageSub')} />

      <div className="card helper-card" id="helper-card" data-state={state}>
        <div className="hc-head">
          <span className="hc-ic">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
              <path d="M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" />
              <path d="M9 12l2 2 4-4" />
            </svg>
          </span>
          <div className="hc-tx">
            <b id="hc-daemon-name">com.polaris.helper</b>
            <div id="hc-daemon-desc">{t('helper.daemonDesc')}</div>
          </div>
        </div>

        <div className="hc-body">
          {installFailure && (
            <div data-testid="helper-install-failure" className="helper-state">
              <FeedbackNotice title={t('helper.installFail')} detail={helperActionErrorText(installFailure, t)}
                hint={t(shellPlatformFromDataOs() === 'lin' ? 'helper.linuxForwardOnlyRecovery' : 'helper.failureNextStep')} announce={false} actions={<>
              {/* Recovery remains status-only. States already exposing recheck below
                  keep their single action; none/repair/upgrade failures need it here. */}
              {!['checking', 'installed', 'cleanup-pending', 'needs-btm', 'admission-unavailable'].includes(state) && (
                <Button variant="ghost" size="sm" onClick={recheck} disabled={busy}>
                  <span>{t('helper.recheck')}</span>
                </Button>
              )}
              {migrationAvailable && (
                <Button variant="ghost" size="sm" disabled={busy} onClick={stopAndUpgrade}>
                  {t('helper.stopAndUpgradeAction')}
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={() => useNavStore.getState().navigate('logs')}>
                {t('helper.viewLogsAction')}
              </Button>
              </>} />
            </div>
          )}
          {state === 'admission-unavailable' && (
            <div className="helper-state" data-s="admission-unavailable" role="status">
              <p>{t(coreAdmissionMessageKey(status?.selectedCore?.admission))}</p>
              {status?.selectedCore?.admission === 'UnsupportedCapability' && (
                <Button variant="ghost" size="sm" onClick={() => useNavStore.getState().setSettingsScreen('update')}>
                  <span>{t('settings.about.checkUpdate')}</span>
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={recheck} disabled={busy}><span>{t('helper.recheck')}</span></Button>
            </div>
          )}
          {state === 'checking' && (
            <div className="helper-state" data-s="checking">
              <div className="hc-status">
                <Spinner />
                <b>{t('helper.statusChecking')}</b>
              </div>
              <div style={{ display: 'flex', gap: 9, marginTop: 14 }}>
                <Button variant="ghost" size="sm" onClick={recheck} disabled={busy}>
                  <span>{t('helper.recheck')}</span>
                </Button>
              </div>
            </div>
          )}

          {state === 'unsupported' && (
            <div className="helper-state" data-s="unsupported">
              <div className="hc-status">
                <Dot variant="idle" />
                {/* 旧文案「提权助手尚未接入」写于 HelperStatusSnapshot 还没有 supported 字段的年代
                    （那时 `!s.supported` 恒真，人人可见，故只能写成"没接线"）。后端早已返真值
                    （runtime/helper.rs::platform_supported → mac/win/linux 全 true），本态如今**只有真正
                    不支持的平台**才到得了，继续说"尚未接入"就成了新的错报。首帧/检测失败已由
                    'checking' 态承接，不再挤在这里。 */}
                <b>{t('helper.unsupportedTitle')}</b>
                <span className="card-sub">
                  {t('helper.unsupportedDesc')}
                </span>
              </div>
            </div>
          )}

          {(state === 'installed' || state === 'cleanup-pending') && (
            <div className="helper-state" data-s={state}>
              <div className="hc-status">
                <Dot variant={state === 'cleanup-pending' ? 'err' : 'ok'} />
                <b>{t(state === 'cleanup-pending' ? 'helper.uninstallFail' : 'helper.statusInstalled')}</b>
              </div>
              {state === 'cleanup-pending' && <div className="card-sub">{t('helper.actionError.partialCleanup')}</div>}
              <div style={{ display: 'flex', gap: 9, marginTop: 14, flexWrap: 'wrap' }}>
                <Button variant="ghost" size="sm" onClick={recheck} disabled={busy}>
                  <span>{t('helper.recheck')}</span>
                </Button>
                {state === 'installed' && <Button variant="ghost" size="sm" onClick={install} disabled={busy}>
                  <span>{t('helper.restoreOfflineAction')}</span>
                </Button>}
                {uninstallButton}
              </div>
            </div>
          )}

          {state === 'none' && (
            <div className="helper-state" data-s="none">
              <div className="hc-status">
                <Dot variant="idle" />
                <b>{t('helper.statusNotInstalled')}</b>
              </div>
              <div className="card-sub">{t('helper.noneDesc')}</div>
              <div className="hc-steps">
                <div className="hc-step">
                  <span className="st-n">1</span>
                  <span>{t('helper.stepCopyBinary')}</span>
                </div>
                <div className="hc-step">
                  <span className="st-n">2</span>
                  <span>{t('helper.stepRegisterDaemon')}</span>
                </div>
              </div>
              <Button variant="flow" onClick={migrationAvailable ? stopAndUpgrade : install} disabled={busy}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
                  <path d="M12 3v11M8 10l4 4 4-4" />
                </svg>
                <span>{migrationAvailable ? t('helper.stopAndUpgradeAction') : t('helper.installAction')}</span>
              </Button>
              {uninstallButton}
            </div>
          )}

          {state === 'installing' && (
            <div className="helper-state" data-s="installing">
              <div className="hc-status">
                <Spinner />
                <b>{t('helper.installing')}</b>
              </div>
              <div className="card-sub">{t('helper.installingDesc')}</div>
            </div>
          )}

          {state === 'needs-upgrade' && (
            <div className="helper-state" data-s="needs-upgrade">
              <div className="hc-status">
                <Dot variant="err" style={{ background: 'hsl(var(--warn))' }} />
                <b style={{ color: 'hsl(var(--warn))' }}>{t('helper.statusUpgradeable')}</b>
                <Pill variant="warn" style={{ marginLeft: 'auto' }}>
                  {t(status?.selectedCore?.state === 'absent' ? 'helper.coreState.absent'
                    : status?.version != null && status.version < status.expectedProtocolVersion ? 'helper.protocolUpgradeNeeded' : 'helper.statusUpgradeable')}
                </Pill>
              </div>
              <div className="card-sub">{t('helper.upgradeDescCard')}</div>
              <div style={{ display: 'flex', gap: 9, marginTop: 14 }}>
                <Button variant="flow" size="sm" onClick={migrationAvailable ? stopAndUpgrade : install} disabled={busy}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
                    <path d="M12 3v11M8 10l4 4 4-4" />
                  </svg>
                  <span>{migrationAvailable ? t('helper.stopAndUpgradeAction') : status?.selectedCore?.state === 'absent' ? t('helper.installSelectedCoreAction') : t('helper.upgradeHelperAction')}</span>
                </Button>
              </div>
            </div>
          )}

          {state === 'needs-btm' && (
            <div className="helper-state" data-s="needs-btm">
              <div className="hc-status">
                <Dot variant="err" />
                <b id="hc-btm-title" style={{ color: 'hsl(var(--warn))' }}>
                  {t('helper.btmTitle')}
                </b>
              </div>
              <div className="card-sub" id="hc-btm-desc">
                {t('helper.btmDesc')}
              </div>
              <div style={{ display: 'flex', gap: 9, marginTop: 14, flexWrap: 'wrap' }}>
                <Button variant="flow" size="sm" onClick={copyLoginItemsHint} disabled={busy}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
                    <rect x="9" y="9" width="11" height="11" rx="2" />
                    <path d="M5 15V5a2 2 0 012-2h10" />
                  </svg>
                  <span id="hc-btm-btn">{t('helper.btmCopyPath')}</span>
                </Button>
                <Button variant="ghost" size="sm" onClick={recheck} disabled={busy}>
                  <span>{t('helper.recheck')}</span>
                </Button>
              </div>
            </div>
          )}

          {state === 'needs-repair' && (
            <div className="helper-state" data-s="needs-repair">
              <div className="hc-status">
                <Dot variant="err" />
                <b style={{ color: 'hsl(var(--warn))' }}>{t('helper.statusNeedsRepair')}</b>
              </div>
              <div className="card-sub">{t(status?.selectedCore?.admission && coreAdmissionIsDamage(status.selectedCore.admission) ? 'helper.coreAdmission.damaged' : 'helper.repairDesc')}</div>
              <div style={{ display: 'flex', gap: 9, marginTop: 14 }}>
                <Button variant="flow" size="sm" onClick={migrationAvailable ? stopAndUpgrade : install} disabled={busy}>
                  <span>{migrationAvailable ? t('helper.stopAndUpgradeAction') : t('helper.repairAction')}</span>
                </Button>
              </div>
            </div>
          )}
          {recoveryNotice && <p role="status" className="card-sub" data-recovery-state={recoveryNotice}>
            {t(recoveryNotice === 'partial' ? 'helper.recoveryPartial' : recoveryNotice === 'unsupported' ? 'helper.recoveryUserUnsupported' : 'helper.recoveryPending')}
          </p>}
          <details className="card-sub" data-testid="helper-diagnostics" style={{ marginTop: 16, minWidth: 0, overflowWrap: 'anywhere' }} onToggle={revealOnToggle}>
            <summary>{t('helper.diagnosticsTitle')}</summary>
            {status && (
              <div data-testid="helper-checks" style={{ marginTop: 12 }}>
                <b>{t('helper.checksTitle')}</b>
                <dl>
                  <dt>{t('helper.checkInstallation')}</dt><dd>{booleanText(status.installed)}</dd>
                  <dt>{t('helper.checkReady')}</dt><dd>{booleanText(status.ready)}</dd>
                  <dt>{t('helper.checkLoaded')}</dt><dd>{booleanText(status.loaded)}</dd>
                  <dt>{t('helper.selectedCoreState')}</dt><dd>{observedCoreState ? t(`helper.coreState.${observedCoreState}`) : t('helper.checkUnknown')}</dd>
                </dl>
                <p>{t('helper.checkScope')}</p>
              </div>
            )}
            <p>{t('helper.diagnosticsScope')}</p>
            <dl style={{ overflowWrap: 'anywhere' }}>
              <dt>{t('helper.observedBuildId')}</dt><dd className="mono">{diagnostics?.helper.helperBuildId ?? t('helper.checkUnknown')}</dd>
              <dt>{t('helper.expectedBuildId')}</dt><dd className="mono">{diagnostics?.helper.expectedBuildId ?? t('helper.checkUnknown')}</dd>
              <dt>{t('helper.observedProtocol')}</dt><dd>{diagnostics?.helper.version ?? t('helper.checkUnknown')}</dd>
              <dt>{t('helper.expectedProtocol')}</dt><dd>{diagnostics?.helper.expectedProtocolVersion ?? t('helper.checkUnknown')}</dd>
              <dt>{t('helper.selectedCoreId')}</dt><dd className="mono">{diagnostics?.selectedCore?.id ?? t('helper.checkUnknown')}</dd>
              <dt>{t('helper.selectedCoreProvider')}</dt><dd className="mono">{diagnostics?.selectedCore?.providerFingerprint ?? t('helper.checkUnknown')}</dd>
            </dl>
            <p>{t('helper.providerFingerprintTip')}</p>
            <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8, marginTop: 14 }}>
              <Button variant="ghost" size="sm" onClick={copyDiagnostics} disabled={!diagnostics} style={{ maxWidth: '100%', whiteSpace: 'normal' }}>
                <span>{t('helper.copyDiagnostics')}</span>
              </Button>
              {copyState !== 'idle' && <p role="status">{t(copyState === 'copied' ? 'helper.diagnosticsCopied' : 'helper.diagnosticsCopyFailed')}</p>}
            </div>
          </details>
        </div>
      </div>

      {/* 右侧此前挂着 `<Pill variant="region">待命</Pill>` —— 纯静态装饰：自动恢复是 Rust
          core-supervisor 内部行为（crash_recovery.rs 的 auto_restart_enabled），**没有任何 IPC 把它的
          实时状态发给渲染端**（api-client.ts:120 已记过同类教训：声明了后端从不发的字段）。恒显「待命」
          的徽章会被读成真状态（"现在是待命，异常时会变别的"），实际永远不变 ⇒ 删掉，宁可无状态也不留假状态。
          接线那天：由 helperApi 或 proxy 状态事件补真值，再把徽章加回来。 */}
      <SetBlock header={t('helper.fallbackTitle')}>
        <SetRow
          label={t('helper.autoRecover')}
          tip={t('helper.autoRecoverDesc')}
        />
      </SetBlock>
    </section>
  );
}
