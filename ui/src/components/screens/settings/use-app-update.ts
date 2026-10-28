/**
 * 应用更新卡的唯一运行时 owner。
 *
 * 进度事件会被广播到每个窗口，因而这里同时持有状态机、订阅清理与安装期的包快照；呈现层只消费
 * 返回的状态和动作，不再复制任何状态迁移或跨 await 的一致性约束。
 */
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  updateApi,
  versionApi,
  type DialogInstallAdvisory,
  type UpdateProgressManifest,
  type VersionInfo,
} from '@/ipc/api-client';
import { toast } from '@/lib/error-handler';
import { useDialogStore } from '../../dialogs/dialog-store';
import { markAppVersionSkipped } from '../../layout/app-update-banner';
import {
  appDownloadIntegrity,
  appUpdateErrText,
  formMismatchKey,
  installFailureKey,
  wireUpdateProgress,
  type AppDownloadIntegrity,
} from './settings-logic';

/* U1 取文腿 2026-09-13 搬进 `settings-logic`：移动端更新页是同一批码的第二个消费点，
   各写一份必然漂（措辞与兜底规则），而漂了两侧都只是「显示了一句话」，不会红。 */
const updateErrText = appUpdateErrText;

export type AppUpdateState =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'downloaded'
  | 'manual'
  | 'error';

interface InstallSubject {
  completionUnverified?: boolean;
  path: string;
  info: UpdateProgressManifest | null;
  integrity: AppDownloadIntegrity;
}

export function useAppUpdate(includePrerelease: boolean) {
  const { t } = useTranslation();
  const openDialog = useDialogStore((s) => s.open);
  const closeDialog = useDialogStore((s) => s.close);
  const [us, setUs] = useState<AppUpdateState>('idle');
  const [appVersionInfo, setAppVersionInfo] = useState<VersionInfo | null>(null);
  const [updateInfo, setUpdateInfo] = useState<UpdateProgressManifest | null>(null);
  const [progress, setProgress] = useState(0);
  const [receivedBytes, setReceivedBytes] = useState<number | null>(null);
  const [errMsg, setErrMsg] = useState('');
  const [downloadedPath, setDownloadedPath] = useState<string | null>(null);
  const [downloadIntegrity, setDownloadIntegrity] = useState<AppDownloadIntegrity>('unknown');
  // 安装调用在飞：状态给按钮置灰用，ref 给入口去重用（同一次渲染里的两次点击读到的是同一个
  // 状态快照，只有 ref 拦得住第二次）。
  const [installing, setInstalling] = useState(false);
  const [manualCompletionUnverified, setManualCompletionUnverified] = useState(false);
  const installInFlight = useRef(false);
  const clearInFlight = useRef(false);
  const [clearingPortable, setClearingPortable] = useState(false);
  const [pendingPortable, setPendingPortable] = useState<VersionInfo['pendingPortableUpdate']>();

  useEffect(() => {
    void versionApi
      .getInfo()
      .then((info) => {
        setAppVersionInfo(info);
        setPendingPortable(info.pendingPortableUpdate);
      })
      .catch(() => undefined);
  }, []);

  // 订阅 + 挂载期快照回读。顺序、竞态否决与「两条路共用同一个 reducer」都在 `wireUpdateProgress`
  // 里（那里跑得进单测，这里跑不进）；本处只剩把一份 patch 摊到各 useState 上。
  useEffect(() => {
    return wireUpdateProgress({
      subscribe: (onFrame) => updateApi.onProgress(onFrame),
      readSnapshot: () => updateApi.getProgress(),
      resetIntegrity: () => setDownloadIntegrity('unknown'),
      applyPatch: (patch) => {
        setUs(patch.us);
        setUpdateInfo(patch.info);
        setDownloadedPath(patch.path);
        setReceivedBytes(patch.received);
        setProgress(patch.percentage);
        if (patch.errorCode !== null) {
          setErrMsg(updateErrText(patch.errorCode, patch.errorDetail, t));
        }
        if (patch.integrity !== null) setDownloadIntegrity(patch.integrity);
      },
    });
  }, []);

  async function checkUpdate() {
    // Ordinary checks, including failures, never discard a manual handoff.
    setUs('checking');
    setDownloadIntegrity('unknown');
    try {
      const r = await updateApi.check({ includePrerelease });
      if (r.hasUpdate && r.updateInfo) {
        setUpdateInfo(r.updateInfo);
        setUs('available');
      } else {
        setUs('idle');
      }
    } catch (error) {
      setUs('error');
      console.error('[update] check failed:', error);
      setErrMsg(updateErrText((error as { code?: string }).code, undefined, t));
    }
  }

  async function discardPortableAndCheck() {
    if (clearInFlight.current || installInFlight.current) return;
    clearInFlight.current = true;
    setClearingPortable(true);
    try {
      await updateApi.clearPortableHandoff();
      // Clear local recovery only after the durable dismissal succeeded.
      setPendingPortable(undefined);
      setManualCompletionUnverified(false);
      await checkUpdate();
    } catch (error) {
      // Keep the manual card/positions available for retry after a write failure.
      toast.error(updateErrText((error as { code?: string }).code, undefined, t));
    } finally {
      clearInFlight.current = false;
      setClearingPortable(false);
    }
  }

  /** 显式重新下载当前通道的当前版本；若实际已有新版，只展示新版，不替用户自动下载另一个目标。 */
  async function reinstallCurrent() {
    setUs('checking');
    setDownloadIntegrity('unknown');
    try {
      const r = await updateApi.check({ includePrerelease, includeCurrent: true });
      if (r.hasUpdate && r.updateInfo) {
        setUpdateInfo(r.updateInfo);
        setUs('available');
      } else if (r.isCurrentVersion && r.updateInfo) {
        setUpdateInfo(r.updateInfo);
        await downloadTarget(r.updateInfo);
      } else {
        setUpdateInfo(null);
        setUs('error');
        setErrMsg(t('settings.update.reinstallUnavailable'));
      }
    } catch (error) {
      setUs('error');
      console.error('[update] reinstall resolution failed:', error);
      setErrMsg(updateErrText((error as { code?: string }).code, undefined, t));
    }
  }

  async function skipVersion() {
    if (updateInfo) {
      try {
        await updateApi.skip(updateInfo.version);
      } catch (error) {
        console.error('[update] skip failed:', error);
      }
      markAppVersionSkipped(updateInfo.version);
    }
    setUs('idle');
  }

  async function downloadTarget(target: UpdateProgressManifest) {
    setUs('downloading');
    setProgress(0);
    setReceivedBytes(0);
    setDownloadIntegrity('unknown');
    try {
      const r = await updateApi.download(target);
      setDownloadIntegrity(appDownloadIntegrity(r));
      if (!r.success) {
        setUs('error');
        setErrMsg(updateErrText(r.errorCode, r.errorDetail, t));
      }
    } catch (error) {
      setUs('error');
      console.error('[update] download failed:', error);
      setErrMsg(updateErrText((error as { code?: string }).code, undefined, t));
    }
  }

  async function downloadUpdate() {
    if (updateInfo) await downloadTarget(updateInfo);
  }

  /** 卡片按钮的安装入口：调用在飞时不再发第二次（这一下可能停核并退出应用）。 */
  async function startInstall(confirmed = false) {
    if (installInFlight.current) return;
    if (clearInFlight.current) return;
    installInFlight.current = true;
    setInstalling(true);
    try {
      await installUpdate(confirmed);
    } finally {
      installInFlight.current = false;
      setInstalling(false);
    }
  }

  function settleInstall(next: 'manual' | 'error', message: string, subject: InstallSubject) {
    setUs(next);
    setManualCompletionUnverified(Boolean(subject.completionUnverified));
    setUpdateInfo(subject.info);
    setDownloadedPath(subject.path);
    setDownloadIntegrity(subject.integrity);
    setErrMsg(message);
    // 落到这两屏 = 安装调用已有结论，屏上的按钮不该还停在「在飞」。
    setInstalling(false);
  }

  async function installUpdate(confirmed = false, subject?: InstallSubject) {
    const subj: InstallSubject = subject ?? {
      path: downloadedPath ?? '',
      info: updateInfo,
      integrity: downloadIntegrity,
    };
    if (!subj.path) return;
    try {
      const result = await updateApi.install(subj.path, confirmed);
      if (result.needConfirm && result.advisory) {
        // Windows 便携版：没有安装程序，这一步不弹确认框，而是落到 `manual` 卡 —— 说明要留在屏上
        // 供用户对照着做，卡上的按钮才带 `confirmed` 重调（后端此时才停核、打开文件夹并退出）。
        // 必须排在通用确认框之前：那一支会按 advisory 名去取一组本条没有的标题/正文键。
        if (result.advisory === 'portableManualReplace') {
          settleInstall(
            'manual',
            (subj.completionUnverified ? t('settings.update.portableCompletionUnverified') + '\n' : '') + t('settings.update.portableManualReplace', {
              path: subj.path,
              dir: result.programDir ?? '',
            }),
            subj,
          );
          return;
        }
        const advisory: DialogInstallAdvisory = result.advisory;
        openDialog({
          kind: 'confirm',
          payload: {
            title: t(`settings.update.advisory.${advisory}.title`),
            message: t(`settings.update.advisory.${advisory}.message`),
            confirmLabel: t('settings.update.advisory.continue'),
            onConfirm: async () => {
              closeDialog();
              await installUpdate(true, subj);
            },
          },
        });
        return;
      }
      if (result.handedToSystem || result.reason === 'form-mismatch') {
        settleInstall('error', t(formMismatchKey(result.handedToSystem)), subj);
      }
    } catch (error) {
      console.error('[update] install failed:', error);
      settleInstall('error', t(installFailureKey((error as { code?: string }).code)), subj);
    }
  }

  // 恢复上次手动交接的位置；同版本修复是否已覆盖无法自动证明，必须保留未确认语义。走的就是
  // 未确认的那次安装调用（后端此时只回告知、不碰代理），于是两个位置由同一条便携告知腿写到屏上，
  // 程序目录取的是此刻的真值。只在卡片空闲时接手（在途的下载 / 已下完的包优先），接手一次即止。
  useEffect(() => {
    if (!pendingPortable || us !== 'idle') return;
    setPendingPortable(undefined);
    void installUpdate(false, { path: pendingPortable.archive, info: null, integrity: 'unknown', completionUnverified: pendingPortable.completionUnverified });
  }, [pendingPortable, us]);

  return {
    appVersionInfo,
    us,
    updateInfo,
    progress,
    receivedBytes,
    errMsg,
    downloadIntegrity,
    installing,
    manualCompletionUnverified,
    clearingPortable,
    discardPortableAndCheck,
    checkUpdate,
    reinstallCurrent,
    skipVersion,
    downloadUpdate,
    installUpdate: startInstall,
  };
}
