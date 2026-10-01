/**
 * WarpDialog —— Cloudflare WARP 接入/编辑弹窗（原型 #warp-dialog :2867，warpSetMode :4974）。
 *
 * **单例槽**：WARP 至多一个节点（`domain/warp.ts#findWarpNode`）。两提交路径共用一个弹窗：
 *  - **注册态**（edit 假）：`api.server.registerWarp(license?)`（REAL：X25519 keypair + Cloudflare 匿名注册）
 *    → 拿 `WarpWireGuardDraft` 草稿 → 用名称与少量高级项覆盖后 `api.server.add` 落为 WireGuard 节点
 *    （registerWarp 本身只返草稿不落盘，见 warp.rs / warp.ts 头注释 + server.rs:320）。
 *  - **编辑态**（edit 真）：预填现有 WARP 节点；WARP+ 许可经 `api.server.applyWarpLicense`（REAL，原地升级免重建），
 *    名称与高级参数改动经 `api.server.update`。
 *
 * 名称/许可保持连续任务流，低频 WireGuard 参数只放一个高级折叠区，不为内部字段强造分组。
 * 参数继续走 D2 FieldSpec 表 + FieldRenderer（§1.4 多字段驱动，复用节点表单同一渲染器）。
 * R1：`key` 绑编辑目标 id（见导出包装）+ useState 同步初始化。
 */

import { useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { toast } from '@/lib/error-handler';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { splitStagedOnly, stagedOnlyIds } from '@/lib/staged-config';
import { api } from '@/ipc';
import type { ServerConfig, WireGuardSettings } from '@/contracts/types';
import { findWarpNode } from '@/domain/warp';
import { registerWarpIfSlotFree } from '@/domain/mesh-singleton-guard';
import { Modal } from './Modal';
import {
  FormSection,
  type FormValue,
  type FormValues,
  type SelectOption,
} from './FieldSpec';
import { applyDetour, endpointDetourOptions, DETOUR_NONE } from './detour-options';
import {
  WARP_DEFAULT_NAME,
  planWarpSubmit,
  warpAdvancedSpec,
  warpDraftFromNode,
  type WarpPlan,
} from './warp-spec';
// 表单 → WireGuardSettings 的整段接线共用 `wg-logic.ts`；WARP 内部的路由/接入模式也在提交边界收口，
// 不能只靠「界面没展示」来假定旧配置里不存在。
import { buildWarpSettings, workersInputInvalid } from './wg-logic';
import { applyOnDemand } from './on-demand-field';
import { useDialogStore } from './dialog-store';
import { InfoIcon } from '@/components/InfoIcon';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { executeWarpRegistration, type WarpRegistrationSession } from './warp-registration';

function WarpIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 000 18M12 3a14 14 0 010 18" />
    </svg>
  );
}

/* 字段表 / 端点解析 / 提交前校验住在 `./warp-spec`（零 React，两个客户端共用；见那份文件头注）。
   桌面「多 VPN 兼容」批新加的 `ON_DEMAND_FIELD` 与草稿里的 `onDemand` 初值已随表搬进那份文件。 */
interface WarpFormProps {
  editNode?: ServerConfig;
  servers: ServerConfig[];
}

function WarpForm({ editNode, servers }: WarpFormProps) {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const close = useDialogStore((s) => s.close);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  /** staged-only 差集的两个入参（展示面 effective + 操作面磁盘镜像），判据与节点卡同一函数。 */
  const diskServers = useAppStore((s) => s.servers);
  const stagedOnly = useMemo(() => stagedOnlyIds(servers, diskServers), [servers, diskServers]);
  const isEdit = editNode != null;
  const interfaces = useNetworkInterfaces();

  // 前置代理候选。此前是「除自身外的全部节点」—— 其中的 WG/TS 节点在生成侧会被
  // `is_mesh_protocol` 那一支直接丢弃（选了等于没选）。收口到与 `WgDialog` /
  // `TsSettingsDialog` 同一个函数，排除判据一处定义、三处生效。
  const detourOpts = endpointDetourOptions(servers, editNode?.id, t('node.detourDirect'));

  // R1 同步初始化。默认名与草稿缺省都取自 `warp-spec`（两端共用，不在这里再写一份）。
  const [name, setName] = useState(editNode?.name ?? WARP_DEFAULT_NAME);
  const [plan, setPlan] = useState<WarpPlan>('free');
  const [license, setLicense] = useState('');
  const [draft, setDraft] = useState<FormValues>(() => warpDraftFromNode(editNode));
  const interfaceOpts: SelectOption[] = buildNetworkInterfaceChoices(
    interfaces.items,
    typeof draft.bindInterface === 'string' ? draft.bindInterface : '',
    {
      defaultLabel: t('node.bindInterfaceInherit'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    },
  ).map(({ value, label, disabled }) => [value, label, disabled]);
  const spec = warpAdvancedSpec(detourOpts, t('warp.endpointAuto'), interfaceOpts);

  const [errName, setErrName] = useState(false);
  const [errLicense, setErrLicense] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [done, setDone] = useState(false);
  const [dirty, setDirty] = useState(false);
  const registration = useRef<WarpRegistrationSession>({});

  const setField = (k: string, v: FormValue) => {
    setDraft((d) => ({ ...d, [k]: v }));
    setDirty(true);
  };
  const requestClose = () => {
    if (!dirty || done) {
      close();
      return;
    }
    open({
      kind: 'confirm',
      payload: {
        title: t('warp.discardTitle'),
        message: t('warp.discardMsg'),
        confirmLabel: t('warp.discard'),
        danger: true,
        onConfirm: () => {
          close();
          close();
        },
      },
    });
  };

  // 表单值 → WireGuardSettings 覆盖片段（注册草稿/编辑基础之上叠加）。实现在 `wg-logic.ts`：
  // 与 WG 那条同名否决并排放，且作为纯函数可被 `wg-logic.test.ts` 直接上牙。
  const settingsOverride = (base: Partial<WireGuardSettings>): WireGuardSettings =>
    buildWarpSettings(base, draft);

  const currentDetour = typeof draft.detour === 'string' ? draft.detour : DETOUR_NONE;

  const doRegister = async (endpointOverride: { host: string; port: number } | null) => {
    // 单例闸**前置到 CF 请求之前**：registerWarp 在 Cloudflare 侧真建一台匿名设备（远端副作用、
    // 本地拦不回来）。接入区卡片只在「打开弹窗那一刻」无 WARP 时才给入口，弹窗停留期间槽位可能被
    // 克隆/导入/WgDialog 抢走 —— 那时先打请求再拦 = 白烧一台孤儿设备。闸不过：toast + 返回 null，零请求。
    return executeWarpRegistration(registration.current, {
      register: () => registerWarpIfSlotFree(servers, t, () =>
        api.server.registerWarp(plan === 'plus' ? license.trim() : undefined)),
      mintId: () => crypto.randomUUID(),
      build: (draftResp, id) => {
        const settings = settingsOverride({
          privateKey: draftResp.privateKey,
          localAddress: draftResp.localAddress,
          peerPublicKey: draftResp.peerPublicKey,
          reserved: draftResp.reserved,
          warpDevice: draftResp.warpDevice,
        });
        const server: ServerConfig = {
          id,
          name: name.trim(),
          protocol: 'wireguard',
          address: endpointOverride?.host ?? draftResp.address,
          port: endpointOverride?.port ?? draftResp.port,
          wireguardSettings: settings,
        };
        applyDetour(server, currentDetour);
        applyOnDemand(server, draft.onDemand);
        const bindInterface = String(draft.bindInterface ?? '').trim();
        if (bindInterface) server.bindInterface = bindInterface;
        return server;
      },
      save: (server) => api.server.add(server),
      refresh: () => loadConfig(true),
      readback: async () => (await api.config.get()).servers ?? [],
    });
  };

  const doEdit = async (ep: { host: string; port: number }) => {
    if (!editNode) return;
    // `block`（ENTITY_ACTION_TABLE）：applyWarpLicense 改的是远端账户等级、update 改的是一台
    // **已注册**的设备 —— 盘上没有这个节点，两者都没有作用对象。
    const split = splitStagedOnly(
      'warp.edit',
      [editNode.id],
      stagedOnly,
      stagedEntries,
      'servers'
    );
    if (split.blocked.length > 0) {
      // 同 NodesScreen:687 / TsSettingsDialog：「还不能做」走 info 单参，文案自述完整。
      toast.info(
        t('home.stagedOnlyBlocked')
      );
      throw new Error('staged-only-blocked');
    }
    if (plan === 'plus' && license.trim()) {
      const r = await api.server.applyWarpLicense(editNode.id, license.trim());
      if (!r.ok) {
        // r.error 是后端原始串（非自述）⇒ 套 title；no-credentials 那支本身自述，单参。
        if (r.error === 'no-credentials') {
          toast.error(t('warp.errNoCreds'));
        } else {
          toast.error(t('warp.errLicense'));
        }
        throw new Error('applyLicense-failed');
      }
    }
    const settings = settingsOverride(editNode.wireguardSettings ?? {});
    const server: ServerConfig = {
      ...editNode,
      name: name.trim(),
      address: ep.host,
      port: ep.port,
      wireguardSettings: settings,
    };
    applyDetour(server, currentDetour);
    const bindInterface = String(draft.bindInterface ?? '').trim();
    if (bindInterface) server.bindInterface = bindInterface;
    else delete server.bindInterface;
    await api.server.update(server);
  };

  const handleSubmit = async () => {
    if (submitting) return;
    if (done) {
      close();
      return;
    }
    /* 三档拒绝走**共用**判据 `planWarpSubmit`（`warp-spec.ts`），移动端消费的是同一个函数 ——
       两端各写一份 `if` 必然在某天对同一份输入给出不同结论。 */
    const plan_ = planWarpSubmit({
      name,
      plan,
      license,
      endpointRaw: String(draft.endpoint ?? ''),
      isEdit,
    });
    if (plan_.reject === 'name') {
      setErrName(true);
      return;
    }
    if (plan_.reject === 'license') {
      setErrLicense(true);
      return;
    }
    // M6：端点解析失败必须内联报错并保持弹窗打开，不得静默回退旧地址后假装提交成功。
    if (plan_.reject === 'endpoint') {
      toast.error(t('warp.errEndpoint'));
      return;
    }
    if (workersInputInvalid(draft.workers)) {
      toast.error(t('wg.errWorkers'));
      return;
    }
    const ep = plan_.endpoint;
    setSubmitting(true);
    try {
      if (isEdit) {
        await doEdit(ep!);
        void loadConfig(true);
        close();
      } else {
        const registered = await doRegister(ep);
        if (registered) {
          setDone(true);
        }
      }
    } catch (e) {
      console.error('[WarpDialog] save failed:', e);
      if (!(e instanceof Error && e.message === 'applyLicense-failed')) {
        toast.error(t('common.saveFailed'));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const primaryLabel = done
    ? t('common.done')
    : isEdit
      ? t('common.save')
      : t('warp.register');

  return (
    <Modal
      titleId="warp-dlg-title"
      title={isEdit ? t('warp.editTitle') : t('warp.addTitle')}
      onClose={requestClose}
      icon={<WarpIcon />}
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
            {primaryLabel}
          </button>
        </>
      }
    >
      {!done && (
        <>
          {!isEdit && (
            <div className="mesh-note">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
                <path d="M18 10h-1.26A8 8 0 109 20h9a5 5 0 000-10z" />
              </svg>
              <span>
                {t('warp.note')}
              </span>
            </div>
          )}

          <div className="fld">
            <label className="fld-l" htmlFor="warp-name">
              {t('warp.name')}
            </label>
            <input
              id="warp-name"
              className="input"
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                setErrName(false);
                setDirty(true);
              }}
            />
            {errName && <div className="err-line">{t('warp.errName')}</div>}
          </div>

          <div className="fld">
            <label className="fld-l">{t('warp.plan')}</label>
            <div className="seg2" role="group" aria-label={t('warp.plan')}>
              <button
                type="button"
                className={plan === 'free' ? 'on' : ''}
                onClick={() => {
                  setPlan('free');
                  setErrLicense(false);
                  setDirty(true);
                }}
              >
                {t('warp.planFree')}
              </button>
              <button
                type="button"
                className={plan === 'plus' ? 'on' : ''}
                onClick={() => {
                  setPlan('plus');
                  setDirty(true);
                }}
              >
                WARP+
              </button>
            </div>
          </div>

          {plan === 'plus' && (
            <div className="fld">
              <label className="fld-l fld-l-info" htmlFor="warp-license">
                <span>{t('warp.licenseLabel')}</span>
                <InfoIcon tip={t('warp.licenseHint')} />
              </label>
              <input
                id="warp-license"
                className="input mono"
                value={license}
                onChange={(e) => {
                  setLicense(e.target.value);
                  setErrLicense(false);
                  setDirty(true);
                }}
                placeholder="xxxxxxxx-xxxxxxxx-xxxxxxxx"
              />
              {errLicense && <div className="err-line">{t('warp.errLicense2')}</div>}
            </div>
          )}
          <FormSection
            title={t('node.formGroup.advanced')}
            fields={spec}
            values={draft}
            onChange={setField}
            collapsible
          />

          {submitting && !isEdit && (
            <div className="ts-status">
              <span className="spinner spin-inline" />
              <span>{t('warp.registering')}</span>
            </div>
          )}
        </>
      )}

      {done && (
        <div className="mesh-success">
          <svg viewBox="0 0 24 24" width={18} fill="none" stroke="currentColor" strokeWidth={2.6} style={{ color: 'hsl(var(--ok))' }}>
            <path d="M20 6L9 17l-5-5" />
          </svg>
          <div>
            <b>{t('warp.doneTitle')}</b>
            <div className="card-sub" style={{ marginTop: 3 }}>
              {t('warp.doneSub')}
            </div>
          </div>
        </div>
      )}
    </Modal>
  );
}

export function WarpDialog({ edit }: { edit?: boolean }) {
  // 展示面：编辑基准 + WARP 单例槽判据（含暂存节点更保守：暂存里已有 WARP 时不再发远端注册）。
  const servers = useEffectiveServers();
  const editNode = edit ? findWarpNode(servers) : undefined;
  // R1：key 绑「edit + 目标节点 id」—— 注册态↔编辑态切换重挂重init。
  return <WarpForm key={editNode?.id ?? (edit ? 'edit' : 'new')} editNode={editNode} servers={servers} />;
}

export default WarpDialog;
