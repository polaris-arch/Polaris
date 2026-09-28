/**
 * TsSettingsDialog —— Tailscale 设置弹窗（原型 #ts-settings-dialog :2802）。
 *
 * 本层**最长表单**（主机名 / 出口节点选择 / 接入模式 / 子网路由两向 / 高级设置）按
 * 基础 / 路由 / 高级三个稳定任务页展示；footer 固定
 * 验收样本：body 超高时 `.dlg-body` 独立滚动、`.dlg-foot` 常驻（Modal 原语的三段结构自动保证）。
 * 多字段驱动走 D2 FieldSpec 表 + FieldRenderer（switch → .swt-row，select → Csel）。
 *
 * ⚠️ **FieldSpec 表是覆盖门的判据面**：`contracts/protocol-settings-coverage.test.ts` 解析 Rust
 * `TailscaleSettings` 的 serde 键集，要求每个键在这张表里有对应控件，否则转红（豁免须写进
 * 该门的 `EXEMPT` 并附理由）。所以给 Rust 结构体加字段而不在这里加一项 = 门红，不是静默漏掉。
 *
 * **后端现状**：
 *  - `api.server.tailscaleGetStatus` = REAL（`server.rs`：读 `MeshRuntime` 的 STATUS 末帧缓存，`connected`
 *    = 主核是否在跑）→ 核未跑 / 无在册 TS 节点时候选为空 → 优雅降级：下拉仅「无 / 自定义…」，
 *    给「未连接或无可用出口，可手动填写」提示，不空白卡死。
 *  - `api.server.tailscaleLogout` = REAL（清 state 目录）。
 *  - 保存经 `api.server.update`（把表单写回该 TS 节点的 tailscaleSettings）。
 *
 * **Auth Key 状态行**（基础页底部）：只呈现「已保存 / 未保存 / 待清除」这一个布尔事实 + 一颗
 * 原地二次点击的「清除」。清除写的仍是上面这条唯一写腿（`buildTsSettings` 的 `clearAuthKey`
 * 入参 → 同一次 `api.server.update` / 同一条暂存条目），**不新开第二条写 config 的路**；
 * 登记见 `lib/config-write-wiring.test.ts` 里本文件那行 `api.server.update(`。
 * key 本体在这一层**结构上不可达**：判据是 `hasTsAuthKey(node)` 这个布尔，不是字符串。
 *
 * 无 TS 节点时（未登录）：给 mesh-note 引导先登录，Save/Logout 置灰（无可写目标）。
 * R1：`key` 绑 TS 节点 id（见导出包装）+ useState 同步初始化。
 */

import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { toast } from '@/lib/error-handler';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { api } from '@/ipc';
import type { MeshInboundPolicy, ServerConfig } from '@/contracts/types';
import type { TailscaleSettings } from '@/contracts/types';
import type { TailscaleStatusPeer } from '@/contracts/tailscale-status';
import { Modal } from './Modal';
import {
  FormTabs,
  type FormValue,
  type FormValues,
  type SelectOption,
} from './FieldSpec';
import {
  buildTsSettings,
  exitNodeOptions,
  initTsDraft,
  invalidTsCidrs,
  invalidControlUrl,
  peersForTsNode,
} from './ts-settings-logic';
import { applyDetour, endpointDetourOptions } from './detour-options';
import { TS_ADV_SPEC, tsMainSpec } from './ts-spec';
import { applyOnDemand, onDemandDraftValue } from './on-demand-field';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { splitStagedOnly, stagedOnlyIds } from '@/lib/staged-config';
import { editRoute } from '@/lib/staged-config';
import { useDialogStore } from './dialog-store';
import { INVALID_NODE_REASON_KEY } from '@/domain/invalid-node-reason';
import { groupTsFields } from './mesh-form-layout';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { hasTsAuthKey } from '@/domain/tailscale-conn-state';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { InfoIcon } from '@/components/InfoIcon';
import { cn } from '@/lib/utils';
import { MeshInboundPolicyEditor, applyMeshInboundPolicy, meshInboundPolicyError, normalizeMeshInboundPolicy } from './MeshInboundPolicyEditor';

/** 「清除 Auth Key」的二次点击槽位（单例节点，全弹窗只有这一颗，无需按 id 分槽）。 */
const AUTH_KEY_CLEAR_KEY = 'ts-authkey-clear';

function TsSetIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" />
    </svg>
  );
}

/* 字段表住在 `./ts-spec`（零 React 纯数据，两个客户端共用；拆分理由见那份文件头注）。
   桌面「多 VPN 兼容」批新加的 `ON_DEMAND_FIELD` 已随 `TS_ADV_SPEC` 搬进那份文件。 */

function TsSettingsForm({ node }: { node?: ServerConfig }) {
  const { t } = useTranslation();
  const open = useDialogStore((s) => s.open);
  const close = useDialogStore((s) => s.close);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  /** staged-only 差集的两个入参（展示面 effective + 操作面磁盘镜像），判据与节点卡同一函数。 */
  const effectiveServers = useEffectiveServers();
  const diskServers = useAppStore((s) => s.servers);
  const stagedOnly = useMemo(
    () => stagedOnlyIds(effectiveServers, diskServers),
    [effectiveServers, diskServers]
  );
  const interfaces = useNetworkInterfaces();

  const [peers, setPeers] = useState<TailscaleStatusPeer[]>([]);
  const [connected, setConnected] = useState<boolean | null>(null);
  const [draft, setDraft] = useState<FormValues>(() => ({
    ...initTsDraft(node),
    bindInterface: node?.bindInterface ?? '',
    onDemand: onDemandDraftValue(node),
  }));
  const [meshPolicy, setMeshPolicy] = useState<MeshInboundPolicy | undefined>(node?.meshInboundPolicy);
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [formTab, setFormTab] = useState('basic');
  /**
   * 「本次提交要不要删掉已存的 authKey」——**草稿意图**，与表单其余字段同一提交时机。
   *
   * 不做成「点一下就立刻写盘」的第二条写腿，理由有二：
   *  ① 本弹窗**每一项**都是保存才生效；一个动作独走会让同一个界面有两套提交语义；
   *  ② 暂存层开着时（`STAGED_CONFIG_ENABLED` 默认 true）`servers` 的编辑恒走暂存，
   *     所谓「立刻」本来就落不了盘 —— 写成立刻只会让文案撒谎。
   * 故这里只记意图，真正删键由 `buildTsSettings(..., authKeyCleared)` 在提交时完成。
   */
  const [authKeyCleared, setAuthKeyCleared] = useState(false);
  const { armed, confirmTwice } = useConfirmTwice();

  // 出口候选：拉状态快照（核未跑 / 无节点时为空）。connected=false 时静态提示手动填写。
  // **原样收下当前节点的全部 peer**，不在这里筛 `exitNodeOption` —— 「没广告出口」与「不在 tailnet 里」
  // 此前在界面上是同一种表现（都不在列表里），用户无从知道该去哪台机器上开那个开关。
  // 筛/排/去重/禁用/注记一律下沉 `exitNodeOptions`（纯函数，有单测）。
  useEffect(() => {
    let cancelled = false;
    setPeers([]);
    setConnected(null);
    api.server
      .tailscaleGetStatus()
      .then((snap) => {
        if (cancelled) return;
        setConnected(snap.connected);
        setPeers(peersForTsNode(snap, node?.id));
      })
      .catch(() => {
        if (cancelled) return;
        setPeers([]); // 非 Tauri / 失败 → 空候选，走手动填写降级。
      });
    return () => {
      cancelled = true;
    };
  }, [node?.id]);

  // 判据取**已保存值**而非草稿值（禁用豁免须在整个弹窗生命期内稳定，见 exitNodeOptions 头注）。
  const savedExit = node?.tailscaleSettings?.exitNode ?? '';
  const exitOpts = exitNodeOptions(peers, savedExit, {
    none: t('ts.exitNone'),
    custom: t('common.customEllipsis'),
    inUse: t('ts.exitInUse'),
    offline: t('ts.exitOffline'),
    notAdvertised: t('ts.exitNotAdvertised'),
  });
  // 前置代理候选：排除自身与 endpoint 类节点（判据对齐生成侧，见 `detour-options.ts`）。
  const detourOpts = endpointDetourOptions(
    effectiveServers,
    node?.id,
    t('node.detourDirect')
  );
  const interfaceOpts: SelectOption[] = buildNetworkInterfaceChoices(
    interfaces.items,
    typeof draft.bindInterface === 'string' ? draft.bindInterface : '',
    {
      defaultLabel: t('node.bindInterfaceInherit'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    },
  ).map(({ value, label, disabled }) => [value, label, disabled]);
  const spec = tsMainSpec(exitOpts, detourOpts, interfaceOpts);
  const setField = (k: string, v: FormValue) => {
    setDraft((d) => ({ ...d, [k]: v }));
    setDirty(true);
  };
  const groups = groupTsFields([...spec, ...TS_ADV_SPEC]);

  const requestClose = () => {
    if (!dirty) {
      close();
      return;
    }
    open({
      kind: 'confirm',
      payload: {
        title: t('ts.discardTitle'),
        message: t('ts.discardMsg'),
        confirmLabel: t('ts.discard'),
        danger: true,
        onConfirm: () => {
          close();
          close();
        },
      },
    });
  };

  const buildSettings = (): TailscaleSettings =>
    buildTsSettings(node?.tailscaleSettings, draft, authKeyCleared);

  /**
   * 已存 authKey 的**存在性**（布尔，绝不是 key 本身）。判据取 `domain/tailscale-conn-state.ts`
   * 那一份 —— 节点卡的 `key-ready` 档说的就是同一件事，两处不许各判各的。
   */
  const authKeyStored = hasTsAuthKey(node);

  /**
   * 清除 Auth Key —— 原地二次点击（与删规则/删节点同一交互类，理由见 `ConfirmDialog` 头注：
   * 破坏性操作不再叠弹窗）。它与「退出登录」是**两件事**，刻意不合并：
   *  · 退出登录（`handleLogout`）清的是磁盘上的 tsnet state 目录 = 当前这次登录的身份；
   *  · 清除 Auth Key 清的是 config 里的静态凭据 = 下次认证拿什么去认。
   * 于是清完 key 后本节点**仍然连着**（state 目录里的 node key 照样有效），变的是「state 一旦
   * 失效/被清，就没有能自动重认的凭据了，得重新登录」。这两句正是 `ts.authKeyClearHint` 的内容。
   */
  const requestClearAuthKey = () => {
    confirmTwice(AUTH_KEY_CLEAR_KEY, () => {
      setAuthKeyCleared(true);
      setDirty(true); // 与改任何一个字段同权：直接关窗要走「放弃更改？」那条闸门
    });
  };

  const handleSave = async () => {
    if (!node) return;
    const policy = normalizeMeshInboundPolicy(meshPolicy);
    const policyError = meshInboundPolicyError(policy, 'tailscale');
    if (policyError) {
      setFormTab('routing');
      toast.error(t(policyError));
      return;
    }
    // 非法 CIDR 必须前端拦：后端 sanitize 对非法项是**静默丢弃**，不拦就成了「界面收下了、盘上没有」。
    const badCidr = invalidTsCidrs(draft);
    if (badCidr.length) {
      setFormTab('routing');
      toast.error(t('ts.errCidr', { list: badCidr.join(', ') }));
      return;
    }
    // control_url 必须前端拦，且必须拦在**保存**这一刻：IP 形式会让 sing-box 在初始化 tailscale
    // endpoint 时直接 panic（见 `domain/control-url.ts` 头注）。后端虽有 fail-closed 兜底（生成配置时
    // 剔掉该节点），但那是**事后**告知——用户那时已经离开本弹窗，看到的只是节点卡置灰。
    // 拦在这里，光标还在这个输入框旁边，改一下就好了。
    const badControl = invalidControlUrl(draft);
    if (badControl) {
      setFormTab('advanced');
      toast.error(t('ts.errControlUrl'), t(INVALID_NODE_REASON_KEY[badControl]));
      return;
    }
    setBusy(true);
    try {
      // detour 在顶层，`buildTsSettings` 够不着 —— 单独写回（哨兵 ⇒ 删键）。
      const next = applyDetour({ ...node, tailscaleSettings: buildSettings() }, draft.detour);
      applyMeshInboundPolicy(next, policy);
      applyOnDemand(next, draft.onDemand);
      const bindInterface = String(draft.bindInterface ?? '').trim();
      if (bindInterface) next.bindInterface = bindInterface;
      else delete next.bindInterface;
      // 配置暂存闸门（与 NodeDialog 同形）。本弹窗改的是**该节点的 tailscaleSettings**，落在
      // `servers` 键上、无远端副作用 ⇒ 走默认腿。（弹窗里从活态回读的只有出口候选列表，不是被写的字段，
      // 故 W-2 不成立；真正有远端效应的是下面的 `handleLogout`，它另走绕过腿。）
      if (editRoute('servers', stagingEnabled) === 'staged') {
        stage({
          id: `server:${node.id}`,
          kind: 'server',
          label: `${t('ts.settingsTitle')} ${node.name}`,
          entityPath: ['servers', node.id],
          nextValue: next,
        });
        close();
        return; // 零 IPC 写、零磁盘写（FR-1）
      }
      await api.server.update(next);
      void loadConfig(true);
      close();
    } catch (e) {
      console.error('[TsSettingsDialog] save failed:', e);
      toast.error(t('common.saveFailed'));
    } finally {
      setBusy(false);
    }
  };

  const handleLogout = async () => {
    if (!node) return;
    // `block`（ENTITY_ACTION_TABLE）：登出清的是磁盘上的 TS state 目录，盘上没有这个节点就没有对象。
    const split = splitStagedOnly(
      'server.tailscaleLogout',
      [node.id],
      stagedOnly,
      stagedEntries,
      'servers'
    );
    if (split.blocked.length > 0) {
      // 与 NodesScreen:687 的同一条闸门同一形态：这是「还不能做」的提示而非错误，走 info 单参
      // （文案自述完整，不套 title）。
      toast.info(
        t('home.stagedOnlyBlocked')
      );
      return;
    }
    setBusy(true);
    try {
      await api.server.tailscaleLogout(node.id);
      void loadConfig(true);
      close();
    } catch (e) {
      // 登出不是保存 —— 标题取 NodesScreen:696 同一操作已在用的那个键，别套 `common.saveFailed`。
      console.error('[TsSettingsDialog] logout failed:', e);
      const busy = e && typeof e === 'object' && 'code' in e && e.code === 'TAILSCALE_LOGOUT_MAIN_CORE';
      toast.error(t(busy ? 'ts.reasonMainCoreInUse' : 'nodes.meshTsLogoutFail'));
    } finally {
      setBusy(false);
    }
  };

  /**
   * Auth Key 状态行 —— 归**基础**页而非高级：本行存在的理由就是「用户此前无从知道盘上还躺着一把
   * 长期凭据」（`TsLoginDialog` 的输入框从不回填、退出登录也明说保留 authKey）。把唯一的可见面
   * 再折进高级页，等于只解决了一半。同族的「退出登录」同样常驻（footer），二者视觉分量相称。
   *
   * ⚠️ 这里渲染的每一样东西都是**布尔派生**：三档状态文案 + 一颗按钮。key 的明文、前缀、后几位、
   * 长度一概不进 DOM —— 截图/录屏/演示都会把 DOM 里的东西带出去，而 pre-auth key 是长期凭据。
   */
  const authKeyRow = node ? (
    <div className="fld swt-row">
      <div className="swt-tx">
        <span className="swt-label">
          <b>{t('ts.authKey')}</b>
          <InfoIcon tip={t('ts.authKeyClearHint')} />
        </span>
        <span className="card-sub">
          {authKeyCleared
            ? t('ts.authKeyClearPending')
            : authKeyStored
              ? t('ts.authKeySaved')
              : t('ts.authKeyNone')}
        </span>
      </div>
      {authKeyStored && !authKeyCleared && (
        <button
          type="button"
          className={cn('btn ghost', armed === AUTH_KEY_CLEAR_KEY && 'confirming')}
          style={{ color: 'hsl(var(--err))', borderColor: 'hsl(var(--err)/0.3)' }}
          onClick={requestClearAuthKey}
        >
          {armed === AUTH_KEY_CLEAR_KEY ? t('ts.authKeyClearAgain') : t('ts.authKeyClear')}
        </button>
      )}
    </div>
  ) : null;

  return (
    <Modal
      titleId="ts-set-title"
      title={t('ts.settingsTitle')}
      onClose={requestClose}
      icon={<TsSetIcon />}
      className="entry-form-dlg"
      footer={
        <>
          <button
            type="button"
            className="btn ghost"
            onClick={() => void handleLogout()}
            disabled={busy || !node}
            style={{ marginRight: 'auto', color: 'hsl(var(--err))', borderColor: 'hsl(var(--err)/0.3)' }}
          >
            {t('ts.logout')}
          </button>
          {/* 提交中**不锁**「取消」：原型 `:2545` 的 ghost 钮无 disabled，且本仓此前四个弹窗锁、
              两个不锁（NodeDialog/SubDialog）—— 不是与原型的差，是实现自己两套。统一为不锁：
              提交卡住（IPC 无应答）时用户必须还能退出，否则弹窗成了死窗。 */}
          <button type="button" className="btn ghost" onClick={requestClose}>
            {t('common.cancel')}
          </button>
          <button type="button" className="btn flow" onClick={() => void handleSave()} disabled={busy || !node}>
            {busy && <span className="spinner spin-inline" style={{ marginRight: 6 }} />}
            {t('common.save')}
          </button>
        </>
      }
    >
      {!node && (
        <div className="mesh-note">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
            <circle cx="12" cy="12" r="9" />
            <path d="M12 8v5M12 16h.01" />
          </svg>
          <span>{t('ts.noNode')}</span>
        </div>
      )}

      <FormTabs
        id="ts-settings-form"
        ariaLabel={t('node.formGroup.aria')}
        tabs={[
          {
            id: 'basic',
            label: t('node.formGroup.basic'),
            fields: groups.basic,
            children: (
              <>
                {connected === false && (
                  <div className="card-sub form-inline-note">{t('ts.exitEmptyHint')}</div>
                )}
                {authKeyRow}
              </>
            ),
          },
          { id: 'routing', label: t('node.formGroup.routing'), fields: groups.routing,
            children: node && <>
              <MeshInboundPolicyEditor idPrefix="ts" value={meshPolicy} onChange={(next) => { setMeshPolicy(next); setDirty(true); }} />
              <div className="card-sub form-inline-note">{t('meshInbound.tsBoundary')}</div>
            </> },
          { id: 'advanced', label: t('node.formGroup.advanced'), fields: groups.advanced },
        ]}
        active={formTab}
        onSelect={setFormTab}
        values={draft}
        onChange={setField}
      />
    </Modal>
  );
}

export function TsSettingsDialog({ serverId }: { serverId: string }) {
  // 展示面：本弹窗的提交腿走暂存（:202），编辑基准必须同源，否则第二次编辑从盘上的旧值起算。
  // 按 id 精确取——Tailscale 不再是单例（多节点时 `.find(protocol==='tailscale')` 会取到任意一个，
  // 不一定是调用方（`node-edit-routing.ts` / `MeshJoinDialog`）想编辑的那个）。取不到（节点已被删 /
  // id 不匹配）**不回落到任意节点**：走 `TsSettingsForm` 已有的 `!node` 空态（mesh-note 引导 + Save/Logout 置灰）。
  const servers = useEffectiveServers();
  // 按 `serverId` 寻址，**不按协议 `.find()`**：Tailscale 已不是单例
  // （`meshSingletonConflict` 只剩 WARP 支），按协议取「任意一个」会让编辑第二个节点
  // 打开/写坏第一个（同 `node-edit-routing.ts` 记的那条缺陷）。
  const node = servers.find((s) => s.id === serverId);
  return <TsSettingsForm key={node?.id ?? 'none'} node={node} />;
}

export default TsSettingsDialog;
