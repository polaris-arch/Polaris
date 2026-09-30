/**
 * 移动端 **Cloudflare WARP 接入表**（注册 / 编辑）—— 批 3，销掉 `register:mesh-join:warp`。
 *
 * # 这一条与另外两张表的差别：它是一次**远端副作用**
 *
 * `api.server.registerWarp` 在 Cloudflare 侧**真建一台匿名设备**（X25519 keypair + 匿名注册），
 * 本地拦不回来。故三件事必须成立，缺一条都会烧掉一台孤儿设备或让用户对着一片空白猜：
 *
 *  ① **闸前置到请求之前** —— 走 `domain/mesh-singleton-guard#registerWarpIfSlotFree`
 *     （**不是**在这里重写一个「有没有 WARP」的 if）。WARP 是单例槽：面板停留期间槽位可能被
 *     克隆 / 导入 / WG 粘贴 `.conf` 抢走，那时先打请求再拦 = 白烧一台设备。
 *     闸不过 ⇒ 它自己弹 toast + 返回 `null`，本面板保持打开，零远端调用。
 *  ② **失败必须可见**。这条腿的失败分支比另外两张表多得多：配额 / 地区 / 已注册 / 许可证无效 /
 *     无凭据 / 网络不通。全部落**面板内的 `notice`**（脚上方，与提交键在同一屏）。
 *     单例槽那一档由 `registerWarpIfSlotFree` 落**全局 toast**（宿主 `MobileToaster` 已在，
 *     `setToastImpl` 真的注入了 —— 见 `half-truth-facts.test.ts` C 组），文案不在这里再抄一份；
 *     🔴 但可见性**不单押在那条跨层通道上**（2026-09-06 复审 major）：这张表是个 `z-index:40`
 *     的全屏层，而 toast 宿主贴在它下面的停靠区上沿 —— 此前宿主没有 `z-index`，那一档的唯一
 *     反馈整片被压住，用户按下「注册」之后屏幕上什么都不发生。层叠已修
 *     （`mobile.css#.m-toast-host` 取 50，判据 `mobile-chrome.test.tsx` ⑥），
 *     同时那一档的同一句理由经 `onBlocked` 回调也落进面板内的 `notice`。
 *  ③ **成功态自己说话**。注册成功不立刻关面板，切到「已接入」那一屏（`warp.doneTitle` /
 *     `warp.doneSub`，逐字同桌面）：注册产物是一台**远端**设备，用户需要一个明确的收据。
 *
 * # 复用的是什么
 *
 *  · **字段表 / 端点解析 / 三档提交校验** —— `components/dialogs/warp-spec`
 *    （本批从 `WarpDialog.tsx` 拆出的零 React 半，两端**同一份**判据）；
 *  · **草稿 → `WireGuardSettings`** —— `wg-logic#buildWarpSettings`（WARP 内部的路由/接入模式
 *    在提交边界收口，不能只靠「界面没展示」假定旧配置里不存在）；
 *  · **前置代理候选 / 写回** —— `detour-options#endpointDetourOptions` / `#applyDetour`
 *    （空值与哨兵 ⇒ **删键**，不写字面量 `'direct'`）；
 *  · **暂存 staged-only 拦截**（编辑态）—— `lib/staged-config#splitStagedOnly`：
 *    `applyWarpLicense` 改的是远端账户等级、`update` 改的是一台**已注册**的设备，
 *    盘上没有这个节点时两者都没有作用对象。
 *
 * # 「重新注册 / 注销」收在编辑态的末尾（2026-09-13 批 16 接通）
 *
 * 桌面把这两颗挂在**组网接入面的 WARP 卡片**上（`MeshJoinDialog` 的 `actions`）。移动端那张
 * 接入面是一列纵向选择（拇指区），塞不下每条 2–3 颗次动作 —— 这条判断不是本批新发明的，
 * 它是批 3 给 Tailscale 那两颗账号级动作定的口径（`MeshJoinPanel` 头注 + `TsSettingsPanel`
 * 末尾那一行），本批只是让 WARP 这两颗**落在同一条口径上**：
 * 「已注册的那一台」的语境只有编辑态这张表有，次动作就摆在那里。
 *
 * 腿本身**一条都不新写**：`mobile/nodes/node-deletion.ts#removeWarpNode` ——
 * 它与逐行删除是同一条编排（暂存事务 / 写入路由分流 / 兜底改选三段复用桌面纯函数），
 * 只换确认与成功文案，外加「重新注册」那一支的 `afterDelete`（删完立刻开注册表，
 * 逐字同桌面 `NodesScreen.tsx#openMeshJoin` 的 `onWarpReregister`）。
 *
 * ⚠️ 这两颗**只在编辑态**出现：注册态那张表还没有 WARP 节点，没有作用对象。
 */

import { useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
/* 「展开即露出」的全仓不变量（`components/reveal.ts`）：高级组在这张表的最底下，
   展开时新长出来的五个字段整段落在 `.m-form-body` 的视区之外。 */
import { revealElement, useRevealAfterCommit } from '@/components/reveal';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import type { ServerConfig } from '@/contracts/types';
import type { FormValue, FormValues, SelectOption } from '@/components/dialogs/field-spec';
import {
  WARP_DEFAULT_NAME,
  planWarpSubmit,
  warpAdvancedSpec,
  warpDraftFromNode,
  type WarpPlan,
} from '@/components/dialogs/warp-spec';
import { applyDetour, endpointDetourOptions, DETOUR_NONE } from '@/components/dialogs/detour-options';
import { buildWarpSettings } from '@/components/dialogs/wg-logic';
import { buildRegisteredWarpServer, saveRegisteredWarp, type WarpRegistrationAttempt } from '@/components/dialogs/warp-registration';
import { findWarpNode } from '@/domain/warp';
import { registerWarpIfSlotFree } from '@/domain/mesh-singleton-guard';
import { splitStagedOnly, stagedOnlyIds } from '@/lib/staged-config';
import { buildNetworkInterfaceChoices, useNetworkInterfaces } from '@/hooks/use-network-interfaces';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useMobileNodeDeletion } from '../nodes/node-deletion';
import { MobileFields } from './FormFields';
import { FormSheet } from './FormSheet';
import { FormGroup } from './FormGroup';
import { useMobileFormStore } from './form-store';

export function WarpPanel({
  instanceId,
  edit,
}: {
  instanceId: string;
  edit?: boolean;
}): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  /* 展示面 effective + 操作面磁盘镜像：staged-only 差集的两个入参，判据与节点行同一函数。 */
  const servers = useEffectiveServers();
  const diskServers = useAppStore((s) => s.servers);
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagedEntries = useStagedConfigStore((s) => s.entries);
  const interfaces = useNetworkInterfaces();

  /* 单例槽：编辑目标由**面板自查**（同桌面 `WarpDialog`），故 `MobileFormDesc` 那一支不带 id。 */
  const editNode = edit === true ? findWarpNode(servers) : undefined;
  const isEdit = editNode !== undefined;

  // R1 同步初始化。默认名与草稿缺省取自 `warp-spec`（两端共用，不在这里再写一份）。
  const [name, setName] = useState(editNode?.name ?? WARP_DEFAULT_NAME);
  const [plan, setPlan] = useState<WarpPlan>('free');
  const [license, setLicense] = useState('');
  const [draft, setDraft] = useState<FormValues>(() => warpDraftFromNode(editNode));
  const [dirty, setDirty] = useState(false);
  const [errName, setErrName] = useState(false);
  const [errLicense, setErrLicense] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [done, setDone] = useState(false);
  const [registration, setRegistration] = useState<WarpRegistrationAttempt | null>(null);
  const [openGroups, setOpenGroups] = useState<ReadonlySet<'advanced'>>(new Set());
  const scheduleReveal = useRevealAfterCommit();
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  const setField = (k: string, v: FormValue): void => {
    setDraft((d) => ({ ...d, [k]: v }));
    setDirty(true);
  };

  /**
   * 「重新注册 / 注销」那条腿。**复用节点屏那一份编排**（`mobile/nodes/node-deletion.ts`），
   * 本面板只提供它的四个出口：
   *  · `clearNotice` → 删除完成后清掉先前的失败；成功短 Toast 由共用删除腿发送。
   *  · `runWrite` → 一层 try/catch：**写调用本身住在 `node-deletion.ts` 里**，
   *    本文件不因此多出一处写腿。
   *  · `confirm` / `dismiss` → 表单栈的 `confirm` 那一支（**不是** `window.confirm`：
   *    它被 dialog 插件 init 覆写成 `plugin:dialog|confirm`，漏授 ACL 时整条腿抛 rejection）。
   *  · `exitBatch` → 本面板没有批选态，空实现。
   */
  const deletion = useMobileNodeDeletion({
    t,
    runWrite: async (op, describe) => {
      try {
        await op();
      } catch (e) {
        console.error('[mobile-warp-form] warp removal failed:', e);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: describe(e) });
      }
    },
    clearNotice: () => setNotice(undefined),
    confirm: (payload) => open({ kind: 'confirm', payload }),
    dismiss: closeInstance,
    exitBatch: () => {},
  });

  /** 重新注册：删掉现有那一台 + **删成之后**才开注册表（桌面 `onWarpReregister` 同形）。 */
  const reregister = (): void => {
    if (editNode === undefined) return;
    deletion.removeWarpNode(editNode, {
      title: t('nodes.meshWarpReRegisterTitle'),
      message: t('nodes.meshWarpReRegisterMsg'),
      okText: t('nodes.meshWarpReRegisterOk'),
      afterDelete: () => {
        /* 本面板的编辑目标刚被删掉 ⇒ 关掉自己，再开一张注册表。
           不原地切成注册态：那张表的草稿、`done` 收据与这张的生命周期绑在一起，
           原地切会让「刚注销的那台」的字段值留在新注册的草稿里。 */
        closeInstance(instanceId);
        open({ kind: 'warp', edit: false });
      },
    });
  };

  /** 注销：同一条腿，不开注册表。 */
  const deregister = (): void => {
    if (editNode === undefined) return;
    deletion.removeWarpNode(editNode, {
      title: t('nodes.meshWarpDeregisterTitle'),
      message: t('nodes.meshWarpDeregisterMsg'),
      okText: t('nodes.meshWarpDeregisterOk'),
      afterDelete: () => closeInstance(instanceId),
    });
  };

  const detourOpts = endpointDetourOptions(servers, editNode?.id, t('node.detourDirect'));
  const interfaceOpts: SelectOption[] = buildNetworkInterfaceChoices(
    interfaces.items,
    typeof draft.bindInterface === 'string' ? draft.bindInterface : '',
    {
      defaultLabel: t('node.bindInterfaceInherit'),
      unavailable: (value) => t('settings.network.interfaceUnavailable', { name: value }),
      down: t('settings.network.interfaceDown'),
    },
  ).map(({ value, label, disabled }) => [value, label, disabled] as SelectOption);
  const spec = warpAdvancedSpec(detourOpts, t('warp.endpointAuto'), interfaceOpts);

  const currentDetour = typeof draft.detour === 'string' ? draft.detour : DETOUR_NONE;
  const bindInterface = String(draft.bindInterface ?? '').trim();

  /**
   * 校验失败 ⇒ 掰开装着出错字段的那一组（这张表只有「高级」一组，端点/MTU/保活/前置代理/出口网卡
   * 五个控件全在里面）。与 `WgPanel#revealGroup` 同形，也同样**不**配 `scheduleReveal`：
   * 那条腿要的是分组的容器元素，而元素句柄只在 `FormGroup` 的 `onToggle` 里才拿得到 ——
   * 这里是提交路径，没有那次点击。错误文案落在脚上方的 `notice` 上，与被掰开的那一组同轴。
   */
  const revealAdvanced = (): void => {
    setOpenGroups(new Set(['advanced'] as const));
  };

  const requestClose = (): void => {
    if (!dirty || done || registration?.saved) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('warp.discardTitle'),
        message: t(registration ? 'warp.discardRegisteredMsg' : 'warp.discardMsg'),
        confirmLabel: t('warp.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  /**
   * 提交。**注册腿与编辑腿都写在这一个 `try` 里**，不抽成两个局部函数 ——
   * 那不是排版偏好：`write-failure-visibility.test.ts` ② 的判据是**词法**的
   * （写调用必须落在一个 `catch` 里真的调了 `setNotice` 的 `try` 块内）。抽出去之后
   * `api.server.registerWarp` / `add` / `applyWarpLicense` / `update` 四处会**逃出辖区**，
   * 而那道门要抓的正是「失败了但用户看不到任何东西」。判据是对的，是代码不该那么摆。
   */
  const submit = async (): Promise<void> => {
    if (done) {
      closeInstance(instanceId);
      return;
    }
    /* 三档拒绝走**共用**判据（`warp-spec#planWarpSubmit`），桌面消费的是同一个函数。 */
    const verdict = planWarpSubmit({
      name,
      plan,
      license,
      endpointRaw: String(draft.endpoint ?? ''),
      isEdit,
    });
    setErrName(verdict.reject === 'name');
    setErrLicense(verdict.reject === 'license');
    if (verdict.reject === 'name' || verdict.reject === 'license') return;
    if (verdict.reject === 'endpoint') {
      /* 出错的那个字段（端点）住在**默认收起**的「高级」组里 —— 只落一条 notice 的话，用户看到
         「端点格式应为 host:port」却看不到那个输入框。掰开它，与 `WgPanel#revealGroup` /
         `TsSettingsPanel` 的两条校验腿同形（2026-09-06 复审 minor）。 */
      revealAdvanced();
      setNotice({ tone: 'err', text: t('warp.errEndpoint') });
      return;
    }
    setSubmitting(true);
    setNotice(isEdit ? undefined : {
      tone: 'info', text: t(registration ? 'warp.registeredPending' : 'warp.registering'),
    });
    let attempt = registration;
    try {
      if (isEdit && editNode !== undefined) {
        /* ── 编辑腿 ─────────────────────────────────────────────────────────
           `block`（ENTITY_ACTION_TABLE）：`applyWarpLicense` 改的是远端账户等级、
           `update` 改的是一台**已注册**的设备 —— 盘上没有这个节点，两者都没有作用对象。 */
        const stagedOnly = stagedOnlyIds(servers, diskServers);
        const split = splitStagedOnly(
          'warp.edit',
          [editNode.id],
          stagedOnly,
          stagedEntries,
          'servers',
        );
        if (split.blocked.length > 0) {
          /* 「还不能做」不是错误：走 info，文案自述完整（同桌面 `WarpDialog#doEdit`）。 */
          setNotice({ tone: 'info', text: t('home.stagedOnlyBlocked') });
          return;
        }
        if (plan === 'plus' && license.trim()) {
          const r = await api.server.applyWarpLicense(editNode.id, license.trim());
          if (!r.ok) {
            /* `no-credentials` 那支文案自述完整；其余套 `warp.errLicense`
               （`r.error` 是后端原始串、不自述，也不外泄给用户）。 */
            setNotice({
              tone: 'err',
              text: r.error === 'no-credentials' ? t('warp.errNoCreds') : t('warp.errLicense'),
            });
            return;
          }
        }
        const next: ServerConfig = {
          ...editNode,
          name: name.trim(),
          address: verdict.endpoint!.host,
          port: verdict.endpoint!.port,
          wireguardSettings: buildWarpSettings(editNode.wireguardSettings ?? {}, draft),
        };
        applyDetour(next, currentDetour);
        if (bindInterface) next.bindInterface = bindInterface;
        else delete next.bindInterface;
        await api.server.update(next);
        await loadConfig(true);
        closeInstance(instanceId);
        toast.success(t('common.saved'));
        return;
      }

      /* ── 注册腿 ───────────────────────────────────────────────────────────
         🔴 单例闸**前置到 Cloudflare 请求之前**（见文件头 ①）：闸不过 ⇒ **零请求**，
         由 `registerWarpIfSlotFree` 自己弹全局 toast 说明槽位被谁占了。 */
      if (attempt === null) {
        const registered = await registerWarpIfSlotFree(
          servers,
          t,
          () => api.server.registerWarp(plan === 'plus' ? license.trim() : undefined),
          /* 闸不过时把**同一句**理由也落到面板内（文案仍只有 `meshSingletonMessage` 一份）。
             全局 toast 那条通道仍在，但不把可见性单押在它身上：这张表是个全屏层，toast 宿主贴在
             它下面的停靠区上沿（层叠已修，见 `mobile.css#.m-toast-host`）。 */
          (message) => setNotice({ tone: 'err', text: message }),
        );
        if (registered === null) {
          /* 理由已由上面那条回调写进 notice（同时也在 toast 上），「正在注册」那句被它顶掉。 */
          return;
        }
        attempt = { registered, id: crypto.randomUUID(), saved: false };
        setRegistration(attempt);
        setDirty(true);
        setNotice({ tone: 'info', text: t('warp.registeredPending') });
      }
      const retainedAttempt = attempt;
      await saveRegisteredWarp(
        retainedAttempt,
        () => buildRegisteredWarpServer(retainedAttempt, name, verdict.endpoint, draft, currentDetour, bindInterface),
        (server) => api.server.add(server),
        () => loadConfig(true),
        () => useAppStore.getState().servers,
      );
      /* 成功不立刻关面板：注册产物是一台**远端**设备，用户需要一个明确的收据（见文件头 ③）。 */
      if (hasInstance(instanceId)) {
        setDone(true);
        setNotice(undefined);
      }
    } catch (e) {
      /* 只吐错误对象：`draft` 里有许可证、`server` 里有私钥，两者都不许进日志。 */
      console.error('[mobile-warp-form] save failed:', e);
      setNotice({ tone: 'err', text: t(attempt
        ? attempt.saved ? 'warp.registeredRefreshFailed' : 'warp.registeredSaveFailed'
        : 'common.saveFailed') });
    } finally {
      if (hasInstance(instanceId)) setSubmitting(false);
    }
  };

  return (
    <FormSheet
      title={isEdit ? t('warp.editTitle') : t('warp.addTitle')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={done ? t('common.done') : isEdit ? t('common.save') : registration ? t('common.retry') : t('warp.register')}
      submitDisabled={submitting}
      onSubmit={() => void submit()}
      notice={notice}
    >
      {done ? (
        <div className="m-form-row">
          <b>{t('warp.doneTitle')}</b>
          <p className="m-form-hint">{t('warp.doneSub')}</p>
        </div>
      ) : (
        <>
          {!isEdit && <p className="m-form-hint">{t('warp.note')}</p>}

          {registration && <p className="m-form-hint">{t('warp.registeredLocked')}</p>}
          <fieldset disabled={submitting || registration !== null} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>

          <div className="m-form-row">
            <label className="m-form-label" htmlFor="mwarp-name">
              {t('warp.name')}
              <span className="m-form-req" aria-hidden>
                *
              </span>
            </label>
            <input
              id="mwarp-name"
              className="m-form-input"
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                setErrName(false);
                setDirty(true);
              }}
            />
            {errName && <p className="m-form-err">{t('warp.errName')}</p>}
          </div>

          <div className="m-form-row">
            <span className="m-form-label" id="mwarp-plan">
              {t('warp.plan')}
            </span>
            <div className="m-form-seg" role="group" aria-labelledby="mwarp-plan">
              <button
                type="button"
                className={plan === 'free' ? 'on' : ''}
                aria-pressed={plan === 'free'}
                onClick={() => {
                  setPlan('free');
                  setErrLicense(false);
                  setDirty(true);
                }}
              >
                {t('warp.planFree')}
              </button>
              <button
                type="button"
                className={plan === 'plus' ? 'on' : ''}
                aria-pressed={plan === 'plus'}
                onClick={() => {
                  setPlan('plus');
                  setDirty(true);
                }}
              >
                WARP+
              </button>
            </div>
          </div>

          {plan === 'plus' && (
            <div className="m-form-row">
              <label className="m-form-label" htmlFor="mwarp-license">
                {t('warp.licenseLabel')}
              </label>
              {/* 许可证是**凭据**：与私钥同一条纪律，输入框遮住，显隐由 44px 触控目标那颗键控制。
                  §4.12：桌面挂在 `InfoIcon` 上的那句说明在这里常驻（触屏没有 hover）。 */}
              <input
                id="mwarp-license"
                type="password"
                className="m-form-input mono"
                value={license}
                placeholder="xxxxxxxx-xxxxxxxx-xxxxxxxx"
                onChange={(e) => {
                  setLicense(e.target.value);
                  setErrLicense(false);
                  setDirty(true);
                }}
              />
              <p className="m-form-hint">{t('warp.licenseHint')}</p>
              {errLicense && <p className="m-form-err">{t('warp.errLicense2')}</p>}
            </div>
          )}

          <FormGroup
            title={t('node.formGroup.advanced')}
            open={openGroups.has('advanced')}
            onToggle={(section) => {
              const willOpen = !openGroups.has('advanced');
              setOpenGroups(willOpen ? new Set(['advanced'] as const) : new Set());
              /* 只在**展开**时滚（折叠只会让内容变短，滚动等于凭空把用户挪走）。 */
              scheduleReveal(willOpen && section !== null ? () => revealElement(section) : null);
            }}
          >
            <MobileFields fields={spec} values={draft} onChange={setField} t={t} />
          </FormGroup>
          </fieldset>

          {/* 设备级动作（桌面摆在组网接入面的 WARP 卡片上，见文件头注）。只在编辑态出现：
              注册态没有作用对象。两颗都是破坏性的 ⇒ 各自叠一层确认面板。 */}
          {isEdit && editNode !== undefined && (
            <div className="m-form-row">
              <span className="m-form-label">{t('meshJoin.managed')}</span>
              <div className="m-form-inline">
                <button
                  type="button"
                  className="m-form-btn"
                  disabled={submitting}
                  onClick={reregister}
                >
                  {t('meshJoin.reregister')}
                </button>
                <button
                  type="button"
                  className="m-form-btn danger"
                  disabled={submitting}
                  onClick={deregister}
                >
                  {t('meshJoin.deregister')}
                </button>
              </div>
            </div>
          )}
        </>
      )}
    </FormSheet>
  );
}
