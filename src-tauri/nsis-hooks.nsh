; Polaris NSIS 安装/卸载钩子 —— 安装前清旧资源；安装成功后归一化运行形态；
; 卸载时清外置到 ProgramData 的提权 helper 服务。
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
;   3. **应用内「完全卸载」**（`runtime/uninstall.rs`）→ 它先经 helper 自卸把服务与 ProgramData 清掉，
;      再唤起本卸载器 ⇒ 届时下面的探测两条都不命中 ⇒ 跳过，**不弹第二次 UAC**。
;
; ── 提权（当前形态：`installMode: currentUser`）──
; 模板对 currentUser 发 `RequestExecutionLevel user`（tauri-cli 2.11.4 的 `installer.nsi`：
; `!if "${INSTALLMODE}" == "currentUser"` → `RequestExecutionLevel user`）⇒ 卸载器默认以**普通用户**
; 运行，而 `sc delete` 与删 ProgramData 需要管理员。经「外层普通 PS 唤起内层提权 PS」完成
; （`Start-Process -Verb RunAs -Wait`），全程只弹一次 UAC。
; **best-effort**：用户取消 UAC 时退出码非 0，此处**不阻断卸载**（宁可残留，也不让卸载卡死）。
; 兜底是下次安装时 helper 安装脚本自身的幂等清理（停删同名旧服务）。
;
; 前瞻（若将来改成 `perMachine`）：模板改发 `RequestExecutionLevel admin`，该属性同时写进安装器与
; 卸载器的 manifest ⇒ 本钩子运行时进程本身已是管理员，`sc delete` 与删 ProgramData 直接就有权限。
; 届时下面那一跳**仍应保留、不要简化**：已提权时 `-Verb RunAs` 不再弹第二次 UAC（直接以当前提升令牌
; 起进程）⇒ 零成本；而它是眼下唯一在真机上验过的执行路径，删它等于在没有真机的情况下换掉已验路径。
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
  !echo "[polaris] NSIS_HOOK_PREINSTALL 已插入 —— 安装前清理 legacy resources"
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

; 用 POSTUNINSTALL 而不是 PREUNINSTALL：本清理与 app 安装目录里的文件互不相干（helper 在
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
    ; ProgramData 的绝对路径：`SetShellVarContext all` 下 `$APPDATA` 即 `C:\ProgramData`。
    ; 这一句不能省：模板在紧邻本钩子之前的「删除应用数据」分支里发了 `SetShellVarContext current`
    ; （勾了复选框时），所以进入本宏时的上下文不确定，必须自己定。
    ;
    ; 取完**立刻还原成 current** —— 不给本宏之后的任何代码留下被改过的上下文。`current` 就是环境值：
    ; 模板在 `un.onInit` 经 `utils.nsh` 的 `SetContext` 按 INSTALLMODE 定上下文，当前形态
    ; `installMode: currentUser` 对应 `SetShellVarContext current`。
    ; 🔮 前瞻：若将来改 `perMachine`，同一段模板会把环境上下文设成 `all`
    ; （`!if "${INSTALLMODE}" == "perMachine"` → `SetShellVarContext all`）⇒ 届时**环境值变成 `all`**，
    ; 下面这句还原会反过来变成「留下被改过的上下文」，要一并改掉。
    SetShellVarContext all
    StrCpy $R9 "$APPDATA\Polaris"
    SetShellVarContext current

    !insertmacro PolarisSelectLang $R8 \
      "Checking Polaris privileged helper service..." \
      "检查 Polaris 提权 helper 服务..." \
      "檢查 Polaris 提權 helper 服務..." \
      "Проверка привилегированной службы-помощника Polaris..." \
      "در حال بررسی سرویس کمکی دارای دسترسی ویژه Polaris..."
    DetailPrint "$R8"

    ; 用 System32 绝对路径调系统命令（`$SYSDIR` = System32），不依赖 PATH ——
    ; 部分设备 PATH 缺 System32 会导致命令未找到，且可被 cwd 劫持。
    nsExec::ExecToStack '"$SYSDIR\sc.exe" query PolarisHelper'
    Pop $R4 ; 退出码：0 = 服务存在，1060 = 不存在
    Pop $R5 ; 输出（丢弃）

    ; 服务在 **或** ProgramData 目录还在 —— 两者都需要管理员才能清，任一命中就提权。
    ; 判据取「或」而不是只看服务：应用内卸载中途失败可能留下「服务已删、目录还在」的半清理态，
    ; 只看服务会漏掉它，而那个目录里躺着 helper 二进制与 token。
    ${If} $R4 == 0
    ${OrIf} ${FileExists} "$R9\*.*"
      !insertmacro PolarisSelectLang $R8 \
        "Removing PolarisHelper service and ProgramData (one admin authorization required)..." \
        "清理 PolarisHelper 服务与 ProgramData（需一次管理员授权）..." \
        "清理 PolarisHelper 服務與 ProgramData（需一次系統管理員授權）..." \
        "Удаление службы PolarisHelper и ProgramData (требуется одно подтверждение администратора)..." \
        "در حال حذف سرویس PolarisHelper و ProgramData (یک تأیید مدیر لازم است)..."
      DetailPrint "$R8"

      InitPluginsDir
      ; 清理命令写进临时 .ps1，避免多层引号嵌套。`$$` 输出字面 `$`，
      ; 使 `$env:ProgramData` 留到**提权后的那个 PS 进程**里展开。
      FileOpen $R6 "$PLUGINSDIR\polaris-helper-uninstall.ps1" w
      FileWrite $R6 `& "$SYSDIR\sc.exe" stop PolarisHelper$\r$\n`
      FileWrite $R6 `Start-Sleep -Milliseconds 500$\r$\n`
      FileWrite $R6 `& "$SYSDIR\sc.exe" delete PolarisHelper$\r$\n`
      FileWrite $R6 `Start-Sleep -Milliseconds 500$\r$\n`
      FileWrite $R6 `Remove-Item -Recurse -Force -Path "$$env:ProgramData\Polaris" -ErrorAction SilentlyContinue$\r$\n`
      FileClose $R6

      ; 外层普通 PS 唤起内层提权 PS：`-Verb RunAs` 触发 UAC，`-Wait` 阻塞至清理完成，
      ; nsExec 再阻塞至外层结束 ⇒ 卸载器不会在清理还没跑完时就退出。
      nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "Start-Process '$SYSDIR\WindowsPowerShell\v1.0\powershell.exe' -Verb RunAs -WindowStyle Hidden -Wait -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','$PLUGINSDIR\polaris-helper-uninstall.ps1'"`
      Pop $R7 ; 退出码丢弃：用户取消 UAC 时非 0，best-effort 不阻断卸载
    ${EndIf}
  ${EndIf}

  Pop $R9
  Pop $R8
  Pop $R7
  Pop $R6
  Pop $R5
  Pop $R4
!macroend
