import { MobileInfo } from '../MobileInfo';
/**
 * 移动端**网络场景**表单（新建 + 编辑；桌面 `components/screens/rules/NetworkProfilePanel.tsx` 的
 * `NetworkProfileDialog` + `ProfileForm` + `ConfigShell` 那一格，spec §6.2）。
 *
 * # 复用的是判据
 *
 * 校验（`validateNetworkProfileDraft`）、落盘形态（`buildNetworkProfile`：去空行 / 搜索域规范化 / 去重）、
 * 「本机将使用」的显示态（`probeDisplay` / `probeInputsDiffer` / `probeWarningKey`）全部来自
 * `@/domain/network-profile`；后端解析结果与文案来自 `network-profile-probes`（与桌面同一份）。
 * 清单编辑走设置屏的 `MobileListEditor`（去重与草稿规则在 `@/domain/list-entries`，桌面 `ListEditor` 同一份）。
 *
 * # 写路径：同一条 `useConfig().update` 漏斗
 *
 * 与桌面场景面板、与本仓 DNS 资源表单同一条（暂存 / 写盘 / 失败回滚只有一处）。`throwOnError` 不可省：
 * 缺省行为是把失败吞进 toast 并回滚，而本面板要把错误留在**表单上**（理由同 `DnsResourceFormPanel` 头注）。
 *
 * # 三处与桌面不同的形态（都是触屏事实，不是排版偏好）
 *
 *  1. 三格说明（地址段 / 搜索域 / 探测方式）桌面挂在 `InfoIcon` 的悬浮提示上 ⇒ 这里是常驻 `m-form-hint`（§4.12）；
 *  2. 探测方式是 `m-form-seg` 三段（同 WARP / WG 表单），不是原生 select：三档一眼看全，切一下不必进全屏选择器；
 *  3. 不可用原因的**措辞**换成手机上的那一句（`network-profile-copy#mobileReasonKey`，原因码不变）。
 */

import { useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { NetworkProbeSource, NetworkProfile, UserConfig } from '@/contracts/types';
import {
  buildNetworkProfile,
  probeDisplay,
  probeInputsDiffer,
  probeReasonKey,
  probeWarningKey,
  validateNetworkProfileDraft,
  type NetworkProfileFormError,
} from '@/domain/network-profile';
import { useConfig, type UseConfigResult } from '@/components/screens/settings/use-config';
import { probeDisplayText, useResolvedProbes } from '@/components/screens/rules/network-profile-probes';
import { useBuiltinDhcpStatus } from '@/components/dialogs/rule-effect-state';
import { MobileListEditor } from '../settings/SettingsChrome';
import { mobileReasonKey } from '../screens/rules/network-profile-copy';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

const PROBE_CHOICES: ReadonlyArray<{ id: NetworkProbeSource; key: string }> = [
  { id: 'auto', key: 'rules.networkProfile.probeAuto' },
  { id: 'system', key: 'rules.networkProfile.probeSystem' },
  { id: 'dhcp', key: 'rules.networkProfile.probeDhcp' },
];

export function NetworkProfileFormPanel({
  instanceId,
  profileId,
  onSaved,
}: {
  instanceId: string;
  profileId?: string;
  onSaved?: (profileId: string) => void;
}): ReactElement {
  const { t } = useTranslation();
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const cfg = useConfig();
  const title = t(profileId ? 'rules.networkProfile.editTitle' : 'rules.networkProfile.newTitle');
  const close = (): void => closeInstance(instanceId);

  /* 配置读取态（桌面 `ConfigShell`）：加载中 / 读失败带重试。不在读失败时画一张空表 ——
     那张空表一提交就会拿「空集合 + 新场景」覆盖掉盘上全部场景。 */
  if (cfg.config === null) {
    const failed = !cfg.loading;
    return (
      <FormSheet
        compact
        title={title}
        onRequestClose={close}
        closeLabel={t('common.close')}
        cancelLabel={t('common.close')}
        submitLabel={failed ? t('common.retry') : undefined}
        onSubmit={failed ? () => void cfg.reload() : undefined}
      >
        <p className="m-form-msg">{failed ? t('common.configLoadFail') : t('common.loading')}</p>
      </FormSheet>
    );
  }

  const base = profileId ? cfg.config.networkProfiles?.find((p) => p.id === profileId) : undefined;
  /* 编辑的那个场景已被删（别处删掉、或暂存被撤销）：说清楚，不悄悄变成「新建」。 */
  if (profileId && !base) {
    return (
      <FormSheet
        compact
        title={title}
        onRequestClose={close}
        closeLabel={t('common.close')}
        cancelLabel={t('common.close')}
      >
        <p className="m-form-msg">{t('rules.networkProfile.missing')}</p>
      </FormSheet>
    );
  }
  return (
    <ProfileForm
      instanceId={instanceId}
      config={cfg.config}
      update={cfg.update}
      base={base}
      onSaved={onSaved}
    />
  );
}

function ProfileForm({
  instanceId,
  config,
  update,
  base,
  onSaved,
}: {
  instanceId: string;
  config: UserConfig;
  update: UseConfigResult['update'];
  base?: NetworkProfile;
  onSaved?: (profileId: string) => void;
}): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const isEdit = base !== undefined;

  const [id] = useState(() => base?.id ?? `np-${crypto.randomUUID()}`);
  const [name, setName] = useState(base?.name ?? '');
  const [cidrs, setCidrs] = useState<string[]>(() => [...(base?.match.dnsServerCidrs ?? [])]);
  const [domains, setDomains] = useState<string[]>(() => [...(base?.match.searchDomains ?? [])]);
  const [probe, setProbe] = useState<NetworkProbeSource>(base?.probe ?? 'auto');
  const [enabled, setEnabled] = useState(base?.enabled ?? true);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<NetworkProfileFormError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const resolved = useResolvedProbes(config);
  /**
   * DHCP 这一档在本机能不能用 —— 后端判据 `ProbeFacts::dhcp_unavailable`（dhcp 源场景与内置 DHCP 解析器
   * 同一个 transport）。Android / iOS 恒不可用（应用沙箱绑不了 UDP 68）。不可用 ⇒ 这一档置灰、下面一行写原因，
   * 不留一个点了也永远不生效的选项；**已经选着它**的存量场景（桌面备份导进来的）照常回显、原因同样写出来。
   */
  const dhcpStatus = useBuiltinDhcpStatus();
  const dhcpBlocked = dhcpStatus !== null && !dhcpStatus.available;

  const touch = (): void => {
    setDirty(true);
    setError(null);
  };
  const draft = buildNetworkProfile({ name, cidrs, domains, enabled, probe }, id);
  /* 草稿改了判据 / 探测方式 / 启停 ⇒ 后端结果是旧的：说「保存后显示」，告警也不显示（同桌面）。 */
  const display = probeDisplay(isEdit ? id : undefined, resolved, probeInputsDiffer(base, draft));
  const warningKey =
    display.kind === 'pending' ? null : probeWarningKey(resolved?.find((r) => r.profileId === id));

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
    const invalid = validateNetworkProfileDraft({ name, cidrs, domains });
    setError(invalid);
    if (invalid !== null) return;
    const profiles = config.networkProfiles ?? [];
    const next = isEdit ? profiles.map((p) => (p.id === id ? draft : p)) : [...profiles, draft];
    setSubmitting(true);
    setNotice(undefined);
    void (async () => {
      try {
        await update({ networkProfiles: next }, { throwOnError: true });
        closeInstance(instanceId);
        onSaved?.(id);
      } catch (err) {
        console.error('[mobile-network-profile-form] save failed:', err);
        /* 表单可能已被关掉（异步 continuation）⇒ 先确认宿主还在再落回显。 */
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: tr('common.saveFailed') });
      } finally {
        if (hasInstance(instanceId)) setSubmitting(false);
      }
    })();
  };

  const errorText = ((): string | null => {
    if (!error) return null;
    if (error.kind === 'name') return tr('rules.networkProfile.errName');
    if (error.kind === 'noCriteria') return tr('rules.networkProfile.errNoCriteria');
    if (error.kind === 'cidr') return tr('rules.networkProfile.errCidr', { value: error.value });
    return tr('rules.networkProfile.errDomain', { value: error.value });
  })();

  const listLabels = {
    addLabel: tr('common.add'),
    importLabel: tr('common.bulkImport'),
    importHint: tr('settings.listImportHint'),
    removeLabel: tr('common.delete'),
    confirmLabel: tr('common.confirm'),
    cancelLabel: tr('common.cancel'),
    emptyLabel: tr('mobileRules.networkProfile.listEmpty'),
  };

  return (
    <FormSheet
      title={tr(isEdit ? 'rules.networkProfile.editTitle' : 'rules.networkProfile.newTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      submitLabel={tr(isEdit ? 'common.save' : 'common.add')}
      submitDisabled={submitting}
      onSubmit={submit}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mnp-name">
          {tr('rules.networkProfile.name')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mnp-name"
          className="m-form-input"
          value={name}
          placeholder={tr('rules.networkProfile.namePh')}
          onChange={(e) => {
            setName(e.target.value);
            touch();
          }}
        />
        {error?.kind === 'name' && <p className="m-form-err">{errorText}</p>}
      </div>

      <p className="m-form-hint">{tr('rules.networkProfile.criteriaHint')}</p>

      <div className="m-form-row">
        <span className="m-form-label">{tr('rules.networkProfile.cidrs')}</span>
        <div className="m-form-hint"><MobileInfo title={tr('rules.networkProfile.cidrs')} summary={tr('mobileHelp.profileCidrs')} details={tr('rules.networkProfile.cidrsHint')} /></div>
        <MobileListEditor
          id="mnp-cidrs"
          value={cidrs}
          onChange={(next) => {
            setCidrs(next);
            touch();
          }}
          placeholder={tr('rules.networkProfile.cidrsPh')}
          ariaLabel={tr('rules.networkProfile.cidrs')}
          {...listLabels}
        />
      </div>

      <div className="m-form-row">
        <span className="m-form-label">{tr('rules.networkProfile.domains')}</span>
        <p className="m-form-hint">{tr('rules.networkProfile.domainsHint')}</p>
        <MobileListEditor
          id="mnp-domains"
          value={domains}
          onChange={(next) => {
            setDomains(next);
            touch();
          }}
          placeholder={tr('rules.networkProfile.domainsPh')}
          ariaLabel={tr('rules.networkProfile.domains')}
          {...listLabels}
        />
      </div>
      {error !== null && error.kind !== 'name' && <p className="m-form-err">{errorText}</p>}

      <div className="m-form-row">
        <span className="m-form-label" id="mnp-probe">
          {tr('rules.networkProfile.probe')}
        </span>
        <div className="m-form-hint"><MobileInfo title={tr('rules.networkProfile.probe')} summary={tr('mobileHelp.profileProbe')} details={tr('rules.networkProfile.probeHint')} /></div>
        <div className="m-form-seg" role="group" aria-labelledby="mnp-probe">
          {PROBE_CHOICES.map((choice) => (
            <button
              key={choice.id}
              type="button"
              className={probe === choice.id ? 'on' : ''}
              aria-pressed={probe === choice.id}
              disabled={choice.id === 'dhcp' && dhcpBlocked && probe !== 'dhcp'}
              onClick={() => {
                setProbe(choice.id);
                touch();
              }}
            >
              {tr(choice.key)}
            </button>
          ))}
        </div>
        {dhcpBlocked && dhcpStatus !== null && (
          <p className="m-form-hint">
            {tr('rules.networkProfile.probeDhcp')} ·{' '}
            {tr('rules.networkProfile.probeUnavailable', {
              reason: tr(mobileReasonKey(probeReasonKey(dhcpStatus.reason))),
            })}
          </p>
        )}
        {/* 「本机将使用：…」由后端算（同生成器一个函数）；不可用 ⇒ 红字带原因。
            Android 上手选 DHCP 恒落这一档（应用沙箱绑不了 UDP 68），原因如实写出来，不藏选项。 */}
        <p className={display.kind === 'unavailable' ? 'm-form-err' : 'm-form-hint'}>
          {probeDisplayText(display, t, mobileReasonKey)}
        </p>
        {warningKey !== null && <p className="m-form-err">{tr(warningKey)}</p>}
      </div>

      <div className="m-form-row m-form-switch">
        <div className="m-form-switch-tx">
          <span className="m-form-label" id="mnp-enabled">
            {tr('rules.networkProfile.enabled')}
          </span>
          <p className="m-form-hint">{tr('rules.networkProfile.enabledHint')}</p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-labelledby="mnp-enabled"
          className={`m-form-swt${enabled ? ' on' : ''}`}
          onClick={() => {
            setEnabled(!enabled);
            touch();
          }}
        />
      </div>
    </FormSheet>
  );
}
