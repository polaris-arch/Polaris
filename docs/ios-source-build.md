# iOS 源码编译（准备中）

## 统一 main 的源码收口边界

当前 iOS 源码由 `c8f57984..1490dabf` 的 18 片独立增量移植到 `0713990a`，并按当前主线适配退出、重启和独立配置检查的入口。主 App 退出或重启保留系统 Packet Tunnel，不经过桌面四类 owner 的 drain 或签发清理证明；桌面检查准入继续生效，iOS 只跳过未提供承载的独立构造检查。

本目录的 iOS Libbox manifest 仍是历史六片 / gomobile v0.1.12 / SDK 27.0 基线。当前共享主线已包含九片补丁及 sing-tun/nftables 依赖固定；后三片与依赖尚未迁移到 iOS 构建输入。因此六片 Framework 只能作为历史开发缓存，不是当前统一源码的最终核心产物。必须完成整组 Apple 来源、补丁、依赖与导出 ABI 核验后，再更新 iOS manifest 并统一重建 Framework/App。当前合流只收纳源码，不产生新的 App、Framework、签名、NE 流量或设备验收收据。

有限独立审查已确认旧六片 Go monitor 可通过已进入的 Swift 回调，在 reload 后触达共享 wrapper 的新 manager。主线 `default-monitor-isolation.patch` 提供 manager/state 隔离、seal/admission 与 native listener identity；Apple 尚未消费该片。Swift close 忽略 listener 的事实存在，但独立的迟到 Close(A) 破坏 B 路径尚未证明，不能靠代理对象地址或一次 generation 检查代替完整迁移。

## 默认 final 入口与显式历史模式

`ios-libbox.py build / verify / check-inputs` 默认都是 final。共同 preflight 只读比较共享 sourceCommit、Go 版本、完整有序 patch(file,sha256)、依赖来源，以及 source manifest/provider、source receipt fingerprint、module graph、GoMod/GoSum、patched tree/version 等共同 pins；它在 Darwin/tool/Git/临时 checkout/cache/patch/gomobile/输出操作或 Framework/receipt 消费之前拒绝。Apple provider/dependency adapter 与最终链接模块策略尚未实现，当前稳定报 `Apple final source inputs not migrated`。只复制九片清单甚至共同 pins 也不会放行。Apple tags、gomobile、SDK、deployment 与 linker 参数仍须独立冻结，不复制 Android 设置。

两个 Xcode `Verify iOS Core` hook 保持无历史选项的默认 final 核验，因此已有六片 Framework 也不能通过当前工程的正常构建。`verify-ios-archive.py` 默认在打开 IPA 前同样拒绝未迁移的输入；外部 Framework receipt 本身不能证明它已静态链接入 IPA。

`--historical` 仅用于受审六片复现与缓存核验。历史 manifest 与补丁 bytes 仍固定；旧 `build-only` receipt 只接受已核 producer `2c8dd8e6ad7a8b2c1906f3cf51f5a9787ebc077d7a04d110cd4b298a45f5657e` 和 shell `1768d8b0e0426ce5d036533fd6b85379e975dfb968739d69bdabea242923790e`，原 receipt 不改写。新历史构建的 receipt 标为 `historical-only` 并绑定新驱动哈希，始终不能作为 final 证据。历史归档检查仅输出 `structure-only`，不证明 source/linkage、签名或 VPN 流量。

无需 Mac、Framework 或 IPA 的入口回归检查：

```sh
python3 scripts/ios-libbox-verify.test.py --preflight-only
python3 scripts/ios-libbox.py check-inputs --historical
```

统一规划的首项仍是实现真实 Apple shared provider/dependency 消费、完整 source receipt 与 Apple linked-module 分区、导出 Objective-C/Swift ABI 和 monitor 生命周期适配，再冻结输入与重建 Framework/App。上述拒绝保护只保住消费边界，不表示该迁移已经完成。

## 历史准备记录

以下记录与六片复现命令属于旧基线，不作为当前统一源码的最终构建或运行验收。

Polaris 的 iOS 构建由使用者在自己的 Mac 上完成并自行签名。项目不在 GitHub Release 发布 IPA。本轮将已冻结的16片 iOS 窄增量重放到统一候选 `42cf48bcdb09fbfc2b965035b0fd3b7fdd2e215f`，生产 App 源副本固定为 `1303e9e608cff04b2e37576ef9a0fabb8ea40e09`。没有双改路径或冲突 hunk；最新 Android resolver fence、容量/DNS 与测试修订完整保留。SDK27.1 模拟器 App 编译及 iPhone/iPad 各一项严格编辑草稿旋转检查通过，键盘显示时检查名称/焦点边框/保存及标题遮挡，不保存草稿或启动 VPN。旧 `d26464ad` 的完整10项布局和无签名设备包是历史收据，本轮没有重跑全量或重建设备包。历史 f699 六片 Libbox 已核验复用，本轮未重建 Framework；后续第七片公开接线及最终来源/哈希冻结后再统一生成。VPN 流量、统计订阅、后台生命周期与内存尚无运行验收，当前仍属开发准备阶段。

旧 `9ee2b576` 的桌面四 owner 退出与 check-custody 合流裁决不适用于当前42候选：本轮基座的退出/预检查源与554逐字相同，保留现有 iOS Host退出早退即可。主配置正常生成与写盘；iOS 早退阻断独立 `run_config_check`/构造及其缓存，不禁止主配置保存。共享 Go 的 `OnCancel`/结果 ABI 与 `Close` 原操作语义单独核对，`proofUnknown` 不得改写为操作失败或 NoOwner 证明。

## 构建主 App 的前置条件

- Xcode、命令行工具及 iOS SDK；历史 App 使用 Xcode 27.1 / SDK 27.1。六片 Libbox 的历史构建工具链仍固定为下节 27.0；原 App 链接已核验的六片 Framework。当前工程的 default final hook 保持拒绝，须完成 Apple 迁移后才能统一构建。首次构建可先在 Xcode 中接受许可。
- Node.js 24+、pnpm 11.24.0、Rust、CocoaPods、Go、Python 3；项目已包含生成的 Xcode 工程。修改 `project.yml` 后需要 XcodeGen；不要重新执行 `tauri ios init` 覆盖 Packet Tunnel target。
- `rustup target add aarch64-apple-ios`。若构建 Apple Silicon 模拟器，再加 `aarch64-apple-ios-sim` 和与 SDK 匹配的 iOS Simulator runtime。
- 真机安装需要 Apple Developer 付费账号、开发证书、设备注册和对应 provisioning profile。Xcode 登录账号并不自动证明本机已有有效签名身份。

## 显式复现历史 pinned iOS Libbox

在仓库根目录运行。脚本将官方核心固定到 `b609f959f57ce34416c51c7b87ce4a76f2e1df56`，应用最终冻结候选 Polaris `18202c52` / Go `f63543e` 的六个原始补丁，并校验各补丁 SHA-256 与应用后的 Git tree。construction SHA-256 为 `f699001b797fdc473f20ad8738ee674f7244371ed34e383367b3cfa33bad7ec5`；不混用前版 `e28b`。完整来源 commit 固定在 iOS manifest 和构建回执中。六片重放 tree `553461561f628a8875c637d6f248226d8b05fadc` 与共享 Go 最终 tree 完全相同。construction 补丁依赖前面的 strict cleanup / transient 补丁，不能单独应用。提供的 core 仓库仅用于读取 Git objects；构建会在 `~/Code` 创建独立临时 checkout。

当前可复现工具链固定为 Go 1.25.5、核心 `go.mod` 指定的 SagerNet gomobile/gobind v0.1.12、Xcode 27.0 build 27A266a 和 iOS / Simulator SDK 27.0。版本不匹配会明确报错；更新工具链需要评审并更新 iOS 专属 manifest。脚本优先使用 `~/go/pkg/mod` 中的 Go 1.25.5，也可通过 `LIBBOX_GO` 指定同版本可执行文件。首次准备该版本后再构建：

安装多个 Xcode 时，可为构建命令设置 `DEVELOPER_DIR` 指向符合此 manifest 的 Xcode 27.0；无需更改全局 `xcode-select`。只有 Xcode27.1 时，当前六片构建器会拒绝其工具链；本轮 App 使用已核验的历史 Framework。新共享补丁、依赖与 Apple 工具链整组冻结后再迁移，不能只更改版本校验或把历史 Framework 标作最终核心产物。

```sh
VERSION=$(python3 -c 'import json; print(json.load(open("src-tauri/core-manifest.json"))["bundledCoreVersion"])')
git clone --depth 1 --branch "v$VERSION" https://github.com/SagerNet/sing-box.git "$HOME/Code/sing-box-ios-v$VERSION"
GOTOOLCHAIN=go1.25.5 go version
bash scripts/build-libbox-ios.sh "$HOME/Code/sing-box-ios-v$VERSION" --historical
python3 scripts/ios-libbox.py verify --historical
python3 scripts/ios-libbox-verify.test.py --historical
```

Go 默认下载已由核心 `go.mod` / `go.sum` 锁定并校验的缺失模块；缓存齐全后可加 `--offline`。gomobile/gobind 安装到临时 checkout 内，不覆盖已有工具。构建使用上游 Apple tags，额外保留 `with_dhcp`、`grpcnotrace` 和 iOS 的 `with_low_memory`，保留 naive/cronet；只生成 iPhone/iPad arm64 与模拟器 arm64/x86_64，不生成 macOS 或 Android 产物。

源码 race 测试、Objective-C 新 API、二进制 tags / 架构与完整文件哈希均通过后，历史脚本才将 `Libbox.xcframework` 与 `libbox-build-receipt.json` 放到 `src-tauri/gen/apple/Frameworks/`。新 receipt 的 evidenceScope 为 `historical-only`，记录 source、patch、toolchain、测试排除及 framework 证据；`verify --historical` 可再次核验，反例脚本检查错版本、错补丁、伪造清理结论及被修改的 framework 均被拒绝。该 receipt 只证明历史构建，构造验证仍为 `CleanupUnknown`。六 manager replacement 发布顺序 P2 已在旧共享候选 `fbdb20c0` 独立复审并集入，六片包含此修订；仍不授予 Exact / NoOwner 或 iOS 运行期资源删除许可。Darwin 不适用的 Linux fake iptables 用例具名记录为排除，不计通过。

平台 protocol 新增 `BindInterfaceControl`，Swift conform 必须适配；真实 binding 调用仅在 Android 分支。需要传播最终 service close 错误的 iOS host 使用 `LibboxNewStrictCommandServer`。构建输入与依赖说明见 [iOS patch manifest](../scripts/libbox-ios-patches/README.md)。

## 无签名归档与历史结构核验

当前工程的 final 构建仍被入口保护阻断。完成真实 Apple 来源迁移、重新冻结并核验最终 Libbox 后，才能执行以下统一构建流程；不要给 Xcode hook 加历史选项绕过它：

```sh
cd ui
pnpm install --frozen-lockfile
cd ../src-tauri
../ui/node_modules/.bin/tauri ios build --debug --target aarch64 --no-sign --ci --ignore-version-mismatches
cd ..
python3 scripts/verify-ios-archive.py src-tauri/gen/apple/build/arm64/Polaris.ipa
```

构建 hook 自动准备随包 dashboard 并构建前端。产物在 `src-tauri/gen/apple/build/arm64/`；无签名 IPA 仅供检查编译、架构与资源，不能直接安装到设备。当前 `src-tauri/tauri.ios.conf.json` 将最低 iOS 版本设为 17.0，Xcode 工程的 device family 为 iPhone 和 iPad。`Frameworks/Libbox.xcframework` 与 `gen/apple/build/` 均被忽略；不要提交签名证书、profile 或 IPA。

对已有历史 IPA，仅可显式执行 `python3 scripts/verify-ios-archive.py --historical <旧IPA路径>` 的结构核验。这个结果是 structure-only；当前没有最终 source receipt 到静态链接 IPA 的证据验证实现。

Apple Silicon 模拟器可将目标换成 `--target aarch64-sim`。当前 Tauri CLI 2.11.4 将可用 Simulator runtime 与 SDK 版本比较；若使用 SDK27.1 而仅安装 27.0 runtime，普通 CLI 归档会提前失败。可启动 `tauri ios build --debug --target aarch64-sim --no-sign --ci --ignore-version-mismatches --open` 保持该 options server 活动，随后由 Xcode 对实际已安装的 27.0 目标执行无签名 build；测试记录必须绑定实际 SDK/runtime，不能据此声称 27.1/Duo runtime 已验。`--ignore-version-mismatches` 仅处理当前固定 OS 插件 JS/Rust 版本提示，不放宽源码或 Framework 哈希验证。安装实际 App 后，用 [独立 XCUITest 工程](../scripts/ios-simulator-acceptance/README.md) 检查五个页面、可触达导航、横竖屏布局和重新启动。模拟器可验收其实际支持的行为；签名和设备专属行为另外验证，UI 测试不能证明 VPN 流量或资源归属。

若重复模拟器构建在最后一步报 `failed to rename app ... Directory not empty`，先将 `build/arm64-sim/Polaris.app` 移到自己的备份目录，再重跑同一构建命令。修改 Swift 源文件后重新生成工程；若同步源码副本，须在同步完成后生成，避免旧 `project.pbxproj` 覆盖新源文件登记。

## 签名与 Network Extension

在 Xcode 的 **Settings → Accounts** 确认 Team，然后为 `polaris_iOS` 和 `polaris_PacketTunnel` 两个 target 配置同一个 Team、Network Extensions / Packet Tunnel 与相同的 App Groups capability。Apple DTS 确认，付费开发者创建 Packet Tunnel Provider [无需另行申请特批](https://developer.apple.com/forums/thread/819032)；Xcode 自动签名仍需为两个 target 生成包含这些 entitlement 的配置文件。

`project.yml` 的 `POLARIS_BUNDLE_ID` 默认 `com.polaris.app`，扩展为其加 `.PacketTunnel`，App Group 为 `group.<POLARIS_BUNDLE_ID>`。源码编译者应选自己 Team 可注册的唯一 ID，将 iOS 配置的 `identifier` 与主 App Bundle ID 保持一致，并在两个 target 使用同一个 App Group。更改工程生成配置后运行 `xcodegen generate`，再由 Xcode 选择签名 Team；macOS 工程与签名流程不参与这些步骤。

主 App 将配置与规则存入 App Group；扩展在自己的进程内启动 Libbox。首次启动 VPN 会触发系统保存 VPN 配置的授权。原生桥只操作 bundle ID 匹配的 Polaris profile；同时存在重复 Polaris profile 时会报错，要求先在系统设置去重。

## 生命周期与能力边界

主 App 的存活缓存由系统 NE 状态观测更新。冷启动发现活动会话时只对账，保留该会话并提示先在 iOS 系统设置中停止；不将 profile 匹配当作配置已接管。起停操作与扩展报告绑定 session、request 和配置 digest，晚到回执不能复活已停止的请求。

关闭或重启 iOS 主 App 保留系统 Packet Tunnel，不执行桌面的退出停核流程。停止是独立的显式操作；已有 profile 没有匹配的扩展终态回执时，系统断开仍被报告为不确定。

当前启动必须由主 App 携带本次配置和身份发起。系统设置重新启动或系统自动重启若传入 `options=nil`，扩展会拒绝启动；尚未实现从共享文件安全恢复的能力。

扩展将 Go 工作放在串行 worker，RPC reload 返回“已接受”，完成状态另记。网络设置、起停与 reload 均有期限；网络设置超时仍可能在系统中生效，当前扩展因此进入不确定状态，拒绝启动另一核。严格 `CloseService` 失败持续写入报告，即使 Go 已清空实例引用也不抹除。报告始终为 `CleanupUnknown`；`runtimeStopped` 和系统断开均不是 Exact / NoOwner。

当前 iOS 没有跨进程资源删除许可，配置保存仅记录删除意图，自动消费删除 journal 保持关闭。删除节点或规则可修改期望配置，但相应旧 state / 资源会保留；不能用主 App 未运行或 NE 断开作为物理删除依据。此限制也覆盖独立 TS 登出的原生命令。

iOS 的 TS 表单可保存节点、Auth Key 并打开主 VPN 返回的授权 URL。独立 TS 登录核和登出尚无 iOS 原生承载，入口按真实能力禁用；保存配置不表示已经登录。TS 状态和文件传输沿用现有入口，其运行可用性仍需核验。

## 共享 DNS 回调契约依赖

当前 `TunnelPlatform.localDNSTransport()` 返回 `nil`；固定 Go 的 `experimental/libbox/config.go` 只在该接口非空时注册 platform DNS transport，因此当前 iOS 接入没有通过该入口触达 Android 的 `ExchangeContext` 回调路径。`NEDNSSettings` 只是隧道 DNS 设置，不提供 callback 排空或查询 owner 证明，DNS 流量仍须另验。

Android 正分片修复 SDK 取消不等待晚 callback、callback 直写 `ExchangeContext`、Go `OnCancel` 无注销及 transport 未跟踪真实平台调用的问题。这些修订尚未成为当前 iOS 冻结输入。未来接平台 resolver 时，异步 callback 仅写纯 mailbox，由原调用栈在返回前交付；取消/超时不结束真实 query lease。Go 取消注册须可注销并等待已进入的 callback 退出，transport seal 与 query admission 共用 gate，lease 只在平台调用真实返回且注册收尾后结束。不得复制 Android 旧 callback 实现，也不得从 cancel、上层查询返回或 `Close()==nil` 推导 Exact / NoOwner。

本轮仅固定 f699 六片，尚未导入取消第七片。私有 AfterFunc/lease helper 的审核不构成公开 `dns.go` 已接线或最终 patch/hash 已冻结；不据此重建 Framework。待完整公开接线、最终共享候选及 SHA-256 冻结后，整组更新补丁/manifest，复核生成的 Objective-C 接口、`OnCancel`/结果 ABI、Close 操作语义及 iOS 可达入口/生命周期，再统一重建 Framework/回执/App。当前 construction `f699` pin 不变，queried resolver 仍 Unknown。

## 尚待验证

- 设备上的签名、NE 授权、连接/停止/重连、DNS 与 IPv4/IPv6 流量、naive/H3、规则文件和后台内存。
- 扩展中的 gRPC 管理 API 与统计订阅，以及从系统设置断开、系统终止扩展、主 App 被关闭后的状态对账。系统存活观测已经接线，实际扩展行为仍待运行验证。
- iOS 尚无主 App 启动前的 Libbox 配置预检查。主 App 不链接 Libbox，在扩展里先 CheckConfig 再启动主核仍缺构造工作目录隔离与清理证明；冻结契约返回 CleanupUnknown，不能据此认定构造 owner 已退出或已支持 Android 同款节点预剥离。
- iPhone Duo：Xcode27.1/SDK27.1 已具备 device type 与原生 verticalBarEdge/reservedRegions API；当前仅有26.5/27.0 runtimes，Duo要求27.1 runtime，下载当前不可用。连续单页面/草稿布局、原生四边安全区、键盘、字号和reserved-region消费已接线；外屏/内屏连续切换、半折区域避让仍未在实际Duo模拟器或真机验收。不能用近似视口夹具替代。
- iPad 分屏仍待验证；已有普通横竖屏验收不覆盖分屏或 Duo 折叠区。
