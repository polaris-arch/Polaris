/**
 * 文本装得下吗 —— i18n 文案 × 受限容器的几何门（2026-07-31 真机：「俄语容易超出导航栏边框」）。
 *
 * # 缺陷长什么样（阳性对照，本门必须抓到它）
 *
 * `.side{width:148px}` + `.nav-item{padding:8px 9px; gap:10px; font-size:13px; white-space:nowrap}`
 * + `.nav-item svg{width:17px; flex:none}`，且**全链路没有任何 overflow/text-overflow**
 * ⇒ 标签既不换行也不截断，长文案直接画到侧栏外面。ru 的 `sidebar.appPolicy` =
 * 「Политика приложений」在 13px 下约 124–172px（随字体族），可用宽只有 83px。
 *
 * # 为什么是「算」不是「渲染量」
 *
 * 本仓 vitest 是 `environment:'node'`（vite.config.ts），无 jsdom / 无 CSSOM / 无 Canvas
 * ⇒ 真实排版在这一层根本不可观测。故用**字符宽度模型**估算，并对模型本身做阳性/阴性自校（见 ①②）。
 *
 * # 宽度模型的系数从哪来（不是拍脑袋）
 *
 * 字体栈是 `--sans: -apple-system, 'Segoe UI Variable', 'Segoe UI', 'PingFang SC',
 * 'Hiragino Sans GB', 'Noto Sans CJK SC', 'Microsoft YaHei', system-ui, sans-serif`
 * （胜出的那条在 index.css 的全部 @import 之后；tokens.css / prototype.css 的同名声明被它压过。
 * `Noto Sans CJK SC` 是 2026-09-04 按契约 B 补的 Android 中文族，**不影响本模型的系数** ——
 * 下面那批标定字体里本来就含 Noto Sans CJK，系数取的是全体上界），
 * 真机落到 SF Pro（mac）/ Segoe UI（win）/ 系统 sans（linux）。系数取自**离线实测的字体 hmtx 表**：
 * DejaVu Sans Regular+Bold、Liberation Sans Regular+Bold（Arial 度量兼容）、Lato、
 * Noto Sans CJK、Noto Sans Arabic Regular+Bold —— 每个字符桶的系数 = 桶内成员在这批字体上的
 * **advance 最大值**（含 Bold），故对单字符是严格上界，对整串亦然。
 * DejaVu Sans 本身就是这套栈的宽端（西里尔比 Segoe UI 宽约 30%），⇒ 模型相对真机再多一层余量。
 *
 * **自校实测（10281 条 = 5 locale 全量 × 逐条比对上述字体的真实 advance 之和）**：
 * 模型 < 实测的条数 **0**（0.00%），中位过估 +10.5%，p95 +61.5%。方向恒为「偏宽」。
 * 波斯语的比对基准用 Presentation Forms-B 做了**连写还原**（见下）。
 *
 * # 波斯语（RTL + 连写）怎么处理，误差朝哪边
 *
 * - **RTL 不影响宽度**：行内推进方向变了，advance 之和不变 ⇒ 模型按 LTR 累加即可。
 * - **连写（initial/medial/final/isolated 四形）**：模型对整个 Arabic 块用**单一系数 0.84**
 *   （= 上述字体 Arabic 区 advance 均值的上界）。离线用 Presentation Forms-B 码位还原了真实
 *   连写形宽度做对照（例：「تست」连写 2.24em，模型 2.52em；「سیاست برنامه‌ها」连写 6.69–7.53em，
 *   模型 11.28em）—— 连写形普遍**窄于**独立形，故模型恒偏宽。
 * - **这是近似**：不建模 kerning、不建模 لا 之类的强制连字、不建模 Persian 专用字体
 *   （Vazir/IRANSans）的度量差异。误差方向 = 过估，不会漏报。
 * - 组合符号（فتحه/کسره 等 U+064B–U+065F、U+0670）与 ZWNJ/ZWJ/方向标记按 0 宽计（正确）。
 *
 * # 覆盖了什么
 *
 * 三个界面里**有硬宽度上限**的文本容器（可用宽 = 容器宽 − padding − 同行 flex:none 兄弟 − gap，
 * 全部从 CSS 现场解析，不写死数字）：
 *   主窗   S1 主侧栏 nav 标签 / S2 设置侧栏 nav 标签 / S3 两侧栏的 nav-group 组头
 *          S4 连接表定宽列表头（c-dest / c-rule / c-chain / c-rate）
 *          S9 首页右列 seg2 三档（接管方式 / 分流策略）——五语种在默认窗口恒横排
 *          S11 节点弹窗（统一录入宽度 540px）的表单字段：`.fld-l` / `.swt-row` 标签与统一
 *              `#tip` 信息提示 —— 标签可换行且不截断，提示受 tooltip 宽度与行数预算约束
 *          S12 导入弹窗的解析结果预览（同一个 `.dlg` 定宽）：`.imp-stat` 三颗计数 pill /
 *              `.card-sub` 清单小标题 / `.imp-badge`「不支持」徽标 —— 徽标那条判的不是溢出
 *              （它 `flex:none` 不收缩、名称轨带 ellipsis 会替它让位），而是「别把名称轨压到
 *              看不清」，见该段
 *   托盘   S6 浮层菜单行（带右侧勾/箭头/延迟的更窄那档）/ S7 浮层状态副标题
 *          S7b 浮层状态标题 / S9 浮层组头（10px uppercase）/ S10 浮层一次性提示条
 *   更新窗 S8 380px 弹窗的按钮行（**可换行**，行数有预算，见该段）
 * 语种：**五种全测** en-US / ru / fa / zh-CN / zh-TW —— 托盘与更新弹窗 2026-07-31 接入 i18n 后
 * 与主窗同口径，不再有「只测 zh/en」的例外。
 *
 * # S9 是一类形态：「基类无约束、变体有约束」
 *
 * S1–S8 的判据是「容器有硬宽度上限」。S9 起加第二条判据：**基类本身没有宽度约束（`inline-flex` /
 * `max-content`），但某个变体选择器给它加了「均分或撑满」**——`.seg2` 是 `inline-flex`（= max-content，
 * 天然装得下），可 `.seg-wrap .seg2{width:100%}` + `.seg-wrap .seg2 button{flex:1}` 把它钉成容器宽。
 * 首页右列另有紧凑内边距变体，必须按变体的最终几何验算，不能只看基类。
 *
 * 全仓按此判据扫过一遍（`width:100%` 落在 inline-* / max-content 基类上、`minmax(0,…)` 网格轨道、
 * `flex:1`+`min-width:0` 的 flex 项），结论逐条记在下方「射程之外」的 h.。
 *
 * # S11 判据为什么是两条（一条硬、一条棘轮）
 *
 * `.fld-l` 是**可折行的块**（无 `white-space:nowrap`）、**无** `text-overflow:ellipsis`、
 * **无** `overflow-wrap:anywhere`。三条合起来决定了它跟 S1（nowrap 画到框外）和 S8（定死窗高裁按钮）
 * 都不同形，判据必须分成两条：
 *
 *  1. **硬判据 —— 不可断长串宽度**（`maxLineWidth > avail`）。没有 `overflow-wrap` ⇒ 一个比字段还宽的
 *     不可断单元（长西里尔词 / `Sec-WebSocket-Protocol` 式 token / URL）真的横向溢出。溢出的下场不是
 *     「画到窗外」（`.dlg{overflow:hidden}` 拦住了），而是 **`.dlg-body` 长出横向滚动条** —— 它写了
 *     `overflow-y:auto`，而 CSS Overflow §3 规定「一轴非 visible 时另一轴的 visible 计算成 auto」
 *     ⇒ 整张表单要左右拖才能读完一个标签。这是真破版，恒判红。
 *  2. **棘轮判据 —— 行数预算**（`lines > maxLines`）。`.dlg` 是 `max-height:calc(100vh - 40px)`、
 *     `.dlg-body` 是 `overflow-y:auto` ⇒ **纵向没有硬上限，超高只是变高、要滚动，不会被裁掉**。
 *     故这条**不是**「顶出可视区」那种量级（那是 S8 更新弹窗的形态：`popup_height_for` 定死窗高 +
 *     `body{overflow:hidden}`，第三行按钮真的点不到）。它的作用是棘轮：预算钉在今天最长的那一格，
 *     再长一句就转红。别把它读成「超了会坏」。
 *
 * 顺带：本节是第一个「可折行 + 无 overflow-wrap + 有大量中文语料」的容器，于是第一次踩到排版模型
 * 里那个「只按空白切词」的近似 —— 详见 `layout()` 的注释（该近似已就地修掉，方向是去掉**假红**）。
 *
 * # 没覆盖什么（射程自曝，别把绿当成「全都装得下」）
 *
 *  a. **运行期才知道长度的用户数据**：节点名 / 订阅名 / 域名 / 进程名 / 规则值 / 版本号 /
 *     速率数字。这些容器（`.nd-name`/`.conn-host`/`.tb-name`/`.tray-node-name`/`.cat-nm` …）
 *     本来就带 `text-overflow:ellipsis`，属于「截断」而非「画到框外」，且没有静态真值可测。
 *  b. **运行期拼接的文案**：带 `{{n}}`/`{{count}}` 插值的键按模板长度测，实际值更长；
 *     `defaultValue` 内联兜底文案不测。
 *  c. ~~托盘浮层只有中/英两态~~ —— **已消除**（2026-07-31）。`tray/labels.ts` 改走
 *     `i18n/auxiliary.ts` 的键查找，文案住进 `locales/*.json` 的 `tray.*`，五语种齐备；本门随之五语全测。
 *  d. ~~更新弹窗完全没有 i18n~~ —— **已消除**（2026-07-31）。文案住进 `updatePopup.*`，五语种齐备。
 *     两条旧例外正是 `i18n/i18n-coverage.test.ts` 那道门要防复发的东西：**「某个界面整个漏在
 *     i18n 体系外」此前没有任何门管**，于是它能一路绿到真机。
 *  e. **未进注册表的受限容器**：`.app-pol`(max-width:112px)、`.ctx-note`(120px)、
 *     `.res-row` 定宽列、`.lock-field`(280px)、`.node-menu`(360px)、`.proto-grid` 等
 *     —— 它们要么已带 ellipsis（信息损失但不破版），要么渲染的是用户数据。
 *     ⚠️ 这条**从来没有涵盖过弹窗表单的 `.fld` 一族**（`.fld-l` / `.swt-row` / 信息提示）：
 *     它们不带 ellipsis、渲染的也不是用户数据，只是**一直没被列进来**——不是判过不进门，是没看见。
 *     2026-08-06 起节点弹窗那份进门（S11），其余弹窗仍在门外，逐条见下面的 i.。
 *  f. **纵向**：只算宽度与行数，不算像素高度。换行后行数增加导致的**纵向**溢出，只有在容器有硬高度
 *     上限时才是「被裁掉」（S8 更新弹窗是这一类）；S10 / S11 的行数预算是**棘轮**不是墙，见各自段注释。
 *  i. **S11 只覆盖 `node.field.*`（ND_SPEC 的 99 个键），节点弹窗里下面这些仍在门外**：
 *     · `NodeDialog.tsx` **内联**的那批（`node.protocol` / `node.label` / `node.serverPort` /
 *       `node.chainVia` / `node.chainHint` / `node.formGroup.*` /
 *       `node.customProbe.*`）—— 它们不在 ND_SPEC 里，是 JSX 里手写的，要进门得再加一层
 *       regex + 手维护槽位表（同 `TRAY_SLOT` 的形态）。它们与已测字段**同槽同宽**，风险同类。
 *     · **其余弹窗的同款 `.fld` 一族**：`SubDialog` / `WarpDialog` / `WgDialog` / `TsSettingsDialog` /
 *       `RuleDialog` / `ImportDialog` / `AppAddDialog` …。部分录入表单同为 540px，其余仍走 `.dlg`
 *       基准宽度；缺的仍是各自的「键 → 槽」映射。别把 S11 的绿读成「所有弹窗都装得下」。
 *     · **`select` 的选项文案与 placeholder**：`TCP` / `xtls-rprx-vision` / `obfs=http;obfs-host=…`
 *       这类专有名词是字面量、不入 i18n（`FieldSpec.tsx` 文件头有说明），且下拉宽度锁触发器宽、
 *       超长走 ellipsis（components.css 的 CSEL 段），属 a. 那一类。
 *     · ~~**`common.optional`「可选」徽标在 fa/ru 缺失**~~ —— **已补齐**（2026-08-07）。本门按
 *       **真实回落链**量它（locale 里有就用 locale 的、没有就用代码里的 zh 默认），此前 `fa`/`ru`
 *       两个语种根本没有这个键 ⇒ 那两种语言的用户在 52 个 `opt:true` 字段上看到的都是中文「可选」。
 *       补上之后徽标从 2 全角（≈20px）变成 ru「Необязательно」≈110px / fa「اختیاری」，标签行宽了一大截：
 *       实测 `echConfig`/`hostKeyAlgorithms`/`idleCheck`/`kexAlgorithm`/`privateKeyPassphrase` 等
 *       多条 ru/fa 标签由 1 行涨到 2 行（仍在预算内），**没有一条越预算** —— 本门是这件事的验收面。
 *  g. **字体真值**：真机字体不在这批离线字体里（SF Pro / Segoe UI 均未安装），模型是它们的
 *     保守上界，不是它们本身。
 *  h. **「基类无约束、变体有约束」全仓扫描里判为不进门的**（2026-07-31，逐条判过，不是没看）：
 *     · `.brand-svg`（`width:100%` 落在 inline-flex 基类上，components.css:253 / index.css:460）
 *       —— 承载的是 SVG，没有文本。
 *     · `.top-grid .top-card-h .seg2`（index.css:1399，同族的第二个 seg2）—— 三档文案是**数字**
 *       （topN = 5/10/20，ConnectionsScreen.tsx:922），与语种无关；且它是 `flex:none`（保 max-content，
 *       压不动），同行的标题才是让宽的那一侧且自带 ellipsis。
 *     · `.term-row code`（`flex:1;min-width:0` + nowrap + **无** ellipsis，components.css:1129）
 *       —— 但它带 `overflow-x:auto`，是**可横向滚动**而非画到框外；且内容是运行期拼的命令行。
 *     · `#s-nodes.nodes-list-view .nd-pills`（`flex:1;min-width:0`，screens.css:295，内含 nowrap 的
 *       `.pill`）—— `.nd-pills{flex-wrap:wrap}`（screens.css:85），pill 会换行不会溢出。
 *     · `.cc-cols` 左列 / `.res-row` / `.cat-item` 的 `minmax(0,…)` 轨道 —— 里面的文本节点
 *       （`.exit-name`/`.exit-addr`/`.cat-nm`/资源名）全是运行期用户数据，且已带 ellipsis。
 *     · `.aad-nc .sel{width:100%}`、`.aad-res-bar/.cond-head 的 .search-box{flex:1}` —— 原生
 *       `<select>` / `<input>`，超长由控件自身裁切，不会画到框外。
 *     · `.row2` / `.field-grid` 的 `1fr 1fr`、`.mesh-grid`/`.node-grid`/`.app-wall`/`.cidr-list`/
 *       `.aad-ico-grid`/`.proto-grid` 的 `repeat(auto-fill,minmax(N,1fr))` —— 轨道有 `minmax` 下限
 *       兜底，且里面的文本要么可换行（无 nowrap）要么是带 ellipsis 的用户数据。
 *     · `.unlock-field .unlock-row{flex:1}`、`.top-bar-row/.ut-prog/.res-prog 的 .bar{flex:1}`、
 *       `#s-home > .topo{flex:1 1 auto}` —— 纵向 flex 容器 / 进度条 / SVG，无 nowrap 文本。
 *     · `.field-lbl`（S9 的同排兄弟，静态 i18n）—— **没有 nowrap**，超宽只会折行不会画到框外；
 *       且同行挂着运行期条件元素 `ReverseRoutingBadge`（reverseMesh 开时才出现），要进门就得给
 *       运行期状态建模，属 a./b. 之外。实测余量：最窄容器下可用 191.7px，最长项 ru
 *       「Маршрутизация」11.5px 下 135.6px，余 56px。
 *
 * # 读不到就报错，不跳过
 *
 * 所有 CSS 值与组件里的键都是**解析出来的**：解析不到 → `throw`（不是 `it.skip`）。
 * 改窄某个容器、加一条导航项、加一个语种、加一条长翻译，都会让本门转红。
 */
import { describe, it, expect } from 'vitest';
import {
  UNCONDITIONAL,
  context,
  contextOf,
  explain,
  resolve,
  type CssContext,
  type CtxName,
} from './css-cascade.test-support';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
// S11 的字段清单**直接取真值源**，不 regex 扒 `node-spec.ts` 的文本：ND_SPEC 是导出的纯数据表
// （对 `FieldSpec` 只有 `import type`，不牵进 React ⇒ node 环境可直接 import），regex 反而会在
// `...F_TRANSPORT` 这类展开处漏字段而**静默少测**。同 `homeSegGroups()` 用 regex 的理由相反：
// 那批常量住在 .tsx 组件文件里、拿不到；这张表拿得到。
import { moduleSource } from '@/contracts/rust-source.test-support';
import { ND_SPEC, type NodeProto } from '../components/dialogs/node-spec';

const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const stripComments = (src: string) =>
  src.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '));

// ════════════════════════════════════════════════════════════════════════════════════════
// ① 字符宽度模型
// ════════════════════════════════════════════════════════════════════════════════════════

/**
 * 字宽 ÷ font-size。每档 = 档内成员在参考字体集上的 advance **最大值**（见文件头「系数从哪来」）。
 * 参考字体集：DejaVu Sans R/B、Liberation Sans R/B、Lato、Noto Sans CJK、Noto Sans Arabic R/B。
 */
export const COEF = {
  space: 0.36, // U+0020/A0/2007/2009/202F —— max 0.348 (DejaVu Bold)
  latThin: 0.4, // i j l I ' . , : ;      —— max 0.400 (DejaVu Bold ':')
  latNarrow: 0.57, // f r t J ! | ( ) [ ] - / \ * "  —— max 0.556 (Liberation Bold 'J')
  latLower: 0.73, // 其余小写拉丁             —— max 0.716 (DejaVu Bold 'b/d/g/p/q')
  latUpper: 0.86, // 其余大写拉丁             —— max 0.850 (DejaVu Bold 'O/Q')
  latWide: 1.1, // m w M W @ % & # + = < > { } ~ ^ $ ? _ – — … —— max 1.103 (DejaVu Bold 'W')
  digit: 0.72, // 0-9                     —— max 0.696 (DejaVu Bold)
  cyrLower: 0.83, // 其余西里尔小写            —— max 0.820 (DejaVu Bold 'м')
  cyrUpper: 0.95, // 其余西里尔大写            —— max 0.940 (DejaVu Bold 'Ъ')
  cyrWide: 1.33, // щ ш ж ф ю ы Щ Ш Ж Ю Ы М Ф —— max 1.326 (DejaVu Bold 'Щ')
  arabic: 0.84, // 阿拉伯/波斯字母（连写前的保守上界，见文件头）—— DejaVu Bold 区均值 0.807
  arabicDigit: 0.64, // ۰-۹ / ٠-٩            —— max 0.639 (Noto Sans Arabic Bold)
  cjk: 1.0, // CJK/假名/谚文/全角        —— 恒等宽 1.000 (Noto Sans CJK)
  zero: 0, // ZWNJ/ZWJ/LRM/RLM/BOM/组合符号
  unknown: 1.1, // 未归档字符：按最宽档兜底（宁可过估）
} as const;

const LAT_THIN = new Set([...`ijlI'.,:;`]);
const LAT_NARROW = new Set([...`frtJ!|()[]-/\\*"‘’“”`]);
const LAT_WIDE = new Set([...'mwMW@%&#+=<>{}~^$?_–—…']);
const CYR_WIDE = new Set([...'щшжфюыЩШЖЮЫМФ']);

export function charEm(ch: string): number {
  const cp = ch.codePointAt(0)!;
  if (cp === 0x20 || cp === 0xa0 || cp === 0x2007 || cp === 0x2009 || cp === 0x202f)
    return COEF.space;
  if (
    cp === 0x200b ||
    cp === 0x200c ||
    cp === 0x200d ||
    cp === 0x200e ||
    cp === 0x200f ||
    cp === 0x061c ||
    cp === 0xfeff ||
    (cp >= 0x0300 && cp <= 0x036f) ||
    (cp >= 0x064b && cp <= 0x065f) ||
    cp === 0x0670
  )
    return COEF.zero;
  if (cp >= 0x30 && cp <= 0x39) return COEF.digit;
  if ((cp >= 0x06f0 && cp <= 0x06f9) || (cp >= 0x0660 && cp <= 0x0669)) return COEF.arabicDigit;
  if (
    (cp >= 0x0600 && cp <= 0x06ff) ||
    (cp >= 0x0750 && cp <= 0x077f) ||
    (cp >= 0xfb50 && cp <= 0xfdff) ||
    (cp >= 0xfe70 && cp <= 0xfeff)
  )
    return COEF.arabic;
  if ((cp >= 0x0400 && cp <= 0x04ff) || (cp >= 0x0500 && cp <= 0x052f)) {
    if (CYR_WIDE.has(ch)) return COEF.cyrWide;
    return (cp >= 0x0410 && cp <= 0x042f) || (cp >= 0x0400 && cp <= 0x040f)
      ? COEF.cyrUpper
      : COEF.cyrLower;
  }
  if (
    (cp >= 0x4e00 && cp <= 0x9fff) ||
    (cp >= 0x3400 && cp <= 0x4dbf) ||
    (cp >= 0x3000 && cp <= 0x303f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x3040 && cp <= 0x30ff) ||
    (cp >= 0xac00 && cp <= 0xd7af)
  )
    return COEF.cjk;
  if (LAT_THIN.has(ch)) return COEF.latThin;
  if (LAT_NARROW.has(ch)) return COEF.latNarrow;
  if (LAT_WIDE.has(ch)) return COEF.latWide;
  if (cp < 0x250) {
    if (ch >= 'a' && ch <= 'z') return COEF.latLower;
    if (ch >= 'A' && ch <= 'Z') return COEF.latUpper;
    if (cp >= 0xc0)
      return ch === ch.toUpperCase() && ch !== ch.toLowerCase() ? COEF.latUpper : COEF.latLower;
    return COEF.latLower;
  }
  return COEF.unknown;
}

export const textEm = (s: string) => [...s].reduce((t, ch) => t + charEm(ch), 0);

interface TypeSpec {
  fontSize: number;
  /** CSS `letter-spacing`，单位 em（px 值需先除 font-size 后传入）。 */
  letterSpacingEm?: number;
  /** CSS `text-transform:uppercase`。大写字母更宽，必须先转换再测。 */
  uppercase?: boolean;
}
export function textPx(text: string, { fontSize, letterSpacingEm = 0, uppercase = false }: TypeSpec) {
  const s = uppercase ? text.toUpperCase() : text;
  // CSS 的 letter-spacing 在**每个**字符后加一份（含末字符），故乘字符数而非 n-1。
  return (textEm(s) + letterSpacingEm * [...s].length) * fontSize;
}

/**
 * 「不可断单元」判定：相邻两个 CJK 表意文字 / 假名 / 谚文之间有断行机会（UAX #14 class ID，
 * `word-break:normal` 下浏览器默认就在这里断）。**标点两侧一律不算**（`（`『，』`）`… 有禁则），
 * 故本集合刻意排除 U+3000–303F、U+FF00–FF60 —— 少给断点 = 偏保守 = 只会高估行数，不会漏报。
 */
const cjkBreakable = (cp: number) =>
  (cp >= 0x4e00 && cp <= 0x9fff) || // CJK 统一表意
  (cp >= 0x3400 && cp <= 0x4dbf) || // 扩展 A
  (cp >= 0x3040 && cp <= 0x30fa) || // 平/片假名（排除 U+30FB「・」）
  (cp >= 0x30fc && cp <= 0x30ff) ||
  (cp >= 0xac00 && cp <= 0xd7af); // 谚文音节

/**
 * 把一行文本切成**不可断单元**：空白处切（空白被吃掉，`space=true` 表示该单元前有一个空格），
 * 相邻 CJK 之间切（不吃字符，`space=false`）。
 */
function breakUnits(s: string): { text: string; space: boolean }[] {
  const out: { text: string; space: boolean }[] = [];
  const chars = [...s];
  let cur = '';
  let space = false;
  const flush = (nextSpace: boolean) => {
    if (cur) out.push({ text: cur, space });
    cur = '';
    space = nextSpace;
  };
  for (let i = 0; i < chars.length; i++) {
    const ch = chars[i];
    if (/[\s​]/u.test(ch)) {
      flush(true);
      continue;
    }
    cur += ch;
    const nx = chars[i + 1];
    if (nx && cjkBreakable(ch.codePointAt(0)!) && cjkBreakable(nx.codePointAt(0)!)) flush(false);
  }
  flush(false);
  return out;
}

/**
 * 贪心排版，返回占几行。
 *
 * - `nowrap` ⇒ 恒 1 行（整串宽 > 可用宽即溢出，由调用方判）。
 * - `breakAnywhere=false` ⇒ 只在**断行机会**处断（空白 + 相邻 CJK 之间，见 `breakUnits`）；
 *   某个不可断单元本身超宽时该行照样溢出（返回的行宽会 > avail，由调用方按 `maxLineWidth` 判溢出）。
 * - `breakAnywhere=true`（CSS `overflow-wrap:anywhere`）⇒ 超宽单元在内部断，永不溢出。
 * - `tailPx` = 行尾还挂着一段**固定宽的 inline 内容**（S11 的 `.fld-opt`「可选」徽标：字号与标签
 *   不同 ⇒ 塞不进同一个 `TypeSpec` 里量）。放不下就跟浏览器一样把它挤到下一行。
 *
 * ── CJK 断点是 2026-08-06 补的，方向是**去掉假红** ────────────────────────────────────────
 * 旧版只按空白切词，于是一整句无空格中文 = **一个不可断的词**，其宽度必然 > 任何窄容器
 * ⇒ 对「不可断单元超宽」那条判据是**误报**方向。S1–S10 一直没踩到：它们要么带
 * `overflow-wrap:anywhere`（词内可断，误差被吸收），要么文案短。S11 的标签与 tooltip 两条都不占，
 * 首次把它暴露成假红（zh-TW `node.field.muxPadHint` 整句算 357.0px vs 可用 346px，而真机在每两个
 * 汉字之间都能断，实测 5 行、最宽行 343.0px）。
 * 加断点只会让行数与最宽行**单调变小**（贪心排版下断点集变大 ⇒ 每行填得不会更少），
 * 故对 S1–S10 只可能更松，不可能把已绿的判红 —— 不是放宽预算，是修掉模型的高估。
 * 仍然保守：不在 `-` / `/` / 标点前后断（真实浏览器会），故真实行数 ≤ 本函数。
 */
function layout(
  s: string,
  avail: number,
  type: TypeSpec,
  opts: { wrap: boolean; breakAnywhere: boolean; tailPx?: number },
): { lines: number; maxLineWidth: number } {
  const w = (t: string) => textPx(t, type);
  const tail = opts.tailPx ?? 0;
  if (!opts.wrap) return { lines: 1, maxLineWidth: w(s) + tail };
  const spaceW = w(' ');
  let lines = 1;
  let cur = 0;
  let maxLine = 0;
  const push = (width: number) => {
    maxLine = Math.max(maxLine, width);
  };
  for (const unit of breakUnits(s)) {
    const ww = w(unit.text);
    const sep = unit.space && cur > 0 ? spaceW : 0;
    if (cur > 0 && cur + sep + ww > avail) {
      push(cur);
      lines++;
      cur = 0;
    }
    if (ww <= avail || !opts.breakAnywhere) {
      cur = cur === 0 ? ww : cur + sep + ww;
      continue;
    }
    // 单元内断：逐字符填满一行再换（`overflow-wrap:anywhere`）。
    for (const ch of unit.text) {
      const cw = w(ch);
      if (cur > 0 && cur + cw > avail) {
        push(cur);
        lines++;
        cur = 0;
      }
      cur += cw;
    }
  }
  if (tail > 0) {
    if (cur > 0 && cur + tail > avail) {
      push(cur);
      lines++;
      cur = 0;
    }
    cur += tail;
  }
  push(cur);
  return { lines, maxLineWidth: maxLine };
}

// ════════════════════════════════════════════════════════════════════════════════════════
// ② CSS 几何解析 —— 读不到即 throw
// ════════════════════════════════════════════════════════════════════════════════════════

/**
 * 取材面。**这里是两条互不相干的层叠链，别当成一条**：
 *
 *  · **桌面链**（前五份）：`index.css` 的 @import 顺序（index.css:14-16）
 *    components → screens → prototype，index.css 自身规则在最后；`tray-overlay.css` 是托盘那个
 *    独立 webview 的覆盖层。下面 `wraps()` / `ALL` 的「后者胜」全部按这个顺序说话。
 *  · **移动链**（后五份）：移动端文档（`ui/mobile.html` → `../mobile/MobileMain.tsx`）**一个
 *    `@import` 都不写**，只 import `tokens.resolved.css` + `mobile.css`；各屏 CSS 由屏组件自引
 *    （`home.css` / `nodes.css` / `rules-screen.css` / `connections.css` 由各屏那条 import 链
 *    带进来）。故它与桌面那五份**永远不会同时生效在同一个文档里** —— 两条链共用本数组只是为了
 *    共用同一套解析器。
 *
 * 两条链之间逐字重名的选择器只有 `:root` / `html` / `body` / `#root` 四个，而本文件的
 * `wraps()` / `clips()` / `breaksAnywhere()` / `colCapPx()` 一次都没查过它们（查的全是 `.nav-item`
 * `.tray-i` `.fld-l` `.h-nodename` 这类具体槽位），`decl()` / `dupAgreed()` / `SPACING` 又都是
 * **按文件**取值 ⇒ 追加移动端那几份不会改变 S1–S12 任何一个槽位的解析结果。
 * 这条不是「我看过一眼」：下面 ⑩ 有一条断言把**两条链的选择器交集**钉死在这四个上 —— 哪天
 * 移动端起了一个与桌面槽位重名的类（`.card-sub` / `.nav-item` …），它当场红，逼人重新过一遍
 * 「后者胜」会不会串台，而不是等桌面某道门莫名其妙地绿。
 *
 * ── 移动端这一半为什么改成「目录树发现 + 恰等对拍」（2026-09-05）───────────────────────
 *
 * 本数组此前只有 `mobile.css` + `home.css`，于是**节点 / 规则 / 连接三个屏一份都没进面**，
 * 而「注册表里没有 = 门看不见」已经是这道门第三次踩同一个坑（S11 `.fld` 一族、⑨ `.imp-*`、
 * ⑩ 移动端首页）。前三次的修法都是「手工把漏的那份加进来」，那修的是这一次不是这一类：
 * 第六个屏落地时照样悄悄留在门外。
 *
 * 故移动端那一半现在有一条**自曝断言**（见下面 ⓪）：`src/mobile/**` 下走目录树发现的全部
 * `.css` 必须与本数组里的移动端条目**恰等**。多一份少一份都红，加一屏就必须在这里显式记一笔
 * （连同它的槽位注册）。桌面那五份仍是手工列表 —— 它们不是「一个目录下的同构文件」，
 * `tokens.css` / `fonts.css` / `tokens.resolved.css` 就在同一个目录里而**不该**进面。
 */
const CSS_FILES = [
  './components.css',
  './screens.css',
  './prototype.css',
  './index.css',
  '../tray/tray-overlay.css',
  '../mobile/mobile.css',
  '../mobile/theme.css', // Token-only dark layer; no text geometry, audited in mobile/theme.test.tsx.
  '../mobile/redesign.css',
  '../mobile/home/home.css',
  '../mobile/nodes/nodes.css',
  '../mobile/screens/rules/rules-screen.css',
  '../mobile/screens/rules/rules-redesign.css',
  '../mobile/connections/connections.css',
  '../mobile/connections/connections-redesign.css',
  '../mobile/settings/settings.css',
  /* 表单宿主（2026-09-06 批 2）。它不属于任何一屏 —— 由 `MobileApp` 挂在外壳之外，
     节点屏与首页都开得动它，故与五屏并列登记一条。 */
  '../mobile/forms/forms.css',
] as const;
type CssFile = (typeof CSS_FILES)[number];

/** 移动链那一半（`CSS_FILES` 的子集）—— ⓪ 的恰等对拍与 ⑩ 的层叠链交集断言都读它。 */
const MOBILE_CSS_FILES = CSS_FILES.filter((f) => f.startsWith('../mobile/'));

/**
 * `src/mobile/**` 下走目录树发现的全部 `.css`（相对本文件的路径，排序稳定）。
 * ⓪ 拿它与 `CSS_FILES` 的移动端条目做恰等对拍。
 */
const mobileCssOnDisk = (): string[] => {
  const root = fileURLToPath(new URL('../mobile', import.meta.url));
  const out: string[] = [];
  const walk = (dir: string, prefix: string) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = `${dir}/${e.name}`;
      if (e.isDirectory()) walk(p, `${prefix}${e.name}/`);
      else if (e.name.endsWith('.css')) out.push(`../mobile/${prefix}${e.name}`);
    }
  };
  walk(root, '');
  return out.sort();
};

/**
 * 解析器缓存。**键面刻意比取材面宽一点**：除了 `CSS_FILES`，还预读 `src/mobile/**` 下全部 `.css`。
 *
 * 这不是「悄悄把它们纳入面」—— 面是 `CSS_FILES`，`ALL`（`wraps()`/`clips()`/`breaksAnywhere()`
 * 的「后者胜」）与 `MOBILE_CSS_FILES`（⓪ 的对拍、⑩ 的层叠链交集）读的都还是它，
 * 一份 CSS 只要不在 `CSS_FILES` 里就**不参与任何层叠判定**。
 *
 * 宽这一点是为了**错误质量**：某份移动端 CSS 被从 `CSS_FILES` 里删掉时（取材面被缩窄），
 * 下面 S14–S16 的几何链是**模块级**求值的，`decl()` 会立刻去查一个不存在的缓存键 ⇒
 * 整份文件在 import 期炸成 `Cannot read properties of undefined`、`Tests no tests`。
 * 那种形态虽然不会被误判成绿，但说话的是一个下游崩溃，不是 ⓪ 那句写好的话；
 * 而「0 test」在只看「有没有 FAIL 用例」的语境里会被读成「没跑」。
 * 预读之后模块照常加载，由 ⓪ 用一句点名到具体文件的断言把它报出来 —— 同时
 * `breaksAnywhere()` 那批读 `ALL` 的判据也会跟着红（那份 CSS 真的退出了层叠面）。
 */
const SRC = new Map<string, string>(
  [...new Set<string>([...CSS_FILES, ...mobileCssOnDisk()])]
    // 登记了却不在磁盘上的那一份（僵尸条目）在这里**跳过**而不是 ENOENT 炸掉整份文件 ——
    // 同上：让 ⓪ 的 `zombie` 那条点名报它，而不是用一个 import 期异常换掉那句写好的话。
    .filter((f) => existsSync(fileURLToPath(new URL(f, import.meta.url))))
    .map((f) => [f, stripComments(read(f))]),
);

interface Rule {
  sel: string;
  body: string;
}
const rulesOf = (file: CssFile): Rule[] => {
  const css = SRC.get(file);
  // 兜底具名化：`SRC` 已覆盖「面 ∪ 移动端磁盘」，还查不到只可能是桌面那五份被改名/搬走。
  if (css === undefined)
    throw new Error(`解析器缓存里没有 ${file} —— 它既不在 CSS_FILES，也不在 src/mobile/** 的磁盘上`);
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({
    // 选择器只取最后一个 `;` 之后：`@import '…';` / `@tailwind x;` 会被 `[^{}]+` 吞进来。
    sel: m[1].split(';').pop()!.trim().replace(/\s+/g, ' '),
    body: m[2].replace(/\s+/g, ' ').trim(),
  }));
};
const ALL: { file: CssFile; sel: string; body: string }[] = CSS_FILES.filter((f) =>
  // 僵尸条目（登记了但磁盘上没有）不进层叠面。它不是「悄悄放过」：⓪ 的 `zombie` 那条会点名报它，
  // 而它守的槽位也会因为退出 `ALL` 而在各自的「修法在位」判据上一起红。
  SRC.has(f),
).flatMap((f) => rulesOf(f).map((r) => ({ file: f, ...r })));

/** 选择器逐字匹配（含逗号分组内的一项）。 */
const selMatches = (sel: string, want: string) =>
  sel.split(',').some((s) => s.trim().replace(/\s+/g, ' ') === want);

/**
 * 每份 CSS 的独立层叠上下文（共享解析器 `styles/css-cascade.test-support.ts`，缓存）。
 *
 * 喂**原文**不喂 `SRC`（那份已被本文件的 `stripComments` 处理过，且不保长度 ⇒ 行号算不回去）。
 * 解析器自己会保长度剥注释。
 */
const CTX = new Map<string, CssContext>();
const ctxOf = (file: string): CssContext => {
  let c = CTX.get(file);
  if (c === undefined) {
    c = contextOf([{ file, css: read(file) }]);
    CTX.set(file, c);
  }
  return c;
};

/** 简写查询：从**四个分量各自的胜出值**重建，不是回读某一条声明的原文。 */
const BOX4 = new Set(['padding', 'margin', 'inset']);
/**
 * `decl(file, sel, 'padding')` 的返回口径是**四分量** `T R B L`（各自走完层叠再拼），
 * 不是 CSS 里那句简写的原文。断言右边用这个helper写出来，读起来还是「四边都继承同一个标量」。
 * 顺带把判据变严：`padding-inline: 20px` 这种同族覆写会让某两个分量变掉 ⇒ 当场红。
 */
const box4 = (v: string) => `${v} ${v} ${v} ${v}`;

function declIn(ctx: CssContext, where: string, selector: string, prop: string): string {
  const r = resolve({ sel: selector, prop, decls: ctx, where: UNCONDITIONAL });
  if (r.winner === null)
    throw new Error(
      `CSS 里读不到 ${where} \`${selector}\` 的 \`${prop}\` —— 选择器被改名/删除？\n${explain(r)}`,
    );
  return r.winner.value;
}

/**
 * 每份 CSS 属于哪条层叠链 —— **单文件上下文的胜出值必须等于整条链上的胜出值**。
 *
 * 2026-09-05 MAJ-4：本文件的 `ctxOf` 逐文件建独立上下文，而进包顺序里
 * `src/styles/index.css`（桌面链）与 `src/mobile/mobile.css`（移动链）都排在**最后** ——
 * 它们的同键覆写在单文件模型里根本不存在。实测：`index.css` 追加 `.tray-i{font-size:20px}`
 * 之后托盘条目字号 12.5→20，而 S6/S8/S9 的宽度预算仍按 12.5 结算并报「放得下」，全量 rc=0。
 * 形态与判据都抄同批的 `components/layout/window-drag-region.test.ts:52-58`。
 */
const CHAIN_OF: Readonly<Record<CssFile, CtxName>> = {
  './components.css': 'desktop',
  './screens.css': 'desktop',
  './prototype.css': 'desktop',
  './index.css': 'desktop',
  '../tray/tray-overlay.css': 'tray',
  '../mobile/mobile.css': 'mobile',
  '../mobile/theme.css': 'mobile',
  '../mobile/redesign.css': 'mobile',
  '../mobile/home/home.css': 'mobile',
  '../mobile/nodes/nodes.css': 'mobile',
  '../mobile/screens/rules/rules-screen.css': 'mobile',
  '../mobile/screens/rules/rules-redesign.css': 'mobile',
  '../mobile/connections/connections.css': 'mobile',
  '../mobile/connections/connections-redesign.css': 'mobile',
  '../mobile/settings/settings.css': 'mobile',
  '../mobile/forms/forms.css': 'mobile',
};

/** 单文件读数与整条链读数不一致 ⇒ 抛并点名是哪条链把它盖掉了。 */
function assertSurvivesChain(file: CssFile, selector: string, prop: string, single: string): void {
  const chain = CHAIN_OF[file];
  const r = resolve({ sel: selector, prop, ctx: chain, where: UNCONDITIONAL });
  if (r.winner === null || r.winner.value === single) return;
  throw new Error(
    `\`${selector} { ${prop} }\` 在 ${file} 里读到 \`${single}\`，` +
      `但在整条 \`${chain}\` 层叠链上的胜出值是 \`${r.winner.value}\`` +
      `（${r.winner.decl.file}:${r.winner.decl.line}）—— 有人在**后面进包**的文件里把它盖掉了，` +
      `本节的几何结算读的是一个真机上不成立的值。\n${explain(r)}`,
  );
}

/**
 * 取 `file` 里 `selector` 上 `prop` 的**层叠胜出值**。找不到 → throw（不静默跳过）。
 *
 * 迁移前是「按源码顺序找**第一条**声明了这个**精确属性名**的规则」，两个洞：
 *  · **首条命中 vs 后者胜**：末尾追加一条 `.tray-i{gap:40px}`，浏览器里是 40，这里读到 10 —— 全绿；
 *  · **同族全盲**：连追加都不用，在同一条规则里补一句 `column-gap:40px` 就够了（`gap` 前是 `-` 不是 `;`，
 *    旧正则匹配不到），托盘条目可用文字宽度当场少 30px 而 S8 照旧绿。
 *
 * 现在收全部命中、`!important` → 源序决胜、简写/长写/逻辑属性折到同一个语义分量。
 * 条件面取 `UNCONDITIONAL`：本门量的是**基础档**的可用宽度，断点块里的覆写另有各节自己的判据；
 * 旧的 `rulesOf` 正则看不见 at-rule、把 `@media{…}` 里的规则拍平成顶层同列，那是另一个方向的错。
 */
function decl(file: CssFile, selector: string, prop: string): string {
  const ctx = ctxOf(file);
  const one = (p: string): string => {
    const v = declIn(ctx, file, selector, p);
    assertSurvivesChain(file, selector, p, v);
    return v;
  };
  if (BOX4.has(prop)) return (['top', 'right', 'bottom', 'left'] as const).map((e) => one(`${prop}-${e}`)).join(' ');
  return one(prop);
}

const px = (v: string): number => {
  if (/^-?0(\.0+)?$/.test(v.trim())) return 0; // CSS 允许 0 省单位（`padding:0 10px 12px`）
  const m = v.match(/(-?[\d.]+)px/);
  if (!m) throw new Error(`不是 px 值：\`${v}\``);
  return parseFloat(m[1]);
};
/** `padding: a b c d` 简写 → 左右内边距之和。 */
function padX(shorthand: string): number {
  const parts = shorthand.trim().split(/\s+/).map(px);
  if (parts.length === 1) return parts[0] * 2;
  return parts[1] * 2; // 2/3/4 值写法里第 2 个都是左右
}
/**
 * 间距阶梯 `--sp-N` 的真值（`prototype.css` 的 `:root`，见 index.css 段注释：prototype.css 是最后一个
 * @import，它自带一整套同选择器令牌声明 ⇒ tokens.css 那份不是生效的那份）。
 * 缺一档即 throw：本门里 `padding:var(--sp-4)` 这类简写全靠它解出 px。
 */
const SPACING: ReadonlyMap<string, number> = (() => {
  const m = new Map<string, number>();
  for (const { sel, body } of rulesOf('./prototype.css')) {
    if (sel !== ':root') continue;
    for (const d of body.split(';')) {
      const mm = d.match(/(--sp-\d+)\s*:\s*([\d.]+)px/);
      if (mm) m.set(mm[1], parseFloat(mm[2]));
    }
  }
  if (m.size < 7)
    throw new Error(`prototype.css 的 :root 里只解析到 ${m.size} 档 --sp-* —— 间距阶梯被改名/搬走？`);
  return m;
})();
/** 把值里的 `var(--sp-N)` 换成 px 字面量；解析不到的 var 直接 throw（不静默留原样）。 */
const resolveSp = (v: string): string =>
  v.replace(/var\(\s*(--sp-\d+)\s*\)/g, (_, k: string) => {
    const n = SPACING.get(k);
    if (n === undefined) throw new Error(`\`${k}\` 不在 prototype.css 的间距阶梯里`);
    return `${n}px`;
  });

/** `letter-spacing` → em。`.1em` 直接取；`normal` = 0。 */
function lsEm(v: string, fontSize: number): number {
  if (/normal/.test(v)) return 0;
  const em = v.match(/(-?[\d.]+)em/);
  if (em) return parseFloat(em[1]);
  return px(v) / fontSize;
}

/**
 * 解析「这段文本会不会换行」：按 @import 顺序扫全部文件，取**最后一条**命中的
 * `white-space` 声明（同特异性后者胜；覆盖层写在 index.css 全部 @import 之后）。
 * `selectors` 按「从祖先继承 → 自身」排列。
 */
function wraps(selectors: string[]): boolean {
  let last: string | undefined;
  for (const { sel, body } of ALL) {
    if (!selectors.some((s) => selMatches(sel, s))) continue;
    const m = body.match(/(?:^|;)\s*white-space\s*:\s*([^;]+)/);
    if (m) last = m[1].trim();
  }
  if (last === undefined) return true; // 无声明 = 初始值 normal = 换行
  return !/nowrap|pre(?!-line|-wrap)/.test(last);
}
/** 该文本链路上有没有 `text-overflow:ellipsis`（有 = 截断而非画到框外）。 */
function clips(selectors: string[]): boolean {
  return ALL.some(
    (r) => selectors.some((s) => selMatches(r.sel, s)) && /text-overflow\s*:\s*ellipsis/.test(r.body),
  );
}
/** 该文本链路上有没有 `overflow-wrap:anywhere|break-word`（有 = 超宽单词可在词内断，永不溢出）。 */
function breaksAnywhere(selectors: string[]): boolean {
  return ALL.some(
    (r) =>
      selectors.some((s) => selMatches(r.sel, s)) &&
      /(?:overflow-wrap|word-break)\s*:\s*(anywhere|break-word|break-all)/.test(r.body),
  );
}

// ════════════════════════════════════════════════════════════════════════════════════════
// ③ i18n 语料 + 容器↔键 映射（都从源码解析，解析不到即 throw）
// ════════════════════════════════════════════════════════════════════════════════════════

/** 语种**从目录列出**，不写死清单 —— 新增一个 locale 文件即自动进门（写死就等于新语种默认免检）。 */
const LOCALES: string[] = readdirSync(fileURLToPath(new URL('../i18n/locales', import.meta.url)))
  .filter((f) => f.endsWith('.json'))
  .map((f) => f.replace(/\.json$/, ''))
  .sort();
type Locale = string;

const flatten = (o: unknown, p = '', out: Record<string, string> = {}) => {
  for (const [k, v] of Object.entries(o as Record<string, unknown>)) {
    const kk = p ? `${p}.${k}` : k;
    if (typeof v === 'string') out[kk] = v;
    else if (v && typeof v === 'object') flatten(v, kk, out);
  }
  return out;
};
/**
 * 语料 = 主分区 + 辅助 webview 分区（`locales/auxiliary/`，托盘与更新弹窗的 `tray.*` / `updatePopup.*`；
 * 分区理由见 `i18n/locale-parity.test.ts` 里那段注释——打包上分家，量文案时合成一份）。
 * `aux/` 读不到即 `read()` 抛错，不静默跳过（跳过 = 托盘/弹窗文案又一次不进门）。
 */
const DICT: Record<Locale, Record<string, string>> = Object.fromEntries(
  LOCALES.map((l) => [
    l,
    {
      ...flatten(JSON.parse(read(`../i18n/locales/${l}.json`))),
      ...flatten(JSON.parse(read(`../i18n/locales/auxiliary/${l}.json`))),
    },
  ]),
) as Record<Locale, Record<string, string>>;
if (LOCALES.length < 5) throw new Error(`只列出 ${LOCALES.length} 个 locale —— locales 目录读错了？`);

const src = (rel: string) => read(rel);
function grepAll(text: string, re: RegExp, what: string): RegExpMatchArray[] {
  const hits = [...text.matchAll(re)];
  if (hits.length === 0) throw new Error(`源码里一条 ${what} 都没解析到 —— 渲染方式变了？正则失效？`);
  return hits;
}

/** 主侧栏 nav 标签键（Sidebar.tsx 的 `labelKey: 'sidebar.x'` + 贴底 settings + 折叠/展开 aria）。 */
function sidebarNavKeys(): string[] {
  const s = src('../components/layout/Sidebar.tsx');
  const keys = grepAll(s, /labelKey:\s*'([\w.]+)'/g, 'Sidebar labelKey').map((m) => m[1]);
  // 贴底「设置」不走 NAV_DEF 表，是内联 `t('sidebar.settings')`（两处：data-tip + span）。
  const inline = grepAll(s, /t\('(sidebar\.settings)'\)/g, 'Sidebar 贴底 settings 键').map((m) => m[1]);
  return [...new Set([...keys, ...inline])];
}
/** 设置侧栏 nav 标签键（SettingsSidebar.tsx 的 `label: t('settings.nav.x')` + 贴底 back）。 */
function settingsNavKeys(): string[] {
  const s = src('../components/screens/settings/SettingsSidebar.tsx');
  const items = grepAll(s, /label:\s*t\('(settings\.nav\.[\w]+)'\)/g, 'SettingsSidebar label').map(
    (m) => m[1],
  );
  const back = grepAll(s, /t\('(settings\.nav\.back)'\)/g, 'SettingsSidebar back 键').map((m) => m[1]);
  return [...new Set([...items, ...back])];
}
/** 两个侧栏的 `.nav-group` 组头键。 */
function navGroupKeys(): string[] {
  const a = grepAll(
    src('../components/layout/Sidebar.tsx'),
    /t\('(sidebar\.group\.[\w]+)'\)/g,
    'Sidebar nav-group',
  ).map((m) => m[1]);
  const b = grepAll(
    src('../components/screens/settings/SettingsSidebar.tsx'),
    /header:\s*t\('(settings\.nav\.[\w]+)'\)/g,
    'SettingsSidebar nav-group',
  ).map((m) => m[1]);
  return [...new Set([...a, ...b])];
}
/** 连接表定宽列的表头：`thSortable('k', t('connections.colX'), 'c-y')` → [键, 列 class]。 */
function connTableFixedCols(): { key: string; cls: string }[] {
  const hits = grepAll(
    src('../components/screens/connections/ConnectionsScreen.tsx'),
    /thSortable\(\s*'[\w]+'\s*,\s*t\('([\w.]+)'\)\s*,\s*'(c-[\w-]+)'\s*\)/g,
    'ConnectionsScreen thSortable 定宽列',
  );
  return hits.map((m) => ({ key: m[1], cls: m[2] }));
}
/**
 * 托盘浮层用到的全部文案键（`t('tray.x')` + `MODES`/`TAKEOVERS` 表里的 `k: 'tray.x'`）。
 *
 * 2026-07-31 前这里扫的是 `t('中文','English')` 双语字面量。那个形态有个**结构性的漏**：正则是单行的，
 * 而 `t(\n '长中文',\n '长英文'\n)` 这种跨行写法扫不到 —— FakeIP 自动启用那条 30 字提示就一直在门外。
 * 改成扫键之后跨行与否都无所谓（键短，恒在一行内）。
 */
function trayKeys(): string[] {
  const s = src('../tray/TrayMenu.tsx');
  // 键后跟 `)`（单参）或 `,`（带 vars 的插值形态，如 W14 的 `t('tray.actionFailed', {…})`）。
  // 消费点检测不需要解析参数——只认 `t('key')` 的话，插值键会从扫描器缝里漏成「死键」假红。
  const inline = grepAll(s, /\bt\('(tray\.[\w]+)'[),]/g, "TrayMenu t('tray.x') 键").map((m) => m[1]);
  const tables = grepAll(s, /\bk:\s*'(tray\.[\w]+)'/g, 'TrayMenu MODES/TAKEOVERS 表键').map((m) => m[1]);
  // 收据状态分支把 key 交给 notice helper；它们仍是实际消费，不是死译文。
  const receiptBranches = grepAll(s, /'(tray\.switch(?:Pending|SavedForNextStart|RequiresApply))'/g, 'TrayMenu 收据提示键').map((m) => m[1]);
  return [...new Set([...inline, ...tables, ...receiptBranches])];
}
/** 更新弹窗按钮行：按 `.row` 收集按钮的 i18n 键（`updatePopup.*`）。 */
function updatePopupButtonRows(): { phase: string; keys: string[] }[] {
  const s = src('../update-popup/main.ts');
  const rows = grepAll(s, /<div class="row(?: between)?">([\s\S]*?)<\/div>/g, 'update-popup 按钮行');
  return rows.map((m, i) => ({
    phase: `row#${i + 1}`,
    keys: [...m[1].matchAll(/<button[^>]*>\$\{esc\(t\('(updatePopup\.[\w]+)'\)\)\}<\/button>/g)].map(
      (b) => b[1],
    ),
  }));
}

// ════════════════════════════════════════════════════════════════════════════════════════
// ④ 受限容器注册表 —— 可用宽全部现算，算式写在每个 `avail` 上
// ════════════════════════════════════════════════════════════════════════════════════════

/** `.side` / `.nav-item` / `.nav-group` 在 components.css 与 prototype.css 各存一份，必须同值。 */
const SIDEBAR_DUP = ['./components.css', './prototype.css'] as const;
function dupAgreed(selector: string, prop: string): string {
  const vals = SIDEBAR_DUP.map((f) => decl(f, selector, prop));
  if (new Set(vals).size !== 1)
    throw new Error(
      `\`${selector}\` 的 \`${prop}\` 在 components.css / prototype.css 不同值（${vals.join(' vs ')}）` +
        ` —— 本仓经典坑：两份重复定义只改了一份，后 @import 的 prototype.css 生效。`,
    );
  return vals[0];
}

/** 侧栏（主 + 设置共用 `.side`）几何。 */
const sideW = px(dupAgreed('.side', 'width')); // 148
const sidePadX = padX(dupAgreed('.side', 'padding')); // 10*2 = 20
const sideInner = sideW - sidePadX; // `*{box-sizing:border-box}`(prototype.css:63) ⇒ padding 吃在 148 内

const navPadX = padX(dupAgreed('.nav-item', 'padding')); // 9*2 = 18
const navGap = px(dupAgreed('.nav-item', 'column-gap')); // 10
const navIcon = px(dupAgreed('.nav-item svg', 'width')); // 17
const navFont = px(dupAgreed('.nav-item', 'font-size')); // 13
/** S1/S2 可用宽 = 148 − 10×2 − 9×2 − 17(icon) − 10(gap) */
const NAV_LABEL_AVAIL = sideInner - navPadX - navIcon - navGap;

const grpPadX = padX(dupAgreed('.nav-group', 'padding')); // 9*2 = 18
const grpFont = px(dupAgreed('.nav-group', 'font-size')); // 10
const grpLs = lsEm(dupAgreed('.nav-group', 'letter-spacing'), grpFont); // .1em
const grpUpper = /uppercase/.test(dupAgreed('.nav-group', 'text-transform'));
/** S3 可用宽 = 148 − 10×2 − 9×2 */
const NAV_GROUP_AVAIL = sideInner - grpPadX;

/** 连接表：语义 table 的 colgroup 定宽列（prototype.css 为唯一连接表布局来源）。 */
const thPadX = padX(decl('./prototype.css', '.conn-table th', 'padding')); // 12*2 = 24
const thFont = px(decl('./prototype.css', '.conn-table th', 'font-size')); // 10.5
const thLs = lsEm(decl('./prototype.css', '.conn-table th', 'letter-spacing'), thFont); // .05em
const thUpper = /uppercase/.test(decl('./prototype.css', '.conn-table th', 'text-transform'));
const sortArFont = px(decl('./prototype.css', '.conn-table th .sort-ar', 'font-size')); // 9
const sortArMl = px(decl('./prototype.css', '.conn-table th .sort-ar', 'margin-left')); // 3
/** 排序箭头 `▲` 常驻 DOM（未排序时只是 opacity:0，仍占宽）。 */
const SORT_AR_W = textPx('▲', { fontSize: sortArFont }) + sortArMl;
/** 列宽：跨全部 CSS 文件找视图专属 `col.c-x`；没有声明返回 null。 */
function colCapPx(cls: string): number | null {
  let cap: number | null = null;
  for (const r of ALL) {
    const matchesCol = r.sel
      .split(',')
      .some((selector) => selector.trim().replace(/\s+/g, ' ').endsWith(`col.${cls}`));
    if (!matchesCol) continue;
    const m = r.body.match(/(?:^|;)\s*(?:max-)?width\s*:\s*([^;]+)/);
    if (m) cap = px(m[1].trim());
  }
  return cap;
}

/**
 * 托盘浮层：窗宽由 Rust `TRAY_WIDTH` 定，卡片靠 margin 收进来。
 *
 * 取材面是**模块** `src-tauri/src/tray`（根文件 + `tray/**`，剔除 `tests/`），不是单文件 `tray.rs`：
 * 该常量已随浮层窗域搬进 `tray/window.rs`，写死单文件路径的旧写法会当场抛（还算体面），
 * 而任何「读不到就回落默认值」的写法会让整条几何链静默按陈旧宽度作证。
 */
const TRAY_WINDOW_W = (() => {
  const rs = moduleSource('src-tauri/src/tray');
  const m = rs.match(/const TRAY_WIDTH:\s*f64\s*=\s*([\d.]+)/);
  if (!m) throw new Error('tray 模块里读不到 TRAY_WIDTH —— 常量被改名？');
  return parseFloat(m[1]);
})();
const trayCardMarginX = padX(decl('../tray/tray-overlay.css', '.tray-menu', 'margin')); // 11*2
const trayCardPadX = padX(decl('./components.css', '.tray-menu', 'padding')); // 6*2
const trayInner = TRAY_WINDOW_W - trayCardMarginX - trayCardPadX;
const trayIPadX = padX(decl('../tray/tray-overlay.css', '.tray-menu .tray-i', 'padding')); // 11*2
const trayIGap = px(decl('./components.css', '.tray-i', 'column-gap')); // 10
const trayIcon = px(decl('./components.css', '.tray-i>svg:first-child', 'width')); // 16
const trayFont = px(decl('./components.css', '.tray-i', 'font-size')); // 12.5
const trayChev = px(decl('./components.css', '.tray-i .tray-chev', 'width')); // 15
/** S5 = 268 − 11×2 − 6×2 − 11×2 − 16(icon) − 10(gap) */
const TRAY_ROW_AVAIL = trayInner - trayIPadX - trayIcon - trayIGap;
/** S6 = S5 − 15(右侧勾/箭头) − 10(gap) */
const TRAY_ROW_TRAILING_AVAIL = TRAY_ROW_AVAIL - trayChev - trayIGap;

const trayStPadX = padX(decl('../tray/tray-overlay.css', '.tray-menu .tray-status', 'padding'));
const trayStGap = px(decl('./components.css', '.tray-status', 'column-gap')); // 9
const trayStMk = px(decl('./components.css', '.tray-status .ts-mk', 'width')); // 26
const trayStSubFont = px(decl('./components.css', '.tray-status div', 'font-size')); // 10.5
const trayStTitleFont = px(decl('./components.css', '.tray-status b', 'font-size')); // 12.5
/** S7 / S7b = 卡内宽 − 11×2 − 26(logo 磁贴) − 9(gap)。`.ts-tx{flex:1;min-width:0}` ⇒ 标题与副标题同槽。 */
const TRAY_STATUS_SUB_AVAIL = trayInner - trayStPadX - trayStMk - trayStGap;

const trayGrpPadX = padX(decl('./components.css', '.tray-group-h', 'padding')); // 11*2
const trayGrpFont = px(decl('./components.css', '.tray-group-h', 'font-size')); // 10
const trayGrpLs = lsEm(decl('./components.css', '.tray-group-h', 'letter-spacing'), trayGrpFont); // .06em
const trayGrpUpper = /uppercase/.test(decl('./components.css', '.tray-group-h', 'text-transform'));
/** S9 = 卡内宽 − 11×2 */
const TRAY_GROUP_AVAIL = trayInner - trayGrpPadX;

const trayNoteMarginX = padX(decl('../tray/tray-overlay.css', '.tray-menu .tray-note', 'margin')); // 8*2
const trayNotePadX = padX(decl('../tray/tray-overlay.css', '.tray-menu .tray-note', 'padding')); // 8*2
const trayNoteFont = px(decl('../tray/tray-overlay.css', '.tray-menu .tray-note', 'font-size')); // 11
/** S10 = 卡内宽 − 8×2(margin) − 8×2(padding)。带 `word-break:break-word` ⇒ 横向不可能溢出，只钉行数。 */
const TRAY_NOTE_AVAIL = trayInner - trayNoteMarginX - trayNotePadX;

const trayUpdateResultMax = px(
  decl('../tray/tray-overlay.css', '.tray-menu .tray-update-result', 'max-width'),
);
const trayUpdateResultPadX = padX(
  decl('../tray/tray-overlay.css', '.tray-menu .tray-update-result', 'padding'),
);
const trayUpdateResultFont = px(
  decl('../tray/tray-overlay.css', '.tray-menu .tray-update-result', 'font-size'),
);
/** S10b = 检查更新按钮右侧短结果徽标；max-width 含横向 padding（全局 border-box）。 */
const TRAY_UPDATE_RESULT_AVAIL = trayUpdateResultMax - trayUpdateResultPadX;

/** 更新弹窗：窗宽由 Rust `POPUP_WIDTH` 定。 */
const POPUP_W = (() => {
  const rs = readFileSync(
    fileURLToPath(new URL('../../../crates/updater/src/popup.rs', import.meta.url)),
    'utf8',
  );
  const m = rs.match(/pub const POPUP_WIDTH:\s*u32\s*=\s*(\d+)/);
  if (!m) throw new Error('crates/updater/src/popup.rs 里读不到 POPUP_WIDTH —— 常量被改名？');
  return parseFloat(m[1]);
})();
const popupDecl = (selector: string, prop: string): string => {
  // 弹窗是**第四个层叠上下文**（`src/update-popup/main.ts` 只 import 自己那一份 style.css），
  // 走共享解析器的 `context('popup')`，口径与上面的 `decl` 完全一致。
  const ctx = context('popup');
  if (BOX4.has(prop))
    return (['top', 'right', 'bottom', 'left'] as const)
      .map((e) => declIn(ctx, 'update-popup/style.css', selector, `${prop}-${e}`))
      .join(' ');
  return declIn(ctx, 'update-popup/style.css', selector, prop);
};
const popupCardPadX = padX(popupDecl('.card', 'padding')); // 16*2
const popupBtnPadX = padX(popupDecl('.btn', 'padding')); // 12*2
const popupBtnBorder = 2; // `.btn{border:1px solid}` 左右各 1px（border-box 下仍占内容宽）
const popupRowGap = px(popupDecl('.row', 'column-gap')); // 8
// `font: 13px/1.5 …` 的字号走**语义分量**：共享解析器把 `font` 简写折成 font-size/line-height/…
// ⇒ 后面补一句 `font-size: 20px` 也算数（读简写原文的旧写法对它全盲）。
const popupFont = px(popupDecl('body', 'font-size'));
/** S8 = 380 − 16×2 */
const POPUP_ROW_AVAIL = POPUP_W - popupCardPadX;

// ── S9 首页右列 seg2：几何链 + `:lang()` 名单 ──────────────────────────────────────────────
//
// 判「装不装得下」用的是**三档 min-content 之和**，不是「3 × 最宽档」：`.seg-wrap .seg2 button{flex:1}`
// = `flex:1 1 0%`，先按均分派 1/3，但 flex 项的 `min-width` 初始值是 `auto` ⇒ 自动最小尺寸 = min-content，
// 而按钮是 `white-space:nowrap` ⇒ 超过 1/3 的那颗冻在自己的 min-content 上、剩余空间再分给没冻的。
// 只有总和超了才真的溢出（整颗按钮越过轨道边框往外画，非文字越过按钮）。
// headless Chromium 逐字复核过这条算式与下面整条几何链（详见 index.css 的 S9 段注释）。

/** 窗口最小宽（`tauri.conf.json`，同时也是默认宽）—— 最小容器宽由它与 `.side` 现算，不写死。 */
const TAURI_MIN_WIDTH = (() => {
  const conf = JSON.parse(read('../../../src-tauri/tauri.conf.json')) as {
    app?: { windows?: { label?: string; minWidth?: number }[] };
  };
  const w = conf.app?.windows?.find((x) => x.label === 'main');
  if (!w || typeof w.minWidth !== 'number')
    throw new Error('tauri.conf.json 里读不到 main 窗口的 minWidth —— 窗口配置结构变了？');
  return w.minWidth;
})();
/** `.main` 是 `container:mainc/inline-size`，其 inline-size = 窗口宽 − `.side`（`flex:none`）。 */
const CONTAINER_MIN = TAURI_MIN_WIDTH - sideW;

const screenPadX = padX(resolveSp(dupAgreed('.screen', 'padding')));
const cardBorderX = px(dupAgreed('.card', 'border-left-width')) * 2;
const connCardPadX = padX(resolveSp(dupAgreed('.conn-card', 'padding')));
/** `.cc-cols` 两条 fr 轨道中的右列占比。 */
const CC_RIGHT_SHARE = (() => {
  const v = dupAgreed('.cc-cols', 'grid-template-columns');
  const fr = [...v.matchAll(/([\d.]+)fr/g)].map((m) => parseFloat(m[1]));
  if (fr.length !== 2) throw new Error(`.cc-cols 不再是两条 fr 轨道：\`${v}\` —— 右列占比得重新推`);
  if (px(dupAgreed('.cc-cols', 'column-gap')) !== 0) throw new Error('.cc-cols 有了 gap —— 右列宽算式要加一项');
  return fr[1] / (fr[0] + fr[1]);
})();
const ccRightPadLeft = px(resolveSp(dupAgreed('.cc-col.right', 'padding-left')));
const segPadX = padX(dupAgreed('.seg2', 'padding'));
const segBorderX = px(dupAgreed('.seg2', 'border-left-width')) * 2;
const segGap = px(dupAgreed('.seg2', 'column-gap'));
const homeSegBtnPadX = padX(dupAgreed('.cc-col.right .seg-wrap .seg2 button', 'padding'));
const segBtnFont = px(dupAgreed('.seg2 button', 'font-size'));

/** 容器宽 → `.seg2` 轨道外宽（`*{box-sizing:border-box}` ⇒ padding/border 都吃在各自宽度内）。 */
const trackFromContainer = (c: number) =>
  (c - screenPadX - cardBorderX - connCardPadX) * CC_RIGHT_SHARE - ccRightPadLeft;
/** 一组 n 档横排所需的轨道外宽 = Σ(文字 + 按钮内边距) + 档间 gap + 轨道 padding/border。 */
const trackNeededFor = (labels: string[]) =>
  labels.reduce((t, s) => t + textPx(s, { fontSize: segBtnFont }) + homeSegBtnPadX, 0) +
  segGap * (labels.length - 1) +
  segPadX +
  segBorderX;

/** 首页右列两组 seg2 的 i18n 键（`INTERCEPT_OPTS` / `ROUTING_OPTS` 的 labelKey，唯一真值源）。 */
function homeSegGroups(): { group: string; keys: string[] }[] {
  const s = src('../components/screens/home/HomeScreen.tsx');
  return ['INTERCEPT_OPTS', 'ROUTING_OPTS'].map((group) => {
    const m = s.match(new RegExp(`const ${group}[^=]*=\\s*\\[([\\s\\S]*?)\\];`));
    if (!m) throw new Error(`HomeScreen.tsx 里读不到 ${group} —— 常量被改名/换写法？`);
    const keys = grepAll(m[1], /labelKey:\s*'([\w.]+)'/g, `${group} 的 labelKey`).map((x) => x[1]);
    if (keys.length < 3) throw new Error(`${group} 只解析到 ${keys.length} 档 —— 正则失效？`);
    return { group, keys };
  });
}

// ── S11 节点弹窗表单字段（`.fld` 一族）：几何链 ──────────────────────────────────────────
//
// 节点弹窗走统一录入表单宽度：`.dlg.entry-form-dlg{width:min(540px, calc(100vw - 40px))}`；
// 主窗最小宽 980 ⇒ calc 支恒 ≥ 940px，永远不是较小的那个。若哪天改成随窗口变，下面解析会
// throw，而不是拿一个错的常数继续发绿。
const ENTRY_DLG_W = (() => {
  const v = decl('./index.css', '.dlg.entry-form-dlg', 'width');
  const m = v.match(/min\(\s*([\d.]+)px\s*,\s*calc\(\s*100vw\s*-\s*([\d.]+)px\s*\)\s*\)/);
  if (!m)
    throw new Error(
      `.dlg.entry-form-dlg 的 width 不再是 \`min(Npx, calc(100vw - Mpx))\`：\`${v}\` —— S11 的可用宽要重推`,
    );
  const [fixed, inset] = [parseFloat(m[1]), parseFloat(m[2])];
  if (TAURI_MIN_WIDTH - inset <= fixed)
    throw new Error(
      `窗口最小宽 ${TAURI_MIN_WIDTH}px 下 calc 支 = ${TAURI_MIN_WIDTH - inset}px ≤ ${fixed}px` +
        ` ⇒ 弹窗在小窗下不再定宽，S11 得改成按最小窗口宽算`,
    );
  return fixed;
})();
const BASE_DLG_W = (() => {
  const v = decl('./components.css', '.dlg', 'width');
  const m = v.match(/min\(\s*([\d.]+)px\s*,\s*calc\(\s*100vw\s*-\s*([\d.]+)px\s*\)\s*\)/);
  if (!m) throw new Error(`.dlg 的基准 width 形态变了：\`${v}\``);
  return parseFloat(m[1]);
})();
const dlgBorderX = px(decl('./components.css', '.dlg', 'border-left-width')) * 2; // 1*2
const dlgBodyPadX = padX(dupAgreed('.dlg-body', 'padding')); // 18*2 = 36
/** S11 录入表单内容槽（任务页没有额外横向 padding）= 540 − 1×2 − 18×2 = 502 */
const FLD_AVAIL = ENTRY_DLG_W - dlgBorderX - dlgBodyPadX;
/** 未使用 entry-form-dlg 的普通弹窗内容槽 = 460 − 1×2 − 18×2 = 422。 */
const BASE_FLD_AVAIL = BASE_DLG_W - dlgBorderX - dlgBodyPadX;

const swtRowGap = px(dupAgreed('.swt-row', 'column-gap')); // 12
const swtW = px(decl('./prototype.css', '.swt', 'width')); // 36
/**
 * 开关行的文本列：`.swt-row{display:flex}` 里 `.swt-tx{flex:1;min-width:0}` 是**唯一** grow 项、
 * flex-basis 0；`.swt` 定宽 36 且此处自由空间为正（不发生收缩）⇒ 文本列 = 容器 − gap − 36。
 */
const swtTextAvail = (container: number) => container - swtRowGap - swtW;

const fldLabelFont = px(dupAgreed('.fld-l', 'font-size')); // 11.5
const swtLabelFont = px(dupAgreed('.swt-row .swt-tx b', 'font-size')); // 12.5
const fldOptFont = px(decl('./components.css', '.fld-l > .fld-opt', 'font-size')); // 10
const tipFont = px(decl('./prototype.css', '#tip', 'font-size')); // 11.5
const tipAvail =
  px(decl('./prototype.css', '#tip', 'max-width')) -
  padX(decl('./prototype.css', '#tip', 'padding')) -
  px(decl('./prototype.css', '#tip', 'border-left-width')) * 2; // 280 − 18 − 2 = 260

/** 字段标签的选择器链（`wraps`/`clips`/`breaksAnywhere` 用）。 */
const FLD_LABEL_SELS = ['.fld-l', '.swt-row .swt-tx b'];

/**
 * `.fld-opt`「可选」徽标贴在标签行尾；52 个字段带 `opt:true`，不建模会把标签行
 * 系统性少算一截。运行时已禁止源码中文/defaultValue，取值链只有当前语种 → en-US。
 * locale 完整性由 i18n coverage 门负责；这里仍显式建模 en-US 回落，和 i18next 保持一致。
 */
const optTailOf = (own: string | undefined, en: string | undefined): string => {
  const value = own ?? en;
  if (!value) throw new Error('common.optional 在当前语种与 en-US 中均缺失');
  return value;
};
const optTail = (loc: Locale) =>
  optTailOf(DICT[loc]['common.optional'], DICT['en-US']['common.optional']);
const optTailPx = (loc: Locale) => textPx(` ${optTail(loc)}`, { fontSize: fldOptFont });

/** S11 的一个测点：某个 i18n 键渲染在哪个槽里（同一个键跨协议复用时只留一份）。 */
type FldSlot = 'LABEL' | 'SWT_LABEL' | 'TIP';
interface FldPoint {
  key: string;
  slot: FldSlot;
  /** cred/adv 只保留 codec 字段来源，展示时都在同宽录入表单内。 */
  section: 'cred' | 'adv';
  /** 该键至少在一处带「可选」徽标 ⇒ 按带徽标量（取最坏）。 */
  opt: boolean;
}
/**
 * 遍历 ND_SPEC 的协议字段，把 {cred, adv} 与专属 groups 一起摊成「键 → 槽」。三者最终都由
 * `nodeFormGroups` 放进 540px 录入弹窗的同宽内容区；section 仅用于诊断来源，不再改变宽度。
 *
 * 槽由 `FieldSpec.t` 决定，与 `FieldRenderer` 的分支一一对应：
 *  - `switch` → 标签渲染成 `.swt-tx b`（12.5px），hint/disabledHint 进入统一 `#tip` 浮层；
 *    开关不再因复杂说明永久增高。switch 分支不渲染 `.fld-opt`（无徽标）。
 *  - 其余 → 标签是 `.fld-l`（11.5px，带可选徽标），hint 同样进入统一 `#tip` 浮层。
 *    **hint 不再只有 `select` 有**：2026-08-07 `hint` 提到了 `FieldBase`，这里继续对所有非 switch
 *    字段无差别收集，否则新加的 text/textarea hint 会静默漏测。
 * 同一个键在多个协议里出现时取「任一处 opt 即 opt」，即最坏情形。
 */
function nodeFieldPoints(): FldPoint[] {
  const byId = new Map<string, FldPoint>();
  const put = (key: string, slot: FldSlot, section: 'cred' | 'adv', opt: boolean) => {
    const id = `${key}|${slot}`;
    const prev = byId.get(id);
    if (!prev) byId.set(id, { key, slot, section, opt });
    else {
      prev.opt ||= opt;
    }
  };
  let n = 0;
  for (const proto of Object.keys(ND_SPEC) as NodeProto[]) {
    for (const section of ['cred', 'adv'] as const)
      for (const f of ND_SPEC[proto][section]) {
        n++;
        if (f.t === 'switch') {
          put(f.label, 'SWT_LABEL', section, false);
          if (f.hint) put(f.hint, 'TIP', section, false);
          if (f.disabledHint) put(f.disabledHint, 'TIP', section, false);
        } else {
          put(f.label, 'LABEL', section, f.opt === true);
          if (f.hint) put(f.hint, 'TIP', section, false);
        }
      }
    for (const group of ND_SPEC[proto].groups ?? [])
      for (const f of group.fields) {
        n++;
        if (f.t === 'switch') {
          put(f.label, 'SWT_LABEL', 'cred', false);
          if (f.hint) put(f.hint, 'TIP', 'cred', false);
          if (f.disabledHint) put(f.disabledHint, 'TIP', 'cred', false);
        } else {
          put(f.label, 'LABEL', 'cred', f.opt === true);
          if (f.hint) put(f.hint, 'TIP', 'cred', false);
        }
      }
  }
  if (n < 100) throw new Error(`ND_SPEC 只走到 ${n} 个字段实例 —— 表结构变了？`);
  return [...byId.values()];
}

/**
 * S11 行数预算 —— **纵向没有硬上限**（`.dlg{max-height:calc(100vh-40px)}` + `.dlg-body{overflow-y:auto}`），
 * 超预算 = 「这一格变得很高、要多滚一段」，**不是**「被裁掉、点不到」（那是 S8 的形态）。
 * 所以这四个数是**棘轮**，不是物理墙；它们钉在今天最长的那一格上，让「再长一句」自曝。
 *
 * 逐档怎么定的（模型口径，模型偏宽 ⇒ 真机行数只会更少）：
 *  - `LABEL` / `SWT_LABEL` = **2**：这是个**设计判据**不是实测跟随 —— 标签是控件的名字，占到第 3 行
 *    就说明它其实是一句说明、该拆进标签旁的信息提示。今天实测最差正好 2 行
 *    （`.fld-l` 15/325 个测点 2 行、`.swt-tx b` 1/60 个测点 2 行），故它同时也是紧的。
 *  - `TIP` = **12**：所有字段的复杂说明进入 280px 的统一 tooltip；扣除内边距与边框后正文宽 260px。
 *    仍以 12 行为上限，避免“收进 i”变成容纳无限长文案的借口。
 */
const FLD_MAX_LINES: Record<FldSlot, number> = {
  LABEL: 2,
  SWT_LABEL: 2,
  TIP: 12,
};

// ── 溢出计算 ────────────────────────────────────────────────────────────────────────────
interface Over {
  where: string;
  loc: string;
  key: string;
  text: string;
  need: number;
  avail: number;
  lines: number;
  budget: number;
}
interface Box {
  where: string;
  avail: number;
  type: TypeSpec;
  wrap: boolean;
  breakAnywhere: boolean;
  /** 允许占几行。`nowrap` 容器恒为 1。 */
  maxLines: number;
  /** 行尾固定宽的 inline 附加物（S11 的「可选」徽标）。随 locale 变 ⇒ 调用点 spread 覆盖。 */
  tailPx?: number;
}
function check(bucket: Over[], box: Box, loc: string, key: string, text: string) {
  const { lines, maxLineWidth } = layout(text, box.avail, box.type, box);
  const over = maxLineWidth > box.avail + 0.001 || lines > box.maxLines;
  if (over)
    bucket.push({
      where: box.where,
      loc,
      key,
      text,
      need: maxLineWidth,
      avail: box.avail,
      lines,
      budget: box.maxLines,
    });
}
const fmt = (o: Over[]) =>
  o
    .map(
      (x) =>
        `  ${x.where} | ${x.loc} | ${x.key} | "${x.text}" | 最宽行 ${x.need.toFixed(1)}px / 可用 ${x.avail.toFixed(1)}px` +
        ` | ${x.lines} 行 / 预算 ${x.budget} 行`,
    )
    .join('\n');

// ════════════════════════════════════════════════════════════════════════════════════════
// ⑤ 门
// ════════════════════════════════════════════════════════════════════════════════════════

/**
 * ⓪ **取材面自曝** —— 「注册表里没有 = 门看不见」这一类的止损。
 *
 * 这道门三次假绿（S11 / ⑨ / ⑩）的成因都不是判据形态不适用，而是那一份 CSS 压根没进
 * `CSS_FILES`。手工列表修不了这一类：下一个屏落地时，没有任何东西会提醒人来这里加一行。
 * 故移动端那一半改成**目录树发现 + 恰等对拍**：树上有而表里没有 ⇒ 红（漏了一屏），
 * 表里有而树上没有 ⇒ 也红（登记了一份已删的僵尸）。
 *
 * ⚠️ 恰等对拍**不等于**「这些槽位都量过了」：进面只保证解析器读得到它，量不量还得有人在
 * 下面写槽位注册。两者的缺口由各节自己的「本节量的键仍真的被消费」那类双向对差兜。
 */
describe('⓪ 取材面：移动端每一份 CSS 都必须在面内（新增一屏不许悄悄留在门外）', () => {
  it('`src/mobile/**` 下的 .css 与 `CSS_FILES` 的移动端条目恰等（多一份少一份都红）', () => {
    const onDisk = mobileCssOnDisk();
    const face: string[] = [...MOBILE_CSS_FILES].sort();
    // 正面断言：走查器真的走到了东西（扫 0 个会让下面两条恒绿）。
    // 门槛不写「≥ 5 个屏」—— 那会把**今天的屏数**钉进自检里，删一屏时先红的是自检而不是
    // 下面那两条点名断言，报文就又指不到具体文件了。外壳 `mobile.css` 是恒在的那一份，用它做锚。
    expect(onDisk, '走查器没扫到外壳 `mobile.css` —— 目录结构变了，下面两条已失去意义').toContain(
      '../mobile/mobile.css',
    );
    expect(face.length, '取材面里一份移动端 CSS 都没有 —— `CSS_FILES` 被整段改写了？').toBeGreaterThan(0);
    // 两个方向**分开报**，而不是只丢一个数组 diff 让人自己找：报文要直接点名是哪一份。
    const missing = onDisk.filter((f) => !face.includes(f));
    const zombie = face.filter((f) => !onDisk.includes(f));
    // 文件名直接拼进**报文**里，不只放在被比较的数组里：vitest 对嵌套数组会印成
    // `[ Array(1) ]`，那样报文就又指不到具体是哪一份了。
    expect(
      missing,
      `磁盘上有、\`CSS_FILES\` 里没有：[${missing.join(', ')}]。` +
        '这个屏的受限容器一格都没人量 ——「注册表里没有 = 门看不见」这一类已经假绿过三次' +
        '（S11 / ⑨ / ⑩）。把它连同槽位注册一起加进来，别只加文件',
    ).toEqual([]);
    expect(
      zombie,
      `\`CSS_FILES\` 里有、磁盘上没有：[${zombie.join(', ')}]。` +
        '这是一条僵尸登记：那份 CSS 读不到 ⇒ 它不进 `ALL`（「后者胜」的层叠面），' +
        '本文件里挂在它身上的槽位从此无人守，而登记表看起来还是满的。' +
        '先确认那个屏是真的没了，再把它连同槽位注册一起摘掉',
    ).toEqual([]);
    // 恰等兜底：上面两条按集合判，这条连**重复登记**也一起挡掉。
    expect(face, '移动端 CSS 与取材面对不上（见上面两条的点名清单）').toEqual(onDisk);
  });

  /**
   * ── 登记：**「设置」屏在门外，这是一次有记录的取舍，不是漏了** ────────────────────────
   *
   * 事实：设置屏一份 `.css` 都没有。`settings/SettingsChrome.tsx` 用**内联样式**
   * （`CSSProperties` 常量：`ROW` / `ROW_TEXT` / `CONTROL_SLOT` / `MobileSelect` …），
   * 理由写在那个文件头：外壳定稿 + `mobile-entry.test.ts` 的 CSS 集合恰等，两条合起来让那一批
   * 只能走内联。故上面那条恰等断言**只覆盖四个屏**，别读成五个。
   *
   * 裁定（2026-09-05，协调者）：**登记在门外，不为它开第三份解析器。**
   * 判据是取材成本与覆盖收益不成比例 —— 要量它就得在「CSS 层叠解析」（本文件）与「TS AST」
   * （`write-failure-visibility` / `nodes-screen` ⑪）之外再维护第三份「TS 对象字面量几何解析」，
   * 而它守的是一个**已经定稿**、且 `mobile-entry.test.ts` 有 CSS 集合恰等钉着的外壳。
   *
   * ⚠️ **但那里确实有一个已知紧槽，算式原样留在这里，让下一个人看得见它没被量**：
   *
   *     卡内宽        = 390 − 2×16(--page-inset) − 2(border) − 2×16(--card-padding) = 324
   *     控件槽        = `CONTROL_SLOT` 是 `flex: '0 0 auto'` ⇒ **不收缩**
   *     `MobileSelect` = `minWidth: '150px'`
   *     `ROW` 的 gap  = 12
   *     ⇒ 带下拉的那些行里，`ROW_LABEL`（0.875rem = 14px）只剩 **324 − 12 − 150 ≈ 162px**
   *
   * 同排挂着按 max-content 取宽且不收缩的兄弟，标题只能拿到剩余宽度。
   * 设置屏这一格今天没人量 —— 不是判过装得下，是判过「不值得为它建第三份解析器」。
   *
   * ⚠️ **2026-09-05：设置屏长出了第一份 `.css`**（`settings/settings.css`，K5-02 的两列列流）。
   * 上面那条恰等断言当场红了 —— 记号如期过期，已连同 `CSS_FILES` 一起登记。
   * 但那份 CSS **只装列流那一条**（`column-count` 与 `@container` 内联表达不了），
   * 行几何仍然全部在内联常量里 ⇒ 上面那段「≈162px 没人量」的登记原样有效，一个字都不改。
   * 下面那条断言随之改判：从「设置屏没有 .css」改成「设置屏那份 .css 的射程只到列流」，
   * 哪天有人把行几何搬进去，它当场红，逼人把上面那段登记重写成真判据。
   */
  it('自曝：设置屏的**行几何**仍走内联样式（`settings.css` 的射程只到两列列流）', () => {
    expect(
      mobileCssOnDisk().filter((f) => f.startsWith('../mobile/settings/')),
      '设置屏的 CSS 不止一份 —— 下面那段射程声明要重写',
    ).toEqual(['../mobile/settings/settings.css']);
    const props = [
      ...new Set(ctxOf('../mobile/settings/settings.css').decls.map((d) => d.prop)),
    ].sort();
    expect(
      props,
      '`settings.css` 声明的属性集变了 —— 它一旦开始装行几何（padding / min-height / font-size…），' +
        '上面那段「≈162px 没人量」的登记就不再成立，必须改写成真判据而不是继续挂在这里',
    ).toEqual([
      'break-inside',
      'column-count',
      'column-gap',
      'display',
      'flex-direction',
      'gap',
      'margin-bottom',
    ]);
    // 正面断言：上面那段登记里**算式的每一项**都还在（不是「文件没了所以恒绿」）。
    // 任何一项改写法 ⇒ 那个 ≈162px 不再成立 ⇒ 这段登记必须重写，而不是继续挂在这里。
    const chrome = read('../mobile/settings/SettingsChrome.tsx');
    for (const [lit, why] of [
      ["flex: '0 0 auto'", '控件槽不再是不收缩的 `flex: 0 0 auto`'],
      ["minWidth: '150px'", '`MobileSelect` 的 150px 下限没了'],
      ["gap: '12px'", '`ROW` 的 12px gap 变了'],
      ["fontSize: '0.875rem'", '`ROW_LABEL` 的 14px 字号变了'],
    ] as const)
      expect(chrome, `SettingsChrome.tsx：${why} —— 上面那段「≈162px」的登记要重写`).toContain(lit);
  });
});

describe('移动端与桌面 CSS 选择器隔离', () => {
  // Keyframe steps are animation offsets, not document selectors. Strip only that
  // nested at-rule before comparing selectors; ordinary rules remain in the gate.
  const withoutKeyframes = (css: string): string => css.replace(/@(?:-[\w]+-)?keyframes\s+[^{}]+\{(?:[^{}]|\{[^{}]*\})*\}/g, '');
  it('动画帧不当成文档选择器，普通同名元素规则仍保留', () => {
    expect(withoutKeyframes('@keyframes spin { from {opacity:0} to {opacity:1} } to {color:red}')).toBe(' to {color:red}');
  });
  it('两条层叠链只共享根和文档选择器', () => {
    const selectors = (files: readonly CssFile[]) => {
      const out = new Set<string>();
      for (const file of files)
        for (const rule of withoutKeyframes(SRC.get(file)!).matchAll(/([^{}]+)\{([^{}]*)\}/g))
          for (const selector of rule[1].split(';').pop()!.split(',')) out.add(selector.trim().replace(/\s+/g, ' '));
      return out;
    };
    const desktop = selectors(['./components.css', './screens.css', './prototype.css', './index.css', '../tray/tray-overlay.css']);
    const mobile = selectors(MOBILE_CSS_FILES);
    expect(mobile.size, '移动端 CSS 没有进入取材面').toBeGreaterThan(300);
    expect([...mobile].filter((selector) => desktop.has(selector)).sort()).toEqual(['#root', ':root', 'body', 'html']);
  });
});

/**
 * 对照组用的**标定时几何**：`.side` 148px 那一版算出来的 83px 可用宽。
 * 写死是刻意的 —— ① 只考模型，不考几何；几何变了该由 ②/③ 报，不该把模型自校一起带红。
 */
const CALIB_NAV_AVAIL = 83;

describe('① 模型自校：已知溢出必须被算出来，已知不溢出不得被误判', () => {
  it('阳性对照 —— ru `sidebar.appPolicy` 在 83px 槽里必须被算成溢出', () => {
    const text = DICT.ru['sidebar.appPolicy'];
    expect(text).toBe('Политика приложений'); // 语料变了就该重新对照，不许静默跟着变
    const w = textPx(text, { fontSize: 13 });
    // 真机实测区间 124px(Segoe UI 估) – 172.0px(DejaVu Sans Bold 实测)；模型必须落在其上。
    expect(w).toBeGreaterThan(172);
    expect(w).toBeGreaterThan(CALIB_NAV_AVAIL);
  });

  it('阴性对照 —— 短标签不得被误判成溢出', () => {
    for (const [loc, key, realMax] of [
      ['ru', 'sidebar.server', 38.9], // "Узлы" DejaVu Bold 实测 38.9px
      ['zh-CN', 'sidebar.rules', 26.0], // "路由" = 2 全角 × 13px
      ['en-US', 'sidebar.home', 34.4], // "Home"
      ['zh-CN', 'sidebar.appPolicy', 52.0], // "应用分流" = 4 全角 × 13px
    ] as const) {
      const w = textPx(DICT[loc][key], { fontSize: 13 });
      expect(w, `${loc}/${key} 被模型算成 ${w.toFixed(1)}px，超了 83px 槽 = 误报`).toBeLessThanOrEqual(
        CALIB_NAV_AVAIL,
      );
      expect(w, `${loc}/${key} 模型 ${w.toFixed(1)}px < 实测 ${realMax}px = 模型漏报`).toBeGreaterThanOrEqual(
        realMax,
      );
    }
  });

  it('分档系数必须逐档单调有序，且 CJK 恒为 1em', () => {
    expect(COEF.latThin).toBeLessThan(COEF.latNarrow);
    expect(COEF.latNarrow).toBeLessThan(COEF.latLower);
    expect(COEF.latLower).toBeLessThan(COEF.latUpper);
    expect(COEF.latUpper).toBeLessThan(COEF.latWide);
    expect(COEF.cyrLower).toBeLessThan(COEF.cyrUpper);
    expect(COEF.cyrUpper).toBeLessThan(COEF.cyrWide);
    expect(textEm('应用分流')).toBe(4);
    expect(textEm('‌‍‎')).toBe(0); // ZWNJ/ZWJ/LRM 零宽
  });
});

describe('② 几何：受限容器的可用宽必须从 CSS 现场算出（不是写死的常数）', () => {
  it('侧栏 nav 标签可用宽 = side.width − side.padX − navItem.padX − icon − gap', () => {
    // 钉的是**原型基线快照**：这几个数变了不等于错，但必须有人重新过一遍下面的行数预算
    // （NAV_MAX_LINES / NAV_GROUP_MAX_LINES 是按 83px / 110px 标定的）。
    expect(sideW, '.side 宽变了 → 重新标定 NAV_MAX_LINES').toBe(148);
    expect(NAV_LABEL_AVAIL).toBeCloseTo(148 - 20 - 18 - 17 - 10, 5); // 83
    expect(NAV_GROUP_AVAIL).toBeCloseTo(148 - 20 - 18, 5); // 110
  });
  it('托盘行可用宽由 Rust TRAY_WIDTH 推导，更新窗由 POPUP_WIDTH 推导', () => {
    expect(TRAY_WINDOW_W).toBe(268);
    expect(POPUP_W).toBe(380);
    expect(TRAY_ROW_AVAIL).toBeCloseTo(268 - 22 - 12 - 22 - 16 - 10, 5); // 186
    expect(POPUP_ROW_AVAIL).toBeCloseTo(380 - 32, 5); // 348
  });
});

/**
 * 行数预算 —— 换行不是免费的：折成一根柱子的导航项照样是缺陷，只是缺陷换了形状。
 *
 * **预算是「模型口径」的行数**，模型偏宽 ⇒ 真机行数只会更少。逐项实测（13px，可用宽 83px）：
 *  - nav 标签 4 行：最长项 ru `sidebar.appPolicy` = "Политика"(模型 87.9px / DejaVu Bold 实测 72.7px)
 *    + "приложений"(模型 114.4 / 实测 94.8) ⇒ 模型 4 行、**最宽真实字体 DejaVu Sans Bold 3 行**、
 *    Segoe UI / SF Pro 2 行。预算按模型给，因为门手里只有模型这一把尺。
 *    仍然有牙：再长一截（模型 > 4×83 = 332px）就转红。
 *  - 组头 2 行：最长项 ru `sidebar.group.routing` = "МАРШРУТИЗАЦИЯ" 模型 144px / 可用 110px = 2 行。
 */
const NAV_MAX_LINES = 4;
const NAV_GROUP_MAX_LINES = 2;

const NAV_LABEL_SELS = ['.nav-item', '.side .nav-item > span:not(.cnt)'];
const NAV_GROUP_SELS = ['.nav-group', '.side .nav-group'];

const navBox = (where: string): Box => ({
  where,
  avail: NAV_LABEL_AVAIL,
  type: { fontSize: navFont },
  wrap: wraps(NAV_LABEL_SELS),
  breakAnywhere: breaksAnywhere(NAV_LABEL_SELS),
  maxLines: wraps(NAV_LABEL_SELS) ? NAV_MAX_LINES : 1,
});

describe('③ 主窗侧栏（主 + 设置）：nav 标签与组头必须装得下', () => {
  it('主侧栏 nav 标签 × 5 语种', () => {
    const keys = sidebarNavKeys();
    expect(keys.length).toBeGreaterThanOrEqual(8);
    const box = navBox('S1 主侧栏');
    const over: Over[] = [];
    for (const loc of LOCALES)
      for (const k of keys) {
        const text = DICT[loc][k];
        if (!text) throw new Error(`${loc} 缺键 ${k}（locale-parity 该先红）`);
        check(over, box, loc, k, text);
      }
    expect(over.length, `主侧栏 nav 标签溢出：\n${fmt(over)}`).toBe(0);
  });

  it('设置侧栏 nav 标签 × 5 语种', () => {
    const keys = settingsNavKeys();
    expect(keys.length).toBeGreaterThanOrEqual(7);
    const box = navBox('S2 设置侧栏');
    const over: Over[] = [];
    for (const loc of LOCALES)
      for (const k of keys) {
        const text = DICT[loc][k];
        if (!text) throw new Error(`${loc} 缺键 ${k}`);
        check(over, box, loc, k, text);
      }
    expect(over.length, `设置侧栏 nav 标签溢出：\n${fmt(over)}`).toBe(0);
  });

  it('两侧栏组头 .nav-group × 5 语种（10px + .1em 字距 + uppercase）', () => {
    const keys = navGroupKeys();
    expect(keys.length).toBeGreaterThanOrEqual(4);
    const wrap = wraps(NAV_GROUP_SELS);
    const box: Box = {
      where: 'S3 组头',
      avail: NAV_GROUP_AVAIL,
      type: { fontSize: grpFont, letterSpacingEm: grpLs, uppercase: grpUpper },
      wrap,
      breakAnywhere: breaksAnywhere(NAV_GROUP_SELS),
      maxLines: wrap ? NAV_GROUP_MAX_LINES : 1,
    };
    const over: Over[] = [];
    for (const loc of LOCALES)
      for (const k of keys) {
        const text = DICT[loc][k];
        if (!text) throw new Error(`${loc} 缺键 ${k}`);
        check(over, box, loc, k, text);
      }
    expect(over.length, `nav-group 组头溢出：\n${fmt(over)}`).toBe(0);
  });

  it('修法必须仍在位：两处标签都得允许折行 + 允许词内断（否则回归成「画到侧栏外」）', () => {
    expect(wraps(NAV_LABEL_SELS), 'nav 标签又变回 nowrap 了').toBe(true);
    expect(breaksAnywhere(NAV_LABEL_SELS), 'nav 标签缺 overflow-wrap ⇒ 超宽单词仍会溢出').toBe(true);
    expect(wraps(NAV_GROUP_SELS), 'nav-group 又变回 nowrap 了').toBe(true);
    expect(breaksAnywhere(NAV_GROUP_SELS), 'nav-group 缺 overflow-wrap').toBe(true);
  });
});

describe('④ 主窗连接表：colgroup 定宽且长表头有完整 tooltip 兜底', () => {
  it('定宽列表头 × 5 语种', () => {
    const cols = connTableFixedCols();
    expect(cols.length).toBeGreaterThanOrEqual(4);
    const sels = ['.conn-table th, .conn-table td', '.conn-table th'];
    const wrap = wraps(sels);
    const over: Over[] = [];
    let gated = 0;
    for (const { key, cls } of cols) {
      const cap = colCapPx(cls);
      if (cap === null) continue;
      gated++;
      const box: Box = {
        where: `S4 ${cls}(${cap}px)`,
        avail: cap - thPadX - SORT_AR_W,
        type: { fontSize: thFont, letterSpacingEm: thLs, uppercase: thUpper },
        wrap,
        breakAnywhere: breaksAnywhere(sels),
        maxLines: wrap ? 2 : 1,
      };
      for (const loc of LOCALES) {
        const text = DICT[loc][key];
        if (!text) throw new Error(`${loc} 缺键 ${key}`);
        check(over, box, loc, key, text);
      }
    }
    expect(gated, '一个 colgroup 定宽列都没识别到 —— 连接表列宽声明被删了？').toBeGreaterThanOrEqual(4);
    expect(
      decl('./prototype.css', '.conn-table th.sortable > span:first-child', 'text-overflow'),
      `部分语种表头超过紧凑列宽，必须由省略号承接：\n${fmt(over)}`,
    ).toBe('ellipsis');
    expect(
      src('../components/screens/connections/ConnectionsScreen.tsx'),
      '省略后的表头必须能悬停查看完整文案',
    ).toContain('data-tip={label}');
  });
});

/**
 * 托盘键 → 它实际渲染在哪个受限容器。
 *
 * 为什么要这张表：同一批键渲染在**四种**不同几何的槽里（12.5px 菜单行 / 10px uppercase 组头 /
 * 10.5px nowrap 副标题 / 11px 提示条）。旧版本把所有标签一律按菜单行量，对组头与提示条是**误报口径**，
 * 对副标题又是**漏报口径**（副标题字号更小但 nowrap 单行，约束方向完全不同）。
 *
 * 未登记的键落 `ROW`（最保守的那档：带右侧勾/箭头时只剩 161px）。下面那条穷尽性断言保证
 * **新增 `tray.*` 键必须在这里表态**——否则转红，不会有键悄悄溜进「反正默认能过」的缝里。
 */
const TRAY_SLOT: Record<
  string,
  'ROW' | 'GROUP_H' | 'STATUS_TITLE' | 'STATUS_SUB' | 'NOTE' | 'BADGE'
> = {
  // `.tray-group-h`（10px + .06em 字距 + uppercase）
  'tray.nodesRecent': 'GROUP_H',
  'tray.nodes': 'GROUP_H',
  'tray.groupMode': 'GROUP_H',
  'tray.groupTakeover': 'GROUP_H',
  // `.tray-status b`（12.5px，与副标题同槽宽，可折行）
  'tray.statusConnected': 'STATUS_TITLE',
  'tray.statusProxyInactive': 'STATUS_TITLE',
  'tray.statusConnecting': 'STATUS_TITLE',
  'tray.statusError': 'STATUS_TITLE',
  'tray.statusDisconnected': 'STATUS_TITLE',
  // `.tray-status div`（10.5px nowrap+ellipsis）—— `nodeName` 那一格的三个非用户数据取值。
  // `modeDirect` / `blocked` 同时也是菜单行，两个槽都要过（见下方 slotsOf）。
  'tray.noNode': 'STATUS_SUB',
  // `.tray-note`（11px，break-word，高度自适应）
  'tray.fakeIpAutoEnabled': 'NOTE',
  'tray.noTestableNodes': 'NOTE',
  'tray.checkingUpdate': 'NOTE',
  // 检查结果不再占 tray-note 新行，而是检查更新按钮右侧的固定宽短徽标。
  'tray.upToDate': 'BADGE',
  'tray.updateCheckFailed': 'BADGE',
  // W14 动作失败回执（`t(key, vars)` 插值形态）：量的是模板串；真实 detail 可变长，
  // NOTE 槽 6 行预算 + `tray_resize` 的 [80,720] 夹取共同兜住极端长错误串。
  'tray.actionFailed': 'NOTE',
  // 只作为上述模板的 detail 插值，不会单独渲染成固定高度菜单行。
  'tray.actionFailedDetail': 'NOTE',
  'tray.switchPending': 'NOTE',
  'tray.switchSavedForNextStart': 'NOTE',
  'tray.switchRequiresApply': 'NOTE',
};
/** 两栖键：既是菜单行、又会出现在状态卡副标题里。 */
const TRAY_DUAL_SLOT = new Set(['tray.modeDirect', 'tray.blocked']);

describe('⑤ 托盘浮层（独立 webview，五语种）', () => {
  const rowSels = ['.tray-i', '.tray-menu .tray-i'];
  const subSels = ['.tray-status div'];
  const grpSels = ['.tray-group-h', '.tray-menu .tray-group-h'];
  const noteSels = ['.tray-note', '.tray-menu .tray-note'];
  const rowWrap = wraps(rowSels);
  const BOXES: Record<string, Box> = {
    ROW: {
      where: 'S6 托盘行',
      avail: TRAY_ROW_TRAILING_AVAIL,
      type: { fontSize: trayFont },
      wrap: rowWrap,
      breakAnywhere: breaksAnywhere(rowSels),
      maxLines: rowWrap ? 2 : 1,
    },
    GROUP_H: {
      where: 'S9 托盘组头',
      avail: TRAY_GROUP_AVAIL,
      type: { fontSize: trayGrpFont, letterSpacingEm: trayGrpLs, uppercase: trayGrpUpper },
      wrap: wraps(grpSels),
      breakAnywhere: breaksAnywhere(grpSels),
      maxLines: wraps(grpSels) ? 2 : 1,
    },
    STATUS_TITLE: {
      where: 'S7b 托盘状态标题',
      avail: TRAY_STATUS_SUB_AVAIL,
      type: { fontSize: trayStTitleFont },
      wrap: true,
      breakAnywhere: false,
      maxLines: 2,
    },
    STATUS_SUB: {
      where: 'S7 托盘状态副标题',
      avail: TRAY_STATUS_SUB_AVAIL,
      type: { fontSize: trayStSubFont },
      wrap: wraps(subSels),
      breakAnywhere: breaksAnywhere(subSels),
      maxLines: 1,
    },
    NOTE: {
      where: 'S10 托盘提示条',
      avail: TRAY_NOTE_AVAIL,
      type: { fontSize: trayNoteFont },
      wrap: wraps(noteSels),
      breakAnywhere: breaksAnywhere(noteSels),
      /*
       * 提示条**横向不可能溢出**（`word-break:break-word`），故这里钉的是**纵向行数**。
       *
       * 预算 6 怎么来的（不是「调到能过为止」）：浮层窗高由前端量完回报、Rust 侧
       * `tray_resize` 夹在 **[80, 720]**（`src-tauri/src/tray.rs`），建窗初始高 420。
       * ⇒ 提示条最多可用 ≈ 720 − 420 = 300px；11px 字号 × 1.45 行高 ≈ 16px/行 ⇒ 物理上限 ≈ 18 行。
       * 6 行 ≈ 96px，远在上限内，同时**卡在当前最长译文那一格**（ru 模型口径 6 行、fa 5 行、
       * en-US 4 行、zh 2 行）⇒ 再长一句就转红，仍是棘轮。
       *
       * 注意本条是 2026-07-31 **新增**的覆盖：旧版本这条提示压根没进门（旧解析器只认单行
       * `t('zh','en')`，而 FakeIP 这条是跨行写的），不是「把 4 放宽成 6」。
       */
      maxLines: 6,
    },
    BADGE: {
      where: 'S10b 托盘检查更新结果徽标',
      avail: TRAY_UPDATE_RESULT_AVAIL,
      type: { fontSize: trayUpdateResultFont },
      wrap: false,
      breakAnywhere: false,
      maxLines: 1,
    },
  };
  const slotsOf = (key: string): Box[] =>
    TRAY_DUAL_SLOT.has(key)
      ? [BOXES.ROW, BOXES.STATUS_SUB]
      : [BOXES[TRAY_SLOT[key] ?? 'ROW']];

  it('每个 tray.* 键都被消费、且都在槽位表里表过态（防新键从缝里溜走）', () => {
    const used = trayKeys();
    expect(used.length, '托盘键解析数量异常偏低 —— 渲染方式变了？').toBeGreaterThanOrEqual(30);
    const declared = Object.keys(DICT['en-US'])
      .filter((k) => k.startsWith('tray.'))
      .sort();
    // 双向对差：locale 里有、代码没消费 = 死键；代码用了、locale 没有 = 漏译（locale-parity 也会红）。
    expect([...used].sort(), 'tray.* 键集与 locale 不一致').toEqual(declared);
    const unslotted = declared.filter(
      (k) => !(k in TRAY_SLOT) && !TRAY_DUAL_SLOT.has(k),
    );
    // 未登记的走 ROW 默认档是允许的，但必须是**有意**的：这里只断言默认档确实量过（下面那条 it 覆盖）。
    expect(unslotted.every((k) => slotsOf(k).length > 0)).toBe(true);
  });

  it('全部 tray.* 文案 × 5 语种 × 各自的槽必须装得下', () => {
    const over: Over[] = [];
    for (const key of trayKeys())
      for (const loc of LOCALES) {
        const text = DICT[loc][key];
        if (!text) throw new Error(`${loc} 缺键 ${key}（locale-parity 该先红）`);
        for (const box of slotsOf(key)) check(over, box, loc, key, text);
      }
    expect(over.length, `托盘文案溢出：\n${fmt(over)}`).toBe(0);
  });

  it('托盘状态副标题的 ellipsis 是刻意的（长节点名走截断而非画到卡外）', () => {
    expect(clips(subSels), '.tray-status div 的 ellipsis 没了 ⇒ 长文案会画到卡外').toBe(true);
  });
});

/**
 * ⑦ 首页右列 seg2 —— 「基类无约束、变体有约束」那一类（见文件头「S9 是一类形态」）。
 *
 * 右列扩为 3:2 中的两份，两组三档使用紧凑内边距；不再按语种改变排版形态。
 * 加语种、改翻译或改几何链都会重新校验默认窗口是否装得下。
 */
describe('⑦ 首页右列 seg2：五语种在默认窗口恒横排', () => {
  const groups = homeSegGroups();
  /** 各语种两组控件中较宽一组需要的轨道外宽。 */
  const needOf = (loc: Locale) =>
    Math.max(
      ...groups.map(({ keys }) =>
        trackNeededFor(
          keys.map((k) => {
            const t = DICT[loc][k];
            if (!t) throw new Error(`${loc} 缺键 ${k}（locale-parity 该先红）`);
            return t;
          }),
        ),
      ),
    );

  it('几何链必须从 CSS / tauri.conf.json 现场解出（这些数变了，下面的阈值全部要重推）', () => {
    expect(groups.length).toBe(2);
    expect(groups.every((g) => g.keys.length === 3)).toBe(true);
    expect(TAURI_MIN_WIDTH, 'tauri.conf.json 的 minWidth 变了 → 重新校验首页横排几何').toBe(980);
    expect(CONTAINER_MIN).toBe(980 - 148); // 832
    expect(CC_RIGHT_SHARE).toBeCloseTo(2 / 5, 6); // 2 / (3+2)
    expect(screenPadX + cardBorderX + connCardPadX).toBe(48 + 2 + 32); // 82
    expect(trackFromContainer(CONTAINER_MIN)).toBeCloseTo(280, 2);
  });

  it('横排前提仍在位：轨道 100%、三档弹性分配、首页紧凑内边距、nowrap 且无截断', () => {
    expect(dupAgreed('.seg-wrap .seg2', 'width')).toBe('100%');
    // `flex: 1` 逐**分量**判：共享解析器把简写折成 grow/shrink/basis ⇒ 后面补一句
    // `flex-basis: 200px` 也算数（读简写原文的旧写法看不见它）。
    expect(dupAgreed('.seg-wrap .seg2 button', 'flex-grow')).toBe('1');
    expect(dupAgreed('.seg-wrap .seg2 button', 'flex-shrink')).toBe('1');
    expect(dupAgreed('.seg-wrap .seg2 button', 'flex-basis')).toBe('0%');
    expect(dupAgreed('.cc-col.right .seg-wrap .seg2 button', 'padding')).toBe('6px 4px 6px 4px');
    expect(wraps(['.seg2 button', '.seg-wrap .seg2 button']), '.seg2 button 不再是 nowrap').toBe(false);
    expect(clips(['.seg2 button', '.seg-wrap .seg2 button']), '.seg2 button 有了 ellipsis').toBe(false);
  });

  it('接管方式与分流策略 × 5 语种在默认窗口全部装得下', () => {
    const available = trackFromContainer(CONTAINER_MIN);
    const over: string[] = [];
    for (const loc of LOCALES) {
      const need = needOf(loc);
      if (need > available)
        over.push(`  ${loc}：需 ${need.toFixed(1)}px / 可用 ${available.toFixed(1)}px`);
    }
    expect(over.length, `首页三档控件横排溢出：\n${over.join('\n')}`).toBe(0);
  });

});

/**
 * 更新弹窗按钮行的行数预算 —— 为什么是 2。
 *
 * 窗高由 Rust `popup_height_for` 按 phase 定死（remind 184 / error 152，1:1 对齐上游，不动）。
 * remind 卡内纵向账（`style.css` 现值）：padding 14×2 + 标题 14×1.5 + 副标题 12×1.5 + 三道 gap 8
 * + 按钮行 (13×1.5 + 6×2 + 1×2) ≈ 124.5px ⇒ 184 − 124.5 = **59.5px 余量**。
 * 一行按钮 = 33.5px；第二行再要 33.5 + 8(gap) = 41.5px < 59.5 ⇒ 装得下。
 * **第三行**再要 41.5px，累计 83px > 59.5px ⇒ 会被 `body{overflow:hidden}` 裁掉，用户点不到按钮。
 * 故 2 行是硬预算，不是拍脑袋。
 *
 * ⚠️ **本段原先的算式是错的（2026-08-05 订正）**：它写「第二行的 41.5px 由 `.notes` 让出（
 * `min-height:0` + 截断区）」，并为此在下面立了一条断言 `.notes` 的 `min-height` 的门。
 * 但 `.notes` **从来没有被渲染过** —— `UpdatePopupState.notes` 自建档（`2076e86`）起从未被任何
 * 代码赋值（全历史 `notes: Some` 与 `.notes =` 两种形式均零命中），且带 `skip_serializing_if`
 * ⇒ 它压根没进过 DOM。那条「让位」机制一次都没运行过，那道门守的是一个不存在的元素。
 * 真实情况反而更宽松：`.notes` 不占位 ⇒ 余量是完整的 59.5px，第二行本来就装得下。
 * 该字段与它的 CSS 已随 #311 一并删除，本段改为直接对着**窗高减内容**这个真账记。
 */
const POPUP_ROW_MAX_LINES = 2;

describe('⑥ 更新弹窗（380px 独立窗，五语种，`.row` 可换行）', () => {
  it('`.row` 必须允许换行（否则俄语按钮直接画到窗外）', () => {
    // 这是上面那笔容器修法的锁：`flex-wrap` 被删回 nowrap 时本条转红。
    expect(popupDecl('.row', 'flex-wrap')).toBe('wrap');
    // 原先这里还断言 `.notes{min-height:0}`「为换行让位」——已删：那个元素从不渲染，
    // 门守的是不存在的东西（理由见上方 POPUP_ROW_MAX_LINES 的订正段）。
    // 2 行预算的**真**门在下面那条按宽度算行数的用例，不在这里。
  });

  it('单个按钮不得超过卡片内宽（换行也救不了的那种撑破）', () => {
    const bad: string[] = [];
    for (const { phase, keys } of updatePopupButtonRows())
      for (const key of keys)
        for (const loc of LOCALES) {
          const text = DICT[loc][key];
          if (!text) throw new Error(`${loc} 缺键 ${key}`);
          const w = textPx(text, { fontSize: popupFont }) + popupBtnPadX + popupBtnBorder;
          if (w > POPUP_ROW_AVAIL)
            bad.push(`  ${phase} ${loc} ${key} "${text}" 需 ${w.toFixed(1)}px / 可用 ${POPUP_ROW_AVAIL.toFixed(1)}px`);
        }
    expect(bad.length, `更新弹窗单个按钮撑破卡宽：\n${bad.join('\n')}`).toBe(0);
  });

  it(`同一 .row 的按钮换行后不得超过 ${POPUP_ROW_MAX_LINES} 行（第 3 行会被固定窗高裁掉）`, () => {
    const rows = updatePopupButtonRows();
    expect(rows.length, '一个按钮行都没解析到 —— render() 的渲染方式变了？').toBeGreaterThanOrEqual(3);
    expect(
      rows.reduce((n, r) => n + r.keys.length, 0),
      '按钮键解析数量异常偏低',
    ).toBeGreaterThanOrEqual(8);
    const bad: string[] = [];
    for (const { phase, keys } of rows) {
      if (keys.length === 0) continue;
      for (const loc of LOCALES) {
        const ws = keys.map(
          (k) => textPx(DICT[loc][k], { fontSize: popupFont }) + popupBtnPadX + popupBtnBorder,
        );
        // flex-wrap 的贪心装箱：装不下就起新行。
        let lines = 1;
        let cur = 0;
        for (const w of ws) {
          const next = cur === 0 ? w : cur + popupRowGap + w;
          if (next > POPUP_ROW_AVAIL) {
            lines++;
            cur = w;
          } else cur = next;
        }
        if (lines > POPUP_ROW_MAX_LINES)
          bad.push(
            `  ${phase} ${loc} [${keys.map((k) => DICT[loc][k]).join(' / ')}] ${lines} 行 / 预算 ${POPUP_ROW_MAX_LINES}`,
          );
      }
    }
    expect(bad.length, `更新弹窗按钮行数超预算：\n${bad.join('\n')}`).toBe(0);
  });
});

/**
 * ⑧ 节点弹窗表单字段（S11）—— `0b0c186` 铺进来的 95 个 `node.field.*` 键 × 5 语种 = 475 条译文；
 * 2026-08-07 拆标签又添 4 条 hint（`h2Host/h2Method/h2Headers/secretKeys` 的括号说明搬出标签），
 * 现为 99 键 × 5 = 495 条。
 *
 * 为什么之前不在门里：这批文案 2026-08-06 之前**根本不存在**（`node-spec.ts` 走
 * `t(key, zhDefault)`，locale 里只有 `noParrot`/`noParrotHint` 两条，其余 93 键在任何语种下都回落
 * 到同一句中文）—— 中文短，量不量都绿。补齐五语之后最长的一条 ru hint 已到 240 字符，
 * 「装不装得下」这才第一次成为一个真问题，而文件头「覆盖了什么」那张表里从来没有 `.fld` 一族
 * （e. 条列的是 `.app-pol`/`.ctx-note`/`.lock-field`/`.node-menu` 那批，**没提过节点弹窗**）
 * ⇒ 它不是「判过不进门」，是**一直没被看见**。
 */
describe('⑧ 节点弹窗表单字段（统一 540px 录入宽度，五语种，标签 + 统一信息提示）', () => {
  const labelWrap = wraps(FLD_LABEL_SELS);
  const BOXES: Record<FldSlot, (section: 'cred' | 'adv') => Box> = {
    LABEL: (section) => ({
      where: `S11 ${section} 字段标签`,
      avail: FLD_AVAIL,
      type: { fontSize: fldLabelFont },
      wrap: labelWrap,
      breakAnywhere: breaksAnywhere(FLD_LABEL_SELS),
      maxLines: labelWrap ? FLD_MAX_LINES.LABEL : 1,
    }),
    SWT_LABEL: (section) => ({
      where: `S11 ${section} 开关标签`,
      avail: swtTextAvail(FLD_AVAIL),
      type: { fontSize: swtLabelFont },
      wrap: labelWrap,
      breakAnywhere: breaksAnywhere(FLD_LABEL_SELS),
      maxLines: labelWrap ? FLD_MAX_LINES.SWT_LABEL : 1,
    }),
    TIP: (section) => ({
      where: `S11 ${section} 开关提示`,
      avail: tipAvail,
      type: { fontSize: tipFont },
      wrap: true,
      breakAnywhere: false,
      maxLines: FLD_MAX_LINES.TIP,
    }),
  };

  it('几何链必须从 CSS / tauri.conf.json 现场解出（这些数变了，下面的行数预算全部要重推）', () => {
    expect(ENTRY_DLG_W, '统一录入弹窗宽度变了 → 重新标定 FLD_MAX_LINES').toBe(540);
    expect(FLD_AVAIL).toBeCloseTo(540 - 2 - 36, 5); // 502
    expect(swtTextAvail(FLD_AVAIL)).toBeCloseTo(502 - 12 - 36, 5); // 454
    expect([fldLabelFont, swtLabelFont, fldOptFont, tipFont]).toEqual([11.5, 12.5, 10, 11.5]);
    expect(tipAvail).toBeCloseTo(260, 5);
    // `.fld-opt` 在 prototype.css 另存一份同名规则（:933），两份必须同值 —— 本仓经典坑。
    expect(px(decl('./prototype.css', '.fld-opt', 'font-size'))).toBe(fldOptFont);
  });

  it('修法前提仍在位：可折行、无 ellipsis、无 overflow-wrap ⇒ 两条判据都必须活着', () => {
    expect(labelWrap, '.fld-l / .swt-tx b 变成了 nowrap ⇒ 判据要改回「单行宽度」那一类').toBe(true);
    expect(clips(FLD_LABEL_SELS), '字段标签有了 ellipsis —— 标签被截断正是这类控件最要避免的事').toBe(
      false,
    );
  });

  it('「可选」徽标只按 locale → en-US 回落，不在源码保留文案兜底', () => {
    const en = DICT['en-US']['common.optional'];
    expect(en, 'en-US 丢了 common.optional ⇒ 下面整条回落链的前提没了').toBeTruthy();
    expect(optTailOf(undefined, en), '缺 locale 时必须回落到 en-US').toBe(
      en,
    );
    expect(() => optTailOf(undefined, undefined), '整条语言链缺键时必须显式失败，不能退回源码硬编码').toThrow(
      'common.optional',
    );
  });

  it('每个 node.field.* 键都被 ND_SPEC 消费、也都被本门量过（双向对差，防死键与漏测）', () => {
    const points = nodeFieldPoints();
    expect(points.length, 'ND_SPEC 摊出来的测点异常偏低 —— 表结构变了？').toBeGreaterThanOrEqual(90);
    const used = [...new Set(points.map((p) => p.key))].sort();
    const declared = Object.keys(DICT['en-US'])
      .filter((k) => k.startsWith('node.field.'))
      .sort();
    // locale 里有、ND_SPEC 没消费 = 死键；ND_SPEC 用了、locale 没有 = 漏译（i18n 门也该先红）。
    expect(used, 'node.field.* 键集与 locale 对不上').toEqual(declared);
    expect(declared.length).toBeGreaterThanOrEqual(99);
  });

  it('全部 node.field.* 文案 × 5 语种 × 各自的槽必须装得下', () => {
    const over: Over[] = [];
    let n = 0;
    for (const p of nodeFieldPoints()) {
      const box = BOXES[p.slot](p.section);
      for (const loc of LOCALES) {
        const text = DICT[loc][p.key];
        if (!text) throw new Error(`${loc} 缺键 ${p.key}（locale-parity 该先红）`);
        n++;
        check(over, { ...box, tailPx: p.opt ? optTailPx(loc) : 0 }, loc, p.key, text);
      }
    }
    expect(n, '测点总数异常偏低 —— 语种或字段少了一批？').toBeGreaterThanOrEqual(495);
    expect(over.length, `节点弹窗表单字段溢出：\n${fmt(over)}`).toBe(0);
  });

  /**
   * 阳性对照 —— 一道加进来却永远不会红的门比没有更坏。这里对**两条判据各造一个已知缺陷**，
   * 断言 `check()` 都抓得到，且报文里带得出槽/键/超出量。用合成串而不是改真译文：
   * 改真译文的对照做完就得还原，还原漏了就变成静默篡改语料。
   */
  it('阳性对照：两条判据各造一个已知缺陷，门必须都抓到', () => {
    const box = BOXES.TIP('adv');
    const bucket: Over[] = [];

    // ① 硬判据：一个比字段还宽的**不可断**单元（长 token / URL 那一类）。
    const longToken = 'X'.repeat(60);
    check(bucket, box, 'ru', 'node.field.__probeWidth', longToken);
    // ② 棘轮判据：可正常折行、但行数超预算。
    const longSentence = 'слово '.repeat(400);
    check(bucket, box, 'ru', 'node.field.__probeLines', longSentence);

    expect(bucket.length, '合成的两个已知缺陷竟然没被抓到 —— 本门是装饰').toBe(2);
    const [w, l] = bucket;
    expect(w.key).toBe('node.field.__probeWidth');
    expect(w.need).toBeGreaterThan(box.avail); // 最宽行确实超了可用宽
    expect(l.key).toBe('node.field.__probeLines');
    expect(l.lines).toBeGreaterThan(FLD_MAX_LINES.TIP);
    // 报文必须指名槽 / 语种 / 键 / 超出量，否则红了也没法定位。
    const msg = fmt(bucket);
    expect(msg).toMatch(/S11 adv 开关提示/);
    expect(msg).toMatch(/\| ru \|/);
    expect(msg).toMatch(/node\.field\.__probeWidth/);
    expect(msg).toMatch(/最宽行 [\d.]+px \/ 可用 260\.0px/);
    expect(msg).toMatch(/\d+ 行 \/ 预算 12 行/);
  });

  /**
   * 阴性对照 —— 模型必须**认识 CJK 断点**，否则一整句无空格中文会被当成一个不可断的词、
   * 在任何窄容器里都判红（假红）。这是 2026-08-06 补 `breakUnits()` 的那个缺陷的定桩。
   */
  it('阴性对照：无空格中文长句必须能折行，不得被算成「不可断长串」', () => {
    const source = DICT['zh-TW']['node.field.muxPadHint'];
    expect(source).toContain('隨機填充'); // 语料换了就该重新对照，不许静默跟着变
    const zh = source.repeat(2); // 合成长句，确保在 260px tooltip 里覆盖折行腿
    const box = BOXES.TIP('adv');
    const { lines, maxLineWidth } = layout(zh, box.avail, box.type, box);
    expect(lines, '中文长句在 260px tooltip 里仍应折行').toBeGreaterThan(1);
    expect(
      maxLineWidth,
      `整句被当成一个不可断的词了（${maxLineWidth.toFixed(1)}px > ${box.avail}px）—— breakUnits 的 CJK 断点没生效`,
    ).toBeLessThanOrEqual(box.avail);
  });
});

// ── ⑨ 导入弹窗解析结果预览（`.imp-*`）：几何链 ────────────────────────────────────────────
//
// `.imp-preview` 是 `.dlg-body` 的直接子项 ⇒ 与 `.fld` 同一个顶层槽（422px）。
// 它自己只有 `border-top` 与 `padding-top`，横向不再收窄。
const impStatFont = px(decl('./index.css', '.imp-stat', 'font-size')); // 10.5
const impStatPadX = padX(decl('./index.css', '.imp-stat', 'padding')); // 7*2 = 14
/** 计数 pill 的文本可用宽 = 422 − 7×2。`.imp-stats{flex-wrap:wrap}` ⇒ 一个 pill 最宽就是整幅。 */
const IMP_STAT_AVAIL = BASE_FLD_AVAIL - impStatPadX;

const impLiPadX = padX(decl('./index.css', '.imp-list > li', 'padding')); // 9*2 = 18
const impLiGap = px(decl('./index.css', '.imp-list > li', 'column-gap')); // 8
/** 清单行的内容宽 = 422 − 9×2。（`.imp-list` 只有 1px 边框，忽略不计入会让判据更严，故计入。） */
const impListBorderX = px(decl('./index.css', '.imp-list', 'border-left-width')) * 2; // 1*2
const IMP_ROW_AVAIL = BASE_FLD_AVAIL - impListBorderX - impLiPadX;

const impBadgeFont = px(decl('./index.css', '.imp-badge', 'font-size')); // 10
const impBadgePadX = padX(decl('./index.css', '.imp-badge', 'padding')); // 6*2 = 12

const cardSubFont = px(decl('./components.css', '.card-sub', 'font-size')); // 11.5

/**
 * 名称轨的最小宽 —— **棘轮，钉在今天最差的那一格上**，不是拍出来的设计余量。
 *
 * `.imp-name` 带 ellipsis，徽标再宽也只会把名字截得更短、不会破版；所以这里没有物理墙可解，
 * 只有「够不够认出是哪条节点」这个可用性问题。既然没有物理墙，就不能拍一个宽松的数
 * —— 那样门永远不会红（第一版拍了 180，把 ru 的徽标换成整句 `Протокол не поддерживается`
 * 仍留 262px，照样全绿，等于没这道门）。
 *
 * 故取今天五语里最差的一格：ru `Не поддерживается` 占 154.6px ⇒ 名称轨剩 **239.4px**。
 * 钉在 239 = 「徽标再宽一格就自曝」，与 `FLD_MAX_LINES` 同一套语义。
 * 真要放宽，先回答「为什么这一行的主体可以更窄」，再动这个数。
 */
const IMP_NAME_MIN = 239;

// ════════════════════════════════════════════════════════════════════════════════════════

/**
 * ⑨ 导入弹窗的解析结果预览。
 *
 * # 为什么补这一节
 *
 * 这块界面（`feat(import)` 那批）落地时**没进本门的注册表** —— 而「注册表里没有 = 门看不见」
 * 正是本文件开头列的那类假绿：`.fld` 一族当初也是这么漏了整整一族的（S11 的由来）。
 * 五个语种的宽度一次都没被量过。
 *
 * # 量什么、不量什么
 *
 *  · `.imp-stat` 三颗计数 pill（已解析 / 不支持 / 已跳过）—— 整幅宽，可折行；
 *  · `.card-sub` 那行「将导入节点」小标题 —— 整幅宽，可折行；
 *  · `.imp-badge`「不支持」徽标 —— `flex:none` ⇒ 基础尺寸取 max-content 且**不收缩**，恒单行；
 *    它不会溢出（名称轨带 ellipsis 会替它让位），故判据换成「别把名称轨压到看不清」。
 *
 * **不量** `.imp-warn` 里的告警条目：那是后端 `ClashParseResult` 生成的运行期文本，
 * 不是 locale 里的键，本门的语料源里没有它。（它有 `line-height:1.55` 且可折行，纵向无上限。）
 * **不量** `.imp-name` / `.imp-proto`：前者是用户数据 + ellipsis，后者是协议标识符、非文案。
 *
 * # 插值按模板长度测（比实际更宽，故保守）
 *
 * 与本文件既有口径一致（射程自曝 b）。这里额外成立的是：`{{count}}` 这个模板串本身
 * 就比它会被替换成的三四位数字更宽（10.5px 下模板约 40px、`9999` 约 24px），
 * 故「按模板测」对这三条计数文案是**偏严**的一侧，不需要另造样例值。
 */
describe('⑨ 导入弹窗解析结果预览（.imp-*，.dlg 460px 定宽，五语种）', () => {
  /** 槽 → [键, 盒]。键必须真的还被 ImportDialog 消费（见下方自检）。 */
  const IMP_STAT_KEYS = ['import.parsed', 'import.resultUnsupported', 'import.skippedTitle'];
  const IMP_TITLE_KEY = 'import.nodesTitle';
  const IMP_BADGE_KEY = 'import.unsupportedBadge';

  const statWrap = wraps(['.imp-stats', '.imp-stat']);
  const titleWrap = wraps(['.card-sub']);

  const STAT_BOX: Box = {
    where: '⑨ 计数 pill（.imp-stat）',
    avail: IMP_STAT_AVAIL,
    type: { fontSize: impStatFont },
    wrap: statWrap,
    breakAnywhere: breaksAnywhere(['.imp-stats', '.imp-stat']),
    // 棘轮，钉在今天最差：en/ru/fa 的 `resultUnsupported` 与 `skippedTitle` **已经是 2 行**
    // （最差 397.5/408px）。2 不是「pill 应该占两行」的设计主张，是现状；第 3 行必须自曝。
    maxLines: statWrap ? 2 : 1,
  };
  const TITLE_BOX: Box = {
    where: '⑨ 清单小标题（.card-sub）',
    avail: FLD_AVAIL,
    type: { fontSize: cardSubFont },
    wrap: titleWrap,
    breakAnywhere: breaksAnywhere(['.card-sub']),
    // 五语今天全是 1 行（最差 ru 149/422px，余量一倍多）。给 2 行就等于给了一整行的免检额度。
    maxLines: 1,
  };

  it('几何链从 CSS 现场解出，且 `.card-sub` 的两份副本同值（本仓经典坑）', () => {
    expect(IMP_STAT_AVAIL).toBeCloseTo(422 - 14, 5);
    expect(IMP_ROW_AVAIL).toBeCloseTo(422 - 2 - 18, 5);
    expect([impStatFont, impBadgeFont, cardSubFont]).toEqual([10.5, 10, 11.5]);
    expect(px(decl('./prototype.css', '.card-sub', 'font-size')), '.card-sub 两份副本分叉').toBe(
      cardSubFont,
    );
  });

  it('本节量的键仍真的被 ImportDialog 消费（改名/删控件后本节不得继续量死键）', () => {
    const dlg = src('../components/dialogs/ImportDialog.tsx');
    for (const k of [...IMP_STAT_KEYS, IMP_TITLE_KEY, IMP_BADGE_KEY]) {
      expect(dlg.includes(`t('${k}'`), `ImportDialog 已不再消费 ${k} —— 本节的注册表要跟着改`).toBe(
        true,
      );
    }
    // 语料侧同理：键不在 locale 里的话下面 `DICT[loc][key]` 会是 undefined 而静默跳过。
    for (const k of [...IMP_STAT_KEYS, IMP_TITLE_KEY, IMP_BADGE_KEY]) {
      for (const loc of LOCALES) {
        expect(DICT[loc][k], `${loc} 缺键 ${k}（i18n 门该先红）`).toBeTruthy();
      }
    }
  });

  it('计数 pill 与清单小标题 × 5 语种必须装得下', () => {
    const over: Over[] = [];
    let n = 0;
    for (const loc of LOCALES) {
      for (const key of IMP_STAT_KEYS) {
        n++;
        check(over, STAT_BOX, loc, key, DICT[loc][key]);
      }
      n++;
      check(over, TITLE_BOX, loc, IMP_TITLE_KEY, DICT[loc][IMP_TITLE_KEY]);
    }
    expect(n, '测点数异常偏低 —— 语种或键少了？').toBe(LOCALES.length * 4);
    expect(over.length, `导入结果预览溢出：\n${fmt(over)}`).toBe(0);
  });

  it('「不支持」徽标不得把节点名压到看不清（徽标不收缩，名字才是这一行的主体）', () => {
    const tight: string[] = [];
    for (const loc of LOCALES) {
      const badgeW = textPx(DICT[loc][IMP_BADGE_KEY], { fontSize: impBadgeFont }) + impBadgePadX;
      const nameW = IMP_ROW_AVAIL - impLiGap - badgeW;
      if (nameW < IMP_NAME_MIN)
        tight.push(
          `  ${loc} | ${IMP_BADGE_KEY}="${DICT[loc][IMP_BADGE_KEY]}" | 徽标 ${badgeW.toFixed(1)}px` +
            ` ⇒ 名称轨只剩 ${nameW.toFixed(1)}px（下限 ${IMP_NAME_MIN}px）`,
        );
    }
    expect(tight.sort(), '徽标过宽，节点名会被 ellipsis 吃掉大半').toEqual([]);
  });


  /** 阳性对照 —— 两条判据各造一个已知缺陷，门必须都抓到。用合成串，不动真语料。 */
  it('阳性对照：过长的计数文案与过宽的徽标都必须被抓到', () => {
    const bucket: Over[] = [];
    // ① 折行超预算（pill 折成 3 行 = 那一栏在视觉上不再是个「标签」）。
    check(bucket, STAT_BOX, 'ru', 'import.__probeLines', 'слово '.repeat(120));
    // ② 不可断长串超宽。
    check(bucket, STAT_BOX, 'ru', 'import.__probeWidth', 'X'.repeat(200));
    expect(bucket.length, '合成的两个已知缺陷竟然没被抓到 —— 本节是装饰').toBe(2);
    expect(fmt(bucket)).toMatch(/⑨ 计数 pill/);
    expect(fmt(bucket)).toMatch(/import\.__probeWidth/);

    // ③ 徽标那条的阳性对照：把徽标从一个词换成一句话（这正是翻译最容易发生的事），
    //    名称轨必须掉到下限以下。第一版的 180px 下限对这句话仍然全绿，故连这条对照一起收紧。
    const fatBadge =
      textPx('Protocol not supported by the current core', { fontSize: impBadgeFont }) + impBadgePadX;
    expect(IMP_ROW_AVAIL - impLiGap - fatBadge).toBeLessThan(IMP_NAME_MIN);
    // 阴性对照：今天真在用的那个最差值必须**恰好**过关（下限不是随手挑的一个宽松数）。
    const worst = textPx('Не поддерживается', { fontSize: impBadgeFont }) + impBadgePadX;
    const worstName = IMP_ROW_AVAIL - impLiGap - worst;
    expect(worstName).toBeGreaterThanOrEqual(IMP_NAME_MIN);
    expect(worstName - IMP_NAME_MIN, '下限已不再贴着今天最差值 —— 语料变了就把它重新钉一次').toBeLessThan(2);
  });
});

// ── ⑩ 移动端首页的两个受限容器（S13）：几何链 ────────────────────────────────────────────
//
// 取材面见文件顶部 `CSS_FILES` 的注释：移动端是**另一条层叠链**，不是桌面那五份的延长。

const MOBILE_CSS = '../mobile/mobile.css' as const;
const HOME_CSS = '../mobile/home/home.css' as const;
const REDESIGN_CSS = '../mobile/redesign.css' as const;

/**
 * 参考视口 **390 × 844 逻辑像素** —— 本节**唯一**允许的硬编码常量。
 *
 * 出处：`~/docs/polaris/design/mobile-kit/component-specs/mobile-screen-shell.md` 的
 * `## Dimensions`「Reference viewport」。仓内没有这个数：移动端窗口尺寸由 Android/iOS 给，
 * 没有 `tauri.conf.json#minWidth` / Rust `TRAY_WIDTH` 那样可解析的真值源 ⇒ 与 `TRAY_WINDOW_W`
 * 那类「常量住在别处、这里解析它」不同，这一格的真值住在**仓外的设计包**里，只能快照。
 * 快照就得带出处：设计包改了参考视口而这里没跟，下面那批预算会在下一次真机对拍时对不上，
 * 而不是悄悄按一个陈旧宽度继续发绿。
 *
 * 为什么用 390 而不是 2026-09-05 真机那台的 412（1080×2400 @2.625）：390 更窄 ⇒ 每一格都更紧
 * ⇒ 判据更严。412 上实测到的两个截断，在 390 下只会更严重（阳性对照那条按 412 复现原始数字）。
 */
const MOBILE_REF_VIEWPORT_W = 390;

/**
 * 移动端根字号。`html{font-size:calc(16px * var(--font-scale))}` + `:root{--font-scale:1}`
 * ⇒ 1x 下 16px，`rem` 排版按它折算。
 *
 * **Dynamic Type（`--font-scale` > 1）是另一根轴，本节不量**：字随缩放变大而几何（padding /
 * gap / inset）按设计恒定 ⇒ 2x 下每一格都更挤。真值由原生层写进根元素内联样式
 * （`MainActivity.kt`，见 mobile.css 头注），前端拿不到 ⇒ 量它等于量一个假设。登记在下面「射程之外」。
 */
const MOBILE_ROOT_FONT = (() => {
  const v = decl(MOBILE_CSS, 'html', 'font-size');
  const m = v.match(/calc\(\s*([\d.]+)px\s*\*\s*var\(\s*--font-scale\s*\)\s*\)/);
  if (!m)
    throw new Error(`mobile.css 的 \`html{font-size}\` 不再是 \`calc(Npx * var(--font-scale))\`：\`${v}\``);
  const scale = parseFloat(decl(MOBILE_CSS, ':root', '--font-scale'));
  if (!Number.isFinite(scale)) throw new Error('mobile.css 的 `:root{--font-scale}` 读不到数值缺省');
  return parseFloat(m[1]) * scale;
})();
/** `rem` → px（按移动端根字号）。非 rem 值原样交给 `px()`，读不到照样 throw。 */
const remPx = (v: string): number => {
  const m = v.trim().match(/^(-?[\d.]+)rem$/);
  return m ? parseFloat(m[1]) * MOBILE_ROOT_FONT : px(v);
};

/**
 * 外壳的三个响应标量（`mobile.css` 的 `:root`，compact 档）。
 * medium/expanded 的覆盖写在 `@container mscreen(...)` 块里的 `.m-page` 上 —— 那些规则被本文件的
 * 规则切分器切成 `@container … { .m-page` 这样的选择器，逐字不等于 `:root` ⇒ `decl()` 拿到的
 * 恒是 compact 那份，也正是三档里**最窄**的一档。
 */
const shellScalar = (name: string) => px(resolveSp(decl(MOBILE_CSS, ':root', name)));
const pageInset = shellScalar('--page-inset'); // var(--sp-4) = 16
const sectionGap = shellScalar('--section-gap'); // var(--sp-3) = 12
const cardPadding = shellScalar('--card-padding'); // var(--sp-4) = 16

/**
 * 页面内容宽 = 参考视口 − 2×`--page-inset`。
 * `.m-page` 的左右内边距是 `calc(var(--page-inset) + var(--safe-l/r))`，而 `--safe-*` 是
 * `env(safe-area-inset-*, 0px)`：参考视口是竖屏、刘海在**上**，左右安全区为 0。
 * 横屏（刘海在侧）会再吃掉一截 —— 那是另一个视口，不在本节射程。
 */
const M_PAGE_CONTENT = MOBILE_REF_VIEWPORT_W - 2 * pageInset;

const hCardBorderX = px(decl(HOME_CSS, '.h-card', 'border-left-width')) * 2;
/** 卡内容宽 = 页面内容宽 − 卡边框 − 卡内边距（`.h-card` 是 `box-sizing:border-box`）。 */
const H_CARD_INNER = M_PAGE_CONTENT - hCardBorderX - 2 * cardPadding;

// ── ① 节点状态卡 ──────────────────────────────────────────────────────────────────────
// 节点卡已移除重复的全局连接状态，静态标题占用整条卡内宽。
const nodeNameFont = remPx(decl(REDESIGN_CSS, '.h-nodename', 'font-size'));
const nodeNameLs = lsEm(decl(HOME_CSS, '.h-nodename', 'letter-spacing'), nodeNameFont);
const NODE_NAME_TYPE: TypeSpec = { fontSize: nodeNameFont, letterSpacingEm: nodeNameLs };

// The old two-chip budget no longer describes this screen. Check the inline
// three-mode control and full-width node title against the actual CSS instead.
describe('移动端首页：标题与三档路由的五语种宽度', () => {
  const routePad = padX(decl(REDESIGN_CSS, '.h-route-option', 'padding'));
  const routeAvail = 64 - routePad;
  const keys = ['home.routingSmart', 'home.routingGlobal', 'home.routingDirect'];
  it('三档标签在普通与两倍字号下均可完整换行', () => {
    expect(decl(REDESIGN_CSS, '.h-routing', 'flex-wrap')).toBe('wrap');
    expect(decl(HOME_CSS, '.h-route-option', 'overflow-wrap')).toBe('anywhere');
    for (const scale of [1, 2]) for (const locale of LOCALES) for (const key of keys) {
      const result = layout(DICT[locale][key], routeAvail,
        {fontSize: remPx(decl(REDESIGN_CSS, '.h-route-option', 'font-size')) * scale},
        {wrap: true, breakAnywhere: true});
      expect(result.maxLineWidth, `${locale}/${key}/${scale}`).toBeLessThanOrEqual(routeAvail);
      expect(result.lines).toBeLessThanOrEqual(scale === 1 ? 3 : 6);
    }
  });
  it('节点的静态标题保留全宽并完整换行', () => {
    for (const scale of [1, 2]) for (const locale of LOCALES)
      for (const key of ['home.routingDirect', 'home.routingBlock', 'home.noServer']) {
        if (!DICT[locale][key]) continue;
        const result = layout(DICT[locale][key], H_CARD_INNER,
          {...NODE_NAME_TYPE, fontSize: nodeNameFont * scale}, {wrap: true, breakAnywhere: true});
        expect(result.maxLineWidth).toBeLessThanOrEqual(H_CARD_INNER);
      }
  });
});

// ════════════════════════════════════════════════════════════════════════════════════════
// S14–S16 移动端另外三个屏（节点 / 规则 / 连接）—— 2026-09-05 补
// ════════════════════════════════════════════════════════════════════════════════════════
//
// # 为什么补这三节
//
// ⑩ 落地时 `CSS_FILES` 里只有 `mobile.css` + `home.css`：五个移动屏中**四个**的 CSS 一份都没进面。
// 「注册表里没有 = 门看不见」这已经是第四次（S11 `.fld` 一族 / ⑨ `.imp-*` / ⑩ 首页 / 这次）。
// 这一次连同 ⓪ 的**取材面自曝**一起补：文件面改成「目录树发现 + 恰等对拍」，
// 下一个屏落地时是它先红，而不是等真机截给人看。
//
// 面一进来就抓到两处真缺陷，两处都当场修了（各自的 CSS 里有就地注释）：
//  · **S15 `.mr-pill`** 是 `white-space: nowrap` + 无 ellipsis + 无 overflow-wrap 的静态角标 ⇒
//    「装不下」的唯一表现就是画到行外。ru 的 `rules.dnsResolverInherit` 在 170px 的槽里要 239.4px。
//  · **S16 `.mc-det-row dt`** 是 `flex: 0 0 5.5em` 的定宽标签列 ⇒ 超宽标签横着盖到右边的值上。
//    模型口径下 50 格里 9 格越界；Android 模拟器（412，Roboto）实测真正越界的是 2 格，都在 ru
//    （`colTime` 80.4px / `colDest` 67.7px，槽 66px），en 的 `Destination` 真机 60.8px 装得下。
//
// # 三节共用的口径（与 ⑩ 一致，不重复论证）
//
//  · 参考视口 `MOBILE_REF_VIEWPORT_W`（390，compact 最窄档）；只量 compact + 竖屏 + `--font-scale:1`。
//  · 几何 px、排版 rem；`rem` 一律经 `remPx()` 按 `MOBILE_ROOT_FONT` 折算。
//  · 所有可用宽**现算**，算式写在每个常量上；读不到即 `throw`（不 `it.skip`）。
//  · 运行期数据（节点名 / 域名 / 资源名 / 应用名 / 速率 / 计数）一律**不测**（文件头「射程之外 a.」）。
//
// # 这三节**没有**覆盖什么（逐条判过，不是没看）
//
//  i.   **横向可滚的分段条**：`.mn-segs`/`.mr-seg`/`.mc-seg` 都是 `overflow-x: auto`，条目
//       `flex: 0 0 auto` + nowrap ⇒ 装不下是**滚动**不是画到框外，与 `.term-row code` 同一档
//       （文件头「射程之外 h.」）。
//  ii.  **动作面板（bottom sheet）条目**：`.mn-sheet-btn` / `.mr-sheet-item` / `.mc-sheet-item`
//       都在一张接近满幅（≈358px）的表里、都可折行、纵向自由生长 ⇒ 槽比本三节量的任何一格都宽，
//       窄的那几档过了它们同构地更松。
//  iii. **`.mr-pill` 在 RuleRow 之外的落点**：资源行（可用 256px）已在下面量；应用行（284px）
//       装的是运行期策略文案，DNS 卡头与折叠标题（≥316px）比资源行更宽。只量最窄的两档。
//  iv.  **`.mc-det-row dd` / `.mc-log-line` / `.mr-row-title` / `.mr-row-sub`**：全部是运行期数据，
//       且都已带 `overflow-wrap: anywhere`。
//  v.   **通告条与常驻说明行**（`.mn-notice-text` / `.mr-note` / `.mc-note` / `.mr-notice-tx`）：
//       静态 i18n、可折行、无宽度上限、纵向自由 ⇒ 只有行数会变，没有「装不下」这件事。
//       它们**没有进面** —— 与 e. 同一句话：不是判过不进门，是判过之后认为棘轮的信息量不抵噪声。
//  vi.  **设置屏**：没有 CSS 文件，几何住在 `SettingsChrome.tsx` 的内联 `CSSProperties` 里。
//       登记在 ⓪，附带那条紧槽（带下拉的行标签只剩 ≈162px）的算式。

/**
 * `tokens.resolved.css` 里的一个 px 令牌。
 *
 * 这份不在 `CSS_FILES` 里（它是**令牌表**不是槽位表，进面会把一大堆与文本无关的规则拖进
 * `ALL` 的「后者胜」里），故这里直接读文件而不是走 `decl()`。读不到即 throw。
 */
const tokenPx = (name: string): number => {
  const m = stripComments(read('./tokens.resolved.css')).match(
    new RegExp(`${name}\\s*:\\s*([\\d.]+)px`),
  );
  if (!m) throw new Error(`tokens.resolved.css 里读不到 \`${name}\` —— 令牌被改名/搬走？`);
  return parseFloat(m[1]);
};

/**
 * 移动端外壳令牌 → px。`--sp-N` 走 `SPACING`；三个响应标量走 `mobile.css` 的 `:root`；
 * `--tap-min`（触控目标下限，真值源在设计包的 `core.json`）走 `tokens.resolved.css`。
 */
const SHELL_VARS: ReadonlyMap<string, number> = new Map([
  ['--page-inset', pageInset],
  ['--section-gap', sectionGap],
  ['--card-padding', cardPadding],
  ['--tap-min', tokenPx('--tap-min')],
  ['--mobile-tap', px(decl(REDESIGN_CSS, ':root', '--mobile-tap'))],
]);
/** 把值里的 `var(--sp-N)` 与外壳标量换成 px 字面量；表里没有的 var 直接 throw（不静默留原样）。 */
const resolveShell = (v: string): string =>
  resolveSp(v).replace(/var\(\s*(--[\w-]+)\s*\)/g, (_, k: string) => {
    const n = SHELL_VARS.get(k);
    if (n === undefined)
      throw new Error(`\`${k}\` 不在移动端外壳标量表里 —— S14–S16 的几何链读到了一个没登记的令牌`);
    return `${n}px`;
  });
/** `border: 1px solid …` → 左右边框之和。 */
const borderX = (file: CssFile, sel: string) => px(decl(file, sel, 'border-left-width')) * 2;

/**
 * 贪心把一排**固定宽**的行内块装进 `width`（gap 计在相邻两项之间），返回行数与最宽行。
 *
 * 与 `layout()` 的区别：那个排的是**字**，这个排的是**块**（按钮 / 芯片 / 链节）。
 * flex + `flex-wrap: wrap` 的换行规则就是这条：项按基础尺寸（`flex-basis:auto` ⇒ max-content）
 * 依次放，放不下就起新行。项本身比整行还宽是另一回事，由各节自己的词级判据管。
 */
function packRow(widths: number[], gap: number, width: number): { lines: number; maxLine: number } {
  let lines = 1;
  let cur = 0;
  let maxLine = 0;
  for (const w of widths) {
    const sep = cur > 0 ? gap : 0;
    if (cur > 0 && cur + sep + w > width) {
      maxLine = Math.max(maxLine, cur);
      lines++;
      cur = 0;
    }
    cur = cur === 0 ? w : cur + sep + w;
  }
  return { lines, maxLine: Math.max(maxLine, cur) };
}

/**
 * 一段 JSX 里出现的 `t('key')` / `tr('key')` 字面量（去重，保持出场顺序）。
 *
 * 键后不限定 `)`：`t('k', {…})` 这类带插值的形态同样要收进来，否则它们会从扫描器的缝里
 * 漏成「死键」假红（与 `trayKeys()` 同一条理由）。
 */
const keysOf = (seg: string, what: string): string[] => [
  ...new Set(grepAll(seg, /\b(?:t|tr)\(\s*'([\w.]+)'/g, what).map((m) => m[1])),
];
/** 从 `src` 里截 `from` → `to` 之间那一段；截不到即 throw。 */
function segment(src: string, from: string, to: string, what: string): string {
  const a = src.indexOf(from);
  const b = a < 0 ? -1 : src.indexOf(to, a + from.length);
  if (a < 0 || b < 0) throw new Error(`截不到 \`${from}\` → \`${to}\` 这一段 —— ${what} 改结构了？`);
  return src.slice(a, b);
}

// ── S14 移动端「节点」屏：几何链 ────────────────────────────────────────────────────────

const NODES_CSS = '../mobile/nodes/nodes.css' as const;
const nodesView = () => src('../mobile/nodes/NodesScreenView.tsx');

/**
 * 页头动作簇（`.mn-head-acts`）与标题同排，统计另起一行。
 *
 * `.mn-top` 用「负外边距把 sticky 块拉到滚动区边沿 + 等量内边距把内容推回原位」，
 * 一进一出净零 ⇒ 页头里的内容宽仍是 `.m-page` 的内容宽。
 *
 * 留给标题 56px 与间距 8px；大字或长译文允许动作换行，文字不得越出点击盒。
 */
const MN_ACTS_TRACK = M_PAGE_CONTENT - 64;
const mnActsGap = px(resolveShell(decl(NODES_CSS, '.mn-head-acts', 'column-gap'))); // 4
const mnActPadX = padX(resolveShell(decl(NODES_CSS, '.mn-act', 'padding'))); // 0 var(--sp-2) → 16
const mnActBorderX = borderX(NODES_CSS, '.mn-act'); // 1*2
const mnActFont = remPx(decl(REDESIGN_CSS, ':root', '--mobile-control-font')); // 0.8125rem = 13
/** 一颗动作按钮的外宽（max-content 口径：`.mn-act` 无 nowrap，但 flex 基础尺寸取 max-content）。 */
const mnActW = (label: string) => textPx(label, { fontSize: mnActFont }) + mnActPadX + mnActBorderX;

/** 批量条：`.mn-batch` 自带内边距与边框，轨道比页头那条窄一截。 */
const mnBatchPadX = padX(resolveShell(decl(NODES_CSS, '.mn-batch', 'padding'))); // var(--sp-2)*2 = 16
const mnBatchBorderX = borderX(NODES_CSS, '.mn-batch');
const mnBatchGap = px(resolveShell(decl(NODES_CSS, '.mn-batch', 'column-gap'))); // var(--sp-2) = 8
const mnBatchCntFont = remPx(decl(NODES_CSS, '.mn-batch-count', 'font-size')); // 0.8125rem = 13
const MN_BATCH_TRACK = M_PAGE_CONTENT - mnBatchBorderX - mnBatchPadX;

/**
 * 列表行主体（`.mn-row-main`）的内容宽。
 *
 * `.mn-list` 提供 1px 外框，连续行没有左右框；当前/批选行多一条 3px 起始边。
 * 按最窄的当前行算，不能让名称或角标在高亮后才溢出。
 */
const mnListBorderX = borderX(NODES_CSS, '.mn-list');
const mnStateInset = 3;
const mnRowGap = px(resolveShell(decl(NODES_CSS, '.mn-row', 'column-gap'))); // var(--sp-1) = 4
const mnRowMoreW = px(resolveShell(decl(REDESIGN_CSS, '.mn-row-more', 'width'))) * 2; // i + more: worst-case two 44px actions
const mnRowMainPadX = px(resolveShell(decl(NODES_CSS, '.mn-row-main', 'padding-left'))) * 2; // horizontal 10px*2
const MN_ROW_MAIN = M_PAGE_CONTENT - mnListBorderX - mnStateInset - mnRowGap - mnRowMoreW - mnRowMainPadX;

/** 行内角标（`.mn-pills` 是 flex-wrap ⇒ 单颗角标最宽可占满整条行主体）。 */
const mnPillPadX = padX(decl(NODES_CSS, '.mn-pill', 'padding')); // 2px 6px → 12
const mnPillBorderX = borderX(NODES_CSS, '.mn-pill');
const mnPillFont = remPx(decl(NODES_CSS, '.mn-pill', 'font-size')); // 0.6875rem = 11
const MN_PILL_AVAIL = MN_ROW_MAIN - mnPillPadX - mnPillBorderX;

/** 页头两颗动作按钮的文案键（新增 + 更多，全部测速在更多动作面）。 */
const mnHeadActKeys = () =>
  keysOf(segment(nodesView(), 'className="mn-head-acts"', '</header>', '页头动作簇'), '页头动作键');
/** 批量条：`[计数前缀, 计数后缀, 四颗按钮]`。首项 `nodes.batch` 是容器 aria-label，不渲染成文字。 */
const mnBatchKeys = () =>
  keysOf(segment(nodesView(), 'className="mn-batch"', '── sheets', '批量条'), '批量条文案键')
    .filter((key) => key !== 'mobileActions.preparing');
/** 行内角标的静态键（协议名与传输摘要是运行期数据，不在这一段里取）。 */
const mnPillKeys = () =>
  keysOf(segment(nodesView(), 'className="mn-pills"', 'mn-note', '列表行角标'), '角标文案键');

/**
 * 行数棘轮（**棘轮，不是墙**）：三处都是 `flex-wrap: wrap` 的横排簇，纵向自由生长
 * （`.m-scroll{overflow-y:auto}`）。数钉在今天最差的那一格，让「再长一句」自曝。
 *
 *  · `MN_HEAD_ACT_MAX_LINES = 1` —— 页头只放两个短动作，大字由容器布局自然回退。
 *  · `MN_BATCH_MAX_LINES = 3` —— 今天最差 ru 三行（en/fa/zh 两行）。
 *  · `MN_PILL_MAX_LINES = 2` —— 关键影响补到短状态，完整原因移进 i，最长译文自然两行。
 */
const MN_HEAD_ACT_MAX_LINES = 1;
const MN_BATCH_MAX_LINES = 3;
const MN_PILL_MAX_LINES = 2;

/**
 * S14 移动端「节点」屏（compact 390px 参考视口，五语种）。
 *
 * 量两个槽：
 *  · **页头动作簇** `.mn-act` —— 缺陷③ 的现场。判据两条腿：整簇的**折行数**（棘轮），
 *    以及单颗按钮里最宽的那个**不可断单元**不得超过轨道（`.mn-act` 没有 `overflow:hidden`，
 *    一个比整行还宽的词会画到按钮外）。
 *  · **行内角标** `.mn-pill` —— 静态 i18n 角标，无 ellipsis、无 `overflow-wrap`、可折行 ⇒
 *    与 S11 `.fld-l` 同形：折行免费，但「一个词比整条行主体还宽」是真溢出。
 */
describe('S14 移动端「节点」屏（compact 390px 参考视口，五语种）', () => {
  const PILL_SELS = ['.mn-pill'];
  const ACT_SELS = ['.mn-act'];

  it('几何链必须从 CSS 现场解出（这些数变了，下面的行数预算全部要重推）', () => {
    // 标题和动作同排，统计独立占满下一行。
    expect(
      decl(NODES_CSS, '.mn-head', 'display'),
      '`.mn-head` 的两行网格失效，统计又会与动作争宽',
    ).toBe('grid');
    expect(decl(NODES_CSS, '.mn-count', 'grid-column')).toBe('1 / -1');
    // 横向仍继承外壳标量，纵向收紧以便同屏显示更多节点。
    expect(decl(REDESIGN_CSS, '.mn-row-main', 'padding-top'), '列表行视觉内距独立于宽屏卡片填充').toBe('6px');
    expect(decl(NODES_CSS, '.mn-row-main', 'padding-left')).toBe('10px');
    expect(decl(NODES_CSS, '.mn', 'column-gap'), '.mn 不再继承外壳的 --section-gap').toBe('var(--section-gap)');
    expect([mnActsGap, mnActPadX, mnActBorderX, mnActFont]).toEqual([4, 16, 2, 13]);
    expect(decl(NODES_CSS, '.mn-row.cur', 'border-left-width')).toBe('3px');
    expect([mnListBorderX, mnStateInset, mnRowGap, mnRowMoreW, mnRowMainPadX]).toEqual([2, 3, 4, 88, 20]);
    expect(MN_ACTS_TRACK).toBeCloseTo(358 - 64, 5);
    expect(MN_BATCH_TRACK).toBeCloseTo(358 - 2 - 16, 5); // 340
    expect(MN_ROW_MAIN).toBeCloseTo(358 - 2 - 3 - 4 - 88 - 20, 5); // 241
    expect(MN_PILL_AVAIL).toBeCloseTo(241 - 12 - 2, 5); // 227: two sibling actions
    // 正面断言：每一个槽在每一个语种下都必须有**正的**可用宽。
    expect(MN_PILL_AVAIL).toBeGreaterThan(0);
    expect(MN_BATCH_TRACK).toBeGreaterThan(0);
  });

  it('本节量的键仍真的被 NodesScreenView 消费，且五语种齐备（双向对差，防死键与漏测）', () => {
    const head = mnHeadActKeys();
    const batch = mnBatchKeys();
    const pills = mnPillKeys();
    // 逐字对着解析结果核：顺序或成员一变，说明 JSX 改了结构，槽位归属必须重判。
    expect(head, '页头动作簇的文案变了 —— 轨道预算要重推').toEqual([
      'nodes.add',
      'nodes.mobileMoreActions',
    ]);
    expect(batch, '批量条的文案变了 —— 轨道预算要重推').toEqual([
      'nodes.batch',
      'nodes.selectedPrefix',
      'nodes.selectedSuffix',
      'nodes.selectAll',
      'nodes.testGroup',
      'nodes.batchCopyLinks',
      'common.delete',
      'nodes.batchExit',
    ]);
    expect(pills, '行内角标的文案变了 —— 槽位表要重钉').toEqual([
      'nodes.selectedChoice',
      'nodes.exitCapableBadge',
      'nodes.lanOnly',
      'mobileHelp.stagedNode',
      'mobileHelp.shadowedNode',
    ]);
    // 判「键在不在」用 `typeof === 'string'` 而不是 `toBeTruthy()`：`nodes.selectedPrefix` 在
    // en-US 里**刻意是空串**（英语的量词在后缀上，「 selected」），空串是有效译文不是缺键。
    for (const k of [...head, ...batch, ...pills])
      for (const loc of LOCALES)
        expect(typeof DICT[loc][k], `${loc} 缺键 ${k}（i18n 门该先红）`).toBe('string');
    expect(nodesView()).toContain("t('mobileActions.preparing')");
    for (const loc of LOCALES)
      expect(typeof DICT[loc]['mobileActions.preparing'], `${loc} 缺少批量复制忙态文案`).toBe('string');
    // 正面对照：这批键里绝大多数是有内容的（不是「全空串也能过」）。
    expect(
      [...head, ...batch, ...pills].filter((k) => LOCALES.every((l) => DICT[l][k] !== '')).length,
      '本节的语料几乎全是空串 —— 键映射错了？',
    ).toBeGreaterThanOrEqual(head.length + pills.length);
  });

  it('页头动作簇 × 5 语种：整簇折行棘轮 + 单颗按钮的词级不越界', () => {
    const keys = mnHeadActKeys();
    const overLines: string[] = [];
    const overWord: string[] = [];
    let widestCluster = 0;
    let n = 0;
    for (const loc of LOCALES) {
      n++;
      const widths = keys.map((k, i) => i === 1 ? 48 : mnActW(DICT[loc][k]));
      const { lines, maxLine } = packRow(widths, mnActsGap, MN_ACTS_TRACK);
      widestCluster = Math.max(widestCluster, maxLine);
      if (lines > MN_HEAD_ACT_MAX_LINES) overLines.push(`${loc} | ${lines} 行 | ${maxLine.toFixed(1)}px`);
      // 词级：按钮无 `overflow:hidden`，一个比轨道还宽的不可断单元会画到按钮外。
      for (const k of keys.slice(0, 1)) {
        const box: Box = {
          where: 'S14 页头动作按钮',
          avail: MN_ACTS_TRACK - mnActPadX - mnActBorderX,
          type: { fontSize: mnActFont },
          wrap: wraps(ACT_SELS),
          breakAnywhere: breaksAnywhere(ACT_SELS),
          maxLines: 99,
        };
        if (layout(DICT[loc][k], box.avail, box.type, box).maxLineWidth > box.avail + 0.001)
          overWord.push(`${loc}|${k}`);
      }
    }
    expect(n).toBe(LOCALES.length);
    // 正面断言：两颗文字动作与 48px 更多菜单真实占了轨道，没有空语料假绿。
    expect(widestCluster).toBeGreaterThan(150);
    expect(widestCluster).toBeLessThanOrEqual(MN_ACTS_TRACK);
    expect(overWord, '页头按钮里有词比整条轨道还宽 —— 会画到按钮外').toEqual([]);
    expect(overLines, `页头动作簇折行了（预算 ${MN_HEAD_ACT_MAX_LINES} 行）：\n${overLines.join('\n')}`).toEqual([]);
  });

  it('批量条 × 5 语种：整条折行棘轮（计数 + 五颗按钮）', () => {
    const keys = mnBatchKeys();
    const [, prefix, suffix, ...btns] = keys;
    const over: string[] = [];
    let worstLines = 0;
    for (const loc of LOCALES) {
      // 计数那一格按两位数量最长（`selectedCount` 是运行期值，取 99 作为可见档的上界）。
      const cnt = textPx(`${DICT[loc][prefix]}99${DICT[loc][suffix]}`, { fontSize: mnBatchCntFont });
      const widths = [cnt, ...btns.map((k) => mnActW(DICT[loc][k]))];
      const { lines } = packRow(widths, mnBatchGap, MN_BATCH_TRACK);
      worstLines = Math.max(worstLines, lines);
      if (lines > MN_BATCH_MAX_LINES) over.push(`${loc} | ${lines} 行`);
    }
    expect(btns.length, '批量条的按钮数变了 —— 预算要重推').toBe(5);
    // 正面断言：预算贴着今天最差的一格，不是一个怎么改都不会红的免检额度。
    expect(worstLines, '批量条行数预算已不再贴着今天最差的一格').toBe(MN_BATCH_MAX_LINES);
    expect(over, `批量条超出行数预算：\n${over.join('\n')}`).toEqual([]);
  });

  it('列表行角标 × 5 语种：词级不越界 + 行数棘轮', () => {
    const keys = mnPillKeys();
    const box = (): Box => ({
      where: 'S14 列表行角标',
      avail: MN_PILL_AVAIL,
      type: { fontSize: mnPillFont },
      wrap: wraps(PILL_SELS),
      breakAnywhere: breaksAnywhere(PILL_SELS),
      maxLines: MN_PILL_MAX_LINES,
    });
    const over: Over[] = [];
    const overWidth: string[] = [];
    let n = 0;
    let widest = 0;
    let worstLines = 0;
    for (const loc of LOCALES)
      for (const key of keys) {
        n++;
        const b = box();
        const text = DICT[loc][key];
        const { lines, maxLineWidth } = layout(text, b.avail, b.type, b);
        widest = Math.max(widest, maxLineWidth);
        worstLines = Math.max(worstLines, lines);
        if (maxLineWidth > b.avail + 0.001) overWidth.push(`${loc}|${key}`);
        if (lines > b.maxLines)
          over.push({ where: b.where, loc, key, text, need: maxLineWidth, avail: b.avail, lines, budget: b.maxLines });
      }
    expect(n, '测点数异常偏低 —— 语种或角标少了一批？').toBe(LOCALES.length * 5);
    // 正面断言：算出了具体宽度，且今天最宽的那一格钉住（语料一涨就要重新看）。
    expect(widest).toBeGreaterThan(0);
    expect(
      Math.max(...LOCALES.map((l) => textPx(DICT[l]['mobileHelp.shadowedNode'], { fontSize: mnPillFont }))),
      '移动端被覆盖状态的完整后果宽度变了 —— 预算要重新核对',
    ).toBeCloseTo(263.12, 1);
    expect(worstLines, '角标行数预算不再贴着最差的一格').toBe(MN_PILL_MAX_LINES);
    expect(overWidth, '角标横向越界 —— 它没有 ellipsis 也没有 overflow-wrap，会画到行外').toEqual([]);
    expect(over.length, `角标行数超预算：\n${fmt(over)}`).toBe(0);
  });
});

// ── S15 移动端「规则」屏：几何链 ────────────────────────────────────────────────────────

const RULES_CSS = '../mobile/screens/rules/rules-screen.css' as const;
const RULES_REDESIGN_CSS = '../mobile/screens/rules/rules-redesign.css' as const;
const ruleRow = () => src('../mobile/screens/rules/RuleRow.tsx');
const resSeg = () => src('../mobile/screens/rules/ResourcesSegment.tsx');
const traffSeg = () => src('../mobile/screens/rules/TrafficSegment.tsx');

/**
 * 实体行主体（`.mr-row-main`）的内容宽 —— 两个档，因为同一个 `.mr-row` 有两种子项组合，
 * 而角标是按**最窄的那一档**判的（宽的那档同构地更松）：
 *
 *  · **规则行**（`RuleRow.tsx`，最窄）：[ `.mr-pri`(min-width 22, flex-shrink:0) | `.mr-row-ic`? |
 *    `.mr-row-main`(1 1 auto, min-width:0) | `.mr-row-acts`(flex-shrink:0 = 开关 48 + gap + 更多 48) ]。
 *    规则行没有 `.mr-row-ic`（图标那一格是应用行才有的），故只扣序号与动作簇。
 *  · **资源行**（`ResourcesSegment.tsx`）：只有 `.mr-row-main` + 一颗「更多」。
 *
 * 有序列表 `.mr-list` **恒单列**（行号 = 优先级，rules-screen.css 的头注写死了这条），
 * 故规则行的轨道就是页面内容宽，不随断点变窄。
 */
const mrRowBorderX = borderX(RULES_CSS, '.mr-row');
const mrRowPadX = padX(resolveShell(decl(RULES_CSS, '.mr-row', 'padding'))); // var(--sp-3)*2 = 24
const mrRowGap = px(resolveShell(decl(RULES_CSS, '.mr-row', 'column-gap'))); // var(--sp-3) = 12
const mrPriW = px(decl(RULES_CSS, '.mr-pri', 'min-width')); // 22
const mrSwitchW = px(resolveShell(decl(RULES_CSS, '.mr-switch', 'width'))); // var(--tap-min) = 48
const mrBtnMinW = px(resolveShell(decl(RULES_CSS, '.mr-btn', 'min-width'))); // var(--tap-min) = 48
const mrActsGap = px(resolveShell(decl(RULES_CSS, '.mr-row-acts', 'column-gap'))); // var(--sp-1) = 4
const MR_ROW_INNER = M_PAGE_CONTENT - mrRowBorderX - mrRowPadX;
/** 规则行：扣序号 + 两道 gap + 动作簇（开关 + gap + 更多）。 */
const MR_MAIN_RULE = MR_ROW_INNER - mrPriW - 2 * mrRowGap - (mrSwitchW + mrActsGap + mrBtnMinW);
/** 资源行：只扣一道 gap + 一颗「更多」。 */
const MR_MAIN_RES = MR_ROW_INNER - mrRowGap - mrBtnMinW;

const mrPillPadX = padX(decl(RULES_CSS, '.mr-pill', 'padding')); // 2px 7px → 14
const mrPillBorderX = borderX(RULES_CSS, '.mr-pill');
const mrPillFont = remPx(decl(RULES_CSS, '.mr-pill', 'font-size')); // 0.6875rem = 11
const MR_PILL_AVAIL_RULE = MR_MAIN_RULE - mrPillPadX - mrPillBorderX;
const MR_PILL_AVAIL_RES = MR_MAIN_RES - mrPillPadX - mrPillBorderX;

/** 卡内宽（`.mr-card` 是 `box-sizing: border-box`，继承外壳的 `--card-padding`）。 */
const mrCardBorderX = borderX(RULES_CSS, '.mr-card');
const mrCardPadX = padX(resolveShell(decl(RULES_CSS, '.mr-card', 'padding')));
const MR_CARD_INNER = M_PAGE_CONTENT - mrCardBorderX - mrCardPadX;

/** 优先级链：`.mr-chain` 是 flex-wrap，链节与箭头交替排。 */
const mrChainGap = px(resolveShell(decl(RULES_CSS, '.mr-chain', 'column-gap'))); // var(--sp-1) = 4
const mrStepPadX = padX(decl(RULES_CSS, '.mr-chain-step', 'padding')); // 3px 8px → 16
const mrStepBorderX = borderX(RULES_CSS, '.mr-chain-step');
const mrStepFont = remPx(decl(RULES_CSS, '.mr-chain-step', 'font-size')); // 0.6875rem = 11
const mrArrowFont = remPx(decl(RULES_CSS, '.mr-chain-arrow', 'font-size')); // 0.75rem = 12
const mrStepW = (label: string) =>
  textPx(label, { fontSize: mrStepFont }) + mrStepPadX + mrStepBorderX;
/** 箭头的字面量从 JSX 现场取（换了符号宽度就变）。 */
const MR_ARROW_GLYPH = (() => {
  const m = src('../mobile/screens/rules/Primitives.tsx').match(
    /className="mr-chain-arrow">([^<]+)</,
  );
  if (!m) throw new Error('Primitives.tsx 里读不到 `.mr-chain-arrow` 的字面量 —— 箭头改写法了？');
  return m[1];
})();

/**
 * `.mr-pill` 的静态键。**两档分开量**，因为它们出现在不同宽的行里（见上面的几何链）。
 *
 * 键来自三处，故这里是**手维护的登记表 + 双向对差**（同 `TRAY_SLOT` 的形态）：
 * 两处在 `RuleRow.tsx` 的 JSX 段里，另两处（动作 pill / DNS 动作文案）住在同文件的两个
 * 辅助函数里、JSX 段扒不到。下面那条「双向对差」把三条路都对一遍。
 *
 * 带插值的 `rules.dnsPill` 不按模板量而是**把插值真的填进去**（`{{answer}}` 只能取
 * `rules.dnsAnswerFakeIp` / `rules.dnsAnswerReal` 两个 i18n 值之一）⇒ 这一格是精确值，
 * 不是文件头「射程之外 b.」那种偏短的模板估计。
 * `rules.dnsActionServer/Group/Hosts` 三条插的是**运行期名字**，属 b.，不测。
 */
const MR_PILL_KEYS_RULE = [
  'rules.targetBlock',
  'rules.targetDirect',
  'rules.policyProxy',
  'rules.dnsResolverProxy',
  'rules.dnsResolverDirect',
  'rules.dnsResolverInherit',
  'rules.dnsActionReject',
  'rules.dnsActionPredefined',
  'home.stagedOnlyBadge',
  'rules.meshOverlap',
  'rules.resourceMissing',
  'rules.targetMissing',
  // 角标⑤「生效网络」的静态那一档（场景已删除）。另一档 `rules.networkProfile.badge`（「仅 {{name}}」）
  // 插的是**场景名**（运行期用户数据），属文件头「射程之外 b.」，不测 —— 登记在下面的运行期插值集里。
  'rules.networkProfile.badgeMissing',
] as const;
const MR_PILL_ANSWERS = ['rules.dnsAnswerFakeIp', 'rules.dnsAnswerReal'] as const;
const MR_PILL_KEYS_RES = [
  'resources.src.builtin',
  'resources.src.external',
  'resources.smartRouting',
  'resources.unreferenced',
  'resources.cancelled',
  'resources.updateFailed',
] as const;

/** 优先级链的两条链（流量段 6 节住在卡里、DNS 段 3 节住在页面上）。窄的那条是卡里那条。 */
const mrChainTrafficKeys = () =>
  keysOf(segment(traffSeg(), '<PriorityFlow', '/>', '流量段优先级链'), '流量链文案键').slice(1);
const MR_CHAIN_MAX_LINES = 3;

/**
 * S15 移动端「规则」屏（compact 390px 参考视口，五语种）。
 *
 * 量两个槽：
 *  · **实体行角标** `.mr-pill` —— 本节的由来。它此前是 `white-space: nowrap` + 无 ellipsis +
 *    无 `overflow-wrap`，「装不下」的唯一表现是画到行外，与 2026-07-31 侧栏那次同形。
 *    修法是改成**换行 + 词内断兜底**（rules-screen.css 就地注释），故这里是**硬判据**：
 *    横向恒不越界。判红就是真破版。
 *  · **优先级链** `.mr-chain-step` —— flex-wrap 的链节簇，判整链折几行（棘轮）+ 单节词级不越界。
 */
describe('S15 移动端「规则」屏（compact 390px 参考视口，五语种）', () => {
  const PILL_SELS = ['.mr-pill'];
  const STEP_SELS = ['.mr-chain-step'];

  it('几何链必须从 CSS 现场解出（这些数变了，下面的预算全部要重推）', () => {
    expect(decl(RULES_CSS, '.mr-card', 'padding'), '.mr-card 不再继承外壳的 --card-padding').toBe(
      box4('var(--card-padding)'),
    );
    expect(decl(RULES_REDESIGN_CSS, '.mr-screen', 'column-gap')).toBe('14px');
    expect([mrRowBorderX, mrRowPadX, mrRowGap, mrPriW]).toEqual([2, 24, 12, 22]);
    expect([mrSwitchW, mrBtnMinW, mrActsGap]).toEqual([48, 48, 4]);
    expect(MR_ROW_INNER).toBeCloseTo(358 - 2 - 24, 5); // 332
    expect(MR_MAIN_RULE).toBeCloseTo(332 - 22 - 24 - 100, 5); // 186
    expect(MR_MAIN_RES).toBeCloseTo(332 - 12 - 48, 5); // 272
    expect(MR_PILL_AVAIL_RULE).toBeCloseTo(186 - 16, 5); // 170
    expect(MR_PILL_AVAIL_RES).toBeCloseTo(272 - 16, 5); // 256
    expect(MR_CARD_INNER).toBeCloseTo(358 - 2 - 32, 5); // 324
    expect([mrPillFont, mrStepFont, mrArrowFont]).toEqual([11, 11, 12]);
    // 正面断言：两个槽在任何语种下都必须有正的可用宽。
    expect(MR_PILL_AVAIL_RULE).toBeGreaterThan(0);
    expect(MR_CARD_INNER - mrStepPadX - mrStepBorderX).toBeGreaterThan(0);
    // 有序列表恒单列 —— 规则行的轨道宽是页面内容宽，这条是它的前提（rules-screen.css 头注）。
    expect(
      rulesOf(RULES_CSS).filter((r) => selMatches(r.sel, '.mr-list')).length,
      '`.mr-list` 出现了不止一条规则 —— 多出来的那条若是断点双列，行宽预算全部作废',
    ).toBe(1);
  });

  it('修法在位：`.mr-pill` 必须能折行、且带词内断兜底（回到 nowrap ⇒ 画到行外）', () => {
    // 这颗 pill 一条兜底都没有（无 ellipsis、无 overflow:hidden）⇒ nowrap 之下装不下就是破版。
    expect(
      wraps(PILL_SELS),
      '`.mr-pill` 又变回 nowrap ⇒ ru 的 `rules.dnsResolverInherit`（239.4px）会画到 170px 的行外',
    ).toBe(true);
    expect(
      breaksAnywhere(PILL_SELS),
      '`.mr-pill` 的词内断兜底没了 ⇒ 一个比整条行主体还宽的词仍会画到行外',
    ).toBe(true);
    expect(clips(PILL_SELS), '`.mr-pill` 长出了 ellipsis —— 静态角标的口径是换行，不是截断').toBe(false);
    // 回放：修复前的口径（nowrap ⇒ 整串一行）下，那一格必须被算成越界 —— 判据打不到它就是摆设。
    const preFix = layout(DICT.ru['rules.dnsResolverInherit'], MR_PILL_AVAIL_RULE, { fontSize: mrPillFont }, {
      wrap: false,
      breakAnywhere: false,
    });
    expect(preFix.maxLineWidth, 'ru `rules.dnsResolverInherit` 的模型宽度变了 —— 回放要重钉').toBeCloseTo(239.4, 1);
    expect(preFix.maxLineWidth).toBeGreaterThan(MR_PILL_AVAIL_RULE);
  });

  it('本节量的键仍真的被消费（双向对差：三条路都对一遍，防死键与漏测）', () => {
    const rowSrc = ruleRow();
    // ① 僵尸自检：表里每个键都必须在源码里逐字出现（删了一个角标却留在表里 ⇒ 红）。
    for (const k of [...MR_PILL_KEYS_RULE, ...MR_PILL_ANSWERS])
      expect(rowSrc, `RuleRow.tsx 里已经没有 ${k} —— 登记表里留了一个死键`).toContain(`'${k}'`);
    for (const k of MR_PILL_KEYS_RES)
      expect(resSeg(), `ResourcesSegment.tsx 里已经没有 ${k} —— 登记表里留了一个死键`).toContain(`'${k}'`);
    // ② 漏测自检：JSX 的 `.mr-row-pills` 段里出现的键必须都在表里（新加一颗角标 ⇒ 红）。
    const jsxRule = keysOf(segment(rowSrc, 'className="mr-row-pills"', '{notes.map', '规则行角标段'), '规则行角标键');
    const table = new Set<string>([...MR_PILL_KEYS_RULE, ...MR_PILL_ANSWERS, 'rules.dnsPill']);
    /** 插的是运行期用户数据（场景名）的角标：不量，但必须显式列在这里（不许从缝里溜走）。 */
    const pillRuntimeInterp = new Set(['rules.networkProfile.badge']);
    expect(rowSrc, 'RuleRow.tsx 里已经没有 rules.networkProfile.badge —— 运行期插值集留了一个死键').toContain(
      "'rules.networkProfile.badge'",
    );
    expect(
      jsxRule.filter((k) => !table.has(k) && !pillRuntimeInterp.has(k)),
      '规则行里出现了没进本节槽位表的角标文案 —— 补进表再量，别让它从缝里溜走',
    ).toEqual([]);
    const jsxRes = keysOf(segment(resSeg(), 'className="rc-title-line"', '<InlineError', '资源行状态段'), '资源行状态键');
    expect(
      jsxRes.filter((k) => !new Set<string>([...MR_PILL_KEYS_RES, 'resources.refCountAria', 'resources.col.size', 'resources.col.updated']).has(k)),
      '资源行里出现了没进本节槽位表的状态文案',
    ).toEqual([]);
    // ③ 两个辅助函数里的键（JSX 段扒不到那两条路）。
    const helper = segment(rowSrc, 'function actionPill', 'export function RuleRow', '动作 pill 工厂');
    const dnsText = segment(rowSrc, 'const dnsActionText', '常驻解释行', 'DNS 动作文案');
    const helperKeys = [...keysOf(helper, '动作 pill 文案键'), ...keysOf(dnsText, 'DNS 动作文案键')];
    const runtimeInterp = new Set(['rules.dnsActionServer', 'rules.dnsActionGroup', 'rules.dnsActionHosts']);
    expect(
      helperKeys.filter((k) => !table.has(k) && !runtimeInterp.has(k)),
      '动作 pill / DNS 动作文案里出现了没进表也没登记成运行期插值的键',
    ).toEqual([]);
    // 五语种齐备。
    for (const k of [...MR_PILL_KEYS_RULE, ...MR_PILL_ANSWERS, ...MR_PILL_KEYS_RES, 'rules.dnsPill'])
      for (const loc of LOCALES) expect(DICT[loc][k], `${loc} 缺键 ${k}（i18n 门该先红）`).toBeTruthy();
  });

  it('实体行角标 × 5 语种 × 2 档行宽：横向恒不越界 + 行数棘轮', () => {
    // 插值真的填进去，不按模板量（模板比实际短 ⇒ 那是漏报方向）。
    // 占位符对不上时 `replace` 是**静默无操作**，故先钉住它存在。
    const dnsPill = (loc: Locale, answerKey: string) => {
      const tpl = DICT[loc]['rules.dnsPill'];
      expect(tpl, `${loc} 的 rules.dnsPill 里没有 {{answer}} 占位符 —— 插值填不进去，会按模板少测`).toContain(
        '{{answer}}',
      );
      return tpl.replace('{{answer}}', DICT[loc][answerKey]);
    };
    const box = (avail: number): Box => ({
      where: 'S15 实体行角标',
      avail,
      type: { fontSize: mrPillFont },
      wrap: wraps(PILL_SELS),
      breakAnywhere: breaksAnywhere(PILL_SELS),
      maxLines: 2,
    });
    const overWidth: string[] = [];
    const overLines: Over[] = [];
    let n = 0;
    let widest = 0;
    /** 全部测点里**余量最小**的那一格（`avail − maxLineWidth`）。两档行宽不同，故比余量不比宽度。 */
    let tightest = Infinity;
    let worstLines = 0;
    const measure = (loc: Locale, key: string, text: string, avail: number, tag: string) => {
      n++;
      const b = box(avail);
      const { lines, maxLineWidth } = layout(text, b.avail, b.type, b);
      widest = Math.max(widest, maxLineWidth);
      tightest = Math.min(tightest, b.avail - maxLineWidth);
      worstLines = Math.max(worstLines, lines);
      if (maxLineWidth > b.avail + 0.001) overWidth.push(`${loc}|${key}|${tag}`);
      if (lines > b.maxLines)
        overLines.push({ where: b.where, loc, key, text, need: maxLineWidth, avail: b.avail, lines, budget: b.maxLines });
    };
    for (const loc of LOCALES) {
      for (const k of MR_PILL_KEYS_RULE) measure(loc, k, DICT[loc][k], MR_PILL_AVAIL_RULE, 'rule');
      for (const a of MR_PILL_ANSWERS) measure(loc, `rules.dnsPill/${a}`, dnsPill(loc, a), MR_PILL_AVAIL_RULE, 'rule');
      for (const k of MR_PILL_KEYS_RES) measure(loc, k, DICT[loc][k], MR_PILL_AVAIL_RES, 'res');
    }
    expect(n, '测点数异常偏低 —— 语种或角标少了一批？').toBe(LOCALES.length * (13 + 2 + 6));
    // 正面断言：算出了具体宽度，且最紧的那一格钉住（语料一涨就要重新看一眼）。
    expect(widest).toBeGreaterThan(0);
    // 今天最紧的那一格是 ru `rules.meshOverlap`（「Переопределяет mesh」，单行 169.29px），
    // 槽只有 170 —— 余量 0.71px。这个数一变就说明语料动了，必须重新看一眼。
    expect(
      tightest,
      '角标里余量最小的那一格变了 —— 今天它离满槽只剩 0.71px，语料一动就要重看',
    ).toBeCloseTo(0.71, 2);
    expect(
      layout(DICT.ru['rules.dnsResolverInherit'], MR_PILL_AVAIL_RULE, { fontSize: mrPillFont }, { wrap: true, breakAnywhere: true }).maxLineWidth,
      'ru `rules.dnsResolverInherit` 折行后的最宽行变了 —— 修法就是让它折进来',
    ).toBeCloseTo(166.0, 1);
    expect(worstLines, '角标行数预算不再贴着今天最差的一格').toBe(2);
    expect(overWidth, '实体行角标横向越界 —— 它没有 ellipsis 也没有 overflow:hidden，会画到行外').toEqual([]);
    expect(overLines.length, `实体行角标行数超预算：\n${fmt(overLines)}`).toBe(0);
  });

  it('优先级链 × 5 语种：整链折行棘轮 + 单节词级不越界', () => {
    const keys = mrChainTrafficKeys();
    expect(keys, '流量段优先级链的节点变了 —— 预算要重推').toEqual([
      'rules.chainCustom',
      'rules.chainApp',
      'rules.chainMesh',
      'rules.chainLan',
      'rules.chainSmart',
      'rules.chainDefault',
    ]);
    const arrowW = textPx(MR_ARROW_GLYPH, { fontSize: mrArrowFont });
    const overLines: string[] = [];
    const overWord: string[] = [];
    let worstLines = 0;
    for (const loc of LOCALES) {
      const items: number[] = [];
      keys.forEach((k, i) => {
        items.push(mrStepW(DICT[loc][k]));
        if (i < keys.length - 1) items.push(arrowW);
      });
      const { lines } = packRow(items, mrChainGap, MR_CARD_INNER);
      worstLines = Math.max(worstLines, lines);
      if (lines > MR_CHAIN_MAX_LINES) overLines.push(`${loc} | ${lines} 行`);
      // 词级：链节无 `overflow:hidden`，一个比卡内宽还宽的不可断单元会画到卡外。
      const avail = MR_CARD_INNER - mrStepPadX - mrStepBorderX;
      for (const k of keys) {
        const w = layout(DICT[loc][k], avail, { fontSize: mrStepFont }, {
          wrap: wraps(STEP_SELS),
          breakAnywhere: breaksAnywhere(STEP_SELS),
        }).maxLineWidth;
        if (w > avail + 0.001) overWord.push(`${loc}|${k}`);
      }
    }
    // 正面断言：预算贴着今天最差的一格（ru 三行），不是免检额度。
    expect(worstLines, '优先级链行数预算不再贴着今天最差的一格').toBe(MR_CHAIN_MAX_LINES);
    expect(overWord, '链节里有词比整张卡还宽 —— 会画到卡外').toEqual([]);
    expect(overLines, `优先级链超出行数预算：\n${overLines.join('\n')}`).toEqual([]);
  });
});

// ── S16 移动端「连接」屏：几何链 ────────────────────────────────────────────────────────

const CONN_CSS = '../mobile/connections/connections.css' as const;
const connView = () => src('../mobile/connections/ConnectionsView.tsx');

/**
 * 展开区里的**定宽标签列** `.mc-det-row dt`。
 *
 * `.mc-det`（`padding: var(--sp-3)`）里是一列 `.mc-det-row`（flex，gap `--sp-3`）=
 * [ `dt`(flex: 0 0 5.5em) | `dd`(flex:1 1 auto, min-width:0) ]。`.mc-row` 只有纵向内边距、
 * `.mc-list` / `.mc-body` 无内边距 ⇒ 展开区外宽就是页面内容宽。
 *
 * `5.5em` 里的 `em` 按 **`dt` 自己的字号**解析，而它继承 `.mc-det` 的 `0.75rem` ⇒ 66px。
 * 这一格随 Dynamic Type 等比放大（em 与 rem 同步），故比例是固定的、只需要在 1x 下量一次。
 *
 * `flex: 0 0` = 既不长也不缩 ⇒ 它是**真定宽**：内容比 66px 宽就横着画到右边的值上。
 */
const mcDetPadX = padX(resolveShell(decl(CONN_CSS, '.mc-det', 'padding'))); // var(--sp-3)*2 = 24
const mcDetFont = remPx(decl(CONN_CSS, '.mc-det', 'font-size')); // 0.75rem = 12
const MC_DET_INNER = M_PAGE_CONTENT - mcDetPadX;
/** `flex: <grow> <shrink> <basis>` 的 basis（只接受 `Nem`，写成别的即 throw）。 */
const mcDtBasisEm = (() => {
  // 逐**分量**读（共享解析器把 `flex` 简写折成 grow/shrink/basis）：后面补一句
  // `flex-grow: 1` / `flex-basis: 10em` 都算数，读简写原文的旧写法对它们全盲。
  const grow = decl(CONN_CSS, '.mc-det-row dt', 'flex-grow');
  const shrink = decl(CONN_CSS, '.mc-det-row dt', 'flex-shrink');
  const basis = decl(CONN_CSS, '.mc-det-row dt', 'flex-basis');
  const m = basis.trim().match(/^([\d.]+)em$/);
  if (!m)
    throw new Error(`\`.mc-det-row dt\` 的 \`flex-basis\` 不再是 \`Nem\`：\`${basis}\``);
  if (grow !== '0' || shrink !== '0')
    throw new Error(
      `\`.mc-det-row dt\` 不再是 \`0 0\`（grow=\`${grow}\` shrink=\`${shrink}\`）—— ` +
        '它一旦可长可缩，本节的「定宽」判据就不成立了',
    );
  return parseFloat(m[1]);
})();
const MC_DT_AVAIL = mcDtBasisEm * mcDetFont;

/** 概览卡：卡头标题被同排那个 `flex:none` 的尾件（分段控件 / 只读徽标）吃掉宽度。 */
const mcCardBorderX = borderX(CONN_CSS, '.mc-card');
const mcCardPadX = padX(resolveShell(decl(CONN_CSS, '.mc-card', 'padding')));
const MC_CARD_INNER = M_PAGE_CONTENT - mcCardBorderX - mcCardPadX;
const mcCardHGap = px(resolveShell(decl(CONN_CSS, '.mc-card-h', 'column-gap'))); // var(--sp-3) = 12
const mcCardHFont = remPx(decl(CONN_CSS, '.mc-card-h', 'font-size')); // 0.9375rem = 15
const mcChoiceGap = px(resolveShell(decl(CONN_CSS, '.mc-choice', 'column-gap'))); // var(--sp-1) = 4
const mcChoiceBtnMinW = px(decl(CONN_CSS, '.mc-choice .mc-btn', 'min-width')); // 40
const mcChoiceBtnPadX = padX(resolveShell(decl(CONN_CSS, '.mc-choice .mc-btn', 'padding'))); // var(--sp-2)*2 = 16
const mcBtnBorderX = borderX(CONN_CSS, '.mc-btn');
const mcBtnFont = remPx(decl(CONN_CSS, '.mc-btn', 'font-size')); // 0.8125rem = 13
const mcPillPadX = padX(resolveShell(decl(CONN_CSS, '.mc-pill', 'padding'))); // 0 var(--sp-2) → 16
const mcPillBorderX = borderX(CONN_CSS, '.mc-pill');
const mcPillFont = remPx(decl(CONN_CSS, '.mc-pill', 'font-size')); // 0.6875rem = 11

/** TOP 卡的档位（真值在 `view-model.ts`，不写死 3 档：加一档卡头就更挤）。 */
const MC_TOP_N = (() => {
  const m = src('../mobile/connections/view-model.ts').match(
    /TOP_N_OPTIONS[^=]*=\s*\[([^\]]+)\]/,
  );
  if (!m) throw new Error('view-model.ts 里读不到 `TOP_N_OPTIONS` —— 常量被改名/换写法？');
  return m[1].split(',').map((x) => parseInt(x.trim(), 10)).filter((x) => Number.isFinite(x));
})();
/** 分段控件的外宽（每颗按钮取 `max(min-width, 文字 + 内边距 + 边框)`）。 */
const MC_CHOICE_W =
  MC_TOP_N.reduce(
    (sum, n) =>
      sum +
      Math.max(mcChoiceBtnMinW, textPx(String(n), { fontSize: mcBtnFont }) + mcChoiceBtnPadX + mcBtnBorderX),
    0,
  ) +
  (MC_TOP_N.length - 1) * mcChoiceGap;

/**
 * 展开区标签的键（顺序即 DOM 序）。
 *
 * `<dt>` 不在这一段里直接写，标签是 `<DetailRow label={t('…')} …/>` 传进去的
 * （`DetailRow` 内部才渲染 `<dt>{label}</dt>`）⇒ 扒的是 `label=` 那一侧。
 * 下面另有一条断言钉着「`DetailRow` 确实把 label 渲染成 `<dt>`」，否则这里量的就是别的槽。
 */
const mcDtKeys = () => {
  const s = connView();
  if (!/<dt>\{label\}<\/dt>/.test(s))
    throw new Error('`DetailRow` 不再把 `label` 渲染成 `<dt>` —— 本节量的槽位换了');
  const seg = segment(s, '<dl className="mc-det"', '</dl>', '展开区定义列表');
  const keys = [
    ...new Set(grepAll(seg, /<DetailRow[\s\S]*?label=\{t\('([\w.]+)'\)\}/g, '展开区标签').map((m) => m[1])),
  ];
  if (keys.length < 8) throw new Error(`展开区只解析到 ${keys.length} 条标签 —— 截段失效了？`);
  return keys;
};
/** 两张概览卡的标题键 + 只读徽标键。 */
const mcOverviewKeys = () => {
  const s = connView();
  const hosts = s.match(/className="mc-card-h">\s*<span>\{t\('([\w.]+)'\)\}<\/span>/);
  const badge = s.match(/className="mc-pill region">\{t\('([\w.]+)'/);
  const out = keysOf(
    segment(s, 'data-segment="overview"', '══════════ 活动', '概览段'),
    '概览段文案键',
  );
  if (!hosts || !badge) throw new Error('ConnectionsView.tsx 的概览卡头改写法了 —— 标题/徽标扒不到');
  const titles = out.filter((k) => /overview\.(hosts|outbounds)Title$/.test(k));
  if (titles.length !== 2) throw new Error(`概览卡标题解析到 ${titles.length} 条（应为 2）`);
  return { titles, badge: badge[1], first: hosts[1] };
};

/** 行数棘轮：`dt` 与卡头都可折行、纵向自由生长，数钉在今天最差的那一格。 */
const MC_DT_MAX_LINES = 2;
const MC_CARD_H_MAX_LINES = 3;

/**
 * 今天**踩到词内断兜底**的标签格（模型口径）。
 *
 * `.mc-det-row dt` 是 `flex: 0 0 5.5em` 的**真定宽**列，2026-09-05 首次量到 50 格里 9 格越界。
 * 修法是给它 `overflow-wrap: anywhere`（connections.css 就地注释：调宽是拿值列换标签列，
 * 且只对今天这批语料有效）⇒ 横向从此**恒不越界**，这张表降级成「谁踩到了兜底」。
 *
 * 表里 9 格：1 格 en（`Destination`，模型 86.2px）+ 8 格 ru。
 *
 * ⚠️ **这张表是模型口径，不是真机口径。** 同日在 Android 模拟器（412 CSS px，栈落到 Roboto）
 * 逐条实测过：真机上真正越界的只有 2 格，都在 ru（`colTime` 80.4px / `colDest` 67.7px，槽 66px）；
 * en 的 `Destination` 真机只有 60.8px，装得下。故这条缺陷**成立但规模比模型小** ——
 * 「必须修」的依据是那 2 格真机越界 + 这一列零兜底的形态，不是模型报的 9 格。
 */
const MC_DT_BREAK_INSIDE = [
  'en-US|connections.colDest',
  'ru|connections.colChain',
  'ru|connections.colDest',
  'ru|connections.colEnded',
  'ru|connections.colProcess',
  'ru|connections.colRule',
  'ru|connections.colTime',
  'ru|connections.colTraffic',
  'ru|mobileConnections.sourceIP',
];

/**
 * S16 移动端「连接」屏（compact 390px 参考视口，五语种）。
 *
 * 量两个槽：
 *  · **展开区标签列** `.mc-det-row dt` —— 本节的由来（定宽 5.5em，9 格越界）。
 *  · **概览卡头标题** `.mc-card-h > span` —— 同排挂着一个
 *    按 max-content 取宽的尾件（分段控件 / 只读徽标），标题拿到的是剩下的那一截。
 */
describe('S16 移动端「连接」屏（compact 390px 参考视口，五语种）', () => {
  const DT_SELS = ['.mc-det-row dt'];
  const CARD_H_SELS = ['.mc-card-h'];

  it('几何链必须从 CSS 现场解出（这些数变了，下面的预算全部要重推）', () => {
    expect(decl(CONN_CSS, '.mc-card', 'padding'), '.mc-card 不再继承外壳的 --card-padding').toBe(
      box4('var(--card-padding)'),
    );
    expect(decl(CONN_CSS, '.mc-body', 'column-gap'), '.mc-body 不再继承外壳的 --section-gap').toBe(
      'var(--section-gap)',
    );
    expect([mcDetPadX, mcDetFont, mcDtBasisEm]).toEqual([24, 12, 5.5]);
    expect(MC_DET_INNER).toBeCloseTo(358 - 24, 5); // 334
    expect(MC_DT_AVAIL).toBeCloseTo(66, 5);
    expect(MC_CARD_INNER).toBeCloseTo(358 - 2 - 32, 5); // 324
    expect([mcCardHGap, mcCardHFont, mcPillFont, mcBtnFont]).toEqual([12, 15, 11, 13]);
    expect(MC_TOP_N, 'TOP 档位变了 —— 卡头的尾件宽度要重算').toEqual([5, 10, 15]);
    expect(MC_CHOICE_W).toBeCloseTo(3 * 40 + 2 * 4, 5); // 128
    // 正面断言：两个槽在任何语种下都必须有**正的**可用宽（尾件一变宽，标题那一格会先变负）。
    expect(MC_DT_AVAIL).toBeGreaterThan(0);
    for (const loc of LOCALES) {
      const badge = mcOverviewKeys().badge;
      const pillW = textPx(DICT[loc][badge], { fontSize: mcPillFont }) + mcPillPadX + mcPillBorderX;
      expect(MC_CARD_INNER - mcCardHGap - pillW, `概览卡头 ${loc} 标题可用宽 ≤ 0`).toBeGreaterThan(0);
      expect(MC_CARD_INNER - mcCardHGap - MC_CHOICE_W, `概览卡头 ${loc} 标题可用宽 ≤ 0`).toBeGreaterThan(0);
    }
  });

  it('修法在位：定宽标签列必须带词内断兜底（没有它，标签会盖到右边的值上）', () => {
    expect(
      breaksAnywhere(DT_SELS),
      '`.mc-det-row dt` 的词内断兜底没了 ⇒ en 的 `Destination`（86.2px）会画到 66px 的格子外、盖住值',
    ).toBe(true);
    expect(clips(DT_SELS), '标签列长出了 ellipsis —— 静态标签的口径是换行，不是截断').toBe(false);
    // 回放：无兜底口径下，9 格必须被算成越界 —— 判据打不到它们就是摆设。
    const preFix: string[] = [];
    for (const loc of LOCALES)
      for (const k of mcDtKeys())
        if (
          layout(DICT[loc][k], MC_DT_AVAIL, { fontSize: mcDetFont }, { wrap: true, breakAnywhere: false })
            .maxLineWidth >
          MC_DT_AVAIL + 0.001
        )
          preFix.push(`${loc}|${k}`);
    expect(preFix.sort(), '无兜底口径下的越界面变了 —— 这批就是 2026-09-05 首次量到的那 9 格').toEqual(
      MC_DT_BREAK_INSIDE,
    );
    expect(
      textPx(DICT['en-US']['connections.colDest'], { fontSize: mcDetFont }),
      'en `Destination` 的**模型**宽度变了 —— 登记表按模型口径钉，真机那 2 格另记在表头注释里',
    ).toBeCloseTo(86.2, 1);
  });

  it('本节量的键仍真的被 ConnectionsView 消费，且五语种齐备', () => {
    const dt = mcDtKeys();
    const ov = mcOverviewKeys();
    expect(dt, '展开区标签变了 —— 登记表要重新钉').toEqual([
      'connections.colHost',
      'connections.colDest',
      'connections.colRule',
      'connections.colChain',
      'connections.colType',
      'connections.colTraffic',
      'connections.colTime',
      'connections.colEnded',
      'connections.colProcess',
      'mobileConnections.sourceIP',
    ]);
    expect(ov.titles.length).toBe(2);
    expect(ov.first, '概览第一张卡的标题键变了').toBe('mobileConnections.overview.hostsTitle');
    expect(ov.badge, '概览第二张卡的只读徽标键变了').toBe('connections.topBadge');
    // 僵尸自检：登记表里的每一格都必须还能在今天的键集合里找到。
    for (const cell of MC_DT_BREAK_INSIDE)
      expect(dt, `登记表里的 ${cell} 指向一个已经不存在的标签`).toContain(cell.split('|')[1]);
    for (const k of [...dt, ...ov.titles, ov.badge])
      for (const loc of LOCALES) expect(DICT[loc][k], `${loc} 缺键 ${k}（i18n 门该先红）`).toBeTruthy();
  });

  it('展开区标签列 × 5 语种：横向恒不越界 + 词内断登记表 + 行数棘轮', () => {
    const keys = mcDtKeys();
    const box = (): Box => ({
      where: 'S16 展开区标签列',
      avail: MC_DT_AVAIL,
      type: { fontSize: mcDetFont },
      wrap: wraps(DT_SELS),
      breakAnywhere: breaksAnywhere(DT_SELS),
      maxLines: MC_DT_MAX_LINES,
    });
    const overWidth: string[] = [];
    const brokeInside: string[] = [];
    const overLines: Over[] = [];
    let n = 0;
    let widest = 0;
    let worstLines = 0;
    for (const loc of LOCALES)
      for (const key of keys) {
        n++;
        const b = box();
        const text = DICT[loc][key];
        const { lines, maxLineWidth } = layout(text, b.avail, b.type, b);
        const noFallback = layout(text, b.avail, b.type, { ...b, breakAnywhere: false });
        widest = Math.max(widest, maxLineWidth);
        worstLines = Math.max(worstLines, lines);
        if (maxLineWidth > b.avail + 0.001) overWidth.push(`${loc}|${key}`);
        if (noFallback.maxLineWidth > b.avail + 0.001) brokeInside.push(`${loc}|${key}`);
        if (lines > b.maxLines)
          overLines.push({ where: b.where, loc, key, text, need: maxLineWidth, avail: b.avail, lines, budget: b.maxLines });
      }
    expect(n, '测点数异常偏低 —— 语种或标签少了一批？').toBe(LOCALES.length * 10);
    // 正面断言：算出了具体宽度，且行数预算贴着今天最差的一格。
    expect(widest).toBeGreaterThan(0);
    expect(worstLines, '标签列行数预算不再贴着今天最差的一格').toBe(MC_DT_MAX_LINES);
    expect(overWidth, '标签列横向越界 —— 词内断兜底没接住，标签会盖到右边的值上').toEqual([]);
    expect(brokeInside.sort(), '踩到词内断兜底的标签格变了（多/少/换都要重新看一眼）').toEqual(
      MC_DT_BREAK_INSIDE,
    );
    expect(overLines.length, `标签列行数超预算：\n${fmt(overLines)}`).toBe(0);
  });

  it('概览卡头标题 × 5 语种：同排尾件吃掉宽度后仍装得下（行数棘轮 + 词级不越界）', () => {
    const ov = mcOverviewKeys();
    const [hostsTitle, outboundsTitle] = ov.titles;
    const overLines: Over[] = [];
    const overWord: string[] = [];
    let n = 0;
    let worstLines = 0;
    for (const loc of LOCALES) {
      // 第一张卡的尾件是分段控件（三颗数字按钮，宽度与语种无关）；第二张是只读徽标（随语种变）。
      const badgeW = textPx(DICT[loc][ov.badge], { fontSize: mcPillFont }) + mcPillPadX + mcPillBorderX;
      for (const [key, tailW] of [
        [hostsTitle, MC_CHOICE_W],
        [outboundsTitle, badgeW],
      ] as const) {
        n++;
        const b: Box = {
          where: 'S16 概览卡头标题',
          avail: MC_CARD_INNER - mcCardHGap - tailW,
          type: { fontSize: mcCardHFont },
          wrap: wraps(CARD_H_SELS),
          breakAnywhere: breaksAnywhere(CARD_H_SELS),
          maxLines: MC_CARD_H_MAX_LINES,
        };
        const text = DICT[loc][key];
        const { lines, maxLineWidth } = layout(text, b.avail, b.type, b);
        worstLines = Math.max(worstLines, lines);
        if (maxLineWidth > b.avail + 0.001) overWord.push(`${loc}|${key}`);
        if (lines > b.maxLines)
          overLines.push({ where: b.where, loc, key, text, need: maxLineWidth, avail: b.avail, lines, budget: b.maxLines });
      }
    }
    expect(n).toBe(LOCALES.length * 2);
    // 正面断言：预算贴着今天最差的一格（fa/ru 三行），不是免检额度。
    expect(worstLines, '卡头标题行数预算不再贴着今天最差的一格').toBe(MC_CARD_H_MAX_LINES);
    expect(overWord, '卡头标题里有词比剩下那一截还宽 —— 会画到尾件下面').toEqual([]);
    expect(overLines.length, `概览卡头标题行数超预算：\n${fmt(overLines)}`).toBe(0);
  });
});

// ── S17 移动端导入预览（`forms.css` + `ImportFormPanel.tsx`）—— 2026-09-25 补 ─────────────────
//
// 移动端导入预览补齐了桌面 `ImportDialog` 预览块的计数三格与「不支持」徽标（⑨ 量的是桌面那份 `.imp-*`）。
// 这些文案在移动端落进的是**可折行的块**（`.m-form-hint` / `.m-form-err` / `.m-form-label`，
// 无 nowrap、无 ellipsis、纵向自由生长），所以只有两件事会破版：
//  · **词级越界** —— 一个比面板内容宽还宽的不可断单元横着画出去（与 S11 `.fld-l` 同形）；
//  · **徽标压名字** —— 节点清单每行是 `[名字 | 协议/徽标]` 的 `space-between` 横排，徽标是这一行的
//    附属，名字才是主体。判据取「徽标 ≤ 行宽的 1/3」，并把今天最宽的那一格钉成棘轮。
// 节点名与协议枚举是运行期数据，不测（文件头「射程之外 a.」）。

const FORMS_CSS = '../mobile/forms/forms.css' as const;
const formFont = (sel: string): number =>
  sel === '.m-form-btn' ? remPx(decl(REDESIGN_CSS, ':root', '--mobile-control-font'))
    : sel === '.m-form-label' ? remPx(decl(REDESIGN_CSS, '.m-form-label', 'font-size'))
      : remPx(decl(FORMS_CSS, sel, 'font-size'));
const importPanel = () => src('../mobile/forms/ImportFormPanel.tsx');
const mfPanelPadX = px(resolveShell(decl(REDESIGN_CSS, '.m-form-panel', 'padding-left'))) * 2; // 16px each
/** 面板内容宽 = 参考视口 − 面板左右内边距（`.m-form-row` 自身无内边距）。 */
const MF_CONTENT = MOBILE_REF_VIEWPORT_W - mfPanelPadX;
/** 预览块里每个键落在哪个类上（决定字号）；未登记的键在下面那条恰等断言里红。 */
const MF_IMPORT_SLOT: Readonly<Record<string, string>> = {
  'import.parsed': '.m-form-hint',
  'import.resultUnsupported': '.m-form-hint',
  'import.unsupportedTip': '.m-form-hint',
  'import.tailscaleJoinHint': '.m-form-hint',
  'import.skippedTitle': '.m-form-err',
  'import.nodesTitle': '.m-form-label',
  'import.unsupportedBadge': '.m-form-err',
};
/**
 * 节点清单行里名字轨的下限：今天最差 ru「Не поддерживается」模型 171.1px ⇒ 358 − 8 − 171.1 = 178.9。
 * 钉在 178 = 徽标再宽一格就自曝（同 ⑨ `IMP_NAME_MIN` 的语义）。
 */
const MF_NAME_MIN = 178;
const mfImportKeys = () =>
  keysOf(segment(importPanel(), '{preview !== null && (', '</FormSheet>', '移动端导入预览'), '导入预览文案键');

describe('S17 移动端导入预览（compact 390px 参考视口，五语种）', () => {
  it('几何链从 CSS 现场解出', () => {
    expect(mfPanelPadX).toBe(2 * pageInset);
    expect(MF_CONTENT).toBeCloseTo(M_PAGE_CONTENT, 5);
    for (const sel of new Set(Object.values(MF_IMPORT_SLOT)))
      expect(formFont(sel), `${sel} 字号读不到`).toBeGreaterThan(0);
  });

  it('本节量的键与 ImportFormPanel 预览块恰等，且五语种齐备', () => {
    const keys = mfImportKeys();
    expect([...keys].sort(), '预览块文案变了 —— 槽位表要重钉').toEqual(Object.keys(MF_IMPORT_SLOT).sort());
    for (const k of keys)
      for (const loc of LOCALES) expect(DICT[loc][k], `${loc} 缺键 ${k}`).toBeTruthy();
  });

  it('全部预览文案 × 5 语种：词级不越界（可折行块，一个词比面板还宽才是真溢出）', () => {
    const overWord: string[] = [];
    let n = 0;
    for (const loc of LOCALES)
      for (const [key, sel] of Object.entries(MF_IMPORT_SLOT)) {
        n++;
        const type = { fontSize: formFont(sel) };
        const opts = { wrap: wraps([sel]), breakAnywhere: breaksAnywhere([sel]) };
        const { maxLineWidth } = layout(DICT[loc][key], MF_CONTENT, type, opts);
        if (maxLineWidth > MF_CONTENT + 0.001) overWord.push(`${loc}|${key}|${maxLineWidth.toFixed(1)}`);
      }
    expect(n).toBe(LOCALES.length * Object.keys(MF_IMPORT_SLOT).length);
    // 前提：这几个类都可折行（回到 nowrap ⇒ 整句都要装进一行，本判据的口径随之作废）。
    for (const sel of new Set(Object.values(MF_IMPORT_SLOT))) expect(wraps([sel]), `${sel} 变成了 nowrap`).toBe(true);
    expect(overWord, '导入预览里有词比面板内容宽还宽 —— 会横着画出面板').toEqual([]);
  });

  it('「不支持」徽标 × 5 语种：名字轨不得被压到棘轮以下（名字才是这一行的主体）', () => {
    // 口径同 ⑨ `IMP_NAME_MIN`：没有物理墙，只有「够不够认出是哪条节点」，故钉在今天最差的一格。
    const font = remPx(decl(FORMS_CSS, '.m-form-err', 'font-size'));
    const gap = px(resolveShell(decl(FORMS_CSS, '.m-form-list li', 'column-gap'))); // var(--sp-2) = 8
    const tight: string[] = [];
    let narrowest = Infinity;
    for (const loc of LOCALES) {
      const badgeW = textPx(DICT[loc]['import.unsupportedBadge'], { fontSize: font });
      const nameW = MF_CONTENT - gap - badgeW;
      narrowest = Math.min(narrowest, nameW);
      if (nameW < MF_NAME_MIN) tight.push(`${loc} | 徽标 ${badgeW.toFixed(1)}px ⇒ 名字轨 ${nameW.toFixed(1)}px`);
    }
    expect(gap).toBe(8);
    // 正面：棘轮贴着今天最差的一格（再宽一格就自曝），不是一个怎么改都不会红的免检额度。
    expect(Math.floor(narrowest), '名字轨最窄档变了 —— 棘轮要重新看一眼').toBe(MF_NAME_MIN);
    expect(tight, '徽标过宽，节点名会被挤到看不清').toEqual([]);
  });
});

// ── S18 移动端表单：ζ 批新文案（导入文件腿 / 订阅创建恢复面）—— 2026-09-25 补 ─────────────────
//
// 三处新可见文案，落在两种盒子里：
//  · `.m-form-btn` —— 按钮**会折行**（无 nowrap），折行 = 按钮长高、与同排按钮高低不齐。
//    `import.pickFile` 独占一排（`.m-form-inline` 里只有它）；恢复面脚上进行中只有「取消添加」
//    一颗，待重试时「关闭 / 重试」两颗各占一半（`.m-form-foot` 是 flex + gap，`flex:1`）。
//    判据：词级不越界 + 按钮不折行（折行 = 同排按钮高低不齐）。
//  · `.m-form-hint` —— 常驻说明，纵向自由生长，只有词级越界会破版（同 S17）。
// 键从源码现场核对「真的在那一处渲染」，不是只信槽位表。

const subPanel = () => src('../mobile/forms/SubFormPanel.tsx');
const mfBtnPadX = padX(resolveShell(decl(FORMS_CSS, '.m-form-btn', 'padding'))); // horizontal padding remains 24
const mfBtnBorder = 2; // 1px solid 两侧
const mfFootGap = px(resolveShell(decl(FORMS_CSS, '.m-form-foot', 'column-gap'))); // var(--sp-2) = 8
/** 独占一排的按钮文字轨。 */
const MF_BTN_FULL = MF_CONTENT - mfBtnPadX - mfBtnBorder;
/** 脚上两颗并排时每颗的文字轨。 */
const MF_BTN_HALF = (MF_CONTENT - mfFootGap) / 2 - mfBtnPadX - mfBtnBorder;

interface MfSlot {
  readonly sel: '.m-form-btn' | '.m-form-hint';
  readonly avail: number;
  readonly maxLines: number;
  /** 这条键真的渲染在哪一段源码里（正面核对）。 */
  readonly source: () => string;
}
const MF_ZETA_SLOTS: Readonly<Record<string, MfSlot>> = {
  'import.pickFile': { sel: '.m-form-btn', avail: MF_BTN_FULL, maxLines: 1, source: () => segment(importPanel(), 'onClick={() => void pickFile()}', '</button>', '导入文件腿按钮') },
  // 进行中：脚上只有这一颗（无提交键）⇒ 独占一排。
  'sub.cancelCreate': { sel: '.m-form-btn', avail: MF_BTN_FULL, maxLines: 1, source: () => segment(subPanel(), 'cancelLabel={running', '\n', '恢复面脚键') },
  // 发布失败待重试：「关闭」（锁着）与「重试」并排各占一半。
  'common.close': { sel: '.m-form-btn', avail: MF_BTN_HALF, maxLines: 1, source: () => segment(subPanel(), 'cancelLabel={running', '\n', '恢复面脚键') },
  'common.retry': { sel: '.m-form-btn', avail: MF_BTN_HALF, maxLines: 1, source: () => segment(subPanel(), 'submitLabel={retry', '\n', '恢复面重试键') },
  'sub.createTaskRecoveredHint': { sel: '.m-form-hint', avail: MF_CONTENT, maxLines: Infinity, source: () => segment(subPanel(), 'export function SubCreateTaskPanel', '</FormSheet>', '恢复面') },
};

describe('S18 移动端表单 ζ 批新文案（compact 390px 参考视口，五语种）', () => {
  it('几何链从 CSS 现场解出', () => {
    expect(mfBtnPadX).toBe(24);
    expect(mfFootGap).toBe(8);
    expect(MF_BTN_FULL).toBeCloseTo(M_PAGE_CONTENT - 26, 5);
    expect(MF_BTN_HALF).toBeCloseTo((M_PAGE_CONTENT - 8) / 2 - 26, 5);
    for (const sel of ['.m-form-btn', '.m-form-hint'])
      expect(formFont(sel), `${sel} 字号读不到`).toBeGreaterThan(0);
  });

  it('每条键真的渲染在登记的那一处，且五语种齐备', () => {
    for (const [key, slot] of Object.entries(MF_ZETA_SLOTS)) {
      expect(keysOf(slot.source(), key), `${key} 不在登记的那段源码里 —— 槽位表过期了`).toContain(key);
      for (const loc of LOCALES) expect(DICT[loc][key], `${loc} 缺键 ${key}`).toBeTruthy();
    }
  });

  it('全部新文案 × 5 语种：词级不越界，按钮行数不超预算', () => {
    const over: string[] = [];
    let worstBtn = 0;
    let n = 0;
    for (const loc of LOCALES)
      for (const [key, slot] of Object.entries(MF_ZETA_SLOTS)) {
        n++;
        const type = { fontSize: formFont(slot.sel) };
        const opts = { wrap: wraps([slot.sel]), breakAnywhere: breaksAnywhere([slot.sel]) };
        const { lines, maxLineWidth } = layout(DICT[loc][key], slot.avail, type, opts);
        if (slot.sel === '.m-form-btn') worstBtn = Math.max(worstBtn, lines);
        if (maxLineWidth > slot.avail + 0.001) over.push(`${loc}|${key}|词宽 ${maxLineWidth.toFixed(1)} > ${slot.avail.toFixed(1)}`);
        if (lines > slot.maxLines) over.push(`${loc}|${key}|${lines} 行 > 预算 ${slot.maxLines}`);
      }
    expect(n).toBe(LOCALES.length * Object.keys(MF_ZETA_SLOTS).length);
    for (const sel of ['.m-form-btn', '.m-form-hint']) expect(wraps([sel]), `${sel} 变成了 nowrap`).toBe(true);
    // 正面：量到了真的按钮文字（最差一格恰为一行），不是对着空串恒绿。
    expect(worstBtn, '按钮文案一行都没量到 —— 取材面塌了').toBe(1);
    expect(over, '新文案在窄屏上破版').toEqual([]);
  });
});

// ── ⑩ 网络场景面板与规则弹窗「生效网络」（`rules.networkProfile.*`）：几何链 ─────────────────────
//
// 场景列表与场景编辑都是 `entry-form-dlg`（540px ⇒ 内容槽 FLD_AVAIL=502）；规则弹窗是基准 `.dlg`
// （460px ⇒ BASE_FLD_AVAIL=422）。探测方式是 `.seg2` 分段控件：`inline-flex` + 按钮 `nowrap`
// ⇒ 三档必须**单行**装进内容槽，超了就是整组按钮画出 `.dlg-body`（横向滚动条，同 S11 硬判据）。
const seg2GapPx = px(decl('./prototype.css', '.seg2', 'column-gap')); // 3
const seg2PadX = padX(decl('./prototype.css', '.seg2', 'padding')); // 3*2
const seg2BorderX = px(decl('./prototype.css', '.seg2', 'border-left-width')) * 2; // 1*2
const seg2BtnFont = px(decl('./prototype.css', '.seg2 button', 'font-size')); // 12.5
const seg2BtnPadX = padX(decl('./prototype.css', '.seg2 button', 'padding')); // 15*2
const errLineFont = px(decl('./components.css', '.err-line', 'font-size')); // 11
const warnLineFont = px(decl('./components.css', '.warn-line', 'font-size')); // 11
const warnLineIcon = px(decl('./components.css', '.warn-line svg', 'width')); // 14
const warnLineGap = px(decl('./components.css', '.warn-line', 'column-gap')); // 7

/**
 * ⑩ 网络场景（N3 新增的两个弹窗 + 规则弹窗新增的一个字段）。
 *
 * 量什么：字段标签（`.fld-l` / 开关标签）、说明行（`.card-sub`）、状态行（`.err-line` / `.warn-line`，
 * 含插值的按模板长度量，射程自曝 b.）、统一信息提示（`#tip`）、探测方式三档（`.seg2` 单行）。
 * 不量：列表行的 `.dns-resource-title/meta`（nowrap + ellipsis，且主体是场景名 / 判据值这些用户数据，
 * 射程自曝 a.）；规则行徽标「仅 {{name}}」（`.pill` 在可折行的 flex 容器里，主体是场景名）。
 *
 * 行数预算是**棘轮**（同 S11：`.dlg-body{overflow-y:auto}`，纵向不裁），钉在今天五语最差那一格。
 */
describe('⑩ 网络场景面板 / 规则弹窗「生效网络」（五语种）', () => {
  const NP = 'rules.networkProfile.';
  const LABELS_ENTRY = ['name', 'cidrs', 'domains', 'probe'].map((k) => NP + k);
  const LABEL_RULE = NP + 'ruleField';
  const SWT_LABEL = NP + 'enabled';
  const CARD_SUB = ['intro', 'criteriaHint', 'probeUses', 'probePending', 'probeUnknown'].map((k) => NP + k);
  const ERR_LINE = ['errName', 'errNoCriteria', 'errCidr', 'errDomain', 'deleteRefsWarn'].map((k) => NP + k);
  const TIPS = ['cidrsHint', 'domainsHint', 'probeHint', 'enabledHint', 'ruleFieldHint'].map((k) => NP + k);
  const SEG = ['probeAuto', 'probeSystem', 'probeDhcp'].map((k) => NP + k);
  // N4 命中态行（`.card-sub.np-match`：7px 圆点 + 6px gap + 文案）。
  const MATCH = ['matchIn', 'matchOut', 'matchUnknown'].map((k) => NP + k);
  const MATCH_DOT_AND_GAP =
    px(decl('./prototype.css', '.dot', 'width')) + px(decl('./screens.css', '.np-match', 'column-gap'));
  const labelWrap = wraps(FLD_LABEL_SELS);
  const box = (where: string, avail: number, fontSize: number, maxLines: number): Box => ({
    where,
    avail,
    type: { fontSize },
    wrap: true,
    breakAnywhere: false,
    maxLines,
  });
  const LABEL_BOX = (avail: number, where: string): Box => ({
    ...box(where, avail, fldLabelFont, labelWrap ? FLD_MAX_LINES.LABEL : 1),
    wrap: labelWrap,
    breakAnywhere: breaksAnywhere(FLD_LABEL_SELS),
  });
  /** 不可用原因插进 probeUnavailable 模板后的最长那句才是真实最坏值（插值不按模板量）。 */
  const unavailableTexts = (loc: Locale) =>
    ['reasonProfileInvalid', 'reasonDhcpNeedsPrivilege', 'reasonSystemNoSearchDomain', 'reasonDhcpMonitorMissing', 'reasonUnknown'].map((r) =>
      DICT[loc][NP + 'probeUnavailable'].replace('{{reason}}', DICT[loc][NP + r]),
    );
  // 棘轮：今天五语最差正好 3 行（模型口径，真机只会更少；实测 2026-09-25：说明行 / 错误行 / 提示行最差均为 3，
  // 标签 1 行，#tip 6 行）。ru/fa 的原因句最长。
  const CARD_SUB_MAX = 3;
  const ERR_LINE_MAX = 3;
  const WARN_LINE_MAX = 3;

  it('本节量的键仍真的被消费、且五语齐备（改名 / 删控件后本节不得继续量死键）', () => {
    // 「本机将使用」一行的文案函数 2026-09-25 搬进零 DOM 依赖的 `network-profile-probes.ts`（移动端规则屏
    // 共用同一份）——面板仍渲染它，取材面是两份文件的并集，不是换成其中一份。
    const panel =
      src('../components/screens/rules/NetworkProfilePanel.tsx') +
      src('../components/screens/rules/network-profile-probes.ts');
    expect(
      src('../components/screens/rules/NetworkProfilePanel.tsx').includes('probeDisplayText('),
      'NetworkProfilePanel 已不再渲染「本机将使用」一行 —— 取材面并进 probes 模块就不再成立',
    ).toBe(true);
    const rule = src('../components/dialogs/RuleDialog.tsx');
    const panelTips = TIPS.filter((k) => k !== NP + 'ruleFieldHint');
    for (const k of [...LABELS_ENTRY, SWT_LABEL, ...CARD_SUB, ...ERR_LINE, ...panelTips, ...SEG, NP + 'probeUnavailable']) {
      expect(panel.includes(`'${k}'`), `NetworkProfilePanel 已不再消费 ${k}`).toBe(true);
    }
    const domain = src('../domain/network-profile.ts');
    for (const k of ['ipv6DhcpWarn', 'reasonProfileInvalid', 'reasonDhcpNeedsPrivilege', 'reasonSystemNoSearchDomain', 'reasonDhcpMonitorMissing', 'matchIn', 'matchOut', 'matchUnknown'])
      expect(domain.includes(`'${NP}${k}'`), `domain/network-profile 已不再消费 ${NP}${k}`).toBe(true);
    expect(rule.includes(`'${LABEL_RULE}'`) && rule.includes(`'${NP}ruleFieldHint'`), 'RuleDialog 不再消费生效网络字段').toBe(true);
    for (const loc of LOCALES)
      for (const k of [...LABELS_ENTRY, LABEL_RULE, SWT_LABEL, ...CARD_SUB, ...ERR_LINE, ...TIPS, ...SEG, ...MATCH])
        expect(DICT[loc][k], `${loc} 缺键 ${k}`).toBeTruthy();
  });

  it('几何链从 CSS 现场解出', () => {
    expect([seg2GapPx, seg2PadX, seg2BorderX, seg2BtnFont, seg2BtnPadX]).toEqual([3, 6, 2, 12.5, 30]);
    expect([errLineFont, warnLineFont, warnLineIcon, warnLineGap]).toEqual([11, 11, 14, 7]);
    expect(MATCH_DOT_AND_GAP, '命中态行：圆点宽 + gap').toBe(13);
    expect(px(decl('./prototype.css', '.err-line', 'font-size')), '.err-line 两份副本分叉').toBe(errLineFont);
  });

  it('标签 / 说明 / 状态行 / 信息提示 × 5 语种装得下', () => {
    const over: Over[] = [];
    for (const loc of LOCALES) {
      for (const k of LABELS_ENTRY) check(over, LABEL_BOX(FLD_AVAIL, '⑩ 场景表单标签'), loc, k, DICT[loc][k]);
      check(over, LABEL_BOX(BASE_FLD_AVAIL, '⑩ 规则弹窗生效网络标签'), loc, LABEL_RULE, DICT[loc][LABEL_RULE]);
      check(
        over,
        { ...box('⑩ 启用开关标签', swtTextAvail(FLD_AVAIL), swtLabelFont, FLD_MAX_LINES.SWT_LABEL), wrap: labelWrap },
        loc,
        SWT_LABEL,
        DICT[loc][SWT_LABEL],
      );
      for (const k of CARD_SUB) check(over, box('⑩ 说明行 .card-sub', FLD_AVAIL, cardSubFont, CARD_SUB_MAX), loc, k, DICT[loc][k]);
      for (const k of MATCH)
        check(over, box('⑩ 命中态行 .card-sub.np-match', FLD_AVAIL - MATCH_DOT_AND_GAP, cardSubFont, CARD_SUB_MAX), loc, k, DICT[loc][k]);
      for (const k of ERR_LINE) check(over, box('⑩ 错误行 .err-line', FLD_AVAIL, errLineFont, ERR_LINE_MAX), loc, k, DICT[loc][k]);
      for (const text of unavailableTexts(loc))
        check(over, box('⑩ 不可用行 .err-line', FLD_AVAIL, errLineFont, ERR_LINE_MAX), loc, NP + 'probeUnavailable', text);
      check(
        over,
        box('⑩ IPv6 提示 .warn-line', FLD_AVAIL - warnLineIcon - warnLineGap, warnLineFont, WARN_LINE_MAX),
        loc,
        NP + 'ipv6DhcpWarn',
        DICT[loc][NP + 'ipv6DhcpWarn'],
      );
      for (const k of TIPS) check(over, box('⑩ 信息提示 #tip', tipAvail, tipFont, FLD_MAX_LINES.TIP), loc, k, DICT[loc][k]);
    }
    expect(over.length, `网络场景文案溢出：\n${fmt(over)}`).toBe(0);
  });

  it('探测方式三档 × 5 语种单行装进内容槽（.seg2 nowrap，超了整组画出弹窗）', () => {
    const rowW = (loc: Locale) =>
      SEG.reduce((sum, k) => sum + textPx(DICT[loc][k], { fontSize: seg2BtnFont }) + seg2BtnPadX, 0) +
      seg2GapPx * (SEG.length - 1) +
      seg2PadX +
      seg2BorderX;
    const bad = LOCALES.filter((loc) => rowW(loc) > FLD_AVAIL).map((loc) => `${loc} ${rowW(loc).toFixed(1)}px > ${FLD_AVAIL}px`);
    expect(bad).toEqual([]);
    // 阳性对照：三档各换成一句话，本判据必须抓到。
    const fat = 3 * (textPx('Read the current system DNS settings', { fontSize: seg2BtnFont }) + seg2BtnPadX);
    expect(fat).toBeGreaterThan(FLD_AVAIL);
  });

  it('阳性对照：超长说明与超宽不可断串都必须被抓到', () => {
    const bucket: Over[] = [];
    check(bucket, box('⑩ 说明行 .card-sub', FLD_AVAIL, cardSubFont, CARD_SUB_MAX), 'ru', `${NP}__probeLines`, 'слово '.repeat(200));
    check(bucket, LABEL_BOX(BASE_FLD_AVAIL, '⑩ 规则弹窗生效网络标签'), 'ru', `${NP}__probeWidth`, 'X'.repeat(200));
    expect(bucket.length, '合成缺陷没被抓到 —— 本节是装饰').toBe(2);
  });
});

// ── S19 移动端网络场景（`rules.networkProfile.*` / `mobileRules.networkProfile.*`）—— 2026-09-25 补 ─────
//
// 三处落点：
//  · **场景表单** `forms/NetworkProfileFormPanel.tsx` 与规则表单「生效网络」一行（`RuleFormPanel.tsx`）——
//    `.m-form-label` / `.m-form-hint` / `.m-form-err` 全部可折行、纵向自由生长 ⇒ 只有**词级越界**会破版（同 S17）；
//  · **探测方式三段** `.m-form-seg > button` —— 三颗 `flex:1` 等分面板内容宽，按钮文字可折行。
//    `.m-form-seg` 是 flex 且不改 `align-items`（缺省 stretch）⇒ 一颗折成两行时三颗一起变高、仍然等高，
//    不会高低不齐；真正破版的只有词级越界。故词级硬判 + 行数棘轮（钉在今天最差：ru「Системный DNS」两行）；
//  · **二级页引用计数角标** `.mr-pill`（`NetworkProfilesPage.tsx`，行里没有序号与动作簇 ⇒ 轨道 = 行内宽）——
//    插值是两个计数，按两位数填进去量（模板比实际短 = 漏报方向）。
// 插值为运行期用户数据的（`badge` 场景名 / `errCidr`·`errDomain` 的坏值）不测（文件头「射程之外 b.」）。

const npForm = () => src('../mobile/forms/NetworkProfileFormPanel.tsx');
const npSources = () =>
  [
    npForm(),
    src('../mobile/forms/RuleFormPanel.tsx'),
    src('../mobile/screens/rules/NetworkProfilesPage.tsx'),
    src('../mobile/screens/rules/RulesScreen.tsx'),
    src('../mobile/screens/rules/network-profile-copy.ts'),
    src('../components/screens/rules/network-profile-probes.ts'),
    // 告警文案键住在共享判据表（`PROBE_WARNING_KEYS`），移动端按后端给的码 `tr(warningKey)` 渲染。
    src('../domain/network-profile.ts'),
  ].join('\n');
const NPM = 'rules.networkProfile.';
const NP_FORM_LABELS = ['name', 'cidrs', 'domains', 'probe', 'enabled', 'ruleField'].map((k) => NPM + k);
const NP_FORM_HINTS = [
  ...['criteriaHint', 'cidrsHint', 'domainsHint', 'probeHint', 'enabledHint', 'ruleFieldHint', 'probePending', 'probeUnknown'].map(
    (k) => NPM + k,
  ),
];
const NP_FORM_ERRS = ['errName', 'errNoCriteria', 'ipv6DhcpWarn', 'badgeMissingTip'].map((k) => NPM + k);
const NP_SEG = ['probeAuto', 'probeSystem', 'probeDhcp'].map((k) => NPM + k);
/** 移动端可能出现的不可用原因（`dhcpNeedsPrivilege` 换成手机上那一句；其余与桌面同源）。 */
const NP_MOBILE_REASONS = [
  'mobileRules.networkProfile.reasonDhcpNoPermission',
  NPM + 'reasonProfileInvalid',
  NPM + 'reasonDhcpMonitorMissing',
  NPM + 'reasonUnknown',
];
const npSegBorderX = borderX(FORMS_CSS, '.m-form-seg');
const npSegBtnPadX = padX(resolveShell(decl(FORMS_CSS, '.m-form-seg > button', 'padding'))); // 0 var(--sp-2) → 16
const npSegBtnFont = remPx(decl(REDESIGN_CSS, ':root', '--mobile-control-font')); // 0.8125rem = 13
/** 三颗等分（`gap: 0`）后每颗的文字轨。 */
const NP_SEG_AVAIL = (MF_CONTENT - npSegBorderX) / 3 - npSegBtnPadX;
/** 棘轮：今天五语最差那一颗的行数（模型口径；ru「Системный DNS」在 ≈102px 的轨里折成两行）。 */
const NP_SEG_MAX_LINES = 2;
const NP_PILL_AVAIL = MR_ROW_INNER - mrPillPadX - mrPillBorderX;

describe('S19 移动端网络场景（compact 390px 参考视口，五语种）', () => {
  it('几何链从 CSS 现场解出', () => {
    expect(px(resolveShell(decl(FORMS_CSS, '.m-form-seg', 'column-gap')))).toBe(0);
    // 「折两行也等高」的前提：三颗在同一个 flex 行里、按缺省 stretch 拉齐（改了它，两行预算就要重判）。
    expect(decl(FORMS_CSS, '.m-form-seg', 'display')).toBe('flex');
    expect(rulesOf(FORMS_CSS).some((r) => selMatches(r.sel, '.m-form-seg') && /(?:^|;)\s*align-items\s*:/.test(r.body)),
      '`.m-form-seg` 改了 align-items —— 两行那颗不再与另两颗等高').toBe(false);
    expect([npSegBorderX, npSegBtnPadX, npSegBtnFont]).toEqual([2, 16, 13]);
    expect(NP_SEG_AVAIL).toBeCloseTo((M_PAGE_CONTENT - 2) / 3 - 16, 5);
    expect(NP_PILL_AVAIL).toBeCloseTo(332 - 16, 5);
  });

  it('本节量的键真的在移动端源码里被消费、且五语齐备', () => {
    const all = npSources();
    for (const k of [...NP_FORM_LABELS, ...NP_FORM_HINTS, ...NP_FORM_ERRS, ...NP_SEG, ...NP_MOBILE_REASONS, NPM + 'probeUses', NPM + 'probeUnavailable', NPM + 'refs', 'mobileRules.networkProfile.intro'])
      expect(all.includes(`'${k}'`), `移动端已不再消费 ${k} —— 本节在量死键`).toBe(true);
    // 正面：探测方式三段真的是 `.m-form-seg`（换成别的控件，本节的几何链就量错了对象）。
    expect(npForm()).toContain('className="m-form-seg"');
    for (const loc of LOCALES)
      for (const k of [...NP_FORM_LABELS, ...NP_FORM_HINTS, ...NP_FORM_ERRS, ...NP_SEG, ...NP_MOBILE_REASONS, 'mobileRules.networkProfile.intro', 'mobileRules.networkProfile.listEmpty'])
        expect(DICT[loc][k], `${loc} 缺键 ${k}`).toBeTruthy();
  });

  it('表单文案 × 5 语种：词级不越界（含把原因插进「本机不可用」之后的整句）', () => {
    const overWord: string[] = [];
    let n = 0;
    const measure = (loc: Locale, key: string, sel: string, text: string) => {
      n++;
      const type = { fontSize: formFont(sel) };
      const { maxLineWidth } = layout(text, MF_CONTENT, type, { wrap: wraps([sel]), breakAnywhere: breaksAnywhere([sel]) });
      if (maxLineWidth > MF_CONTENT + 0.001) overWord.push(`${loc}|${key}|${maxLineWidth.toFixed(1)}`);
    };
    for (const loc of LOCALES) {
      for (const k of NP_FORM_LABELS) measure(loc, k, '.m-form-label', DICT[loc][k]);
      for (const k of NP_FORM_HINTS) measure(loc, k, '.m-form-hint', DICT[loc][k]);
      for (const k of NP_FORM_ERRS) measure(loc, k, '.m-form-err', DICT[loc][k]);
      for (const src of ['sourceSystem', 'sourceDhcp'])
        measure(loc, `${NPM}probeUses/${src}`, '.m-form-hint', DICT[loc][NPM + 'probeUses'].replace('{{source}}', DICT[loc][NPM + src]));
      for (const r of NP_MOBILE_REASONS) {
        const tpl = DICT[loc][NPM + 'probeUnavailable'];
        expect(tpl, `${loc} 的 probeUnavailable 没有 {{reason}} 占位符 —— 插值填不进去会按模板少测`).toContain('{{reason}}');
        const sentence = tpl.replace('{{reason}}', DICT[loc][r]);
        measure(loc, `${NPM}probeUnavailable/${r}`, '.m-form-err', sentence);
        // DHCP 那一档置灰时的说明行：「DHCP · 本机不可用：…」。
        measure(loc, `dhcpBlocked/${r}`, '.m-form-hint', `${DICT[loc][NPM + 'probeDhcp']} · ${sentence}`);
      }
    }
    expect(n).toBe(LOCALES.length * (NP_FORM_LABELS.length + NP_FORM_HINTS.length + NP_FORM_ERRS.length + 2 + 2 * NP_MOBILE_REASONS.length));
    for (const sel of ['.m-form-label', '.m-form-hint', '.m-form-err']) expect(wraps([sel]), `${sel} 变成了 nowrap`).toBe(true);
    expect(overWord, '网络场景表单里有词比面板内容宽还宽 —— 会横着画出面板').toEqual([]);
  });

  it('探测方式三段 × 5 语种：词级不越界 + 行数棘轮（折行 = 三颗高低不齐）', () => {
    const over: Over[] = [];
    let worst = 0;
    for (const loc of LOCALES)
      for (const k of NP_SEG) {
        const opts = { wrap: wraps(['.m-form-seg > button']), breakAnywhere: breaksAnywhere(['.m-form-seg > button']) };
        const { lines } = layout(DICT[loc][k], NP_SEG_AVAIL, { fontSize: npSegBtnFont }, opts);
        worst = Math.max(worst, lines);
        check(
          over,
          { where: 'S19 探测方式三段', avail: NP_SEG_AVAIL, type: { fontSize: npSegBtnFont }, ...opts, maxLines: NP_SEG_MAX_LINES },
          loc,
          k,
          DICT[loc][k],
        );
      }
    expect(worst, '三段按钮行数预算不再贴着今天最差的一颗').toBe(NP_SEG_MAX_LINES);
    expect(over.length, `探测方式三段装不下：\n${fmt(over)}`).toBe(0);
    // 阳性对照：一颗换成一句话，本判据必须抓到。
    const fat: Over[] = [];
    check(fat, { where: 'S19 阳性对照', avail: NP_SEG_AVAIL, type: { fontSize: npSegBtnFont }, wrap: true, breakAnywhere: false, maxLines: NP_SEG_MAX_LINES }, 'ru', 'probe', 'Читать текущие системные настройки DNS');
    expect(fat.length, '合成缺陷没被抓到 —— 本节是装饰').toBe(1);
  });

  it('二级页命中态行（N4）× 5 语种：扣掉圆点与间距后词级不越界', () => {
    // `.mr-np-match` 是 flex 行：圆点一项、文案一个匿名 flex 项（min-width:auto ⇒ 最长的词决定它最窄能缩到多少）。
    // 行数不设棘轮（`.mr-note` 纵向自由，见 S15 头注 v.），只判「最长的词装不装得下」。
    const dotW = px(decl(RULES_CSS, '.mr-row .dot', 'width'));
    const gap = px(decl(RULES_CSS, '.mr-np-match', 'column-gap'));
    expect([dotW, gap]).toEqual([7, 6]);
    expect(decl(RULES_CSS, '.mr-np-match', 'display')).toBe('flex');
    const avail = MR_ROW_INNER - dotW - gap;
    const type = { fontSize: remPx(decl(RULES_CSS, '.mr-note', 'font-size')) };
    const keys = ['matchIn', 'matchOut', 'matchUnknown'].map((k) => NPM + k);
    const all = npSources();
    for (const k of keys) expect(all.includes(`'${k}'`), `移动端已不再消费 ${k}`).toBe(true);
    const over: string[] = [];
    let n = 0;
    for (const loc of LOCALES)
      for (const k of keys) {
        n++;
        expect(DICT[loc][k], `${loc} 缺键 ${k}`).toBeTruthy();
        const { maxLineWidth } = layout(DICT[loc][k], avail, type, { wrap: true, breakAnywhere: false });
        if (maxLineWidth > avail + 0.001) over.push(`${loc}|${k}|${maxLineWidth.toFixed(1)}`);
      }
    expect(n).toBe(LOCALES.length * keys.length);
    expect(over, '命中态文案里有词比「行宽 − 圆点 − 间距」还宽').toEqual([]);
    // 阳性对照：一个超长的词必须被抓到。
    const fat = layout('Ж'.repeat(60), avail, type, { wrap: true, breakAnywhere: false });
    expect(fat.maxLineWidth, '合成缺陷没被抓到 —— 本条是装饰').toBeGreaterThan(avail);
  });

  it('二级页引用计数角标 × 5 语种：横向不越界 + 两行以内（按两位数填插值）', () => {
    const over: Over[] = [];
    let n = 0;
    for (const loc of LOCALES) {
      const tpl = DICT[loc][NPM + 'refs'];
      expect(tpl, `${loc} 的 refs 占位符不全 —— 插值填不进去会按模板少测`).toMatch(/\{\{route\}\}[\s\S]*\{\{dns\}\}|\{\{dns\}\}[\s\S]*\{\{route\}\}/);
      for (const [key, text] of [
        [NPM + 'refs', tpl.replace('{{route}}', '12').replace('{{dns}}', '34')],
        [NPM + 'disabledBadge', DICT[loc][NPM + 'disabledBadge']],
      ] as const) {
        n++;
        check(
          over,
          { where: 'S19 场景行角标', avail: NP_PILL_AVAIL, type: { fontSize: mrPillFont }, wrap: wraps(['.mr-pill']), breakAnywhere: breaksAnywhere(['.mr-pill']), maxLines: 2 },
          loc,
          key,
          text,
        );
      }
    }
    expect(n).toBe(LOCALES.length * 2);
    expect(over.length, `场景行角标溢出：\n${fmt(over)}`).toBe(0);
  });
});
