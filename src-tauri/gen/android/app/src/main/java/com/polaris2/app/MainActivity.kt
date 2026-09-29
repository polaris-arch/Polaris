package com.polaris2.app

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import android.provider.Settings
import android.util.Log
import android.view.View
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.webkit.ScriptHandler
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import java.util.Locale

/**
 * web 层往原生投递生效主题用的 JS 对象名（`addWebMessageListener` 注入到 `window` 上）。
 *
 * 探针载荷 [THEME_PROBE_JS] **由本常量拼出**，不另写一份字面量：两处各写一份的话，一侧改名而
 * 另一侧没改会让 `postMessage` 打在 `undefined` 上 —— 状态栏从此停在最后一个已知值，
 * 运行期不报任何错。这里让编译期把它折成一份，那种失配结构上就不成立。
 */
private const val THEME_BRIDGE = "polarisStatusBar"

/**
 * web 层用来「把应用交还系统」的 JS 对象名（同样经 `addWebMessageListener` 注入到 `window` 上）。
 *
 * # 为什么非有它不可
 *
 * 前端一旦注册了 tauri `AppPlugin` 的 `back-button` 监听，那个插件的返回回调就**只 trigger 事件**，
 * 两条原生腿（`webView.goBack()` / `activity.onBackPressed()`）一条都不走
 * （`tauri-2.11.5/.../AppPlugin.kt:28-46`）⇒ **原生兜底整条失效**。
 * 根页那一跳因此必须由 JS 自己兑现，否则表现是「按返回完全没反应」，比接线之前的「直接退出」更糟。
 *
 * # 为什么不是 `invoke('plugin:app|exit')`
 *
 * 那个 `@Command fun exit` 在 Kotlin 侧确实存在，但 `exit` **不在** `tauri-2.11.5/build.rs` 的
 * `core:app` 命令表里 ⇒ 根本不存在 `allow-exit` 这个权限标识符，ACL 闸会直接拒掉。
 * 而 `register_listener` / `remove_listener` 在表里且标 `true`（进 default 集合），
 * 本仓 `capabilities/default.json` 已授 `core:app:default` ⇒ 监听那条腿零新增权限。
 *
 * # 为什么 `moveTaskToBack(true)` 而不是 `finish()`
 *
 * VPN 客户端按返回的语义是「收起界面」，不是「结束会话」：任务栈与 VPN 前台服务都该留着，
 * 用户再进来时看到的是原来那一屏。`finish()` 会让下一次进入重走冷启动。
 *
 * 名字与载荷的对端是 `ui/src/mobile/MobileApp.tsx` 的 `BACK_NAV_BRIDGE` / `BACK_NAV_EXIT`，
 * 门按字面对拍两侧 —— 一侧改名而另一侧没改会让 `postMessage` 打在 `undefined` 上，
 * 表现是根页按返回**静默无反应**、运行期零报错。
 */
private const val BACK_NAV_BRIDGE = "polarisBackNav"

/**
 * web 层用来「跳到系统 VPN 设置页」的 JS 对象名（同样经 `addWebMessageListener` 注入到 `window` 上）。
 *
 * # 为什么必须是原生这一侧
 *
 * web 侧手上唯一能开外部东西的腿是 `shell_open_external` → tauri-plugin-shell 的 `ShellPlugin.open`，
 * 而那个方法体只发 `Intent(Intent.ACTION_VIEW, Uri.parse(url))`
 * （`tauri-plugin-shell-2.3.5/android/src/main/java/ShellPlugin.kt:22`）。Android 的设置页按
 * **action** 打开、匹配的是 `CATEGORY_DEFAULT` + 那个 action 的 intent-filter，`ACTION_VIEW`
 * 匹配不上 ⇒ 从 web 侧发过去只会得到 `ActivityNotFoundException`。故这条腿只能由原生发 intent。
 *
 * # 深度到 VPN 列表页为止，这是公开面的上限、不是没做完
 *
 * `android.provider.Settings` 里**没有**任何 always-on VPN 的 action 常量（`ACTION_VPN_SETTINGS`
 * 是最深的一格；实测取自 android-34 的 `android.jar` 常量池）。「Polaris → 始终开启的 VPN」
 * 那两跳仍由界面上那行常驻路径（`mobileSettings.general.bootConnectPath`）承担。
 *
 * # 为什么这条桥要回执，而 [BACK_NAV_BRIDGE] 不要
 *
 * `startActivity` 会因为设备上没有这个 Activity（改过的 ROM / TV 形态）真的抛异常。
 * 一个点了没反应的跳转项与一个拨了不生效的开关是同一类缺陷，故两个分支各发一条回执，
 * 由 web 侧折成设置页那一行红字。回执发在同一个回调里（`startActivity` 立刻返回），
 * 故 web 侧不需要超时。
 */
private const val VPN_SETTINGS_BRIDGE = "polarisVpnSettings"

/** 回执：跳成功。与 `ui/src/mobile/settings/system-settings-bridge.ts` 的 `VPN_SETTINGS_OK` 逐字相同。 */
private const val VPN_SETTINGS_OK = "ok"

/** 回执：`startActivity` 抛了。与 web 侧 `VPN_SETTINGS_FAILED` 逐字相同。 */
private const val VPN_SETTINGS_FAILED = "failed"

/**
 * web 层用来开关 `FLAG_SECURE` 的 JS 对象名（W-16 复审 major）。
 *
 * # 遮罩挡不住的那一格：最近任务里的缩略图
 *
 * 自动隐私锁的**典型发生时刻是应用已经在后台**（闲置 10 分钟 = 用户把手机放下了）。
 * 而 Android 的任务快照是在 Activity 停止那一刻拍的、之后不再刷新 —— 锁上时那张缩略图早就拍完了。
 * 于是遮罩虽然在（不可见的）WebView 里挂上了，最近任务里仍是锁定前的那一屏：订阅名、节点名、
 * 出口国旗、流量数字全都可读，**全程不需要密码**。这与 `settings.general.autoPrivacyModeDesc`
 * 承诺的「防偷窥 / nobody can read over your shoulder」直接冲突 —— 把手机递给别人正是那句话
 * 设定的场景。遮罩在这一格上帮不了忙：快照根本早于遮罩存在。
 *
 * `FLAG_SECURE` 是系统级的解法：置上之后系统不再为本窗口留可读快照。
 *
 * # 为什么跟着**开关**走，不跟着锁定态走
 *
 * 跟着锁定态走等于「锁上的那一刻才置 flag」—— 而那一刻快照早已拍完，置了也不会重拍。
 * 必须在**进后台之前**就置上，故真值取「自动隐私锁开着吗」（`autoPrivacyMode`）。
 *
 * # 为什么不无条件常开
 *
 * `FLAG_SECURE` 连用户自己的截图一起禁掉。没开隐私锁的用户没有理由被剥夺截图（反馈问题、
 * 存一张节点配置），故这条桥两个方向都要有：开关拨开置上、拨回清掉。
 * **少了 `clearFlags` 那半**的表现是「拨回去之后截图永久不能用，重装才恢复」，故门对两条腿逐个断言。
 */
private const val SECURE_SCREEN_BRIDGE = "polarisSecureScreen"

/** 载荷：置上 `FLAG_SECURE`。与 `ui/src/mobile/secure-screen-bridge.ts` 的 `SECURE_SCREEN_ON` 逐字相同。 */
private const val SECURE_SCREEN_ON = "on"

/** 载荷：清掉 `FLAG_SECURE`。与 web 侧 `SECURE_SCREEN_OFF` 逐字相同。 */
private const val SECURE_SCREEN_OFF = "off"

/** logcat 标签。跳转失败只落日志 —— 用户可见的那一半由 web 侧的回执渲染。 */
private const val TAG = "PolarisMainActivity"

/**
 * 读 `<html data-theme>` 并上报的探针。**是常量**（不含任何运行期值），故文档起始脚本只注册一次
 * 就够 —— 与安全区/字号那两条"每次变化都要重注册"的形态不同，理由在类头注。
 *
 * 三处一个都不能少：
 *  · `send()` 里**惰性**取 `window.<THEME_BRIDGE>`：注入对象与文档起始脚本都在 document-start，
 *    彼此先后没有保证；提到闭包外取会在"脚本先跑"那一序里永久拿到 `undefined`。
 *  · `MutationObserver` 接住的是**运行期改主题**，以及冷启动时"种子比本脚本晚一步"的那一序。
 *  · `DOMContentLoaded` 那次补报是第三张网：到那时注入对象与种子必然都已就位。
 *
 * `__polarisStatusBarWatch` 是幂等守卫：`onResume` 会重跑本脚本（只为补报一次当前值），
 * 少了它每跑一次就多挂一个观察者。
 */
private const val THEME_PROBE_JS =
  "(function(){var el=document.documentElement;" +
    "function send(){var b=window." + THEME_BRIDGE + ";" +
    "var t=el&&el.getAttribute('data-theme');" +
    "if(b&&t)b.postMessage(t);}" +
    "send();" +
    "if(window.__polarisStatusBarWatch)return;" +
    "window.__polarisStatusBarWatch=1;" +
    "new MutationObserver(send).observe(el,{attributes:true,attributeFilter:['data-theme']});" +
    "document.addEventListener('DOMContentLoaded',send);})();"

/**
 * 安全区与 Dynamic Type 的**生产端**（消费端分别在 `ui/src/styles/tokens.resolved.css` 的四个
 * `--safe-*` 与 `ui/src/mobile/mobile.css` 的 `--font-scale`）。
 *
 * # 为什么 CSS 那边光靠 `env(safe-area-inset-*)` 是错的
 *
 * 2026-09-05 真机实测（Pixel 6 / Android 16 / 1080×2400 @2.625）：
 *
 * | 来源                                    | top    | bottom |
 * |-----------------------------------------|--------|--------|
 * | `dumpsys` `type=statusBars`             | 128 px | —      |
 * | `dumpsys` `type=navigationBars`（手势条）| —      | 63 px  |
 * | `dumpsys` `mDisplayCutout`              | 128 px | 0 px   |
 * | WebView 里 `env(safe-area-inset-*)`     | 49 css | **0**  |
 *
 * 49 css × 2.625 = 128 —— 与 **cutout** 逐值相等，与 navigationBars 无关。即：
 * **Android WebView 的 `env(safe-area-inset-*)` 只反映 display cutout，不反映系统栏。**
 * 手势条 63 px 因此对 CSS 完全不可见，底部导航被压在它下面（五个屏全中）。
 *
 * 更要紧的是**顶部那条是巧合**：这台机器的挖孔 cutout 高度恰好等于状态栏高度，才让 `--safe-t`
 * 看起来"是对的"。换一台没有 cutout 的机器（多数横屏姿态、大量中低端机），`--safe-t` 会是 0，
 * 内容直接钻到状态栏底下 —— 同一个根因的另一条腿，只是这次没被看见。故本类把**四个方向**
 * 一起接管，不只补底部。
 *
 * # 接缝形态：两条量共用一条（写根元素的**内联**自定义属性）
 *
 * `mobile.css` 为 Dynamic Type 立下这条约定：原生读真值 → 写成根元素内联样式 → CSS 侧零改动。
 * 安全区走同一条，好处是 `tokens.resolved.css` 里的 `env()` 天然成为**回落**（内联样式压过 `:root`
 * 规则，无需 `!important`，也无需为"原生有没有报"再造一个变量）：
 *   · Android：本类报，内联值生效；
 *   · iOS / 浏览器直开 / 本类没来得及报：`env()` 那条继续生效。
 *
 * `--font-scale` 同理：`mobile.css` 的 `:root{--font-scale:1}` 是缺省（生产端缺席 ⇒ 按 1x 正常工作），
 * 本类把 `Configuration.fontScale` 写成内联值压过它。`html{font-size:calc(16px*var(--font-scale))}`
 * ⇒ 全部 rem 排版跟着放大，px 几何不动；容器查询的 em 阈值随之收窄，横排行按
 * `mobile-screen-shell.md`「Horizontal clipping fallbacks」自己改竖排。
 *
 * # ⚠️ 引擎自带一套，不关掉就是**相乘**（2026-09-05 实测，Android 16 / WebView Chrome 133）
 *
 * Android WebView 的 `WebSettings.textZoom` **缺省不是 100**，而是由 WebView 自己从
 * `Configuration.fontScale` 初始化。实测（本类落地前的包，`--font-scale` 恒 1）：
 *
 * | system font_scale | `html{font-size:16px}` 算出 | `.m-scroll` 容器 em 宽（412 CSS px） |
 * |-------------------|----------------------------|--------------------------------------|
 * | 1.0               | 16 px                      | 25.76 em                             |
 * | 1.3               | 20.8 px                    | 19.82 em                             |
 * | 2.0               | 32 px                      | 12.88 em                             |
 *
 * 即：**排版侧的 Dynamic Type 在 Android 上一直是通的**，通的那条是引擎、不是本应用。
 * 于是"照着接缝再报一次"会让两套相乘 —— 实测把 `--font-scale` 报成 1.300 后根字号变成
 * **27.04 px = 16 × 1.3 × 1.3**，那不是修复，是回归。
 *
 * 故本类先 `textZoom = 100` 把引擎那份钉成 1.0 倍，再报 `--font-scale`。收益不是"让字会变大"
 * （那本来就会），而是把缩放收敛成**一个来源**：
 *   · `--font-scale` 在 CSS / JS 里读得到真值，断点与将来任何按字号分档的规则有据可依；
 *   · 引擎那份是不受应用控制的第二套系统 —— 与 `mobile.css` 关掉 font boosting 同一条理由；
 *   · iOS / 浏览器直开走同一条接缝，三端只有一套机制要维护。
 * 代价是这两行必须成对存在，故 `mobile-entry.test.ts` ⑧ 把 `textZoom` 那条一并钉住。
 *
 * # 两条投递通道，缺一条就有一个真实的洞
 *
 *  1. `evaluateJavascript` —— 打给**当前已加载**的文档。运行期 inset 变化（转屏、切三键导航、
 *     多窗口）走这条。
 *  2. `addDocumentStartJavaScript` —— 注册给**今后每一次**文档加载，在文档建立后、页面脚本执行前
 *     运行。它管两件 ①管不了的事：
 *     · **冷启动竞态**：`onWebViewCreate` 时 URL 还没开始加载，此刻 `evaluateJavascript` 打空；
 *     · **`renderer-recovery` 的 `location.reload()`**（12s 没等到 `renderer:ready` 就重载）——
 *       重载会把内联样式连同整个文档一起丢掉，只有①的话安全区从此消失且再也不回来。
 *     它还顺带消掉首帧闪动：值在第一次布局之前就在了。
 *
 * `DOCUMENT_START_SCRIPT` 要 WebView 105+，与本仓 CSS 下限同一档；低于它时只剩①，
 * 故①不是可有可无的兜底，两条都要在。
 *
 * # 第三条量：系统栏明暗跟的是 **Polaris 生效主题**，不是系统夜间模式
 *
 * 规格 `component-specs/platform-status-bar.md`：「Choose light or dark system-bar appearance
 * from the **effective Polaris theme** and actual header contrast」。
 *
 * 此前这里只有一句无参 `enableEdgeToEdge()`，它的缺省 `SystemBarStyle.auto` 按
 * `Configuration` 的夜间位判明暗 —— 那是**系统**的明暗，与应用画的是什么颜色无关。失败形态：
 * 手机系统浅色 + 用户在 设置→显示 里选了「深色」时，应用画深色页头，Android 仍按浅底绘制状态栏
 * 图标（深色时间/电量/信号）⇒ 深字压在深底上，整条状态栏读不出来。反向组合同样成立。
 *
 * ## 真值取 `<html data-theme>`，**不在这里另写一套判断**
 *
 * 生效主题的折算口径只有一份：`ui/src/components/layout/theme-state.ts` 的 `resolveTheme`
 * （运行期）与它在主进程的同构体 `tray::theme_boot_script`（首帧种子）。两者的**共同产物**就是
 * 根元素上的 `data-theme` 属性，而 `tokens.resolved.css` 的 `:root[data-theme='dark']` 正是靠它
 * 选色 ⇒ **页头是什么颜色，`data-theme` 就是什么值**。本类因此只做搬运：读这个属性、把它送过来。
 *
 * 反面做法是在 Kotlin 里照 `resolveTheme` 再写一遍（读 config.uiTheme + 夜间位）。那会立刻多出
 * 一条要同步的规则，而两份判断分叉时**没有任何门看得见** —— 表现只是"某些组合下状态栏不对"。
 * 故本文件里 `uiMode` 这个词只出现在注释里，一行代码都不读它（门为此正面对拍）。
 *
 * ## 冷启动那一格由谁负责
 *
 * `onCreate` 里的无参 `enableEdgeToEdge()` **保留不动**，它管的是"WebView 还没画出任何东西"的
 * 那几十毫秒：此刻屏幕上是窗口背景，而窗口主题是 `Theme.MaterialComponents.DayNight.NoActionBar`
 * —— 它本来就跟随系统夜间模式 ⇒ 那一格按夜间位取明暗恰恰是**对的**。它同时还提供 scrim 与
 * `navigationBarContrastEnforced` 的缺省，改成显式 `SystemBarStyle` 就得把 androidx 的私有
 * scrim 常量抄进来，为一个不需要改的量引入一份会漂移的副本。
 *
 * 内容一上屏就换真值：文档起始脚本在**页面脚本执行之前**跑，那时 `data-theme` 要么已被种子写好
 * （立刻上报），要么随后一瞬被种子写上（`MutationObserver` 接住）—— 两种注入顺序都覆盖到，
 * 故用户看见 Polaris 页头的那一帧，状态栏已经是对的。
 *
 * ## 运行期三条腿
 *
 *  · **改主题**（设置→显示）：`DisplayPage` 落 `data-theme` → `MutationObserver` → 上报。
 *    这条腿不经任何 Tauri command，故也不需要在 Rust 侧加命令、不碰桌面平台一行代码。
 *  · **回前台 / 配置变化**：按最后一次已知值重申一次。`enableEdgeToEdge` 是 `onCreate`
 *    里的一次性调用，而本 Activity 的 `configChanges` **含 `uiMode`** ⇒ 系统深浅色切换时
 *    Activity 不重建、没有任何东西会重新施加外观。这两条是那条缺口的兜底。
 *  · 注意重算的依据仍是 `polarisDark`（即 `data-theme`），**不是**新的夜间位：移动入口没有桌面
 *    `AppShell` 那条 `matchMedia('change')` 监听 ⇒ `uiTheme='system'` 时系统切深浅色，页面颜色
 *    **不会跟着变**。此刻若把状态栏按新夜间位翻过去，制造的正是本条要修的那种失配。
 *
 * 投递方向与另两条量相反（web → 原生），故用 `WebViewCompat.addWebMessageListener` 而不是
 * `addJavascriptInterface`：前者能限定来源、回调保证在 UI 线程，且用的是本文件已经在用的
 * 同一个 androidx.webkit，不引新依赖。
 */
class MainActivity : TauriActivity() {
  /** 上一次注册的文档起始脚本；inset 变了要先撤旧的，否则同一份文档会被灌进两组值。 */
  private var safeAreaScript: ScriptHandler? = null

  /** 同上，字号缩放那一条。两条量各留各的句柄：合成一条会让任一方变化都把另一方的值一起重写。 */
  private var fontScaleScript: ScriptHandler? = null

  /** 回前台时要重报一次，故留一份引用；`onWebViewCreate` 之前它是 null。 */
  private var insetTarget: View? = null

  /**
   * 同上，字号那条要往 WebView 上打，故也留一份。
   *
   * 安全区那条不需要：它的重报走 `requestApplyInsets` → 监听器回调，WebView 在闭包里。
   * 字号没有等价的系统回调可借（`Configuration` 变化不经过 inset 通道），只能自己拿着。
   */
  private var webViewTarget: WebView? = null

  /**
   * 最近一次从 web 层收到的生效主题（`true` = 深）。`null` = **一次都还没收到过** ——
   * 此时不去猜：`enableEdgeToEdge()` 按窗口背景那一档给的值仍然成立（见类头注「冷启动那一格」），
   * 拿一个编出来的值去覆盖它只会把对的一格改错。
   */
  private var polarisDark: Boolean? = null

  /** 探针的文档起始脚本句柄。载荷是常量 ⇒ 只注册一次，非空即已注册。 */
  private var themeProbeScript: ScriptHandler? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    // 无参形态**是刻意的**：它管的是内容上屏之前那几十毫秒（那时屏幕上是 DayNight 的窗口背景，
    // 按夜间位取明暗正确），并提供 scrim 与对比度的缺省。生效主题那一格由 `onThemeReport`
    // 在文档起始脚本跑到时接管。理由见类头注「冷启动那一格由谁负责」。
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  /**
   * 监听器**挂内容根视图，不挂 WebView**。
   *
   * `setOnApplyWindowInsetsListener` 是替换不是追加，而 Chromium WebView 自己正是**用监听器**
   * 接 display cutout 的 —— 挂在 WebView 上会把它那份顶掉。实测（2026-09-05）：装上监听器后
   * `env(safe-area-inset-top)` 从 49px 掉到 0，且改成在监听器里回调
   * `ViewCompat.onApplyWindowInsets(webView, insets)` **也救不回来**（那只补 View 的默认实现，
   * 补不了被替换掉的监听器）。后果是 `tokens.resolved.css` 那四条 `env()` 回落在 Android 上
   * 被静默打死 —— CSS 文本一个字没改，任何只读 CSS 的门都看不见。
   *
   * 挂在 `android.R.id.content`（wry `setContentView` 放 WebView 的那个 FrameLayout）上则两不相扰：
   * `ViewGroup.dispatchApplyWindowInsets` 是**先跑自己的监听器、再往子视图分发**，只要不 consume，
   * WebView 那份监听器照常收到。
   */
  private fun insetHost(): View = findViewById(android.R.id.content)

  override fun onWebViewCreate(webView: WebView) {
    val host = insetHost()
    insetTarget = host
    webViewTarget = webView
    // The app has its own responsive layout and system-font-scale path. Disable only WebView page
    // gestures (pinch / double tap); do not intercept touch events, scrolling, or input focus.
    webView.settings.apply {
      setSupportZoom(false)
      setBuiltInZoomControls(false)
    }
    // 字号：这里报的是**冷启动**那一次。`evaluateJavascript` 此刻多半打空（URL 还没开始加载，
    // 见头注「两条投递通道」①），真正兑现冷启动的是同一次调用里注册的文档起始脚本。
    publishFontScale(webView)
    // 系统栏明暗：先把收信口开好（注入对象必须早于任何页面脚本），再放探针。
    installThemeBridge(webView)
    publishThemeProbe(webView)
    // 返回键的根页出口。与主题桥同为 web → 原生方向，故同样用 `addWebMessageListener`。
    installBackNavBridge(webView)
    // 系统 VPN 设置页的跳转出口（W-17）。同上，web → 原生方向。
    installVpnSettingsBridge(webView)
    // 隐私锁开着时禁掉系统快照/截图（W-16）。同上，web → 原生方向。
    installSecureScreenBridge(webView)
    ViewCompat.setOnApplyWindowInsetsListener(host) { view, insets ->
      // systemBars（状态栏 + 导航栏/手势条）∪ displayCutout：取并集而不是二选一 ——
      // 横屏时刘海在侧边而系统栏不在，竖屏时反过来，两者都得让开。
      val bars =
        insets.getInsets(
          WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
        )
      publishSafeArea(webView, bars, view.resources.displayMetrics.density)
      // **不 consume**：insets 要继续往子视图走，WebView 的 cutout 监听器还等着它
      // （`tokens.resolved.css` 那四条 `env()` 回落的命就在这一行）。
      ViewCompat.onApplyWindowInsets(view, insets)
    }
  }

  override fun onResume() {
    super.onResume()
    // 回到前台时重报一次：后台期间用户可能改过导航模式（手势 ↔ 三键，24dp ↔ 48dp）。
    insetTarget?.let { ViewCompat.requestApplyInsets(it) }
    // 字号：用户是**离开应用**去系统设置改的，回来必经这里。实测下这一步多半是重建后的
    // 第二次重报（冗余但无副作用）；它真正管的是「重建没发生」的那些路径，
    // 例如后台被 config 变化叫醒却未 relaunch，或将来 `configChanges` 加上 `fontScale`。
    webViewTarget?.let {
      publishFontScale(it)
      // 顺带让探针补报一次当前值（幂等，见 `THEME_PROBE_JS` 的守卫）。
      publishThemeProbe(it)
    }
    // 系统栏外观：`WindowInsetsController` 的这两位不随本 Activity 的生命周期保管，
    // 后台期间被系统重置过就再也回不来 —— 重申一次是最便宜的兜底。
    reapplySystemBarAppearance()
  }

  /**
   * 运行期字号变化。**先查清了再接线**（2026-09-05 模拟器实测，Android 16 / API 36）：
   *
   * 本 Activity 的 `android:configChanges` 是
   * `orientation|keyboardHidden|keyboard|screenSize|locale|smallestScreenSize|screenLayout|uiMode`
   * —— **不含 `fontScale`**。实测 `settings put system font_scale 1.3` 后 logcat 出现
   * `WindowManager: finishDrawing of relaunch: Window{… com.polaris2.app/.MainActivity}`，
   * 进程 pid 不变而 Activity 被**重建** ⇒ 走的是 `onCreate` → `onWebViewCreate` 那条冷启动路径，
   * 本回调对字号变化**根本不会触发**。
   *
   * 那为什么还要有它：`configChanges` 是一行随时可能被加长的清单（加 `fontScale` 是很自然的
   * 一次「避免重建」优化）。真到那天，重建路径消失、`onResume` 也不再走（用户从系统设置返回时
   * Activity 没离开过前台的场景），字号会**静默冻在旧值**上，且没有任何门看得见。
   * 这个回调是那条缺口的兜底：它对已声明的那几项（转屏、深浅色）今天就在跑，
   * 故不是一段没人执行过的死代码 —— 转屏一次即可验证它确实被调用。
   */
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    webViewTarget?.let { publishFontScale(it) }
    // 系统深浅色切换走的正是这里（`configChanges` 含 `uiMode` ⇒ Activity 不重建），
    // 而 `enableEdgeToEdge` 是 `onCreate` 里的一次性调用 —— 不重申的话外观会停在旧值。
    // **重申的是 `polarisDark`，不是新的夜间位**：页面颜色不跟随系统切换（移动入口没有
    // `AppShell` 那条 matchMedia 监听），跟着夜间位翻就是在造本条要修的那种失配。
    reapplySystemBarAppearance()
  }

  /**
   * 开收信口：把 [THEME_BRIDGE] 注入到 web 层的 `window` 上。
   *
   * 用 `addWebMessageListener` 而不是 `addJavascriptInterface` 的三条理由：能限定来源、
   * 回调由 androidx 保证在 **UI 线程**（`addJavascriptInterface` 走的是 WebView 私有的
   * JavaBridge 线程，碰 `window.decorView` 要自己 `runOnUiThread`），以及用的是本文件已经在用的
   * 同一个 androidx.webkit。特性下限 WebView 88，低于本仓 CSS 那道 105 的下限。
   *
   * 允许面用 `*`，与两条文档起始脚本同一条理由（见 `publishSafeArea`）：装载的只可能是本应用
   * 自己的文档，而写死打包态或 dev 档其中一个会让另一档静默失效。
   */
  private fun installThemeBridge(webView: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return
    WebViewCompat.addWebMessageListener(webView, THEME_BRIDGE, setOf("*")) { _, message, _, _, _ ->
      // 类型必须先判：`getData()` 在载荷是 ArrayBuffer 时**抛异常**而不是返回 null。
      // 本应用只会 post 字符串，但一个会抛的取值器不该靠"调用方不会那么做"来保证。
      onThemeReport(if (message.type == WebMessageCompat.TYPE_STRING) message.data else null)
    }
  }

  /**
   * 开返回键的收信口（见 [BACK_NAV_BRIDGE] 的头注）。
   *
   * **不判载荷**：这条桥只有一个语义，前端也只 post 一个字符串。判了反而多一条
   * 「字面量在两侧漂了 ⇒ 静默不生效」的失效面，而它换不来任何能力。
   * `moveTaskToBack` 由 androidx 保证回调在 UI 线程（同 `installThemeBridge` 的第二条理由），
   * 故这里不需要 `runOnUiThread`。
   */
  private fun installBackNavBridge(webView: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return
    WebViewCompat.addWebMessageListener(webView, BACK_NAV_BRIDGE, setOf("*")) { _, _, _, _, _ ->
      moveTaskToBack(true)
    }
  }

  /**
   * 开系统 VPN 设置页的收信口（见 [VPN_SETTINGS_BRIDGE] 的头注）。
   *
   * `FLAG_ACTIVITY_NEW_TASK` 与 `ShellPlugin.open` 同一条理由：设置页属于另一个应用，
   * 不该压进本应用的任务栈（压进去之后用户按返回会先回到设置页）。
   *
   * `runCatching` 罩的是 `ActivityNotFoundException` 与 `SecurityException` 两类 ——
   * 前者是设备上没有这个 Activity，后者是被策略挡下（受管设备）。两者对用户是同一件事
   * （「这里跳不过去，请照着路径手动走」），故不分开报码。
   * `replyProxy` 由 androidx 保证回调在 UI 线程，与另两条桥同。
   */
  private fun installVpnSettingsBridge(webView: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return
    WebViewCompat.addWebMessageListener(webView, VPN_SETTINGS_BRIDGE, setOf("*")) {
      _,
      _,
      _,
      _,
      replyProxy ->
      val opened =
        runCatching {
            startActivity(
              Intent(Settings.ACTION_VPN_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
          }
          .onFailure { Log.w(TAG, "打不开系统 VPN 设置页", it) }
          .isSuccess
      replyProxy.postMessage(if (opened) VPN_SETTINGS_OK else VPN_SETTINGS_FAILED)
    }
  }

  /**
   * `FLAG_SECURE` 的收信口（见 [SECURE_SCREEN_BRIDGE] 的头注）。
   *
   * **两条腿都必须在**：`addFlags` 让系统不再为本窗口留可读快照，`clearFlags` 在用户关掉隐私锁时
   * 把截图能力还回去。只留前半 = 拨回去之后截图永久不能用。
   *
   * 载荷类型先判（同 [installThemeBridge]）：`getData()` 在载荷是 ArrayBuffer 时**抛异常**。
   * 不认识的载荷只落日志、不改窗口标志 —— 猜一个方向执行会把「两侧字面量漂了」变成
   * 「静默按错误方向生效」。androidx 保证回调在 UI 线程，故不需要 `runOnUiThread`。
   * **不回执**：`addFlags` / `clearFlags` 没有失败这一支，一个恒成功的回执只是噪声。
   */
  private fun installSecureScreenBridge(webView: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return
    WebViewCompat.addWebMessageListener(webView, SECURE_SCREEN_BRIDGE, setOf("*")) {
      _,
      message,
      _,
      _,
      _ ->
      val payload = if (message.type == WebMessageCompat.TYPE_STRING) message.data else null
      when (payload) {
        SECURE_SCREEN_ON -> window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        SECURE_SCREEN_OFF -> window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
        else -> Log.w(TAG, "未知的 FLAG_SECURE 载荷，窗口标志不动：$payload")
      }
    }
  }

  /**
   * 放探针。两条投递通道与另两条量同构，但**文档起始脚本只注册一次**：载荷是常量，
   * 重注册除了多一次撤销/新建没有任何作用（安全区与字号那两条必须重注册，因为它们的值会变）。
   */
  private fun publishThemeProbe(webView: WebView) {
    webView.evaluateJavascript(THEME_PROBE_JS, null)
    if (themeProbeScript != null) return
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) return
    themeProbeScript =
      WebViewCompat.addDocumentStartJavaScript(webView, THEME_PROBE_JS, setOf("*"))
  }

  /**
   * 消费一次上报。归一化与 `theme-state.ts` 的 `normalizeSeed` 同口径：**只认 `dark` / `light`**，
   * 其余（含 null、被改坏的属性值）一律当没收到 —— 停在旧值也好过按一个读不懂的字符串翻转。
   */
  private fun onThemeReport(theme: String?) {
    val dark =
      when (theme) {
        "dark" -> true
        "light" -> false
        else -> return
      }
    polarisDark = dark
    applySystemBarAppearance(dark)
  }

  /** 按最后一次已知值重申一次；一次都没收到过时**不动**（见 `polarisDark` 的注释）。 */
  private fun reapplySystemBarAppearance() {
    polarisDark?.let { applySystemBarAppearance(it) }
  }

  /**
   * 真正改系统栏外观的那一句。
   *
   * `isAppearanceLight*Bars` 说的是**图标要不要按浅色底来画**（深色图标），故它取的是
   * `!dark`：Polaris 深色主题 ⇒ 页头是深底 ⇒ 图标必须浅色 ⇒ 这两位为 false。
   * 写成 `= dark` 会把缺陷从"跟错了源"变成"永远反着"，两条都读不出来。
   *
   * 导航栏那一位一并设：底部导航 `.m-dock` 铺的是 `surface.primary`，手势条压在它上面，
   * 只改状态栏会在深浅色两侧各留一半不可读。
   */
  private fun applySystemBarAppearance(dark: Boolean) {
    val controller = WindowCompat.getInsetsController(window, window.decorView)
    controller.isAppearanceLightStatusBars = !dark
    controller.isAppearanceLightNavigationBars = !dark
  }

  private fun publishSafeArea(webView: WebView, bars: Insets, density: Float) {
    val js = safeAreaScript(bars, density)
    webView.evaluateJavascript(js, null)
    if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
      safeAreaScript?.remove()
      // 允许面用 `*`：装载的只可能是本应用自己的文档（打包态 `tauri.localhost`、
      // dev 档是 devUrl），而写死其中一个会让另一档静默失效；载荷只有四个 CSS 长度值。
      safeAreaScript = WebViewCompat.addDocumentStartJavaScript(webView, js, setOf("*"))
    }
  }

  private fun safeAreaScript(bars: Insets, density: Float): String {
    val t = cssPx(bars.top, density)
    val r = cssPx(bars.right, density)
    val b = cssPx(bars.bottom, density)
    val l = cssPx(bars.left, density)
    return "(function(){var s=document.documentElement.style;" +
      "s.setProperty('--safe-t','$t');" +
      "s.setProperty('--safe-r','$r');" +
      "s.setProperty('--safe-b','$b');" +
      "s.setProperty('--safe-l','$l');})();"
  }

  /**
   * 字号缩放的两条通道，与 `publishSafeArea` 逐字同构（同一条接缝，不另发明一套）。
   *
   * 载荷是**一个无单位数**：`mobile.css` 的 `html{font-size:calc(16px * var(--font-scale))}`
   * 要的是纯数，写成 `1.3px` 会让整条 `calc()` 失效并静默退回缺省 1。
   */
  private fun publishFontScale(webView: WebView) {
    // **必须先把引擎自带的那一套关掉**，否则两套缩放相乘。见类头注「⚠️ 引擎自带一套」。
    // `textZoom` 缺省不是 100，而是由 WebView 从 `Configuration.fontScale` 初始化；显式钉 100
    // 会把它整个替换成 1.0 倍，且是精确的整数百分比，不产生任何舍入残差。
    webView.settings.textZoom = 100
    val js = fontScaleScript(resources.configuration.fontScale)
    webView.evaluateJavascript(js, null)
    if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
      fontScaleScript?.remove()
      fontScaleScript = WebViewCompat.addDocumentStartJavaScript(webView, js, setOf("*"))
    }
  }

  /**
   * **必须钉 `Locale.US`**，与 `cssPx` 同一个理由：俄语/法语等区域下 `%.3f` 会给出 `1,300`，
   * CSS 当场解析失败、`--font-scale` 静默退回缺省 1 —— 一个只在部分语言下复现的「字号不生效」。
   *
   * 不做上下限钳制：钳住就等于替用户否掉他在系统里选的无障碍档位，而本类存在的理由正是兑现它。
   */
  private fun fontScaleScript(scale: Float): String =
    "(function(){document.documentElement.style.setProperty('--font-scale','" +
      String.format(Locale.US, "%.3f", scale) +
      "');})();"

  /**
   * 设备像素 → CSS 像素。**必须钉 `Locale.US`**：跟随系统语言的话，法语/俄语等区域会把小数点
   * 格式化成逗号（`24,00px`），CSS 当场解析失败并静默退回 `env()` —— 一个只在部分语言下复现的
   * 安全区丢失。
   */
  private fun cssPx(devicePx: Int, density: Float): String =
    String.format(Locale.US, "%.3fpx", devicePx / density)
}
