/**
 * 字段描述符与草稿的**平台无关纯数据层** —— 从 `FieldSpec.tsx` 里拆出来的那一半。
 *
 * # 为什么要拆（2026-09-06，移动端表单宿主落地时）
 *
 * `FieldSpec.tsx` 一个文件里住着两样东西：**字段规格**（类型 + 三个纯函数，与平台无关）与
 * **桌面渲染器**（`FieldRenderer` / `FormFields` / `FormSection` / `FormTabs`，类名对齐
 * `components.css`、控件走 `Csel`/`Fold`/`InfoIcon`）。移动端要复用的只有前者：呈现层必须重写
 * （桌面那套外观住在 `prototype.css` / `components.css` = 桌面层叠链，契约 A1 禁入移动端）。
 *
 * 而 `nodes-screen.test.tsx` ② 的 A 腿边界断言按**扩展名**判：`.tsx` = 桌面屏组件，一律不许被
 * 移动端 import（理由是「它内部的控件会藏到 A 腿的源码取材面之外」）。规格与渲染器同住一个
 * `.tsx` 时，移动端要么整份引进来（把 `Csel`/`Modal`/`Fold` 一起拖进移动包，且违背那条边界的**意图**），
 * 要么在移动端重抄一份 `parseNumberField` —— 而那正是 `FieldSpec.tsx` 头注写着「**全库唯一实现点
 * （R2）**」的那个函数，抄一份等于给「空串该不该压成 0」造第二个答案。
 *
 * 拆开之后两侧各自成立：本文件**零 import**（连 react 都不认识），两个客户端共用；
 * `FieldSpec.tsx` 只剩渲染器，并把本文件的公共面**原样再导出**，故桌面既有的 20 余个
 * `from './FieldSpec'` 调用点一个字都不用改。
 */

/** 表单草稿值域：文本/数值/开关/未填。number 空 = undefined（R2）。 */
export type FormValue = string | number | boolean | undefined;

/**
 * select 的一个选项：`[值, 文案]`，可选第三位 = **不可选**（省略 ⇒ 可选）。
 *
 * 为什么是「可选第三元素」而不是改成 `{value,label,disabled}` 对象：`Csel` 早就支持
 * `disabled`（点击拦截 `Csel.tsx:172`、键盘跳过 `:241`、样式与 `aria-disabled` `:315/318`），
 * 唯一断点就是本层把选项拍平成了二元组、表达不出禁用。而二元组字面量对
 * `[string, string, boolean?]` 天然可赋值 ⇒ 全仓 22 处 select 调用点的选项字面量**一处都没改**
 * （实测 `tsc --noEmit` 全绿），要禁用的那一处多写一位即可。改成对象则要么全量改写、
 * 要么两种形状并存。
 */
export type SelectOption = readonly [value: string, label: string, disabled?: boolean];

/** 表单草稿：键（FieldSpec.k）→ 值。协议特定字段的扁平袋，protoCodec 在此与 ServerConfig 往返。 */
export type FormValues = Record<string, FormValue>;

/** 字段描述符公共部分。`when` = 显隐谓词（通用：节点的 tls/reality 条件、规则的类型条件都用它）。 */
interface FieldBase {
  /** 草稿键（= protoCodec 读写的键）。 */
  k: string;
  /** 标签 i18n key。 */
  label: string;
  /** 显隐谓词（返回 false = 该字段在当前草稿下隐藏）。缺省恒显。 */
  when?: (values: FormValues) => boolean;
  /** 「可选」徽标。 */
  opt?: boolean;
  /**
   * 字段说明：所有字段类型统一收进标签后的 `InfoIcon`，不在控件下方常驻铺开。
   *
   * 加这一支最初是因为「选了会怎样」有时**不能只靠标签表达**：endpoint 的前置代理是实例——
   * WireGuard 的握手走 UDP，前置代理不支持 UDP 转发就**静默不通**（不回落直连，见
   * `crates/config-engine/src/singbox/endpoint.rs` 的实测），而 Tailscale 那侧只需 TCP。
   * 两句话不同、都必须能从控件旁到达，否则用户只能靠试。
   *
   * 提到 `FieldBase` 是因为 text/textarea 也有同样的需求，而此前它们**没有说明位**，于是说明
   * 只能塞进标签：`node.field.h2Host` = 「HTTP/2 Host（逗号分隔，留空回落 SNI/节点地址）」。
   * 这不是排版偏好问题 —— 标签是控件的**名字**，`styles/text-fit.test.ts` 给 `.fld-l` 定的 2 行预算
   * 正是这条判据的具象（占到第 3 行就说明它其实是一句说明），而这四条恰恰是把预算刚好用满的那批。
   * 「不为两个字段去扩 union」的旧结论（`node-spec.ts` h2 段的原注释）在字段涨到 4 条、且行数预算
   * 把它顶出来之后不再成立。
   *
   * ⚠️ 移动端**不用** `InfoIcon`（触屏没有 hover，§4.12）：同一条 `hint` 在
   * `mobile/forms/FormFields.tsx` 里渲染成控件下方的常驻一行。描述符两端共用，落点两端不同。
   */
  hint?: string;
}

/**
 * 字段描述符（discriminated union，§1.4）。渲染器按 `t` 穷尽 switch。
 * 新增字段类型 → 加一支 union + 一个 case（never 兜底保证补全）。
 *
 * ⚠️ 今天有**两个**渲染器消费它（桌面 `FieldSpec.tsx#FieldRenderer` 与移动
 * `mobile/forms/FormFields.tsx#MobileField`）。加一支 union 时两处的 `never` 兜底会同时编译不过 ——
 * 那正是要的：一个新字段类型在某个客户端上「静默不渲染」比编译失败坏得多。
 */
export type FieldSpec =
  | (FieldBase & { t: 'text'; ph?: string; mono?: boolean; secret?: boolean })
  | (FieldBase & { t: 'number'; ph?: string; mono?: boolean })
  | (FieldBase & { t: 'textarea'; ph?: string; mono?: boolean; rows?: number; secret?: boolean })
  | (FieldBase & { t: 'select'; options: readonly SelectOption[] })
  | (FieldBase & {
      t: 'switch';
      /**
       * 禁用态 —— **静态布尔，不是谓词**，由构表处算好传进来（同 `SelectOption` 第三位 `disabled`
       * 那条既定形态）。
       *
       * 为什么不做成 `when` 那样的谓词：`when` 是**调用方** filter 掉的
       * （`spec.filter(f => !f.when || f.when(draft))`），而 `FieldRenderer` 只收到单个 spec，
       * 拿不到整份草稿。要谓词就得再给渲染器传一个 `values` prop —— 那样「某个调用点忘了传」会把
       * 禁用**静默退化成可用**，而这个开关禁用与否是阻断级的（见 WarpDialog 的 `advSpec`）。
       * 静态值没有这条退化路径：表里写了就是写了。
       */
      disabled?: boolean;
      /**
       * 禁用时**取代** `hint` 的说明（讲「为什么不能开」）。缺省 ⇒ 仍显示 `hint`。
       * 之所以是取代而非追加：`hint` 描述的是开启后的行为，而那件事在禁用场景下结构上永远不会发生，
       * 照显等于对着一个拨不动的开关解释它拨动后会怎样。
       */
      disabledHint?: string;
    });

/**
 * number 字段解析 —— **全库唯一实现点（R2）**。
 *  - 空串（含纯空白）→ `undefined`：允许退格删空重录，不被压成 0；
 *  - 非空 → 十进制解析；解析失败（NaN/Infinity）→ `undefined`，**绝不硬塞 0**。
 * 抽为纯函数供 NumberField 分支与 NodeDialog 的 port 字段共用（单一逻辑），并入 vitest。
 */
export function parseNumberField(raw: string): number | undefined {
  const s = raw.trim();
  if (s === '') return undefined;
  const n = Number(s);
  return Number.isFinite(n) ? n : undefined;
}

/**
 * 从 FieldSpec 列表构造初始草稿（新增态默认）：select→首选项、switch→false、number→undefined、text→''。
 * fromConfig 在此之上覆盖存量值（编辑态），保证每个键都有合法默认、缺省不漏键。
 */
export function draftFromSpecs(specs: readonly FieldSpec[]): FormValues {
  const d: FormValues = {};
  for (const f of specs) {
    if (f.t === 'select') d[f.k] = f.options[0]?.[0] ?? '';
    else if (f.t === 'switch') d[f.k] = false;
    else if (f.t === 'number') d[f.k] = undefined;
    else d[f.k] = '';
  }
  return d;
}

/**
 * 归一化后的 select 选项 —— 渲染器无关的 `{值, 已译文案, 是否禁用}`。
 *
 * 桌面 `Csel` 的 `CselOption` 是它的超集（多 `description` / `icon` 两个只有自绘浮层才画得出的位），
 * 故 `NormalizedSelectOption[]` 可直接赋给 `CselOption[]`；移动端用原生 `<option>` 消费同一份。
 */
export interface NormalizedSelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

/**
 * `SelectOption[]` → 归一化选项 —— **两个客户端的唯一实现点**。
 *
 * # 为什么住在这里而不是各渲染器里（2026-09-06 复审 minor）
 *
 * 这里面有**两条判据**，此前桌面 `FieldSpec.tsx#toCselOptions` 与移动端 `FormFields.tsx` 各写了一份
 * （连正则都是逐字抄的）：
 *
 *  1. **哪些 label 要翻译** —— 点分键才过 `translate`；`TCP` / `xtls-rprx-vision` 这类专有名词是
 *     字面量。判据是那条 `^[A-Za-z0-9_]+(\.[A-Za-z0-9_]+)+$`。
 *  2. **未知当前值保留** —— 选项集是**前端选的展示档位**，而磁盘上的值域由 sing-box/后端拥有且更宽
 *     （ss `method`、uTLS `fingerprint`、`vmessSecurity` 都是开放集，Rust 侧就是 `String`）。
 *     存量/订阅节点的值落在表外时若照直渲染，下拉是空选中态，用户**一碰就被迫改成表内某档** ——
 *     静默改坏一个本来能用的节点，且没有撤销入口。故当前值不在表里就并入首位（值即文案）。
 *     空串不并入：它是「未设置」而非未知取值，且多张表里 `''` 本身就是合法首项（flow=none / bbr=默认）。
 *
 * 抄一份的具体后果不是审美：哪天桌面把标签判据放宽（允许带连字符的 key，或改成显式 `i18nKey` 标记），
 * 移动端那份不会跟着改 ⇒ **同一张 `ND_SPEC` 在两个客户端上给出两种标签**，而这类分叉本仓没有任何门
 * （契约 A1 只管「有没有 import 桌面 .tsx」，不管「有没有抄一份」）。
 *
 * 抽成纯函数还有一层：本仓 vitest 是 `environment:'node'`（无 jsdom），两侧渲染器都渲染不动 ⇒
 * 这条映射内联在组件里时「少映一个字段」不会被任何门发现（`disabled` 正是这么一路断在这一层的）。
 */
export function normalizeSelectOptions(
  options: readonly SelectOption[],
  current?: FormValue,
  translate: (key: string) => string = (key) => key,
): NormalizedSelectOption[] {
  const opts = options.map(([value, label, disabled]) => ({
    value,
    label: /^[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)+$/.test(label) ? translate(label) : label,
    disabled,
  }));
  if (typeof current === 'string' && current !== '' && !opts.some((o) => o.value === current)) {
    opts.unshift({ value: current, label: current, disabled: undefined });
  }
  return opts;
}
