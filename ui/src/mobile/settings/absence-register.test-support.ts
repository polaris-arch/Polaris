/**
 * 设置屏的**与桌面逐块对差登记表**。形态与口径见 `../parity-registry.ts`；
 * 门在 `../screen-parity.test.ts`。
 *
 * # 这一屏叠**两个**动作面：`controls` + `config-fields`
 *
 * 🔴 上一版只有 `config-fields` 一个，理由写的是「整个目录里字面意义的 `<button` 只有 12 颗，
 * 全是脚手架」——**那是提取器的性质，不是这一屏的事实**（2026-09-06 复审 major）：这一屏真正的
 * 动作全走同目录 `Primitives.tsx` 的 `<Button>`，共 39 处调用点，它们一个都不写 `update({…})`，
 * 于是两个面都看不见。终端环境变量的两颗复制、`proxy.clear`（清理系统代理残留）就是从这条缝
 * 漏掉的：既没接、也没登记、也没有任何门看得见。
 *
 * 两个面各答一半，缺一不可：
 *  · `controls` 答「这一屏有哪些动作」（复制 / 清理 / 重试 / 安装助手 / 换核 …）；
 *  · `config-fields` 答「它能改哪些配置」——那是设置屏特有的能力面，`<Switch>` / `<Select>` /
 *    `<TextInput>` 这些原语本身在 `controls` 面上只是三条原语登记，数不出它们各自改的是哪一格。
 *
 * 🔴 **两个面加起来仍然缩掉了什么、由谁兜**（不许把本表说成覆盖了设置屏的全部粒度）：
 *  · 缩掉的是「同一个字段用哪种控件改、那颗控件长什么样、在哪一行」。
 *  · 兜它的是 `MobileSettings.test.tsx`：① 逐项对差九张子页（本表的 blocks 面也再对一次，
 *    见下），⑬ 钉开关的命中盒，⑯ 钉行高，⑩ 钉每一处写失败都有行内回显。
 *    本门另有一条**正面断言**把这个交接钉死：桌面 `SettingsPage.tsx` 路由到的九个子页组件，
 *    必须恰好等于 `MOBILE_SETTINGS_PAGES` ∪ `MOBILE_ABSENT_SETTINGS_PAGE` —— 那两个常量
 *    正是 `MobileSettings.test.tsx` ① 的真值源。交接处对不上就红，不是「相信它兜住了」。
 *    ⑥ 组还逐条打开这两道 `describe` 核对，交接给一个不存在的门 = 没交接。
 *
 * # 两条 `config-fields` 面独有的判定方向
 *
 *  · **`ported` 由门自己算，不由本表声明**：移动端设置树里有 `update({ <同一个字段>` 就是接了。
 *    这个信号是**写**，不是文案 —— 不存在「字面命中但其实没接」那种假绿（首页那张表就栽在
 *    文案信号上：`home.ruleProxy` 在移动端出现过，但那是环形图上的一句标签，不是「加规则」那条腿）。
 *  · 因此本表**只登记缺口**。登记了却又真的写了同一个字段 ⇒ 门当场报「僵尸登记」。
 *
 * # 这张表让哪些事第一次可见
 *
 * · **配置字段面**：桌面写的字段里，一批是平台上真的没有那个对象（托盘 / 窗口几何 /
 *   窗口特效 / 内核独立更新），一批是裁定明确要求不提供（管理 API 那一族，含那条计算键）。
 *   🔴 **2026-09-13 这一轮把「真的欠着」那一栏清了四条，靠的是补证据、不是改措辞**：
 *   `bypassLAN` / `bypassLANList`（生成侧只在 `platform == "win32"` 那一支消费，android 臂恒空）、
 *   `hardwareAcceleration`（逃生门三条臂各挂 `#[cfg(target_os)]`，Android 上函数体不编译）、
 *   `autoLightweightMode`（轻量态的**回程**在桌面是托盘，移动端没有托盘 ⇒ 销毁之后回不来）。
 *   上一版这四条写着「拿不出证据就该留在债务里红着」—— 那是对的纪律，这一轮做的是去把证据找出来，
 *   四条锚现在都指着一条机器可核对的平台判定。
 * · **动作面（2026-09-06 新加）**：终端代理环境变量的两颗复制与那整块 `TerminalEnvBlock`
 *   曾是零处、零登记、零说明的活缺口（它既不写配置、又住在 `SettingsNetwork.tsx` 文件内部）。
 *   2026-09-13 判定为 `platform-absent`：那几行要复制走的是 `http_proxy=http://127.0.0.1:<mixedPort>`，
 *   而 Android 上根本不发 mixed inbound —— 复制出去的是一条必然坏掉的指令。
 *   🔴 2026-09-25 改判 `adjudicated-out`：不发 mixed 是**安全裁定**（共享回环 + 零认证），不是平台做不到。
 * · **今天这张表上还剩的债，只剩一族**：应用内下载安装那五条（`AppUpdateCard` 四颗 +
 *   `field:autoDownloadUpdate`）。2026-09-13 复审把它们从豁免族撤了回来（见 actions 面逐条的 🔴），
 *   判据是同一条：**一个能做而没做的事被判成平台边界，就等于给它拆掉了将来会响的那只闹钟**。
 *   🔴 同日**批 17** 销掉的三条不在上面这一族里，逐条是「能力接上了」而不是「改了措辞」：
 *   `field:dnsDefaults` 与 `field:dnsDefaults.unmatchedAction`（v2 默认策略两格现在写得动，
 *   落点 `DnsPage.tsx#DnsDefaultsRows`）、`Csel.tsx|f:header`（那两格的候选表是分组的，
 *   组头可折叠，复用规则屏的 `SelectSheetPanel`）。前两条按本面的规矩**整条删掉**而不是改判。
 *   逐条理由见各自的 `note`。
 */
import type { ScreenParityRegister } from '../parity-registry.test-support';

/** 核随应用打包（`libbox.aar`）⇒ 没有「独立于应用的内核」这个对象可更新。三条内核更新字段共用。 */
const CORE_IS_BUNDLED = {
  file: 'src-tauri/gen/android/app/build.gradle.kts',
  mustContain: 'implementation(files("libs/libbox.aar"))',
} as const;

/**
 * 托盘：`tauri::tray` / `tauri::menu` 在 mobile target 上整个不存在，故凡直接消费它们的项都挂
 * `#[cfg(desktop)]`。锚指的是**那条守卫底下的第一个函数签名**，不是头注里那句话 ——
 * 2026-09-06 复审 minor：锚指注释时，给 Android 落一个实现、把守卫删掉而注释原样留着，豁免继续成立。
 */
const NO_TRAY = {
  file: 'src-tauri/src/app_tray.rs',
  mustContain: '#[cfg(desktop)]\npub(crate) fn set_tray_state(app: &tauri::AppHandle',
} as const;

/**
 * 管理 API 那一族（局域网访问 / 密钥 / 内置面板）由 IA 裁定 #9 明确要求移动端不提供，
 * 且 `MobileSettings.test.tsx ③` **正面钉着它不许出现**。
 * 🔴 锚指的就是那道门 —— 门被删掉，这三条豁免当场失效、退回债务。
 */
const MGMT_API_ADJUDICATED = {
  file: 'ui/src/mobile/settings/MobileSettings.test.tsx',
  mustContain: '不出现「允许局域网访问管理 API」开关',
} as const;

/** 提权助手：移动端没有 helper 进程 —— 隧道来自 `VpnService`，权限来自系统授权弹窗。 */
const HELPER_IS_VPN_SERVICE = {
  file: 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt',
  mustContain: 'fun vpnAuthStatus(invoke: Invoke)',
} as const;

/**
 * 系统代理这个对象在 Android 上不存在：`config-engine` 只在**非** android/ios 时才发 mixed inbound。
 * 「清理系统代理残留」「去首页切换接管方式」两条共用它。终端代理环境变量那一整块（含两颗复制）
 * 2026-09-25 起不再挂它，改按 `adjudicated-out` 锚 [`TERMINAL_ENV_ADJUDICATED`]（主因是安全裁定）。
 */
const NO_MIXED_INBOUND = {
  file: 'crates/config-engine/src/builder/inbounds.rs',
  mustContain: 'Platform::Android | Platform::Ios => false,',
} as const;

/*
 * 🔴 **这里刻意没有「应用内下载安装」的 platform-absent 常量**（2026-09-13 复审撤回）。
 *
 * 曾经写过一条，锚指 `app_update.rs` 的 `AssetPlatform::from_os(std::env::consts::OS)` ——
 * 它能过门（`PLATFORM_TOKEN` 收 `Platform::` 这个词法标记），但它描述的是**发布流水线的
 * 资产命名**，不是 Android 的能力边界。三条反证：
 *  · `installApk` 已经接上（`…/vpn/PolarisVpnPlugin.kt` 的 `fun installApk(invoke: Invoke)`），
 *    `REQUEST_INSTALL_PACKAGES` 也已在 manifest 里；
 *  · `UpdatePage.tsx` 头注自己写着这是一次**取舍**（权限画像要单独过一次审），不是平台判定；
 *  · 同形先例已纠正过一次：`privacyMode` 当年也从这一档改判回 `absent`
 *    （Android 上隐私锁并非不可实现），后来批 5 真的把它接上了。
 *
 * 放进豁免族的后果是：release 真出 APK 那天**没有任何东西会红**来提醒有人把这条腿接上 ——
 * 那正是本表反复警告的「有理由的洞变成没人看着的洞」。故这一族留在债务侧（`absent` +
 * 一句已渲染的用户可见理由），逐条见 actions 面。
 */

/**
 * 终端代理环境变量块：**安全裁定**的后果，不是平台做不到（2026-09-25 改判，盘点 §4.1「标签不准」）。
 * 那块复制走的是 `http_proxy=http://127.0.0.1:<mixedPort>`，而 Android 上**按裁定不发** mixed inbound
 * （`crates/config-engine/src/builder/inbounds.rs` 的 mixed 段：共享回环 + 零认证，AmneziaVPN #2452 同类）。
 * 🔴 锚指的是那道门：`MobileSettings.test.tsx` ③ 里那一条同时钉「块不出现」与「裁定本体
 * （`emits_mixed_inbound` 的 Android 臂）还在」—— 裁定被撤时它当场红，这四条随之退出豁免。
 */
const TERMINAL_ENV_ADJUDICATED = {
  file: 'ui/src/mobile/settings/MobileSettings.test.tsx',
  mustContain: '终端代理环境变量块不出现（安全裁定：Android 不发 mixed inbound）',
} as const;

/**
 * D8：陈先生 2026-09-25 裁定放弃的三项低价值能力（硬件加速逃生门 / 应用内卸载 / 轻量模式）。
 * 三项在 Android 上**都做得到**（盘点 §2 D8），故不再挂 `platform-absent`；
 * 锚指 `MobileSettings.test.tsx` ⑨ 里正面钉着「三项一处都不出现」的那一条。
 */
const D8_ADJUDICATED = {
  file: 'ui/src/mobile/settings/MobileSettings.test.tsx',
  mustContain: 'D8 用户裁定放弃（2026-09-25）：硬件加速逃生门 / 应用内卸载 / 轻量模式一处都不出现',
} as const;

export const SETTINGS_PARITY: ScreenParityRegister = {
  screen: 'settings',
  desktopDirs: ['ui/src/components/screens/settings'],
  mobileDir: 'ui/src/mobile/settings',

  // 🔴 **两个面都要**（上一版只有 `config-fields`）：那一版的理由写的是「整个目录里字面意义的
  // `<button` 只有 12 颗，全是脚手架」—— 那是提取器的性质，不是这一屏的事实。真正的动作全走同目录
  // `Primitives.tsx` 的 `<Button>`（39 处调用点），`config-fields` 面一个都看不见（它们不写
  // `update({…})`）。终端环境变量的两颗复制、`proxy.clear`（清理系统代理残留）就是从这条缝漏掉的。
  actionFaces: ['controls', 'config-fields'],
  // `Primitives.tsx` 是这一屏的原语库（Button / Switch / Card / SetRow …）：它声明的组件不是
  // 「数据块」。豁免是挣来的 —— ⓪ 组要求它在本屏 desktopDirs 里、且自己一句 `t('…')` 都不渲染。
  primitiveModules: ['ui/src/components/screens/settings/Primitives.tsx'],
  extraBlockSources: [],
  sharedPrimitives: [
    'ui/src/components/Fold.tsx',
    'ui/src/components/Icons.tsx',
    'ui/src/components/InfoIcon.tsx',
    'ui/src/components/dialogs/Csel.tsx',
  ],
  slotHandoffGates: [
    { file: 'ui/src/mobile/settings/MobileSettings.test.tsx', mustContain: '① 分段构成 = 桌面 9 子页去掉 helper' },
    { file: 'ui/src/mobile/settings/MobileSettings.test.tsx', mustContain: '⑩ 写失败必须可见' },
  ],
  actions: [
    /* ══════════════ `controls` 面：桌面这一屏的每一个动作元素 ══════════════
     * 🔴 上一版这一屏**只有** `config-fields` 一个面，理由写的是「整个目录里字面意义的 `<button`
     * 只有 12 颗，全是脚手架」——那是提取器的性质，不是这一屏的事实：真正的动作全走同目录
     * `Primitives.tsx` 的 `<Button>`（39 处调用点），一个都不写 `update({…})`，两个面都看不见。
     * 终端环境变量的两颗复制与 `proxy.clear` 就是从这条缝漏掉的（2026-09-06 复审 major）。
     */

    /* ── 应用更新卡（整卡的处置见 blocks 面 AppUpdateCard 一条）──────────────── */
    {
      id: 'AppUpdateCard.tsx|k:settings.about.checkUpdate',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: 'runAppUpdateCheck(checkAppUpdateViaIpc, config)',
        },
      },
      note:
        '检查应用更新（卡面主键）。2026-09-06 批 W-19 接上，2026-09-13 复核为真：后端那一档走 ' +
        '`check_app_update_release_only`（只比版本、不选资产），此前「`AssetPlatform::from_os("android")` ' +
        '返 `None` ⇒ 取平台那一步就早退成 `{hasUpdate:false}`」的结构性恒答「已是最新」已经修掉 ' +
        '（`app_update.rs:517` 那条 `let Some(platform) = platform else` 分支）。上一版这里写的 ' +
        '「移动端更新页只有一行说明」在接线那天就已经为假。',
    },
    {
      id: 'AppUpdateCard.tsx|k:settings.about.checkUpdate#2',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: "disabled={check.phase === 'checking'}",
        },
      },
      note:
        '检查更新的第二个渲染位（桌面在失败态下重新出现的那一颗）。移动端不按状态换按钮：同一颗 ' +
        '`app-update` 按钮恒在，只在 `checking` 时禁用 ⇒ 失败之后它照样点得动，桌面那两个渲染位在 ' +
        '移动端是同一颗。锚指的是那条「只有正在查时才禁用」的判据 —— 它要是变成恒禁用，本条当场失配。',
    },
    {
      id: 'AppUpdateCard.tsx|k:settings.update.download',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: 'updateApi.download(target).then(',
        },
      },
      note:
        '下载新版本。**2026-09-13（批 15）接上**，上一版记的两件销账条件同批都成立了：' +
        '① 资产发得出来 —— `AssetPlatform::Android` 落地（选 `*-android-arm64.apk`，' +
        '命名契约与 `.github/workflows/android.yml` 的 `release-apk` job 逐字对拍），' +
        '② 这一屏把下载→交系统安装器那一跳画出来了。锚指的是那条真的下载 IPC，不是按钮文案。' +
        '⚠️ **发布腿还欠一件仓外的前置**：`release-apk` 要四个签名 secret（密钥库按裁定在仓外），' +
        '今天一个都没有 ⇒ 那条腿一次都没跑过。故「有新版本但这个 release 没发 APK」是一档**真实**' +
        '会发生的结果：后端那一档如实回空的三个资产字段，本屏据此不画下载按钮（判据 ' +
        '`appUpdateDownloadTarget`），退回「打开发布页」。',
    },
    {
      id: 'AppUpdateCard.tsx|k:settings.update.restartAndInstall',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: 'updateApi.install(staged).then(',
        },
      },
      note:
        '重启并安装。**2026-09-13（批 15）接上**，但**对位不是同名的那句文案**：桌面那颗的落地方式是' +
        '「写安装脚本 → 停代理 → 退出应用 → 重启装上」，Android 是「经 FileProvider 交系统安装器 → ' +
        '本进程继续活着等用户在系统 UI 上确认」（后端 `update_install` 的 Android 分支明写不写脚本、' +
        '不停代理、不退应用）。故移动端这颗叫「安装」而不是「重启并安装」，' +
        '`MobileSettings.test.tsx ⑥` 正面钉着桌面那四句话不许出现在这一屏上。' +
        '交付的五种结局各有各的码（`REASON_*`），逐码一句话，取文在 `app-update-install.ts`。',
    },
    {
      id: 'AppUpdateCard.tsx|k:settings.update.bannerSkip',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: 'markAppVersionSkipped(foundVersion);',
        },
      },
      note:
        '跳过这个版本。**2026-09-13 接上**：移动端确实没有横幅那一层，但可跳过的对象从来不是横幅 —— 是那个版本号。' +
        '后端 `update_skip` 把它持久化进 updater state，`update_check` 的第四道闸据此过滤（两端同一条腿）；' +
        '会话级那一半走桌面横幅同一个 `markAppVersionSkipped`，故设置根页那行「有新版本」入口同时收声。',
    },
    {
      id: 'AppUpdateCard.tsx|k:settings.update.reinstallCurrent+settings.update.reinstallCurrentTip',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: "commit(\n                  'app-reinstall',",
        },
      },
      note:
        '重装当前版本（修复损坏安装）。**2026-09-13（批 15）接上**：`update_check(includeCurrent:true)` → ' +
        '`reinstallTarget` 选**当前版本**的资产 → 同一个 `updateApi.download` → 交系统安装器。' +
        '桌面那颗的 `data-tip`（「只重新下载并校验当前安装包」）在移动端是常驻第二行（IA §4.12，' +
        '触屏没有 hover）。三条早退（真有新版 / 通道最新版与已装版本对不上 / 那个 release 没有可下载的' +
        '资产）折成同一句 `settings.update.reinstallUnavailable`，与桌面共用那条键。' +
        '🔴 后端那一臂的顺序承重：`includeCurrent` 必须排在 Android 的「只比版本」兜底**之前**，' +
        '否则这条腿在 Android 上永远拿不到目标（`app_update.rs` 那两条 match 臂上写着为什么）。',
    },
    {
      id: 'AppUpdateCard.tsx|k:common.retry',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/UpdatePage.tsx',
          mustContain: "dl.phase === 'error'\n                      ? t('common.retry')",
        },
      },
      note:
        '更新流程失败后的重试。🔴 它重试的是**下载**不是检查（桌面那颗的 `onClick` 就是 `downloadUpdate`，' +
        '外面还套着 `updateInfo &&`）—— **2026-09-13（批 15）随下载腿一起接上**：移动端不另起一颗按钮，' +
        '同一颗下载按钮在 `dl.phase === \'error\'` 时改写成「重试」，对象与桌面逐字相同（同一个 ' +
        '`updateApi.download(target)`）。锚指的是那条改写判据 —— 它要是变成恒显「下载」，本条当场失配。' +
        '⚠️ 检查那条腿的重试仍是同一颗检查按钮再点一次（`k:settings.about.checkUpdate#2`，失败态不禁用）。',
    },

    /* ── TUN 组网网段报告：手动刷新与加载反馈 ─────────────────────────────── */
    {
      id: 'EndpointForceRouteBlock.tsx|k:common.refresh+settings.tun.forceRouteBlock+settings.tun.forceRouteHint+settings.tun.forceRouteLoading',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/TunReports.tsx', mustContain: 'className="ms-force-refresh" onClick={onRefresh} disabled={loading}' },
      },
      note: '桌面报告标题中的刷新动作；移动端同一报告块在加载时禁用按钮，刷新失败保留上一份报告并显式报错。',
    },
    {
      id: 'EndpointForceRouteBlock.tsx|k:common.refresh+settings.tun.forceRouteLoading',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/TunReports.tsx', mustContain: "t('settings.tun.forceRouteLoading')" },
      },
      note: '报告读取中的可见文案：初次加载占位、已有结果刷新时的行内状态共用此键。',
    },

    /* ── 内核更新卡 / 版本横幅（核随 APK 打包，整族没有对象）────────────────── */
    {
      id: 'CoreUpdateCard.tsx|k:settings.coreManagement.checkCoreUpdate',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '检查内核更新。Android 上核是随 APK 打进去的 `libbox.aar`（进程内 .so），没有「独立于应用的内核」这个对象可检查。',
    },
    {
      id: 'CoreUpdateCard.tsx|k:settings.coreManagement.updateNow',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '立即更新内核。同上：换内核 = 装新版应用。',
    },
    {
      id: 'CoreUpdateCard.tsx|k:settings.coreManagement.applyNow',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '应用已下载的内核（`core_swap` 原子替换）。这条腿的对象是一个可替换的可执行文件，Android 上不存在。',
    },
    {
      id: 'CoreUpdateCard.tsx|k:settings.coreManagement.manualSwap',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '手动换核（自选一个 sing-box 二进制换上去）。同上：Android 上没有可替换的可执行文件。',
    },
    {
      id: 'CoreUpdateCard.tsx|k:settings.core.noBackup+settings.core.rollbackConfirm+settings.coreManagement.rollback',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '回滚到上一版内核（依赖换核时留下的备份）。没有换核就没有备份，也没有可回滚的对象。',
    },
    {
      id: 'CoreUpdateCard.tsx|k:settings.coreManagement.resetFactory',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '恢复出厂内核（丢掉全部换核痕迹回到随包版本）。同族，同样没有对象。',
    },
    {
      id: 'CoreVersionBanner.tsx|k:settings.core.swapFailedShort+settings.coreManagement.manualSwap',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '换核失败横幅上的「手动换核」补救键。没有换核这条腿，就没有它要报的那个事件。',
    },
    {
      id: 'CoreVersionBanner.tsx|k:settings.coreVersion.dismiss',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '关掉内核版本变更横幅。没有独立换核这条腿，就没有它要报的那个事件。',
    },

    /* ── 通用清单编辑器（五处复用；整块的处置见 blocks 面 ListEditor 一条）────── */
    {
      id: 'ListEditor.tsx|f:add',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: "<MobileButton onClick={() => commit([...draft, ''])}>{addLabel}</MobileButton>" },
      },
      note:
        '往清单里加一条。移动端的清单编辑器是 `MobileListEditor`（同屏 `SettingsChrome.tsx`，' +
        '2026-09-06 随连入来源排除一起落地），这颗「添加」就在它身上，`TunPage.tsx` 的 ' +
        '`tun-inbound-exclude` 正在用。🔴 上一版写的「移动端一处都没有」说的是**组件不存在**，' +
        '那句话在那一批之后就为假了 —— 今天真正缺的是「哪几张清单还没接到它上面」，' +
        '而那是 `field:browserDohList` / `field:fakeIpFilterList` / `field:bypassLANList` ' +
        '三条各自的账，不是这颗按钮的账。',
    },
    {
      id: 'ListEditor.tsx|k:common.delete',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'onClick={() => commit(draft.filter((_, i) => i !== index))}' },
      },
      note: '删掉清单里的一条。`MobileListEditor` 每行尾那颗叉，与上一条同一个组件。',
    },
    {
      id: 'ListEditor.tsx|f:setImportOpen',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: '<MobileButton onClick={() => setImportOpen((v) => !v)}>{importLabel}</MobileButton>' },
      },
      note: '批量导入（一次粘贴多行）。移动端是就地展开的一块 `textarea`，不另起弹层（本屏没有弹窗宿主）。',
    },
    {
      id: 'ListEditor.tsx|k:common.confirm',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'commit(parseBulkEntries(importDraft, [...draft]));' },
      },
      note:
        '批量导入的确认。解析与去重走 `@/domain/list-entries` 的 `parseBulkEntries` —— ' +
        '与桌面**同一个函数**，不是第二份实现。',
    },
    {
      id: 'ListEditor.tsx|k:common.cancel',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: '{cancelLabel}' },
      },
      note: '批量导入的取消（收起那块 `textarea` 并丢掉草稿）。与上一条同一跳的另一半。',
    },

    /* ── 原语本身（`Primitives.tsx` 在 primitiveModules 里，但它的**动作**照样进面）──── */
    {
      id: 'Primitives.tsx|f:disabled',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'export function MobileSwitch' } },
      note: '开关原语（`role="switch"` 的 `<span>`）。移动端设置屏有自己的 `MobileSwitch`（原生 `<button role="switch">`），命中盒由 `MobileSettings.test.tsx ⑬` 钉着。',
    },
    {
      id: 'Primitives.tsx|f:disabled#2',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'onChange={(e) => onChange(e.currentTarget.value)}' } },
      note: '二选一分段控件（`Seg2`，`role="radio"` 的两颗）。移动端设置屏用共享 MobileSelect 选择面表达枚举，保留原生 select 的表单与 change 语义。',
    },
    {
      id: 'Primitives.tsx|h:837de410',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'export function MobileButton' } },
      note: '通用按钮原语（这一屏 39 处动作全走它）。移动端设置屏有自己的 `MobileButton`；逐颗动作的处置见本面其余各条。',
    },

    /* ── 侧栏（桌面设置屏的导航铬）─────────────────────────────────────────── */
    {
      id: 'SettingsSidebar.tsx|f:setSettingsScreen',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/MobileSettingsScreen.tsx', mustContain: 'onOpen={() => onOpen(id)}' } },
      note: '侧栏里点一个子页。移动端是根页列表的一行，点进去是二级页（同一份 `MOBILE_SETTINGS_PAGES`）。',
    },
    {
      id: 'SettingsSidebar.tsx|f:onClick',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/MobileSettingsScreen.tsx', mustContain: 'onOpen={() => onOpen(id)}' } },
      note: '导航项本体那颗 `<button>`（上一条是它的渲染位）。移动端同一条腿。',
    },
    {
      id: 'SettingsSidebar.tsx|k:settings.nav.back',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'data-back="settings"' } },
      note: '从设置返回上一层。移动端设置屏头上有同一颗返回键（另受返回栈门约束）。',
    },
    {
      id: 'SettingsSidebar.tsx|k:sidebar.collapse+sidebar.expand',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/MobileSettingsScreen.tsx', mustContain: 'onOpen={() => onOpen(id)}' },
      },
      note:
        '折叠 / 展开侧栏。桌面这颗解决的问题是「常驻导航占着宽度 vs 内容占着宽度」，' +
        '它有两个状态是因为导航**恒在场**。移动端的导航不恒在场：根页列表本身就是一个整页，' +
        '点进二级页时它整个让位、系统返回键把它带回来（`MobileSettingsScreen` 的 `onOpen` + ' +
        '返回栈）—— 同一个取舍由导航形态本身表达，没有第二个状态可折叠。' +
        '🔴 与「界面上一个字都没提」的区别在这里：那一档说的是能力缺了而用户看不出来，' +
        '而这里用户每次点进子页都在做同一件事。',
    },
    {
      id: 'SettingsPage.tsx|k:common.retry',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/MobileSettingsScreen.tsx', mustContain: "<MobileButton onClick={() => void reload()}>{t('common.retry')}</MobileButton>" } },
      note: '配置加载失败时的重试。移动端设置屏同一颗，同一条重载腿。',
    },

    /* ── 关于页 ───────────────────────────────────────────────────────────── */
    {
      id: 'SettingsAbout.tsx|c:about-link',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/AboutPage.tsx', mustContain: 'function ExternalRow' } },
      note: '关于页的外链（开源声明 / 发布页 / 反馈）。移动端是 `ExternalRow` 一行一条，落到系统浏览器。',
    },
    {
      id: 'SettingsAbout.tsx|k:settings.about.uninstallAction+settings.about.uninstallConfirmAgain+settings.about.uninstallInProgress',
      disposition: { kind: 'adjudicated-out', enforcedBy: D8_ADJUDICATED },
      note:
        '应用内卸载。陈先生 2026-09-25 裁定放弃（低价值，D8）。🔴 2026-09-25 从 `platform-absent` 改判：' +
        '上一版说「六步编排在这个平台上一步都没有对象」，结论对、标签错 —— 应用可以用 ' +
        '`Intent.ACTION_DELETE`（需 `REQUEST_DELETE_PACKAGES`）请求系统卸载自己，这台设备做得到。' +
        '关于页仍然常驻 `mobileSettings.about.uninstallNote`（去系统的应用管理里卸载），' +
        '`MobileSettings.test.tsx ⑥` 正面钉着它不许被静默丢掉。',
    },

    /* ── 备份页 ───────────────────────────────────────────────────────────── */
    {
      id: 'SettingsBackup.tsx|k:settings.backup.exportSelected',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/BackupPage.tsx', mustContain: 'api.backup.export(' },
      },
      note:
        '导出选中的备份内容。2026-09-06 批 4（W-18）接上：`commands/picked_file.rs` 给了 content URI ' +
        '感知的写腿，SAF 交回的 `FilePath::Url` 不再被 `into_path().ok()` 吃成「用户取消了」。',
    },
    {
      id: 'SettingsBackup.tsx|k:settings.backup.import',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/BackupPage.tsx', mustContain: 'importPick()' },
      },
      note:
        '导入备份。2026-09-06 批 4（W-18）接上：`commands/picked_file.rs` 给了 content URI 感知的读腿，'  +
        'SAF 交回的 `FilePath::Url` 不再被 `into_path().ok()` 吃成「用户取消了」；移动端走两步'  +
        '（importPick → 就地预览类目与条数 → importApply），因为没有弹窗宿主。',
    },

    /* ── 通用页的隐私锁密码 ───────────────────────────────────────────────── */
    {
      id: 'SettingsGeneral.tsx|k:settings.general.privacyPasswordChange+settings.general.privacyPasswordSetBtn',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: 'PrivacyPasswordRow' },
      },
      note:
        '设置 / 修改隐私锁密码。2026-09-06 批 5 接上：通用页的密码行 + `privacy_unlock` 解锁链。' +
        '上一版这里写着「移动端整个隐私锁没有运行时」——那句话现在是假的。',
    },
    {
      id: 'SettingsGeneral.tsx|k:common.save',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: 'PrivacyPasswordRow' },
      },
      note: '密码保存。2026-09-06 批 5 接上：移动端不走弹层，密码行就地提交（`PrivacyPasswordRow`），故与上一条同判。',
    },
    {
      id: 'SettingsGeneral.tsx|k:common.cancel',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: 'PrivacyPasswordRow' },
      },
      note: '密码取消。2026-09-06 批 5 接上：移动端就地行内编辑，取消即收起该行，与保存同一跳。',
    },

    /* ── 提权助手页（移动端没有 helper 进程这个对象）───────────────────────── */
    {
      id: 'SettingsHelper.tsx|k:helper.installAction',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '安装提权助手。移动端的隧道来自 `VpnService`、权限来自系统授权弹窗，没有可安装的助手进程。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.upgradeHelperAction',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '升级助手到与应用匹配的版本。同上，没有被升级的那个对象。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.repairAction',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '修复助手安装（重装 + 重注册守护）。同上，没有被修复的那个对象。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.uninstall+helper.uninstallConfirmAgain',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '卸载提权助手（二次确认）。同上：没有那个进程，也就没有可卸载的东西。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.recheck',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '重新检测助手状态。移动端对应的「授权状态」由 `vpnAuthStatus` 读来并常驻在根页那一行上，不需要人工重检这一跳。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.recheck#2',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '重新检测的第二个渲染位（另一档状态下出现）。同一条腿，一并登记。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.recheck#3',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '重新检测的第三个渲染位（另一档状态下出现）。同上，同一条腿。',
    },
    {
      id: 'SettingsHelper.tsx|k:helper.btmCopyPath',
      disposition: { kind: 'platform-absent', evidence: HELPER_IS_VPN_SERVICE },
      note: '复制助手守护进程的路径（排障用）。没有那个进程，也就没有路径可复制。',
    },

    /* ── 网络页 ───────────────────────────────────────────────────────────── */
    {
      id: 'SettingsNetwork.tsx|f:onCopy',
      disposition: { kind: 'adjudicated-out', enforcedBy: TERMINAL_ENV_ADJUDICATED },
      note:
        '终端代理环境变量的**整组复制**。要复制走的字面量是 `http_proxy=http://127.0.0.1:<mixedPort>`，' +
        '而 Android 上按安全裁定不发 mixed inbound（`inbounds.rs` 的 mixed 段）⇒ 那个端口没有任何东西在监听。' +
        '🔴 2026-09-25 从 `platform-absent` 改判（盘点 §4.1「标签不准」）：不发 mixed 是我们的安全决定，' +
        '不是这台设备做不到 —— 标签要表达那个事实。',
    },
    {
      id: 'SettingsNetwork.tsx|c:term-copy',
      disposition: { kind: 'adjudicated-out', enforcedBy: TERMINAL_ENV_ADJUDICATED },
      note: '终端代理环境变量的**逐行复制**。与上一条同一块（`TerminalEnvBlock`）的另一颗，同一条安全裁定（2026-09-25 改判）。',
    },
    {
      id: 'SettingsNetwork.tsx|k:proxy.clear',
      disposition: { kind: 'platform-absent', evidence: NO_MIXED_INBOUND },
      note: '清理系统代理残留（桌面上进程被杀后系统代理设置可能留着，指向一个已经没人听的端口）。Android 上根本不发 mixed inbound、也没有「系统代理设置」这个对象 —— 没有可残留的东西可清。',
    },
    {
      id: 'SettingsNetwork.tsx|k:common.hideSecret+common.showSecret+settings.network.secretToggleTip',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '显示 / 隐藏管理 API 密钥。裁定 #9 那一族：移动端不开这个口子，`MobileSettings.test.tsx ③` 正面钉着 `clashApiSecret` 在源码面一处都不许出现。',
    },
    {
      id: 'SettingsNetwork.tsx|k:common.copy+settings.network.copySecret',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '复制管理 API 密钥到剪贴板。同族同裁定，源码面同样零处。',
    },
    {
      id: 'SettingsNetwork.tsx|k:settings.network.secretRegenerateTip',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '重新生成管理 API 密钥。同族同裁定，源码面同样零处。',
    },
    {
      id: 'SettingsNetwork.tsx|k:settings.advanced.openDashboard',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '打开内置 sing-box 面板（由管理 API 提供服务）。同族同裁定，且移动端连那个独立窗口都没有。',
    },
    {
      id: 'SettingsNetwork.tsx|k:settings.network.copyDashboardConnection',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '复制面板连接信息（地址 + 密钥）。同族同裁定，那两格在移动端都不存在。',
    },
    {
      id: 'SettingsNetwork.tsx|k:settings.network.refreshDashboard',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '刷新内置面板窗口。同族同裁定，移动端连那个窗口都没有。',
    },

    /* ── TUN 页 ───────────────────────────────────────────────────────────── */
    {
      id: 'SettingsTun.tsx|k:settings.tun.switchOnHome',
      disposition: { kind: 'platform-absent', evidence: NO_MIXED_INBOUND },
      note: '「去首页切换接管方式」那条链接。Android 上接管方式恒为 TUN（不发 mixed inbound），首页那一维整个不存在，链接自然也没有落点。',
    },
    {
      id: 'SettingsTun.tsx|k:settings.general.enableIPv6+settings.network.enableFakeIpAction+settings.network.enableIPv6Desc+settings.network.ipv6NodeFakeIpHint',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/DnsPage.tsx', mustContain: 'toggleFakeIp' } },
      note: 'IPv6 那一行（整行可点，附带「节点走 FakeIP 时的注意」提示与一颗就地启用 FakeIP 的动作）。移动端把 IPv6 开关放在网络页、FakeIP 开关放在 DNS 页，两颗都在，只是不合并在一行里。',
    },
    {
      id: 'SettingsTun.tsx|k:settings.network.enableFakeIpAction',
      disposition: { kind: 'reachable-elsewhere', mobile: { file: 'ui/src/mobile/settings/DnsPage.tsx', mustContain: "<MobileSwitch checked={dns.enableFakeIp}" } },
      note: '上一行里那颗「就地启用 FakeIP」的快捷动作。移动端不做这条捷径，能力本体是 DNS 页上的 FakeIP 开关。',
    },

    /* ── 自定义下拉（渲染一跳；设置屏用它做枚举选择）─────────────────────────── */
    {
      id: 'Csel.tsx|f:preventDefault',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'onChange={(e) => onChange(e.currentTarget.value)}' } },
      note: '自定义下拉的触发器。移动端设置屏统一用共享 MobileSelect：可见触发器与应用风格选择面，原生 select 保持表单与 change 语义。',
    },
    {
      id: 'Csel.tsx|f:choose',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/UpdatePage.tsx', mustContain: "<option value=\"0\">{t('settings.update.intervalManualOnly')}</option>" } },
      note: '在下拉里选中一项。移动端由 MobileSelect 选择面触发原生 `<option>` 的 change 语义。',
    },
    {
      id: 'Csel.tsx|f:header',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/settings/DnsPage.tsx',
          // 锚指**「可折叠」这一格本身**：`id` 是 `CselGroup` 里唯一决定组头折不折的字段
          // （`csel-logic.ts` 的 `CselGroup.id`：带 id ⇒ 组头变成可点的展开钮，省略 ⇒ 恒展开）。
          // 有人把这一行改成恒等映射，组头当场退回纯视觉分隔，而本锚同时失配。
          mustContain:
            'function collapsibleGroups(groups: readonly CselGroup[]): CselGroup[] {\n  return groups.map((group) => ({ ...group, id: group.label }));',
        },
      },
      note:
        '下拉里**可折叠的分组组头**。2026-09-13（批 17）接通：DNS 页的 v2 默认策略两格' +
        '（`DnsDefaultsRows`）是设置屏第一颗吃分组候选表的选择器 —— 候选由共享的' +
        '`buildDnsActionGroups` 出（DNS 服务器组 / 服务器 / Hosts / 响应动作四组），' +
        '经 `collapsibleGroups` 配上稳定键后交给 `SelectSheetPanel`，组头即可折叠 + 带组内计数。' +
        '🔴 **组头的渲染实现一份都没有新写**：折叠行序走全仓同一个 `buildCselRows`' +
        '（`dialogs/csel-logic.ts`），组件直接复用规则屏那一份 `SelectSheetPanel`' +
        '（`mobile/screens/rules/Primitives.tsx`，批 10 落的）—— 本屏是它的第三个消费点' +
        '（前两个是规则屏自己与 `mobile/forms/RuleFormPanel.tsx`）。' +
        '不复用连接屏那一份（`ConnectionsView.tsx`，批 13）：它吃的是连接屏自己的' +
        '`SheetVM`/`SheetGroup`，且住在 `ActionSheet`（命令语义、无选中态）里；而默认策略是' +
        '**选择**，要的是带勾的 `select-sheet`，且 `SelectSheetPanel` 的入参**就是** `CselGroup[]`' +
        '—— 与 `buildDnsActionGroups` 的返回类型逐字相同，零形状搬运。' +
        '⚠️ 本屏其余下拉是共享 MobileSelect（`SettingsChrome#MobileSelect`）：那些喂的是' +
        '三五档定值，折叠对它们没有意义。这条登记问的是「这个能力在本屏有没有」，不是「有几处在用」。' +
        '🔴 2026-09-13 曾判成 `reachable-elsewhere`（锚指节点表单的 `<optgroup>`），**已撤回过一次**：' +
        '那个锚在**另一个屏**上，且只够得着「分组」、够不着「折叠」。这次的锚在本屏、且指着' +
        '「折叠」那一格本身。',
    },

    /* ══════════════ `config-fields` 面：桌面这一屏 `update({…})` 写出去的每个字段 ══════════════ */

    /* ── 计算键与嵌套子字段（上一版的正则一条都抓不到）───────────────────────── */
    {
      id: 'field:[key]',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '`SettingsNetwork.tsx` 的 `commitPort` 那个漏斗：一个调用点用计算键写 `mixedPort` / `controlPort` 两个字段。键名要到运行期才知道，故它作为一条**必须显式判一次**的登记进面。两个字段在移动端都不存在（Android 不发 mixed inbound、管理 API 整族裁定不提供），`MobileSettings.test.tsx ③` 在源码面与文案面各钉一次。',
    },
    {
      id: 'field:dnsConfig.enableFakeIp',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/DnsPage.tsx', mustContain: "patchDns('fake-ip', fakeIpTogglePatch(next))" } },
      note: 'FakeIP 总开关（桌面写在 `update({ dnsConfig: { …, enableFakeIp } })` 里，是**第二层**的键）。移动端经同一个共享补丁函数 `fakeIpTogglePatch` 写同两格 —— 换了调用形态，不是换了腿。',
    },
    {
      id: 'field:dnsConfig.fakeIpTunAutoEnable',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/DnsPage.tsx', mustContain: 'fakeIpTogglePatch,' } },
      note: '关掉 FakeIP 时一并清掉「TUN 打开时自动启用」这一格（两格恒同写，判据收在 `fakeIpTogglePatch` 一个函数里）。移动端 import 的就是那个函数。',
    },
    /*
     * 🔴 这里**刻意没有** `field:dnsDefaults` / `field:dnsDefaults.unmatchedAction` 两条
     * （2026-09-13 / 批 17 销账后删除，不是漏了）。
     *
     * 那两格（默认服务器 + 未命中默认动作）现在真的改得动：`DnsPage.tsx#DnsDefaultsRows` 写
     * `update({ dnsConfig: …, dnsDefaults: { ...defaults, unmatchedAction } })`。
     * 本面的 `ported` 由门自己算（移动端设置树里出现同一个字段的写腿即已接），故登记必须同批删掉
     * —— 留一条 `absent` 在这里会被僵尸检测当场判红，而那正是「接上了却还在账上说没接」的形态。
     */

    /* ── 平台上没有这个对象（8 条）───────────────────────────────────────── */
    /*
     * 🔴 这里**刻意没有** `field:autoStart`（2026-09-25 删除，不是漏了）。
     *
     * 旧登记判它 `platform-absent`，理由写「Android 的对应物是 always-on VPN，应用侧既无权注册开机自启」——
     * 后半句不成立（`RECEIVE_BOOT_COMPLETED` 是 normal 权限），前半句也只在「always-on 真能起核」时成立，
     * 而那条路当时**一定**失败（系统拉起服务时配置为 null）。现在两件都兑现了：always-on 能起核
     * （`vpn/SystemStart.kt`），应用自带「开机自动连接」开关写 `update({ autoStart })`
     * （`GeneralPage.tsx`，执行侧 `vpn/BootReceiver.kt`）。本面的 `ported` 由门自己算，故整条删掉。
     */
    {
      id: 'field:silentStart',
      disposition: { kind: 'platform-absent', evidence: NO_TRAY },
      note: '静默启动 = 主窗建成即隐藏、只驻托盘等唤出。没有托盘就没有「隐藏之后还回得来」这个前提，这一档在移动端不是「关着」，是没有这个状态。',
    },
    {
      id: 'field:minimizeToTray',
      disposition: { kind: 'platform-absent', evidence: NO_TRAY },
      note: '关窗时最小化到托盘。移动端没有托盘，也没有「关窗」这个动作（返回键交给系统）。',
    },
    {
      id: 'field:keepTrayMenuWarm',
      disposition: { kind: 'platform-absent', evidence: NO_TRAY },
      note: '托盘菜单常驻预热。同上，连被预热的那个对象都不存在。',
    },
    {
      id: 'field:rememberWindowSize',
      disposition: {
        kind: 'platform-absent',
        // 同上：锚指 `#[cfg(desktop)]` 底下那条判断，不是解释它的注释。
        evidence: { file: 'src-tauri/src/lib.rs', mustContain: '#[cfg(desktop)]\n            if config_remember_window_size(raw_config.as_deref()) {' },
      },
      note: '记忆窗口大小。Activity 的尺寸与位置由系统、折叠态与分屏决定，应用无从「恢复」它；window-state 插件也无 android 实现，整块不编译。',
    },
    {
      id: 'field:windowEffects',
      disposition: {
        kind: 'platform-absent',
        // 锚指 `#[cfg(target_os = "windows")]` 臂里那条 Mica 调用（vibrancy 那一臂同形）。
        evidence: { file: 'src-tauri/src/lib.rs', mustContain: '#[cfg(target_os = \"windows\")]\n    {\n        if !apply_effects {' },
      },
      note: '窗口特效（mac vibrancy / Windows Mica）。它是建窗期的 per-platform 窗口铬，Android 上没有可加特效的窗口铬这一层。',
    },
    {
      id: 'field:autoUpdateCore',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '自动更新内核。Android 上核是随 APK 打进去的 `libbox.aar`（进程内 .so，不是可执行文件），桌面那套 `core_swap` 原子替换在这个形态下没有对象 —— 换内核 = 装新版应用。',
    },
    {
      id: 'field:coreUpdateChannel',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '内核更新通道（稳定 / 预发布）。没有独立的内核更新，就没有通道可选。',
    },
    {
      id: 'field:restrictCoreUpdateToCompatibleMinor',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '内核更新限制在兼容小版本内。同上，它约束的那条更新腿不存在。',
    },

    /* ── 裁定要求不提供（3 条，管理 API 那一族）───────────────────────────── */
    {
      id: 'field:allowLan',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '允许局域网访问管理 API。裁定 #9 / §4.7 / §4.16：移动端不开这个口子（手机常年挂在不可信的 Wi-Fi 上，而管理 API 能改全部路由）。',
    },
    {
      id: 'field:clashApiSecret',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '管理 API 访问密钥。它只在管理 API 对外开放时才有意义，与上一条同进同出。',
    },
    {
      id: 'field:singboxDashboard',
      disposition: { kind: 'adjudicated-out', enforcedBy: MGMT_API_ADJUDICATED },
      note: '内置 sing-box 面板（由管理 API 提供服务、桌面用一个独立窗口装）。同一族，且移动端连那个独立窗口都没有。',
    },

    /* ── 真的欠着的（9 条）─────────────────────────────────────────────────── */
    /* 2026-09-06:`field:autoPrivacyMode` 整条摘除。批 5 把隐私锁接上了,
       而这一面的 `ported` **由门自己算**(移动端写了同一个字段就算接上),
       表里只登记缺口 —— 留一条 `kind:'ported'` 反而会被僵尸检测判红。 */
    /* 2026-09-13 把 `field:appUpdateChannel` 整条摘除：更新页接上了通道 `<Select>`
       （`UpdatePage.tsx` 的 `update({ appUpdateChannel: … })`），而这一面的 `ported`
       **由门自己算** —— 留一条 `kind:'ported'` 反而会被僵尸检测判红。
       这一格此前是「配置里在、界面上看不见」的原样形态：`appUpdateIncludePrerelease(config)`
       早就被本页那颗检查按钮读着，手机上却没有任何控件能改它。 */
    /* 2026-09-13（批 15）`field:autoDownloadUpdate` 整条摘除：应用内下载那条腿接上了，
       于是「要不要自动下载」才有了对象，更新页也真的写了那个字段
       （`UpdatePage.tsx` 的 `update({ autoDownloadUpdate: v })`）。这一面的 `ported`
       **由门自己算**（移动端设置树里出现 `update({ <同一个字段>` 就算接了），表里只登记缺口，
       故留一条 `kind:'ported'` 反而会被僵尸检测判红。
       ⚠️ 那颗开关**不是装饰**：后端 `startup_tasks::spawn_auto_download` 在 Android 上照跑
       （`auto_download_applicable` 经 `decide_install_plan` 认 `.apk`），下好的包由更新页
       经 `update:progress` 的快照回读看见，出一颗「安装」。 */
    /*
     * 🔴 `field:browserDohList` 与 `field:fakeIpFilterList` 两条 2026-09-13 **从表里删掉**
     * （批 10 / 规则屏顺带接的）。
     *
     * 删而不是改判 `ported`：`config-fields` 这一面的口径是「**表里只登记缺口**，已接的那些由门
     * 自己算」（门按「移动端有没有写这个 config 字段」判）。两条清单现在真的改得动 ——
     * `DnsPage.tsx` 在各自总开关打开时渲染 `MobileListEditor`（去重与草稿规则读
     * `@/domain/list-entries`，与桌面同一份），于是它们留在表里就成了僵尸登记，
     * 而门有一条正面断言专门抓这个形态。
     *
     * ⚠️ **本屏的登记表由另一批次持有**，本次只删这两条，其余一行未碰。
     */
    /*
     * 🔴 `field:bypassLAN` 与 `field:bypassLANList` 两条 2026-09-25 **从表里删掉**（A6）。
     *
     * 它们记的是 platform-absent，锚指 `inbounds.rs` 的 `deps.platform == "win32" && should_bypass_lan`
     * —— 那是本仓的选择，不是平台限制：android 臂恒空的依据只有「`excludeRoute` 拒收回环前缀」
     * 这一条模拟器实证，它不覆盖 RFC1918（「连入来源排除」那条腿早已在 Android 上把非回环段
     * 发进 `route_exclude_address`）。现在 android / ios 臂在开关开着时发清单网段子集
     * （`mobile_bypass_lan_exclude`），`TunPage.tsx` 写这两个字段 ⇒ 门自己算成已接，留着就是僵尸登记。
     * 删而不改判 `ported`，口径同上面 `browserDohList` / `fakeIpFilterList` 那两条。
     */
    {
      id: 'field:hardwareAcceleration',
      disposition: { kind: 'adjudicated-out', enforcedBy: D8_ADJUDICATED },
      note:
        '硬件加速（关掉它 = 走软件渲染的图形逃生门）。陈先生 2026-09-25 裁定放弃（低价值，D8）。' +
        '🔴 从 `platform-absent` 改判：本仓的逃生门实现（`graphics_compat.rs` 三条 `#[cfg(target_os)]` 臂）' +
        '确实不覆盖 Android，但 Android 上可以用 WebView `setLayerType(LAYER_TYPE_SOFTWARE)` 做同一件事 —— ' +
        '这台设备做得到，缺的是价值（没有真机 WebView GPU 故障样本）。',
    },
    {
      id: 'field:autoLightweightMode',
      disposition: { kind: 'adjudicated-out', enforcedBy: D8_ADJUDICATED },
      note:
        '闲置自动进轻量模式（隐藏 / 最小化 10 分钟后销毁主 WebView）。陈先生 2026-09-25 裁定放弃（低价值，D8）。' +
        '🔴 从 `platform-absent` 改判：上一版的理由「没有托盘就没有回程」不成立 —— 前台通知的 ' +
        'contentIntent 就是回程（`BoxService.kt`）。真正的问题是 WebView 与 libbox 同进程、后台内存' +
        '是否值得回收，缺实测数据；按裁定不做。',
    },
  ],

  blocks: [
    /* ── 九张子页：与 `MobileSettings.test.tsx ①` 同一份真值源，本门另有正面断言把交接钉死 ── */
    {
      id: 'SettingsGeneral',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "general: 'settings.general.pageTitle'" },
      },
      note: '通用页（启动、连接、日志那一批）。逐条配置字段的处置见上面的 actions 面 —— 子页在不等于页里每一行都在。',
    },
    {
      id: 'SettingsDisplay',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "display: 'settings.nav.appearance'" },
      },
      note: '外观页（主题、语言、通知）。同上：页在，页里 `autoLightweightMode` 等几行的账另记在 actions 面。',
    },
    {
      id: 'SettingsNetwork',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "network: 'settings.nav.network'" },
      },
      note: '网络页（测速、分流开关、网卡、TLS 分片那一批）。管理 API 那一块不在移动端，逐条见 actions 面。',
    },
    {
      id: 'SettingsDns',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "dns: 'DNS'" },
      },
      note: 'DNS 页。只承载全局运行时设置，规则那一半住在规则屏的 DNS 分段 —— 两端同一条切分，不是移动端把它切碎了。',
    },
    {
      id: 'SettingsTun',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "tun: 'TUN'" },
      },
      note: 'TUN 页（栈、MTU、NAT、路由排除那一批）。`strict_route` 那一行两端都不显示，理由在 MobileSettings.test.tsx ④。',
    },
    {
      id: 'SettingsUpdate',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "update: 'settings.nav.update'" },
      },
      note: '更新页。页在，但桌面那两张更新卡在移动端都不是可操作卡 —— 逐张的处置见下面 AppUpdateCard / CoreUpdateCard 两条。',
    },
    {
      id: 'SettingsBackup',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "backup: 'settings.nav.backup'" },
      },
      note: '备份页。页在，导出那两颗按钮今天是在场置灰（`mobileSettings.backup.pickerPending`，账已记在 i18n 与控件两面上）。',
    },
    {
      id: 'SettingsAbout',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "about: 'settings.nav.about'" },
      },
      note: '关于页。许可文案按平台分叉（Android 产物 = GPLv3），那一格由 MobileSettings.test.tsx ⑥ 守。',
    },
    {
      id: 'SettingsHelper',
      disposition: {
        kind: 'platform-absent',
        evidence: {
          file: 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt',
          mustContain: 'fun vpnAuthStatus(invoke: Invoke)',
        },
      },
      note: '提权助手页。移动端没有 helper 进程：隧道来自 `VpnService`，权限来自系统授权弹窗 —— 锚指的就是那个替代对象的后端腿。它的替代不是「助手页的移动版」，而是根页上一行 VPN 授权（回答同一个用户问题、对着完全不同的对象）。',
    },

    /* ── 四个非子页的块 ──────────────────────────────────────────────────── */
    {
      id: 'AppUpdateCard',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/UpdatePage.tsx', mustContain: "id=\"app-update\"" },
      },
      note:
        '应用更新卡。移动端的形态是更新页顶上那一行（版本芯片 + 检查 + 结果提示 + 查到新版本时的' +
        '「打开发布页」/「跳过此版本」）加下面一行更新通道。**三阶段里只少了下载与安装那两段**，' +
        '而那两段在这个平台上没有可下载的资产（逐颗处置见 actions 面 `k:settings.update.download` ' +
        '一族）—— 不是这张卡没移植。',
    },
    {
      id: 'CoreUpdateCard',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '内核更新卡（分级更新 / 回滚 / 恢复出厂）。核随 APK 打包，这张卡的三件事在这个形态下都没有对象。',
    },
    {
      id: 'CoreVersionBanner',
      disposition: { kind: 'platform-absent', evidence: CORE_IS_BUNDLED },
      note: '内核版本变更横幅（换核之后提示重启生效）。没有独立换核这条腿，就没有它要报的那个事件。',
    },
    {
      id: 'ListEditor',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'export function MobileListEditor(' },
      },
      note:
        '通用清单编辑器。移动端对位是 `MobileListEditor`（添加 / 删除 / 批量导入 / 确认 / 取消五条腿齐，' +
        '逐颗见 actions 面），去重与草稿三条纯函数走 `@/domain/list-entries`，与桌面同一份。' +
        '**组件在，不等于每张清单都接到了它上面**：DoH 域名与 FakeIP 过滤两张的账记在 ' +
        '`field:browserDohList` / `field:fakeIpFilterList`，绕过网段那张记在 `field:bypassLANList`，' +
        'MAC 过滤与邻居域名整块两端同缺（局域网网关只有 Linux/macOS 有实现，见 `TunPage.tsx` 头注）。',
    },
    /* ── 定义在别的文件**内部**的块（上一版按文件名取块时整批不可见）───────────── */
    {
      id: 'TerminalEnvBlock',
      disposition: { kind: 'adjudicated-out', enforcedBy: TERMINAL_ENV_ADJUDICATED },
      note:
        '终端代理环境变量块（逐行 `http_proxy=…` + 逐行复制 + 整组复制）。那几行的端口来自 ' +
        '`localProxyPort(config)`，指向本机 mixed inbound，而 Android 上按安全裁定不发 mixed inbound ' +
        '（`inbounds.rs` 的 mixed 段：共享回环 + 零认证，被按应用排除的应用能借道）。' +
        '在手机上画这块 = 递给用户一串指向无人监听端口的 export。' +
        '🔴 2026-09-25 从 `platform-absent` 改判 `adjudicated-out`：主因是裁定，不是平台。',
    },
    {
      id: 'CopyIcon',
      disposition: { kind: 'adjudicated-out', enforcedBy: TERMINAL_ENV_ADJUDICATED },
      note: '复制图标（这一屏只有终端环境变量块用它）。随上一块同判（2026-09-25 改判）；单独登记是因为块面按组件名成条，它哪天被别处复用时这条会自己变成僵尸、当场红。',
    },
    /* ── 桌面「多 VPN 兼容」批（2026-09-12 合入 main）新加的两块只读报告 ───────────── */
    {
      id: 'TunnelConflictBlock',
      disposition: {
        kind: 'platform-absent',
        evidence: {
          file: 'crates/system-integration/src/route_probe.rs',
          mustContain: 'Platform::Android | Platform::Ios => Ok(TunnelProbeOutcome::Unsupported(self.platform))',
        },
      },
      note:
        'Android/iOS 的外来隧道路由探测恒返回 Unsupported。用户裁定移动端不显示恒无能力报告；' +
        '不是 Probed/无冲突，也不再发这笔无意义请求。桌面保留完整四态。',
    },
    {
      id: 'Warn',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/TunReports.tsx', mustContain: 'className="ms-force-feedback error" role="alert"' },
      },
      note:
        '告警行原语。移动端在报告手动刷新失败后用 role=alert 的行内错误说明；' +
        '桌面平台告警仍只在桌面显示，移动端不把不支持的路由探测说成无冲突。',
    },
    {
      id: 'EndpointForceRouteBlock',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/TunReports.tsx', mustContain: 'export function MobileMeshRouteBlock(' },
      },
      note:
        'TUN 组网网段报告使用共享的快照作用域、来源、有效集合和先行遮盖证据；' +
        '空集合与未知分开，草稿和加载未确认时不宣称当前生效。节点卡片和规则页复用同一详情。',
    },
    {
      id: 'Block',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/settings/TunReports.tsx', mustContain: 'summary={t(meshRouteSummaryKey(report, undefined, previous, legacy))}' },
      },
      note:
        '移动端块壳由 SettingsGroup 承载，概要随证据状态而变，详细限定语与来源由同一枚 i 展开。',
    },
    {
      id: 'AboutExternalAction',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/AboutPage.tsx', mustContain: 'function ExternalRow' } },
      note: '关于页的外链动作块（图标 + 文案 + 落到系统浏览器）。移动端是 `ExternalRow`，同一件事。',
    },
    {
      id: 'SetNavItem',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/MobileSettingsScreen.tsx', mustContain: 'onOpen={() => onOpen(id)}' } },
      note: '侧栏的一个导航项。移动端是根页列表的一行，读的是同一份 `MOBILE_SETTINGS_PAGES`（⑥ 组那条恰等断言钉着这个交接）。',
    },
    {
      id: 'GroupHeader',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'export function SettingsGroup' } },
      note: '侧栏的分组标题。移动端根页与各二级页用 `SettingsGroup` 的 `header` 承担同一件事。',
    },
  ],

  /**
   * 桌面设置屏的具名数据槽基线（69 个）。**变更探测器，不是对差面**。
   * 桌面加一行带 id 的设置槽 ⇒ 这条不等 ⇒ 红 ⇒ 逼一次「移动端要不要有」的显式决定。
   */
  slotBaseline: [
    'app-update-card', 'auto-core-swt', 'auto-dl-swt', 'backup-block', 'backup-master',
    'browser-doh-list', 'browser-doh-swt', 'bypass-lan-swt', 'cidr-bypass-off-note', 'cidr-list',
    'control-port-input', 'core-ver-banner', 'disable-log-file-swt', 'dns-bootstrap-row',
    'dns-connection-resolution', 'dns-custom-list', 'dns-input-domestic', 'dns-input-remote',
    'dns-optimistic-swt', 'dns-preset-domestic', 'dns-preset-remote', 'dns-single-resolver',
    'dns-timeout-input', 'fakeip-filter-list', 'fakeip-filter-swt', 'fakeip-swt',
    'fold-browser-doh', 'fold-bypass', 'fold-env-others', 'fold-fakeip-filter',
    'fold-inbound-exclude', 'fold-mac-filter', 'fold-neighbor-domains', 'fold-route-exclude',
    'gh-custom-input', 'gh-mirror-sel', 'hc-btm-btn', 'hc-btm-desc', 'hc-btm-title',
    'hc-daemon-desc', 'hc-daemon-name', 'helper-card', 'inbound-cidr-list', 'inbound-lin-warn',
    'ipv6-swt', 'le-bypass', 'mac-filter-list', 'mac-filter-mode', 'main-session-via-proxy-swt',
    'mgmt-block', 'mixed-port-input', 'neighbor-domain-list', 'race-doh-count', 'race-ups',
    'res-auto-swt', 'set-graphics', 'set-hwaccel', 'set-lan-gateway', 'set-mac-filter',
    'set-window-effects', 'speed-test-url-input', 'term-env', 'tun-anchor-tx', 'tun-mtu',
    'tun-nat-type', 'webrtc-note', 'webrtc-row', 'webrtc-seg',
  ],
};
