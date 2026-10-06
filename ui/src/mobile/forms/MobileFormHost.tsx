/**
 * 移动端表单宿主 —— `MobileFormDesc` 的**唯一**渲染点（对应桌面 `dialogs/DialogHost.tsx`）。
 *
 * # 整叠都渲染，不只渲染栈顶
 *
 * 「节点表单 → 放弃更改确认」「删除 → 二次确认」都是叠上去的。只渲染栈顶 ⇒ 下面那层被 React
 * **卸载**，草稿随之消失，用户按「取消」回到的是一张空表单。故整叠都在 DOM 里，`z-index` 随
 * 序号递增，上面那层的遮罩挡住下面那层。
 *
 * 这条同时给了返回键正确的层序：`FormSheet` 在**渲染期**登记进 `back-stack`（见那份文件），
 * 渲染顺序 = 栈顺序 ⇒ 系统返回键先关最上面那一层。
 *
 * # `never` 兜底
 *
 * `renderPanel` 的 `switch` 以 `const never: never = entry` 收口：`MobileFormDesc` 加一支而这里
 * 没补 case ⇒ **编译不过**。这是「后续每加一个 kind 都要重构宿主」与「加一行 case」之间的全部差别，
 * 也是本批把 union 一次建对的意义所在。
 *
 * # 挂在哪
 *
 * `MobileApp` 里、`MobileShell` **之外**（同级兄弟）。表单层是 `position: fixed` 的全屏层，
 * 不参与外壳的滚动区/停靠区排版；放进外壳会让它被 `overflow` 裁掉，也会逼外壳多认识一个概念。
 */

import { Fragment, type ReactElement } from 'react';
import './forms.css';
import { AppAddPanel } from './AppAddPanel';
import { ConfirmPanel } from './ConfirmPanel';
import { DnsGroupFormPanel, DnsServerFormPanel } from './DnsResourceFormPanel';
import { ImportFormPanel } from './ImportFormPanel';
import { MeshJoinPanel } from './MeshJoinPanel';
import { NetworkProfileFormPanel } from './NetworkProfileFormPanel';
import { NodeFormPanel } from './NodeFormPanel';
import { ResCatalogPanel, ResUrlPanel } from './ResourceAddPanel';
import { RuleFormPanel } from './RuleFormPanel';
import { SubCreateTaskPanel, SubFormPanel } from './SubFormPanel';
import { TaildropPanel } from './TaildropPanel';
import { TsExitPanel } from './TsExitPanel';
import { TsLoginPanel } from './TsLoginPanel';
import { TsSettingsPanel } from './TsSettingsPanel';
import { WarpPanel } from './WarpPanel';
import { WgPanel } from './WgPanel';
import { useMobileFormStore, type MobileFormEntry } from './form-store';

function renderPanel(entry: MobileFormEntry): ReactElement {
  switch (entry.kind) {
    case 'node':
      return (
        <NodeFormPanel
          instanceId={entry.instanceId}
          serverId={entry.serverId}
          initialProto={entry.initialProto}
        />
      );
    case 'sub':
      return (
        <SubFormPanel
          instanceId={entry.instanceId}
          subId={entry.subId}
          focus={entry.focus}
          onAdded={entry.onAdded}
        />
      );
    case 'sub-create-task':
      return <SubCreateTaskPanel instanceId={entry.instanceId} operationId={entry.operationId} />;
    case 'import':
      return <ImportFormPanel instanceId={entry.instanceId} onAdded={entry.onAdded} />;
    case 'mesh-join':
      return <MeshJoinPanel instanceId={entry.instanceId} />;
    case 'warp':
      return <WarpPanel instanceId={entry.instanceId} edit={entry.edit} />;
    case 'ts-login':
      return <TsLoginPanel instanceId={entry.instanceId} serverId={entry.serverId} replaceIdentity={entry.replaceIdentity} />;
    case 'ts-settings':
      return <TsSettingsPanel instanceId={entry.instanceId} serverId={entry.serverId} />;
    case 'wg':
      return <WgPanel instanceId={entry.instanceId} serverId={entry.serverId} />;
    case 'ts-exit':
      return <TsExitPanel instanceId={entry.instanceId} serverId={entry.serverId} />;
    case 'taildrop':
      return <TaildropPanel instanceId={entry.instanceId} serverId={entry.serverId} />;
    case 'rule':
      return (
        <RuleFormPanel
          instanceId={entry.instanceId}
          ruleId={entry.ruleId}
          preset={entry.preset}
          initialPlane={entry.initialPlane}
        />
      );
    case 'dns-server':
      return <DnsServerFormPanel instanceId={entry.instanceId} serverId={entry.serverId} />;
    case 'dns-group':
      return <DnsGroupFormPanel instanceId={entry.instanceId} groupId={entry.groupId} />;
    case 'network-profile':
      return (
        <NetworkProfileFormPanel
          instanceId={entry.instanceId}
          profileId={entry.profileId}
          onSaved={entry.onSaved}
        />
      );
    case 'app-add':
      return <AppAddPanel instanceId={entry.instanceId} />;
    case 'res-catalog':
      return <ResCatalogPanel instanceId={entry.instanceId} />;
    case 'res-url':
      return <ResUrlPanel instanceId={entry.instanceId} />;
    case 'confirm':
      return <ConfirmPanel instanceId={entry.instanceId} payload={entry.payload} />;
    default: {
      /* 穷尽兜底：union 加一支而这里没补 case ⇒ 这一行编译不过（见文件头注）。 */
      const never: never = entry;
      throw new Error(`unhandled MobileFormDesc kind: ${JSON.stringify(never)}`);
    }
  }
}

export function MobileFormHost(): ReactElement | null {
  const stack = useMobileFormStore((s) => s.stack);
  if (stack.length === 0) return null;
  /* 叠序**靠 DOM 顺序**，不靠逐层递增的 `z-index`：五层都是同一个 `z-index` 的 fixed 元素，
     同值时后出现的那个画在上面 —— 这正是「后 push 的在最上面」。给每层算一个不同的 z 值反而
     多引进一个要与栈序保持一致的数，而它一旦漂了没有任何东西会红。
     外层不再包一个定位容器：那个容器会是又一处 `position:fixed; inset:0`，被
     `back-navigation.test.tsx` ② 的遮罩取材面（B 形态腿）当成第三层背幕算进去，
     而它并不登记可关闭层 —— 那条红是对的，判据没错，是容器不该存在。 */
  return (
    <>
      {stack.map((entry) => (
        <Fragment key={entry.instanceId}>{renderPanel(entry)}</Fragment>
      ))}
    </>
  );
}
