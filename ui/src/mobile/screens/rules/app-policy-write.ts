/**
 * 移动端应用分流策略的**写入形态** —— 「选了哪一档」到「配置里落成什么」的唯一一处映射。
 *
 * # 为什么单独一个模块，而不是写在容器里
 *
 * 这段映射是一条**跨语言契约**的 UI 那一端：`crates/config-engine/src/builder/inbounds.rs:717
 * android_exclude_packages` 只收 `enabled && action == Direct` 的规则，再由
 * `VpnService.Builder.addDisallowedApplication` 在系统边界兑现。写成容器里的一个对象字面量，
 * 判据就只能靠正则去认那段字面量，而正则认得出的东西与「生产真的写了什么」不是一回事。
 * 抽出来之后两侧各有一条判据，且**必须成对交**：
 *  · 纯函数这一半：`app-policy-write.test.ts` 拿它的输出与
 *    `crates/config-engine/fixtures/mobile-app-policy-writes.json` 逐字段对拍；
 *  · 接线那一半：同一份判据断言 `RulesScreen.tsx` 的写路径**真的调用它**，
 *    而不是自己另拼一个对象（「测了纯函数 ≠ 生产在用它」）。
 * 而 Rust 侧 `tests/golden_inbounds_android.rs` 读**同一份**夹具当输入，断言配置生成侧
 * 真的把包名放进了 `exclude_package`。三条判据串起来才叫「这颗控件真的改得动隧道」。
 *
 * # 🔴 2026-09-13（批 10）：四档全部可写，`proxy` / `block` 的置灰理由已被推翻
 *
 * 上一版只放行 `follow` / `direct` 两档，理由是「另两档要的是按应用身份匹配，而
 * `singbox/route.rs` 的 `RouteRule` 至今没有 `package_name` 字段」。
 *
 * 那句话**本身仍然成立**，但它证明不了「不许在这一屏改」：
 *  · 这两档在这台设备上**真的兑现**，只是按流量特征认 —— `builder/route.rs:880-904` 那条
 *    `rule_set` 腿逐条发射（16 条内置预设的 `geosite_tags` 无一为空），Block ⇒ `action:"reject"`，
 *    Proxy 且指定节点 ⇒ `outbound:"rule-sel-app-<id>"`。`AppsSegment.tsx` 头注对这条腿有逐行取证。
 *  · 上一版自己也承认「置灰的是**改**，不是它们本身：从桌面同步过来的档位照常生效」。
 *    那就不是「平台兑现不了」，而是「移动端没接这一跳」—— 未接线，不是取舍。
 * ⇒ 处置改为：四档全部可写，**识别精度**这条平台事实由顶部那条常驻说明承担
 *   （`mobileRules.appRoutingIdentityNote`），不再靠置灰去表达。
 *
 * # 本表的白名单今天守什么
 *
 * `APP_POLICY_WRITABLE` 现在是四档全集 ⇒ 它不再挡任何一档，但**没有退化成恒真**：
 * 它挡的是**档位串本身不合法**的调用（面板之外的调用方传进一个拼错的值 / 一个已废弃的旧档名）。
 * 少了它，那种调用会静默落成一条 `action` 为 `undefined` 的规则 —— 引擎读不出动作，
 * 这条应用规则整条失效而界面上看不出来。
 */
export { APP_POLICY_WRITABLE, isAppPolicyWritable, appRuleForPick } from '@/components/screens/app-policy/app-policy-logic';
export type { AppPolicyWritablePick } from '@/components/screens/app-policy/app-policy-logic';
