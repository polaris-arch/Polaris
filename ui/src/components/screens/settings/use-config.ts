/** Shared authoritative feed, instance-local direct intent, then staged projection. */
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { configApi, windowApi } from '@/ipc/api-client';
import { toast } from '@/lib/error-handler';
import { useDialogStore } from '@/components/dialogs/dialog-store';
import { useAppStore, effectiveConfigOf, acquireConfigSync, refreshConfig, beginConfigMutation, reconcileConfigMutation } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { withConfigWriteLock } from '@/lib/config-write-lock';
import { splitPatchByRoute } from './config-patch-route';
import { STAGED_SETTING_SECTION_LABELS } from './settings-logic';
import {
  appRestartRequiredChanges,
  appRestartRequiredDiff,
  restartKeysStillPending,
  type AppRestartRequiredKey,
} from '@/domain/app-restart-keys';
import type { UserConfig } from '@/contracts/types';
import { configEditState, displayConfig, beginDirect, commitDirect, failDirect, acceptConfig, directKeys, type ConfigEditState } from './use-config-state';

/** 弹窗里列出「哪几项要重启」时用的标签键 —— 直接复用设置页那三行自己的 label，文案不另起一套。 */
const RESTART_KEY_LABEL: Record<AppRestartRequiredKey, string> = {
  hardwareAcceleration: 'settings.general.hardwareAcceleration',
  windowEffects: 'settings.general.windowEffects',
  rememberWindowSize: 'settings.general.rememberWindowSize',
};

/**
 * 「已落盘、但要重启 App 才生效」的确认弹窗（U-7）。复用既有 confirm 基建（`dialog-store` + `ConfirmDialog`）。
 *
 * 文案三段，缺一不可：
 *  1. **哪些项**没生效（否则用户不知道弹窗在说谁）；
 *  2. **选「稍后」不等于没保存** —— 改动已经写进 config.json，只是本次运行仍按旧值跑。
 *     不写这句，「稍后」会被读成「放弃保存」，用户会去把开关来回拨；
 *  3. 代理在跑时补一句**重启会断连** —— 后端重启腿恒经 `ExitRequested` 停核（`commands/window.rs::app_restart`），
 *     这是真会发生的副作用，不许含糊。
 *
 * 只在配置 patch **成功之后**调用：保存失败时用户看到的是 toast + 回滚，此时说「已落盘」是撒谎。
 */
function promptAppRestart(keys: AppRestartRequiredKey[], t: TFunction): void {
  const items = keys.map((k) => t(RESTART_KEY_LABEL[k])).join(t('common.listSeparator'));
  // 读 app-store 而非本 hook 的 config：代理运行态不在 UserConfig 里。用 getState() 取瞬时值即可
  //（弹窗是一次性快照，不需要订阅）。
  const proxyRunning = useAppStore.getState().proxyStatus?.running === true;
  const paragraphs = [
    t(
      'settings.restartApp.message',
      { items },
    ),
    t('settings.restartApp.persistNote'),
  ];
  if (proxyRunning) {
    paragraphs.push(
      t('settings.restartApp.proxyNote'),
    );
  }
  useDialogStore.getState().open({
    kind: 'confirm',
    payload: {
      title: t('settings.restartApp.title'),
      // 段间空行由 ConfirmDialog 的 `white-space: pre-line` 渲染。
      message: paragraphs.join('\n\n'),
      confirmLabel: t('settings.restartApp.confirm'),
      cancelLabel: t('settings.restartApp.later'),
      onConfirm: () => {
        // ConfirmPayload 契约：onConfirm 自负关闭。先关再发重启——重启腿若失败（IPC 不通），
        // 留一个关不掉的模态比失败本身更糟。
        useDialogStore.getState().close();
        void windowApi.restartApp().catch((e) => {
          console.error('[useConfig] restart app failed:', e);
          toast.error(t('settings.restartApp.failed'));
        });
      },
    },
  });
}

export interface UseConfigResult {
  config: UserConfig | null;
  loading: boolean;
  /**
   * **仅**加载失败，且**仅**在阻塞腿上（显式 reload / 无种子时的首次挂载拉取）—— 有种子时挂载那次
   * 走 silent，拉失败就继续显示种子那份，不该把一屏可用的设置换成错误屏。保存失败也不写这里，
   * 见 `update` 的注释：
   * 消费方（SettingsPage）用本字段决定「整屏塌成错误屏」，保存失败塌屏会把用户正在编辑的表单卸载掉。
   */
  error: string | null;
  /** 局部 patch（顶层字段合并）；嵌套字段需先解构再传完整对象 */
  update: (
    patch: Partial<UserConfig>,
    options?: { throwOnError?: boolean },
  ) => Promise<void>;
  /** 强制重拉（如内核版本变化后） */
  reload: () => Promise<void>;
}

/** [`configFirstFrame`] 的返回 —— 与 `useConfig` 首帧那两个 `useState` 的初值一一对应。 */
export interface ConfigFirstFrame {
  /** 本 hook 自持的**磁盘副本** state 的初值。 */
  config: UserConfig | null;
  /** `loading` 的初值。 */
  loading: boolean;
}

/**
 * 首帧决策：app-store 里已 hydrate 的那份直接当种子 ⇒ `loading` 首帧即 `false`，转圈**结构上**不出现
 * （不是「更快了」，是那一帧压根不存在）。
 *
 * 抽成纯函数不是为了复用（调用点只有下面 `useConfig` 一处），是为了**可测**：本仓 vitest 跑 node
 * 环境、刻意不装 jsdom（见 `vite.config.ts` test 段），真 hook 一行都跑不起来，而这次要守的恰恰就是
 * 「首帧那两个初值是什么」。同款做法见 `settings-logic.ts` 的 `wireUpdateProgress` 与
 * `tray/tray-menu-height.ts`。
 *
 * 入参口径写死为**磁盘副本**（生产侧传 `useAppStore.getState().config`）：喂
 * `effectiveConfigOf(...)` 的结果会把暂存值烧进磁盘副本，理由见头注。`null` ⇒ 回落今天的行为。
 */
export function configFirstFrame(hydrated: UserConfig | null): ConfigFirstFrame {
  return { config: hydrated, loading: hydrated === null };
}

// Bounded notification metadata only: no second configuration cache.
let restartPublication = -1;
let promptedRestartKeys = new Set<AppRestartRequiredKey>();
function claimRestartKeys(serial: number, keys: AppRestartRequiredKey[]): AppRestartRequiredKey[] {
  if (!keys.length) return [];
  if (restartPublication !== serial) { restartPublication = serial; promptedRestartKeys = new Set(); }
  const fresh = keys.filter((key) => !promptedRestartKeys.has(key));
  fresh.forEach((key) => promptedRestartKeys.add(key));
  return fresh;
}

export function useConfig(): UseConfigResult {
  const { t } = useTranslation();
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const [seed] = useState(() => configFirstFrame(useAppStore.getState().config));
  const [config, setConfig] = useState<UserConfig | null>(seed.config);
  const [loading, setLoading] = useState(seed.loading);
  const [error, setError] = useState<string | null>(null);
  const edits = useRef(configEditState(seed.config));
  const alive = useRef(false);
  const lifecycle = useRef(0);
  const operationId = useRef(0);
  const requestId = useRef(0);
  const requests = useRef(new Set<AbortController>());
  const filledOnce = useRef(false);
  const startupFlags = useRef<Partial<UserConfig> | null>(null);
  const tRef = useRef(t);
  tRef.current = t;
  const apply = useCallback((state: ConfigEditState) => {
    edits.current = state;
    if (alive.current) setConfig(displayConfig(state));
  }, []);

  const load = useCallback(async (silent: boolean, fresh = false) => {
    const controller = new AbortController();
    requests.current.add(controller);
    const request = ++requestId.current;
    const life = lifecycle.current;
    if (!silent) { setLoading(true); setError(null); }
    try {
      await refreshConfig({ fresh, reason: fresh ? 'reload' : 'mount', signal: controller.signal });
    } catch (error) {
      if (!alive.current || life !== lifecycle.current || request !== requestId.current) return;
      console.error('[useConfig] load config failed:', error);
      if (!silent) setError(tRef.current('common.configLoadFail'));
    } finally {
      requests.current.delete(controller);
      if (alive.current && life === lifecycle.current && request === requestId.current && !silent) setLoading(false);
    }
  }, []);
  const reload = useCallback(() => load(false, true), [load]);

  useEffect(() => {
    alive.current = true;
    const life = ++lifecycle.current;
    // Subscribe before acquiring: the ready-registered initial GET cannot be missed.
    const offPublication = useAppStore.subscribe((state) => {
      const publication = state.configPublication;
      if (!alive.current || life !== lifecycle.current || !state.config || !publication || publication.serial <= edits.current.serial) return;
      const prev = edits.current.baseline;
      const ownKeys = directKeys(edits.current);
      const firstFill = !filledOnce.current;
      filledOnce.current = true;
      apply(acceptConfig(edits.current, state.config, publication.serial, publication.epoch));
      if (!firstFill && prev && publication.reason !== 'reload' && publication.reason !== 'mount') {
        const keys = restartKeysStillPending(appRestartRequiredDiff(prev, state.config)
          .filter((key) => !ownKeys.has(key)), startupFlags.current, state.config);
        const claimed = claimRestartKeys(publication.serial, keys);
        if (claimed.length) promptAppRestart(claimed, tRef.current);
      }
    });
    const release = acquireConfigSync({ scope: 'main' });
    void load(seed.config !== null);
    void windowApi.startupConfigFlags().then((flags) => {
      if (alive.current && life === lifecycle.current) startupFlags.current = flags;
    }).catch(() => {});
    return () => {
      alive.current = false;
      lifecycle.current++;
      requestId.current++;
      requests.current.forEach((controller) => controller.abort());
      offPublication();
      release();
    };
  }, [apply, load, seed]);

  const update = useCallback(async (patch: Partial<UserConfig>, options?: { throwOnError?: boolean }) => {
    const prev = displayConfig(edits.current);
    if (!prev || !alive.current) return;
    const life = lifecycle.current;
    const isAlive = () => alive.current && lifecycle.current === life;
    const { staged, direct } = splitPatchByRoute(patch, stagingEnabled);
    for (const [key, value] of staged) {
      const sectionKey = STAGED_SETTING_SECTION_LABELS[key];
      stage({ id: `setting:${key}`, kind: 'setting',
        label: t('home.stagedSetting', { key: sectionKey ? t(sectionKey) : key }),
        entityPath: [key], nextValue: value });
    }
    if (Object.keys(direct).length === 0) return;
    const id = ++operationId.current;
    const startSerial = edits.current.serial;
    const restartKeys = restartKeysStillPending(appRestartRequiredChanges(prev, direct), startupFlags.current, { ...prev, ...direct });
    apply(beginDirect(edits.current, id, direct));
    let saved: UserConfig;
    try {
      saved = await withConfigWriteLock(async () => {
        beginConfigMutation();
        return configApi.patch(direct);
      });
    } catch (error) {
      // A rejected write may have committed before a transport error. Reconcile
      // independently; never retry the write or turn a failed write into success.
      void reconcileConfigMutation().settled.catch(() => {});
      if (isAlive()) {
        apply(failDirect(edits.current, id));
        console.error('[useConfig] save config failed:', error);
        toast.error(tRef.current('common.saveFailed'));
      }
      if (options?.throwOnError) throw error;
      return;
    }
    // Another mounted instance may already have announced this committed value
    // while our native write reply was delayed. Only notification metadata is shared.
    const unannounced = restartKeys.filter((key) => !(restartPublication > startSerial
      && promptedRestartKeys.has(key) && edits.current.baseline?.[key] === saved[key]));
    const confirmation = reconcileConfigMutation();
    if (isAlive()) {
      // Only the direct keys belong to this reply. Other fields can predate a
      // newer external publication; never put the full reply in either baseline.
      apply(commitDirect(edits.current, id, saved, confirmation.barrier));
    }
    let confirmedSerial: number | null = null;
    let pendingRestartKeys = unannounced;
    try {
      const publication = await confirmation.settled;
      confirmedSerial = publication.serial;
      pendingRestartKeys = restartKeysStillPending(unannounced, startupFlags.current, publication.config);
    }
    catch (error) {
      // Save succeeded. Retain committed direct values until a later fresh read.
      // A read failure is not a save toast/error and does not close a draft dialog.
      console.error('[useConfig] post-save config refresh failed:', error);
    }
    if (isAlive() && pendingRestartKeys.length) {
      const keys = confirmedSerial === null ? pendingRestartKeys : claimRestartKeys(confirmedSerial, pendingRestartKeys);
      if (keys.length) promptAppRestart(keys, tRef.current);
    }
  }, [apply, stage, stagingEnabled, t]);

  return { config: effectiveConfigOf(config, stagedEntries), loading, error, update, reload };
}
