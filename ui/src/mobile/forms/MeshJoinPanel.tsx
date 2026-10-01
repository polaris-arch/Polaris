import { MobileInfo } from '../MobileInfo';
/**
 * 移动端**组网接入选择器**（「添加」四项里的第二项）。
 *
 * 桌面 `MeshJoinDialog` 摆五个选择：托管服务两个（Cloudflare WARP / Tailscale）+ 隧道三个
 * （OpenConnect / OpenVPN / WireGuard）。**2026-09-06（批 3）起五个全部接通** ——
 * 三张各自独立的表（WARP 注册 / Tailscale 登录 / WireGuard 字段表）本批落地，它们的规格随之
 * 从桌面 `.tsx` 里拆进零 React 的 `.ts`（`warp-spec` / `ts-spec` / `wg-spec`），两端共用同一份。
 *
 * 处置逐条登记在 `nodes/absence-register.ts#MESH_JOIN_CHOICES`，由 `nodes-screen.test.tsx`
 * 拿桌面那六个 `<Choice` 与本表条目数对差：桌面加第七个、或这里少画一个，都会红。
 *
 * # 两个托管服务是**有状态**的入口（与三条隧道的差别）
 *
 * WARP 仍有单例槽，已有节点时打开编辑。Tailscale 可有多个 userspace 节点：这张「添加」
 * 选择卡始终创建新节点；既有节点的设置和收件箱从各自节点行进入，不能在这里任选第一个。
 *
 * # 桌面每张卡片上那五颗**次动作**不在这里，但**五颗都在**
 *
 * 桌面在 WARP 卡片上挂「重新注册 / 注销」，在 Tailscale 卡片上挂「Taildrop / 切换账号 / 退出登录」。
 * 移动端这张接入面是一列纵向选择（拇指区），塞不下每条 2–3 颗次动作 ⇒ 五颗各自收进
 * **它作用对象所在的那张表**，逐颗的处置登记在 `absence-register.ts#MESH_JOIN_ACTIONS`：
 *  · Tailscale 三颗（Taildrop / 切换账号 / 退出登录）→ `TsSettingsPanel` 末尾那一行；
 *  · WARP 两颗（重新注册 / 注销）→ `WarpPanel` 编辑态末尾那一行。
 * 2026-09-13（批 16）起五颗全部 `ported`，本文件不再有「还没有」的那一档。
 */

import { type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { findWarpNode } from '@/domain/warp';
import type { ServerConfig } from '@/contracts/types';
import { useEffectiveServers } from '@/store/app-store';
import { MESH_JOIN_CHOICES } from '../nodes/absence-register';
import { FormSheet } from './FormSheet';
import { useMobileFormStore, type MobileFormDesc } from './form-store';

/**
 * 一个选择点下去开哪张表 —— **五支全在这里，没有一支落空**。
 * WARP 按槽位分流；Tailscale 每次新建；三条隧道落节点表单的对应协议。
 *
 * 提到模块顶层（不留在组件闭包里）是为了让它能被判据**直接驱动**：本批销掉的四条债全部落在
 * 「点一下开哪张表」这一跳上，而闭包里的函数只能靠 grep 源码取证（复审实测：把 `open(target)`
 * 换成 `open({ kind: 'node' })` —— 三张表全废而全仓门一条不红）。现在门可以拿真 `ServerConfig` 夹具
 * 逐个 id 调它、断言映射结果恰等（`mesh-forms.test.tsx` ⑤-a），再单独钉住
 * 「`onClick` 把它的返回值交给了 `open`」那一跳（⑤-b）。
 */
export function meshJoinFormFor(
  id: string,
  servers: readonly ServerConfig[],
): MobileFormDesc | null {
  const warpNode = findWarpNode([...servers]);
  switch (id) {
    case 'warp':
      return { kind: 'warp', edit: warpNode !== undefined };
    case 'tailscale':
      return { kind: 'ts-login' };
    case 'wireguard':
      return { kind: 'wg' };
    case 'openconnect':
      return { kind: 'node', initialProto: 'openconnect' };
    case 'openvpn':
      return { kind: 'node', initialProto: 'openvpn-client' };
    case 'masque':
      return { kind: 'node', initialProto: 'masque-client' };
    default:
      return null;
  }
}

export function MeshJoinPanel({ instanceId }: { instanceId: string }): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const servers = useEffectiveServers();

  const warpNode = findWarpNode(servers);

  /** 已接入的那两个换一句说明（不换的话，用户会以为点进去是再建一个）。 */
  const descKeyFor = (id: string, fallback: string): string => {
    if (id === 'warp' && warpNode !== undefined) return 'meshJoin.warpConfigured';
    return fallback;
  };

  return (
    <FormSheet
      title={t('meshJoin.title')}
      onRequestClose={() => closeInstance(instanceId)}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
    >
      <div className="m-form-hint"><MobileInfo title={t('node.field.meshRoutes')} summary={t('mobileHelp.meshJoinRoutes')} details={t('meshJoin.routesHint')} /></div>
      {MESH_JOIN_CHOICES.map((choice) => {
        const disposition = choice.disposition;
        const target = meshJoinFormFor(choice.id, servers);
        /* 登记成缺席 ⇒ **在场置灰 + 理由**（§3.3 第 2 条）。今天五支全是 `ported`，
           这一支因此是空转的 —— 留着不是装饰：桌面加第六个选择时，登记表逼一次显式处置决定，
           而那一条落进来时这里已经有画法了，不必再改一次呈现层。 */
        const blocked = disposition.kind !== 'ported' || target === null;
        return (
          <div key={choice.id} className="m-form-choice">
            <button
              type="button"
              className="m-form-choice-btn"
              disabled={blocked}
              onClick={() => {
                if (target === null) return;
                closeInstance(instanceId);
                open(target);
              }}
            >
              <b>{choice.label}</b>
              <span className="m-form-hint">{t(descKeyFor(choice.id, choice.descKey))}</span>
            </button>
            {/* 置灰的理由**常驻**，不挂在按钮上：`disabled` 的按钮触屏根本碰不到。 */}
            {disposition.kind !== 'ported' && (
              <p className="m-form-hint">{t(disposition.reasonKey)}</p>
            )}
          </div>
        );
      })}
    </FormSheet>
  );
}
