import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端 **DNS 服务器 / 服务器组** 表单（新建 + 编辑，两张表一个文件，同桌面 `DnsResourceDialog.tsx`）。
 *
 * # 复用的是判据
 *
 * 校验（`validateDnsServerForm` / `validateDnsGroupForm`）、Hosts 记录往返
 * （`parseHostsPredefined` / `formatHostsPredefined`）、成员重排（`moveDnsGroupMember`）、
 * 内置保护（`isProtectedDnsServer`）全部来自 `@/components/dialogs/dns-resource-logic` ——
 * 那份纯 `.ts` 是本批从两个桌面 `.tsx` 里搬出来的，**判据一行未改**。
 * 显示名与描述走 `dns-action-options` 的 `dnsServerDisplayName` / `dnsServerDescription`
 * （内置三个服务器的名字必须过它，否则界面上会显示盘上的英文稳定名）。
 *
 * # 写路径：同一条 `useConfig().update` 漏斗
 *
 * 与桌面、与移动端设置屏、与规则屏的 DNS 开关**同一条**（`config-write-wiring.test.ts` 的
 * `SITES` 里本文件登记为 `funnel`）。不在这里另造保存协议：暂存 / 写盘 / 失败回滚只有一处。
 *
 * 🔴 `{ throwOnError: true }` 不可省。`useConfig().update` 的缺省行为是自己把失败吞进 toast 并
 * 回滚，而本面板要在**表单上**留住错误（用户的下一步就在这张表里）——不 opt-in 抛错的话，
 * 提交失败时表单会静静关掉、草稿全丢。这与规则屏那两个 DNS 开关是同一条理由。
 */

import { useMemo, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  DnsServerGroup,
  DnsServerKind,
  DnsServerResource,
} from '@/contracts/types';
import {
  dnsServerDescription,
  dnsServerDisplayName,
} from '@/components/dialogs/dns-action-options';
import {
  formatHostsPredefined,
  isProtectedDnsServer,
  moveDnsGroupMember,
  parseHostsPredefined,
  validateDnsGroupForm,
  validateDnsServerForm,
  type DnsGroupFormError,
  type DnsServerFormError,
} from '@/components/dialogs/dns-resource-logic';
import { isIpLiteral } from '@/domain/ip-literal';
import { useConfig } from '@/components/screens/settings/use-config';
import { toast } from '@/lib/error-handler';
import { editRoute } from '@/lib/staged-config';
import { useStagingActive } from '@/store/use-staging-active';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

/** 各类型的缺省端口（桌面 `DnsResourceDialog.tsx#defaultPort` 逐值同）。 */
function defaultPort(type: DnsServerKind): string {
  if (type === 'https') return '443';
  if (type === 'tls') return '853';
  if (type === 'udp' || type === 'tcp') return '53';
  return '';
}

export function DnsServerFormPanel({
  instanceId,
  serverId,
}: {
  instanceId: string;
  serverId?: string;
}): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const { config, update } = useConfig();
  const stagingEnabled = useStagingActive();

  const dnsServers = useMemo(() => config?.dnsServers ?? [], [config?.dnsServers]);
  const base = serverId === undefined ? undefined : dnsServers.find((s) => s.id === serverId);
  const isEdit = base !== undefined;
  const builtin = base ? isProtectedDnsServer(base.id) : false;
  const bootstrap = base?.id === 'builtin-bootstrap';

  const [name, setName] = useState(() =>
    base ? dnsServerDisplayName(base, t) : tr('settings.dns.serverNewName'),
  );
  const [type, setType] = useState<DnsServerKind>(base?.type ?? 'https');
  const [host, setHost] = useState(base?.endpoint?.host ?? '1.1.1.1');
  const [port, setPort] = useState(
    base?.endpoint?.port != null ? String(base.endpoint.port) : defaultPort(base?.type ?? 'https'),
  );
  const [path, setPath] = useState(base?.endpoint?.path ?? '/dns-query');
  const [outbound, setOutbound] = useState(() =>
    base?.outbound?.type === 'node' ? `node:${base.outbound.nodeId}` : (base?.outbound?.type ?? 'direct'),
  );
  const [bootstrapServerId, setBootstrapServerId] = useState(base?.bootstrapServerId ?? '');
  const [paths, setPaths] = useState((base?.paths ?? []).join(', '));
  const [hosts, setHosts] = useState(formatHostsPredefined(base?.predefined));
  const [error, setError] = useState<DnsServerFormError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  const network = type !== 'local' && type !== 'hosts';
  const touch = (): void => {
    setDirty(true);
    setError(null);
  };

  /**
   * 可当 bootstrap 的候选：排除自身、必须启用、非 hosts，且（`local` 或「直连出口 + IP 字面主机」）。
   * 逐条同桌面 —— 一个自己也要靠域名解析的服务器当不了引导服务器，那是一条环。
   */
  const bootstrapCandidates = useMemo(
    () =>
      dnsServers.filter(
        (server) =>
          server.id !== base?.id
          && server.enabled !== false
          && server.type !== 'hosts'
          && (server.type === 'local'
            || (server.outbound?.type === 'direct'
              && typeof server.endpoint?.host === 'string'
              && isIpLiteral(server.endpoint.host))),
      ),
    [dnsServers, base?.id],
  );
  const validBootstrapIds = useMemo(
    () => new Set(bootstrapCandidates.map((server) => server.id)),
    [bootstrapCandidates],
  );

  const errorText = (kind: DnsServerFormError): string | undefined => {
    if (error !== kind) return undefined;
    if (kind === 'name') return tr('settings.dns.serverNameRequired');
    if (kind === 'host') return tr('settings.dns.serverHostRequired');
    if (kind === 'port') return tr('settings.dns.serverPortInvalid');
    if (kind === 'bootstrapIp') return tr('settings.dns.bootstrapIpOnly');
    return tr('settings.dns.serverBootstrapRequired');
  };

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
    const invalid = validateDnsServerForm({
      name,
      type,
      host,
      port,
      isBootstrap: bootstrap,
      bootstrapServerId,
      validBootstrapServerIds: validBootstrapIds,
    });
    setError(invalid);
    if (invalid !== null) return;

    const trimmedHost = host.trim();
    const next: DnsServerResource = {
      id: base?.id ?? `dns-${crypto.randomUUID()}`,
      /* 内置项保留盘上那个稳定名：`name` 这一格显示的是 i18n 名，写回去等于把译文当数据存。 */
      name: builtin && base ? base.name : name.trim(),
      /* 启停不在这张表里（列表侧那颗开关才是它的载体）⇒ 新建恒 true、编辑沿用。 */
      enabled: base?.enabled ?? true,
      type,
      endpoint: network
        ? {
            host: trimmedHost,
            ...(port.trim() ? { port: Number(port) } : {}),
            ...(type === 'https' && path.trim() ? { path: path.trim() } : {}),
          }
        : undefined,
      bootstrapServerId:
        network && !bootstrap && !isIpLiteral(trimmedHost) ? bootstrapServerId : undefined,
      outbound:
        network && !bootstrap
          ? outbound.startsWith('node:')
            ? { type: 'node', nodeId: outbound.slice(5) }
            : { type: outbound as 'direct' | 'currentExit' }
          : { type: 'direct' },
      paths:
        type === 'hosts'
          ? paths.split(',').map((entry) => entry.trim()).filter(Boolean)
          : undefined,
      predefined: type === 'hosts' ? parseHostsPredefined(hosts) : undefined,
    };
    const nextServers = isEdit
      ? dnsServers.map((server) => (server.id === next.id ? next : server))
      : [...dnsServers, next];

    setSubmitting(true);
    setNotice(undefined);
    void (async () => {
      try {
        await update({ dnsServers: nextServers }, { throwOnError: true });
        closeInstance(instanceId);
        if (editRoute('dnsServers', stagingEnabled) === 'direct') toast.success(tr('common.saved'));
      } catch (err) {
        console.error('[mobile-dns-server-form] save failed:', err);
        /* 表单可能已被关掉（异步 continuation）⇒ 先确认宿主还在再落回显。 */
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: tr('common.saveFailed') });
      } finally {
        if (hasInstance(instanceId)) setSubmitting(false);
      }
    })();
  };

  return (
    <FormSheet
      title={isEdit ? tr('settings.dns.serverEditTitle') : tr('settings.dns.serverAddTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      submitLabel={tr('common.save')}
      submitDisabled={submitting}
      onSubmit={submit}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mdsf-name">
          {tr('settings.dns.serverName')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mdsf-name"
          className="m-form-input"
          value={name}
          disabled={builtin}
          onChange={(e) => {
            setName(e.target.value);
            touch();
          }}
        />
        {builtin && <p className="m-form-hint">{tr('settings.dns.builtinRequired')}</p>}
        {errorText('name') != null && <p className="m-form-err">{errorText('name')}</p>}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mdsf-type">
          {tr('settings.dns.serverType')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <MobileSelect
          id="mdsf-type"
          className="m-form-select"
          value={type}
          onChange={(e) => {
            const nextType = e.target.value as DnsServerKind;
            /* 端口只在「空 或 仍是上一个类型的默认值」时跟着换：用户填过的端口不许被换类型抹掉。 */
            if (port.trim() === '' || port === defaultPort(type)) setPort(defaultPort(nextType));
            setType(nextType);
            touch();
          }}
        >
          <option value="https">{tr('settings.dns.serverType_https')}</option>
          <option value="tls">{tr('settings.dns.serverType_tls')}</option>
          <option value="udp">{tr('settings.dns.serverType_udp')}</option>
          <option value="tcp">{tr('settings.dns.serverType_tcp')}</option>
          <option value="local">{tr('settings.dns.serverType_local')}</option>
          {/* 内置三个服务器不许改成 hosts：它们是引导链与两条默认解析腿的载体。 */}
          {!builtin && <option value="hosts">{tr('settings.dns.serverType_hosts')}</option>}
        </MobileSelect>
      </div>

      {network && (
        <>
          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mdsf-outbound">
              {tr('settings.dns.serverOutbound')}
              <span className="m-form-req" aria-hidden>
                *
              </span>
            </label>
            <MobileSelect
              id="mdsf-outbound"
              className="m-form-select"
              value={bootstrap ? 'direct' : outbound}
              disabled={bootstrap}
              onChange={(e) => {
                setOutbound(e.target.value);
                touch();
              }}
            >
              <option value="direct">{tr('settings.dns.outboundDirect')}</option>
              {!bootstrap && (
                <option value="currentExit">{tr('settings.dns.outboundCurrentExit')}</option>
              )}
              {!bootstrap
                && (config?.servers ?? []).map((node) => (
                  <option key={node.id} value={`node:${node.id}`}>
                    {tr('settings.dns.outboundNode', { name: node.name })}
                  </option>
                ))}
            </MobileSelect>
            {/* 引导服务器只能直连 —— 它要在隧道建立**之前**解析出口域名。桌面把这句挂在
                下拉的 `data-tip` 上；触屏没有 hover ⇒ 常驻一行（§4.12）。 */}
            {bootstrap && <p className="m-form-hint">{tr('settings.dns.bootstrapDirectOnly')}</p>}
          </div>

          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mdsf-host">
              {tr('settings.dns.serverHost')}
              <span className="m-form-req" aria-hidden>
                *
              </span>
            </label>
            <input
              id="mdsf-host"
              className="m-form-input mono"
              value={host}
              onChange={(e) => {
                setHost(e.target.value);
                touch();
              }}
            />
            {(errorText('host') ?? errorText('bootstrapIp')) != null && (
              <p className="m-form-err">{errorText('host') ?? errorText('bootstrapIp')}</p>
            )}
          </div>

          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mdsf-port">
              {tr('settings.dns.serverPort')}
              <span className="m-form-opt">{tr('common.optional')}</span>
            </label>
            <input
              id="mdsf-port"
              className="m-form-input mono"
              inputMode="numeric"
              value={port}
              onChange={(e) => {
                setPort(e.target.value);
                touch();
              }}
            />
            {errorText('port') != null && <p className="m-form-err">{errorText('port')}</p>}
          </div>

          {type === 'https' && (
            <div className="m-form-row">
              <label className="m-form-label" htmlFor="mdsf-path">
                {tr('settings.dns.serverPath')}
              </label>
              <input
                id="mdsf-path"
                className="m-form-input mono"
                value={path}
                onChange={(e) => {
                  setPath(e.target.value);
                  touch();
                }}
              />
            </div>
          )}

          {/* Bootstrap 只在「主机是域名」时才需要：IP 字面量不用解析。
              这条显隐判据与 `validateDnsServerForm` 的同一条，写反了会要求一个用不上的字段。 */}
          {!bootstrap && host.trim() !== '' && !isIpLiteral(host.trim()) && (
            <div className="m-form-row">
              <label className="m-form-label" htmlFor="mdsf-bootstrap">
                {tr('settings.dns.serverBootstrap')}
                <span className="m-form-req" aria-hidden>
                  *
                </span>
              </label>
              <MobileSelect
                id="mdsf-bootstrap"
                className="m-form-select"
                value={bootstrapServerId}
                onChange={(e) => {
                  setBootstrapServerId(e.target.value);
                  touch();
                }}
              >
                <option value="">{tr('settings.dns.serverBootstrapRequired')}</option>
                {bootstrapCandidates.map((server) => (
                  <option key={server.id} value={server.id}>
                    {dnsServerDisplayName(server, t)}
                  </option>
                ))}
              </MobileSelect>
              {errorText('bootstrapMissing') != null && (
                <p className="m-form-err">{errorText('bootstrapMissing')}</p>
              )}
            </div>
          )}
        </>
      )}

      {type === 'hosts' && (
        <>
          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mdsf-paths">
              {tr('settings.dns.hostsPaths')}
            </label>
            <input
              id="mdsf-paths"
              className="m-form-input mono"
              value={paths}
              onChange={(e) => {
                setPaths(e.target.value);
                touch();
              }}
            />
          </div>
          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mdsf-hosts">
              {tr('settings.dns.hostsInline')}
            </label>
            <textarea
              id="mdsf-hosts"
              className="m-form-input m-form-area mono"
              rows={5}
              value={hosts}
              placeholder={tr('settings.dns.hostsInlinePlaceholder')}
              onChange={(e) => {
                setHosts(e.target.value);
                touch();
              }}
            />
          </div>
        </>
      )}
    </FormSheet>
  );
}

export function DnsGroupFormPanel({
  instanceId,
  groupId,
}: {
  instanceId: string;
  groupId?: string;
}): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const { config, update } = useConfig();
  const stagingEnabled = useStagingActive();

  const dnsServers = useMemo(() => config?.dnsServers ?? [], [config?.dnsServers]);
  const dnsGroups = useMemo(() => config?.dnsServerGroups ?? [], [config?.dnsServerGroups]);
  const base = groupId === undefined ? undefined : dnsGroups.find((g) => g.id === groupId);
  const isEdit = base !== undefined;

  const [name, setName] = useState(base?.name ?? tr('settings.dns.groupNewName'));
  const [mode, setMode] = useState<DnsServerGroup['mode']>(base?.mode ?? 'race');
  const [members, setMembers] = useState<string[]>([...(base?.members ?? [])]);
  const [fallbackServerId, setFallbackServerId] = useState(base?.fallbackServerId ?? '');
  const [error, setError] = useState<DnsGroupFormError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  const touch = (): void => {
    setDirty(true);
    setError(null);
  };

  const addable = dnsServers.filter(
    (server) => server.enabled !== false && !members.includes(server.id),
  );
  /**
   * race 模式要至少两个**可用**成员才谈得上竞速。少于两个时它退化成单上游，
   * 而界面仍显示「竞速」—— 这句提醒就是那条退化的可见通道。
   */
  const usableMembers = members.filter(
    (id) => dnsServers.find((server) => server.id === id)?.enabled !== false,
  );
  const raceDegraded = mode === 'race' && members.length > 0 && usableMembers.length < 2;

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
    const invalid = validateDnsGroupForm({ name, members });
    setError(invalid);
    if (invalid !== null) return;
    const next: DnsServerGroup = {
      id: base?.id ?? `dns-group-${crypto.randomUUID()}`,
      name: name.trim(),
      enabled: base?.enabled ?? true,
      mode,
      members,
      fallbackServerId: fallbackServerId || undefined,
    };
    const nextGroups = isEdit
      ? dnsGroups.map((group) => (group.id === next.id ? next : group))
      : [...dnsGroups, next];

    setSubmitting(true);
    setNotice(undefined);
    void (async () => {
      try {
        await update({ dnsServerGroups: nextGroups }, { throwOnError: true });
        closeInstance(instanceId);
        if (editRoute('dnsServerGroups', stagingEnabled) === 'direct') toast.success(tr('common.saved'));
      } catch (err) {
        console.error('[mobile-dns-group-form] save failed:', err);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: tr('common.saveFailed') });
      } finally {
        if (hasInstance(instanceId)) setSubmitting(false);
      }
    })();
  };

  return (
    <FormSheet
      title={isEdit ? tr('settings.dns.groupEditTitle') : tr('settings.dns.groupAddTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      submitLabel={tr('common.save')}
      submitDisabled={submitting}
      onSubmit={submit}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mdgf-name">
          {tr('settings.dns.groupName')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mdgf-name"
          className="m-form-input"
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            touch();
          }}
        />
        {error === 'name' && <p className="m-form-err">{tr('settings.dns.groupNameRequired')}</p>}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mdgf-mode">
          {tr('settings.dns.groupMode')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <MobileSelect
          id="mdgf-mode"
          className="m-form-select"
          value={mode}
          onChange={(e) => {
            setMode(e.target.value as DnsServerGroup['mode']);
            touch();
          }}
        >
          <option value="race">{tr('settings.dns.groupRace')}</option>
          <option value="fallback">{tr('settings.dns.groupFallback')}</option>
        </MobileSelect>
        <p className="m-form-hint">
          {tr(mode === 'race' ? 'settings.dns.groupRaceDesc' : 'settings.dns.groupFallbackDesc')}
        </p>
        {raceDegraded && <p className="m-form-err">{tr('settings.dns.groupRaceDegraded')}</p>}
      </div>

      <div className="m-form-row">
        <span className="m-form-label">
          {tr('settings.dns.groupMembers')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </span>
        {members.length === 0 ? (
          <p className="m-form-hint">{tr('settings.dns.groupNoMembers')}</p>
        ) : (
          members.map((id, index) => {
            const server = dnsServers.find((candidate) => candidate.id === id);
            const unavailable = server === undefined || server.enabled === false;
            return (
              <div key={id} className="m-form-row">
                <span className="m-form-label">
                  {index + 1}. {server ? dnsServerDisplayName(server, t) : id}
                </span>
                <p className="m-form-hint">
                  {unavailable
                    ? tr('rules.dnsActionUnavailable')
                    : dnsServerDescription(server, config?.servers ?? [], t)}
                </p>
                <div className="m-form-foot">
                  <button
                    type="button"
                    className="m-form-btn"
                    disabled={index === 0}
                    onClick={() => {
                      setMembers(moveDnsGroupMember(members, index, index - 1));
                      touch();
                    }}
                  >
                    {tr('rules.moveUp')}
                  </button>
                  <button
                    type="button"
                    className="m-form-btn"
                    disabled={index === members.length - 1}
                    onClick={() => {
                      setMembers(moveDnsGroupMember(members, index, index + 1));
                      touch();
                    }}
                  >
                    {tr('rules.moveDown')}
                  </button>
                  <button
                    type="button"
                    className="m-form-btn danger"
                    onClick={() => {
                      setMembers(members.filter((member) => member !== id));
                      touch();
                    }}
                  >
                    {tr('settings.dns.groupRemoveMember')}
                  </button>
                </div>
              </div>
            );
          })
        )}
        {/* 「添加成员」是一颗 value 恒为空的 select：选中即 append（同桌面，不做多选面板）。 */}
        <MobileSelect
          className="m-form-select"
          aria-label={tr('settings.dns.groupAddMember')}
          value=""
          onChange={(e) => {
            if (e.target.value === '') return;
            setMembers([...members, e.target.value]);
            touch();
          }}
        >
          <option value="">{tr('settings.dns.groupAddMember')}</option>
          {addable.map((server) => (
            <option key={server.id} value={server.id}>
              {dnsServerDisplayName(server, t)}
            </option>
          ))}
        </MobileSelect>
        {error === 'members' && (
          <p className="m-form-err">{tr('settings.dns.groupMembersRequired')}</p>
        )}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mdgf-fallback">
          {tr('settings.dns.groupFallbackServer')}
          <span className="m-form-opt">{tr('common.optional')}</span>
        </label>
        <MobileSelect
          id="mdgf-fallback"
          className="m-form-select"
          value={fallbackServerId}
          onChange={(e) => {
            setFallbackServerId(e.target.value);
            touch();
          }}
        >
          <option value="">{tr('settings.dns.groupNoFallback')}</option>
          {dnsServers
            .filter(
              (server) =>
                server.enabled !== false
                && server.type !== 'hosts'
                && (!members.includes(server.id) || server.id === fallbackServerId),
            )
            .map((server) => (
              <option key={server.id} value={server.id}>
                {dnsServerDisplayName(server, t)}
                {members.includes(server.id) ? ` · ${tr('settings.dns.groupFallbackRedundant')}` : ''}
              </option>
            ))}
        </MobileSelect>
      </div>
    </FormSheet>
  );
}
