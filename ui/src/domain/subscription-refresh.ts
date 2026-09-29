/**
 * 订阅手动拉取 + 三态 toast 的单一真值（NodesScreen 刷新按钮 与 SubDialog 新增后自动拉取共用）。
 *
 * 三态由后端 `updateServers` 的真实返回驱动（契约 §16.3.4）：
 *  - 业务失败（`success:false`）→ 报后端真实 `error`（不用「部分节点解析失败」这类笼统文案替代真值）；
 *  - 无变化（`unchanged`，304/内容等价）→ 中性提示（否则计数 0/0/0 会误显「已更新」）；
 *  - 有变化 → 成功报节点变化数。
 *
 * 抽出成 domain 函数是为了让「新增订阅后拉取」与「列表手动刷新」共用同一份三态语义，防两处分叉。
 *
 * # 为什么返回结构体而不是 `boolean`（2026-09-05）
 *
 * 失败详情由 `subscriptionErrorDetail(r, t)` 算出来后**只进了 toast**，而 toast 的实现只在桌面
 * 注入，而当时 `components/layout/Toaster.tsx` 是全仓唯一的 `setToastImpl` 调用点（移动端自己的
 * 宿主 `mobile/MobileToaster.tsx` 是 2026-09-06 才有的）⇒ 移动外壳里
 * `toast.error(...)` 就是 `console.error(...)`。移动端节点屏此前只拿得到 `boolean`，于是屏内
 * 只能回显一句笼统的「刷新失败」——「为什么失败」这条信息在跨端复用的这一格上**丢了**。
 *
 * 两条修法里选了「把详情一并返回」，没选「移动端另走一条不经 toast 的调用路径」：
 *  · 三态语义（失败 / 无变化 / 有变化各报什么）**就是**本函数存在的理由。多一条入口就是多一份
 *    三态实现，两端迟早分叉 —— 那正是 2026 年把它抽成 domain 函数时要防的东西。
 *  · 桌面那条路一个字节都不用改：toast 照旧在本函数里发，调用点连返回值都不读。
 *    「桌面行为不变」不靠人眼看 diff，靠 `subscription-refresh.desktop-toast.test.ts` 的
 *    行为对照钉住（三态 × toast 通道 × 文案实参逐条断言）。
 *  · 消费方只多读一个字段，不需要 `?? t('nodes.subRefreshFail')` 这类第二层兜底 ——
 *    判别联合让「失败一定有详情」变成类型层面的事实。
 */
import type { TFunction } from 'i18next';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { subscriptionErrorDetail } from './subscription-error-text';

/**
 * 拉取结果。**失败一定带详情**（判别联合，不是 `detail?: string`）：
 * 可选字段会让消费方写一个「详情没有就回落笼统文案」的分支，而那个分支正是本次要消灭的东西。
 */
export type SubscriptionRefreshOutcome =
  | { readonly ok: true }
  | { readonly ok: false; readonly detail: string };

/**
 * 拉取指定订阅并弹三态 toast。
 *
 * 返回后端业务是否成功；失败时**一并返回**与 toast 正文同一份详情，供没有 toast 宿主的外壳
 * （移动端）自己回显。异常/失败均已 toast，桌面调用方无需再报。
 */
export async function refreshSubscriptionWithToast(
  subId: string,
  t: TFunction
): Promise<SubscriptionRefreshOutcome> {
  try {
    const r = await api.subscription.updateServers(subId);
    if (!r.success) {
      const detail = subscriptionErrorDetail(r, t);
      toast.error(t('nodes.subRefreshFail'), detail);
      return { ok: false, detail };
    }
    if (r.unchanged) {
      toast.info(t('nodes.subRefreshUnchanged'));
      return { ok: true };
    }
    const changed = r.addedServers + r.updatedServers + r.deletedServers;
    toast.success(
      t('nodes.subRefreshOk', { count: changed })
    );
    return { ok: true };
  } catch (err) {
    console.error('[subscription-refresh]', err);
    // IPC reject 不保证来自本命令；不能把潜在的 Rust/transport 文案直接带到 UI。
    toast.error(t('nodes.subRefreshFail'), t('nodes.subRefreshFail'));
    return { ok: false, detail: t('nodes.subRefreshFail') };
  }
}
