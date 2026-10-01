import { useCallback, useId, useRef, useState, type ReactElement, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from './back-stack';
import { InfoIcon } from './InfoIcon';
import { SheetHeading } from './SheetHeading';
import { useSheetFocus } from './use-sheet-focus';

export interface MobileInfoProps {
  readonly title: string;
  /** Explicit semantic summary. Omit when a nearby status/pill already states the consequence. */
  readonly summary?: ReactNode;
  readonly details: ReactNode;
  readonly triggerLabel?: string;
  readonly tone?: 'plain' | 'warn';
  readonly className?: string;
}

/** Read-only details reuse the form sheet geometry, but do not add a redundant footer action. */
export function MobileInfoPanel({ title, children, onClose, closeLabel, restoreFocusTo, dialogId }: {
  readonly title: string;
  readonly children: ReactNode;
  readonly onClose: () => void;
  readonly closeLabel: string;
  readonly restoreFocusTo?: RefObject<HTMLButtonElement | null>;
  readonly dialogId?: string;
}): ReactElement {
  const panel = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDismissableLayer(true, onClose);
  useSheetFocus(panel, true, onClose, { restoreFocusTo });
  return (
    <div id={dialogId} className="m-form-layer m-info-layer" data-compact="" role="dialog" aria-modal="true" aria-labelledby={titleId}
      onClick={(event) => event.stopPropagation()}>
      <button type="button" className="m-form-scrim" aria-label={closeLabel} onClick={onClose} tabIndex={-1} />
      <div ref={panel} className="m-form-panel" tabIndex={-1}>
        <SheetHeading title={title} onClose={onClose} closeLabel={closeLabel} titleId={titleId}
          className="m-form-head" titleClassName="m-form-title" closeClassName="m-form-x" />
        <div className="m-form-body m-info-body">{children}</div>
      </div>
    </div>
  );
}

/** An i opens the complete explanation; no text-length heuristics or error folding live here. */
export function MobileInfo({ title, summary, details, triggerLabel, tone = 'plain', className }: MobileInfoProps): ReactElement {
  const { t } = useTranslation();
  const trigger = useRef<HTMLButtonElement>(null);
  const dialogId = useId();
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  return (
    <span className={`m-info ${summary === undefined ? 'm-info-icon-only' : 'm-info-with-summary'} ${tone}${className ? ` ${className}` : ''}`}>
      {summary !== undefined && <span className="m-info-summary">{summary}</span>}
      <button ref={trigger} type="button" className="m-info-trigger" aria-label={triggerLabel ?? t('mobileHelp.viewDetails', { title })}
        aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? dialogId : undefined} onClick={(event) => { event.stopPropagation(); setOpen(true); }}>
        <InfoIcon />
      </button>
      {open && typeof document !== 'undefined' && createPortal(
        <MobileInfoPanel title={title} onClose={close} closeLabel={t('common.close')} restoreFocusTo={trigger} dialogId={dialogId}>{details}</MobileInfoPanel>, document.body,
      )}
    </span>
  );
}
