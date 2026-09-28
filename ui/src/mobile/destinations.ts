/**
 * 移动端五个主目的地的**唯一**登记表。
 *
 * 真值源是设计交接包 `~/docs/polaris/design/mobile-kit/ui-manifest.json` 的
 * `productScope.primaryDestinations`（id + label + 顺序）；`ui/src/mobile/mobile-entry.test.ts`
 * 把本表的 `labelKey` 在 `zh-CN` 下解析出来，与那份登记表的原文逐值对拍 ——
 * 三方（设计登记表 / 本表 / locale）任一处漂了都会红。
 *
 * # 为什么底部导航与路由必须共用这一份
 *
 * 两张表（一张给导航画按钮、一张给路由挑屏）是这类外壳最典型的失效形态：加一个目的地时改了一边，
 * 另一边静默不动 —— 表现是"按钮点了没反应"或"有屏但进不去"，两者都不报错。`BottomNavigation`
 * 与 `MobileApp` 都只读本表，门另断言两者不得自带任何标签字面量。
 *
 * # 为什么另开 `mobileNav` 命名空间而不复用 `sidebar.*`
 *
 * 五个中有三个（节点 / 连接 / 设置）的桌面文案恰好同字，但 `home` 与 `rules` **不同**：
 * 桌面是「主页」「路由」，移动端登记表是「首页」「规则」。后者不是笔误 —— IA spec §1.1 把八个桌面屏
 * 收进五个目的地，移动端的「规则」聚合了流量规则 / DNS / 规则资源，比桌面那一个「路由」屏宽。
 * 混用两个命名空间会得到一张一半来自桌面、一半自有的标签表，改一处时没人说得清该改哪边。
 *
 * # 不在本表里的东西
 *
 * Android「哪些应用走隧道」**不做、UI 上不出现、文案不得暗示以后会有**（IA 裁定 #7）。
 * 它既不是目的地，也不许在任何占位屏里留位。
 *
 * ⚠️ **原理由已被同仓拆掉，2026-09-06 重写**：原文写的是「配置里仍有零认证回环入站可被绕过
 * ⇒ 那是一个我们守不住的承诺」。而 `crates/config-engine/src/builder/inbounds.rs:104` 今天写死
 * `platform != "android" && platform != "ios"` 才发 mixed inbound，`:76-88` 逐字解释「洞的载体
 * 不存在」，`:704-708` 更把它与 `exclude_package` 并称「同一条防线的两半」。⇒ 那个洞没了，
 * 而裁定还挂着它当理由。**裁定保留，理由换成今天仍然成立的这条**：
 *
 * **承诺的是一张双向名单，存在的只有减法那一半，而且只在一个平台上、只对一部分应用成立。**
 * 一个叫「哪些应用走隧道」的顶层目的地，用户读到的是「逐个应用决定进／出」：
 *  · **「出隧道」这一半有实现**：`inbounds.rs:672-678` 的 android 臂发 `tun.exclude_package`
 *    （`:717 android_exclude_packages`），由 `VpnService.Builder.addDisallowedApplication` 兑现；
 *  · **「只让这些应用进隧道」那一半刻意不做**：同函数的映射表 `:692` 写死「`include_package`
 *    **恒不发射**」，`:694-700` 给了理由（它是换模式而非减法，配上「16 条预设默认全 proxy」
 *    会把其余全设备流量踢出 VPN 明文直出）。这不是没来得及做，是不该做；
 *  · **减法那一半也只覆盖一部分应用**：`app_rules_preset_data.rs:252`(epic) / `:275`(riot) 的
 *    `package_names: &[]` 不贡献包名，用户自建的应用则**恒不贡献**
 *    （`user_config/app_rules_preset.rs:76 package_names: Vec::new()`，理由写在其上）；
 *  · **且只在 Android 上**：`inbounds.rs:673` 是单 android 臂，`:97-98` 明写 iOS 连
 *    `exclude_package` 这条承诺都没有。
 * ⇒ 立一个目的地 = 画一张双向的按应用开关表，而背后只有单向、单平台、覆盖不全的一件事。
 *
 * ⚠️ **不要把它读成「应用分流三档只兑现一档」** —— 那是本文件 2026-09-06 上一版写错的话：
 * `builder/route.rs:880-904` 为**每一条**应用规则另发一条 `geosite`/`geoip` 的 `rule_set` 规则，
 * 带的是该档自己的 action（Block ⇒ `action:"reject"`），**没有任何平台门**。所以三档在 Android 上
 * 都有兑现手段，差别在**识别精度**（按域名 / IP，不按应用身份），不在「生效／不生效」。
 * 规则屏的应用段（`screens/rules/AppsSegment.tsx` 头注）按这份实况逐行说明，本裁定与它不冲突：
 * 那一段答的是「这个应用用哪个出站」，本裁定答的是「这个应用进不进隧道」，两个问题。
 *
 * ⚠️ **iOS 侧连减法那一半也没有**：`inbounds.rs:97-98` 逐字如此，故 iOS 上这个目的地更加无从谈起。
 */

/** 目的地 id。路由表、导航表、占位屏表共用它，多一个少一个都编译不过。 */
export type DestinationId = 'home' | 'nodes' | 'rules' | 'connections' | 'settings';

export type Destination = {
  readonly id: DestinationId;
  /** i18n key；**不是**文案。五种语言的取值在 `src/i18n/locales/*.json` 的 `mobileNav`。 */
  readonly labelKey: string;
};

/** 顺序即导航从左到右的顺序，与 manifest 的数组序一致。 */
export const DESTINATIONS: readonly Destination[] = [
  { id: 'home', labelKey: 'mobileNav.home' },
  { id: 'nodes', labelKey: 'mobileNav.nodes' },
  { id: 'rules', labelKey: 'mobileNav.rules' },
  { id: 'connections', labelKey: 'mobileNav.connections' },
  { id: 'settings', labelKey: 'mobileNav.settings' },
];

/** 冷启动落点。 */
export const DEFAULT_DESTINATION: DestinationId = 'home';

/** id → labelKey。`Record<DestinationId, …>` 派生不出来（会要一次 `as`），故按需查表。 */
export const labelKeyOf = (id: DestinationId): string =>
  DESTINATIONS.find((d) => d.id === id)?.labelKey ?? id;
