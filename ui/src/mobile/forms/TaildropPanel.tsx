import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端 **Taildrop 收件箱**（2026-09-13 批 16 接通，销 `register:mesh-action:taildrop`）。
 * 桌面对位：`components/dialogs/TaildropDialog.tsx`。
 *
 * # 它治的是一个已经张开的口子（这一条决定了本面板为什么必须存在）
 *
 * 核从 sing-box 1.14.0-beta.15 起**无条件**建收件目录并注册收件 handler，所以只要 tailnet 授了
 * `cap/file-sharing`，对端发来的文件早就在往盘上落。没有这张面之前，移动端用户拥有的是一个
 * **看不见、也清不掉**的收件箱 —— 桌面头注那句「不是新功能，是补一个已经张开的口子」在这台
 * 设备上只多不少：手机上没有第二个办法去翻那个目录。
 *
 * # 入口在哪，以及为什么不在组网接入面上（本批的一处判断，写清依据）
 *
 * 桌面把这颗挂在 `MeshJoinDialog` 的 Tailscale 卡片上，而且**本轮刚把那张卡改成多节点分行** ——
 * 理由是 `kind:'taildrop'` 全仓只此一个 `go`，绑在 `.find()` 取到的第一个节点上等于让多节点
 * 用户永远进不去别的账号的收件箱。
 *
 * 移动端把它收进 `TsSettingsPanel` 的账号级动作那一行（与「切换账号 / 退出登录」同处），
 * 于是**桌面那条理由在这里不成立**：
 *  · `TsSettingsPanel` 的 union 那一支**必须带 `serverId`**（`form-store.ts`），它是移动端唯一
 *    一处「这一个 Tailscale 节点」的语境 —— 收件箱因此天然按节点寻址，不存在「绑第一个」；
 *  · 每个 Tailscale 节点在节点屏上各有一行，行动作面的「编辑」→ `mobileEditFormFor` →
 *    `{ kind:'ts-settings', serverId }` ⇒ **每一个**节点的收件箱都到得了；
 *  · 接入面是一列纵向选择（拇指区），它的职责是「接入一个新的」，不是「管理已有的 N 个」；
 *    按节点分行会把三条隧道挤出首屏，还要连带移植 `tsAccountLabel` 副标题与一份整表
 *    `tailscaleStatuses` 订阅 —— 那份订阅在一张短命面板上买不到东西。
 * 这条判断与批 3 给 `ts-switch-account` / `ts-logout` 定的口径是同一条，不是本批新发明的。
 *
 * # 三态而不是「灰掉」
 *
 * 能不能用由 `domain/taildrop#taildropAvailability` 判三态（与桌面同一函数），界面据此**说出原因**：
 * 核没跑 / tailnet 没授权 / 可用。尤其 `notGranted` 那一格 —— 在本应用里怎么点都没用，
 * 得去 admin console 开 —— 不说出来就是本仓反复记过的「拨了不生效的控件」。
 *
 * # 取件与发件在 Android 上**真的接上了**（2026-09-13 批 16 的 Rust 侧改动）
 *
 * 🔴 **这一段此前写的是另一件事，记在这里当口径。** 上一版说「两条腿在 Android 上必失败，
 * 但照样画，因为后端会显式报错」，并援引 `MobileConnectionsScreen` 的 `archive-legacy` 当先例。
 * 那个先例**不成立**：`archiveLegacy` 那两颗按钮由 `legacyLog?.exists` 门控，而
 * `LEGACY_SINGBOX_LOG` 是 W26 之前的**桌面**遗留文件、移动端从来没产生过它 ⇒ 在 Android 上
 * 那两颗**根本不渲染**，用户点不到。而这两颗点得动。
 * 「用户点不到」与「用户点了必报错」不是同一档 —— 后者正是本仓反复判过的
 * 「画一颗按下去不产生任何发射差异的控件，比没有更坏」。
 *
 * 真正的处置是把腿接上，而机器批 4 就造好了（`src-tauri/src/commands/picked_file`）：
 *  · **发件** —— `taildrop_send` 不再要求 `into_path()`，改经 `open_selected_targets`：
 *    SAF 目标走 `open_picked_for_read` 拿 fd、`file_name_of` 取名；
 *  · **取件** —— `taildrop_save` 的 URI 支先把整份内容流式落进应用私有临时文件，
 *    完整之后再一次性经 `stream_into_picked` 灌进用户选的文档。
 *
 * # 两处**真实的射程边界**（不是「没做」，是形态冲突；后端逐条写清，这里同口径复述）
 *
 * 1. **取件的原子性有一条拿不回来。** 桌面是同目录 `.part` → `rename`，提交是原子的；
 *    SAF 没有 rename，`tauri-plugin-fs` 也不暴露 `deleteDocument` ⇒ 写到一半失败会在用户选的
 *    文档上留下**半截**。先落临时文件把窗口缩到最小（网络中断再也碰不到目标文档），
 *    但那扇窗仍非零。降级表逐行写在 `taildrop.rs#taildrop_save` 的注释里。
 * 2. **发件的文件名在 SAF 目标上会退化**成 URI 末节（`file_name_of` 的既定口径，它刻意不做
 *    百分号解码）—— 对端可能收到一个像 document id 的名字。**文件内容一个字节不受影响**。
 *    另有一档当场拒绝：云盘类 provider 交回的管道 fd 给不出确定长度，而 Taildrop 协议要求
 *    先声明每个文件的大小，声明错的长度会让对端收到「传完了却损坏」的文件。
 *
 * 三个动作各自的状态因此是：**收件箱列表 / 删除 / 取消接收**本来就通（纯 gRPC，不经选择框）；
 * **取件**与**发件**本批接上，各带上面那条如实登记的边界。
 *
 * # 发件进度读**应用级** store，不读面板自持的 state（2026-09-25 ζ 批 A12）
 *
 * 🔴 **这一段此前写的是「不接任务事件订阅，用刷新代替」**：进度存在面板自己的 `useState` 里，
 * 只在打开 / 操作后 / 按刷新时拉一次 ⇒ 发件进行中进度不会自己跳，关掉面板再打开也只是再拉一次
 * 当时的快照。发一个大文件时，这一格等于一张静态截图。
 *
 * 现在与桌面同形：`mobile/app-wiring.ts` 在应用级先挂 `subscribeTaildropTaskEvents()`、再
 * `hydrateTaildropTasks()`（顺序与理由同桌面 `App.tsx`），事件逐帧落 `useTaildropTaskStore`；
 * 本面板只是那张表的**视图**（`visibleTaildropTasks`，桌面 `TaildropDialog` 读同一份）。
 * 面板关了订阅照样活着，再打开读到的就是当下那一帧；打开时 / 操作后那次 pull 仍在，但它现在
 * 是**并进** store（per-task revision 拒绝旧快照覆盖新事件），不再覆盖本地 state。
 */

import { useCallback, useEffect, useMemo, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { IpcError } from '@/ipc/ipc-client';
import type { TaildropInbox, TaildropTaskSnapshot } from '@/contracts/taildrop';
import { fmtBytes } from '@/components/screens/shared/format';
import { relativeTimeText } from '@/lib/relative-time';
import { receivingPercent, taildropAvailability, taildropErrorKey } from '@/domain/taildrop';
import { useAppStore } from '@/store/app-store';
import { useTaildropTaskStore, visibleTaildropTasks } from '@/store/use-taildrop-task-store';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

const EMPTY: TaildropInbox = { files: [], receiving: [] };

/**
 * 后端失败 → i18n 键（**纯函数，不碰 UI**）。
 *
 * 它故意**只算键、不落 notice**：`write-failure-visibility.test.ts` ② 的判据是**词法**的 ——
 * 每一处写调用必须落在一个「catch 里真的调了 `setNotice`」的 try 块内。把 `setNotice` 包进一个
 * `report(e)` 帮手里，写调用就会整片逃出辖区，而那道门要抓的正是「失败了但用户看不到任何东西」。
 * 判据是对的，是代码不该那么摆（`WarpPanel#submit` 头注为同一条记过一次）。
 *
 * **绝不把 `error` 里的英文诊断显示给用户**：那是给日志的（Rust 侧用户可见 sink 禁裸中文，
 * 文案一律住 locale）。查不到的 code 回落通用文案，不回落成把诊断串贴出去。
 */
function errorKeyOf(e: unknown): string {
  return taildropErrorKey(e instanceof IpcError ? e.code : undefined);
}

export function TaildropPanel({
  instanceId,
  serverId,
}: {
  instanceId: string;
  serverId: string;
}): ReactElement {
  const { t } = useTranslation();
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const open = useMobileFormStore((s) => s.open);
  /* 按 `serverId` 取这一个节点的状态帧：Tailscale 已不是单例，`.find()` 取任意一个会让
     角标、peer 候选与可用性三样都算到别人头上。 */
  const status = useAppStore((s) => s.tailscaleStatuses[serverId]);
  const availability = taildropAvailability(status);

  const [inbox, setInbox] = useState<TaildropInbox>(EMPTY);
  /* 发件任务读应用级 store（见头注 A12）：面板关掉再打开，进度仍是事件推过来的当下那一帧。 */
  const taskMap = useTaildropTaskStore((s) => s.tasks);
  const applyTaskSnapshot = useTaildropTaskStore((s) => s.applySnapshot);
  const hydrateTaskSnapshots = useTaildropTaskStore((s) => s.hydrateSnapshots);
  const tasks: readonly TaildropTaskSnapshot[] = useMemo(
    () => visibleTaildropTasks(taskMap, serverId),
    [taskMap, serverId],
  );
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [peerStableId, setPeerStableId] = useState('');
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  /**
   * 可选的收件方。**离线 / 没开收文件的 peer 留在列表里但禁用**，并在标签上写出原因 ——
   * 直接过滤掉会让用户以为「那台设备不在我的 tailnet 里」，那是另一回事。
   */
  const peerOptions = useMemo(
    () =>
      (status?.peers ?? [])
        .filter((peer) => Boolean(peer.stableID))
        .map((peer) => {
          const canReceive = peer.details?.canReceiveFiles === true;
          const why = !peer.online
            ? t('taildrop.peerOffline')
            : !canReceive
              ? t('taildrop.peerCannotReceive')
              : '';
          return {
            value: peer.stableID as string,
            label: why ? `${peer.hostName} · ${why}` : peer.hostName,
            disabled: !peer.online || !canReceive,
          };
        }),
    [status?.peers, t],
  );

  /* 选中的那个变得不可用（对端下线 / 关掉了收文件）⇒ 自动挪到第一个还能收的。 */
  useEffect(() => {
    if (peerOptions.some((p) => p.value === peerStableId && !p.disabled)) return;
    setPeerStableId(peerOptions.find((p) => !p.disabled)?.value ?? '');
  }, [peerOptions, peerStableId]);

  const refresh = useCallback(async (): Promise<void> => {
    if (availability !== 'ready') return;
    setLoading(true);
    try {
      /* 两份一起拉：收件箱与发件任务是同一屏上的两块，分两次拉会让刷新出现半新半旧的一帧。 */
      const [snapshot, taskList] = await Promise.all([
        api.server.taildropList(serverId),
        api.server.taildropTasks(serverId),
      ]);
      /* 任务快照并进应用级 store（按 revision 合并，不会用旧 pull 盖掉更新的事件帧）；
         面板已经关了也照并 —— store 不属于这张面板。 */
      hydrateTaskSnapshots(taskList);
      if (!hasInstance(instanceId)) return;
      setInbox(snapshot);
    } catch (e) {
      console.error('[mobile-taildrop] refresh failed:', e);
      if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t(errorKeyOf(e)) });
    } finally {
      if (hasInstance(instanceId)) setLoading(false);
    }
  }, [availability, hasInstance, hydrateTaskSnapshots, instanceId, serverId, t]);

  /**
   * 打开时拉一次 + 清一次未读角标。STATUS 的三个真实计数变化会重跑本 effect
   * （新接收开始 / 完成 / 未读改变时无需用户手刷）—— 那条事件流移动端**已经接了**
   * （`app-wiring.ts` 的 `tailscaleStatuses`），与任务事件不是同一条。
   *
   * 标记已读**失败静默**：它纯属体验，不该盖住真正的内容错误。
   */
  useEffect(() => {
    void refresh();
    if (availability === 'ready') void api.server.taildropMarkRead(serverId).catch(() => {});
  }, [
    refresh,
    availability,
    serverId,
    status?.waitingFileCount,
    status?.receivingFileCount,
    status?.unreadFileCount,
  ]);

  /*
   * ── 五条写腿 ──────────────────────────────────────────────────────────────────
   * 每一条**各自**一个 `try { …写… } catch { setNotice(…) }`，不抽公共 wrapper：
   * 理由与 `errorKeyOf` 头注同一条（词法判据），`WarpPanel#submit` 为同一条记过一次。
   * 五条的共同形状：操作期间禁用那一行的按钮 → 成功后重拉（后端无推送，不拉看不到结果）
   * → 失败落**本面板的 notice**（表单宿主的既定通道：失败时用户的下一步就在这张表里）。
   */

  /**
   * 取件。**2026-09-13 批 16 起在 Android 上真的落得了盘**（后端 `taildrop_save` 的 URI 支：
   * 先流式落进应用私有临时文件，完整之后再一次性灌进用户选的文档）。
   *
   * 仍会失败的那一档是真失败（写不进去 / provider 拒绝），落一句已本地化的红字；
   * 提交那一跳的原子性在 SAF 上拿不回来，边界写在文件头注与 `taildrop.rs` 的降级表里。
   */
  const onSave = (name: string): void => {
    const key = `save:${name}`;
    setBusy(key);
    setNotice(undefined);
    void (async () => {
      try {
        const r = await api.server.taildropSave(serverId, name);
        /* 取消不是失败：用户按了保存框的取消，什么都不该提示。后端把那一档与「选好了位置但
           这台设备写不进去」**分开**报，正是为了让这里分得开。 */
        if (!r.canceled) toast.success(t('taildrop.saved'));
        await refresh();
      } catch (e) {
        console.error('[mobile-taildrop] save failed:', e);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t(errorKeyOf(e)) });
      } finally {
        if (hasInstance(instanceId)) setBusy(null);
      }
    })();
  };

  /** 删除是破坏性的 ⇒ 叠一层确认面板（触屏没有 hover，桌面那种「翻红再点一次」在这里不成立）。 */
  const onDelete = (name: string): void => {
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('common.delete'),
        message: name,
        confirmLabel: t('common.delete'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          const key = `del:${name}`;
          setBusy(key);
          setNotice(undefined);
          void (async () => {
            try {
              await api.server.taildropDelete(serverId, name);
              await refresh();
            } catch (e) {
              console.error('[mobile-taildrop] delete failed:', e);
              if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t(errorKeyOf(e)) });
            } finally {
              if (hasInstance(instanceId)) setBusy(null);
            }
          })();
        },
      },
    });
  };

  /** 取消一条**接收中**的传输。 */
  const onCancelReceiving = (senderId: string, name: string): void => {
    const key = `cancel:${senderId}:${name}`;
    setBusy(key);
    setNotice(undefined);
    void (async () => {
      try {
        await api.server.taildropCancel(serverId, senderId, name);
        await refresh();
      } catch (e) {
        console.error('[mobile-taildrop] cancel receiving failed:', e);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t(errorKeyOf(e)) });
      } finally {
        if (hasInstance(instanceId)) setBusy(null);
      }
    })();
  };

  /** 取消一条**发件**任务。 */
  const onCancelSend = (taskId: string): void => {
    const key = `task:${taskId}`;
    setBusy(key);
    setNotice(undefined);
    void (async () => {
      try {
        /* 回执本身就是一帧快照（同桌面）：先并进 store，取消态当场可见，不等事件。 */
        applyTaskSnapshot(await api.server.taildropTaskCancel(taskId));
        await refresh();
      } catch (e) {
        console.error('[mobile-taildrop] cancel send failed:', e);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t(errorKeyOf(e)) });
      } finally {
        if (hasInstance(instanceId)) setBusy(null);
      }
    })();
  };

  /**
   * 发件。**2026-09-13 批 16 起在 Android 上真的发得出去**（后端 `open_selected_targets` 的
   * URI 支：`open_picked_for_read` 拿 fd、`file_name_of` 取名）。
   *
   * 两处如实登记的边界见文件头注：名字在 SAF 目标上退化成 URI 末节；云盘类 provider 的管道 fd
   * 给不出确定长度，那一档后端当场拒绝（发一个长度错的声明会让对端收到损坏文件）。
   */
  const onSend = (): void => {
    if (!peerStableId || sending) return;
    setSending(true);
    setNotice(undefined);
    void (async () => {
      try {
        await api.server.taildropSend(serverId, peerStableId);
        await refresh();
      } catch (e) {
        console.error('[mobile-taildrop] send failed:', e);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: t(errorKeyOf(e)) });
      } finally {
        if (hasInstance(instanceId)) setSending(false);
      }
    })();
  };

  const body = (): ReactElement => {
    if (availability === 'offline') return <p className="m-form-hint">{t('taildrop.offline')}</p>;
    if (availability === 'notGranted')
      return <p className="m-form-hint">{t('taildrop.notGranted')}</p>;

    const hasUsablePeer = peerOptions.some((p) => !p.disabled);
    return (
      <>
        <div className="m-form-row">
          <label className="m-form-label" htmlFor="mtd-peer">
            {t('taildrop.send')}
          </label>
          <MobileSelect
            id="mtd-peer"
            className="m-form-select"
            value={peerStableId}
            disabled={sending || peerOptions.length === 0}
            onChange={(e) => setPeerStableId(e.target.value)}
          >
            {peerOptions.length === 0 && <option value="">{t('taildrop.peerPlaceholder')}</option>}
            {peerOptions.map((p) => (
              <option key={p.value} value={p.value} disabled={p.disabled}>
                {p.label}
              </option>
            ))}
          </MobileSelect>
          <div className="m-form-inline">
            <button
              type="button"
              className="m-form-btn"
              disabled={!peerStableId || sending}
              onClick={onSend}
            >
              {sending ? t('taildrop.sending') : t('taildrop.chooseFiles')}
            </button>
          </div>
          <p className="m-form-hint">
            {hasUsablePeer ? t('taildrop.sendHint') : t('taildrop.noPeers')}
          </p>
        </div>

        {tasks.length > 0 && (
          <div className="m-form-row">
            <span className="m-form-label">{t('taildrop.outgoing')}</span>
            {tasks.map((task) => {
              const peerName =
                status?.peers.find((p) => p.stableID === task.peerStableId)?.hostName
                ?? t('taildrop.unknownPeer');
              const phaseText =
                task.phase === 'completed'
                  ? t('taildrop.sent', {
                      count: task.files.length,
                      bytes: fmtBytes(task.totalBytes),
                    })
                  : task.phase === 'failed'
                    ? `${t('taildrop.taskPhase.failed')} · ${t(taildropErrorKey(task.errorCode))}`
                    : t(`taildrop.taskPhase.${task.phase}`);
              const cancelable = task.phase === 'connecting' || task.phase === 'sending';
              return (
                <div key={task.taskId} className="mr-sel-opt-tx">
                  <span>{t('taildrop.toPeer', { peer: peerName })}</span>
                  {/* 进度给**数字**不给条：条那一套样式住在桌面层叠链上（契约 A1），
                      而这里要回答的是「传了多少」，数字答得同样准。 */}
                  <span className="mr-sheet-item-sub">
                    {phaseText}
                    {' · '}
                    {receivingPercent(task.sentBytes, task.totalBytes)}
                    {'% · '}
                    {t('taildrop.progressBytes', {
                      sent: fmtBytes(task.sentBytes),
                      total: fmtBytes(task.totalBytes),
                    })}
                  </span>
                  {(cancelable || task.phase === 'canceling') && (
                    <div className="m-form-inline">
                      <button
                        type="button"
                        className="m-form-btn danger"
                        disabled={task.phase === 'canceling' || busy === `task:${task.taskId}`}
                        onClick={() => onCancelSend(task.taskId)}
                      >
                        {task.phase === 'canceling'
                          ? t('taildrop.taskPhase.canceling')
                          : t('taildrop.cancelSend')}
                      </button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        <div className="m-form-row">
          <span className="m-form-label">{t('taildrop.inbox')}</span>

          {inbox.receiving.map((r) => {
            const key = `cancel:${r.senderID}:${r.name}`;
            return (
              <div key={key} className="mr-sel-opt-tx">
                <span className="mono">{r.name}</span>
                <span className="mr-sheet-item-sub">
                  {t('taildrop.receiving')}
                  {' · '}
                  {t('taildrop.fromSender', { sender: r.senderName })}
                  {' · '}
                  {receivingPercent(r.receivedBytes, r.size)}
                  {`% · ${fmtBytes(r.receivedBytes)} / ${fmtBytes(r.size)}`}
                </span>
                <div className="m-form-inline">
                  <button
                    type="button"
                    className="m-form-btn danger"
                    disabled={busy === key}
                    onClick={() => onCancelReceiving(r.senderID, r.name)}
                  >
                    {t('taildrop.cancelReceiving')}
                  </button>
                </div>
              </div>
            );
          })}

          {inbox.files.map((f) => (
            <div key={f.name} className="mr-sel-opt-tx">
              <span className="mono">{f.name}</span>
              <span className="mr-sheet-item-sub">
                {t('taildrop.waiting')}
                {' · '}
                {t('taildrop.fromSender', { sender: f.senderName })}
                {` · ${fmtBytes(f.size)} · `}
                {/* 后端给的是 Unix **秒**，`relativeTimeText` 吃毫秒 —— 少这个 ×1000 会把
                    每一条都渲染成 1970 年。 */}
                {relativeTimeText(f.modifiedAt * 1000, t)}
              </span>
              <div className="m-form-inline">
                <button
                  type="button"
                  className="m-form-btn"
                  disabled={busy === `save:${f.name}`}
                  onClick={() => onSave(f.name)}
                >
                  {t('common.save')}
                </button>
                <button
                  type="button"
                  className="m-form-btn danger"
                  disabled={busy === `del:${f.name}`}
                  onClick={() => onDelete(f.name)}
                >
                  {t('common.delete')}
                </button>
              </div>
            </div>
          ))}

          {inbox.files.length === 0 && inbox.receiving.length === 0 && (
            <p className="m-form-hint">{loading ? t('common.loading') : t('taildrop.empty')}</p>
          )}
        </div>
      </>
    );
  };

  return (
    <FormSheet
      title={t('taildrop.title')}
      /* 发件期间锁住关闭：那条 continuation 还要往这张面板写 notice。 */
      onRequestClose={() => closeInstance(instanceId)}
      closeLocked={sending}
      closeLabel={t('common.close')}
      cancelLabel={t('common.close')}
      submitLabel={t('common.refresh')}
      submitDisabled={loading || sending || availability !== 'ready'}
      onSubmit={() => void refresh()}
      notice={notice}
    >
      {body()}
    </FormSheet>
  );
}
