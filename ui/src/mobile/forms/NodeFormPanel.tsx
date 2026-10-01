import { MobileInfo } from '../MobileInfo';
import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端**节点表单**（新增 / 编辑 / 组网隧道新增）—— W-00 的主消费者。
 *
 * # 复用的是什么，重写的是什么
 *
 * 复用（一条判据都不新造）：
 *  · 协议清单与分组 —— `node-spec.ts` 的 `PROTO_OPTIONS` / `PROTO_GROUP_ORDER` / `protosInGroup`
 *    / `meshTunnelNodeProtocols` / `defaultPortPlaceholder`；
 *  · 字段表与任务分组 —— `allFields` / `nodeFormGroups`（`nodeFormUsesTabs` **不复用**：见下）；
 *  · 草稿 ⇄ `ServerConfig` 的对称层 —— `protoCodec[proto].fromConfig/toConfig`（R5 以 base 起底
 *    保全非模型字段）；
 *  · 组网隧道的本地语法门 —— `mesh-form-layout.ts#meshTunnelDraftError`；
 *  · 组网单例硬闸门 —— `domain/mesh-singleton-guard#blockedByMeshSingleton`（**凡直调 `server:add`
 *    者皆过闸**，这条不变量必须钉在每一条腿上）；
 *  · 暂存分流 —— `lib/staged-config#editRoute`（**唯一**闸门，不在别处再写第二个 if）；
 *  · 网卡下拉投影 —— `hooks/use-network-interfaces#buildNetworkInterfaceChoices`；
 *  · 无地址协议 / Tailcat 保存前同宽门 —— `domain/server-completeness` 的 `isAddresslessProtocol`
 *    / `tailcatSettingsError`（镜像 Rust `tailcat_emit_check`），拒因文案走 `INVALID_NODE_REASON_KEY`；
 *  · 编解码被拒时的出错分组 —— `nodeFieldGroup`；custom 探测判读 —— `describeProbeResult`。
 *
 * 与桌面表单级能力的逐条对差由 `node-form-parity.test.tsx` 从 `NodeDialog.tsx` 源码派生
 * （并喂进 `wiring-completeness.test.ts` 的 B 面）：桌面这里再长一条能力，移动端没跟就红。
 *
 * 重写：呈现层（契约 A1），以及**分组的呈现形态**。
 *
 * # 为什么不用 `nodeFormUsesTabs`（唯一一处刻意的形态偏离）
 *
 * 桌面对 5 个字段最多的协议开页签（`FormTabs`）。手机上页签把一张本来就要纵向滚的表切成
 * 「看不见另一半」——校验失败要先跳页签再滚动，而错误就在同一根滚动轴上。这里改成**逐组可折叠段**：
 * `basic` 与连接主路径 `transport` 默认展开；可选的 `routing` / `advanced` 默认折叠，
 * 编辑态已配置的可选组随初始草稿展开。报错时自动展开并定位出错的组。
 * 页签与折叠段承载的是同一个 `nodeFormGroups` 分区，没有第二套分组真值。
 *
 * # 编辑态的同步初始化（R1）
 *
 * `useState(初始化器)` 挂载即带正确值，**不存在挂载后 reset 的路径**；换节点由宿主按
 * `instanceId` 重挂（每次 `open` 一个新实例）。这条与桌面 `NodeDialog` 的 `key={serverId ?? 'new'}`
 * 等价，理由同 R1。
 */

import { useMemo, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
/* 「展开即露出」的全仓不变量（`components/reveal.ts`，纯 `.ts`、无 CSS、不导出组件 ⇒ 契约 A1
   的两条风险一条都不成立）。本面板的 `.m-form-body` 同样是 `overflow-y:auto` 的单一滚动容器：
   底部那个分组展开时，新长出来的字段整段落在视区之外 —— 与桌面四个分组菜单**同一形状**，
   故走同一条腿，不在这里另写一个滚动。 */
import { revealElement, useRevealAfterCommit } from '@/components/reveal';
import { api, IpcError } from '@/ipc';
import { toast } from '@/lib/error-handler';
import type { MeshInboundPolicy, ServerConfig } from '@/contracts/types';
import {
  parseNumberField,
  draftFromSpecs,
  type FormValue,
  type FormValues,
} from '@/components/dialogs/field-spec';
import {
  PROTO_GROUP_ORDER,
  PROTO_OPTIONS,
  allFields,
  defaultPortPlaceholder,
  describeProbeResult,
  isMeshTunnelNodeProtocol,
  meshTunnelNodeProtocols,
  nodeFieldGroup,
  nodeFormGroups,
  protosInGroup,
  type NodeFieldGroupId,
  type NodeProto,
  type ProbeDisplay,
} from '@/components/dialogs/node-spec';
import { protoCodec, ProtoCodecError } from '@/components/dialogs/proto-codec';
import { meshTunnelDraftError } from '@/components/dialogs/mesh-form-layout';
import { applyMeshInboundPolicy, meshInboundPolicyError, normalizeMeshInboundPolicy } from '@/components/dialogs/mesh-inbound-policy';
import { applyDetour } from '@/components/dialogs/detour-options';
import { blockedByMeshSingleton } from '@/domain/mesh-singleton-guard';
import { isAddresslessProtocol, tailcatSettingsError } from '@/domain/server-completeness';
import { INVALID_NODE_REASON_KEY } from '@/domain/invalid-node-reason';
import { editRoute } from '@/lib/staged-config';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { useAppStore, useEffectiveConfig, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { MobileFields } from './FormFields';
import { MeshInboundPolicyFields } from './MeshInboundPolicyFields';
import { FormGroup } from './FormGroup';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';
import { expandedNodeFormGroups, initialNodeFormGroups } from './node-form-visibility';

const NODE_PROTOS = new Set<string>(PROTO_OPTIONS.map(([p]) => p));
function isNodeProto(p: string): p is NodeProto {
  return NODE_PROTOS.has(p);
}

export function NodeFormPanel({
  instanceId,
  serverId,
  initialProto,
}: {
  instanceId: string;
  serverId?: string;
  initialProto?: NodeProto;
}): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const servers = useEffectiveServers();
  const config = useEffectiveConfig();
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const interfaces = useNetworkInterfaces();

  const base = serverId === undefined ? undefined : servers.find((s) => s.id === serverId);
  const isEdit = base !== undefined;

  const initProto: NodeProto =
    base !== undefined && isNodeProto(base.protocol) ? base.protocol : (initialProto ?? 'vless');
  const meshTunnelForm = isMeshTunnelNodeProtocol(initProto);

  // R1 同步初始化：挂载即带正确值，绝不挂载后 reset。
  const [proto, setProto] = useState<NodeProto>(initProto);
  const [name, setName] = useState(base?.name ?? '');
  const [address, setAddress] = useState(base?.address ?? '');
  const [portStr, setPortStr] = useState(base?.port != null ? String(base.port) : '443');
  const [detour, setDetour] = useState(base?.detour ?? '');
  const [bindInterface, setBindInterface] = useState(base?.bindInterface ?? '');
  const [draft, setDraft] = useState<FormValues>(() =>
    base !== undefined && isNodeProto(base.protocol)
      ? protoCodec[base.protocol].fromConfig(base)
      : draftFromSpecs(allFields(initProto)),
  );
  const [meshPolicy, setMeshPolicy] = useState<MeshInboundPolicy | undefined>(base?.meshInboundPolicy);
  const [meshPolicyError, setMeshPolicyError] = useState<string | null>(null);
  const [meshPolicyErrorVersion, setMeshPolicyErrorVersion] = useState(0);
  const [dirty, setDirty] = useState(false);
  const [errName, setErrName] = useState(false);
  const [errAddr, setErrAddr] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const [openGroups, setOpenGroups] = useState<ReadonlySet<NodeFieldGroupId>>(
    () => initialNodeFormGroups(initProto, draft, {
      editing: base !== undefined,
      detour,
      bindInterface,
      bindInterfaceEditable: base?.subscriptionId === undefined,
    }),
  );
  /* 「本次提交之后再露出」：点的那一刻字段还没渲染，当场量到的是旧 DOM（同 `Csel` 的用法）。 */
  const scheduleReveal = useRevealAfterCommit();
  /* custom 协议内核兼容性探测（同桌面 C10）：结果只在协议为 custom 时被读。 */
  const [probing, setProbing] = useState(false);
  const [probeResult, setProbeResult] = useState<ProbeDisplay | null>(null);
  /* Tailcat 客户端公钥（同桌面 D5）：纯展示态，由后端从私钥推导；私钥一改就作废，不落盘、不进日志。 */
  const [tcPublicKey, setTcPublicKey] = useState('');
  const [tcKeyBusy, setTcKeyBusy] = useState(false);

  const setField = (k: string, v: FormValue): void => {
    setDraft((d) => ({ ...d, [k]: v }));
    setDirty(true);
    /* 两条失效规则逐字同桌面 `NodeDialog.setField`：JSON 改过 ⇒ 上次探测结果对不上新文本；
       私钥改过 ⇒ 展示的公钥已不是它推出来的那把。 */
    if (k === 'outbound') setProbeResult(null);
    if (k === 'privateKey') setTcPublicKey('');
  };

  const changeProto = (next: NodeProto): void => {
    setProto(next);
    /* 换协议：公共字段（名/址/端口/detour）留在各自 state；协议特定字段重置为新协议默认
       （逐字同桌面 `NodeDialog.changeProto`）。 */
    const nextDraft = draftFromSpecs(allFields(next));
    setDraft(nextDraft);
    setDirty(true);
    setOpenGroups(initialNodeFormGroups(next, nextDraft, {
      detour,
      bindInterface,
      bindInterfaceEditable: base?.subscriptionId === undefined,
    }));
    setProbeResult(null);
    setTcPublicKey('');
  };

  /** 把用户带到出错的那一组（页签那条腿在移动端换成「展开那一组」，理由见文件头注）。 */
  const revealGroup = (group: NodeFieldGroupId): void => {
    setOpenGroups((prev) => expandedNodeFormGroups(prev, group));
    scheduleReveal(() => {
      const key = `${instanceId}:${group}`;
      const section = [...document.querySelectorAll<HTMLElement>('[data-form-group]')]
        .find(element => element.dataset.formGroup === key);
      if (!section) return;
      const body = section.closest<HTMLElement>('.m-form-body');
      const header = section.querySelector<HTMLElement>('.m-form-group-h');
      if (body && header && header.getBoundingClientRect().top < body.getBoundingClientRect().top) {
        body.scrollBy({ top: header.getBoundingClientRect().top - body.getBoundingClientRect().top, behavior: 'auto' });
      } else {
        revealElement(section);
      }
    });
  };

  /**
   * custom 协议内核兼容性探测（同桌面 `NodeDialog.runCompatProbe`）：JSON 语法先在本地校验
   * （非法 JSON 过不了 `serde_json::Value` 反序列化），过了才调后端；结果统一经共用的
   * `describeProbeResult` 映射成展示态，两端不各写一份判读。
   */
  const runCompatProbe = async (): Promise<void> => {
    const raw = typeof draft.outbound === 'string' ? draft.outbound : '';
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      setProbeResult({ kind: 'invalidJson' });
      return;
    }
    setProbing(true);
    setProbeResult(null);
    try {
      const r = await api.proxy.probeOutbound(parsed, draft.isEndpoint === true);
      setProbeResult(describeProbeResult(r));
    } catch (e) {
      /* IPC 本身失败不是内核给出的兼容性判定：诊断只进日志；界面落表单通知（本表单宿主的失败回显
         通道），**不**冒充成一次「配置校验失败」的内核判定。 */
      console.error('[mobile-node-form] custom outbound probe failed:', e);
      setNotice({ tone: 'err', text: t('errors.operationFailed') });
    } finally {
      if (hasInstance(instanceId)) setProbing(false);
    }
  };

  /**
   * Tailcat 客户端密钥（同桌面 `NodeDialog.runTailcatKeypair`）：私钥空 ⇒ 后端生成新密钥对并回填私钥；
   * 私钥已填 ⇒ 只推导公钥。推导走后端 `tailcat_keypair`（与 sing-box 同一 X25519 推导），前端不复刻。
   * 失败只给一句本地化文案，不回显后端原文（那里可能带着输入）。
   */
  const runTailcatKeypair = async (): Promise<void> => {
    const current = typeof draft.privateKey === 'string' ? draft.privateKey.trim() : '';
    setTcKeyBusy(true);
    try {
      const r = await api.server.tailcatKeypair(current || undefined);
      if (!current) setField('privateKey', r.privateKey);
      setTcPublicKey(r.publicKey);
    } catch (e) {
      console.error('[mobile-node-form] tailcat keypair failed:', e instanceof IpcError ? e.code : 'unknown');
      setNotice({ tone: 'err', text: t('node.tcKeyFailed') });
    } finally {
      if (hasInstance(instanceId)) setTcKeyBusy(false);
    }
  };

  const groups = useMemo(() => nodeFormGroups(proto), [proto]);

  /* 协议下拉的分组。组网隧道入口只列隧道那两个 —— 两套入口互斥，编辑时也沿用节点所属入口
     （同桌面：不把 OpenConnect/OpenVPN 混回普通协议分类）。 */
  const protoLabel = new Map<string, string>(PROTO_OPTIONS.map(([v, l]) => [v, l]));
  const protoGroups = meshTunnelForm
    ? [{ label: t('meshJoin.tunnels'), protos: meshTunnelNodeProtocols() }]
    : PROTO_GROUP_ORDER.map((g) => ({
        label: t(`node.protoGroup.${g}`),
        protos: protosInGroup(g),
      })).filter((g) => g.protos.length > 0);

  /* 前置代理候选：`direct`（哨兵，提交时翻译成删键）+ 其余节点（排除自身）。
     与桌面 `NodeDialog` 的 `detourOpts` 同一构造，不改射程。 */
  const detourOptions = [
    { value: 'direct', label: t('node.detourDirect') },
    ...servers.filter((s) => s.id !== base?.id).map((s) => ({ value: s.id, label: s.name })),
  ];

  const subscriptionInterface =
    base?.subscriptionId !== undefined
      ? (config?.subscriptions?.find((sub) => sub.id === base.subscriptionId)?.proxyBindInterface ??
        config?.networkInterfaces?.proxy ??
        '')
      : '';
  const effectiveInterfaceValue =
    base?.subscriptionId !== undefined ? subscriptionInterface : bindInterface;
  const interfaceOptions = buildNetworkInterfaceChoices(interfaces.items, effectiveInterfaceValue, {
    defaultLabel:
      base?.subscriptionId !== undefined
        ? t('node.bindInterfaceInheritedAuto')
        : t('node.bindInterfaceInherit'),
    unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
    down: t('settings.network.interfaceDown'),
  });

  /** 关闭意图的唯一出口。脏态 ⇒ 叠一层确认（复用 `confirm` 地基，不新造一种问法）。 */
  const requestClose = (): void => {
    if (!dirty) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('node.discardTitle'),
        message: t('node.discardMsg'),
        confirmLabel: t('node.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  const submit = async (): Promise<void> => {
    const nameEmpty = name.trim() === '';
    const port = parseNumberField(portStr);
    /* 无地址协议（Tor / Tailcat）不收地址行，也就不校验它；落盘写空地址 + 0 端口（同桌面 D8）。 */
    const addressless = isAddresslessProtocol(proto);
    const addrEmpty = !addressless && (address.trim() === '' || port === undefined);
    setErrName(nameEmpty);
    setErrAddr(addrEmpty);
    if (nameEmpty || addrEmpty) return;

    const meshError = meshTunnelDraftError(proto, draft);
    if (meshError !== null) {
      /* 页签那条腿在移动端换成「展开出错的那一组」：错误与它所在的字段必须在同一根滚动轴上
         同时可见，否则提示说了「基础里有必填没填」而用户看着一个收起来的段。 */
      revealGroup(meshError.group);
      setNotice({
        tone: 'err',
        text:
          meshError.key === 'json'
            ? t('node.meshTunnelJsonInvalid')
            : t('node.meshTunnelRequired'),
      });
      return;
    }

    const policy = normalizeMeshInboundPolicy(meshPolicy);
    const policyError = isMeshTunnelNodeProtocol(proto) ? meshInboundPolicyError(policy, proto) : null;
    setMeshPolicyError(policyError);
    if (policyError) {
      setMeshPolicyErrorVersion((version) => version + 1);
      setNotice({ tone: 'err', text: t(policyError) });
      return;
    }

    setSubmitting(true);
    try {
      const meta: ServerConfig = {
        id: base?.id ?? '',
        name: name.trim(),
        protocol: proto,
        address: addressless ? '' : address.trim(),
        port: addressless ? 0 : (port as number),
      };
      if (detour !== '') meta.detour = detour;
      if (base?.subscriptionId === undefined && bindInterface !== '') meta.bindInterface = bindInterface;
      if (base !== undefined) {
        meta.createdAt = base.createdAt;
        meta.subscriptionId = base.subscriptionId;
        meta.providerName = base.providerName;
      }
      // 同协议编辑 → base 起底保全非模型字段（R5）；改型/新增 → 干净 base 防旧协议残留。
      const codecBase = base !== undefined && base.protocol === proto ? { ...base, ...meta } : meta;
      const full = protoCodec[proto].toConfig(draft, codecBase);
      if (isMeshTunnelNodeProtocol(proto)) applyMeshInboundPolicy(full, policy);
      if (base?.subscriptionId !== undefined || bindInterface === '') delete full.bindInterface;
      else full.bindInterface = bindInterface;
      /* 🔴 前置代理与 `bindInterface` **同形**（2026-09-06 复审，两端同批改）：上面那句
         `if (detour !== '') meta.detour = detour` 在选了「不串联」时不写键，而 `codecBase` 是
         `{ ...base, ...meta }` ⇒ 存量 `base.detour` 从 spread 里活下来，`protoCodec.toConfig`
         全程不碰 detour ⇒ 界面显示「不串联」而盘上的串联关系没清掉、流量照走前置代理。
         走共用的 `applyDetour`（空值/哨兵 ⇒ **删键**，不写字面量 'direct'）——
         Wg / Ts / Warp 三个表单本来就在用它，只有节点表单这条腿此前手写。 */
      applyDetour(full, detour);

      /* Tailcat 的 key / DERP 形态门，与生成侧、store 落盘门同一判据（`tailcatSettingsError` 镜像 Rust
         `tailcat_emit_check`，同桌面）。不在这里拦，节点会被 store 的 sanitize 静默丢掉 ——
         用户看到的是「保存了但没了」。密钥与 DERP 字段都在 basic 组。 */
      const tailcatReason =
        full.protocol === 'tailcat' ? tailcatSettingsError(full.tailcatSettings) : null;
      if (tailcatReason !== null) {
        revealGroup('basic');
        setNotice({
          tone: 'err',
          text: `${t('common.saveFailed')}${t('common.colon')}${t(INVALID_NODE_REASON_KEY[tailcatReason])}`,
        });
        return; // submitting 由下方 finally 复位
      }

      /* 组网单例硬闸门。今天在本表单恒不命中（`PROTO_OPTIONS` 不含 wireguard/tailscale），
         留着不是防御性冗余：它把「凡直调 `server:add` 者皆过闸」钉在**每一条**腿上，
         协议表增补一行就会让本腿变成活腿，届时没人会想起来补闸门（同桌面 NodeDialog 的理由）。 */
      if (blockedByMeshSingleton(full, servers, t, base?.id)) {
        setSubmitting(false);
        return;
      }

      // 暂存灰度的**唯一**闸门（与桌面同一个 `editRoute`），不在别处再写第二个 if。
      if (editRoute('servers', stagingEnabled) === 'staged') {
        /* 新增时前端自铸 id：后端 `ensure_server_id` 只在落盘那一刻补 id，而暂存条目现在就需要
           一个稳定的实体寻址键（同一节点重复编辑要覆盖同一条）。 */
        const entityId = full.id !== '' ? full.id : crypto.randomUUID();
        stage({
          id: `server:${entityId}`,
          kind: 'server',
          label: `${isEdit ? t('node.editTitle') : t('node.addTitle')} ${meta.name}`,
          entityPath: ['servers', entityId],
          nextValue: { ...full, id: entityId },
        });
        closeInstance(instanceId);
        return; // 零 IPC 写、零磁盘写
      }

      if (isEdit && base !== undefined) {
        await api.server.update(full);
      } else {
        const { id: _id, ...rest } = full;
        await api.server.add(rest);
      }
      /* 写后端即刷 store（同桌面五个写节点的表单）：`store.servers` 只由 loadConfig/saveConfig 写，
         不刷则本屏列表看不到这次改动 —— 后端广播是慢路径，表单已经关了。 */
      await loadConfig(true);
      closeInstance(instanceId);
      toast.success(t('common.saved'));
    } catch (e) {
      console.error('[mobile-node-form] save failed:', e);
      /* 编解码被拒（证书固定值 / DERP 行 / 扩展 JSON 非法…）⇒ 展开出错字段所在的那一组（同桌面）。 */
      // The shared codec identifies most rejected fields directly. Certificate pin
      // and custom JSON errors predate that metadata; both have an unambiguous
      // group in ND_SPEC, so the mobile fold can still reveal the failing input.
      const codecField = e instanceof ProtoCodecError
        ? e.field ?? (e.code === 'certPinInvalid' ? 'certSha256'
          : e.code === 'customJsonInvalid' || e.code === 'customJsonObject' || e.code === 'customJsonTypeRequired'
            ? 'outbound' : undefined)
        : undefined;
      const codecGroup = codecField ? nodeFieldGroup(proto, codecField) : null;
      if (codecGroup !== null) revealGroup(codecGroup);
      const detail =
        e instanceof ProtoCodecError
          ? t(`node.codecError.${e.code}`, { detail: e.detail ?? '' })
          : e instanceof IpcError
            ? t('errors.operationFailed')
            : t('errors.operationFailed');
      setNotice({ tone: 'err', text: `${t('common.saveFailed')}${t('common.colon')}${detail}` });
    } finally {
      if (hasInstance(instanceId)) setSubmitting(false);
    }
  };

  const groupTitle = (id: NodeFieldGroupId): string =>
    id === 'transport'
      ? t('node.formGroup.transport')
      : id === 'routing'
        ? t('node.formGroup.routing')
        : id === 'advanced'
          ? t('node.formGroup.advanced')
          : t('node.formGroup.basic');

  return (
    <FormSheet
      title={isEdit ? t('node.editTitle') : meshTunnelForm ? t('meshJoin.title') : t('node.addTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={isEdit ? t('common.save') : t('node.add')}
      submitDisabled={submitting}
      onSubmit={() => void submit()}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mnf-proto">
          {t('node.protocol')}
        </label>
        <MobileSelect
          id="mnf-proto"
          className="m-form-select"
          value={proto}
          onChange={(e) => changeProto(e.target.value as NodeProto)}
        >
          {protoGroups.map((g) => (
            <optgroup key={g.label} label={g.label}>
              {g.protos.map((p) => (
                <option key={p} value={p}>
                  {protoLabel.get(p) ?? p}
                </option>
              ))}
            </optgroup>
          ))}
        </MobileSelect>
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mnf-name">
          {t('node.label')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mnf-name"
          className="m-form-input"
          value={name}
          placeholder={t('node.labelPh')}
          onChange={(e) => {
            setName(e.target.value);
            setDirty(true);
            setErrName(false);
          }}
        />
        {errName && <p className="m-form-err">{t('node.errName')}</p>}
      </div>

      {/* 地址 / 端口（无地址协议不渲染，同桌面 D8：对端由协议自己的字段定位）。 */}
      {!isAddresslessProtocol(proto) && (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor="mnf-addr">
            {t('node.serverPort')}
            <span className="m-form-req" aria-hidden>
              *
            </span>
          </label>
          <div className="m-form-pair">
            <input
              id="mnf-addr"
              className="m-form-input"
              value={address}
              placeholder="example.com"
              aria-label={t('node.server')}
              onChange={(e) => {
                setAddress(e.target.value);
                setDirty(true);
                setErrAddr(false);
              }}
            />
            <input
              className="m-form-input"
              inputMode="numeric"
              value={portStr}
              placeholder={defaultPortPlaceholder(proto)}
              aria-label={t('node.port')}
              onChange={(e) => {
                setPortStr(e.target.value);
                setDirty(true);
                setErrAddr(false);
              }}
            />
          </div>
          {errAddr && <p className="m-form-err">{t('node.errAddr')}</p>}
        </div>
      )}

      {groups.map((g) => (
        <FormGroup
          key={g.id}
          groupId={`${instanceId}:${g.id}`}
          title={groupTitle(g.id)}
          open={openGroups.has(g.id)}
          onToggle={(section) => {
            const willOpen = !openGroups.has(g.id);
            setOpenGroups((prev) => {
              const next = new Set(prev);
              if (next.has(g.id)) next.delete(g.id);
              else next.add(g.id);
              return next;
            });
            /* 只在**展开**时滚：折叠只会让内容变短，此时滚动等于凭空把用户挪走
               （逐字同 `revealOnToggle` 的那条理由）。 */
            scheduleReveal(willOpen && section !== null ? () => revealElement(section) : null);
          }}
        >
          <MobileFields fields={g.fields} values={draft} onChange={setField} t={t} />
          {/* Tailcat 客户端公钥 + 生成 / 推导（同桌面挂在「连接」页末尾；移动端 = basic 组末尾，
              紧挨私钥那一格）。按钮独占一行：窄屏上与公钥框并排会把公钥挤窄。 */}
          {g.id === 'basic' && proto === 'tailcat' && (
            <div className="m-form-row">
              <label className="m-form-label" htmlFor="mnf-tc-pub">
                {t('node.tcPublicKey')}
              </label>
              {/* 只读多行框而非单行 input：44 字符的 base64 公钥在 360dp 窄屏上单行放不下，
                  单行框会把尾巴藏进横向滚动里 —— 这把钥匙是要被人抄给服务端的，得整串可见。 */}
              <textarea
                id="mnf-tc-pub"
                className="m-form-input m-form-area mono"
                readOnly
                rows={2}
                value={tcPublicKey}
              />
              <button
                type="button"
                className="m-form-btn"
                disabled={tcKeyBusy}
                onClick={() => void runTailcatKeypair()}
              >
                {typeof draft.privateKey === 'string' && draft.privateKey.trim() !== ''
                  ? t('node.tcDerivePublic')
                  : t('node.tcGenerate')}
              </button>
              <p className="m-form-hint">{t('node.tcPublicKeyHint')}</p>
            </div>
          )}
          {g.id === 'advanced' && (
            <>
              <div className="m-form-row">
                <label className="m-form-label" htmlFor="mnf-detour">
                  {t('node.chainVia')}
                </label>
                <MobileSelect
                  id="mnf-detour"
                  className="m-form-select"
                  value={detour === '' ? 'direct' : detour}
                  onChange={(e) => {
                    setDetour(e.target.value === 'direct' ? '' : e.target.value);
                    setDirty(true);
                  }}
                >
                  {detourOptions.map((o) => (
                    <option key={o.value} value={o.value}>
                      {o.label}
                    </option>
                  ))}
                </MobileSelect>
                {/* Tailcat 的前置代理只承载 DERP 中继连接，点对点直连不经过它。 */}
                <div className="m-form-hint">
                  {proto === 'tailcat' ? <MobileInfo title={t('node.chainVia')} summary={t('mobileHelp.tcChain')} details={t('node.tcChainHint')} /> : t('node.chainHint')}
                </div>
              </div>

              <div className="m-form-row">
                <label className="m-form-label" htmlFor="mnf-bind">
                  {t('node.bindInterface')}
                </label>
                <MobileSelect
                  id="mnf-bind"
                  className="m-form-select"
                  value={effectiveInterfaceValue}
                  disabled={base?.subscriptionId !== undefined}
                  onChange={(e) => {
                    setBindInterface(e.target.value);
                    setDirty(true);
                  }}
                >
                  {interfaceOptions.map((o) => (
                    <option key={o.value} value={o.value} disabled={o.disabled === true}>
                      {o.label}
                    </option>
                  ))}
                </MobileSelect>
                <p className="m-form-hint">
                  {base?.subscriptionId !== undefined
                    ? t('node.bindInterfaceSubscriptionHint')
                    : t('node.bindInterfaceHint')}
                </p>
                {interfaces.failed && <p className="m-form-err">{t('settings.network.interfaceListFailed')}</p>}
              </div>
            </>
          )}
        </FormGroup>
      ))}

      {isMeshTunnelNodeProtocol(proto) && <MeshInboundPolicyFields idPrefix="mnf" protocol={proto}
        value={meshPolicy} errorKey={meshPolicyError} errorVersion={meshPolicyErrorVersion}
        onChange={(next) => { setMeshPolicy(next); setMeshPolicyError(null); setDirty(true); }} />}

      {/* custom 协议内核兼容性探测（同桌面 C10）：只有 custom 的原始 JSON 才有「内核认不认识」这一档风险。 */}
      {proto === 'custom' && (
        <div className="m-form-row">
          <button
            type="button"
            className="m-form-btn"
            disabled={probing || !(typeof draft.outbound === 'string' && draft.outbound.trim() !== '')}
            onClick={() => void runCompatProbe()}
          >
            {probing ? t('node.customProbe.testing') : t('node.customProbe.test')}
          </button>
          {probeResult?.kind === 'invalidJson' && (
            <p className="m-form-err">{t('node.customProbe.invalidJson')}</p>
          )}
          {probeResult?.kind === 'supported' && (
            <p className="m-form-hint">{t('node.customProbe.supported')}</p>
          )}
          {probeResult?.kind === 'indeterminate' && (
            <p className="m-form-hint">{t('node.customProbe.indeterminate')}</p>
          )}
          {probeResult?.kind === 'unsupported' && (
            <p className="m-form-err">
              {probeResult.keyPath
                ? t('node.customProbe.unsupportedWithPath', { path: probeResult.keyPath })
                : t('node.customProbe.unsupported')}
            </p>
          )}
        </div>
      )}

    </FormSheet>
  );
}
