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
import type { AppRule } from '@/contracts/types';

/** 四个档位。`proxy` = 指定节点（必须带 `targetServerId`），`follow` = 跟随全局。 */
export type AppPolicyWritablePick = 'follow' | 'direct' | 'block' | 'proxy';

/**
 * 档位白名单。面板给出的四档都在其中；它今天守的是「档位串合法」这一格（见文件头注末节）。
 */
export const APP_POLICY_WRITABLE: ReadonlySet<string> = new Set<AppPolicyWritablePick>([
  'follow',
  'direct',
  'block',
  'proxy',
]);

export function isAppPolicyWritable(pick: string): pick is AppPolicyWritablePick {
  return APP_POLICY_WRITABLE.has(pick);
}

/**
 * 一次选档落成的 `AppRule`。与桌面 `AppPolicyScreen.tsx:259 setAppPolicy` 逐字段同义，
 * 两处**刻意**的差别各自有理由：
 *
 *  ① `follow` 落成 `{ action: 'proxy', 不带 targetServerId }` —— 桌面快选「默认代理」的落盘形态，
 *     两端 `appPolicyView` 都把它显示成「跟随全局」。它不是「没有规则」：显式写一条
 *     跟随全局的规则，才能把一条已存在的 `direct` 从 `exclude_package` 里拿掉。
 *
 *  ② **`enabled` 恒 `true`**，不沿用 `existing?.enabled ?? true`。移动端**没有**逐条启停应用规则的
 *     控件（桌面在卡片上有），沿用一条 `enabled:false` 的旧规则会让用户看见档位变了、而
 *     `inbounds.rs:717` 的 `filter(|r| r.enabled && …)` 把它整条滤掉 —— 又一颗「拨了不生效」的控件。
 *     这是**扩大**了一次用户显式动作的效果，故写在这里，不是顺手。
 *
 * ⚠️ **`targetServerId` 只在 `proxy` 档且真的给了节点时才写**（连 `undefined` 都不写）：
 * `AppRule` 的这一格是可选的，Rust 侧 `rule.rs:293` 带 `skip_serializing_if = "Option::is_none"`。
 * 写成 `targetServerId: undefined` 会让 `JSON.stringify` 的结果与夹具在**深比较**上分叉。
 * 反过来，`proxy` 档**没给**节点 id 时落成的就是 `follow` 那条形态 —— 那正是它的语义
 * （「代理但没指定去哪」= 跟随全局），不是一个需要报错的非法输入。
 */
export function appRuleForPick(
  appId: string,
  pick: AppPolicyWritablePick,
  targetServerId?: string,
): AppRule {
  if (pick === 'direct') return { appId, action: 'direct', enabled: true };
  if (pick === 'block') return { appId, action: 'block', enabled: true };
  if (pick === 'proxy' && targetServerId !== undefined && targetServerId !== '') {
    return { appId, action: 'proxy', enabled: true, targetServerId };
  }
  return { appId, action: 'proxy', enabled: true };
}
