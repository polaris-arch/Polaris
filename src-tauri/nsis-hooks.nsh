!define POLARIS_NATIVE_CLEANER_PAYLOAD "${__FILEDIR__}\..\resources\win\polaris-cleaner.exe"
; Polaris NSIS 安装/卸载钩子 —— 安装/卸载动文件前结束映像位于本安装目录内的内核进程；
; 安装前清旧资源；安装成功后归一化运行形态；卸载时清外置到 ProgramData 的提权 helper 服务。
;
; 挂载点：tauri.conf.json 的 `bundle.windows.nsis.installerHooks`。
; 移植自 上游 `build/installer.nsh` 的 `customUnInstall`（同一失败面、同一提权手法）。
;
; ── 为什么需要它 ──
; helper 在**运行期**被外置安装到 `C:\ProgramData\Polaris`，并注册为 LocalSystem 服务 `PolarisHelper`
; （真值源：`crates/helper/src/platform/windows/mod.rs` 的 `SERVICE_NAME` / `DEFAULT_SUPPORT_DIR`）。
; 这两样都**不在 NSIS 的安装清单里**（安装器没装过它们，是 app 自己装的）⇒ Tauri 默认卸载器只删
; 安装目录、注册表卸载项与快捷方式，管不到 SCM 服务与 ProgramData ⇒ 不补此钩子，用户走「设置 /
; 控制面板 → 卸载」之后机器上会留一个**孤儿 LocalSystem 服务**常驻 + helper 二进制与 token 残留，
; 且服务名/落点用户完全不知情，无从自行清理。
;
; ── 范围：只清提权那一半，用户数据不碰 ──
; 用户数据**已由 Tauri 模板自己处理**（实证：tauri-cli 2.11.4 内嵌模板的 Section Uninstall 里
; `${If} $DeleteAppDataCheckboxState = 1 ${AndIf} $UpdateMode <> 1` → `RmDir /r "$APPDATA\${BUNDLEID}"`
; + `"$LOCALAPPDATA\${BUNDLEID}"`）。Polaris 的配置目录 `<app_config_dir>/polaris` 与更新包缓存
; `<app_cache_dir>/updates` 都落在 `com.polaris.app` 之下 ⇒ 已被那个复选框覆盖。
; 本钩子**刻意不重复删一遍** —— 那会把用户明确没勾选的数据也删掉。
; （上游的对应钩子额外清了 `%APPDATA%\上游`，那是因为 electron-builder 的
;   `deleteAppDataOnUninstall:false` 让它没有等价机制，不是本仓的情况。）
;
; ── 三条卸载路径的处置 ──
;   1. **应用内更新**（updater 以 `/UPDATE` 跑旧版卸载器）→ 整体跳过。外置 helper 与 app 解耦，
;      更新只换 app 文件、服务原样常驻；此处若动服务 = 每次更新断流 + 弹一次 UAC，正好违背外置初衷。
;   2. **控制面板 / 设置里直接卸载**（app 未参与）→ 提权一次，清服务 + ProgramData。**本钩子的唯一目标场景。**
;   3. **应用内「完全卸载」**（`runtime/uninstall.rs`）→ 先同步请求 bundled cleaner UAC 清理，
;      再唤起本卸载器；本钩子仍交 native cleaner 复核。普通权限探测不能证明受保护目录已清空，
;      因此此处可能再次请求 UAC，不作「已无服务就跳过」的承诺。
;
; ── 提权（当前形态：`installMode: currentUser`）──
; 模板对 currentUser 发 `RequestExecutionLevel user`（tauri-cli 2.11.4 的 `installer.nsi`：
; `!if "${INSTALLMODE}" == "currentUser"` → `RequestExecutionLevel user`）⇒ 卸载器默认以**普通用户**
; 运行。卸载器嵌入固定用途 native cleaner，普通 launcher 固定实际映像及祖先句柄，
; 再经原生 ShellExecuteExW runas 请求 UAC；提权的是 cleaner，不是 GUI 或脚本。
; 取消、partial 与 success 分别消费。取消或失败不阻断普通用户应用卸载，残留写入日志。
; genuine cleaner 使用与现有安装相同的固定 C:\ProgramData\Polaris，按祖先/对象句柄验证 owner、DACL、
; reparse、身份，再对同一对象句柄删除。unsigned/currentUser 载体真实性风险仍保留。
; NSIS POSTUNINSTALL 的 payload 来自卸载器自身 File 数据块，不依赖已删安装目录。
;
; ── 🔮 前瞻登记：currentUser → perMachine 会是**安装形态变更**，不是原地升级 ──
; 下面这一整节描述的是**假如**把 `installMode` 改成 `perMachine` 会发生什么。当前形态是
; `currentUser`，所以这些后果**眼下都不成立**；登记在这里是因为它们是那次改动的前置清单，
; 逐条都按 tauri-cli 2.11.4 的 `installer.nsi` / `utils.nsh` 原文核过，别在下一轮重新推一遍。
;
; 模板的「已装过旧版」探测读的是 `SHCTX` 下的卸载键（`ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""`，
; 取不到就 `Abort` 掉重装页），而 `SHCTX` 由 `utils.nsh` 的 `SetContext` 按 INSTALLMODE 定：
; currentUser→HKCU、perMachine→HKLM。于是在一台**只装过 per-user 旧版**的机器上：
;   · 新的 per-machine 安装器会在 HKLM 里查不到任何东西 ⇒ **不提示、不卸载旧版**，直接装进 Program Files；
;   · 旧的 `%LOCALAPPDATA%\Polaris` 副本、HKCU 卸载项、per-user 开始菜单/桌面快捷方式**原样留下**
;     ⇒ 用户会在「应用和功能」里看到两个 Polaris，开始菜单里看到两个同名项（新的在 All Users 侧，
;        因为 per-machine 下 `SetShellVarContext all`）。
;   · `RestorePreviousInstallLocation` 同样读 `SHCTX "${MANUPRODUCTKEY}"` ⇒ 也不会把安装目录拉回旧路径。
;   · 用户数据不受影响：`%APPDATA%\com.polaris.app` / `%LOCALAPPDATA%\com.polaris.app` 是 per-user 目录，
;     与装机形态无关 ⇒ 新装的 app 读到的是同一份配置（这是好的那一面）。
;   · 自启项是隐患：`tauri-plugin-autostart` 写的是 HKCU `…\CurrentVersion\Run`，值指向**旧副本的 exe**。
;     旧副本还在盘上 ⇒ 开机自启拉起的仍是旧版本，直到用户卸掉旧副本或重新开关一次自启。
;   · 若用户事后卸掉那个遗留的 per-user 条目：旧卸载器会跑**它自己那一版**的本钩子 ⇒ `sc delete
;     PolarisHelper` + 删 `C:\ProgramData\Polaris`，把新装 app 仍在用的 helper 一并清掉。后果是
;     TUN 暂不可用 + 下次起核时走既有「安装 helper」引导（多一次 UAC），**不 brick**。
;
; ── 🔮 前瞻登记：那条迁移腿为什么**不能**写在本文件里（2026-09-16 核实，结论与出处原样保留）──
; 同样是「假如将来改 per-machine」才用得上的结论，但它值得先写下来，因为它的失效方向最坏：
; 在本文件里实现那段清理，代码**写得出、编得过、看起来也对**，而它是一个提权漏洞。
; 结论是「换执行者」，不是「换个写法」。
;
; 前提：per-machine 的安装器是**提权进程**（模板对它发 `RequestExecutionLevel admin`）。
; 而迁移要清的四样东西**全部住在用户可写域**里，于是逐条都是「受信提权动作消费不受信输入」——
; 与本批要堵的那条链（提权脚本拷一份用户可写的 `polaris-helper.exe` 注册成 SYSTEM 服务）
; **同一个根因**。在这里实现迁移，等于一边堵旧洞一边开新洞：
;
;   ① **跑旧版自带的卸载器 = 以管理员身份执行用户可写的二进制。** 路径取自 HKCU 的
;      `UninstallString`（用户可写）；即便改成从 `HKLM\…\ProfileList\<SID>\ProfileImagePath`
;      （只有管理员能写）推出默认路径 `<profile>\AppData\Local\Polaris\uninstall.exe`，
;      **那个文件本身仍躺在用户可写目录里** ⇒ 攻击者换掉它，受信安装器替他提权执行。
;      路径怎么推出来都救不了：不受信的是**文件**，不是路径。
;      （附带事实，留给下一批：旧卸载器带 `/UPDATE` 跑时，本文件的 POSTUNINSTALL 整段被
;        `${If} $UpdateMode <> 1` 跳过 ⇒ **不会**删 helper 服务与 ProgramData；而模板侧的
;        `DeleteRegKey HKCU "${UNINSTKEY}"` 不受 UpdateMode 闸门约束 ⇒ 卸载项照样清掉。
;        所以「跑旧卸载器会打掉 helper」这一条其实绕得开 —— 绕不开的是上面那条 EoP。）
;
;   ② **提权后 `RMDir /r` 一个用户可控路径 = 任意目录删除。** `InstallLocation` 在 HKCU
;      （用户可写），指到 `C:\Windows\System32` 就删 System32。加「目录名必须是 Polaris /
;      必须含 uninstall.exe+Polaris.exe」这类形态校验也堵不住：NSIS 的 `RMDir /r`**跟随 junction**
;      —— 实证 NSIS `Source/exehead/util.c` 的 `myDelete()`：命中 `FILE_ATTRIBUTE_DIRECTORY` 就
;      `myDelete(buf,flags)` 递归，全函数**没有一处** `FILE_ATTRIBUTE_REPARSE_POINT` 检查，而
;      `FindFirstFile("<junction>\*.*")` 枚举的是 junction 指向的目标。建 junction 不需要任何特权
;      ⇒ 攻击者在自己 profile 的旧安装目录里挂一个指向 System32 的 junction，提权递归删除就变成
;      任意文件删除。**凡是在用户可写树里做提权递归删除都有这个洞**，与路径来源无关。
;
;   ③ **提权后的 HKCU 不一定是发起用户的。** 用户本身是管理员时走 UAC 过滤令牌提权，SID 不变、
;      HKCU 对；标准用户输入**别人的**管理员凭据提权时，进程属于那个管理员 ⇒ HKCU 换了 hive，
;      `SetShellVarContext current` 下的 `$SMPROGRAMS` / `$DESKTOP` 也全部解析到管理员的 profile
;      ⇒ 清理**静默什么都没做**（不报错、不留痕，最坏的一种失败）。
;      遍历 `HKEY_USERS` 只能看见**已加载**的 hive（当前已登录的用户），没登录的用户看不见；
;      要覆盖全部用户得逐个 `reg load` 他们的 `NTUSER.DAT`，那是另一个量级的风险，不做。
;
;   ④ 🔴 **路径常量陷阱 —— 这一条与 installMode 无关，现在就成立**：NSIS 的 `$LOCALAPPDATA`
;      在 **all 上下文**下映射到 `CSIDL_COMMON_APPDATA`，实证 NSIS `Source/build.cpp`：
;      `m_ShellConstants.add(_T("LOCALAPPDATA"), CSIDL_LOCAL_APPDATA, CSIDL_COMMON_APPDATA);`
;      ⇒ 在 all 上下文里写 `$LOCALAPPDATA\Polaris` 得到的是 **`C:\ProgramData\Polaris`**，
;      正是 helper 的受保护目录。本文件**眼下就有一段跑在 all 上下文里**（下面 POSTUNINSTALL 的
;      `SetShellVarContext all`，那里是**刻意**要 `$APPDATA` 解析成 ProgramData）；若将来改
;      per-machine，模板会在 `un.onInit` 把整个卸载器都设成 all ⇒ 射程扩到全文件。
;      谁按 per-user 直觉在那个上下文里写这个常量，删掉的就是 helper 而不是旧副本。
;
; 而这四样要清的东西**没有一样需要管理员权限**：HKCU 卸载键、HKCU `…\CurrentVersion\Run` 自启值、
; per-user 开始菜单/桌面快捷方式、`%LOCALAPPDATA%\Polaris` 目录树 —— 全部是发起用户自己就能删的。
; 既然不需要提权、而提权反倒同时制造 ①②③，迁移腿的正确执行者是**以该用户身份运行的 app 本体**
; （首次启动自检并清理），不是安装器。换到那一侧，③ 顺带变成零成本：每个用户第一次跑新 app 时清
; 自己的残留，天然落在对的 hive 与对的 profile 上，连「哪个用户」这个问题都不存在。
;
; 自启值那一项在 app 侧也不必猜格式：`tauri-plugin-autostart` 的 `enable()` 用 `current_exe()`
; 重写 HKCU Run 值（`auto-launch` 0.5.0 `windows.rs`：值名 = app name、值 = `"{app_path} {args}"`），
; 而它的 `is_enabled()` 只看值**在不在**、不看指向谁 ⇒ 存量用户的自启值会一直指着旧 exe，
; 直到有人显式 `enable()` 一次。「重写还是删除」因此不是取舍：重写是顺手的，删除才要额外写代码，
; 且删除会让本来开机自启的用户静默失去自启。
; （同处顺带登记，改 per-machine 会新出现的一条：`auto-launch` 写 Run 值时**不给路径加引号**，
;   而 per-machine 的落点 `C:\Program Files\Polaris\Polaris.exe` 必然含空格（当前的
;   `%LOCALAPPDATA%\Polaris\Polaris.exe` 只在用户名含空格时才含）。CreateProcess 的前缀歧义
;   启发式会依次试 `C:\Program.exe` 再试全路径，故能起来。它会不会同时变成劫持点，取决于 `C:\`
;   根的 ACL 是否允许标准用户建**文件**（通行说法是只允许建目录、不允许建文件，**本仓未实测**）——
;   要下结论得在真机上验，别照抄这句。）
;
; 这条边界由 `scripts/verify-packaging.mjs` 的 `checkWindowsHookPrivilegeBoundary` 钉在**代码面**上，
; **而且它与 installMode 无关、现在就生效**：写在注释里的「别在这儿做」对下一个改本文件的人没有任何
; 强制力，而上面 ①②④ 三条在 currentUser 下也一样是真的（本文件已经有一段跑在 all 上下文里，
; 且卸载腿本来就会把自己提权到管理员再去删 ProgramData）。
;
; 若将来真要做那条迁移腿：它的执行者是 app，不是安装器；发布说明在迁移腿落地之前得指引用户手动
; 卸掉旧的 per-user 条目，并**连带写上**上面那条后果（手动卸旧条目会跑旧版自己的 POSTUNINSTALL
; ⇒ helper 服务与 `C:\ProgramData\Polaris` 被清掉 ⇒ 下次起核多一次 UAC 重装 helper）。
; 只说「请卸掉旧的那个」，用户会把随后那次 UAC 当成新版本的缺陷。

; 安装器文案 i18n：运行时按 `$LANGUAGE` 的 LCID 选 English / 简中 / 繁中 / Russian / Farsi。
;
; 为什么用运行时判断而不是 LangString：LangString 必须在对应语言被 `MUI_LANGUAGE` 加载**之后**定义，
; 而本文件由模板在 `!include MUI2.nsh` 之后、`!insertmacro MUI_LANGUAGE` 之前 include（实证：
; tauri-cli 2.11.4 内嵌模板 `{{#if installer_hooks}} !include "{{installer_hooks}}"` 位于语言块之前）
; ⇒ 在此定义 LangString 会踩「language table 缺该 string」的编译期告警。纯数字比较不依赖任何编译期
; 语言常量，本宏在函数体内展开、届时 `$LANGUAGE` 已是当前 LCID。
;
; Tauri 固定的 NSIS 3.11 发行包提供 Farsi.nlf（LCID 1065、CP1256、RTL）。Tauri 自带的一个历史
; 语言命名与该固定 NSIS 发行包不匹配；本仓以 `Farsi` 为唯一 token，
; 自定义 Tauri 消息见 `nsis-languages/Farsi.nsh`。
!macro PolarisSelectLang OUT EN ZHCN ZHTW RU FA
  StrCpy ${OUT} "${EN}"
  ${If} $LANGUAGE == 2052
    StrCpy ${OUT} "${ZHCN}"
  ${ElseIf} $LANGUAGE == 1028
    StrCpy ${OUT} "${ZHTW}"
  ${ElseIf} $LANGUAGE == 1049
    StrCpy ${OUT} "${RU}"
  ${ElseIf} $LANGUAGE == 1065
    StrCpy ${OUT} "${FA}"
  ${EndIf}
!macroend

; ── 安装/卸载动文件前：结束映像位于本安装目录内的内核进程 ─────────────────────────
;
; 内核 `sing-box.exe` 直接从安装目录运行（`$INSTDIR\_up_\resources\win\`；被安装器覆盖的便携目录
; 则是 `$INSTDIR\resources\win\`）。Windows 不允许改写或删除一个正在运行的映像文件，于是只要
; 还有一份内核活着：
;   · 安装/升级：模板逐文件 `File /a` 覆盖到 `sing-box.exe` 时打不开目标 —— 交互安装弹
;     「无法写入文件」的中止/重试/忽略框；选忽略则装出「新应用 + 旧内核」。
;   · 卸载：模板逐文件 `Delete` 对它静默失败，随后的 `RMDir` 因目录非空逐级失败 ⇒ 卸载项已从
;     「应用和功能」消失，盘上却留着内核文件与半棵目录树。
;
; 什么时候会有活着的内核：
;   · 应用内更新 / 应用内「完全卸载」先确认所有内核收口才交给安装器，这两条路径上正常没有。
;   · **用户直接运行安装包，或从「设置 / 控制面板」卸载，而应用正开着代理** —— 模板会结束主程序
;     （见下）。应用侧已用作业对象保证主程序退出时内核结束；本钩子兜底处理旧版本遗留、作业对象
;     创建失败或被强制脱离的情形。
;   · 旧版本（没有作业对象）在上一会话异常退出后留下的孤儿内核。
;   经提权 helper 以 SYSTEM 起的那一份不在其列：它的映像在 `C:\ProgramData\Polaris\core\`，
;   不占安装目录里的任何文件，本宏也不碰它。
;
; ── 顺序：先让主程序退出，再结束内核 ──
; 主程序还活着时结束它的内核没有用：崩溃自恢复会在约 2 秒后把内核重新拉起来，而模板自己的
; 「主程序在运行」检查排在 PREINSTALL / PREUNINSTALL **之后**、复制/删除文件**之前**，两者之间没有
; 钩子位。所以本文件的两条 PRE 钩子先展开模板自带的 `CheckIfAppIsRunning`（同一个宏、同一套
; 询问与取消语义，不另写一份结束主程序的逻辑），再展开本宏；模板随后自己的那一次检查此时
; 已无对象，等于空操作。
; 该宏名与参数形态取自 tauri-cli 2.12.1 内嵌的 `utils.nsh`
; （`!macro CheckIfAppIsRunning executablePath productName`）。上游改名时这里是**编译错误**
; 而不是静默跳过 —— 刻意不加 `!ifmacrodef` 保护。
; 同一个宏在同一个 Section 里展开两次不会撞标签：它的标签后缀取 `${__LINE__}`，而宏内的
; `${__LINE__}` 是「外层行号.宏内行号」逐层拼接的（NSIS `Source/scriptpp.cpp` 的
; `set_line_predefine`），两次展开的外层行号不同。
;
; ── 判据：完整映像路径，不是进程名 ──
; 用户完全可能自己另跑着一份 sing-box。只结束同时满足下面三条的进程：
;   1. 映像文件名是 `sing-box.exe`（WMI 的 `Name` 过滤，只为缩小枚举面，本身不构成结束的理由）；
;   2. 映像完整路径规范化后，**不分大小写地**以 `<规范化后的 $INSTDIR>\` 为前缀（尾部分隔符先去掉
;      再补一个，所以 `…\Polaris` 不会匹配 `…\Polaris Beta\`）；
;   3. 前缀之后余下的部分恰为 `_up_\resources\win\sing-box.exe` 或 `resources\win\sing-box.exe`
;      —— 内核在安装目录下只会从这两个位置运行（后者是旧版裸 resources 布局与被安装器覆盖的便携
;      目录）。只用前缀不够：用户把应用直接装进一个公共父目录时，同目录下别的软件自带的 sing-box
;      也会满足前缀。
; 规范化 = 去掉 `\\?\` / `\\?\UNC\` 前缀后过 `GetFullPath`（斜杠方向、重复分隔符、`.`/`..` 段）。
; `$INSTDIR` 是卷根（盘符根，或 UNC 共享根）时整条跳过。
;
; 这段判据的三个函数（`PolarisFullPath` / `PolarisSweepRoot` / `PolarisIsInstalledCore`）与下面整段
; 脚本的唯一真值在 `scripts/lib/nsis-core-sweep.mjs`：本文件里的 `FileWrite` 行必须与它逐字相同
; （`verify-packaging.mjs confs` 与 `scripts/nsis-core-sweep.test.mjs` 都会比），**改脚本先改那边**。
; 那里另有一份 JS 等价实现与样例表（空格、引号、`$`、反引号、分号、`%`、非 ASCII、尾部反斜杠、
; 卷根、UNC、大小写、斜杠混用、`\\?\` 前缀、邻名目录等）；样例表在 Windows 上还会喂给这三个
; 函数的原文真执行。
;
; 已知限制（判据不处理，失效方向都是「少结束」而不是「多结束」，届时由 NSIS 自己的写入失败框接手）：
;   · 安装目录经符号链接 / 联接点 / `subst` 或映射盘访问时，系统报告的映像路径是解析后的真实
;     路径，与 `$INSTDIR` 的字面前缀对不上；
;   · 8.3 短路径：`GetFullPath` 只在路径真实存在时才展开短名，这一条没有在真机上验过；
;   · 段尾带空格或句点的目录名。
;
; ── 进程侧 ──
; 路径经 WMI `Win32_Process.ExecutablePath` 取，而不是 `Get-Process` 的 `.Path`：NSIS 安装器是
; 32 位进程，`$SYSDIR` 在它眼里被重定向到 SysWOW64，拉起的是 32 位 PowerShell，读不了 64 位进程的
; 模块表；WMI 由系统服务代查，不受此限。取不到路径的进程（别的用户的、权限更高的）一律跳过，
; 不猜；单个进程的路径规范化出错也只跳过它自己。
; 结束前先取得该进程的句柄（此后这个 PID 不会被复用），再按启动时间核对它就是枚举到的那一个
; （枚举到取得句柄之间 PID 可能已被复用）—— PID 加启动时间唯一确定一个进程，映像路径与启动时间
; 取自同一条 WMI 记录，所以不再另读一次映像路径（32 位进程也读不了）。然后最多等 5 秒确认它
; 真的退出：映像文件的占用在进程退出后才释放。
; `$INSTDIR` 经环境变量交给脚本，不拼进脚本文本或命令行：路径里的引号、空格、`$`、反引号、分号、
; `%`、末尾反斜杠都不参与任何一层解析。脚本文本本身是纯 ASCII（`FileWrite` 按 ANSI 代码页落盘）。
;
; ── 执行环境与失败不阻断 ──
; 经 `nsExec` 执行：无窗口，30 秒上界（超时即结束 PowerShell 并返回 `timeout`），退出码原样回到
; `$R4`。PowerShell 取 `$SYSDIR` 下的绝对路径，带 `-NoProfile -NonInteractive -ExecutionPolicy Bypass`。
; 下列情形脚本跑不起来或中途退出，都只在安装日志里留一行，然后继续：
;   · 组策略强制了执行策略（`Bypass` 被覆盖）、PowerShell 被禁用或被安全软件拦下 ⇒ `error` 或非零；
;   · 受限语言模式（AppLocker / WDAC）：脚本里的 .NET 方法调用被拒 ⇒ 预期落进最外层 catch，退出码 1；
;   · WMI 服务不可用或 `Get-CimInstance` 不存在（PowerShell 2.0）⇒ 退出码 1；卡住 ⇒ `timeout`；
;   · 结束失败或 5 秒内没退出 ⇒ 退出码 2；安装目录是卷根 ⇒ 退出码 3。
; 此时的表现与没有本宏时相同（安装侧由 NSIS 自己的写入失败框接手），不会更糟；反过来若在这里
; 中止，一台 WMI 服务异常或管控严格的机器就再也装不上、卸不掉。
;
; 只能在 Windows 真机或安装器实际运行时确认的：`System::Call` 设的环境变量确实被 `nsExec` 的子进程
; 继承；32 位 PowerShell 对 64 位内核取句柄、读启动时间、结束都成功；WMI 的 `CreationDate` 与
; `Process.StartTime` 的差在 2 秒内；上面列的各种受限环境下的实际退出码；整段在 30 秒内跑完。
;
; ── 两种装机形态 ──
;   · `currentUser`（当前形态）：安装器以发起用户身份运行，能看到并结束的只有该用户自己的进程
;     —— 用户态内核正是该用户起的，够用。
;   · `perMachine`（前瞻）：安装器已提权，能看到并结束任何用户会话里的进程；判据仍是映像路径在
;     `$INSTDIR` 内，射程不因提权而变宽。本宏不读取也不执行安装目录里的任何文件，只按路径结束
;     进程，不属于顶部那一节说的「提权动作消费用户可写输入」。
;
; 便携版不经过 NSIS，本宏管不到：用户手动把新 zip 解压覆盖到旧目录时，仍在运行的主程序与内核
; 由资源管理器按「文件正在使用」逐个提示，跳过即留下新旧混装的目录。
!macro PolarisStopInstalledCores
  Push $R4
  Push $R6
  Push $R8
  Push $R9

  !insertmacro PolarisSelectLang $R8 \
    "Stopping Polaris core processes started from this installation folder (if any)..." \
    "结束从本安装目录启动的 Polaris 内核进程（如有）..." \
    "結束從本安裝目錄啟動的 Polaris 核心處理程序（如有）..." \
    "Остановка процессов ядра Polaris, запущенных из этой папки установки (если есть)..." \
    "در حال متوقف کردن فرایندهای هسته Polaris که از این پوشه نصب اجرا شده‌اند (در صورت وجود)..."
  DetailPrint "$R8"

  StrCpy $R9 "$INSTDIR"
  System::Call 'kernel32::SetEnvironmentVariable(t "POLARIS_SWEEP_ROOT", t R9)'

  InitPluginsDir
  ; 下面的 FileWrite 行由 `scripts/lib/nsis-core-sweep.mjs` 的脚本真值渲染（`$$` 输出字面 `$`，留给
  ; PowerShell 展开），不要在这里手改。
  FileOpen $R6 "$PLUGINSDIR\polaris-stop-installed-cores.ps1" w
  FileWrite $R6 `$$ErrorActionPreference = 'Stop'$\r$\n`
  FileWrite $R6 `$$sep = '\'$\r$\n`
  FileWrite $R6 `$$ext = '\\?\'$\r$\n`
  FileWrite $R6 `$$tails = @('_up_\resources\win\sing-box.exe', 'resources\win\sing-box.exe')$\r$\n`
  FileWrite $R6 `function PolarisFullPath([string]$$p) {$\r$\n`
  FileWrite $R6 `  if ($$p.StartsWith($$ext + 'UNC' + $$sep, [System.StringComparison]::OrdinalIgnoreCase)) { $$p = $$sep + $$sep + $$p.Substring(8) } elseif ($$p.StartsWith($$ext, [System.StringComparison]::Ordinal)) { $$p = $$p.Substring(4) }$\r$\n`
  FileWrite $R6 `  return [System.IO.Path]::GetFullPath($$p)$\r$\n`
  FileWrite $R6 `}$\r$\n`
  FileWrite $R6 `function PolarisSweepRoot([string]$$dir) {$\r$\n`
  FileWrite $R6 `  $$full = (PolarisFullPath $$dir).TrimEnd($$sep)$\r$\n`
  FileWrite $R6 `  if ($$full -eq [System.IO.Path]::GetPathRoot($$full + $$sep).TrimEnd($$sep)) { return $$null }$\r$\n`
  FileWrite $R6 `  return $$full + $$sep$\r$\n`
  FileWrite $R6 `}$\r$\n`
  FileWrite $R6 `function PolarisIsInstalledCore([string]$$root, [string]$$image) {$\r$\n`
  FileWrite $R6 `  $$full = PolarisFullPath $$image$\r$\n`
  FileWrite $R6 `  if (-not $$full.StartsWith($$root, [System.StringComparison]::OrdinalIgnoreCase)) { return $$false }$\r$\n`
  FileWrite $R6 `  $$tail = $$full.Substring($$root.Length)$\r$\n`
  FileWrite $R6 `  foreach ($$t in $$tails) { if ([string]::Equals($$tail, $$t, [System.StringComparison]::OrdinalIgnoreCase)) { return $$true } }$\r$\n`
  FileWrite $R6 `  return $$false$\r$\n`
  FileWrite $R6 `}$\r$\n`
  FileWrite $R6 `try {$\r$\n`
  FileWrite $R6 `  $$root = PolarisSweepRoot $$env:POLARIS_SWEEP_ROOT$\r$\n`
  FileWrite $R6 `  if (-not $$root) { exit 3 }$\r$\n`
  FileWrite $R6 `  $$left = 0$\r$\n`
  FileWrite $R6 `  foreach ($$p in @(Get-CimInstance -ClassName Win32_Process -Filter "Name = 'sing-box.exe'")) {$\r$\n`
  FileWrite $R6 `    try {$\r$\n`
  FileWrite $R6 `      if (-not $$p.ExecutablePath) { continue }$\r$\n`
  FileWrite $R6 `      if (-not (PolarisIsInstalledCore $$root $$p.ExecutablePath)) { continue }$\r$\n`
  FileWrite $R6 `    } catch { continue }$\r$\n`
  FileWrite $R6 `    try {$\r$\n`
  FileWrite $R6 `      $$proc = [System.Diagnostics.Process]::GetProcessById([int]$$p.ProcessId)$\r$\n`
  FileWrite $R6 `      $$null = $$proc.Handle$\r$\n`
  FileWrite $R6 `      if ([Math]::Abs(($$proc.StartTime - $$p.CreationDate).TotalSeconds) -gt 2) { continue }$\r$\n`
  FileWrite $R6 `      $$proc.Kill()$\r$\n`
  FileWrite $R6 `      if (-not $$proc.WaitForExit(5000)) { $$left++ }$\r$\n`
  FileWrite $R6 `    } catch {$\r$\n`
  FileWrite $R6 `      if (Get-Process -Id $$p.ProcessId -ErrorAction SilentlyContinue) { $$left++ }$\r$\n`
  FileWrite $R6 `    }$\r$\n`
  FileWrite $R6 `  }$\r$\n`
  FileWrite $R6 `  if ($$left -gt 0) { exit 2 }$\r$\n`
  FileWrite $R6 `  exit 0$\r$\n`
  FileWrite $R6 `} catch { exit 1 }$\r$\n`
  FileClose $R6

  nsExec::Exec /TIMEOUT=30000 `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\polaris-stop-installed-cores.ps1"`
  Pop $R4 ; 0 = 没有或已全部结束；1 = 枚举失败或脚本被环境拒绝；2 = 有进程没能结束；3 = 安装目录是卷根；timeout / error = 脚本没跑完
  ${If} $R4 != 0
    !insertmacro PolarisSelectLang $R8 \
      "Could not confirm that all Polaris core processes from this folder have stopped; continuing." \
      "未能确认本安装目录下的 Polaris 内核进程已全部结束，继续执行。" \
      "未能確認本安裝目錄下的 Polaris 核心處理程序已全部結束，繼續執行。" \
      "Не удалось подтвердить остановку всех процессов ядра Polaris из этой папки; продолжение." \
      "تأیید توقف همه فرایندهای هسته Polaris از این پوشه ممکن نشد؛ ادامه داده می‌شود."
    DetailPrint "$R8 ($R4)"
  ${EndIf}

  Pop $R9
  Pop $R8
  Pop $R6
  Pop $R4
!macroend

; ── 安装/升级前：清理旧版裸 resources 布局 ───────────────────────────────────────
;
; 当前 Tauri 资源的权威安装位置是 `$INSTDIR\_up_\resources\`；早期安装包曾把同一批资源铺在
; `$INSTDIR\resources\`。NSIS 升级只覆盖本次清单，不会删除那棵旧目录（`.207` 在 2026-08-23
; 从旧包升级到 `417277b` 后真机仍残留旧 core/helper/dashboard/data）。如果任它留下：
;   - 旧包约百 MiB 永久占盘；
;   - 任何仍把裸目录当首选的客户端都会静默命中旧 core/helper。
; 本宏只删**安装目录内、由旧安装包拥有**的 legacy 根；用户配置在 AppData，外置 helper 在
; ProgramData，portable 不经过 NSIS，均不在射程。随后模板才复制本包 `_up_` 资源，失败也不会回落
; 旧 payload 冒充安装成功。
;
; 🔮 前瞻：当前形态 `installMode: currentUser` 下 `$INSTDIR` 是 `%LOCALAPPDATA%\Polaris`，本宏清的
; 就是这一份，射程完整。若将来改 `perMachine`，`$INSTDIR` 变成 `%PROGRAMFILES%\Polaris`
; ⇒ 本宏只清得到新那一份，而**旧 per-user 时代留在 `%LOCALAPPDATA%\Polaris` 的整棵树落到射程之外**
; （详见顶部「前瞻登记：currentUser → perMachine」一节）。届时也**不要**在这里顺手去删那棵树：
; 它是旧安装器的资产，删了会把「应用和功能」里那条卸载项变成指向空目录的死项，
; 而且提权进程删用户可写树本身就是那一节 ②说的那个洞。
!macro NSIS_HOOK_PREINSTALL
  !echo "[polaris] NSIS_HOOK_PREINSTALL 已插入 —— 安装前结束本目录内核、清理 legacy resources"
  ; 顺序不可换：主程序先退出 → 再结束内核 → 最后才动文件（成因见 PolarisStopInstalledCores 上方）。
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro PolarisStopInstalledCores
  Push $R8
  !insertmacro PolarisSelectLang $R8 \
    "Removing obsolete Polaris resources from an older installation (if present)..." \
    "清理旧版安装遗留的 Polaris 资源（如有）..." \
    "清理舊版安裝遺留的 Polaris 資源（如有）..." \
    "Удаление устаревших ресурсов Polaris из предыдущей установки (если есть)..." \
    "در حال حذف منابع قدیمی Polaris از نصب قبلی (در صورت وجود)..."
  DetailPrint "$R8"
  RMDir /r "$INSTDIR\resources"
  Pop $R8
!macroend

; ── 安装/升级成功后：移除便携版形态标记 ─────────────────────────────────────────
;
; Windows 便携包靠 exe 同级 `portable.marker` 判定 loose 形态，NSIS 安装版则必须没有它。用户若把
; 便携目录放在默认安装路径后再运行安装器，Tauri 模板只覆盖安装清单内的文件，不会删除这个 marker；
; 结果是已经由 NSIS 安装的 app 仍被更新器误判为便携版，后续收到 portable zip 而不是 setup。
;
; 必须放 POSTINSTALL 而不是 PREINSTALL：只有新安装主体成功后才归一化形态。若复制新文件中途失败，
; 旧便携副本的 marker 仍在，不会因一次失败安装被提前改判为 installed。便携 zip 不经过 NSIS，故不受影响。
!macro NSIS_HOOK_POSTINSTALL
  !echo "[polaris] NSIS_HOOK_POSTINSTALL 已插入 —— 安装成功后清理 portable marker"
  Push $R8
  !insertmacro PolarisSelectLang $R8 \
    "Finalizing the installed Polaris layout..." \
    "完成 Polaris 安装版布局整理..." \
    "完成 Polaris 安裝版佈局整理..." \
    "Завершение настройки установленной версии Polaris..." \
    "در حال نهایی‌سازی چیدمان نسخه نصب‌شده Polaris..."
  DetailPrint "$R8"
  Delete "$INSTDIR\portable.marker"
  Pop $R8
!macroend

; ── 卸载动文件前：结束映像位于本安装目录内的内核进程 ─────────────────────────────
;
; 带 `/UPDATE` 的卸载同样执行：那条路径一样要逐文件删除安装目录里的内核。
!macro NSIS_HOOK_PREUNINSTALL
  !echo "[polaris] NSIS_HOOK_PREUNINSTALL 已插入 —— 卸载前结束本目录内核"
  ; 顺序不可换：主程序先退出 → 再结束内核（成因见 PolarisStopInstalledCores 上方）。
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro PolarisStopInstalledCores
!macroend

; helper 清理用 POSTUNINSTALL 而不是 PREUNINSTALL：它与 app 安装目录里的文件互不相干（helper 在
; ProgramData，不会锁住 $INSTDIR 里的任何东西），放到最后可以保证「UAC 弹窗 / 提权失败」绝不
; 干扰正常的卸载主体流程。
!macro NSIS_HOOK_POSTUNINSTALL
  ; 🔴 **编译期自曝**（不是装饰）：Tauri 模板对 hook 的插入是
  ;     `!ifmacrodef NSIS_HOOK_POSTUNINSTALL` + `!insertmacro NSIS_HOOK_POSTUNINSTALL`
  ; —— 宏名**拼错一个字母就静默跳过，且构建照常绿**，产出一个「看起来修好了、实际没有钩子」的
  ; 安装包，而这正是本文件要修的那个缺陷（卸载后留孤儿 root 服务）原样复发。
  ;
  ; 产物侧也验不了：NSIS 用 LZMA 实体压缩，字符串表进了压缩体 —— 对 setup.exe 直接 grep
  ; 连产品名 `Polaris` 都 0 命中（已做正向对照，方法本身是瞎的）。
  ;
  ; ⚠️ **下面这行 `!echo` 目前验不了任何东西**（2026-08-05 实测订正，别再照着它推结论）：
  ; 加它的初衷是「CI 日志里看得到 = 钩子插上了」。实测 run 30996066681：日志里 0 命中，但那**不构成
  ; 反证** —— 正向对照显示 `Running makensis to produce …` 之后整整 60 秒零输出，tauri-bundler
  ; 成功时根本不透传 makensis 的 stdout。通道是死的，命中与否都没有信息量。
  ;
  ; 留着它：零成本，且构建**失败**时 tauri 会把 makensis 输出打出来，届时这行仍是有用线索。
  ;
  ; 🔴 **要真正证明钩子被插入，用变异探针**：在本宏体内故意写一行非法 NSIS 指令 → 跑一次 Windows
  ; 打包腿。构建**失败**即证明宏体被展开（= 钩子确实插上了）；构建照常**成功**则说明宏根本没被插入
  ; （`!ifmacrodef` 判假），那正是本文件要防的静默失效。做完记得改回来。
  ; 之所以需要这么绕：NSIS 宏是插入点纯文本展开，**未被插入的宏体连语法都不会被检查** ——
  ; 所以「构建通过」这件事对本钩子是否存在**一个字节的信息都不提供**。
  !echo "[polaris] NSIS_HOOK_POSTUNINSTALL 已插入 —— 卸载时将清理 PolarisHelper 服务与 ProgramData"

  ; 本宏展开在 Section Uninstall 末尾。寄存器仍显式 Push/Pop 保存：宏被插进别人的 Section，
  ; 不该对「此处之后没人再读 $R4-$R9」这个当前恰好成立的事实下注。
  Push $R4
  Push $R5
  Push $R6
  Push $R7
  Push $R8
  Push $R9

  ${If} $UpdateMode <> 1
    ; A currentUser uninstaller cannot prove absence inside the protected tree.
    ; Always ask the fixed native cleaner to establish SCM/tree absence or clean
    ; it under UAC; denied enumeration is never treated as an absent directory.
      !insertmacro PolarisSelectLang $R8 \
        "Removing PolarisHelper service and ProgramData (one admin authorization required)..." \
        "清理 PolarisHelper 服务与 ProgramData（需一次管理员授权）..." \
        "清理 PolarisHelper 服務與 ProgramData（需一次系統管理員授權）..." \
        "Удаление службы PolarisHelper и ProgramData (требуется одно подтверждение администратора)..." \
        "در حال حذف سرویس PolarisHelper و ProgramData (یک تأیید مدیر لازم است)..."
      DetailPrint "$R8"

      ; Payload is embedded in the uninstaller data block, independent of the
      ; app installation directory already removed by POSTUNINSTALL. The
      ; unprivileged native launcher locks its actual image and ancestors before
      ; asking UAC to execute that same image with only the fixed --worker role.
      ; Unsigned carrier authenticity remains the existing distribution risk.
      InitPluginsDir
      StrCpy $R6 $OUTDIR
      SetOutPath "$PLUGINSDIR"
      File /oname=polaris-cleaner.exe "${POLARIS_NATIVE_CLEANER_PAYLOAD}"
      SetOutPath $R6
      nsExec::ExecToStack '"$PLUGINSDIR\polaris-cleaner.exe" --launch'
      Pop $R7
      Pop $R5
      ; 0 complete; 2 custody/ancestor refusal; 3 partial cleanup;
      ; 20 UAC cancelled; 23 launch/unknown completion; error = launcher failed.
      ${If} $R7 != 0
        !insertmacro PolarisSelectLang $R8 \
          "Cleanup of the PolarisHelper service and the Polaris folder under ProgramData did not complete." \
          "PolarisHelper 服务与 ProgramData 下 Polaris 目录的清理未完成。" \
          "PolarisHelper 服務與 ProgramData 下 Polaris 目錄的清理未完成。" \
          "Очистка службы PolarisHelper и папки Polaris в ProgramData не завершена." \
          "پاک‌سازی سرویس PolarisHelper و پوشه Polaris در ProgramData کامل نشد."
        ${If} $R7 == 20
          !insertmacro PolarisSelectLang $R8 \
            "Administrator authorization was not granted; the PolarisHelper service and the Polaris folder under ProgramData were left in place." \
            "未获得管理员授权，PolarisHelper 服务与 ProgramData 下的 Polaris 目录未清理。" \
            "未取得系統管理員授權，PolarisHelper 服務與 ProgramData 下的 Polaris 目錄未清理。" \
            "Права администратора не получены; служба PolarisHelper и папка Polaris в ProgramData не удалены." \
            "مجوز مدیر دریافت نشد؛ سرویس PolarisHelper و پوشه Polaris در ProgramData حذف نشدند."
        ${EndIf}
        ${If} $R7 == 2
          !insertmacro PolarisSelectLang $R8 \
            "The Polaris folder under ProgramData was not removed: it is not owned by Administrators or SYSTEM, grants write access to other accounts, or contains links. Remove it manually if it is no longer needed." \
            "ProgramData 下的 Polaris 目录未清理：其属主不是 Administrators 或 SYSTEM、对其它账户开放了写权限，或其中含有链接。如不再需要请手动删除。" \
            "ProgramData 下的 Polaris 目錄未清理：其擁有者不是 Administrators 或 SYSTEM、對其他帳戶開放了寫入權限，或其中含有連結。如不再需要請手動刪除。" \
            "Папка Polaris в ProgramData не удалена: её владелец не Administrators и не SYSTEM, другим учётным записям разрешена запись, либо она содержит ссылки. Удалите её вручную, если она больше не нужна." \
            "پوشه Polaris در ProgramData حذف نشد: مالک آن Administrators یا SYSTEM نیست، به حساب‌های دیگر اجازه نوشتن داده شده، یا شامل پیوند است. در صورت عدم نیاز آن را دستی حذف کنید."
        ${EndIf}
        ${If} $R7 == 3
          !insertmacro PolarisSelectLang $R8 \
            "The Polaris folder under ProgramData could not be removed completely (files, permissions, or service state may prevent completion)." \
            "ProgramData 下的 Polaris 目录未能完全删除（可能有占用、权限或服务状态问题）。" \
            "ProgramData 下的 Polaris 目錄未能完全刪除（可能仍有檔案使用中）。" \
            "Папку Polaris в ProgramData не удалось удалить полностью (возможно, некоторые файлы ещё используются)." \
            "پوشه Polaris در ProgramData به‌طور کامل حذف نشد (ممکن است برخی فایل‌ها هنوز در حال استفاده باشند)."
        ${EndIf}
        DetailPrint "$R8 ($R7)"
      ${EndIf}
  ${EndIf}

  Pop $R9
  Pop $R8
  Pop $R7
  Pop $R6
  Pop $R5
  Pop $R4
!macroend
