// 内置应用分流预设表 —— **单一真值（SoT）**。16 条，行是维护单元。
//
// 曾经的形态（已终结）：本表自称「来源 = src/shared/app-rules-preset.ts（1:1 提取）」，即 TS 是真源、
// 本表是手抄投影 —— 同一份数据两处维护，必然漂移。现在反过来：**本表是唯一数据源**，前端经
// `app_presets_list` command 一次 invoke 拉取（`ui/src/shared/app-rules-preset.ts` 已无表）。
//
// 为何 UI 列（labelKey/emoji/iconUrl）也在这里 —— 不违背「UI 层不进 config-engine」：
//   ① 它们是**表内数据**不是 UI 逻辑（先例：`category` 早已在此，且注释自认「后端不消费」）。
//   ② **按列切表 = 加一条预设要改两个语言的两个文件**，正是本表要消灭的漂移形态。
//   ③ 真实边界由**类型**守住，不由文件守住：路由生成消费的 `AppPreset` **不含** UI 列
//      （`all_presets()` 投影），UI 列只走 `all_presets_dto()` → `AppPresetDto`。builder 零污染。
// 详见 ~/docs/polaris/design/polaris-dialog-layer-and-governance.md §3.2。

/// Qure Color 彩色图标集基址。上游 `QURE_BASE`。
///
/// 用 `macro_rules!` + `concat!` 而非 `const`：`concat!` 只接受字面量，无法拼 const；
/// 而 `format!` 不是 const 上下文。宏展开后仍是 `&'static str`，零运行时开销、零依赖。
macro_rules! qure {
    ($file:literal) => {
        concat!(
            "https://fastly.jsdelivr.net/gh/Koolson/Qure/IconSet/Color/",
            $file
        )
    };
}

/// 预设数据行（扁平 &'static str 数组，运行时投影为 `AppPreset` / `AppPresetDto`）。
struct PresetRaw {
    id: &'static str,
    /// i18n key，对应 `rules.apps.XXX`（UI 列）。
    label_key: &'static str,
    /// iconUrl 加载失败时的兜底图标（UI 列）。
    emoji: &'static str,
    /// 彩色图标集 URL（UI 列）。
    icon_url: Option<&'static str>,
    geosite_tags: &'static [&'static str],
    geoip_tags: &'static [&'static str],
    process_names: &'static [&'static str],
    /// Android applicationId（包名）。与 `process_names` 是**同一件事的两个平台形态** —— 桌面按
    /// 进程名/路径认应用，Android 按包名认，故同列同行、不切表（切表就是本文件头部要消灭的漂移形态）。
    ///
    /// **只填能核实的**，其余留空。空 = 该预设在 Android 上不进 `exclude_package`，退回 geosite/geoip
    /// 那条腿（`builder::route` 的 b. 腿，与桌面同源）—— 失效方向是「少排除」而不是「排错应用」，
    /// 且用户可见行为仍是直连。反过来填错一个包名会把**别的**应用踢出隧道，那才是不可接受的一侧。
    ///
    /// 取证（2026-09-04）：逐条拉 `play.google.com/store/apps/details?id=<包名>&hl=en&gl=US`，
    /// 按 **HTTP 200 + `og:title` + JSON-LD `author.name`** 三项同时对上才收；404 或开发者对不上
    /// 一律留空。留空的两条（`epic` / `riot`）各自写了为什么，那是结论不是待办。
    ///
    /// **本列与 `process_names` 的空/非空不必对齐**：两者列的是「该平台上真实存在的那个应用」。
    /// `youtube`/`gemini`/`google` 在桌面是网页（无进程可认）、在 Android 是独立 app，故桌面空、
    /// Android 非空 —— 这是平台事实，不是漏填。由此这三条在 Android 上的排除范围**宽于**桌面
    /// （整个 app vs 仅该预设的域名），那正是两边各自的用户预期。
    package_names: &'static [&'static str],
    category: &'static str,
}

const PRESETS_RAW: &[PresetRaw] = &[
    // ── 视频 ──────────────────────────────────────────
    PresetRaw {
        id: "youtube",
        label_key: "youtube",
        emoji: "▶️",
        icon_url: Some(qure!("YouTube.png")),
        geosite_tags: &["youtube"],
        geoip_tags: &[],
        process_names: &[],
        package_names: &["com.google.android.youtube"],
        category: "video",
    },
    PresetRaw {
        id: "netflix",
        label_key: "netflix",
        emoji: "🎬",
        icon_url: Some(qure!("Netflix.png")),
        geosite_tags: &["netflix"],
        // geoip-netflix 覆盖 Netflix 的 CDN 直连 IP（如 AWS/Akamai 上的 Netflix 专属 IP 段）
        geoip_tags: &["netflix"],
        process_names: &["Netflix", "Netflix.exe"],
        package_names: &["com.netflix.mediaclient"],
        category: "video",
    },
    PresetRaw {
        id: "tiktok",
        label_key: "tiktok",
        emoji: "🎵",
        icon_url: Some(qure!("TikTok.png")),
        geosite_tags: &["tiktok"],
        geoip_tags: &[],
        process_names: &["TikTok", "TikTok.exe"],
        package_names: &["com.zhiliaoapp.musically"],
        category: "video",
    },
    // ── 社交 ──────────────────────────────────────────
    PresetRaw {
        id: "telegram",
        label_key: "telegram",
        emoji: "✈️",
        icon_url: Some(qure!("Telegram.png")),
        geosite_tags: &["telegram"],
        // geoip-telegram 覆盖 Telegram DC（数据中心）的 IP 段，确保 DC IP 直连也走代理
        geoip_tags: &["telegram"],
        process_names: &["Telegram", "Telegram.exe", "Telegram Desktop"],
        package_names: &["org.telegram.messenger"],
        category: "social",
    },
    PresetRaw {
        id: "twitter",
        label_key: "twitter",
        emoji: "🐦",
        icon_url: Some(qure!("X.png")),
        geosite_tags: &["twitter"],
        // geoip-twitter 覆盖 Twitter/X 使用 QUIC 协议时直连的 IP 段
        // 这是修复 Twitter 在系统代理模式下 UDP 流量不走代理的关键
        geoip_tags: &["twitter"],
        process_names: &["Twitter", "X", "Twitter.exe"],
        package_names: &["com.twitter.android"],
        category: "social",
    },
    PresetRaw {
        id: "instagram",
        label_key: "instagram",
        emoji: "📷",
        icon_url: Some(qure!("Instagram.png")),
        geosite_tags: &["instagram"],
        geoip_tags: &[],
        process_names: &["Instagram", "Instagram.exe"],
        package_names: &["com.instagram.android"],
        category: "social",
    },
    // ── AI ────────────────────────────────────────────
    PresetRaw {
        id: "openai",
        label_key: "openai",
        emoji: "🤖",
        icon_url: Some(qure!("ChatGPT.png")),
        geosite_tags: &["openai"],
        geoip_tags: &[],
        process_names: &["ChatGPT", "ChatGPT.exe"],
        package_names: &["com.openai.chatgpt"],
        category: "ai",
    },
    PresetRaw {
        id: "anthropic",
        label_key: "anthropic",
        emoji: "🧠",
        icon_url: Some(
            "https://raw.githubusercontent.com/lige47/QuanX-icon-rule/main/icon/04ProxySoft/claude.png",
        ),
        // 注意：SagerNet geosite 数据库中没有独立的 geosite-anthropic.srs
        // 使用 geosite-category-ai 兜底（包含 OpenAI/Anthropic/Gemini 等主流 AI 服务）
        // 同时加入 claude.ai 域名通过自定义规则覆盖
        geosite_tags: &["anthropic", "category-ai"],
        geoip_tags: &[],
        process_names: &["Claude", "Claude.exe"],
        package_names: &["com.anthropic.claude"],
        category: "ai",
    },
    PresetRaw {
        id: "gemini",
        label_key: "gemini",
        emoji: "✨",
        icon_url: Some(
            "https://raw.githubusercontent.com/lige47/QuanX-icon-rule/main/icon/04ProxySoft/gemini.png",
        ),
        // 使用 geosite-google 同时覆盖 Gemini 所有相关域名（gemini.google.com 等）
        // 注意：如果用户同时启用了 Google 应用分流，两者共享同一个 rule_set 不会冲突
        // sing-box 会自动去重，不会重复下载
        geosite_tags: &["google"],
        geoip_tags: &[],
        process_names: &[],
        package_names: &["com.google.android.apps.bard"],
        category: "ai",
    },
    // ── 工具 ──────────────────────────────────────────
    PresetRaw {
        id: "github",
        label_key: "github",
        emoji: "🐙",
        icon_url: Some(qure!("GitHub.png")),
        geosite_tags: &["github"],
        geoip_tags: &[],
        process_names: &["GitHub Desktop", "GitHubDesktop.exe", "git", "git.exe", "GitHub"],
        package_names: &["com.github.android"],
        category: "tools",
    },
    PresetRaw {
        id: "google",
        label_key: "google",
        emoji: "🔍",
        icon_url: Some(qure!("Google_Search.png")),
        geosite_tags: &["google"],
        geoip_tags: &[],
        process_names: &[],
        package_names: &["com.google.android.googlequicksearchbox"],
        category: "tools",
    },
    PresetRaw {
        id: "spotify",
        label_key: "spotify",
        emoji: "🎧",
        icon_url: Some(qure!("Spotify.png")),
        geosite_tags: &["spotify"],
        geoip_tags: &[],
        process_names: &["Spotify", "Spotify.exe"],
        package_names: &["com.spotify.music"],
        category: "tools",
    },
    // ── 游戏 ──────────────────────────────────────────
    PresetRaw {
        id: "steam",
        label_key: "steam",
        emoji: "🎮",
        icon_url: Some(qure!("Steam.png")),
        geosite_tags: &["steam"],
        geoip_tags: &[],
        // 包含 Steam 客户端及常见热门游戏的进程名
        // 游戏通过 Steam 启动后是独立进程，仅匹配 "Steam" 无法覆盖游戏本体的 UDP 流量
        process_names: &[
            // Steam 客户端及辅助进程（steam_osx = macOS Steam binary）
            "Steam", "steam.exe", "steamwebhelper", "steamwebhelper.exe",
            "GameOverlayUI.exe", "GameOverlayUI", "steam_osx",
            // 热门 FPS / 竞技游戏（CS2 / Dota 2 / PUBG / Apex / Tarkov / Fortnite）
            "cs2", "cs2.exe", "dota2", "dota2.exe",
            "TslGame.exe", "TslGame", "r5apex.exe", "r5apex",
            "EscapeFromTarkov.exe", "FortniteClient-Win64-Shipping.exe",
            // MOBA / RPG（炉石 / 原神 / 星穹铁道 / 绝区零）
            "Hearthstone.exe", "Hearthstone",
            "GenshinImpact.exe", "YuanShen.exe", "StarRail.exe", "ZenlessZoneZero.exe",
            // 生存 / 沙盒（Valheim / Rust / 幻兽帕鲁）
            "valheim.exe", "valheim", "RustClient.exe", "Palworld-Win64-Shipping.exe",
        ],
        package_names: &["com.valvesoftware.android.steam.community"],
        category: "game",
    },
    PresetRaw {
        id: "epic",
        label_key: "epic",
        emoji: "🎮",
        icon_url: Some(
            "https://raw.githubusercontent.com/lige47/QuanX-icon-rule/main/icon/07Game/epicgames.png",
        ),
        geosite_tags: &["epicgames"],
        geoip_tags: &[],
        process_names: &[
            "EpicGamesLauncher", "EpicGamesLauncher.exe", "EpicWebHelper.exe",
            "FortniteClient-Win64-Shipping.exe", "UnrealEngineLauncher.exe",
        ],
        // 空是**核实结论**不是待办：Epic Games Store 为规避 Play 抽成，Android 版自 2024-08-16 起
        // 绕开 Play 独立分发，`com.epicgames.portal` 在 Play 上 404。Play 里那个 Epic 官方的
        // `com.epicgames.ega` 是账号/2FA/聊天配套 app，不是商店本体，不能当等价物。
        package_names: &[],
        category: "game",
    },
    PresetRaw {
        id: "riot",
        label_key: "riot",
        emoji: "⚔️",
        icon_url: Some(
            "https://raw.githubusercontent.com/lige47/QuanX-icon-rule/main/icon/07Game/riot.png",
        ),
        geosite_tags: &["riot"],
        geoip_tags: &[],
        process_names: &[
            "RiotClientServices.exe", "RiotClientServices",
            // 英雄联盟客户端 / 游戏本体 / Valorant / Legends of Runeterra
            "LeagueClient.exe", "LeagueClient",
            "League of Legends.exe", "League of Legends",
            "VALORANT-Win64-Shipping.exe", "LoR.exe",
        ],
        // 空是**核实结论**：Riot 在 Play 上没有单一官方客户端，只有逐游戏包名
        // （`com.riotgames.league.wildrift` / `...teamfighttactics` / `...legendsofruneterra`）
        // 加一个资讯类配套 app `com.riotgames.mobile.leagueconnect`。桌面那串进程名对应的
        // 端游在 Android 上不存在，硬凑一个包名等于把「排除 Riot」实现成「排除某一款手游」。
        package_names: &[],
        category: "game",
    },
    PresetRaw {
        id: "disney",
        label_key: "disney",
        emoji: "🏰",
        icon_url: Some(qure!("Disney.png")),
        geosite_tags: &["disney"],
        geoip_tags: &[],
        process_names: &["Disney+"],
        package_names: &["com.disney.disneyplus"],
        category: "video",
    },
];

/// 路由生成投影：`Vec<AppPreset>`（**不含 UI 列** —— builder 消费面零污染）。
pub(crate) fn all_presets() -> Vec<AppPreset> {
    PRESETS_RAW
        .iter()
        .map(|r| AppPreset {
            id: r.id.to_string(),
            geosite_tags: r.geosite_tags.iter().map(|s| s.to_string()).collect(),
            geoip_tags: r.geoip_tags.iter().map(|s| s.to_string()).collect(),
            process_names: r.process_names.iter().map(|s| s.to_string()).collect(),
            package_names: r.package_names.iter().map(|s| s.to_string()).collect(),
            category: r.category.to_string(),
        })
        .collect()
}

/// 全列投影：`Vec<AppPresetDto>`（含 UI 列）—— `app_presets_list` command 下发前端。
///
/// **刻意不带 `package_names`**：DTO 的键集是与前端 `AppPreset` interface 的逐字契约
/// （`tests/frontend_sot_guard.rs` 锁死），而包名当前无任何渲染消费方。加一个前端不读的键
/// 只会让那道契约门变松（多出的键谁都不看，漂了也没人知道）。
pub fn all_presets_dto() -> Vec<AppPresetDto> {
    PRESETS_RAW
        .iter()
        .map(|r| AppPresetDto {
            id: r.id.to_string(),
            label_key: r.label_key.to_string(),
            emoji: r.emoji.to_string(),
            icon_url: r.icon_url.map(str::to_string),
            geosite_tags: r.geosite_tags.iter().map(|s| s.to_string()).collect(),
            geoip_tags: r.geoip_tags.iter().map(|s| s.to_string()).collect(),
            process_names: r.process_names.iter().map(|s| s.to_string()).collect(),
            category: r.category.to_string(),
        })
        .collect()
}
