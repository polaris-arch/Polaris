import { IdentityRepairDialog } from './IdentityRepairDialog';
/**
 * DialogHost —— 弹窗唯一挂载点（挂 AppShell 内，§1.3）。
 *
 * 按 store.stack 顺序渲染每个弹窗组件（各自包 <Modal> 原语）。原生 dialog 按 showModal 顺序叠 top-layer，
 * store 栈镜像之 —— 数组顺序 = 叠放次序（末尾最顶层）。instanceId 同时作为 key 和关闭目标，
 * 下层异步完成只移除自己的实例，保留上层实例及表单状态。
 *
 * **本文件在 Phase A 冻结**：所有 kind 的 case + import 一次到位，各批（D2–D5）只覆写自己那个
 * *Dialog 组件文件（单一 owner），不改本文件 → 并行零写冲突。switch 的 default 走 never 兜底：
 * 新增 DialogDesc.kind 未补 case → 编译期报错（穷尽性）。组件与 DialogDesc 解耦：payload 经此处
 * narrow 后以 props 下传，故各组件定义自己的 props 接口、不 import DialogDesc（避免循环耦合）。
 */

import { Fragment } from 'react';
import { useDialogStore, type DialogEntry } from './dialog-store';
import { ResUrlDialog } from './ResUrlDialog';
import { ConfirmDialog } from './ConfirmDialog';
import { NodeDialog } from './NodeDialog';
import { ImportDialog } from './ImportDialog';
import { SubDialog } from './SubDialog';
import { SubscriptionCreateTaskDialog } from './SubscriptionCreateTaskDialog';
import { WarpDialog } from './WarpDialog';
import { TsLoginDialog } from './TsLoginDialog';
import { TsSettingsDialog } from './TsSettingsDialog';
import { TaildropDialog } from './TaildropDialog';
import { WgDialog } from './WgDialog';
import { RuleDialog } from './RuleDialog';
import { ProcPickDialog } from './ProcPickDialog';
import { RulePickDialog } from './RulePickDialog';
import { AppAddDialog } from './AppAddDialog';
import { ResCatalogDialog } from './ResCatalogDialog';
import { BackupImportDialog } from './BackupImportDialog';
import { MeshJoinDialog } from './MeshJoinDialog';
import { DnsGroupDialog, DnsServerDialog } from './DnsResourceDialog';
import {
  NetworkProfileDialog,
  NetworkProfilesDialog,
} from '../screens/rules/NetworkProfilePanel';
import { VpnAuthDialog } from './VpnAuthDialog';

function renderDialog(desc: DialogEntry) {
  switch (desc.kind) {
    case 'identity-repair': return <IdentityRepairDialog instanceId={desc.instanceId} />;
    case 'res-url':
      return <ResUrlDialog instanceId={desc.instanceId} />;
    case 'confirm':
      return <ConfirmDialog payload={desc.payload} />;
    case 'node':
      return <NodeDialog instanceId={desc.instanceId} serverId={desc.serverId} initialProto={desc.initialProto} />;
    case 'import':
      return <ImportDialog instanceId={desc.instanceId} onAdded={desc.onAdded} />;
    case 'sub':
      return <SubDialog instanceId={desc.instanceId} subId={desc.subId} focus={desc.focus} onAdded={desc.onAdded} />;
    case 'sub-create-task':
      return <SubscriptionCreateTaskDialog instanceId={desc.instanceId} operationId={desc.operationId} />;
    case 'warp':
      return <WarpDialog instanceId={desc.instanceId} edit={desc.edit} />;
    case 'ts-login':
      return <TsLoginDialog instanceId={desc.instanceId} serverId={desc.serverId} replaceIdentity={desc.replaceIdentity} />;
    case 'ts-settings':
      return <TsSettingsDialog instanceId={desc.instanceId} serverId={desc.serverId} />;
    case 'taildrop':
      return <TaildropDialog serverId={desc.serverId} />;
    case 'vpn-auth':
      return <VpnAuthDialog instanceId={desc.instanceId} protocol={desc.protocol} serverId={desc.serverId} />;
    case 'wg':
      return <WgDialog instanceId={desc.instanceId} serverId={desc.serverId} />;
    case 'mesh-join':
      return (
        <MeshJoinDialog
          onTsLogout={desc.onTsLogout}
          onWarpReregister={desc.onWarpReregister}
          onWarpDeregister={desc.onWarpDeregister}
        />
      );
    case 'rule':
      return <RuleDialog instanceId={desc.instanceId} ruleId={desc.ruleId} preset={desc.preset} initialPlane={desc.initialPlane} />;
    case 'dns-server':
      return <DnsServerDialog instanceId={desc.instanceId} serverId={desc.serverId} />;
    case 'dns-group':
      return <DnsGroupDialog instanceId={desc.instanceId} groupId={desc.groupId} />;
    case 'network-profiles':
      return <NetworkProfilesDialog instanceId={desc.instanceId} />;
    case 'network-profile':
      return <NetworkProfileDialog instanceId={desc.instanceId} profileId={desc.profileId} onSaved={desc.onSaved} />;
    case 'proc-pick':
      return <ProcPickDialog initialSelected={desc.initialSelected} onPick={desc.onPick} />;
    case 'rule-pick':
      return <RulePickDialog subject={desc.subject} onPick={desc.onPick} />;
    case 'app-add':
      return <AppAddDialog instanceId={desc.instanceId} />;
    case 'res-catalog':
      return <ResCatalogDialog instanceId={desc.instanceId} />;
    case 'backup-import':
      return <BackupImportDialog instanceId={desc.instanceId} />;
    default: {
      // 穷尽性守卫：新增 kind 未补 case → 编译期报错。
      const _exhaustive: never = desc;
      return _exhaustive;
    }
  }
}

export function DialogHost() {
  const stack = useDialogStore((s) => s.stack);
  return (
    <>
      {stack.map((desc) => (
        <Fragment key={desc.instanceId}>{renderDialog(desc)}</Fragment>
      ))}
    </>
  );
}

export default DialogHost;
