import { useEffect, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import type { ServerConfig } from '@/contracts/types';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { taildropBadgeCount } from '@/domain/taildrop';
import { tsAccountLabel } from '@/domain/tailscale-conn-state';
import { findWarpNode } from '@/domain/warp';
import { isAccountBasedProtocol } from '@/domain/endpoint-routes';
import { Modal } from './Modal';
import { useDialogStore } from './dialog-store';
import { InfoIcon } from '@/components/InfoIcon';

interface MeshJoinDialogProps {
  onTsLogout: (node: ServerConfig) => void;
  onWarpReregister: (node: ServerConfig) => void;
  onWarpDeregister: (node: ServerConfig) => void;
}

function JoinIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M9 15l6-6M8 8a3 3 0 10-3 3M16 16a3 3 0 103 3" />
    </svg>
  );
}

function Choice({
  title,
  description,
  icon,
  onClick,
  actions,
}: {
  title: string;
  description: string;
  icon: ReactNode;
  onClick: () => void;
  actions?: ReactNode;
}) {
  return (
    <div className="mesh-choice">
      <button type="button" className="mesh-col clickable" onClick={onClick}>
        <span className="mesh-ic">{icon}</span>
        <span className="mesh-tx">
          <span className="mesh-col-h"><b>{title}</b></span>
          <span className="mesh-col-sub">{description}</span>
        </span>
        <svg className="mesh-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
          <path d="M9 6l6 6-6 6" />
        </svg>
      </button>
      {actions && <div className="mesh-choice-actions">{actions}</div>}
    </div>
  );
}

export function MeshJoinDialog(props: MeshJoinDialogProps) {
  const servers = useEffectiveServers();
  const tsNodeIds = servers.filter((server) => isAccountBasedProtocol(server.protocol)).map((node) => node.id);
  const tsNodeIdsKey = JSON.stringify(tsNodeIds);
  const [tsStates, setTsStates] = useState<Record<string, boolean>>({});
  useEffect(() => {
    if (tsNodeIds.length === 0) return;
    let alive = true;
    let revision = 0;
    const readStates = (): void => {
      const request = ++revision;
      setTsStates({});
      void api.server.tailscaleStateExists(tsNodeIds, true).then(
        (states) => { if (alive && request === revision) setTsStates(states); },
        () => { if (alive && request === revision) setTsStates({}); },
      );
    };
    readStates();
    window.addEventListener('focus', readStates);
    return () => { alive = false; window.removeEventListener('focus', readStates); };
  }, [tsNodeIdsKey]);
  return <MeshJoinDialogView {...props} servers={servers} tsStates={tsStates} />;
}

/** Pure action view: the native state read above decides which account actions are truthful. */
export function MeshJoinDialogView({ onTsLogout, onWarpReregister, onWarpDeregister, servers, tsStates }:
  MeshJoinDialogProps & { servers: ServerConfig[]; tsStates: Record<string, boolean> }) {
  const { t } = useTranslation();
  const open = useDialogStore((state) => state.open);
  const close = useDialogStore((state) => state.close);
  /* Tailscale 不再是单例（`meshSingletonConflict` 已只剩 WARP 一支）⇒ 这里不能再 `.find()` 取
     「任意一个」：那样第二个及以后的账号在这张卡上不存在。 */
  const tsNodes = servers.filter((server) => isAccountBasedProtocol(server.protocol));
  const singleTsNode = tsNodes.length === 1 ? tsNodes[0] : undefined;
  const warpNode = findWarpNode(servers);
  // 入口只跟「配置里有 TS 节点」绑定；离线 / tailnet 未授权时也必须能打开，弹窗会给出可行动的原因。
  // 若只在 ready 时画按钮，`taildropAvailability` 的两条解释分支永远不可达，用户只会看到入口凭空消失。
  //
  // 整表订阅（而非像 NodeCard 那样只订本节点那一格）：这张卡同时要为 N 个 TS 节点取角标与账号标识，
  // 逐节点订阅在组件层做不到（hook 数量随节点数变）。代价是任一 TS 节点的 STATUS 帧会重渲本弹窗 ——
  // 它是模态弹窗、同屏至多一个，与节点网格的 N 张卡不是一个量级。
  const tailscaleStatuses = useAppStore((state) => state.tailscaleStatuses);

  const go = (next: Parameters<typeof open>[0]) => {
    close();
    open(next);
  };
  const action = (run: () => void) => {
    close();
    run();
  };
  const shield = (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
      <path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z" />
    </svg>
  );

  /**
   * 一个 TS 节点的账号动作。**单节点与多节点共用这一份**；有本机 state 时展示登出，
   * 没有时提供登录，读取失败时仍能手动切换账号。
   *
   * 每颗都携 `node.id`，不读任何外层的「当前 TS 节点」—— 多节点时那个概念不存在。
   */
  const tsActions = (node: ServerConfig) => {
    const unread = taildropBadgeCount(tailscaleStatuses[node.id]);
    const accountActionLabel = tsStates[node.id] === true ? t('meshJoin.switchAccount') : t('ts.signIn');
    return (
      <>
        <button
          type="button"
          className="btn ghost sm"
          onClick={() => go({ kind: 'taildrop', serverId: node.id })}
        >
          {t('meshJoin.taildrop')}
          {unread > 0 && <span className="tdrop-badge">{unread}</span>}
        </button>
        <button
          type="button"
          className="btn ghost sm"
          onClick={() => go({ kind: 'ts-login', serverId: node.id })}
        >
          {accountActionLabel}
        </button>
        {tsStates[node.id] === true && <button type="button" className="btn ghost sm danger-text" onClick={() => action(() => onTsLogout(node))}>
          {t('meshJoin.logout')}
        </button>}
      </>
    );
  };

  return (
    <Modal
      titleId="mesh-join-title"
      title={t('meshJoin.title')}
      icon={<JoinIcon />}
      onClose={close}
      className="access-picker-dlg"
      footer={
        <button type="button" className="btn ghost" onClick={close}>
          {t('common.cancel')}
        </button>
      }
    >
      <div className="field-lbl"><span>{t('meshJoin.managed')}</span></div>
      <div className="mesh-grid mesh-choice-grid">
        <Choice
          title="Cloudflare WARP"
          description={warpNode
            ? t('meshJoin.warpConfigured')
            : t('meshJoin.warpNew')}
          icon={<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}><path d="M13 2L4 14h6l-1 8 9-12h-6z" /></svg>}
          onClick={() => go({ kind: 'warp', edit: !!warpNode })}
          actions={warpNode && (
            <>
              <button type="button" className="btn ghost sm" onClick={() => action(() => onWarpReregister(warpNode))}>
                {t('meshJoin.reregister')}
              </button>
              <button type="button" className="btn ghost sm danger-text" onClick={() => action(() => onWarpDeregister(warpNode))}>
                {t('meshJoin.deregister')}
              </button>
            </>
          )}
        />
        {tsNodes.length > 1 ? (
          // 多节点：一节点一行。标题带节点名（用户自己起的名字，最直接的区分维度），
          // 副标题优先用账号标识 —— 两个节点都叫 "Tailscale"、都显示「已配置」时，
          // 只有 `登录名 · tailnet` 这一段答得出「这一行是哪个账号」。取不到就回落到原来那句。
          tsNodes.map((node) => (
            <Choice
              key={node.id}
              title={`Tailscale · ${node.name}`}
              description={
                tsAccountLabel(tailscaleStatuses[node.id]?.details) ?? t('meshJoin.tsConfigured')
              }
              icon={<JoinIcon />}
              onClick={() => go({ kind: 'ts-settings', serverId: node.id })}
              actions={tsActions(node)}
            />
          ))
        ) : singleTsNode ? (
          <Choice
            title="Tailscale"
            description={t('meshJoin.tsConfigured')}
            icon={<JoinIcon />}
            onClick={() => go({ kind: 'ts-settings', serverId: singleTsNode.id })}
            actions={tsActions(singleTsNode)}
          />
        ) : null}
        <Choice
          title={tsNodes.length === 0 ? 'Tailscale' : t('meshJoin.tsAdd')}
          description={t('meshJoin.tsNew')}
          icon={<JoinIcon />}
          onClick={() => go({ kind: 'ts-login' })}
        />
      </div>

      <div className="field-lbl field-lbl-info">
        <span>{t('meshJoin.tunnels')}</span>
        <InfoIcon tip={t('meshJoin.routesHint')} />
      </div>
      <div className="mesh-grid mesh-choice-grid">
        <Choice title="OpenConnect" description={t('meshJoin.oc')} icon={shield} onClick={() => go({ kind: 'node', initialProto: 'openconnect' })} />
        <Choice title="OpenVPN" description={t('meshJoin.ovpn')} icon={shield} onClick={() => go({ kind: 'node', initialProto: 'openvpn-client' })} />
        <Choice title="WireGuard" description={t('meshJoin.wg')} icon={shield} onClick={() => go({ kind: 'wg' })} />
        <Choice title="MASQUE" description={t('meshJoin.masque')} icon={shield} onClick={() => go({ kind: 'node', initialProto: 'masque-client' })} />
      </div>
    </Modal>
  );
}

export default MeshJoinDialog;
