//! 宽容反序列化（sanitize-don't-throw，维度7 #7）。
//!
//! Polaris 锚点：`ConfigManager.ts#loadConfig`（反序列化宽容、坏字段跳过不崩）+
//! `validateConfig` 中对各字段的逐项 sanitize（subscriptions/servers/customRules 逐项过滤坏条目、
//! 纯开关字段非 boolean 删除而非 throw）。
//!
//! 纪律（维度7 #7 HIGH）：反序列化绝不因单坏字段崩溃，更绝不清空用户配置。
//! 坏字段策略：
//!   - 结构性必填字段类型错（servers 非数组、proxyMode 非法等）→ [`crate::StoreError::Validation`]，
//!     交 loadConfig catch（已备份不覆盖），与 TS validateConfig 的 throw 同口径。
//!   - 数组内单条坏元素 → 剔除该条、保留其余（servers/subscriptions/customRules/CIDR）。
//!   - 纯开关字段非 boolean → 删除（读取侧 `!== false` 视为默认开，语义自洽）。
//!   - 可选字段类型错 → 删除（回落默认）。

#![forbid(unsafe_code)]

use polaris_config_engine::builder::Platform;
use serde_json::{Map, Value};

/// 解析结果：成功得到清洗后的 Value（可直接 into 强类型），或结构错误。
pub type SanitizeResult = Result<Value, crate::StoreError>;

/// 宽容解析 JSON 字符串 → 清洗后的 [`Value`]。
///
/// 坏 JSON（SyntaxError）→ Err（交上层回落默认配置 + 备份损坏文件，**不覆盖**）。
/// JSON 合法但字段脏 → 逐字段 sanitize，坏字段剔除/删除，返回清洗后的 Value。
///
/// 这一层只做「JSON 形状清洗」；语义校验（端口范围、协议必填等）在 [`crate::validate`] 做。
pub fn sanitize_config(content: &str) -> SanitizeResult {
    let mut value: Value = serde_json::from_str(content).map_err(crate::StoreError::from)?;
    if !value.is_object() {
        // 顶层非对象 → 结构错误（交 catch 回落默认，不覆盖）。
        return Err(crate::StoreError::validation(
            "config root must be an object",
        ));
    }
    // 就地清洗：坏字段剔除 / 退化，绝不 throw。
    sanitize_value_in_place(&mut value);
    Ok(value)
}

/// 就地清洗 Value 树（已知字段逐项 sanitize）。坏字段删除，不 throw。
///
/// 仅清洗 ConfigManager.validateConfig 中会「就地改写 / 剔除单条」的字段；
/// 会 throw 的字段（proxyMode/proxyModeType/logLevel/tunConfig）留到 validate 阶段。
pub(crate) fn sanitize_value_in_place_pub(value: &mut Value) {
    sanitize_value_in_place(value)
}

fn sanitize_value_in_place(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    // ── 数组型字段：非数组删除（留 validate 处理必填语义）──────────────
    // servers / subscriptions / customRules / appRules / bypassProcesses / customAppPresets / rule_resources
    // TS validateConfig：servers/subscriptions 非数组 → throw（交 catch）；
    // customRules/appRules 非数组 → 视为 []（不 throw）。这里统一：非数组删除，
    // validate 阶段对必填的 servers 重新判定（缺失 → throw 或回落）。
    ensure_array_or_remove(obj, "servers");
    ensure_array_or_remove(obj, "subscriptions");
    ensure_array_or_remove(obj, "customRules");
    ensure_array_or_remove(obj, "policyRules");
    ensure_array_or_remove(obj, "trafficRules");
    ensure_array_or_remove(obj, "dnsRules");
    ensure_array_or_remove(obj, "routeRuleOrder");
    ensure_array_or_remove(obj, "dnsRuleOrder");
    ensure_array_or_remove(obj, "dnsServers");
    ensure_array_or_remove(obj, "dnsServerGroups");
    ensure_array_or_remove(obj, "networkProfiles");
    ensure_array_or_remove(obj, "appRules");
    ensure_array_or_remove(obj, "customAppPresets");
    ensure_array_or_remove(obj, "ruleResources");
    ensure_array_or_remove(obj, "bypassProcesses");
    ensure_array_or_remove(obj, "fakeIpFilterList");

    // ── 纯开关字段：非 boolean 删除（读取侧 !== false 视为开）──────────
    // appRoutingEnabled / singboxDashboard / mainSessionViaProxy / hardwareAcceleration /
    // windowEffects / fakeIpFilter / blockQuic / allowLan / bypassLAN / enableIPv6 /
    // interruptConnectionsOnSwitch / tlsFragment
    for key in [
        "appRoutingEnabled",
        "singboxDashboard",
        "mainSessionViaProxy",
        "hardwareAcceleration",
        "windowEffects",
        "fakeIpFilter",
        "blockQuic",
        "allowLan",
        "bypassLAN",
        "enableIPv6",
        "interruptConnectionsOnSwitch",
        "tlsFragment",
        "autoStart",
        "silentStart",
        "autoConnect",
        "minimizeToTray",
        "autoCheckUpdate",
        "autoLightweightMode",
        "keepTrayMenuWarm",
        "keepTrayMenuWarmDefaultMigrated",
        "rememberWindowSize",
        "restartOnNodeChange",
        "disableLogFile",
        "desktopNotifications",
        "autoUpdateSubscriptionOnStart",
        "ruleResourceAutoUpdate",
        "autoPrivacyMode",
        "periodicSpeedTestEnabled",
    ] {
        bool_or_remove(obj, key);
    }

    // restartOnNodeChange：TS validateConfig 归一为严格 boolean（=== true）。
    // 非 true → false（确保 switchMode 的真值转换不致 ON）。这里 sanitize 已删非 bool；
    // 若为 true 保留；缺失时不回填（读取端默认 false）。
    if matches!(obj.get("restartOnNodeChange"), Some(Value::Bool(true))) {
        // 保留 true
    } else if obj.contains_key("restartOnNodeChange") {
        obj.insert("restartOnNodeChange".into(), Value::Bool(false));
    }

    // ── 端口字段：非正整数删除（留 validate 校验范围）────────────────
    for key in ["mixedPort", "controlPort", "httpPort", "socksPort"] {
        port_or_remove(obj, key);
    }

    // ── 字符串字段：非字符串删除 ──────────────────────────────────
    // language（空串也删——TS 注释：留空串会让渲染端迁移 effect !language 守卫永不收敛）
    string_or_remove(
        obj, "language", /*trim*/ true, /*reject_empty*/ true,
    );
    string_or_remove(obj, "singboxDashboardUrl", true, true);
    string_or_remove(obj, "clashApiSecret", false, false);
    string_or_remove(obj, "speedTestUrl", false, false);
    // subscriptionProxyPolicy：三态，非法值删除（回落 follow）
    policy_or_remove(obj, "subscriptionProxyPolicy");
    // 已退役的内核更新设置：应用内不再更新内核，旧配置里留下的键一律删掉，不随配置继续流转。
    for key in [
        "autoUpdateCore",
        "coreUpdateChannel",
        "restrictCoreUpdateToCompatibleMinor",
    ] {
        obj.remove(key);
    }
    // appUpdateChannel：二态（stable / prerelease），非法值删除；缺省 stable。
    update_channel_or_remove(obj, "appUpdateChannel");
    // 测速并发：`"auto"` 或 4 到本平台上限的整数，其余删除（回落自动）。上限随平台不同，
    // 所以一份在桌面设为 48 的配置拿到手机上会被清成自动。
    speed_test_concurrency_or_remove(obj, Platform::current());
    // 计费网络下的周期测速：三态，非法值删除（回落降频）。
    if obj
        .get("speedTestMeteredPolicy")
        .is_some_and(|v| !matches!(v.as_str(), Some("pause" | "reduced" | "normal")))
    {
        obj.remove("speedTestMeteredPolicy");
    }

    // ── 对象字段：非对象删除 ─────────────────────────────────────
    // tunConfig / dnsConfig / regionRouting / builtinGeoMeta
    for key in [
        "tunConfig",
        "dnsConfig",
        "regionRouting",
        "dnsDefaults",
        "routeDefaults",
        "networkInterfaces",
    ] {
        object_or_remove(obj, key);
    }
    // builtinGeoMeta：非对象删除（TS：读取侧全容错、无爆炸半径）
    object_or_remove(obj, "builtinGeoMeta");

    // ── selectedServerId：哨兵 '__direct__' / '__block__' 或 string 或 null ──────
    // 非 string/null 删除（留 validate 校验存在性）
    match obj.get("selectedServerId") {
        Some(Value::Null) | None => {}
        Some(Value::String(_)) => {}
        _ => {
            obj.remove("selectedServerId");
        }
    }

    // ── 嵌套：逐节点 / 逐订阅 / 逐规则清洗 ─────────────────────────
    sanitize_servers(obj);
    sanitize_subscriptions(obj);
    // 订阅清洗之后再判：意图指向的订阅必须在清洗后的订阅表里。
    sanitize_selection_intent(obj);
    sanitize_network_interfaces(obj);
    sanitize_custom_rules(obj);
    sanitize_policy_rules(obj);
    sanitize_rules(obj, "trafficRules", false);
    sanitize_rules(obj, "dnsRules", false);
    sanitize_dns_policy(obj);
    sanitize_network_profiles(obj);
    sanitize_tun_config(obj);
    sanitize_dns_config(obj);
}

/// 非 array → 删除键。
fn ensure_array_or_remove(obj: &mut Map<String, Value>, key: &str) {
    if let Some(v) = obj.get(key) {
        if !v.is_array() {
            obj.remove(key);
        }
    }
}

/// 非 bool → 删除键（读取侧 !== false 视为开，删除=回落默认开，语义自洽）。
fn bool_or_remove(obj: &mut Map<String, Value>, key: &str) {
    if let Some(v) = obj.get(key) {
        if !v.is_boolean() {
            obj.remove(key);
        }
    }
}

/// 非正整数（u16 范围）→ 删除键。
fn port_or_remove(obj: &mut Map<String, Value>, key: &str) {
    if let Some(v) = obj.get(key) {
        let ok = match v {
            Value::Number(n) => n
                .as_u64()
                .map(|u| (1..=65535).contains(&(u as u32)))
                .unwrap_or(false),
            _ => false,
        };
        if !ok {
            obj.remove(key);
        }
    }
}

/// 非字符串 / （可选）空白 → 删除键。
fn string_or_remove(obj: &mut Map<String, Value>, key: &str, trim: bool, reject_empty: bool) {
    if let Some(v) = obj.get_mut(key) {
        match v {
            Value::String(s) => {
                if trim {
                    let t = s.trim().to_string();
                    *s = t;
                }
                if reject_empty && s.trim().is_empty() {
                    obj.remove(key);
                }
            }
            _ => {
                obj.remove(key);
            }
        }
    }
}

/// subscriptionProxyPolicy 三态校验：非 follow/proxy/direct → 删除（回落 follow）。
fn policy_or_remove(obj: &mut Map<String, Value>, key: &str) {
    if let Some(Value::String(s)) = obj.get(key) {
        if !matches!(s.as_str(), "follow" | "proxy" | "direct") {
            obj.remove(key);
        }
    } else if obj.contains_key(key) {
        obj.remove(key);
    }
}

/// 更新通道只认 `stable` / `prerelease`，其余（含非字符串）删除。
///
/// 删除而非回落写值：缺省本就等价 `stable`（读侧 `unwrap_or(false)`），写一个显式值只会在
/// config 里留下一条用户从没设过的记录。
fn update_channel_or_remove(obj: &mut Map<String, Value>, key: &str) {
    if let Some(Value::String(s)) = obj.get(key) {
        if !matches!(s.as_str(), "stable" | "prerelease") {
            obj.remove(key);
        }
    } else if obj.contains_key(key) {
        obj.remove(key);
    }
}

/// 探针池的槽位上限，也是「测速并发」可填的最大值。槽位在起核时一次建好，实际并发在上限内
/// 每轮另定。全部改回 16 即回到改动前的池大小。
#[must_use]
pub const fn speed_test_slot_cap(platform: Platform) -> usize {
    match platform {
        Platform::Mac | Platform::Win | Platform::Linux => 64,
        Platform::Android => 32,
        Platform::Ios | Platform::Other => 16,
    }
}

/// 周期测速的全局总开关缺省值。关掉后任何订阅都不建周期计划。
pub const PERIODIC_SPEED_TEST_ENABLED_DEFAULT: bool = true;

/// 逐订阅「周期测速」开关的缺省值。读取侧、类型默认与测试都从这一处取，改默认只动这里。
pub const PERIODIC_SPEED_TEST_SUBSCRIPTION_DEFAULT: bool = false;

/// 计费网络下周期测速的缺省行为：`reduced`（周期乘 4）。另两个取值是 `pause` 与 `normal`。
pub const SPEED_TEST_METERED_POLICY_DEFAULT: &str = "reduced";

/// 逐订阅测速周期留空时的取值（分钟）。
pub const SPEED_TEST_INTERVAL_MINUTES_DEFAULT: u64 = 30;

/// 「测速并发」可填的最小值。
pub const SPEED_TEST_CONCURRENCY_MIN: u64 = 4;

/// 逐订阅测速周期（分钟）的取值范围。
pub const SPEED_TEST_INTERVAL_MINUTES: std::ops::RangeInclusive<u64> = 5..=360;

fn speed_test_concurrency_or_remove(obj: &mut Map<String, Value>, platform: Platform) {
    let ok = match obj.get("speedTestConcurrency") {
        None => return,
        Some(Value::String(s)) => s == "auto",
        Some(v) => v.as_u64().is_some_and(|n| {
            (SPEED_TEST_CONCURRENCY_MIN..=speed_test_slot_cap(platform) as u64).contains(&n)
        }),
    };
    if !ok {
        obj.remove("speedTestConcurrency");
    }
}

/// 非对象 → 删除键。
fn object_or_remove(obj: &mut Map<String, Value>, key: &str) {
    if let Some(v) = obj.get(key) {
        if !v.is_object() {
            obj.remove(key);
        }
    }
}

/// 逐节点清洗（servers[]）：剔除结构非法节点 + sanitize endpoint CIDR。
///
/// Polaris 锚点 validateConfig servers 段：缺 id/name/未知协议/缺 address/port/协议必填缺失 → 剔除该节点；
/// endpoint（WG/Tailscale）allowedIPs/routes/localAddress 非法 CIDR 丢弃保留合法；
/// allowInternet 非 bool 删除；多个合法 Tailscale 节点均保留，System 接口冲突由生成侧判定。
fn sanitize_servers(obj: &mut Map<String, Value>) {
    let Some(Value::Array(servers)) = obj.get_mut("servers") else {
        return;
    };
    let mut kept: Vec<Value> = Vec::with_capacity(servers.len());
    for s in servers.drain(..) {
        let Some(so) = s.as_object() else {
            continue; // 非对象 → 丢弃
        };
        // id / name 必须是非空字符串
        if !str_nonempty(so, "id") || !str_nonempty(so, "name") {
            continue;
        }
        // protocol 必须是已知协议（小写匹配）
        let Some(proto) = so.get("protocol").and_then(|v| v.as_str()) else {
            continue;
        };
        let proto_lower = proto.to_ascii_lowercase();
        if !crate::validate::is_allowed_protocol(&proto_lower) {
            continue;
        }
        // **无地址协议** / custom 豁免 address/port；其余必须有合法 address + port∈1..=65535。
        //
        // TS/Tor/Tailcat 的对端由协议设置定位；System TS/WG 同名接口的运行约束不属于存储清洗。
        let addressless = matches!(proto_lower.as_str(), "tailscale" | "tor" | "tailcat");
        let is_custom = proto_lower == "custom";
        if !addressless && !is_custom {
            if !str_nonempty(so, "address") {
                continue;
            }
            let Some(port) = so.get("port").and_then(|v| v.as_u64()) else {
                continue;
            };
            if !(1..=65535).contains(&port) {
                continue;
            }
        }
        // 协议必填校验（单一真值 crate::validate::protocol_requirement_ok）
        if !crate::validate::protocol_requirement_ok(&proto_lower, &s) {
            continue;
        }
        // 拷贝并 sanitize endpoint CIDR
        let mut server = s.clone();
        if let Some(map) = server.as_object_mut() {
            string_or_remove(map, "bindInterface", true, true);
        }
        if let Some(Value::Object(map)) = server.get_mut("wireguardSettings") {
            sanitize_cidr_list(map, "localAddress");
            sanitize_cidr_list(map, "allowedIPs");
            bool_or_remove(map, "allowInternet");
        }
        if let Some(Value::Object(map)) = server.get_mut("tailscaleSettings") {
            sanitize_cidr_list(map, "routes");
            sanitize_cidr_list(map, "advertiseRoutes");
            bool_or_remove(map, "allowInternet");
        }
        // 缺席=存量行为；显式坏策略必须降为 block，绝不可清掉后放开入站。
        if server.get("meshInboundPolicy").is_some() {
            let valid = server.get("meshInboundPolicy").is_some_and(
                polaris_config_engine::user_config::server_config::mesh_inbound_policy_shape_ok,
            ) && serde_json::from_value::<
                polaris_config_engine::user_config::server_config::ServerConfig,
            >(server.clone())
            .ok()
            .is_some_and(|typed| {
                polaris_config_engine::user_config::server_config::validate_mesh_inbound_policy(
                    &typed,
                )
                .is_ok()
                    && typed.mesh_inbound_policy.is_some()
            });
            if !valid {
                server.as_object_mut().expect("server is object").insert(
                    "meshInboundPolicy".into(),
                    serde_json::json!({"mode":"block"}),
                );
            }
        }
        kept.push(server);
    }
    *servers = kept;
}

/// 选择意图的顶层键。缺席即手动选择；自动选择时存 `{ mode, scope, subscriptionId }`。
/// `selectedServerId` 始终是实际出口，自动选择下即当前胜出节点。
pub const SELECTION_INTENT_KEY: &str = "selectionIntent";

/// 自动选择意图指向的订阅 id。键缺席、形状损坏或本版本不认识时为 `None`。
///
/// 读取与清洗共用这一套解析。本版本认识的形状只有一种：`mode == "auto"`、
/// `scope == "subscription"`、`subscriptionId` 为非空字符串。
#[must_use]
pub fn selection_intent_subscription(config: &Value) -> Option<&str> {
    match classify_selection_intent(config.get(SELECTION_INTENT_KEY)?) {
        IntentShape::Subscription(id) => Some(id),
        IntentShape::Unrecognized | IntentShape::Broken => None,
    }
}

/// 配置里是否存着一个结构完好、但本版本不认识的选择意图（更新的版本写的模式或作用域）。
/// 它原样留在盘上，本版本不据它做任何事；读状态时如实报出。
#[must_use]
pub fn selection_intent_unrecognized(config: &Value) -> bool {
    config
        .get(SELECTION_INTENT_KEY)
        .is_some_and(|intent| classify_selection_intent(intent) == IntentShape::Unrecognized)
}

/// 意图键的值属于哪一种形状。
#[derive(Debug, PartialEq, Eq)]
enum IntentShape<'a> {
    /// 本版本认识：自动选择，作用域是一个订阅。
    Subscription(&'a str),
    /// 结构完好（对象，`mode` 与 `scope` 都是非空字符串），但取值本版本不认识。
    Unrecognized,
    /// 损坏：不是对象、缺 `mode` 或 `scope`、类型不对，或认识的形状缺了它必需的字段。
    Broken,
}

fn classify_selection_intent(intent: &Value) -> IntentShape<'_> {
    let field = |name: &str| {
        intent
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let (Some(mode), Some(scope)) = (field("mode"), field("scope")) else {
        return IntentShape::Broken;
    };
    if mode != "auto" || scope != "subscription" {
        return IntentShape::Unrecognized;
    }
    field("subscriptionId").map_or(IntentShape::Broken, IntentShape::Subscription)
}

/// 自动选择意图的规范形（写入方只经这里造值）。
#[must_use]
pub fn selection_intent_auto(subscription_id: &str) -> Value {
    serde_json::json!({
        "mode": "auto",
        "scope": "subscription",
        "subscriptionId": subscription_id,
    })
}

/// 选择意图的清洗：
/// - 形状损坏 → 移除（回到手动选择）；
/// - 本版本认识、但指向的订阅不在订阅表里 → 移除；
/// - 结构完好而取值本版本不认识 → **原样保留**。它多半是更新的版本写的（新的模式或作用域）：
///   在这里删掉，用户回到新版本时意图就丢了。本版本读取时按不生效处理。
pub(crate) fn sanitize_selection_intent(obj: &mut Map<String, Value>) {
    let Some(intent) = obj.get(SELECTION_INTENT_KEY) else {
        return;
    };
    let keep = match classify_selection_intent(intent) {
        IntentShape::Subscription(id) => obj
            .get("subscriptions")
            .and_then(Value::as_array)
            .is_some_and(|subs| {
                subs.iter()
                    .any(|sub| sub.get("id").and_then(Value::as_str) == Some(id))
            }),
        IntentShape::Unrecognized => true,
        IntentShape::Broken => false,
    };
    if !keep {
        obj.remove(SELECTION_INTENT_KEY);
    }
}

/// 逐订阅清洗（subscriptions[]）：缺 id/name/url → 剔除。
fn sanitize_subscriptions(obj: &mut Map<String, Value>) {
    let Some(Value::Array(subs)) = obj.get_mut("subscriptions") else {
        return;
    };
    let mut kept: Vec<Value> = Vec::with_capacity(subs.len());
    for s in subs.drain(..) {
        let Some(so) = s.as_object() else {
            continue;
        };
        if str_nonempty(so, "id") && str_nonempty(so, "name") && str_nonempty(so, "url") {
            let mut sub = s;
            if let Some(map) = sub.as_object_mut() {
                string_or_remove(map, "proxyBindInterface", true, true);
                bool_or_remove(map, "periodicSpeedTest");
                // 越界的周期删除而不是钳到边界：钳位会让用户以为自己填的值在生效。
                if map.get("speedTestIntervalMinutes").is_some_and(|v| {
                    !v.as_u64()
                        .is_some_and(|n| SPEED_TEST_INTERVAL_MINUTES.contains(&n))
                }) {
                    map.remove("speedTestIntervalMinutes");
                }
            }
            kept.push(sub);
        }
    }
    *subs = kept;
}

/// 网卡名是 sing-box 的 OS 设备标识（如 en0 / Wi-Fi）。这里只做字符串边界清洗；接口是否存在
/// 属于运行态事实，由枚举命令、导入预览和起核前校验负责。
fn sanitize_network_interfaces(obj: &mut Map<String, Value>) {
    let Some(Value::Object(policy)) = obj.get_mut("networkInterfaces") else {
        return;
    };
    string_or_remove(policy, "direct", true, true);
    string_or_remove(policy, "proxy", true, true);
    policy.retain(|key, _| matches!(key.as_str(), "direct" | "proxy"));
    if policy.is_empty() {
        obj.remove("networkInterfaces");
    }
}

/// 逐规则清洗（customRules[]）：结构非法丢弃，值非法仅告警保留。
fn sanitize_custom_rules(obj: &mut Map<String, Value>) {
    sanitize_rules(obj, "customRules", true);
}

/// v2 策略与 customRules 共用形状；policyRules 不再放行待迁移的旧 DomainRule。
fn sanitize_policy_rules(obj: &mut Map<String, Value>) {
    sanitize_rules(obj, "policyRules", false);
}

fn sanitize_rules(obj: &mut Map<String, Value>, key: &str, allow_legacy: bool) {
    let Some(Value::Array(rules)) = obj.get_mut(key) else {
        return;
    };
    let mut kept: Vec<Value> = Vec::with_capacity(rules.len());
    for r in rules.drain(..) {
        // 旧版 DomainRule（无 `type` + `domains` 数组）→ **原样保留**，交 migrate 阶段
        // （`migrate::migrate_custom_rules_domain_rule`）转新 Rule。此前 sanitize 先于 migrate 执行、又要求
        // `type` 非空，会在迁移前把无 type 的旧规则整条剥光 → `customRules==[]` 静默全丢（1 旧规则配置过 load
        // 即丢失）。对齐 上游 ConfigManager.validateConfig（L392-395：customRules 逐项校验前先 migrateCustomRules）——
        // 本 crate 把 migrate 拆成独立 pass，故在 sanitize 侧放行旧 shape 待迁移，达成同一 end-state。
        // migrate 产出恒为干净新 Rule（合法 type/values/action/enabled），无需回过 sanitize 逐项清洗。
        if allow_legacy && crate::migrate::is_legacy_domain_rule(&r) {
            kept.push(r);
            continue;
        }
        let Some(ro) = r.as_object() else {
            continue;
        };
        // id 必须是非空字符串
        if !str_nonempty(ro, "id") {
            continue;
        }
        // type 必须是已知规则类型
        let Some(t) = ro.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if !crate::validate::is_known_rule_type(t) {
            continue;
        }
        // values 必须是数组
        if !ro.get("values").is_some_and(|v| v.is_array()) {
            continue;
        }
        // action 必须是 proxy/direct/block
        let action_ok = ro
            .get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|a| matches!(a, "proxy" | "direct" | "block"));
        if !action_ok {
            continue;
        }
        // enabled 必须是 boolean
        if !ro.get("enabled").is_some_and(|v| v.is_boolean()) {
            continue;
        }
        let mut rule = r.clone();
        // 过滤 values 非字符串元素
        if let Some(Value::Array(vals)) = rule.get_mut("values") {
            vals.retain(|v| v.is_string());
        }
        // sanitize conditions（多条件）
        sanitize_rule_conditions(&mut rule);
        // combineMode 非 and/or 删除
        if let Some(cm) = rule.get("combineMode") {
            let ok = cm.as_str().is_some_and(|m| matches!(m, "and" | "or"));
            if !ok {
                if let Some(map) = rule.as_object_mut() {
                    map.remove("combineMode");
                }
            }
        }
        // effects 是统一规则模型的权威效果字段。旧配置没有它时保持原样；字段损坏时只移除
        // 损坏的 effect，让 Rule 退回旧 action/targetServerId 兼容镜像，不能因一个可选字段
        // 反序列化失败而丢掉整份配置。
        sanitize_rule_effects(&mut rule);
        kept.push(rule);
    }
    *rules = kept;
}

/// 清洗 rule.effects：route/dns 独立退化，二者都不可用时删除 effects 并回退旧动作镜像。
fn sanitize_rule_effects(rule: &mut Value) {
    let Some(map) = rule.as_object_mut() else {
        return;
    };
    let Some(raw) = map.get_mut("effects") else {
        return;
    };
    let Some(effects) = raw.as_object_mut() else {
        map.remove("effects");
        return;
    };

    let route_ok = effects.get("route").is_none_or(|route| {
        route.as_object().is_some_and(|route| {
            route
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|action| matches!(action, "proxy" | "direct" | "block"))
        })
    });
    if !route_ok {
        effects.remove("route");
    } else if let Some(Value::Object(route)) = effects.get_mut("route") {
        if route
            .get("enabled")
            .is_some_and(|value| !value.is_boolean())
        {
            route.remove("enabled");
        }
        if route
            .get("targetServerId")
            .is_some_and(|value| !value.is_string())
        {
            route.remove("targetServerId");
        }
        if route.get("destinationResolution").is_some_and(|value| {
            serde_json::from_value::<polaris_config_engine::user_config::DestinationResolution>(
                value.clone(),
            )
            .is_err()
        }) {
            route.remove("destinationResolution");
        }
    }

    let dns_ok = effects.get("dns").is_none_or(|dns| {
        dns.as_object().is_some_and(|dns| {
            dns.get("resolver")
                .and_then(Value::as_str)
                .is_some_and(|resolver| matches!(resolver, "inherit" | "direct" | "proxy"))
                && dns
                    .get("answerMode")
                    .and_then(Value::as_str)
                    .is_some_and(|mode| matches!(mode, "real" | "fakeIp"))
        })
    });
    if !dns_ok {
        effects.remove("dns");
    } else if let Some(Value::Object(dns)) = effects.get_mut("dns") {
        for key in ["enabled", "migratedImplicitResolve"] {
            if dns.get(key).is_some_and(|value| !value.is_boolean()) {
                dns.remove(key);
            }
        }
        if dns.get("action").is_some_and(|value| {
            serde_json::from_value::<polaris_config_engine::user_config::DnsPolicyAction>(
                value.clone(),
            )
            .is_err()
        }) {
            dns.remove("action");
        }
    }

    if !effects.contains_key("route") && !effects.contains_key("dns") {
        map.remove("effects");
    }
}

/// 一等 DNS 配置形状清洗：坏条目逐项剔除，不让一个端口/枚举脏值拖垮整份 UserConfig。
fn sanitize_dns_policy(obj: &mut Map<String, Value>) {
    for key in ["routeRuleOrder", "dnsRuleOrder"] {
        let Some(Value::Array(order)) = obj.get_mut(key) else {
            continue;
        };
        let mut seen = std::collections::BTreeSet::new();
        order.retain(|value| {
            value
                .as_str()
                .is_some_and(|id| !id.is_empty() && seen.insert(id.to_string()))
        });
    }

    if let Some(Value::Array(servers)) = obj.get_mut("dnsServers") {
        let mut seen = std::collections::BTreeSet::new();
        servers.retain(|value| {
            let id = value.get("id").and_then(Value::as_str).unwrap_or("");
            !id.is_empty()
                && seen.insert(id.to_string())
                && serde_json::from_value::<polaris_config_engine::user_config::DnsServerResource>(
                    value.clone(),
                )
                .is_ok()
        });
    }
    if let Some(Value::Array(groups)) = obj.get_mut("dnsServerGroups") {
        let mut seen = std::collections::BTreeSet::new();
        groups.retain(|value| {
            let id = value.get("id").and_then(Value::as_str).unwrap_or("");
            !id.is_empty()
                && seen.insert(id.to_string())
                && serde_json::from_value::<polaris_config_engine::user_config::DnsServerGroup>(
                    value.clone(),
                )
                .is_ok()
        });
    }

    if obj.get("dnsDefaults").is_some_and(|value| {
        serde_json::from_value::<polaris_config_engine::user_config::DnsPolicyDefaults>(
            value.clone(),
        )
        .is_err()
    }) {
        obj.remove("dnsDefaults");
    }
    if obj.get("routeDefaults").is_some_and(|value| {
        serde_json::from_value::<polaris_config_engine::user_config::RoutePolicyDefaults>(
            value.clone(),
        )
        .is_err()
    }) {
        obj.remove("routeDefaults");
    }
}

/// 网络场景形状清洗（spec §7 sanitize）：缺 id / 重复 id / 结构坏的条目逐条丢弃（不能让整份配置失败；
/// 引用它的规则在生成侧按「引用失效」不生成，fail-closed）；`searchDomains` 规范化（去首尾空白与点、
/// 小写，空值丢弃、去重）。值是否合法（CIDR / 域名形状）归写入校验
/// [`crate::validate::validate_for_save`]，这里不静默吞。
fn sanitize_network_profiles(obj: &mut Map<String, Value>) {
    use polaris_config_engine::user_config::network_profile::normalize_search_domain;
    let Some(Value::Array(profiles)) = obj.get_mut("networkProfiles") else {
        return;
    };
    let mut seen = std::collections::BTreeSet::new();
    profiles.retain(|value| {
        let id = value.get("id").and_then(Value::as_str).unwrap_or("").trim();
        !id.is_empty()
            && seen.insert(id.to_string())
            && serde_json::from_value::<polaris_config_engine::user_config::NetworkProfile>(
                value.clone(),
            )
            .is_ok()
    });
    for profile in profiles.iter_mut() {
        let Some(Value::Array(domains)) = profile
            .get_mut("match")
            .and_then(|m| m.get_mut("searchDomains"))
        else {
            continue;
        };
        let mut normalized: Vec<Value> = Vec::with_capacity(domains.len());
        for domain in domains
            .iter()
            .filter_map(Value::as_str)
            .filter_map(normalize_search_domain)
        {
            let domain = Value::String(domain);
            if !normalized.contains(&domain) {
                normalized.push(domain);
            }
        }
        *domains = normalized;
    }
}

/// sanitize rule.conditions（多条件）：非数组删除；逐条件结构校验、过滤非字符串值。
fn sanitize_rule_conditions(rule: &mut Value) {
    let Some(map) = rule.as_object_mut() else {
        return;
    };
    let conds_present = map.contains_key("conditions");
    if !conds_present {
        return;
    }
    let Some(Value::Array(conds)) = map.get_mut("conditions") else {
        // 非数组 → 删除（退化为单条件 type/values）
        map.remove("conditions");
        return;
    };
    let mut cleaned: Vec<Value> = Vec::with_capacity(conds.len());
    for c in conds.drain(..) {
        let Some(co) = c.as_object() else {
            continue;
        };
        let Some(t) = co.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if !crate::validate::is_known_rule_type(t) {
            continue;
        }
        let Some(vals) = co.get("values") else {
            continue;
        };
        if !vals.is_array() {
            continue;
        }
        let mut cond = c.clone();
        if let Some(Value::Array(v)) = cond.get_mut("values") {
            v.retain(|x| x.is_string());
            if v.is_empty() {
                continue; // 空值条件丢弃
            }
        }
        cleaned.push(cond);
    }
    if cleaned.is_empty() {
        map.remove("conditions");
    } else {
        // 镜像同步：type/values = 首 conditions（TS validateConfig 强制重镜像 cleaned[0]）
        let first = cleaned[0].clone();
        if let (Some(Value::String(t)), Some(Value::Array(v))) =
            (first.get("type").cloned(), first.get("values").cloned())
        {
            map.insert("type".into(), Value::String(t));
            map.insert("values".into(), Value::Array(v));
        }
        map.insert("conditions".into(), Value::Array(cleaned));
    }
}

/// CIDR 列表 sanitize：非数组保留；逐项非法丢弃（含范围校验）；空 → 删字段。
fn sanitize_cidr_list(map: &mut Map<String, Value>, key: &str) {
    let Some(v) = map.get_mut(key) else {
        return;
    };
    let Value::Array(list) = v else {
        // 非数组 → 删除（TS：list 非数组时 sanitizeCidrs 原样返回，但 endpoint CIDR 统一删）
        map.remove(key);
        return;
    };
    let cleaned: Vec<Value> = list
        .drain(..)
        .filter_map(|c| match c.as_str() {
            Some(s) => {
                let t = s.trim();
                if crate::validate::is_valid_ip_cidr(t) {
                    Some(Value::String(t.to_string()))
                } else {
                    None
                }
            }
            None => None,
        })
        .collect();
    if cleaned.is_empty() {
        map.remove(key);
    } else {
        *v = Value::Array(cleaned);
    }
}

/// tunConfig.inboundExcludeCidrs sanitize（normalizeTunExcludeCidr：裸 IP 补 /32|/128、
/// 拒 catch-all/过宽/非法；空/全非法 → 删字段避免 [] 触发 norm 翻转）。
fn sanitize_tun_config(obj: &mut Map<String, Value>) {
    let Some(Value::Object(tun)) = obj.get_mut("tunConfig") else {
        return;
    };
    let present = tun.contains_key("inboundExcludeCidrs");
    if !present {
        return;
    }
    let Some(Value::Array(list)) = tun.get_mut("inboundExcludeCidrs") else {
        tun.remove("inboundExcludeCidrs"); // 非数组 → 删
        return;
    };
    let mut cleaned: Vec<String> = Vec::new();
    for c in list.drain(..) {
        if let Some(s) = c.as_str() {
            if let Some(n) = crate::validate::normalize_tun_exclude_cidr(s) {
                cleaned.push(n);
            }
        }
    }
    // 去重保序
    cleaned = crate::dedupe_str(&cleaned);
    if cleaned.is_empty() {
        tun.remove("inboundExcludeCidrs");
    } else {
        *list = cleaned.into_iter().map(Value::String).collect();
    }
}

/// dnsConfig.dnsTimeoutMs sanitize：须 1..=60000 有限正整数，否则删除；非整数取整。
fn sanitize_dns_config(obj: &mut Map<String, Value>) {
    let Some(Value::Object(dns)) = obj.get_mut("dnsConfig") else {
        return;
    };
    if let Some(v) = dns.get("dnsTimeoutMs") {
        let ok = v
            .as_f64()
            .map(|ms| ms.is_finite() && (1.0..=60000.0).contains(&ms))
            .unwrap_or(false);
        if ok {
            if let Some(n) = v.as_f64() {
                if !n.fract().eq(&0.0) {
                    dns.insert("dnsTimeoutMs".into(), Value::from(n.round() as i64));
                }
            }
        } else {
            dns.remove("dnsTimeoutMs");
        }
    }
}

/// 字段是否为非空字符串。
fn str_nonempty(obj: &Map<String, Value>, key: &str) -> bool {
    obj.get(key)
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;
