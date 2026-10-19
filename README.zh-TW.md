# Polaris

<div align="center">

[简体中文](README.md) · [English](README.en.md) · **繁體中文** · [Русский](README.ru.md) · [فارسی](README.fa.md)

[![release](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fapi.github.com%2Frepos%2Fpolaris-arch%2FPolaris%2Freleases%2Flatest&query=%24.tag_name&style=flat-square&color=0E98A4&label=release)](https://github.com/polaris-arch/Polaris/releases/latest)
[![sing-box](https://img.shields.io/badge/sing--box-1.15-0E98A4?style=flat-square)](https://github.com/polaris-arch/sing-box-polaris)
[![platform](https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux%20%C2%B7%20Android-0E98A4?style=flat-square)](#安裝)
[![license](https://img.shields.io/badge/license-MIT-0E98A4?style=flat-square)](LICENSE)
[![stars](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fapi.github.com%2Frepos%2Fpolaris-arch%2FPolaris&query=%24.stargazers_count&style=flat-square&color=0E98A4&label=stars)](https://github.com/polaris-arch/Polaris/stargazers)

</div>

**北極星** — 基於 [sing-box-polaris](https://github.com/polaris-arch/sing-box-polaris) 的跨平台網路代理用戶端。Tauri 2（Rust + React）。

![首頁](docs/screenshots/home.png)

## 功能

| 領域 | 能力 |
|---|---|
| 接管方式 | TUN · 系統代理 / 本機連接埠（桌面） |
| 分流 | 智慧 / 全域 / 直連 · 自訂規則 · 應用程式分流 · 地區分流（智慧模式，含回國） |
| 一般代理 | VLESS · VMess · Trojan · Hysteria 2 / 1 · TUIC · Shadowsocks · AnyTLS · Naive · Snell · SOCKS · HTTP · SSH · Tor[1](#tor-note) |
| 組網接入[2](#mesh-note) | WireGuard · Tailscale · WARP (WireGuard) · OpenConnect · OpenVPN · MASQUE[3](#masque-note) |
| DNS | FakeIP · DoH / DoT · 節點網域解析競速 |
| 網路控制 | IPv6 開關 · 瀏覽器 DoH 攔截 · WebRTC 策略（僅 TUN） |
| 診斷 | 連線拓撲 · 即時記錄 · 節點測速與進度 · 規則命中顯示 · 串流與 AI 解鎖偵測 |
| 維運 | 訂閱管理 · 設定備份還原與確認 · 隱私鎖 · 桌面系統匣常駐 |
| 應用程式更新 | 正式版 / 測試版通道 · 可重新下載目前版本 · 安裝檔摘要驗證與缺失提示 |
| 桌面記憶體最佳化 | 啟用自動輕量模式後，主視窗連續隱藏或最小化 10 分鐘會釋放主 WebView；統計、連線與記錄按需訂閱 |

<a id="tor-note"></a>

1. Tor 需要可用的 Tor 執行檔；平台上的實際執行條件須另行滿足。

<a id="mesh-note"></a>

2. 組網新增入口包含 OpenConnect、OpenVPN 和 MASQUE；內網路由能力仍取決於設定的內網網段（meshRoutes），不能僅憑入口分類判斷。

<a id="masque-note"></a>

3. MASQUE 使用 sing-box 自有實作，不支援 Cloudflare 官方的 MASQUE 接入（包括 WARP 的 MASQUE 接入）。此限制僅針對該接入方式。

<table>
<tr>
<td width="50%"><img src="docs/screenshots/nodes.png" alt="節點"><br><sub>節點管理與測速</sub></td>
<td width="50%"><img src="docs/screenshots/rules.png" alt="規則"><br><sub>自訂分流規則</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/connections.png" alt="連線"><br><sub>即時連線</sub></td>
<td><img src="docs/screenshots/settings.png" alt="設定"><br><sub>設定</sub></td>
</tr>
</table>

## 安裝

正式包發布後，可從 [Releases](https://github.com/polaris-arch/Polaris/releases) 下載對應平台安裝包；原始碼或 CI 驗證通過不表示已有公開下載。

目前發布流程依 [release-assets.mjs](scripts/release-assets.mjs) 使用以下檔名，`<version>` 為應用程式版本（不含 `v` 前綴）。macOS 按 Apple Silicon / Intel 分包，Windows 為 x64，Linux 為 amd64；Android 依序為 ARM64、ARMv7 和 universal，universal 僅含這兩種 ARM 架構，不含 x86。下載時以所選 Release 的實際附件為準，並用同頁 `SHA256SUMS` 核對檔案。

| 平台 | 檔案 |
|---|---|
| macOS | `Polaris_<version>_aarch64-mac.dmg` / `Polaris_<version>_x64-mac.dmg` |
| Windows | `Polaris_<version>_x64-win-setup.exe` / `Polaris_<version>_x64-win-Portable.zip` |
| Linux | `Polaris_<version>_amd64-linux.deb` / `Polaris_<version>_amd64-linux.AppImage` |
| Android | `Polaris_<version>_arm64-v8a-android.apk` / `Polaris_<version>_armeabi-v7a-android.apk` / `Polaris_<version>_universal-android.apk` |

桌面安裝檔目前不做付費程式碼簽章，首次啟動需依平台放行。

Windows 安裝程式不內嵌 WebView2 Runtime；系統缺少時會連網取得。精簡版 / LTSC 或免安裝版使用者若缺少
Runtime，請先從微軟官方下載並安裝 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。
Polaris 不提供離線 WebView2 安裝檔。

### macOS 首次安裝

macOS 可能提示應用程式「已損毀」或「無法驗證開發者」。未簽章或未經公證
的版本可能被阻擋，但這些提示不能證明檔案安全，也可能涉及損毀或惡意修改。

1. 安裝前，確認 DMG 來自專案官方 Release，且符合預期的平台及版本。
若發布者提供校驗碼，請核對；校驗碼一致不代表沒有惡意軟體。
自行建置時，應先確認原始碼來源及建置過程。來源或完整性無法確認，
或校驗碼不符時，請停止，不要略過警告。

2. 確認後，將 Polaris 拖入右側的「應用程式 (Applications)」資料夾，
再嘗試正常開啟。

3. 若僅因無法驗證開發者而被阻擋，且你信任已核實的版本，可使用 macOS
提供的「系統設定 → 隱私權與安全性 → 強制打開」（若可用），閱讀
並確認系統提示。介面可能因系統版本而異。

若提示「已損毀」、偵測到惡意軟體，或仍無法開啟，請停止，重新從官方
來源取得並核對檔案，或向專案維護者求助。不要清除隔離屬性或關閉系統
安全保護來強行開啟。每次手動替換都應重新核實檔案。

[README First.txt](packaging/macos-dmg-open-guide.txt) · [Apple Support](https://support.apple.com/en-us/102445)

### Windows 首次安裝

SmartScreen 提示時選擇「其他資訊」→「仍要執行」。

## 建置

以下供維護者建置目前原始碼；原始碼建置成功不等於已發布安裝包通過執行驗收。平台相依套件、行動工具鏈與打包約定見下方建置文件。

原始碼建置入口：[桌面與 Android](docs/build-and-package.md) · [iOS / iPadOS](docs/ios-source-build.md) · [行動核心與固定工具鏈](scripts/mobile-core/README.md)。

iOS 目前不提供 IPA 或越獄包，越獄相容性未驗證。非越獄裝置自行簽署需具備 Network Extension 能力的 Apple Developer Program 帳號，以及匹配 App、PacketTunnel 與 App Group 權限的 [provisioning profiles](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles)；免費 [Personal Team 不支援 NE provider](https://developer.apple.com/forums/thread/128767)，側載本身不會授予這些權限。編譯成功或安裝成功均不能取代執行驗收。

需要 Rust stable、Node.js 24+（CI 目前使用 Node 26）、pnpm 11.24.0、Go、[Tauri CLI 2](https://v2.tauri.app/)。

請依[完整桌面建置步驟](docs/build-and-package.md#打包出安装包)準備工具鏈與 protoc/PATH、固定核心、helper（Windows 另含 cleaner）及安裝身分清單，再編譯 App。僅下載核心後執行 `tauri build` 不足以產生可用的完整安裝包。

核心不入庫，打包前必須拉取。平台 `--config` 不可省：缺了會打出**沒有核心的安裝檔**，且建置期零錯誤，
直到執行期才暴露。完整說明、CI 分工、Windows 安裝程式與更新器選檔契約見
[建置與打包](docs/build-and-package.md)。

開發門檻：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd ui && npx pnpm@11.24.0 test
```

## 架構

```
ui/          React + Zustand + Vite + Tailwind
src-tauri/   Tauri 2 主行程
crates/      domain crates（config-engine / core-supervisor / helper / updater / …）
resources/   sing-box 核心 + libcronet（建置期拉取，不入庫）
```

桌面端（Windows / macOS / Linux）核心以 sidecar 子行程執行，經 gRPC 管理面通訊。需要特權的 TUN 起核與系統接管委託三平台
Rust helper；無需 helper 的 System Proxy 路徑由應用程式側 `system-integration` 直接管理並負責回復。

Android 將 libbox 載入應用程式行程，採用行動端 VPN 整合，不使用上述桌面 sidecar/helper 路徑。

## 文件

| 檔案 | 內容 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 目前分層、執行交易、DNS/路由、特權邊界與大檔案判定準則 |
| [docs/build-and-package.md](docs/build-and-package.md) | 建置、CI、打包不變量、更新器選檔契約 |
| [docs/troubleshooting.zh-TW.md](docs/troubleshooting.zh-TW.md) | MASQUE 相容限制、不簽章說明、白畫面 / 花屏 / GPU 當機排障 |

截圖由 `node scripts/capture-screenshots.mjs` 產生：無頭 Chrome 算繪前端建置產物 + 注入樁資料，
不需要安裝應用程式或啟動核心。

## 上游

| 專案 | 用途 |
|---|---|
| [sing-box-polaris（實際使用的 fork）](https://github.com/polaris-arch/sing-box-polaris) | 實際使用的核心 fork；凍結原始碼、版本與 sing-box-polaris 資產以所選建置清單為準 |
| [sing-box（原始上游）](https://github.com/SagerNet/sing-box) | 原始上游專案與技術來源；保留致謝 |
| [Tauri 2](https://github.com/tauri-apps/tauri) | 桌面執行階段 |
| [cronet-go](https://github.com/SagerNet/cronet-go) | NaiveProxy 的 libcronet |
| [sing-box-dashboard](https://github.com/SagerNet/sing-box-dashboard) | 內建面板 |
| [meta-rules-dat](https://github.com/MetaCubeX/meta-rules-dat) | 規則集與地理資料（`.srs`） |

各元件版權歸其作者。以子行程 / 二進位形式整合的元件見 `NOTICE`；連結進產物的原始碼層相依套件
（Tauri / React / 數百個 Rust crate）逐一登記在 `THIRD-PARTY-LICENSES.md`。

## 適用範圍與免責聲明

Polaris 是通用網路代理用戶端與診斷工具，不提供、銷售或維運代理節點、訂閱及網路服務。請僅在遵守
所在地法律法規、服務條款與所在網路管理制度，並已取得必要授權的情境中使用；不得用於未經授權的存取、
侵害他人權益或其他違法濫用。使用者應自行評估設定、節點與第三方資源的可信度，並對使用行為及其後果負責。

本軟體按「現狀」提供，不承諾網路可用性、匿名性、安全性、特定服務可存取性或資料完整性。TUN、系統代理、
DNS 與路由變更可能暫時影響網路連線；進行重要操作前請備份設定。除適用法律另有強制規定外，維護者與貢獻者
不對因使用或無法使用本軟體所產生的直接或間接損失承擔責任。本說明不構成法律或其他專業意見。

## 授權

Polaris 自身原始碼 MIT（見 `LICENSE`）。**但產物的散布條款按平台不同**：

- **桌面（Windows / macOS / Linux）：MIT。** sing-box（GPLv3）以 sidecar 子行程形式整合，
  依 mere-aggregation 立場不影響本專案授權。**該立場未經法院檢驗**，見 `NOTICE`。
- **Android：整體 GPLv3。** 該平台上 sing-box 只能以 libbox 形態載入應用行程
  （tun 描述子只能經行程內介面取得），mere aggregation 不成立；Kotlin 側亦移植自 GPLv3 的
  sing-box-for-android。取用 APK 的人得到 GPLv3，只取本倉 MIT 原始碼檔的人仍得到 MIT。

第三方元件與完整論述見 `NOTICE`。

## Star 趨勢

<a href="https://www.star-history.com/?repos=polaris-arch%2FPolaris&type=date&legend=top-left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=polaris-arch/Polaris&type=date&theme=dark&legend=top-left&sealed_token=TJg9RA5l3wyd1IgSMMq05QxhNvxS_OcrWbDJxZuwdUwgs-zVIBeoZz2j6swI3y5BxlztkoJMSkkxL6ZbZtw6oyqaRHZSAv0ZS60aVPPuBMdvm8tkxUjyKN1ttiVtPUwJEKObGpBH7BsPhjr6JwFfl_20UYjxgRVOq_V_Q6gKleib6K8LqP3K3nSwPvIJ" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=polaris-arch/Polaris&type=date&legend=top-left&sealed_token=TJg9RA5l3wyd1IgSMMq05QxhNvxS_OcrWbDJxZuwdUwgs-zVIBeoZz2j6swI3y5BxlztkoJMSkkxL6ZbZtw6oyqaRHZSAv0ZS60aVPPuBMdvm8tkxUjyKN1ttiVtPUwJEKObGpBH7BsPhjr6JwFfl_20UYjxgRVOq_V_Q6gKleib6K8LqP3K3nSwPvIJ" />
    <img alt="Polaris Star History Chart" src="https://api.star-history.com/chart?repos=polaris-arch/Polaris&type=date&legend=top-left&sealed_token=TJg9RA5l3wyd1IgSMMq05QxhNvxS_OcrWbDJxZuwdUwgs-zVIBeoZz2j6swI3y5BxlztkoJMSkkxL6ZbZtw6oyqaRHZSAv0ZS60aVPPuBMdvm8tkxUjyKN1ttiVtPUwJEKObGpBH7BsPhjr6JwFfl_20UYjxgRVOq_V_Q6gKleib6K8LqP3K3nSwPvIJ" />
  </picture>
</a>
