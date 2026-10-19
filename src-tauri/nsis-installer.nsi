# Polaris Windows 安装器配置（B10 发布工程）
#
# ⚠️ 本文件是**纯说明文档**，不含任何 NSIS 代码，也不会被 makensis 读取（扩展名 `.nsi` 属历史遗留，
# 易被误认为是脚本）。真正被注入构建的是 `nsis-hooks.nsh`（经 tauri.conf.json 的
# `bundle.windows.nsis.installerHooks`）。
#
# 本文件说明 Windows 侧 NSIS 与 WebView2 策略（§I-Q2 + §E.2）。Tauri 2 不需要手写完整 .nsi 脚本
# （官方 NSIS 模板已覆盖通用 webview 应用流程），而是通过 tauri.conf.json 的 bundle.windows.nsis
# 字段配置；需要深度定制时再经 installerHooks / template 注入。
#
# === installerHooks（2026-08-05 起启用）===
# `nsis-hooks.nsh` 实现四条窄钩子。`NSIS_HOOK_PREINSTALL` 与 `NSIS_HOOK_PREUNINSTALL` 在动安装目录的
# 文件之前，先让主程序退出，再结束映像路径恰为 `$INSTDIR` 之内两处已知位置的 `sing-box.exe`（内核直接
# 从安装目录运行，活着的内核会占住自己的映像文件；判据是完整映像路径，不按进程名结束；应用侧另有
# 作业对象在主程序退出时结束内核，这里是兜底）。`NSIS_HOOK_PREINSTALL`
# 随后在复制新文件前删除旧安装包遗留的
# `$INSTDIR\resources`（当前权威资源在 `$INSTDIR\_up_\resources`）；`NSIS_HOOK_POSTINSTALL` 在安装成功后
# 删除仅属于 portable zip 的 `$INSTDIR\portable.marker`，避免覆盖便携目录时把 NSIS 安装版继续误判为
# 便携版；`NSIS_HOOK_POSTUNINSTALL` 在真卸载（非 `/UPDATE`）时提权清理运行期外置的 `PolarisHelper`
# 服务与 `C:\ProgramData\Polaris`。后两样不在 NSIS 安装清单里，默认卸载器管不到，不补则控制面板
# 卸载后残留孤儿 LocalSystem 服务。用户数据不在这些钩子范围内 —— Tauri 模板自带的「删除应用数据」
# 复选框已覆盖 `%APPDATA%\com.polaris.app` 与
# `%LOCALAPPDATA%\com.polaris.app`。
#
# === WebView2（§E.2）===
# CI 只产一个 polaris-{version}-{arch}-setup.exe，使用 tauri.conf.json 的 DownloadBootstrapper。
# Polaris 不内嵌或分发 WebView2 Runtime。普通 Win10/11 通常已预装；精简版 / LTSC 缺失时，
# 安装器需要联网获取微软 Runtime。便携用户需先安装微软官方 Runtime，README 给出官方下载入口。
#
# === 不签名（§I-Q1 用户定调）===
# Windows 无代码签名（沿 上游 现状）。后果保留、不可删：
#   - SmartScreen 「Windows 已保护你的电脑」提示首次运行 → 用户点「更多信息 → 仍要运行」。
#   - UAC 提权流（helper 安装 / TUN）照常触发，未经 Authenticode 签名只多一次确认。
#   - updater 自定义安装脚本（§B.5 updater crate）不依赖签名清单信任模型（故不用 tauri-plugin-updater）。
# signingIdentity=null / certificateThumbprint=null / digestAlgorithm=sha256（仅 hashing，不签名）已配。
#
# === installMode: currentUser（2026-09-16 复核后维持；这一行必须显式存在）===
# per-user 安装到 `%LOCALAPPDATA%\Polaris`（`.207` 当前 Tauri 2 真机路径）：**不需要管理员就能装**，
# 不污染 Program Files。helper 与 TUN 提权仍走运行期 UAC 弹窗（独立于安装动作），与上游一致。
#
# 选它是产品决策 ——「标准用户（无管理员权限）也必须能装上 App」。这不意味着无需管理员
# 就能安装/升级 LocalSystem helper：后者由 App 的运行期 UAC 路径单独处理，不由 NSIS installMode 放行。
# helper 安装参数包含私有暂存路径、实际 helper/manifest hash、core id 与原生用户身份；
# 提权脚本先封存并核对字节，校验安装选择和现有 helper 策略，必要时经 prepare-upgrade 准入，
# 再停止/替换服务（crates/helper-client/src/manager/install_windows.rs）。不能称为「拷贝前零校验」。
# 这些校验不等于 Authenticode 签名，也不证明同账户可写的整个 App 未被替换；这仍是 per-user
# App 目录的信任边界，不能把安装阶段的字节校验宣传为来源签名。
#
# 为什么不是「改 perMachine 就完了」：`perMachine` 会把 app 装进 `%PROGRAMFILES%\Polaris`
# （默认 ACL 对 Users 只读，改变 App 目录的写入边界），但模板给 per-machine 的落点**只有** `$PROGRAMFILES64` /
# `$PROGRAMFILES` 两支（tauri-cli 2.11.4 `installer.nsi` 的 `.onInit`），没有任何一支落在标准用户
# 可写的位置，且它同时发 `RequestExecutionLevel admin`。于是**强制** per-machine 与「标准用户能自己装」
# 结构性互斥 —— 而「标准用户必须能装上」是产品硬要求，所以强制 per-machine 出局。
#
# 第三个取值 `both`（让用户在安装时选「只给我」/「所有用户」）正在评估中，**尚未选用**：
# 它的模板走 MultiUser.nsh、发的是 `RequestExecutionLevel highest` 而不是 `admin`，所以不与上面那条
# 互斥；但它的存量检测、更新路径、卸载权限与本文件钩子的上下文解析都要逐条核过才能下结论。
# 在那份调查落地之前，本行保持 `currentUser`，残留按上面登记。
#
# 本行的**显式存在**由 `scripts/verify-packaging.mjs` 的 `checkWindowsInstallMode` 持有：缺键会静默
# 回落到 Tauri 的默认值（恰好也是 currentUser），产物一模一样，但「选过」与「没人想过」不一样。
#
# 🔮 若将来改 `perMachine`，随之而来的形态变化（已核过，别重推一遍）：app 变 all-user（快捷方式落
# All Users 开始菜单 / 公共桌面，卸载项写 HKLM）；安装/卸载各需一次管理员授权；应用内更新跑 setup
# 时会多一次 UAC；**用户数据仍在各自的 per-user 目录**（`%APPDATA%\com.polaris.app` /
# `%LOCALAPPDATA%\com.polaris.app`），不随装机形态迁移；而且那是**安装形态变更而非原地升级** ——
# 旧的 per-user 安装不会被新安装器发现或卸载，两份并存。完整清单与出处登记在 `nsis-hooks.nsh`
# 顶部的两节前瞻登记里。
#
# === portable 形态（§I-Q4）===
# 上游 支持 portable exe + 专属更新逻辑（§C #40/#33）。Tauri 2 的 NSIS 无原生 portable 产物，
# 由 CI 单独产一个 zip（解压即用的目录形态，绕过安装器，便携盘/U盘场景）。portable 启动检测
# WebView2 缺失时需安装微软官方 Runtime；Polaris 不提供离线 Runtime 或第二安装器。
#
# 语言：English / 简体中文 / 繁体中文 / Russian / Farsi；`displayLanguageSelector=true` 首装让用户选，
# 系统语言命中时默认预选，未命中回退首项 English。Farsi 的 Tauri 自定义消息在
# `nsis-languages/Farsi.nsh`，NSIS 3.11 自带 Farsi.nlf（LCID 1065、RTL）；不要改为错误的 Persian token。
