/**
 * 「跳到系统 VPN 设置页」这条腿的 web 侧（W-17）。原生侧是 `MainActivity.kt` 的
 * `installVpnSettingsBridge`。
 *
 * # 为什么不能走 `shell_open_external`（先核清了再写，不猜 intent 串）
 *
 * `systemApi.openExternal` → Rust `shell_open_external`（`commands/misc/logs.rs:599`）→
 * `app.shell().open(url)`。在移动端这条链的终点是 tauri-plugin-shell 的 Kotlin 半边
 * （`tauri-plugin-shell-2.3.5/android/src/main/java/ShellPlugin.kt:19-29`）：
 *
 * ```kotlin
 * val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
 * ```
 *
 * 它**只会**发 `ACTION_VIEW` + 一个 URI。而 Android 的设置页是按 **action** 打开的：
 * `Settings.ACTION_VPN_SETTINGS`，取值 `"android.settings.VPN_SETTINGS"`
 * （实测取自 `~/Android/Sdk/platforms/android-34/android.jar` 里 `android/provider/Settings.class`
 * 的常量池，不是从文档抄的）。系统设置那个 Activity 的 intent-filter 匹配的是这个 action +
 * `CATEGORY_DEFAULT`，`ACTION_VIEW` 匹配不上 ⇒ 走 `openExternal` 的结果是
 * `ActivityNotFoundException`，不是「跳过去了」。
 *
 * 同一次核对还确认了**深度的上限**：`android.jar` 的 `Settings` 里没有任何
 * always-on VPN 的 action 常量（`strings android/provider/Settings.class | grep -i always` 为空），
 * 公开面能到的最深处就是 VPN 列表页。故这条腿把用户送到列表页，**剩下那两跳仍然由
 * `bootConnectPath` 那行常驻路径承担** —— 它不是「跳转没做完」的补丁，是公开 API 的边界。
 *
 * # 为什么是 `addWebMessageListener` 桥，不是新加一条 Tauri command
 *
 * 仓里已有两条同型的 web → 原生桥（`polarisStatusBar` 主题上报、`polarisBackNav` 返回键出口），
 * 用的都是 `WebViewCompat.addWebMessageListener`。走它：
 *  · 一行 Rust 都不用动（不新增 command、不动 `IPC_CHANNELS`、不动 ACL 能力面）；
 *  · 不进 `check-android-bridge.mjs` 的 `@Command` 双向对拍面（那道门守的是 Rust ⇄ Kotlin 的
 *    插件命令契约，这条腿两端都不在那条链上）；
 *  · 与既有两条桥同一套失效面（名字两侧漂了 ⇒ `postMessage` 打在 `undefined` 上），
 *    而那条失效面已经有门守着 —— 本模块的字面量由 `MobileSettings.test.tsx` ⑤ 组与 Kotlin 侧逐字对拍。
 *
 * # 为什么要回执（`polarisBackNav` 那条刻意不要）
 *
 * 返回键那条桥只有一个语义、且失败时用户还有别的出路；这条不一样：`startActivity` 会因为
 * 设备没有 VPN 设置 Activity（改过的 ROM / TV 形态）真的抛异常，而一个点了没反应的跳转项
 * 与一个拨了不生效的开关是同一类缺陷（裁定 #14）。故原生侧把结果 `postMessage` 回来，
 * 本模块折成一个会 reject 的 promise，由设置页的 `commit` 落成那一行红字。
 *
 * **不设超时**：回执是原生侧在同一个回调里同步发出的（`startActivity` 立刻返回），
 * 而超时的代价是「跳成功了、WebView 恰好被系统暂停一下」也会报一句假错误。
 * 「桥在但从不回执」这一档结构上不成立（两个分支各发一条），故不为它加机制。
 */

/**
 * 原生注入到 `window` 上的对象名。**与 `MainActivity.kt` 的 `VPN_SETTINGS_BRIDGE` 逐字相同**，
 * 两侧由 `MobileSettings.test.tsx` ⑤ 组对拍：一侧改名而另一侧没改会让 `postMessage` 打在 `undefined` 上，
 * 表现是「点了没反应」且运行期零报错。
 */
export const VPN_SETTINGS_BRIDGE = 'polarisVpnSettings';

/** 桥上唯一的请求载荷。原生侧不解析内容，留一个字面量是为了两侧对拍时有得比。 */
export const VPN_SETTINGS_OPEN = 'open';

/** 原生侧的回执：跳成功。 */
export const VPN_SETTINGS_OK = 'ok';

/** 原生侧的回执：`startActivity` 抛了（设备上没有能处理这个 action 的 Activity）。 */
export const VPN_SETTINGS_FAILED = 'failed';

/** 回执通道的最小形状（`JsReplyProxy` 那一侧发来的消息经 `onmessage` 到达）。 */
interface VpnSettingsBridge {
  postMessage: (message: string) => void;
  onmessage: ((event: { data?: unknown }) => void) | null;
}

/**
 * 取注入对象。用 `Reflect.get` 而不是 `(globalThis as unknown as Record<…>)[…]`：
 * 后者是**双重断言**，而本屏有一道门明令禁止它（`MobileSettings.test.tsx` ⑯ 的 K5-RV07，
 * 起因是 `as unknown as` 会把两侧的类型关系整个切断）。这里只有一次断言，且断的是
 * 「这个全局槽位上放的是那个桥」——那正是原生侧的契约，两侧由 `MainActivity.kt` 逐字对拍。
 */
function bridge(): VpnSettingsBridge | undefined {
  return Reflect.get(globalThis, VPN_SETTINGS_BRIDGE) as VpnSettingsBridge | undefined;
}

/**
 * 收信口在不在。**这条腿要不要画成可点的行，由它决定**（同 `MobileApp.hasBackNavBridge` 的口径）：
 * 桌面调试档 / 浏览器直开 / WebView < 88 上桥不存在，此时画一个点了必然失败的跳转项
 * 比不画更坏。桥不在时那一行退化成一句说明 + 常驻路径 —— 与本批之前的形态相同。
 */
export function hasVpnSettingsBridge(): boolean {
  return bridge() !== undefined;
}

/**
 * 请原生打开系统 VPN 设置页。
 *
 * 桥不在 ⇒ 立刻 reject（调用方本不该走到这里，见 [`hasVpnSettingsBridge`]；但一个「桥不在就
 * 静默成功」的实现会让那条早退失效时无声无息）。
 */
export function openSystemVpnSettings(): Promise<void> {
  return new Promise((resolve, reject) => {
    const target = bridge();
    if (target === undefined) {
      reject(new Error(`${VPN_SETTINGS_BRIDGE} bridge is absent`));
      return;
    }
    target.onmessage = (event) => {
      target.onmessage = null;
      if (event.data === VPN_SETTINGS_OK) resolve();
      else reject(new Error(String(event.data ?? VPN_SETTINGS_FAILED)));
    };
    target.postMessage(VPN_SETTINGS_OPEN);
  });
}
