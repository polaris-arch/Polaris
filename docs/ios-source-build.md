# iOS 源码构建（ARM64，自行签名）

项目保留 iOS 源码与编译入口，不在 GitHub Releases 发布 iOS 安装包。以下步骤说明当前源码的设备 ARM64 无签名编译/链接路径，不表示签名、安装、Network Extension 授权或实际 VPN 流量已验收。

在 Mac 上准备 Rust stable 与 aarch64-apple-ios、Node.js 24+（CI 26）、pnpm 11.24.0、Python 3，以及 Xcode 27.1 build 27A9275；设备与模拟器 SDK 都须 27.1。Go 1.27.2（以 `source-contract.json` 的 `goVersion` 为准）、SagerNet gomobile/gobind v0.1.13 须用明确工具路径提供。本文使用正式 Python 构建入口，不使用历史 `tauri ios build` 流程；通用 Tauri CLI 版本见 `scripts/tauri-cli.version`（当前 2.12.1），不应据此假定 UI 依赖安装会提供本地 CLI。不要运行 `gomobile init` 来替换已固定 gobind。

当前内核来源为 [sing-box-polaris](https://github.com/polaris-arch/sing-box-polaris)，移动端固定输入由 source-contract 锁定。来源选择不表示 App 运行验收完成，也不提供已发布 IPA 下载；核心仓的原始组件不能替代 App 构建器生成并核验的组件收据。

核心来源以 [source-contract.json](../scripts/mobile-core/source-contract.json) 为准。`--source` 指向保存对应 tag/commit/tree 的核心 checkout，不是 Polaris应用目录；不应用历史六片补丁。先准备组件所需现行工具/来源，完整约束见 [移动内核构建说明](../scripts/mobile-core/README.md)。移动源构建无需等待桌面公开 Release，但必须核验这个真实 tag/object/commit/tree；实际 ObjC/Swift importer、App 与 PacketTunnel 编译和原始组件/载体收据仍须本次生成。输入合同中的 artifactPins/producerReceipt 继续为 null，所有验收标志保持 false，不能据 source pin 宣称已验收或提供设备运行结果。

构建前还须运行 `node scripts/fetch-protoc.mjs`，将输出的 `bin` 目录加入当前 shell 的 PATH，并核对 `protoc --version`（当前 `libprotoc 35.1`）。本地脚本不自动修改 PATH；Rust 的 `singbox-grpc` 构建需要该工具。仅在修改 `project.yml` 后重新生成工程时需要 XcodeGen。

在 Polaris 仓库根目录：

```sh
rustup target add aarch64-apple-ios
pnpm --dir ui install --frozen-lockfile
node scripts/fetch-dashboard.mjs
pnpm --dir ui build
python3 -B scripts/mobile-core/build_apple_app.py \
  --source "$CORE_SOURCE" \
  --go "$GO_BIN" --gomobile "$GOMOBILE_BIN" --gobind "$GOBIND_BIN" \
  --developer "$DEVELOPER_DIR" \
  --work "$APPLE_WORK" --cache "$APPLE_CACHE"
```

上述变量须替换/设置成真实明确路径；`DEVELOPER_DIR` 指向选定 Xcode 的 `Contents/Developer`。work/cache 为固定用户自有目录，须位于同一文件系统并满足构建器资源准入；不覆盖已有归档或挪用历史 receipt。入口会生产或重新核验共享组件，再构建并核验设备 App/PacketTunnel；无需额外先跑一次组件构建器。不传 `--signing-team` 即无签名编译/链接。

共享静态组件包含设备与模拟器两个 arm64 slice，最低 iOS 15；App/PacketTunnel最低 iOS 17。默认 Apple托管工作流只要求 device-arm64；模拟器 App 构建是现有手动选项，不由共享双 slice成功自动推出。Intel simulator/macOS/tvOS 不属于本 iOS正式目标。

真机安装须另行准备用户自己的 Apple开发签名身份、已注册设备和匹配的 profiles；主 App与 PacketTunnel使用同一 Team及共同App Group，并具备 Packet Tunnel entitlement。选择自己可注册的 Bundle ID，主 App与 Tauri identifier一致，扩展 ID 加 `.PacketTunnel`；修改 `src-tauri/gen/apple/project.yml` 后，在该文件所在目录运行 `xcodegen generate`，并核对两个 target。现有 `--signing-team` 只选择 Team，不能替代这些条件。实际设备授权、连接/停止、DNS/IPv4/IPv6 及后台生命周期需另验。不要提交证书、profiles 或构建产物。
