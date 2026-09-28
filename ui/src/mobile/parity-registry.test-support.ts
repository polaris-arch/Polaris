/**
 * 四屏（首页 / 规则 / 连接 / 设置）**与桌面逐块对差**的登记表所共用的类型与口径。
 *
 * 形态照 `nodes/absence-register.ts`（那张表是本仓的先例，**不重构它**）：一条登记 = 一个
 * 桌面侧的可点面 / 数据块 + 一个移动端处置码 + 依据。差别只有两处，两处都是被门逼出来的：
 *
 * # 差别一：依据是**锚**，不是一句话
 *
 * 节点屏那张表把依据写在注释里，靠人读。`wiring-completeness.test.ts` 头注记着本仓在这上面栽过的
 * 那一次（2026-09-06 复审 major）：白名单只校验理由「长得像 `file:line`」、从不打开那个文件，
 * 于是依据的事实整条消失而门一动不动。故这里的 `Anchor` 是 `{ file, mustContain }`：
 * 门会**打开那个文件**核对那段文本还在不在，不在就当场红。
 *
 * **不写行号**是有意的：行号会被上方任何一次无关改动推走，那是脆不是严。要「file:line」的时候
 * 由门**现场算**（`mustContain` 命中在第几行），报告里印出来的是当下真值，而不是一个会腐烂的常量。
 *
 * # 差别二：五个处置码，不是三个
 *
 * 节点屏那三个码（`ported` / `disabled` / `absent`）回答的是「这条腿在移动端有没有」，
 * 而它那一屏里凡不是 `ported` 的都是**真的欠着的活**。本表的射程横跨设置屏，那里有一整批
 * 「桌面有、Android 上根本没有这个对象」的条目（托盘 / 窗口特效 / 提权助手 / 记住窗口尺寸）。
 * 把它们和「表单层还没移植」写成同一个码，会让债务表把**永远不会做的事**记成待办。
 *
 * 所以这里显式分成两族，**分法逐字照抄 `wiring-completeness.test.ts` 的 `DISPOSITIONS`**
 * （那份文件早就定义了这两种、且只有这两种豁免），只是把它挪到登记的那一刻：
 *
 *  · **豁免族**（不进债务，但每条都必须交一个被机器打开的锚）
 *      `ported`              —— 移动端**这一屏**有等价入口。
 *      `reachable-elsewhere` —— 不在这一屏，但移动端别处就有（锚指那处真的渲染/路由）。
 *      `platform-absent`     —— 平台上没有这个对象（锚指那条证明：两端同缺的源码、或平台事实）。
 *      `adjudicated-out`     —— 产品裁定要求移动端**不提供**这个能力（管理 API 那一族）。
 *                               🔴 它的锚必须指向**一道正面钉着「这东西不许出现」的门**，不是一句说明：
 *                               用不出这样一道门的，就不许用这个码，老实留在债务里红着。
 *                               这一条比另外三档更严 —— 另外三档的锚只要求「事实还在」，
 *                               这一档要求「有人在持续证明它不在」。
 *  · **债务族**（逐条进 `wiring-completeness.test.ts` 的 B 面）
 *      `disabled` —— 这条腿在，但被移植之外的原因恒不可用，且带一句已本地化的理由。
 *      `absent`   —— 移动端没有这条腿。`reasonKey` **可选**：有它表示界面上至少说了一句
 *                    「这里没有」；没有它表示**界面上一个字都没提** —— 那是更坏的一档，
 *                    它今天正是靠这张表才第一次可见（W-23 / W-24 / W-27 都死在这一档）。
 *
 * 🔴 豁免不是免检，三条机器判据（2026-09-06 复审逐条逼出来的，别读成建议）：
 *
 *  1. **锚在剥掉注释之后核对**。上一版是对原文裸 `indexOf`，于是一句墓碑注释
 *     （「此处曾有 X，整块已移走」）就能喂饱它 —— 实测把移动端反向分流指示器整块删掉、
 *     只留一行这样的注释，W-27 那条专门的回放断言照绿。现在锚**只认代码行**。
 *  2. **`ported` / `reachable-elsewhere` 的锚必须落在 `ui/src/mobile/` 下**。
 *     字段名叫 `mobile`，上一版却不校验它指哪儿 —— 实测把首页连接键那条的锚改成桌面自己的
 *     `id="connect-btn"`，门全绿。移植时顺手从桌面文件里复制一段字面量当锚是最省事的写法。
 *     （喂数腿表不受这条约束：那张表按**读点**取材，写入方合法地可以住在共享模块里，
 *     `useDialogStore.stack` 那条的理由写在 `feed-register.ts` 里。）
 *  3. **`platform-absent` 的锚必须指向一条平台判定**。写「Android 没有托盘」而锚指一段
 *     与托盘无关的文本、或指一句注释，门当场红：命中行的邻域（剥注释后 ±8 行）或文件路径里
 *     必须出现 `#[cfg(` / `cfg!(` / `target_os` / `android` / `ios` / `mobile` / `desktop`
 *     之一。上一版三条锚全指着 `lib.rs` 里的中文注释 —— 给 Android 落一个 autostart 实现、
 *     注释原样留着，豁免继续成立。
 */

/**
 * 被机器打开核对的锚：`file` **相对仓根**（逐字同 `wiring-completeness.test.ts` 的 `Anchor`，
 * 两处的锚可以互相搬），必须存在，且必须真的含 `mustContain`。
 */
export interface Anchor {
  readonly file: string;
  readonly mustContain: string;
}

/** 处置码。豁免四档各自带锚，债务两档带（可选的）用户可见理由。 */
export type ParityDisposition =
  | { readonly kind: 'ported'; readonly mobile: Anchor }
  | { readonly kind: 'reachable-elsewhere'; readonly mobile: Anchor }
  | { readonly kind: 'platform-absent'; readonly evidence: Anchor }
  | { readonly kind: 'adjudicated-out'; readonly enforcedBy: Anchor }
  | { readonly kind: 'disabled'; readonly reasonKey: string; readonly mobile: Anchor }
  | { readonly kind: 'absent'; readonly reasonKey?: string };

/** 动作面的取材形态。两种可以叠，取并集。 */
export type ActionFaceKind = 'controls' | 'config-fields';

/** 进债务表的两个码。豁免族一律不进（它们各自交了锚）。 */
export const DEBT_KINDS = ['disabled', 'absent'] as const;

export function isDebt(disposition: ParityDisposition): boolean {
  return (DEBT_KINDS as readonly string[]).includes(disposition.kind);
}

/** 豁免族随身带的那个锚（`ported` / `reachable-elsewhere` 指移动端，`platform-absent` 指依据）。 */
export function exemptionAnchor(disposition: ParityDisposition): Anchor | null {
  switch (disposition.kind) {
    case 'ported':
    case 'reachable-elsewhere':
      return disposition.mobile;
    case 'platform-absent':
      return disposition.evidence;
    case 'adjudicated-out':
      return disposition.enforcedBy;
    case 'disabled':
      return disposition.mobile;
    default:
      return null;
  }
}

/**
 * 一条登记。
 *
 * `id` 由门从**桌面源码**里算出来，不是人起的名字 —— 人起名字就会出现「桌面那颗改名了，
 * 登记表照旧对得上」的僵尸。算法见门里的 `actionId()` / `blockId()`，口径是
 * 「`id=` 属性 → i18n 键集合 → className 字面量 → onClick 指纹」，先命中先用。
 *
 * `note` 是给读代码的人的，门只校验它**不是排期**（「本批不做」不是判定，逐字同 `app-wiring.test.tsx` ⑫）。
 */
export interface ParityEntry {
  readonly id: string;
  readonly disposition: ParityDisposition;
  readonly note: string;
}

/** 一屏的登记表。三个面各自与桌面对差，`desktopDirs` 是取材面的根。 */
export interface ScreenParityRegister {
  /** 屏名，只进报告。 */
  readonly screen: string;
  /**
   * 桌面取材面的目录（相对仓根），**可以有多个**。
   *
   * 🔴 **一个移动屏未必只对一个桌面目录**：IA §1.1 把八个桌面屏收进五个移动目的地，
   * 于是移动规则屏同时宿主着桌面的 `rules` / `app-policy` / `resources` 三屏，
   * 移动连接屏宿主着 `connections` / `logs` 两屏。2026-09-06 复审 blocker 记的正是这一条：
   * 上一版这里是单数 `desktopDir`，那三个桌面目录（合计 28 颗动作）**不在任何一面上**，
   * `logs.archiveLegacy` / `logs.deleteLegacy` 两颗因此既没接也没登记也没人看得见。
   * 门另有一条 ⓪ 组断言：`ui/src/components/screens/` 下每个目录都必须被某张表认领，
   * 或在门里那张显式的「另有门兜」表上 —— 桌面新开一个屏不许静默掉在面外。
   */
  readonly desktopDirs: readonly string[];
  /** 移动端这一屏的生产源码目录（相对仓根）。 */
  readonly mobileDir: string;
  /**
   * 动作面的取材形态，**可以叠**（两个面取并集，各自的判定方向不同）。
   *
   *  · `controls`       —— 桌面这一屏渲染的每一个**动作元素**：`<button>`，以及任何带 `onClick=`
   *    或动作性 `role=`（button / switch / tab / menuitem / link）的开标签。
   *    🔴 上一版只认字面 `<button`，2026-09-06 复审 major 记着它漏掉了什么：桌面出口面板里的
   *    「按延迟排序」是一枚 `<span role="switch" onClick={…}>`，移动端整块移植了却没有这一维，
   *    而集合恒等断言让人连手工补登都做不到（补一条就报「只在表里」）。
   *  · `config-fields` —— 桌面这一屏 `update({ … })` 写出去的每一个配置字段（设置屏叠在 `controls` 上）。
   *    设置屏的可改面大半由 `<Switch>` / `<Select>` / `<TextInput>` 承担，只数动作元素答不出
   *    「它能改哪些配置」；只数配置字段又答不出「那些不写配置的按钮（复制、清理、重试）在不在」。
   *    两个面**都要**，这也是 2026-09-06 复审那条 major 的结论。
   */
  readonly actionFaces: readonly ActionFaceKind[];
  /** 动作面：逐条登记。`config-fields` 面**只登记缺口**，见门的头注「两种动作面的判定方向」。 */
  readonly actions: readonly ParityEntry[];
  /** 数据块面：这一屏渲染的每一个**内容块**，逐个登记（口径见 `blockSources` 三个字段）。 */
  readonly blocks: readonly ParityEntry[];
  /**
   * `desktopDirs` **内部**的原语库文件（相对仓根）：它们声明的组件不算这一屏的内容块。
   *
   * 门对这条豁免有一条正面断言：文件必须真的在 `desktopDirs` 里，且**自己一句 `t('…')` 都不渲染**
   * —— 一个渲染自有文案的模块不是原语，是内容。
   */
  readonly primitiveModules: readonly string[];
  /**
   * 这一屏渲染、但组件定义在 `desktopDirs` **之外**的**内容块**来源（相对仓根）。
   *
   * 例：规则屏逐条规则的详情面住在 `components/hover-cards/RuleHoverCard.tsx` ——
   * 那是这一屏的内容，不是通用原语（`rules.noConditions` 那句「（无条件）」就是从这条缝漏掉的）。
   */
  readonly extraBlockSources: readonly string[];
  /**
   * 这一屏一跳 import 进来、判为**通用原语 / 跨屏共用件**的文件（相对仓根）：不算这一屏的内容块。
   *
   * 🔴 它们的**动作**照样在动作面上（动作面吃「自有目录 ∪ 一跳渲染 import」的全部文件）——
   * 本字段只回答「算不算这一屏的数据块」。门有一条 ⓪ 组断言把这张分类表钉成**恰等**：
   * 一跳 import 里被当作 JSX 标签渲染的每一个仓内文件，必须落在
   * `extraBlockSources ∪ sharedPrimitives` 里，多一个少一个都红 —— 新 import 必须被显式判一次。
   */
  readonly sharedPrimitives: readonly string[];
  /**
   * 本门显式缩掉的那几格，**接手门**的锚（门逐条打开核对）。
   *
   * 门的头注「射程自曝」第 1 条把逐槽粒度点名交给各屏自己的渲染门。交接给一个不存在的门 = 没交接，
   * 所以那句话必须落成机器判据：这里填接手门的文件 + 那一组 `describe` 的字面标题。
   * （2026-09-06 复审 minor：上一版四道接手门里只有 `MobileSettings.test.tsx` 一道被断言存在。）
   */
  readonly slotHandoffGates: readonly Anchor[];
  /**
   * 具名数据槽的**基线**（`desktopDirs` 全部文件的 `id="…"` 全集）。
   *
   * 🔴 这一格**不是对差面，是变更探测器**，别把它读成「每个数据槽都对过了」：它只回答
   * 「桌面这一屏的数据槽集合有没有在没人看的情况下变过」。桌面新加一个槽 ⇒ 基线不等 ⇒ 红 ⇒
   * 逼一次「移动端要不要画」的显式决定。它**答不出**移动端今天画没画其中某一个 ——
   * 那一格由 `blocks` 面（整块粒度）与各屏自己的渲染门（逐槽粒度）分担，见门的头注「射程自曝」。
   */
  readonly slotBaseline: readonly string[];
}
