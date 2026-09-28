/**
 * 移动端 **WireGuard 字段表**（新增 / 编辑）—— 批 3，销掉 `register:mesh-join:wireguard`。
 *
 * # 复用的是什么，重写的是什么
 *
 * 复用（一条判据都不新造）：
 *  · **字段表** —— `components/dialogs/wg-spec#wgSpec`。本批把它从 `WgDialog.tsx` 里拆进零 React
 *    的 `.ts`（拆分理由见那份文件头注），于是「普通 WG 节点有没有 Reserved 入口」「接入模式在
 *    WARP 下禁不禁用」这些判据两端**同一张表**，桌面加一项移动端自动跟上；
 *  · **分组** —— `mesh-form-layout#groupWgFields`（桌面渲染成页签，这里渲染成可折叠段）；
 *  · **`.conf` 解析** —— `wg-logic#parseConfToDraft` → `domain/wg-quick#parseWgQuickConf`，
 *    **勿重写解析器**；
 *  · **草稿 ⇄ ServerConfig** —— `emptyWgDraft` / `draftFromServer` / `buildWgServer`；
 *  · **必填校验与 Reserved 校验** —— `validateWgDraft` / `reservedInputInvalid`
 *    （后者拦的是「填了但不满足消费侧谓词」，不拦就是「界面收下了、盘上没有」）；
 *  · **组网单例硬闸门** —— `domain/mesh-singleton-guard#blockedByMeshSingleton`。
 *    🔴 **本面板是那道闸最真实的旁路腿**：粘贴 Cloudflare 的 wg-quick `.conf`，端点
 *    `engage.cloudflareclient.com` 会被 `isWarpServer` 的域名兜底判成 WARP —— 已有 WARP 时
 *    造出第二个 ⇒ 两者抢内核 utun ⇒ `Connect: resource busy` FATAL（真机实证）；
 *  · **暂存分流** —— `lib/staged-config#editRoute`（**唯一**闸门，不在别处再写第二个 if）。
 *
 * 重写：呈现层（契约 A1），以及分组的呈现形态（页签 → 可折叠段，理由逐字同 `NodeFormPanel`：
 * 手机上页签把一张要纵向滚的表切成「看不见另一半」，而校验失败的那个字段就在同一根滚动轴上）。
 *
 * # 私钥（`secret`）在这条腿上的三条纪律
 *
 *  ① **输入框类型**：`wgSpec` 把 `privateKey` / `preSharedKey` 标成 `secret: true`，
 *     `FormFields#MobileField` 据此渲染 `type="password"`（默认遮住）+ 一颗有文字标签的显隐键。
 *     这条不在本文件里判 —— 判据在那张共用的表上，两端同源。
 *  ② **不进日志**：本文件的 `console.error` 只吐 `e`（IPC 错误对象），**绝不吐 `draft`**。
 *  ③ **不进错误消息**：失败回显走 `common.saveFailed` 与既有的 `wg.err*` 键，它们都不带值插值。
 *     `buildWgServer` 的产物也不会被打出来。
 */

import { useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { revealElement, useRevealAfterCommit } from '@/components/reveal';
import { api } from '@/ipc';
import type { MeshInboundPolicy } from '@/contracts/types';
import type { FormValue, SelectOption } from '@/components/dialogs/field-spec';
import { wgSpec } from '@/components/dialogs/wg-spec';
import { groupWgFields, type WgFormGroup } from '@/components/dialogs/mesh-form-layout';
import {
  buildWgServer,
  draftFromServer,
  emptyWgDraft,
  isWarpDraft,
  parseConfToDraft,
  reservedInputInvalid,
  validateWgDraft,
  splitCsv,
  type WgDraft,
} from '@/components/dialogs/wg-logic';
import { endpointDetourOptions } from '@/components/dialogs/detour-options';
import { applyMeshInboundPolicy, meshInboundPolicyError, normalizeMeshInboundPolicy } from '@/components/dialogs/mesh-inbound-policy';
import { blockedByMeshSingleton } from '@/domain/mesh-singleton-guard';
import { editRoute } from '@/lib/staged-config';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { MobileFields } from './FormFields';
import { MeshInboundPolicyFields } from './MeshInboundPolicyFields';
import { FormSheet } from './FormSheet';
import { FormGroup } from './FormGroup';
import { useMobileFormStore } from './form-store';

/** 分组的呈现顺序与标题键（分区本身来自 `groupWgFields`，这里只决定顺序与标题）。 */
const WG_GROUPS: ReadonlyArray<readonly [WgFormGroup, string]> = [
  ['basic', 'node.formGroup.connection'],
  ['routing', 'node.formGroup.routing'],
  ['advanced', 'node.formGroup.advanced'],
];

export function WgPanel({
  instanceId,
  serverId,
}: {
  instanceId: string;
  serverId?: string;
}): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  /* 展示面：编辑基准取 effective（读盘的话暂存过的节点再打开会显示改前的旧值），
     单例槽判据也必须含暂存节点 —— 暂存了一个 WARP 还能再建第二个就白守了。 */
  const servers = useEffectiveServers();
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const interfaces = useNetworkInterfaces();

  const base = serverId === undefined ? undefined : servers.find((s) => s.id === serverId);
  const isEdit = base !== undefined;

  // R1 同步初始化：挂载即带正确值，绝不挂载后 reset（换编辑目标由宿主按 instanceId 重挂）。
  const [src, setSrc] = useState<'manual' | 'conf'>('manual');
  const [name, setName] = useState(base?.name ?? '');
  const [draft, setDraft] = useState<WgDraft>(() =>
    base !== undefined ? draftFromServer(base) : emptyWgDraft(),
  );
  const [meshPolicy, setMeshPolicy] = useState<MeshInboundPolicy | undefined>(base?.meshInboundPolicy);
  const [meshPolicyError, setMeshPolicyError] = useState<string | null>(null);
  const [meshPolicyErrorVersion, setMeshPolicyErrorVersion] = useState(0);
  const [confText, setConfText] = useState('');
  const [dirty, setDirty] = useState(false);
  const [errName, setErrName] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const [openGroups, setOpenGroups] = useState<ReadonlySet<WgFormGroup>>(new Set(['basic']));
  const scheduleReveal = useRevealAfterCommit();

  const setField = (k: string, v: FormValue): void => {
    setDraft((d) => ({ ...d, [k]: v }) as WgDraft);
    setDirty(true);
  };

  /* 前置代理候选：排除自身与 endpoint 类节点（判据对齐生成侧，见 `detour-options.ts`）。
     与桌面 `WgDialog` **同一个函数**，不在这里另算一份射程。 */
  const detourOpts = endpointDetourOptions(servers, base?.id, t('node.detourDirect'));
  const interfaceOpts: SelectOption[] = buildNetworkInterfaceChoices(
    interfaces.items,
    draft.bindInterface,
    {
      defaultLabel: t('node.bindInterfaceInherit'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    },
  ).map(({ value, label, disabled }) => [value, label, disabled] as SelectOption);
  const groups = groupWgFields(wgSpec(draft, base, detourOpts, interfaceOpts));

  const onParse = (): void => {
    const parsed = parseConfToDraft(confText);
    if (parsed === null) {
      setNotice({ tone: 'err', text: t('wg.parseErr') });
      return;
    }
    /* `.conf` 只承载协议字段；物理出口是本机策略，粘贴配置时不得顺手清掉（同桌面）。 */
    setDraft({ ...parsed, bindInterface: draft.bindInterface });
    setNotice({ tone: 'ok', text: t('wg.parsed') });
    setSrc('manual');
    setDirty(true);
  };

  /** 关闭意图的唯一出口。脏态 ⇒ 叠一层确认（复用 `confirm` 地基，不新造一种问法）。 */
  const requestClose = (): void => {
    if (!dirty) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('wg.discardTitle'),
        message: t('wg.discardMsg'),
        confirmLabel: t('wg.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  /**
   * 校验失败 ⇒ 回手动填写页并**掰开出错字段所在的那一组**（页签那条腿在移动端的等价物）。
   * 错误文案落在脚上方的 `notice` 上，与被掰开的那一组在同一根滚动轴上。
   */
  const revealGroup = (group: WgFormGroup): void => {
    setSrc('manual');
    setOpenGroups((prev) => new Set([...prev, group]));
  };

  const submit = async (): Promise<void> => {
    const invalid = validateWgDraft(name, draft);
    if (invalid !== null) {
      if (invalid.field === 'name') {
        setErrName(true);
        setSrc('manual');
        return;
      }
      /* 文案自述完整（列全了缺哪些字段），不套 title —— 同桌面。 */
      revealGroup('basic');
      setNotice({ tone: 'err', text: t('wg.errRequired') });
      return;
    }
    /* 填了但不满足消费侧谓词 ⇒ 拦下。不拦的话后端**静默忽略**，用户只会看到「保存成功但没生效」。 */
    if (reservedInputInvalid(draft.reserved)) {
      revealGroup('advanced');
      setNotice({ tone: 'err', text: t('wg.errReserved') });
      return;
    }
    const policy = normalizeMeshInboundPolicy(meshPolicy);
    const policyError = !isWarpDraft(draft, base)
      ? meshInboundPolicyError(policy, 'wireguard', splitCsv(draft.localAddress)) : null;
    setMeshPolicyError(policyError);
    if (policyError) {
      setMeshPolicyErrorVersion((version) => version + 1);
      setSrc('manual');
      setNotice({ tone: 'err', text: t(policyError) });
      return;
    }
    setSubmitting(true);
    try {
      const server = buildWgServer(name, draft, base);
      if (!isWarpDraft(draft, base)) applyMeshInboundPolicy(server, policy);
      /* WARP 单例硬闸门（见文件头「本面板是那道闸最真实的旁路腿」）。
         传 `base?.id`：编辑现有 WARP 节点不算「再加一个」，必须放行。 */
      if (
        blockedByMeshSingleton(server, servers, t, base?.id, (message) =>
          /* 闸不过的**唯一**反馈此前只有全局 toast，而这张表是个全屏层、toast 宿主贴在它下面的
             停靠区上沿（层叠已修，见 `mobile.css#.m-toast-host`）。文案仍只有
             `meshSingletonMessage` 一份，这里只是把同一句也落进面板内。 */
          setNotice({ tone: 'err', text: message }),
        )
      ) {
        setSubmitting(false);
        return;
      }
      // 暂存灰度的**唯一**闸门（与桌面同一个 `editRoute`），不在别处再写第二个 if。
      if (editRoute('servers', stagingEnabled) === 'staged') {
        /* 新增时前端自铸 id：后端 `ensure_server_id` 只在落盘那一刻补 id，而暂存条目现在就需要
           一个稳定的实体寻址键（同一节点重复编辑要覆盖同一条）。 */
        const entityId = server.id !== '' ? server.id : crypto.randomUUID();
        stage({
          id: `server:${entityId}`,
          kind: 'server',
          label: `${isEdit ? t('wg.editTitle') : t('wg.addTitle')} ${server.name}`,
          entityPath: ['servers', entityId],
          nextValue: { ...server, id: entityId },
        });
        closeInstance(instanceId);
        return; // 零 IPC 写、零磁盘写
      }
      if (isEdit) {
        await api.server.update(server);
      } else {
        const { id: _id, ...rest } = server;
        await api.server.add(rest);
      }
      /* 写后端即刷 store（同桌面五个写节点的表单）：`store.servers` 只由 loadConfig/saveConfig 写，
         不刷则本屏列表看不到这次改动 —— 后端广播是慢路径，表单已经关了。 */
      await loadConfig(true);
      closeInstance(instanceId);
    } catch (e) {
      /* 🔴 只吐错误对象，**绝不吐 `draft` / `server`** —— 里面有私钥（见文件头纪律 ②③）。 */
      console.error('[mobile-wg-form] save failed:', e);
      setNotice({ tone: 'err', text: t('common.saveFailed') });
    } finally {
      if (hasInstance(instanceId)) setSubmitting(false);
    }
  };

  return (
    <FormSheet
      title={isEdit ? t('wg.editTitle') : t('wg.addTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={isEdit ? t('common.save') : t('wg.add')}
      submitDisabled={submitting}
      onSubmit={() => void submit()}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mwg-name">
          {t('wg.name')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mwg-name"
          className="m-form-input"
          value={name}
          placeholder={t('wg.namePh')}
          onChange={(e) => {
            setName(e.target.value);
            setErrName(false);
            setDirty(true);
          }}
        />
        {errName && <p className="m-form-err">{t('wg.errName')}</p>}
      </div>

      <div className="m-form-row">
        <span className="m-form-label" id="mwg-src">
          {t('wg.source')}
        </span>
        <div className="m-form-seg" role="group" aria-labelledby="mwg-src">
          <button
            type="button"
            className={src === 'manual' ? 'on' : ''}
            aria-pressed={src === 'manual'}
            onClick={() => setSrc('manual')}
          >
            {t('wg.manual')}
          </button>
          <button
            type="button"
            className={src === 'conf' ? 'on' : ''}
            aria-pressed={src === 'conf'}
            onClick={() => setSrc('conf')}
          >
            {t('wg.paste')}
          </button>
        </div>
      </div>

      {src === 'conf' && (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor="mwg-conf">
            {t('wg.pasteLabel')}
          </label>
          <textarea
            id="mwg-conf"
            className="m-form-input m-form-area mono"
            rows={7}
            value={confText}
            placeholder={'[Interface]\nPrivateKey = wOE...=\nAddress = 10.0.0.2/32\n\n[Peer]\nPublicKey = HIg...=\nEndpoint = 203.0.113.7:51820\nAllowedIPs = 0.0.0.0/0, ::/0'}
            onChange={(e) => {
              setConfText(e.target.value);
              setDirty(true);
            }}
          />
          <button type="button" className="m-form-btn" onClick={onParse}>
            {t('wg.parse')}
          </button>
          {/* 解析成功即填进下面同一套字段（原型语义：parse 填 manual 表单），
              不另画一块「预览」—— 手机上那块预览会把真正要看的字段挤出屏幕。
              这句说的正是这件事；此前这里误抄成了上面那颗 label 的键（`wg.pasteLabel`），
              渲染出来是同一句「粘贴 wg-quick .conf」重复两遍（2026-09-06 复审 minor）。 */}
          <p className="m-form-hint">{t('wg.pasteHint')}</p>
        </div>
      )}

      {src === 'manual' &&
        WG_GROUPS.map(([id, titleKey]) => (
          <FormGroup
            key={id}
            title={t(titleKey)}
            open={openGroups.has(id)}
            onToggle={(section) => {
              const willOpen = !openGroups.has(id);
              setOpenGroups((prev) => {
                const next = new Set(prev);
                if (next.has(id)) next.delete(id);
                else next.add(id);
                return next;
              });
              scheduleReveal(willOpen && section !== null ? () => revealElement(section) : null);
            }}
          >
            <MobileFields fields={groups[id]} values={draft} onChange={setField} t={t} />
          </FormGroup>
        ))}
      {!isWarpDraft(draft, base) && <MeshInboundPolicyFields idPrefix="mwg" protocol="wireguard"
        value={meshPolicy} errorKey={meshPolicyError} errorVersion={meshPolicyErrorVersion}
        onChange={(next) => { setMeshPolicy(next); setMeshPolicyError(null); setDirty(true); }} />}
    </FormSheet>
  );
}
