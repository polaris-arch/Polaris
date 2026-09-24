/**
 * 网络场景面板（spec §6.1 / §6.2，D13）—— 规则页两个平面共用同一个入口。
 *
 *  - `NetworkProfilesDialog`：场景列表（判据摘要 · 引用计数 · 本机探测源 · 启用开关 · 编辑 · 删除）。
 *  - `NetworkProfileDialog`：单个场景的编辑表单（名称 / DNS 服务器地址段 / 搜索域 / 探测方式 / 启用）。
 *
 * 写入沿用 DNS 资源同一条腿：`useConfig().update({ networkProfiles })`（逐键经 `splitPatchByRoute` →
 * `editRoute` 分流暂存或直落盘，调用点不做 class 判定）；编辑在本地草稿里进行，提交时一次性替换整个集合，
 * 取消不留半份配置（口径同 `DnsResourceDialog`）。
 *
 * 「本机将使用哪种探测、可不可用」由后端算好经 `api.networkProfile.resolvedSources()` 返回，本文件只显示
 * （spec §4.4 末段：渲染端重算必然漂移）。第一期不显示命中态（D8，放 N4）。
 */

import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { NetworkProbeSource, NetworkProfile, ResolvedProbe } from '@/contracts/types';
import { api } from '@/ipc';
import { useEffectiveRules } from '@/store/app-store';
import { useConfirmTwice } from '@/lib/confirm-twice';
import {
  buildNetworkProfile,
  criteriaSummary,
  probeDisplay,
  probeInputsDiffer,
  probeWarningKey,
  profileRowStatus,
  profileRefCounts,
  validateNetworkProfileDraft,
  type NetworkProfileFormError,
  type ProbeDisplay,
} from '@/domain/network-profile';
import { useDialogStore } from '@/components/dialogs/dialog-store';
import { Modal } from '@/components/dialogs/Modal';
import { InfoIcon } from '@/components/InfoIcon';
import { useConfig, type UseConfigResult } from '../settings/use-config';
import { ListEditor } from '../settings/ListEditor';
import { Segmented, Spinner, Switch, TextInput } from '../settings/Primitives';

function ProfileIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M5 12.5a10 10 0 0114 0M8.5 16a5 5 0 017 0M12 19.5h.01M2 9a14.5 14.5 0 0120 0" />
    </svg>
  );
}

/**
 * 后端解析后的探测源。`dep` 变了就重拉（场景、代理模式、TUN 配置都会改变解析结果）。
 * 拉不到（IPC 失败 / 后端还没有这条命令 / 返回不是数组）⇒ `null` ⇒ 显示「暂时无法获取」，不猜。
 */
export function useResolvedProbes(dep: unknown): ResolvedProbe[] | null {
  const [resolved, setResolved] = useState<ResolvedProbe[] | null>(null);
  useEffect(() => {
    let active = true;
    api.networkProfile
      .resolvedSources()
      .then((list) => {
        if (active) setResolved(Array.isArray(list) ? list : null);
      })
      .catch(() => {
        if (active) setResolved(null);
      });
    return () => {
      active = false;
    };
  }, [dep]);
  return resolved;
}

const SOURCE_KEY = {
  system: 'rules.networkProfile.sourceSystem',
  dhcp: 'rules.networkProfile.sourceDhcp',
} as const;

/** 「本机将使用：…」一行（表单）与列表行尾的短标签共用的文案。 */
export function probeDisplayText(display: ProbeDisplay, t: TFunction): string {
  switch (display.kind) {
    case 'pending':
      return t('rules.networkProfile.probePending');
    case 'unknown':
      return t('rules.networkProfile.probeUnknown');
    case 'ok':
      return t('rules.networkProfile.probeUses', { source: t(SOURCE_KEY[display.source]) });
    case 'unavailable':
      return t('rules.networkProfile.probeUnavailable', { reason: t(display.reasonKey) });
  }
}

function ProbeLine({ display, t }: { display: ProbeDisplay; t: TFunction }) {
  const text = probeDisplayText(display, t);
  return display.kind === 'unavailable' ? (
    <div className="err-line">{text}</div>
  ) : (
    <div className="card-sub" style={{ marginTop: 6 }}>{text}</div>
  );
}

/** 后端告警（`available` 仍为 true）：文案键来自 `probeWarningKey`，判据只在后端。 */
function ProbeWarn({ warningKey, t }: { warningKey: string; t: TFunction }) {
  return (
    <div className="warn-line">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
        <path d="M12 9v4M12 17h.01M10.3 3.9L2 18a2 2 0 001.7 3h16.6a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z" />
      </svg>
      <span>{t(warningKey)}</span>
    </div>
  );
}

// ─────────────────────────────── 列表面板 ───────────────────────────────

function ProfileList({ config, update }: { config: NonNullable<UseConfigResult['config']>; update: UseConfigResult['update'] }) {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const { armed, confirmTwice } = useConfirmTwice();
  const trafficRules = useEffectiveRules('route');
  const dnsRules = useEffectiveRules('dns');
  const profiles = useMemo(() => config.networkProfiles ?? [], [config.networkProfiles]);
  const resolved = useResolvedProbes(config);

  const commit = (next: NetworkProfile[]) => {
    void update({ networkProfiles: next });
  };

  if (profiles.length === 0) {
    return (
      <div className="stub">
        <p>{t('rules.networkProfile.empty')}</p>
      </div>
    );
  }

  return (
    <div className="dns-resource-list">
      {profiles.map((profile) => {
        const refs = profileRefCounts(profile.id, trafficRules, dnsRules);
        const summary = criteriaSummary(profile);
        const status = profileRowStatus(profile, resolved);
        const delKey = `network-profile:${profile.id}`;
        const confirming = armed === delKey;
        return (
          <article key={profile.id} className="card dns-resource-card">
            <div
              className="dns-resource-summary"
              style={{ gridTemplateColumns: 'minmax(0,1fr) auto' }}
            >
              <span className="dns-resource-title">
                {profile.name}
                {!profile.enabled && (
                  <span className="pill" style={{ marginInlineStart: 6 }}>
                    {t('rules.networkProfile.disabledBadge')}
                  </span>
                )}
              </span>
              <span className="dns-resource-actions">
                <Switch
                  checked={profile.enabled}
                  onChange={(enabled) =>
                    commit(profiles.map((p) => (p.id === profile.id ? { ...p, enabled } : p)))
                  }
                  aria-label={t('rules.networkProfile.enabled')}
                />
                <button
                  type="button"
                  className="btn ghost sm"
                  onClick={() => open({ kind: 'network-profile', profileId: profile.id })}
                >
                  {t('common.edit')}
                </button>
                <button
                  type="button"
                  className={confirming ? 'btn ghost sm danger-text confirming' : 'btn ghost sm danger-text'}
                  onClick={() =>
                    confirmTwice(delKey, () => commit(profiles.filter((p) => p.id !== profile.id)))
                  }
                >
                  {t(confirming ? 'common.confirmAgain' : 'common.delete')}
                </button>
              </span>
              <span className="dns-resource-meta" style={{ gridColumn: '1 / -1' }}>
                {summary.cidrs > 0 && `${t('rules.networkProfile.summaryCidrs')} ×${summary.cidrs}`}
                {summary.cidrs > 0 && summary.domains > 0 && ' · '}
                {summary.domains > 0 && `${t('rules.networkProfile.summaryDomains')} ×${summary.domains}`}
                {summary.sample && ` (${summary.sample})`}
                {' · '}
                {t('rules.networkProfile.refs', { route: refs.route, dns: refs.dns })}
              </span>
              <span style={{ gridColumn: '1 / -1' }}>
                {/* 停用的场景只显示「已停用」（标题旁的徽标），不显示探测结果：后端对它恒报
                    profileInvalid，再画一行红字只是重复同一件事。 */}
                {status.kind === 'probe' && <ProbeLine display={status.display} t={t} />}
                {status.kind === 'probe' && status.warningKey && (
                  <ProbeWarn warningKey={status.warningKey} t={t} />
                )}
                {/* 删除有引用的场景：原地二次点击的武装态里说清后果（spec §6.2「删除有引用的场景时二次确认，
                    并说明这些规则会停止生效」）。规则本身不动 —— 悬空引用按 fail-closed 不生成（§3.4-2）。 */}
                {confirming && refs.route + refs.dns > 0 && (
                  <div className="err-line">
                    {t('rules.networkProfile.deleteRefsWarn', { count: refs.route + refs.dns })}
                  </div>
                )}
              </span>
            </div>
          </article>
        );
      })}
    </div>
  );
}

/** 加载 / 失败态的外壳（口径同 `DnsResourceState`）。 */
function ConfigShell({
  titleId,
  title,
  render,
}: {
  titleId: string;
  title: string;
  render: (cfg: UseConfigResult & { config: NonNullable<UseConfigResult['config']> }) => React.ReactNode;
}) {
  const { t } = useTranslation();
  const close = useDialogStore((s) => s.close);
  const cfg = useConfig();
  if (cfg.loading || cfg.error || !cfg.config) {
    return (
      <Modal
        titleId={titleId}
        title={title}
        icon={<ProfileIcon />}
        onClose={close}
        className="entry-form-dlg"
        footer={
          <>
            <button type="button" className="btn ghost" onClick={close}>{t('common.close')}</button>
            {cfg.error && (
              <button type="button" className="btn flow" onClick={() => void cfg.reload()}>
                {t('common.retry')}
              </button>
            )}
          </>
        }
      >
        {cfg.loading ? (
          <div className="dns-workspace-loading"><Spinner /></div>
        ) : (
          <div className="stub"><p>{t('common.configLoadFail')}</p></div>
        )}
      </Modal>
    );
  }
  return <>{render({ ...cfg, config: cfg.config })}</>;
}

export function NetworkProfilesDialog() {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const close = useDialogStore((s) => s.close);
  const title = t('rules.networkProfile.title');
  return (
    <ConfigShell
      titleId="network-profiles-title"
      title={title}
      render={({ config, update }) => (
        <Modal
          titleId="network-profiles-title"
          title={title}
          icon={<ProfileIcon />}
          onClose={close}
          className="entry-form-dlg"
          footer={
            <>
              <button type="button" className="btn ghost" onClick={close}>{t('common.close')}</button>
              <button
                type="button"
                className="btn flow"
                onClick={() => open({ kind: 'network-profile' })}
              >
                {t('rules.networkProfile.add')}
              </button>
            </>
          }
        >
          <div className="card-sub" style={{ marginBottom: 12 }}>{t('rules.networkProfile.intro')}</div>
          <ProfileList config={config} update={update} />
        </Modal>
      )}
    />
  );
}

// ─────────────────────────────── 编辑表单 ───────────────────────────────

function Field({
  label,
  tip,
  required,
  children,
}: {
  label: string;
  tip?: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="fld">
      <span className="fld-l">
        {label}
        {required && <span className="req-star"> *</span>}
        {tip && <InfoIcon tip={tip} />}
      </span>
      {children}
    </div>
  );
}

function ProfileForm({
  config,
  update,
  base,
  onSaved,
}: {
  config: NonNullable<UseConfigResult['config']>;
  update: UseConfigResult['update'];
  base?: NetworkProfile;
  onSaved?: (profileId: string) => void;
}) {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const close = useDialogStore((s) => s.close);
  const isEdit = base != null;
  const [id] = useState(() => base?.id ?? `np-${crypto.randomUUID()}`);
  const [name, setName] = useState(base?.name ?? '');
  const [cidrs, setCidrs] = useState<string[]>(() => [...(base?.match.dnsServerCidrs ?? [])]);
  const [domains, setDomains] = useState<string[]>(() => [...(base?.match.searchDomains ?? [])]);
  const [probe, setProbe] = useState<NetworkProbeSource>(base?.probe ?? 'auto');
  const [enabled, setEnabled] = useState(base?.enabled ?? true);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<NetworkProfileFormError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const resolved = useResolvedProbes(config);

  const touch = () => {
    setDirty(true);
    setError(null);
  };
  const draft = buildNetworkProfile({ name, cidrs, domains, enabled, probe }, id);
  const display = probeDisplay(isEdit ? id : undefined, resolved, probeInputsDiffer(base, draft));
  // 草稿改了判据 / 探测方式 / 启停 ⇒ 后端结果是旧的，告警也不显示（与「保存后显示」同一口径）。
  const warningKey =
    display.kind === 'pending' ? null : probeWarningKey(resolved?.find((r) => r.profileId === id));

  const requestClose = () => {
    if (!dirty) {
      close();
      return;
    }
    open({
      kind: 'confirm',
      payload: {
        title: t('rules.discardTitle'),
        message: t('rules.discardMsg'),
        confirmLabel: t('rules.discard'),
        danger: true,
        onConfirm: () => {
          close();
          close();
        },
      },
    });
  };

  const handleSubmit = async () => {
    const err = validateNetworkProfileDraft({ name, cidrs, domains });
    setError(err);
    if (err) return;
    const profiles = config.networkProfiles ?? [];
    const next = isEdit ? profiles.map((p) => (p.id === id ? draft : p)) : [...profiles, draft];
    setSubmitting(true);
    try {
      await update({ networkProfiles: next }, { throwOnError: true });
      close();
      onSaved?.(id);
    } catch {
      // useConfig 已显示保存失败原因；保留弹窗和草稿供用户修正或重试。
    } finally {
      setSubmitting(false);
    }
  };

  const errorText = (() => {
    if (!error) return null;
    if (error.kind === 'name') return t('rules.networkProfile.errName');
    if (error.kind === 'noCriteria') return t('rules.networkProfile.errNoCriteria');
    if (error.kind === 'cidr') return t('rules.networkProfile.errCidr', { value: error.value });
    return t('rules.networkProfile.errDomain', { value: error.value });
  })();

  return (
    <Modal
      titleId="network-profile-title"
      title={t(isEdit ? 'rules.networkProfile.editTitle' : 'rules.networkProfile.newTitle')}
      icon={<ProfileIcon />}
      onClose={requestClose}
      className="entry-form-dlg"
      footer={
        <>
          <button type="button" className="btn ghost" onClick={requestClose} disabled={submitting}>
            {t('common.cancel')}
          </button>
          <button type="button" className="btn flow" onClick={() => void handleSubmit()} disabled={submitting}>
            {t(isEdit ? 'common.save' : 'common.add')}
          </button>
        </>
      }
    >
      <Field label={t('rules.networkProfile.name')} required>
        <TextInput
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            touch();
          }}
          placeholder={t('rules.networkProfile.namePh')}
          aria-label={t('rules.networkProfile.name')}
        />
        {error?.kind === 'name' && <div className="err-line">{errorText}</div>}
      </Field>

      <div className="card-sub" style={{ marginBottom: 10 }}>{t('rules.networkProfile.criteriaHint')}</div>

      <Field label={t('rules.networkProfile.cidrs')} tip={t('rules.networkProfile.cidrsHint')}>
        <ListEditor
          value={cidrs}
          onChange={(next) => {
            setCidrs(next);
            touch();
          }}
          placeholder={t('rules.networkProfile.cidrsPh')}
          ariaLabel={t('rules.networkProfile.cidrs')}
          addLabel={t('common.add')}
          importLabel={t('common.bulkImport')}
        />
      </Field>

      <Field label={t('rules.networkProfile.domains')} tip={t('rules.networkProfile.domainsHint')}>
        <ListEditor
          value={domains}
          onChange={(next) => {
            setDomains(next);
            touch();
          }}
          placeholder={t('rules.networkProfile.domainsPh')}
          ariaLabel={t('rules.networkProfile.domains')}
          addLabel={t('common.add')}
          importLabel={t('common.bulkImport')}
        />
      </Field>
      {error && error.kind !== 'name' && <div className="err-line">{errorText}</div>}

      <Field label={t('rules.networkProfile.probe')} tip={t('rules.networkProfile.probeHint')}>
        <div>
          <Segmented<NetworkProbeSource>
            options={[
              { value: 'auto', label: t('rules.networkProfile.probeAuto') },
              { value: 'system', label: t('rules.networkProfile.probeSystem') },
              { value: 'dhcp', label: t('rules.networkProfile.probeDhcp') },
            ]}
            value={probe}
            onChange={(next) => {
              setProbe(next);
              touch();
            }}
            ariaLabel={t('rules.networkProfile.probe')}
          />
        </div>
        <ProbeLine display={display} t={t} />
        {warningKey && <ProbeWarn warningKey={warningKey} t={t} />}
      </Field>

      <div className="fld swt-row">
        <div className="swt-tx">
          <span className="swt-label">
            <b>{t('rules.networkProfile.enabled')}</b>
            <InfoIcon tip={t('rules.networkProfile.enabledHint')} />
          </span>
        </div>
        <Switch
          checked={enabled}
          onChange={(next) => {
            setEnabled(next);
            touch();
          }}
          aria-label={t('rules.networkProfile.enabled')}
        />
      </div>
    </Modal>
  );
}

export function NetworkProfileDialog({
  profileId,
  onSaved,
}: {
  profileId?: string;
  onSaved?: (profileId: string) => void;
}) {
  const { t } = useTranslation();
  const close = useDialogStore((s) => s.close);
  const title = t(profileId ? 'rules.networkProfile.editTitle' : 'rules.networkProfile.newTitle');
  return (
    <ConfigShell
      titleId="network-profile-title"
      title={title}
      render={({ config, update }) => {
        const base = profileId ? config.networkProfiles?.find((p) => p.id === profileId) : undefined;
        if (profileId && !base) {
          return (
            <Modal
              titleId="network-profile-title"
              title={title}
              icon={<ProfileIcon />}
              onClose={close}
              className="entry-form-dlg"
              footer={<button type="button" className="btn ghost" onClick={close}>{t('common.close')}</button>}
            >
              <div className="stub"><p>{t('rules.networkProfile.missing')}</p></div>
            </Modal>
          );
        }
        return <ProfileForm config={config} update={update} base={base} onSaved={onSaved} />;
      }}
    />
  );
}
