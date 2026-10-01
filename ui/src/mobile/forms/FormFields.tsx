import { MobileInfo } from '../MobileInfo';
import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端字段渲染器 —— `FieldSpec` 那张表在触屏上的呈现层。
 *
 * # 与桌面 `FieldSpec.tsx#FieldRenderer` 的分工
 *
 * **描述符共用，呈现层重写**（契约 A1：桌面渲染器的类名 `.fld` / `.input` / `.swt` 全落在
 * `components.css` = 桌面层叠链，import 进移动端会得到一堆没有样式的裸控件）。
 * 复用的那一半住在 `@/components/dialogs/field-spec`：类型、`parseNumberField`（R2 单点）、
 * `draftFromSpecs`。这里一条判据都不新造。
 *
 * # 三处**必须**与桌面不同（不是排版偏好）
 *
 *  1. **关键约束常驻，明确登记的机制说明由触屏 i 打开全文**。禁用原因仍完整显示；
 *     未登记的说明不折叠，不按字数或首句自动抽取摘要。
 *  2. Select fields share the app-themed MobileSelect sheet. Disabled options remain visible;
 *     native select is retained only for standard form and change-event semantics.
 *  3. **`secret` 不做「◉/◎ 切换」那颗小按钮**，改成整行下方一颗有文字标签的按钮：44px 触控目标
 *     在一个 20px 的字形上不成立。
 *
 * # 渲染器与描述符的穷尽关系
 *
 * `switch (spec.t)` 以 `never` 兜底：`FieldSpec` 加一支而这里没补 case ⇒ **编译不过**。
 * 桌面那侧是 `if` 链 + 末尾兜底（text 档），加一支新类型会被静默渲染成文本框；这里不重复那个形态。
 */

import { useId, useState, type ReactElement } from 'react';
import {
  normalizeSelectOptions,
  parseNumberField,
  type FieldSpec,
  type FormValue,
  type FormValues,
} from '@/components/dialogs/field-spec';

export interface MobileFieldProps {
  readonly spec: FieldSpec;
  readonly value: FormValue;
  readonly onChange: (value: FormValue) => void;
  /** 已本地化的取词器（本组件不 import i18next：呈现层要能在 node 环境下被真渲染）。 */
  readonly t: (key: string) => string;
}

/** Reviewed opt-ins only: format/risk stays in the authored summary; disabled reasons never fold. */
export const MOBILE_FIELD_HINT_SUMMARIES: Readonly<Record<string, string>> = {
  'node.field.certPinHint': 'mobileHelp.certPin',
  'node.field.noParrotHint': 'mobileHelp.noParrot',
  'node.field.spoofMethodHint': 'mobileHelp.spoofMethod',
  'node.field.tlsEngineHint': 'mobileHelp.tlsEngine',
  'node.field.tlsFragmentHint': 'mobileHelp.tlsFragment',
  'node.field.meshRoutesHint': 'mobileHelp.meshRoutes',
  'node.field.sysIfaceHint': 'mobileHelp.sysIface',
  'node.field.extraJsonHint': 'mobileHelp.extraJson',
  'node.field.masqueVersionHint': 'mobileHelp.masqueVersion',
  'node.field.tcServerPubHint': 'mobileHelp.tcServerPub',
  'node.field.tcPrivateKeyHint': 'mobileHelp.tcPrivateKey',
  'node.field.derpModeHint': 'mobileHelp.derpMode',
  'node.field.derpMapUrlHint': 'mobileHelp.derpMapUrl',
  'node.field.derpServersHint': 'mobileHelp.derpServers',
  'node.onDemandHint': 'mobileHelp.onDemand',
  'wg.detourHint': 'mobileHelp.wgDetour',
  'wg.reverseMeshHint': 'mobileHelp.reverseMesh',
  'ts.detourHint': 'mobileHelp.tsDetour',
  'ts.reverseMeshHint': 'mobileHelp.reverseMesh',
  'ts.listenPortHint': 'mobileHelp.tsListenPort',
  'ts.resolveByNameHint': 'mobileHelp.tsResolveByName',
};

function Hint({ hintKey, title, t, disabled = false }: {
  hintKey?: string; title: string; t: (key: string) => string; disabled?: boolean;
}): ReactElement | null {
  if (hintKey === undefined) return null;
  const text = t(hintKey);
  if (text === '') return null;
  const summaryKey = disabled ? undefined : MOBILE_FIELD_HINT_SUMMARIES[hintKey];
  if (summaryKey === undefined) return <p className="m-form-hint">{text}</p>;
  return <div className="m-form-hint"><MobileInfo title={title} summary={t(summaryKey)} details={text} /></div>;
}

/** 一个字段。`switch` 以 `never` 兜底 —— 描述符加一支而这里没补 case ⇒ 编译不过。 */
export function MobileField({ spec, value, onChange, t }: MobileFieldProps): ReactElement {
  const fid = useId();
  const [secretVisible, setSecretVisible] = useState(false);
  const label = t(spec.label);

  switch (spec.t) {
    case 'switch': {
      const on = value === true;
      const off = spec.disabled === true;
      /* 禁用时用 `disabledHint` **取代** `hint`（同桌面）：`hint` 讲的是「开了会怎样」，
         而那件事在禁用场景下结构上永远不会发生。 */
      const hintKey = off && spec.disabledHint !== undefined ? spec.disabledHint : spec.hint;
      return (
        <div className="m-form-row m-form-switch">
          <div className="m-form-switch-tx">
            <span className="m-form-label" id={fid}>
              {label}
            </span>
            <Hint hintKey={hintKey} title={label} t={t} disabled={off} />
          </div>
          {/* 原生 `disabled`（而非 `aria-disabled` + 自行拦截）：它连点击事件都不派发 ⇒
              `onChange` 结构上不可达，而不是「拦得住就好」。同桌面那条理由。 */}
          <button
            type="button"
            role="switch"
            aria-checked={on}
            aria-labelledby={fid}
            className={`m-form-swt${on ? ' on' : ''}`}
            disabled={off}
            onClick={() => onChange(!on)}
          />
        </div>
      );
    }
    case 'select':
      return (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor={fid}>
            {label}
            {spec.opt === true && <span className="m-form-opt">{t('common.optional')}</span>}
          </label>
          {/* 「点分键才翻译」与「未知当前值并入首位」两条判据走**共用**的
              `normalizeSelectOptions`（`@/components/dialogs/field-spec`），不在这里再抄一份 ——
              抄一份等于让同一张 `ND_SPEC` 在两个客户端上有两种标签，而那类分叉没有任何门守。 */}
          <MobileSelect
            id={fid}
            className="m-form-select"
            value={typeof value === 'string' ? value : ''}
            onChange={(e) => onChange(e.target.value)}
          >
            {normalizeSelectOptions(spec.options, value, t).map((o) => (
              <option key={o.value} value={o.value} disabled={o.disabled === true}>
                {o.label}
              </option>
            ))}
          </MobileSelect>
          <Hint hintKey={spec.hint} title={label} t={t} />
        </div>
      );
    case 'number':
      return (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor={fid}>
            {label}
            {spec.opt === true && <span className="m-form-opt">{t('common.optional')}</span>}
          </label>
          <input
            id={fid}
            className={`m-form-input${spec.mono === true ? ' mono' : ''}`}
            inputMode="numeric"
            value={value === undefined || value === null ? '' : String(value)}
            placeholder={spec.ph ?? '—'}
            /* R2 单点：空串 → undefined（允许退格删空重录），**绝不硬塞 0**。 */
            onChange={(e) => onChange(parseNumberField(e.target.value))}
          />
          <Hint hintKey={spec.hint} title={label} t={t} />
        </div>
      );
    case 'textarea':
      return (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor={fid}>
            {label}
            {spec.opt === true && <span className="m-form-opt">{t('common.optional')}</span>}
          </label>
          <textarea
            id={fid}
            className={`m-form-input m-form-area${spec.mono === true ? ' mono' : ''}${
              spec.secret === true && !secretVisible ? ' masked' : ''
            }`}
            rows={spec.rows ?? 4}
            value={typeof value === 'string' ? value : ''}
            placeholder={spec.ph}
            onChange={(e) => onChange(e.target.value)}
          />
          {spec.secret === true && (
            <button
              type="button"
              className="m-form-reveal"
              aria-pressed={secretVisible}
              onClick={() => setSecretVisible((v) => !v)}
            >
              {secretVisible ? t('common.hideSecret') : t('common.showSecret')}
            </button>
          )}
          <Hint hintKey={spec.hint} title={label} t={t} />
        </div>
      );
    case 'text':
      return (
        <div className="m-form-row">
          <label className="m-form-label" htmlFor={fid}>
            {label}
            {spec.opt === true && <span className="m-form-opt">{t('common.optional')}</span>}
          </label>
          <input
            id={fid}
            type={spec.secret === true && !secretVisible ? 'password' : 'text'}
            className={`m-form-input${spec.mono === true ? ' mono' : ''}`}
            value={typeof value === 'string' ? value : ''}
            placeholder={spec.ph ?? '—'}
            onChange={(e) => onChange(e.target.value)}
          />
          {spec.secret === true && (
            <button
              type="button"
              className="m-form-reveal"
              aria-pressed={secretVisible}
              onClick={() => setSecretVisible((v) => !v)}
            >
              {secretVisible ? t('common.hideSecret') : t('common.showSecret')}
            </button>
          )}
          <Hint hintKey={spec.hint} title={label} t={t} />
        </div>
      );
    default: {
      /* 穷尽兜底。`FieldSpec` 加一支而这里没补 case ⇒ 这一行编译不过（见文件头注）。 */
      const never: never = spec;
      throw new Error(`unhandled FieldSpec kind: ${JSON.stringify(never)}`);
    }
  }
}

export interface MobileFieldsProps {
  readonly fields: readonly FieldSpec[];
  readonly values: FormValues;
  readonly onChange: (key: string, value: FormValue) => void;
  readonly t: (key: string) => string;
}

/** 一组字段。显隐谓词（`when`）只在这一处执行，与桌面 `FormFields` 同一条。 */
export function MobileFields({ fields, values, onChange, t }: MobileFieldsProps): ReactElement {
  return (
    <>
      {fields
        .filter((f) => f.when === undefined || f.when(values))
        .map((f) => (
          <MobileField
            key={f.k}
            spec={f}
            value={values[f.k]}
            onChange={(v) => onChange(f.k, v)}
            t={t}
          />
        ))}
    </>
  );
}
