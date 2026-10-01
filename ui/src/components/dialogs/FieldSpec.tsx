/**
 * 数据驱动表单基建 —— FieldSpec 描述符 + 单一 FieldRenderer（§1.4 表单层）。
 *
 * **可复用**：节点表单（D2，8 协议）+ 组网表单（D3）+ 规则表单（D4）共用此层，勿节点化。
 * 消灭 上游 `server-config-dialog.tsx:332-501` 那种「15 个 {proto==='X' && <XForm/>} 同构分支」——
 * 一张数据表（ND_SPEC/规则表）+ 一个渲染器 map 出字段，新协议/新字段只加表项不复制 JSX。
 *
 * 淬火机制（polaris-node-form-hardening-requirements.md，逐条落为结构而非自觉）：
 *  - **R2 单点 number 渲染**：`parseNumberField` 是全库唯一的 number 解析实现 —— 空串 → `undefined`
 *    （允许退格删空重录）、非空十进制解析、异常 → `undefined`（**绝不硬塞 0**）。所有 number 字段
 *    （port / keepalive / mtu / alterId / up·down 带宽 …）都走它，「15 个手写 `parseInt(x)||0`」时代
 *    的重犯面整类消失。
 *  - **R1 无 radix/RHF**：select 走 D1 `<Csel>`（受控、无懒挂 Portal、无伪 onValueChange），
 *    reset-race 根因整类不存在（配合 NodeDialog 的 `key` 重挂 + 同步初始化）。
 *
 * i18n：label/hint 只存 i18n key，五语键完整性由 `i18n/i18n-coverage.test.ts` 与 locale parity 门守住；
 * 描述符不再保留中文默认值，避免 locale 与组件各有一份文案真值。
 *
 * select 选项文案多为专有名词（TCP/xtls-rprx-vision/…）直接字面量；通用自然语言可传点分 i18n key。
 */

import { useState, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Fold } from '@/components/Fold';
import { InfoIcon } from '@/components/InfoIcon';
import { Csel, type CselOption } from './Csel';
/* 规格与草稿的纯数据层住在 `./field-spec`（零 import，两个客户端共用；拆分理由见那份文件头注）。
   本文件把它**原样再导出**，故既有的 `from './FieldSpec'` 调用点一个字都不用改。 */
import {
  normalizeSelectOptions,
  parseNumberField,
  type FieldSpec,
  type FormValue,
  type FormValues,
  type SelectOption,
} from './field-spec';

export type { FieldSpec, FormValue, FormValues, SelectOption } from './field-spec';
export { draftFromSpecs, normalizeSelectOptions, parseNumberField } from './field-spec';

/**
 * select 选项元组 → `Csel` 选项对象。
 *
 * **本体住在 `./field-spec#normalizeSelectOptions`**（零 import 纯函数，两个客户端共用）：
 * 那里面的两条判据（点分键才翻译 / 未知当前值并入首位）此前在移动端 `forms/FormFields.tsx` 里
 * 被抄了一份，连正则都逐字相同 —— 抄一份等于给同一个问题造第二个答案，而这类分叉本仓没有任何门。
 * 本函数只剩「归一化选项 → `CselOption`」这一步类型收窄（`CselOption` 是超集），保留是为了
 * 桌面既有的调用点与那批单测一个字都不用改。
 */
export function toCselOptions(
  options: readonly SelectOption[],
  current?: FormValue,
  translate: (key: string) => string = (key) => key,
): CselOption[] {
  return normalizeSelectOptions(options, current, translate);
}

export interface FieldRendererProps {
  spec: FieldSpec;
  value: FormValue;
  onChange: (value: FormValue) => void;
}

/**
 * 唯一字段渲染器。按 spec.t 分派；enum → `<Csel>`（非原生 select、非 radix），number → 单点 R2 分支。
 * 类名对齐原型（`.fld`/`.fld-l`/`.input`/`.swt-row` 等）→ 样式复用 components.css 无需新写。
 */
export function FieldRenderer({ spec, value, onChange }: FieldRendererProps) {
  const { t } = useTranslation();
  const [secretVisible, setSecretVisible] = useState(false);
  const label = t(spec.label);
  const fid = `nd-f-${spec.k}`;

  if (spec.t === 'switch') {
    const on = value === true;
    const off = spec.disabled === true;
    const hintKey = off && spec.disabledHint ? spec.disabledHint : spec.hint;
    return (
      <div className="fld swt-row">
        <div className="swt-tx">
          <span className="swt-label">
            <b>{label}</b>
            {hintKey && <InfoIcon tip={t(hintKey)} />}
          </span>
        </div>
        {/* 原生 `disabled`（而非 `aria-disabled` + 自行拦截）：它连点击事件都不派发 ⇒ `onChange`
            结构上不可达，而不是「拦得住就好」。禁用的开关是「结构上永远不能开」的语义载体，
            这一层不能只做视觉。视觉见 index.css 的 `.swt:disabled`。 */}
        <button
          type="button"
          role="switch"
          aria-checked={on}
          aria-label={label}
          className={`swt${on ? ' on' : ''}`}
          disabled={off}
          onClick={() => onChange(!on)}
        />
      </div>
    );
  }

  const labelContents = (
    <>
      <span>{label}</span>
      {spec.opt && <span className="fld-opt"> {t('common.optional')}</span>}
      {spec.hint && <InfoIcon tip={t(spec.hint)} />}
    </>
  );
  const labelEl = (
    <label className="fld-l fld-l-info" htmlFor={fid}>
      {labelContents}
    </label>
  );
  // 自定义下拉已经有一整块可见 button 触发器。若这里也用 label[for]，浏览器会把点击标题或
  // InfoIcon 转发成 button.click()，凭空扩出一块不可见触发区；输入框仍保留上面的 label 聚焦语义。
  const selectLabelEl = <div className="fld-l fld-l-info">{labelContents}</div>;

  if (spec.t === 'select') {
    // 传 value：当前值落在选项集外时并入选项，避免「一碰下拉就被迫改值」（见 toCselOptions 注释）。
    const opts = toCselOptions(spec.options, value, t);
    return (
      <div className="fld">
        {selectLabelEl}
        <Csel
          id={fid}
          ariaLabel={label}
          value={typeof value === 'string' ? value : ''}
          onChange={(v) => onChange(v)}
          options={opts}
        />
      </div>
    );
  }

  if (spec.t === 'number') {
    return (
      <div className="fld">
        {labelEl}
        <input
          id={fid}
          className={`input${spec.mono ? ' mono' : ''}`}
          inputMode="numeric"
          value={value === undefined || value === null ? '' : String(value)}
          onChange={(e) => onChange(parseNumberField(e.target.value))}
          placeholder={spec.ph ?? '—'}
        />
      </div>
    );
  }

  if (spec.t === 'textarea') {
    const textarea = (
      <textarea
        id={fid}
        className={`input${spec.mono ? ' mono' : ''}${spec.secret && !secretVisible ? ' secret-masked' : ''}`}
        rows={spec.rows ?? 4}
        value={typeof value === 'string' ? value : ''}
        onChange={(e) => onChange(e.target.value)}
        placeholder={spec.ph}
      />
    );
    return (
      <div className="fld">
        {labelEl}
        {spec.secret ? (
          <div className="secret-field">
            {textarea}
            <button
              type="button"
              className="secret-toggle"
              aria-label={secretVisible ? t('common.hideSecret') : t('common.showSecret')}
              aria-pressed={secretVisible}
              onClick={() => setSecretVisible((visible) => !visible)}
            >
              {secretVisible ? '◉' : '◎'}
            </button>
          </div>
        ) : textarea}
      </div>
    );
  }

  // text（默认）
  const input = (
    <input
      id={fid}
      type={spec.secret && !secretVisible ? 'password' : 'text'}
      className={`input${spec.mono ? ' mono' : ''}`}
      value={typeof value === 'string' ? value : ''}
      onChange={(e) => onChange(e.target.value)}
      placeholder={spec.ph ?? '—'}
    />
  );
  return (
    <div className="fld">
      {labelEl}
      {spec.secret ? (
        <div className="secret-field">
          {input}
          <button
            type="button"
            className="secret-toggle"
            aria-label={secretVisible ? t('common.hideSecret') : t('common.showSecret')}
            aria-pressed={secretVisible}
            onClick={() => setSecretVisible((visible) => !visible)}
          >
            {secretVisible ? '◉' : '◎'}
          </button>
        </div>
      ) : input}
    </div>
  );
}

export interface FormFieldsProps {
  fields: readonly FieldSpec[];
  values: FormValues;
  onChange: (key: string, value: FormValue) => void;
}

/** 同一分组的字段渲染入口；显隐判据只在这一处执行。 */
export function FormFields({ fields, values, onChange }: FormFieldsProps) {
  return (
    <>
      {fields
        .filter((field) => !field.when || field.when(values))
        .map((field) => (
          <FieldRenderer
            key={field.k}
            spec={field}
            value={values[field.k]}
            onChange={(value) => onChange(field.k, value)}
          />
        ))}
    </>
  );
}

export function FormSection({
  title,
  fields,
  values,
  onChange,
  collapsible,
  forceOpen,
  children,
}: FormFieldsProps & {
  title: ReactNode;
  collapsible?: boolean;
  forceOpen?: boolean;
  children?: ReactNode;
}) {
  const visibleCount = fields.filter((field) => !field.when || field.when(values)).length;
  if (visibleCount === 0 && children === undefined) return null;
  const body = (
    <div className="form-field-section-body">
      <FormFields fields={fields} values={values} onChange={onChange} />
      {children}
    </div>
  );
  if (collapsible) {
    return (
      <Fold
        className="form-field-fold"
        title={title}
        count={visibleCount + (children === undefined ? 0 : 1)}
        forceOpen={forceOpen}
      >
        {body}
      </Fold>
    );
  }
  return (
    <section className="form-field-section">
      <div className="form-field-section-title">{title}</div>
      {body}
    </section>
  );
}

export interface FormTabItem {
  id: string;
  label: ReactNode;
  fields: readonly FieldSpec[];
  /** 归属当前任务页、但不适合放进 FieldSpec 的手写控件/状态块。 */
  children?: ReactNode;
}

/**
 * 接入表单唯一页签原语。只管「此刻看哪个任务页」，不管表单草稿与脏态：
 * 页签点击只调 `onSelect`，结构上无法误触 `onChange`，因而「只切页→取消」不会弹放弃更改。
 *
 * 受控 active 由调用方持有，是为了让校验失败能精确切到出错页；不在本组件内再存第二份状态。
 */
export function FormTabs({
  id,
  ariaLabel,
  tabs,
  active,
  onSelect,
  values,
  onChange,
}: Pick<FormFieldsProps, 'values' | 'onChange'> & {
  id: string;
  ariaLabel: string;
  tabs: readonly FormTabItem[];
  active: string;
  onSelect: (id: string) => void;
}) {
  const available = tabs.filter((tab) => tab.fields.length > 0 || tab.children !== undefined);
  if (available.length === 0) return null;
  const current = available.find((tab) => tab.id === active) ?? available[0];

  const onTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex: number | null = null;
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % available.length;
    else if (event.key === 'ArrowLeft') nextIndex = (index - 1 + available.length) % available.length;
    else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = available.length - 1;
    if (nextIndex === null) return;
    event.preventDefault();
    const next = available[nextIndex];
    onSelect(next.id);
    window.requestAnimationFrame(() => document.getElementById(`${id}-tab-${next.id}`)?.focus());
  };

  return (
    <>
      <div className="sub-tabs form-tabs" role="tablist" aria-label={ariaLabel}>
        {available.map((tab, index) => {
          const selected = current.id === tab.id;
          return (
            <button
              id={`${id}-tab-${tab.id}`}
              key={tab.id}
              type="button"
              role="tab"
              className={selected ? 'on' : ''}
              aria-selected={selected}
              aria-controls={`${id}-panel`}
              tabIndex={selected ? 0 : -1}
              onClick={() => onSelect(tab.id)}
              onKeyDown={(event) => onTabKeyDown(event, index)}
            >
              {tab.label}
            </button>
          );
        })}
      </div>
      <div
        id={`${id}-panel`}
        role="tabpanel"
        className="form-tab-panel"
        aria-labelledby={`${id}-tab-${current.id}`}
      >
        <FormFields fields={current.fields} values={values} onChange={onChange} />
        {current.children}
      </div>
    </>
  );
}

export default FieldRenderer;
