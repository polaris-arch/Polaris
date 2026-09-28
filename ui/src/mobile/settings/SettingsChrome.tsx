import { MobileInfo } from '../MobileInfo';
import { MobileSelect as AppSelect } from '../MobileSelect';
/**
 * 移动端设置屏的呈现原语（F4）。
 *
 * # 样式分工
 *
 * 设置行的布局与阅读间距在这里用内联常量维护；`settings.css` 负责根页分栏，
 * 共享按钮/选择面的按压与焦点态在 `mobile.css`。内联属性不进入 CSS 特性测试的取材面，
 * 因此本文件只使用 Chromium ≤105 已支持的基础布局属性。
 *
 * # `data-tip` 债在这里还（IA §4.12）
 *
 * 桌面把大量「为什么不能」的解释挂在 `InfoIcon` 的悬浮提示上。触屏没有 hover ⇒ 移动端保留关键约束与当前影响的常驻摘要；明确 opt-in 的静态机制说明可通过 i 打开全文。
 * 错误、禁用原因与动态状态不经过这条折叠通道。
 */

import { useEffect, useId, useRef, useState, type CSSProperties, type ReactElement, type ReactNode } from 'react';
import { nextDraft, parseBulkEntries, sameEntries } from '@/domain/list-entries';
import { useWriteError } from './write-feedback';

/* ── 尺度：几何 px（不随 Dynamic Type 缩放），排版 rem（随缩放）── 与 mobile.css 头注同一口径 ── */

const CARD: CSSProperties = {
  background: 'hsl(var(--surface))',
  border: '1px solid hsl(var(--line))',
  /* 移动端卡角 = `radius.cardMobile`，而它在 kit 里是 `alias:radius.lg`（=14），**不是** `--r`（11）。
     契约 D-3 只要求「引用、不复制字面量」，故这里直接引 `--r-lg`；把那层 mobile 专有别名
     （`--r-card-mobile: var(--r-lg)`，原型 :16 有）补进 `styles/tokens.resolved.css` 属 token 入口面，
     不在本屏射程内 —— 补了之后本行换成引它即可，渲染值不变。 */
  borderRadius: 'var(--r-lg)',
  padding: 'var(--sp-3)',
  boxSizing: 'border-box',
  display: 'flex',
  flexDirection: 'column',
  gap: '2px',
};

const CARD_HEADER: CSSProperties = {
  color: 'hsl(var(--fg-dim))',
  fontSize: '0.75rem',
  fontWeight: 600,
  letterSpacing: '0.02em',
  paddingBottom: '6px',
};

/* 行用 48px 命中下限，辅助信息随文字自然换行增高；宽屏不为留白放大空行。 */
const ROW: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: '12px',
  minHeight: 'var(--tap-min)',
  boxSizing: 'border-box',
  padding: '6px 0',
  borderTop: '1px solid hsl(var(--hair))',
};

const ROW_FIRST: CSSProperties = { ...ROW, borderTop: 'none' };

const ROW_TEXT: CSSProperties = {
  flex: '1 1 auto',
  minWidth: 0,
  display: 'flex',
  flexDirection: 'column',
  gap: '2px',
};

const ROW_LABEL: CSSProperties = {
  fontSize: '0.875rem',
  fontWeight: 500,
  lineHeight: 1.35,
  color: 'hsl(var(--fg))',
};

const ROW_DESC: CSSProperties = {
  fontSize: '0.75rem',
  lineHeight: 1.5,
  color: 'hsl(var(--fg-dim))',
};

const CONTROL_SLOT: CSSProperties = { flex: '0 0 auto', display: 'flex', alignItems: 'center', gap: '8px' };

/** 控件放不进标题行时的整行堆叠（spec「compact」第二条）。 */
const ROW_STACKED: CSSProperties = { ...ROW, flexDirection: 'column', alignItems: 'stretch' };

export interface SettingsRowProps {
  /** 稳定标识：判据按它定位一行，不靠文案（文案会被翻译）。 */
  id: string;
  label: ReactNode;
  /**
   * 常驻说明。桌面同一句话住在悬浮提示里；移动端**必须**是常驻第二行（IA §4.12），
   * 故这里没有「hover 才显示」的形态可选。
   */
  desc?: ReactNode;
  /** Full static explanation; desc remains the explicit decision summary. */
  descDetails?: ReactNode;
  descTitle?: string;
  /** 状态/校验行（红色），与 `desc` 并存：`desc` 说的是这项是什么，本行说的是它现在怎么了。 */
  problem?: ReactNode;
  /**
   * 约束/条件说明（**非错误色**）。桌面把这一类挂在控件的 `tip` 上（`Switch` 的 `tip` 槽），
   * 触屏没有 hover ⇒ 与 `desc` 同法落成常驻行（§4.12）。
   *
   * 与 `problem` 分开是因为**两者不是同一件事**：`problem` 说「这一项现在是错的」（DNS 地址非法、
   * 超时值越界），`hint` 说「这一项现在按规则做不了」（竞速额度已满）。配额用满是配置的正常状态，
   * 不是这一屏出了错 —— 用 `--err` 画它会与真正的校验错误长成同一个视觉等级，把人引去排查故障。
   * 桌面同一句（`settings.dns.raceQuotaReached`）走的也是非错误的 `tip`，本槽与它同调。
   */
  hint?: ReactNode;
  /**
   * ⚠️ 写失败提示**不是 prop**：它由本组件按 `id` 从 `WriteErrorsContext` 自取（见 `write-feedback.ts`）。
   * 做成 prop 就等于要求每一页在每一行手动接一次 —— 「忘一处就静默一处」，而那正是裁定 #14
   * 要消掉的那个类别。
   */
  control?: ReactNode;
  /** 控件与标题不同行（长输入框、分段控件）。 */
  stacked?: boolean;
  first?: boolean;
}

export function SettingsRow({
  id,
  label,
  desc,
  descDetails,
  descTitle,
  problem,
  hint,
  control,
  stacked,
  first,
}: SettingsRowProps): ReactElement {
  const base = stacked ? ROW_STACKED : first ? ROW_FIRST : ROW;
  const style = stacked && first ? { ...base, borderTop: 'none' } : base;
  const writeError = useWriteError(id);
  return (
    <div style={style} data-setting={id}>
      <div style={ROW_TEXT}>
        <span style={ROW_LABEL}>{label}</span>
        {desc !== undefined && (descDetails === undefined ? <span style={ROW_DESC}>{desc}</span> :
          <div style={ROW_DESC}><MobileInfo title={descTitle ?? (typeof label === 'string' ? label : '')} summary={desc} details={descDetails} /></div>)}
        {/* 顺序刻意是 `hint` 在前、`problem` 在后：两条并存时，红字是更急的那一条，压在最下面
            紧贴控件。`data-hint` 只是判据的定位锚，不进 `rows()` 的切行标记表（它不是一行）。 */}
        {hint !== undefined && (
          <span data-hint={id} style={ROW_DESC}>
            {hint}
          </span>
        )}
        {problem !== undefined && (
          <span style={{ ...ROW_DESC, color: 'hsl(var(--err))' }}>{problem}</span>
        )}
        {/* 写失败：紧贴这颗控件的行内提示。它与 `problem` 并存而不是二选一 ——
            「这个值不合法」与「这次没存进去」是两件不同的事，盖掉任一条都会让人修错方向。 */}
        {writeError !== undefined && (
          <span data-write-error={id} style={{ ...ROW_DESC, color: 'hsl(var(--err))' }}>
            {writeError}
          </span>
        )}
      </div>
      {control !== undefined && (
        <div style={stacked ? { ...CONTROL_SLOT, paddingTop: '8px' } : CONTROL_SLOT}>{control}</div>
      )}
    </div>
  );
}

export function SettingsGroup({
  header,
  children,
}: {
  header?: ReactNode;
  children: ReactNode;
}): ReactElement {
  return (
    /* `ms-card` 是给 `settings.css` 的列流用的钩子（`break-inside: avoid` + 断点档的纵向节奏）；
       卡片自身的外观仍然全部内联，见文件头注。 */
    <section className="ms-card" style={CARD}>
      {header !== undefined && <div style={CARD_HEADER}>{header}</div>}
      {children}
    </section>
  );
}

/**
 * 常驻说明段（能力缺席登记的用户可见那一半）。
 *
 * `tone='warn'` 用于「这项没有做/做不到」，`tone='plain'` 用于中性说明。**没有 `error` 档**：
 * 缺席不是错误，用错误色会让用户以为出了故障。
 */
export function SettingsNote({
  id,
  tone = 'plain',
  children,
  summary,
  title,
}: {
  id: string;
  tone?: 'plain' | 'warn';
  children: ReactNode;
  summary?: ReactNode;
  title?: string;
}): ReactElement {
  if (summary !== undefined) return (
    <div data-note={id} style={{ margin: 0, padding: '8px 0 0', fontSize: '0.75rem', lineHeight: 1.55, color: tone === 'warn' ? 'hsl(var(--warn))' : 'hsl(var(--fg-dim))' }}><MobileInfo title={title ?? ''} summary={summary} details={children} tone={tone} /></div>
  );
  return (
    <p
      data-note={id}
      style={{
        margin: 0,
        padding: '8px 0 0',
        fontSize: '0.75rem',
        lineHeight: 1.55,
        color: tone === 'warn' ? 'hsl(var(--warn))' : 'hsl(var(--fg-dim))',
      }}
    >
      {children}
    </p>
  );
}

/* ── 控件 ─────────────────────────────────────────────────────────────────── */

const SWITCH_W = 44;
const SWITCH_H = 26;

/*
 * 命中盒扩到 `--tap-min`（48），**视觉一格不动** —— IA §3.1 #11 的原话是「48 × 48 hit area around an
 * unchanged visual」，`core.json` 又把 touch target 列进「不随 Dynamic Type 缩放」并给了理由
 * 「48 是手指，不是字母」。故要扩的是可点区，不是把开关画大。
 *
 * 手法是**负外边距**：它只抵消布局占位，不裁命中盒 ⇒ 按钮的边框盒是 48×48（点得中），而它在行里
 * 仍只占 44×26（行不被撑高、轨道不被画大）。溢出的 11px 落在行的 48px 最小高度内；
 * 长说明只会增加可用高度。
 *
 * 伪元素扩热区那条路内联样式走不了（写不了 `::before`），故轨道另起一层 `<span>`：按钮只留命中盒 +
 * 复位，轨道与圆钮的视觉整体下移到那一层。
 */
const HIT_INSET_Y = `calc((${SWITCH_H}px - var(--tap-min)) / 2)`;
const HIT_INSET_X = `calc((${SWITCH_W}px - var(--tap-min)) / 2)`;

/**
 * 开关。`role="switch"` 的原生 `<button>`（不是 `<input type=checkbox>`）：后者的视觉必须靠
 * `appearance:none` + 伪元素重画，而伪元素内联写不了。
 */
export function MobileSwitch({
  checked,
  onChange,
  ariaLabel,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  ariaLabel: string;
  disabled?: boolean;
}): ReactElement {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        minWidth: 'var(--tap-min)',
        minHeight: 'var(--tap-min)',
        margin: `${HIT_INSET_Y} ${HIT_INSET_X}`,
        padding: 0,
        border: 0,
        background: 'none',
        opacity: disabled ? 0.5 : 1,
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'center',
        boxSizing: 'border-box',
        cursor: disabled ? 'default' : 'pointer',
        touchAction: 'manipulation',
      }}
    >
      {/* 轨道：视觉与改动前逐值相同（44×26、3px 内边距、18px 圆钮、140ms 背景过渡）。 */}
      <span
        aria-hidden="true"
        style={{
          width: `${SWITCH_W}px`,
          height: `${SWITCH_H}px`,
          padding: '3px',
          border: '1px solid hsl(var(--line))',
          borderRadius: `${SWITCH_H}px`,
          background: checked ? 'hsl(var(--flow))' : 'hsl(var(--surface-2))',
          display: 'flex',
          justifyContent: checked ? 'flex-end' : 'flex-start',
          alignItems: 'center',
          boxSizing: 'border-box',
          transition: 'background 140ms ease',
        }}
      >
        <span
          style={{
            width: '18px',
            height: '18px',
            borderRadius: '50%',
            background: 'hsl(var(--bg))',
            display: 'block',
          }}
        />
      </span>
    </button>
  );
}

const FIELD: CSSProperties = {
  minHeight: 'var(--tap-min)',
  boxSizing: 'border-box',
  padding: '8px 10px',
  border: '1px solid hsl(var(--line))',
  borderRadius: 'var(--r-sm)',
  background: 'hsl(var(--surface-2))',
  color: 'hsl(var(--fg))',
  fontFamily: 'inherit',
  fontSize: '0.875rem',
  width: '100%',
};

export function MobileSelect({
  value,
  onChange,
  ariaLabel,
  children,
  disabled,
}: {
  value: string;
  onChange: (next: string) => void;
  ariaLabel: string;
  children: ReactNode;
  disabled?: boolean;
}): ReactElement {
  return (
    <AppSelect
      value={value}
      aria-label={ariaLabel}
      disabled={disabled}
      onChange={(e) => onChange(e.currentTarget.value)}
      style={{ ...FIELD, minWidth: '150px' }}
    >
      {children}
    </AppSelect>
  );
}

export function MobileTextInput({
  value,
  onChange,
  onCommit,
  ariaLabel,
  placeholder,
  inputMode,
  type,
  invalid,
  mono,
}: {
  value: string;
  onChange: (next: string) => void;
  /** 失焦/回车提交。逐键写盘会让端口/URL 的中间态真的落进配置（桌面同款理由）。 */
  onCommit: () => void;
  ariaLabel: string;
  placeholder?: string;
  inputMode?: 'numeric' | 'url' | 'text';
  type?: 'text' | 'password';
  invalid?: boolean;
  mono?: boolean;
}): ReactElement {
  return (
    <input
      type={type ?? 'text'}
      value={value}
      aria-label={ariaLabel}
      aria-invalid={invalid || undefined}
      placeholder={placeholder}
      inputMode={inputMode}
      onChange={(e) => onChange(e.currentTarget.value)}
      onBlur={onCommit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur();
      }}
      style={{
        ...FIELD,
        fontFamily: mono ? 'var(--mono)' : 'inherit',
        borderColor: invalid ? 'hsl(var(--err))' : 'hsl(var(--line))',
      }}
    />
  );
}

const BUTTON: CSSProperties = {
  minHeight: 'var(--tap-min)',
  padding: '0 14px',
  border: '1px solid hsl(var(--line))',
  borderRadius: 'var(--r-sm)',
  background: 'hsl(var(--surface-2))',
  color: 'hsl(var(--fg))',
  fontFamily: 'inherit',
  fontSize: '0.875rem',
  fontWeight: 500,
  cursor: 'pointer',
  touchAction: 'manipulation',
};

export function MobileButton({
  onClick,
  children,
  disabled,
  tone = 'plain',
}: {
  onClick: () => void;
  children: ReactNode;
  disabled?: boolean;
  tone?: 'plain' | 'primary';
}): ReactElement {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      style={{
        ...BUTTON,
        opacity: disabled ? 0.5 : 1,
        cursor: disabled ? 'default' : 'pointer',
        background: tone === 'primary' ? 'hsl(var(--flow))' : 'hsl(var(--surface-2))',
        color: tone === 'primary' ? 'hsl(var(--bg))' : 'hsl(var(--fg))',
        borderColor: tone === 'primary' ? 'hsl(var(--flow))' : 'hsl(var(--line))',
      }}
    >
      {children}
    </button>
  );
}

/**
 * 字符串清单编辑器（移动端）—— 桌面 `screens/settings/ListEditor.tsx` 的对位。
 *
 * # 为什么不 import 桌面那一个
 *
 * 它 import `./Primitives` → `dialogs/Csel`，而那一族的外观**全部**落在 `prototype.css` /
 * `components.css` 上 —— 那是契约 A1 禁止进入移动入口的层叠链。照搬会得到一堆没有样式的裸控件。
 * **但去重与草稿规则一个字都不重写**：`parseBulkEntries` / `sameEntries` / `nextDraft` 三条
 * 2026-09-06 已经搬进 `@/domain/list-entries`，两端读同一份（那正是本仓在 DNS 预设表上
 * 吃过一次的亏 —— 两份实现必然各自漂移）。
 *
 * # 草稿 + 失焦提交（与本屏三个文本框同一条口径）
 *
 * 打字**只动草稿**：逐键写盘会让中间态真的落进配置，且代理运行时每个字符触发一次整核重启评估。
 * 删除 / 添加 / 批量导入是**离散动作**，立即提交，但一律基于**草稿**而非 `value` ——
 * 基于后者会把另一行里还没提交的编辑一起丢掉（桌面那份的原话，同一个坑）。
 *
 * `nextDraft` 那条守卫也照搬：外部刷新（托盘 / 备份恢复 / 另一屏保存）不许打断正在敲字的人。
 */
export function MobileListEditor({
  id,
  value,
  onChange,
  placeholder,
  ariaLabel,
  addLabel,
  importLabel,
  importHint,
  removeLabel,
  confirmLabel,
  cancelLabel,
  emptyLabel,
}: {
  id: string;
  value: readonly string[];
  onChange: (next: string[]) => void;
  placeholder?: string;
  ariaLabel: string;
  addLabel: string;
  importLabel: string;
  importHint: string;
  removeLabel: string;
  confirmLabel: string;
  cancelLabel: string;
  /** 空清单时的一句话。**不是缺席声明** —— 清单可编辑，只是现在是空的。 */
  emptyLabel: string;
}): ReactElement {
  const [draft, setDraft] = useState<readonly string[]>(value);
  const seeded = useRef<readonly string[]>(value);
  useEffect(() => {
    setDraft((cur) => nextDraft(cur, seeded.current, value));
    seeded.current = value;
  }, [value]);

  const [importOpen, setImportOpen] = useState(false);
  const [importDraft, setImportDraft] = useState('');

  /** 提交到父级。与 `value` 逐项相同就**不写** —— 免一次无谓落盘（同桌面那份）。 */
  function commit(next: readonly string[]): void {
    setDraft(next);
    if (!sameEntries(next, value)) onChange([...next]);
  }

  return (
    <div data-list-editor={id} style={{ display: 'flex', flexDirection: 'column', gap: '8px', width: '100%' }}>
      {draft.length === 0 && <span style={ROW_DESC}>{emptyLabel}</span>}
      {draft.map((entry, index) => (
        <div key={index} style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <MobileTextInput
            mono
            value={entry}
            ariaLabel={ariaLabel}
            placeholder={placeholder}
            onChange={(next) => setDraft((cur) => cur.map((s, i) => (i === index ? next : s)))}
            onCommit={() => commit(draft)}
          />
          <button
            type="button"
            aria-label={removeLabel}
            onClick={() => commit(draft.filter((_, i) => i !== index))}
            style={{
              flex: '0 0 auto',
              width: 'var(--tap-min)',
              height: 'var(--tap-min)',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              padding: 0,
              border: '1px solid hsl(var(--line))',
              borderRadius: 'var(--r-sm)',
              background: 'hsl(var(--surface-2))',
              color: 'hsl(var(--fg-dim))',
              cursor: 'pointer',
              touchAction: 'manipulation',
            }}
          >
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden="true">
              <path d="M5 5l14 14M19 5L5 19" />
            </svg>
          </button>
        </div>
      ))}
      <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
        <MobileButton onClick={() => commit([...draft, ''])}>{addLabel}</MobileButton>
        <MobileButton onClick={() => setImportOpen((v) => !v)}>{importLabel}</MobileButton>
      </div>
      {importOpen && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
          <textarea
            rows={4}
            value={importDraft}
            aria-label={importLabel}
            placeholder={placeholder}
            onChange={(e) => setImportDraft(e.currentTarget.value)}
            style={{ ...FIELD, fontFamily: 'var(--mono)', minHeight: 'auto', resize: 'vertical' }}
          />
          <span style={ROW_DESC}>{importHint}</span>
          <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
            <MobileButton
              tone="primary"
              onClick={() => {
                commit(parseBulkEntries(importDraft, [...draft]));
                setImportDraft('');
                setImportOpen(false);
              }}
            >
              {confirmLabel}
            </MobileButton>
            <MobileButton
              onClick={() => {
                setImportDraft('');
                setImportOpen(false);
              }}
            >
              {cancelLabel}
            </MobileButton>
          </div>
        </div>
      )}
    </div>
  );
}

/** 右向尖括号（进入二级页）。纯装饰，`aria-hidden`：行本身已经是可读的按钮。 */
function Chevron(): ReactElement {
  return (
    <svg
      viewBox="0 0 24 24"
      width="18"
      height="18"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      aria-hidden="true"
    >
      <path d="M9 5l7 7-7 7" />
    </svg>
  );
}

/** 根页的「进入二级页」行。 */
export function SettingsPushRow({
  id,
  label,
  desc,
  onOpen,
  first,
}: {
  id: string;
  label: string;
  desc?: ReactNode;
  onOpen: () => void;
  first?: boolean;
}): ReactElement {
  return (
    <button
      type="button"
      data-push={id}
      onClick={onOpen}
      style={{
        ...(first === true ? ROW_FIRST : ROW),
        width: '100%',
        margin: 0,
        textAlign: 'start',
        background: 'none',
        borderLeft: 'none',
        borderRight: 'none',
        borderBottom: 'none',
        color: 'hsl(var(--fg))',
        fontFamily: 'inherit',
        cursor: 'pointer',
        touchAction: 'manipulation',
      }}
    >
      <span style={ROW_TEXT}>
        <span style={ROW_LABEL}>{label}</span>
        {desc !== undefined && <span style={ROW_DESC}>{desc}</span>}
      </span>
      <span style={{ ...CONTROL_SLOT, color: 'hsl(var(--fg-dim))' }}>
        <Chevron />
      </span>
    </button>
  );
}

/**
 * 只读状态行。
 *
 * 存在的理由是「有一个用户会问的问题，但我们今天答不出真值」——**渲染成待定，而不是编一个已授权**
 * （IA §2.4「Data positions with no registered source」）。故它刻意**不可点**：一个点了没反应的行
 * 与一个拨了不生效的开关是同一类缺陷。
 */
export function SettingsStatusRow({
  id,
  label,
  desc,
  descDetails,
  status,
  first,
}: {
  id: string;
  label: ReactNode;
  desc: ReactNode;
  descDetails?: ReactNode;
  status: ReactNode;
  first?: boolean;
}): ReactElement {
  return (
    <SettingsRow
      id={id}
      first={first}
      label={label}
      desc={desc}
      descDetails={descDetails}
      control={
        <output
          data-status={id}
          style={{
            fontSize: '0.75rem',
            color: 'hsl(var(--fg-dim))',
            fontWeight: 500,
            whiteSpace: 'nowrap',
          }}
        >
          {status}
        </output>
      }
    />
  );
}

/** 页头（二级页带返回）。`h1` 每屏只有一个，与 IA「screen header」同一格。 */
export function SettingsHeader({
  title,
  onBack,
  backLabel,
}: {
  title: string;
  onBack?: () => void;
  backLabel: string;
}): ReactElement {
  const titleId = useId();
  return (
    <header
      style={{
        /* IA §2.4「Fixed and scrolling」把 screen header 列在**固定**那一格。外壳没有页头槽位
           （`MobileShell` 把 children 整个交给滚动区），故与首页同法：屏内 `sticky` 兑现同一语义，
           外壳零改动（`home/home.css:52-63` 的 `.h-header` 逐条同构）。
           `top: 0` 不吃 `--safe-t` —— 状态栏安全区已由 `.m-shell` 在外层让过一次。
           卡片会从下面滚过去 ⇒ 必须是不透明实底 + 一条发丝线收边，否则字叠字。 */
        position: 'sticky',
        top: 0,
        zIndex: 2,
        background: 'hsl(var(--bg))',
        borderBottom: '1px solid hsl(var(--hair))',
        display: 'flex',
        alignItems: 'center',
        gap: '8px',
        minHeight: '48px',
      }}
      aria-labelledby={titleId}
    >
      {onBack !== undefined && (
        <button
          type="button"
          data-back="settings"
          aria-label={backLabel}
          onClick={onBack}
          style={{
            minWidth: 'var(--tap-min)',
            minHeight: 'var(--tap-min)',
            margin: '0 0 0 -10px',
            padding: 0,
            border: 0,
            background: 'none',
            color: 'hsl(var(--fg))',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            cursor: 'pointer',
            touchAction: 'manipulation',
          }}
        >
          <svg
            viewBox="0 0 24 24"
            width="22"
            height="22"
            fill="none"
            stroke="currentColor"
            strokeWidth={1.8}
            aria-hidden="true"
          >
            <path d="M15 5l-7 7 7 7" />
          </svg>
        </button>
      )}
      <h1
        id={titleId}
        style={{
          margin: 0,
          fontFamily: 'var(--disp)',
          fontSize: '1.3125rem',
          fontWeight: 600,
          letterSpacing: '-0.01em',
          lineHeight: 1.25,
        }}
      >
        {title}
      </h1>
    </header>
  );
}
