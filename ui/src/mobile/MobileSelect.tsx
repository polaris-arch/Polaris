import { Children, isValidElement, useCallback, useId, useRef, useState, type ReactElement, type ReactNode, type SelectHTMLAttributes } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from './back-stack';
import { SheetHeading } from './SheetHeading';
import { useSheetFocus } from './use-sheet-focus';

type Option = { value: string; label: ReactNode; disabled: boolean; group?: string };
function optionsOf(children: ReactNode, group?: string, disabled = false): Option[] {
  return Children.toArray(children).flatMap((child): Option[] => {
    if (!isValidElement<{ value?: string; children?: ReactNode; label?: string; disabled?: boolean }>(child)) return [];
    if (child.type === 'option') return [{ value: String(child.props.value ?? child.props.children ?? ''), label: child.props.children, disabled: disabled || child.props.disabled === true, group }];
    return optionsOf(child.props.children, child.type === 'optgroup' ? child.props.label : group, disabled || child.props.disabled === true);
  });
}

/** Native select retains form/change semantics; the visible picker is an app-themed sheet. */
export function MobileSelect(props: SelectHTMLAttributes<HTMLSelectElement>): ReactElement {
  const { t } = useTranslation();
  const uid = useId();
  const native = useRef<HTMLSelectElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState('');
  const options = optionsOf(props.children);
  const value = String(props.value ?? props.defaultValue ?? options[0]?.value ?? '');
  const selected = options.find((option) => option.value === value);
  const close = useCallback(() => { setOpen(false); }, []);
  useDismissableLayer(open, close);
  useSheetFocus(panel, open, close, { restoreFocusTo: trigger, initialFocus: '[aria-selected="true"]:not(:disabled)' });
  return <>
    <select {...props} id={undefined} ref={native} hidden style={{ ...props.style, display: 'none' }} aria-hidden="true" tabIndex={-1} />
    <button
      ref={trigger} id={props.id} type="button" role="combobox" className={`${props.className ?? ''} m-select-trigger`}
      style={props.style} disabled={props.disabled} aria-label={props['aria-label']} aria-labelledby={props['aria-labelledby']}
      aria-describedby={props['aria-describedby']} aria-invalid={props['aria-invalid']} aria-required={props.required}
      aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? uid : undefined}
      onClick={() => {
        const label = props['aria-label'] ?? trigger.current?.labels?.[0]?.textContent ?? selected?.label;
        setTitle(typeof label === 'string' ? label : t('mobileActions.chooseOption'));
        setOpen(true);
      }}
    ><span>{selected?.label ?? value}</span><span aria-hidden="true">⌄</span></button>
    {open && createPortal(
      <div className="m-select-layer" onClick={(event) => { event.stopPropagation(); close(); }}>
        <div ref={panel} className="m-select-panel" role="dialog" aria-modal="true" aria-label={title}
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => {
            const buttons = [...(panel.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
            const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
            if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
              event.preventDefault();
              const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (event.key === 'ArrowUp' ? -1 : 1) + buttons.length) % buttons.length;
              buttons[next]?.focus();
            }
          }}>
          <SheetHeading title={title} onClose={close} closeLabel={t('common.close')} className="m-select-head" closeClassName="m-select-x" />
          <div id={uid} role="listbox" aria-label={title}>
            {options.map((option, index) => <div key={`${option.value}-${index}`}>
              {option.group && options[index - 1]?.group !== option.group && <p className="m-select-group">{option.group}</p>}
              <button type="button" role="option" aria-selected={option.value === value} disabled={option.disabled}
                onClick={() => {
                  if (native.current) {
                    native.current.value = option.value;
                    native.current.dispatchEvent(new Event('change', { bubbles: true }));
                  }
                  close();
                }}>
                <span>{option.label}</span>{option.value === value && <span className="m-select-check" aria-hidden="true">✓</span>}
              </button>
            </div>)}
          </div>
        </div>
      </div>, document.body)}
  </>;
}
