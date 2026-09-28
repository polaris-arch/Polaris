/**
 * 移动端**隐私锁**的全部非呈现逻辑（W-16）。
 *
 * # 它补的缺口
 *
 * 到本批为止，移动端的「自动隐私锁」是**一句缺席说明**：`GeneralPage` 上写着「闲置计时与锁屏遮罩
 * 这两条腿还没接」。桌面那两条腿分别住在 `App.tsx`（`useIdlePrivacyLock`）与 `AppShell.tsx`
 * （`<LockOverlay/>`），移动入口一条都够不着 ⇒ 在手机上把开关拨开，配置写进去了，**没有任何东西会锁**。
 * 本文件 + [`MobileLockOverlay`] + `app-wiring.ts` 那条 effect 是那两条腿的移动端对位物。
 *
 * # 🔴 解锁必须走 `privacy_unlock`，不许就地翻标志位（IA §4.11 / 裁定 #10）
 *
 * 后端 `privacy_unlock`（`commands/config.rs:1410`）做三件本层做不到的事：常量时间比对哈希、
 * 失败 300ms 弱限速（`apply_unlock_rate_limit`）、legacy SHA-256 的透明升级。绕过它直接
 * `setPrivacyMode(false)` 的话，遮罩会照常消失 —— 界面看不出任何差别，而**密码从此不再被校验**。
 * 这正是「安全承诺被静默掏空」的形态，故：
 *  · **退出**隐私态（`setPrivacyMode(false)`）全仓只有 [`submitPrivacyUnlock`] 里那一处，且在
 *    `verdict.ok` 之后 —— 判据 D 组正面对拍「设置目录一处 `setPrivacyMode` 都没有」。
 *    **进入**隐私态有两个触发点（闲置计时到点、冷启动补锁），两条都在 `app-wiring.ts` 里、都只
 *    `setPrivacyMode(true)`；进隐私态不经校验本就不需要校验，故它们不构成绕行路；
 *  · [`submitPrivacyUnlock`] 的顺序是**先校验、后翻转**，`ok:false` 直接早退，绝不落到翻转那一步；
 *  · 判据（`privacy-lock.test.tsx` B 组）拿假 IPC 直接驱动它，并对「换成就地 `setPrivacyMode(false)`」
 *    这个变异做过红绿对照。
 *
 * # 为什么翻转走 `config_set_privacy_mode` 而不是直接写 store
 *
 * 逐字同桌面 `LockOverlay` 的既定收敛范式：`privacy_unlock` 只返 `{ok}`，**不翻 `PRIVACY_MODE`、
 * 也不 emit**（`config.rs:1410-1435` 实证）。真正退出隐私态的是 `config_set_privacy_mode(false)`，
 * 它在状态跃迁时 emit `exitPrivacyMode`，由 `app-wiring.ts` 那条订阅收敛进 store ⇒ 遮罩卸载。
 * 在这里手改 store 会多出第二个收敛点，而后端才是那个 atomic 的所有者（跨 WebView 重建存活）。
 *
 * # 为什么闲置计时抽成一个吃「事件靶子」的普通函数
 *
 * 与 `MobileSettingsScreen.watchVpnAuth` 同一条理由（那份头注写得更细）：本仓 vitest 跑
 * `environment:'node'`、没有 jsdom，`renderToStaticMarkup` 不跑 effect ⇒ 计时逻辑若留在 hook 体里，
 * 「把 arm 整段删掉」这个变异**没有任何门守得住**。抽出来之后判据能喂一个假靶子 + 假计时器直接驱动它。
 */

import { api } from '@/ipc';
import { IDLE_PRIVACY_LOCK_MS, resolveUnlockAttempt } from '@/domain/privacy';

/**
 * 闲置计时监听的用户活动事件。
 *
 * 比桌面那份（`lib/use-idle-privacy-lock.ts`）少了 `mousemove` / `wheel` —— 触屏上这两条要么不发、
 * 要么只在接了鼠标的平板上零星发；多留两个恒不触发的监听不产生任何能力。`scroll` 保留：
 * 滚动是移动端最常见的「人在看」信号，而它**不一定**伴随 `touchstart`（惯性滚动期间没有触点）；
 * 接了外接鼠标/触控板只用滚轮浏览的那一档，它更是唯一的活动信号（`wheel` 已被去掉）。
 *
 * 🔴 **`scroll` 必须捕获相才收得到**（2026-09-06 复审 minor，已修）。本仓滚动的是内层
 * `div.m-scroll{overflow-y:auto}`（`mobile.css:150`），**不是** document —— 而 `scroll` 事件在
 * Element 上派发时**不冒泡**（只有 Document 那一发会投到 `window`）。此前这个监听挂在 `window`
 * 的冒泡相上 ⇒ 用户在应用里滚多久都一次不触发，五个事件里实际只有四个是活的。
 * 见 [`armIdlePrivacyLock`] 里那次 `capture: true` 与判据 A 组那条捕获相断言。
 */
export const MOBILE_IDLE_ACTIVITY_EVENTS = [
  'touchstart',
  'touchmove',
  'pointerdown',
  'keydown',
  'scroll',
] as const;

/**
 * [`armIdlePrivacyLock`] 需要的最小事件靶子。生产传 `window`；判据传一个假的。
 *
 * `options` 两侧都带 `capture`：**注册与拆卸的 capture 必须相等**，否则 `removeEventListener`
 * 认不出那条监听（DOM 规范按 `(type, callback, capture)` 三元组匹配），拆卸会静默失败 ——
 * 表现是切一次开关就多叠一条永不清除的监听。
 */
export interface IdleActivityTarget {
  addEventListener(
    type: string,
    listener: () => void,
    options?: { passive?: boolean; capture?: boolean },
  ): void;
  removeEventListener(type: string, listener: () => void, options?: { capture?: boolean }): void;
}

/** [`armIdlePrivacyLock`] 需要的最小计时器（判据用假计时器把 10 分钟压成一次手动触发）。 */
export interface IdleTimers {
  setTimeout(handler: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/**
 * 武装闲置计时：`delayMs` 内没有任何用户活动 ⇒ 调 `lock()`；返回拆卸闭包。
 *
 * **挂载即起计时**（不是「等第一次活动之后才开始」）：用户可能刚进来就把手机放下。
 * 每次活动重置计时，与桌面同构。
 *
 * ⚠️ 调用方负责判「该不该武装」（`shouldArmIdleLock`）—— 已锁时再武装等于重复触发 `setPrivacyMode(true)`。
 * 那条判定住在 `@/domain/privacy`，两个平台读同一份。
 */
export function armIdlePrivacyLock(
  target: IdleActivityTarget,
  lock: () => void,
  delayMs: number = IDLE_PRIVACY_LOCK_MS,
  timers: IdleTimers = globalThis,
): () => void {
  let handle: unknown;
  const arm = (): void => {
    timers.clearTimeout(handle);
    handle = timers.setTimeout(lock, delayMs);
  };
  for (const event of MOBILE_IDLE_ACTIVITY_EVENTS) {
    /* 🔴 `capture: true` 是 `scroll` 那一条的**成因**（见 `MOBILE_IDLE_ACTIVITY_EVENTS` 头注）：
       元素滚动不冒泡，冒泡相的 window 监听一次都收不到。捕获相从根往下走，收得到。
       其余四条事件本来就冒泡，捕获相对它们只是提前一相到达，不改变语义。 */
    target.addEventListener(event, arm, { passive: true, capture: true });
  }
  arm();
  return () => {
    timers.clearTimeout(handle);
    /* 拆卸的 capture 必须与注册相等（见 [`IdleActivityTarget`] 头注）。 */
    for (const event of MOBILE_IDLE_ACTIVITY_EVENTS) {
      target.removeEventListener(event, arm, { capture: true });
    }
  };
}

/* ── 冷启动补锁（W-16 复审 blocker）───────────────────────────────────────────── */

/**
 * 本进程做没做过冷启动补锁。模块级而不是组件级 `useRef`：判的是「本进程一次」，
 * 而 React 的 ref 随组件卸载一起死（同一条根因见 `settings/app-update-check.ts` 那把闸）。
 */
let coldStartLockResolved = false;

/** 判据用：把「本进程已做过冷启动补锁」这一格复位（模块态跨用例存活）。 */
export function resetColdStartPrivacyLock(): void {
  coldStartLockResolved = false;
}

/**
 * 冷启动补锁的**判定**：后端说没锁、但已设密码 ⇒ 这一拍该补一次锁（返 `true`）。
 *
 * ⚠️ **它自己不写**。翻转那一跳留在 `app-wiring.ts` 的 `reportIfFails(` 里，与闲置计时那条同一个
 * 出口 —— 「没锁上」是一条用户会照着它行动的安全承诺，失败必须落成全局 toast
 * （`write-failure-visibility.test.ts` 按这条口径把本文件的写腿全部收在 `showOutcome` 辖区内，
 * 一条不带回执的 `setPrivacyMode` 写在这里会被那道门当场判红 —— 那正是它该守的东西）。
 *
 * 「本进程一次」这把闸由**本函数**消费（不是调用方）：判定与闸门分开会让下一个人在别处再调一次。
 *
 * # 它补的是一条真实的绕行路（2026-09-06 复审 blocker）
 *
 * 锁定态的真值是 Rust 进程内的一个 `AtomicBool`（`commands/config.rs:783`，源码里写明「随重启复位」），
 * 全仓没有任何落盘/恢复腿。**桌面上这是合理的**：那里的「重启」是用户显式动作。
 * Android 上不是 —— 应用长期在后台，系统回收**整个进程**是日常且非自愿的，而闲置计时也随进程一起死。
 * 于是原本的形态是：锁上 → 放着 → 系统回收进程 → 用户再打开 → `getPrivacyMode()` 返 false
 * ⇒ **不输密码直接进全部界面**，正是 `autoPrivacyModeDesc`（「用密码锁定界面防偷窥」）要挡的场景。
 * 连带失效的还有 `privacy_set_password` 的锁屏闸（`config.rs:1372` 读的是同一个 flag）——
 * 那一档任何人都能把密码保存成空串（= 删掉这把锁）。
 *
 * # 为什么是「冷启动就锁」，不是「把锁定态存下来」
 *
 * 存标记只能恢复「死之前锁着」这一档，而**进程死在闲置计时跑完之前**的那一档它救不了：
 * 没有活着的进程，那个 10 分钟的计时永远不会到点，标记永远是「未锁」。
 * 「已武装 + 已设密码 ⇒ 冷启动先锁上」把两档一起关掉，且不引入第二个持久真值。
 * 代价是每次冷启动要输一次密码 —— 这正是这个开关承诺的东西。
 *
 * # 三条早退各自的理由
 *
 *  · **本进程已经做过** ⇒ 不重做（StrictMode 下 effect 跑两遍；且这是「冷启动」不是「每次渲染」）；
 *  · **后端说锁着** ⇒ 什么都不做：水合那一拍已经把遮罩挂上了，再调一次 `setPrivacyMode(true)`
 *    在同值时不 emit，纯浪费；
 *  · **没设密码** ⇒ 不锁：那样得到的是一个「随便敲一个字就过」的遮罩（后端 `unlock_core` 对空
 *    哈希恒放行），只有摩擦没有保护。读不到密码状态时 [`readPrivacyHasPassword`] 返 `true`
 *    （保守方向，见它的头注）—— 这一档宁可多锁一次。
 */
export async function shouldLockAfterColdStart(): Promise<boolean> {
  if (coldStartLockResolved) return false;
  coldStartLockResolved = true;
  /* 读不到后端状态时按「没锁」处理，让下面两道闸继续判 —— 反过来（当成已锁）会在这一档
     什么都不做，而这一档正是要补锁的那一档。 */
  const backendLocked = await api.config.getPrivacyMode().catch((err: unknown) => {
    console.error('[mobile] 冷启动读隐私态失败，按「未锁」继续判:', err);
    return false;
  });
  if (backendLocked) return false;
  return await readPrivacyHasPassword();
}

/**
 * 一次解锁尝试的结局。四支各有各的界面呈现，**不许折叠**：
 *  · `require-input` —— 设了密码却没输入。本地判定，不打后端（同桌面 `resolveUnlockAttempt`）。
 *  · `rejected` —— 后端说密码不对（`{ok:false}`）。
 *  · `failed` —— 这次调用本身没走通（IPC 断了 / 后端报错）。**与 `rejected` 分开**：
 *    把「没问成」显示成「密码错」会让用户一遍遍重打一个其实正确的密码。
 *  · `unlocked` —— 校验通过且隐私态已翻回去。
 */
export type UnlockOutcome = 'require-input' | 'unlocked' | 'rejected' | 'failed';

/**
 * 读「有没有设过锁屏密码」。
 *
 * 决定的是**空密码放不放行**：未设密码时后端 `unlock_core` 对空 hash 恒返 `ok:true`（自由解锁），
 * 此时不该拦在本地的「请输入密码」上。读不到一律当**已设密码**（保守方向）——
 * 反过来会在读取失败时放行一次空密码解锁。
 */
export async function readPrivacyHasPassword(): Promise<boolean> {
  try {
    return await api.privacy.hasPassword();
  } catch (err) {
    console.error('[mobile] 读锁屏密码状态失败，按「已设密码」处理:', err);
    return true;
  }
}

/**
 * 提交一次解锁。**移动端唯一的解锁路径。**
 *
 * `showOutcome` 是这一层的用户可见通道（遮罩上那行错误 / 遮罩卸载），
 * 故 `write-failure-visibility.test.ts` 把本文件按 `reporter` 形态登记在它名下：
 * 两条写腿都在 `try` 里，`catch` 真的把失败送进它。
 */
export async function submitPrivacyUnlock(
  hasPassword: boolean,
  input: string,
  showOutcome: (outcome: UnlockOutcome) => void,
): Promise<void> {
  if (resolveUnlockAttempt(hasPassword, input) === 'require-input') {
    showOutcome('require-input');
    return;
  }
  try {
    /* 🔴 第一步永远是校验。`ok:false` 在这里早退 —— 下面那一跳（真正退出隐私态）不许被绕过去。 */
    const verdict = await api.privacy.unlock(input);
    if (!verdict.ok) {
      showOutcome('rejected');
      return;
    }
    /* 校验通过才翻转。翻转由后端 emit 收敛回 store（见文件头注），本层不碰 store。 */
    await api.config.setPrivacyMode(false);
    showOutcome('unlocked');
  } catch (err) {
    console.error('[mobile] 隐私解锁失败:', err);
    showOutcome('failed');
  }
}

/** 解锁结局 → 遮罩上那行提示的 i18n 键；`null` = 这一支不该显示任何错误。 */
export function unlockOutcomeMessageKey(outcome: UnlockOutcome): string | null {
  switch (outcome) {
    case 'require-input':
      return 'privacy.enterPassword';
    case 'rejected':
      return 'privacy.wrongPassword';
    case 'failed':
      /* 「没问成」不是「密码错」：单独一句，绝不与 `wrongPassword` 共用 —— 共用会让用户
         一遍遍重打一个其实正确的密码。 */
      return 'privacy.unlockFailed';
    case 'unlocked':
      return null;
  }
}
