import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * **Tailscale 出口设备选择器**（W-05）。
 *
 * 桌面把这一格放在 `TsSettingsDialog` 那张 20 余项的大表里（`TsSettingsDialog.tsx:73` 那一行
 * `{ t: 'select', k: 'exitNode', … }`）。移动端只搬**这一格**：出口设备是 TsExitWarning 那条安全
 * 注脚唯一能解除的东西（IA 裁定 #3），而整张 TS 设置表不在本批射程内。
 *
 * # 复用而不是重写
 *
 *  · **候选构造** —— `ts-settings-logic.ts#exitNodeOptions`（带单测的平台无关纯函数）。
 *    它负责：列全部 peer（不筛 `exitNodeOption`，否则「没广告出口」与「不在 tailnet 里」在界面上
 *    长得一模一样）、按值去重、排序、禁用注记、已保存值恒可寻址。这些一条都不许在这里重判。
 *  · **写回** —— `buildTsSettings(base, draft)`：它保全未建模字段（`authKey` 由登录腿写入，
 *    本面绝不能覆写掉），并按「缺省即默认」删键而不是写显式值。
 *  · **单一写入口** —— `api.server.update` + `editRoute` 暂存闸门，与桌面同一条。
 *  · **读侧** —— `api.server.tailscaleGetStatus()`，同桌面那条 effect（核未跑 ⇒ 空候选，
 *    降级成手填，不是报错）。
 *
 * # 草稿只有两个键
 *
 * `buildTsSettings` 吃的是**整份**草稿，缺键会把该字段按「用户没填」写回去 ⇒ 必须从
 * `initTsDraft(node)` 起底（它按各字段真实缺省回填），只覆盖 `exitNode` / `exitNodeCustom` 两格。
 * 直接喂 `{exitNode}` 会把 hostname / routes / advertiseTags 全部清空 —— 一个「选出口」的动作
 * 顺手抹掉一堆别的配置，且用户看不见。
 */

import { useEffect, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import type { TailscaleStatusPeer } from '@/contracts/tailscale-status';
import type { FormValues } from '@/components/dialogs/field-spec';
import {
  EXIT_CUSTOM,
  buildTsSettings,
  exitNodeOptions,
  initTsDraft,
  peersForTsNode,
} from '@/components/dialogs/ts-settings-logic';
import { editRoute } from '@/lib/staged-config';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

export function TsExitPanel({
  instanceId,
  serverId,
}: {
  instanceId: string;
  serverId: string;
}): ReactElement {
  const { t } = useTranslation();
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const servers = useEffectiveServers();
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);

  const node = servers.find((s) => s.id === serverId);
  const savedExit = node?.tailscaleSettings?.exitNode ?? '';

  const [peers, setPeers] = useState<readonly TailscaleStatusPeer[]>([]);
  const [connected, setConnected] = useState<boolean | null>(null);
  const [exit, setExit] = useState(savedExit);
  const [custom, setCustom] = useState('');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

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

  const options = exitNodeOptions(peers, savedExit, {
    none: t('ts.exitNone'),
    custom: t('common.customEllipsis'),
    inUse: t('ts.exitInUse'),
    offline: t('ts.exitOffline'),
    notAdvertised: t('ts.exitNotAdvertised'),
  });

  const save = async (): Promise<void> => {
    if (node === undefined) return;
    setBusy(true);
    try {
      /* 从 `initTsDraft` 起底再覆盖两格：`buildTsSettings` 吃整份草稿，缺键 = 清空该字段。 */
      const draft: FormValues = { ...initTsDraft(node), exitNode: exit, exitNodeCustom: custom };
      const next = { ...node, tailscaleSettings: buildTsSettings(node.tailscaleSettings, draft) };
      if (editRoute('servers', stagingEnabled) === 'staged') {
        stage({
          id: `server:${node.id}`,
          kind: 'server',
          label: `${t('ts.exitNode')} ${node.name}`,
          entityPath: ['servers', node.id],
          nextValue: next,
        });
        closeInstance(instanceId);
        return;
      }
      await api.server.update(next);
      await loadConfig(true);
      closeInstance(instanceId);
    } catch (e) {
      console.error('[mobile-ts-exit] save failed:', e);
      setNotice({ tone: 'err', text: t('common.saveFailed') });
    } finally {
      if (hasInstance(instanceId)) setBusy(false);
    }
  };

  return (
    <FormSheet
      title={t('ts.exitNode')}
      onRequestClose={() => closeInstance(instanceId)}
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
            <label className="m-form-label" htmlFor="mts-exit">
              {t('ts.exitNode')}
            </label>
            <MobileSelect
              id="mts-exit"
              className="m-form-select"
              value={exit}
              onChange={(e) => setExit(e.target.value)}
            >
              {options.map(([value, label, disabled]) => (
                <option key={value} value={value} disabled={disabled === true}>
                  {label}
                </option>
              ))}
            </MobileSelect>
            {/* 核没跑 ⇒ 候选恒空，只剩「无 / 自定义…」。如实说明而不是留一个空下拉让人猜。 */}
            {connected === false && <p className="m-form-hint">{t('ts.exitEmptyHint')}</p>}
          </div>
          {exit === EXIT_CUSTOM && (
            <div className="m-form-row">
              <label className="m-form-label" htmlFor="mts-custom">
                {t('ts.exitNodeCustom')}
              </label>
              <input
                id="mts-custom"
                className="m-form-input mono"
                value={custom}
                placeholder="100.x.y.z / hostname"
                onChange={(e) => setCustom(e.target.value)}
              />
            </div>
          )}
        </>
      )}
    </FormSheet>
  );
}
