/**
 * WgDialog —— WireGuard 添加/编辑弹窗（原型 #wg-dialog :2835；wgSetMode :4910）。
 *
 * 两来源（seg2）：手动填写 / 粘贴 wg-quick .conf。**.conf 解析复用** `domain/wg-quick.ts#parseWgQuickConf`
 * （经 `wg-logic.ts` 的 `parseConfToDraft` 薄封装接线，勿重写解析器）——解析成功即填入同一套表单字段
 * （原型语义：parse 填 manual 表单），再提交。多字段驱动走 D2 FieldSpec 表 + FieldRenderer。
 *
 * WG 允许多实例（携 serverId，异于 WARP 单例槽）：serverId 定义 → 编辑态预填（`draftFromServer`）。
 * 提交经 `api.server.add`（新增）/ `api.server.update`（编辑），protocol:'wireguard' + wireguardSettings
 * 由 `buildWgServer` 组装。R1：`key` 绑 serverId（见导出包装）+ useState 同步初始化。
 */

import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { toast } from '@/lib/error-handler';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { api } from '@/ipc';
import type { MeshInboundPolicy, ServerConfig } from '@/contracts/types';
import { Modal } from './Modal';
import {
  FormTabs,
  type FormValue,
  type SelectOption,
} from './FieldSpec';
import { endpointDetourOptions } from './detour-options';
import { wgSpec } from './wg-spec';
import {
  emptyWgDraft,
  draftFromServer,
  parseConfToDraft,
  buildWgServer,
  validateWgDraft,
  reservedInputInvalid,
  workersInputInvalid,
  isWarpDraft,
  splitCsv,
  type WgDraft,
} from './wg-logic';
import { blockedByMeshSingleton } from '@/domain/mesh-singleton-guard';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { editRoute } from '@/lib/staged-config';
import { useDialogStore } from './dialog-store';
import { groupWgFields } from './mesh-form-layout';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { MeshInboundPolicyEditor, applyMeshInboundPolicy, meshInboundPolicyError, normalizeMeshInboundPolicy } from './MeshInboundPolicyEditor';

const CATCH_ALL = new Set(['0.0.0.0/0', '::/0']);

/* 字段表住在 `./wg-spec`（零 React 纯数据，两个客户端共用；拆分理由见那份文件头注）。
   本文件把它**原样再导出**，故既有的调用点一个字都不用改。
   桌面「多 VPN 兼容」批新加的 `ON_DEMAND_FIELD` 已随表搬进 `./wg-spec`，不在这里再列一遍。 */
export { wgSpec } from './wg-spec';

function WgIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z" />
    </svg>
  );
}

interface WgPreview {
  peer: string;
  address: string;
  pubKey: string;
  routing: string;
  keepalive: string;
}
function previewFromDraft(d: WgDraft): WgPreview {
  const trunc = (k: string) => (k.length > 22 ? `${k.slice(0, 22)}…` : k);
  const specific = splitCsv(d.allowedIPs).filter((a) => !CATCH_ALL.has(a));
  const full = d.allowInternet;
  const routing =
    (full ? 'full tunnel' : '') + (specific.length ? (full ? ' + ' : '') + specific.join(', ') : full ? '' : '—');
  return {
    peer: `${d.address}:${d.port ?? ''}`,
    address: d.localAddress,
    pubKey: trunc(d.peerPublicKey),
    routing,
    keepalive: `${d.persistentKeepalive ?? 25}s`,
  };
}

function WgForm({ base }: { base?: ServerConfig }) {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const close = useDialogStore((s) => s.close);
  const loadConfig = useAppStore((s) => s.loadConfig);
  // 展示面：组网单例槽判据必须含暂存节点，否则暂存了一个 WARP/TS 还能再建第二个。
  const servers = useEffectiveServers();
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const isEdit = base != null;

  const [src, setSrc] = useState<'manual' | 'conf'>('manual');
  const [name, setName] = useState(base?.name ?? '');
  const [draft, setDraft] = useState<WgDraft>(() => (base ? draftFromServer(base) : emptyWgDraft()));
  const [meshPolicy, setMeshPolicy] = useState<MeshInboundPolicy | undefined>(base?.meshInboundPolicy);
  const [confText, setConfText] = useState('');
  const [confErr, setConfErr] = useState<string | null>(null);
  const [preview, setPreview] = useState<WgPreview | null>(null);

  const [dirty, setDirty] = useState(false);
  const [errName, setErrName] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [formTab, setFormTab] = useState('connection');
  const interfaces = useNetworkInterfaces();

  const setField = (k: string, v: FormValue) => {
    setDraft((d) => ({ ...d, [k]: v }) as WgDraft);
    setDirty(true);
  };

  // 前置代理候选：排除自身与 endpoint 类节点（判据对齐生成侧，见 `detour-options.ts`）。
  const detourOpts = endpointDetourOptions(servers, base?.id, t('node.detourDirect'));
  const interfaceOpts: SelectOption[] = buildNetworkInterfaceChoices(
    interfaces.items,
    draft.bindInterface,
    {
      defaultLabel: t('node.bindInterfaceInherit'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    },
  ).map(({ value, label, disabled }) => [value, label, disabled]);
  const fields = wgSpec(draft, base, detourOpts, interfaceOpts);

  const onParse = () => {
    const d = parseConfToDraft(confText);
    if (!d) {
      setPreview(null);
      setConfErr(t('wg.parseErr'));
      return;
    }
    setConfErr(null);
    // .conf 只承载协议字段；物理出口是本机策略，粘贴配置时不得顺手清掉。
    setDraft({ ...d, bindInterface: draft.bindInterface });
    setPreview(previewFromDraft(d));
    setDirty(true);
  };

  const requestClose = () => {
    if (!dirty) {
      close();
      return;
    }
    open({
      kind: 'confirm',
      payload: {
        title: t('wg.discardTitle'),
        message: t('wg.discardMsg'),
        confirmLabel: t('wg.discard'),
        danger: true,
        onConfirm: () => {
          close();
          close();
        },
      },
    });
  };

  const handleSubmit = async () => {
    const err = validateWgDraft(name, draft);
    if (err) {
      if (err.field === 'name') {
        setErrName(true);
      } else {
        setSrc('manual');
        setFormTab('connection');
        // 文案自述完整（列全了缺哪些字段），不套 title。
        toast.error(t('wg.errRequired'));
      }
      return;
    }
    // 填了但不满足消费侧谓词 ⇒ 拦下。不拦的话后端**静默忽略**，用户只会看到「保存成功但没生效」
    // （判据与理由见 `wg-logic.ts#reservedInputInvalid`）。回手动填写页，让出错的那个框可见。
    if (reservedInputInvalid(draft.reserved)) {
      setSrc('manual');
      setFormTab('advanced');
      toast.error(t('wg.errReserved'));
      return;
    }
    if (workersInputInvalid(draft.workers)) {
      setSrc('manual');
      setFormTab('advanced');
      toast.error(t('wg.errWorkers'));
      return;
    }
    const policy = normalizeMeshInboundPolicy(meshPolicy);
    const policyError = !isWarpDraft(draft, base)
      ? meshInboundPolicyError(policy, 'wireguard', splitCsv(draft.localAddress))
      : null;
    if (policyError) {
      setSrc('manual');
      setFormTab('routing');
      toast.error(t(policyError));
      return;
    }
    const server = buildWgServer(name, draft, base);
    if (!isWarpDraft(draft, base)) applyMeshInboundPolicy(server, policy);
    // WARP 单例硬闸门。**本弹窗是它最真实的旁路腿**：粘贴 Cloudflare 的 wg-quick `.conf`，端点
    // `engage.cloudflareclient.com` 会被 `isWarpServer` 的域名兜底判成 WARP（`domain/warp.ts:31-37`），
    // 于是在已有 WARP 时造出第二个 → 两者抢内核 utun → `Connect: resource busy` FATAL。
    // 传 base?.id：编辑现有 WARP 节点不算「再加一个」，必须放行。
    if (blockedByMeshSingleton(server, servers, t, base?.id)) return;

    setSubmitting(true);
    try {
      // 配置暂存闸门（与 NodeDialog 同形）。`editRoute` 是**唯一**判据：总开关关 / W-0 豁免 /
      // W-1·2·3 绕过任一命中都返 'direct'，走下面那条与今天逐字节相同的直落盘腿。
      // 手填 / 粘贴 .conf 两条来源都没有远端副作用，故 WG 节点整体落默认腿（进暂存）。
      if (editRoute('servers', stagingEnabled) === 'staged') {
        // 新增时前端自铸 id：后端 `ensure_server_id` 只在落盘那一刻补 id，而条目现在就需要一个稳定的
        // 实体寻址键（同一节点重复编辑要覆盖同一条）。带 id 提交后端照收。
        const entityId = server.id !== '' ? server.id : crypto.randomUUID();
        stage({
          id: `server:${entityId}`,
          kind: 'server',
          label: `${isEdit ? t('wg.editTitle') : t('wg.addTitle')} ${server.name}`,
          entityPath: ['servers', entityId],
          nextValue: { ...server, id: entityId },
        });
        close();
        return; // 零 IPC 写、零磁盘写（FR-1）
      }
      if (isEdit && base) {
        await api.server.update(server);
      } else {
        const { id: _id, ...rest } = server;
        await api.server.add(rest);
      }
      void loadConfig(true);
      close();
    } catch (e) {
      console.error('[WgDialog] save failed:', e);
      toast.error(t('common.saveFailed'));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      titleId="wg-dlg-title"
      title={isEdit ? t('wg.editTitle') : t('wg.addTitle')}
      onClose={requestClose}
      icon={<WgIcon />}
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
            {isEdit ? t('common.save') : t('wg.add')}
          </button>
        </>
      }
    >
      <div className="fld">
        <label className="fld-l" htmlFor="wg-name">
          {t('wg.name')}
        </label>
        <input
          id="wg-name"
          className="input"
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            setErrName(false);
            setDirty(true);
          }}
          placeholder={t('wg.namePh')}
        />
        {errName && <div className="err-line">{t('wg.errName')}</div>}
      </div>

      <div className="fld">
        <label className="fld-l">{t('wg.source')}</label>
        <div className="seg2" role="group" aria-label={t('wg.source')} style={{ display: 'flex' }}>
          <button type="button" style={{ flex: 1 }} className={src === 'manual' ? 'on' : ''} onClick={() => setSrc('manual')}>
            {t('wg.manual')}
          </button>
          <button type="button" style={{ flex: 1 }} className={src === 'conf' ? 'on' : ''} onClick={() => setSrc('conf')}>
            {t('wg.paste')}
          </button>
        </div>
      </div>

      {src === 'conf' && (
        <>
          <div className="fld">
            <label className="fld-l" htmlFor="wg-conf">
              {t('wg.pasteLabel')}
            </label>
            <textarea
              id="wg-conf"
              className="input mono"
              rows={7}
              value={confText}
              onChange={(e) => {
                setConfText(e.target.value);
                setConfErr(null);
                setDirty(true);
              }}
              placeholder={
                '[Interface]\nPrivateKey = wOE...=\nAddress = 10.0.0.2/32\n\n[Peer]\nPublicKey = HIg...=\nEndpoint = 203.0.113.7:51820\nAllowedIPs = 0.0.0.0/0, ::/0'
              }
            />
          </div>
          <button type="button" className="btn ghost sm" onClick={onParse}>
            <svg viewBox="0 0 24 24" width={14} fill="none" stroke="currentColor" strokeWidth={1.8}>
              <path d="M9 15l6-6M8 8a3 3 0 10-3 3M16 16a3 3 0 103 3" />
            </svg>
            <span>{t('wg.parse')}</span>
          </button>
          {confErr && <div className="wg-conf-err">{confErr}</div>}
          {preview && (
            <div className="wg-preview">
              <div className="wpv-h">
                <svg viewBox="0 0 24 24" width={14} fill="none" stroke="currentColor" strokeWidth={2.6}>
                  <path d="M20 6L9 17l-5-5" />
                </svg>
                {t('wg.parsed')}
              </div>
              <div className="wpv-row"><span>{t('wg.pvPeer')}</span><span>{preview.peer}</span></div>
              <div className="wpv-row"><span>{t('wg.pvAddr')}</span><span>{preview.address}</span></div>
              <div className="wpv-row"><span>{t('wg.pvPub')}</span><span>{preview.pubKey}</span></div>
              <div className="wpv-row"><span>{t('wg.pvRoute')}</span><span>{preview.routing}</span></div>
              <div className="wpv-row"><span>{t('wg.pvKeep')}</span><span>{preview.keepalive}</span></div>
            </div>
          )}
        </>
      )}

      {src === 'manual' && (() => {
        const groups = groupWgFields(fields);
        return (
          <FormTabs
            id="wg-form"
            ariaLabel={t('node.formGroup.aria')}
            tabs={[
              { id: 'connection', label: t('node.formGroup.connection'), fields: groups.basic },
              { id: 'routing', label: t('node.formGroup.routing'), fields: groups.routing,
                children: !isWarpDraft(draft, base) && <MeshInboundPolicyEditor idPrefix="wg" value={meshPolicy} onChange={(next) => { setMeshPolicy(next); setDirty(true); }} /> },
              { id: 'advanced', label: t('node.formGroup.advanced'), fields: groups.advanced },
            ]}
            active={formTab}
            onSelect={setFormTab}
            values={draft}
            onChange={setField}
          />
        );
      })()}
    </Modal>
  );
}

export function WgDialog({ serverId }: { serverId?: string }) {
  // 展示面：编辑基准（读盘的话暂存过的节点再打开会显示改前的旧值）。
  const servers = useEffectiveServers();
  const base = serverId ? servers.find((s) => s.id === serverId) : undefined;
  // R1：key 绑 serverId —— 切换编辑目标 = 重挂 = 同步重新初始化。
  return <WgForm key={serverId ?? 'new'} base={base} />;
}

export default WgDialog;
