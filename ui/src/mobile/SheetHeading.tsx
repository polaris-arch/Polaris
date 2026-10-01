import type { ReactElement, ReactNode } from 'react';

/** Shared visual heading; each caller keeps its own close/draft/back-stack semantics. */
export function SheetHeading({ title, onClose, closeLabel, className, titleClassName, closeClassName, closeDisabled, titleId, leading }: {
  readonly title: string;
  readonly onClose: () => void;
  readonly closeLabel: string;
  readonly className?: string;
  readonly titleClassName?: string;
  readonly closeClassName?: string;
  readonly closeDisabled?: boolean;
  readonly titleId?: string;
  readonly leading?: ReactNode;
}): ReactElement {
  return <div className={`m-sheet-heading${className ? ` ${className}` : ''}`}>
    {leading}
    <h2 id={titleId} className={`m-sheet-title${titleClassName ? ` ${titleClassName}` : ''}`}>{title}</h2>
    <button type="button" className={`m-sheet-close${closeClassName ? ` ${closeClassName}` : ''}`} aria-label={closeLabel} onClick={onClose} disabled={closeDisabled}>
      {/* Existing registered action.close glyph, shared by form and menu headings. */}
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
        <path d="M5 5l14 14M19 5L5 19" />
      </svg>
    </button>
  </div>;
}
