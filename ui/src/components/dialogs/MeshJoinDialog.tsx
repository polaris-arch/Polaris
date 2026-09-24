import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { ServerConfig } from '@/contracts/types';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { taildropBadgeCount } from '@/domain/taildrop';
import { tsAccountLabel } from '@/domain/tailscale-conn-state';
import { findWarpNode } from '@/domain/warp';
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

export function MeshJoinDialog({ onTsLogout, onWarpReregister, onWarpDeregister }: MeshJoinDialogProps) {
  const { t } = useTranslation();
  const servers = useEffectiveServers();
  const open = useDialogStore((state) => state.open);
  const close = useDialogStore((state) => state.close);
  /* Tailscale 不再是单例（`meshSingletonConflict` 已只剩 WARP 一支）⇒ 这里不能再 `.find()` 取
     「任意一个」：那样第二个及以后的节点在这张卡上根本不存在，而 **Taildrop 收件箱的全仓唯一入口
     就在这张卡上**（`kind:'taildrop'` 全仓只此一处 go），把它绑在第一个节点上 = 多节点用户永远
     进不去别的账号的收件箱。

     形态按节点数分两支，刻意不新开弹窗、不造选择器：
      - 0 / 1 个 ⇒ 与此前**逐像素相同**的单块 tile（零回归，由 MeshJoinDialog.ts-nodes.test.tsx 钉住）；
      - ≥2 个   ⇒ 每个节点一行，各自带自己的 taildrop / 切换账号 / 登出，标题用节点名、副标题用
                  `tsAccountLabel`（登录名 · tailnet）区分是哪个账号 —— 同为「已登录」时，
                  节点名可能都叫 Tailscale，账号段才是能区分的那一维。 */
  const tsNodes = servers.filter((server) => server.protocol === 'tailscale');
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
   * 一个 TS 节点的三颗动作。**单节点与多节点共用这一份**：两支各写一遍，改一处漏一处只是时间问题，
   * 而「逐像素相同」那条零回归判据正是靠共用同一份 JSX 成立的。
   *
   * 三颗都携 `node.id`，不读任何外层的「当前 TS 节点」—— 多节点时那个概念不存在。
   */
  const tsActions = (node: ServerConfig) => {
    const unread = taildropBadgeCount(tailscaleStatuses[node.id]);
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
          {t('meshJoin.switchAccount')}
        </button>
        <button type="button" className="btn ghost sm danger-text" onClick={() => action(() => onTsLogout(node))}>
          {t('meshJoin.logout')}
        </button>
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
        ) : (
          <Choice
            title="Tailscale"
            description={singleTsNode
              ? t('meshJoin.tsConfigured')
              : t('meshJoin.tsNew')}
            icon={<JoinIcon />}
            onClick={() => go(singleTsNode ? { kind: 'ts-settings', serverId: singleTsNode.id } : { kind: 'ts-login' })}
            actions={singleTsNode && tsActions(singleTsNode)}
          />
        )}
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
