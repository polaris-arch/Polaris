/**
 * 「隐私锁开着时禁掉系统快照/截图」这条腿的 web 侧（W-16 复审 major）。
 * 原生侧是 `MainActivity.kt` 的 `installSecureScreenBridge`。
 *
 * # 它补的是遮罩**结构上挡不住**的那一格
 *
 * 自动隐私锁的典型发生时刻是**应用已经在后台**（闲置 10 分钟 = 用户把手机放下了）。而 Android 的
 * 任务快照是在 Activity 停止那一刻拍的、之后不再刷新 —— 锁上时那张缩略图早就拍完了。于是
 * `MobileLockOverlay` 虽然在（不可见的）WebView 里挂上了，最近任务里仍是**锁定前**的那一屏：
 * 订阅名、节点名、出口国旗、流量数字全都可读，全程不需要密码。
 * 遮罩在这一格上帮不了忙 —— 快照根本早于遮罩存在，这不是「遮罩挡不挡得住截图」的问题。
 *
 * `FLAG_SECURE` 是系统级的解法：置上之后系统不再为本窗口留可读快照，也禁掉截图。
 *
 * # 🔴 跟着**开关**走，不跟着锁定态走
 *
 * 跟着锁定态走 = 「锁上的那一刻才置 flag」，而那一刻快照早已拍完、置了也不会重拍。必须在进后台
 * **之前**就置上，故真值取 `autoPrivacyMode`（用户有没有开这把锁），不是 `privacyMode`（现在锁着吗）。
 *
 * # 为什么两个方向都要有
 *
 * `FLAG_SECURE` 连用户自己的截图一起禁掉。没开隐私锁的用户没有理由被剥夺截图，故拨回去时必须
 * 清掉它 —— 少了 `off` 那一半的表现是「关掉隐私锁之后截图永久不能用」。两条腿由
 * `privacy-lock.test.tsx` F 组的跨语言门逐条断言（把 `clearFlags` 那半删掉 ⇒ 必须红）。
 *
 * # 为什么是 `addWebMessageListener` 桥，不是新加一条 Tauri command
 *
 * 与 `settings/system-settings-bridge.ts` 同一条理由（那份头注写得更细）：仓里已有三条同型的
 * web → 原生桥，走它一行 Rust 都不用动、不进 `check-android-bridge.mjs` 的 `@Command` 对拍面，
 * 失效面（两侧名字漂了 ⇒ `postMessage` 打在 `undefined` 上）已经有门守着。
 *
 * # 为什么不要回执
 *
 * `Window.addFlags` / `clearFlags` 没有失败这一支（不像 `startActivity` 会抛
 * `ActivityNotFoundException`），一个恒成功的回执只是噪声。桥**不在**这一档由
 * [`applySecureScreen`] 的返回值如实报告，调用方据此落日志。
 */

/**
 * 原生注入到 `window` 上的对象名。**与 `MainActivity.kt` 的 `SECURE_SCREEN_BRIDGE` 逐字相同**，
 * 两侧由 `privacy-lock.test.tsx` F 组对拍：一侧改名而另一侧没改会让 `postMessage` 打在 `undefined` 上，
 * 表现是「隐私锁开着，最近任务里照样看得见上一屏」且运行期零报错。
 */
export const SECURE_SCREEN_BRIDGE = 'polarisSecureScreen';

/** 载荷：置上 `FLAG_SECURE`。与 Kotlin 侧 `SECURE_SCREEN_ON` 逐字相同。 */
export const SECURE_SCREEN_ON = 'on';

/** 载荷：清掉 `FLAG_SECURE`。与 Kotlin 侧 `SECURE_SCREEN_OFF` 逐字相同。 */
export const SECURE_SCREEN_OFF = 'off';

/** 桥的最小形状（只发不收，故没有 `onmessage`）。 */
interface SecureScreenBridge {
  postMessage: (message: string) => void;
}

/**
 * 取注入对象。`Reflect.get` 而不是双重断言，理由同 `settings/system-settings-bridge.ts`
 * （⑯ 组的 K5-RV07 明令禁止 `as unknown as`）。
 */
function bridge(): SecureScreenBridge | undefined {
  return Reflect.get(globalThis, SECURE_SCREEN_BRIDGE) as SecureScreenBridge | undefined;
}

/** 收信口在不在（桌面调试档 / 浏览器直开 / WebView < 88 上不存在）。 */
export function hasSecureScreenBridge(): boolean {
  return bridge() !== undefined;
}

/**
 * 把 `FLAG_SECURE` 的目标状态投给原生。返回**有没有真的投递出去** —— 桥不在时返 `false`
 * 而不是静默成功：一个「桥不在也算办到了」的实现会让这条安全腿失效时无声无息。
 */
export function applySecureScreen(on: boolean): boolean {
  const target = bridge();
  if (target === undefined) return false;
  target.postMessage(on ? SECURE_SCREEN_ON : SECURE_SCREEN_OFF);
  return true;
}
