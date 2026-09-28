import type { ReactElement } from 'react';

/** Registered `state.info`: the existing rules/Primitives circle-i, shared without importing a screen. */
export function InfoIcon(): ReactElement {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="7.7" r="1.25" fill="currentColor" stroke="none" />
      <path d="M12 11.25v5.25" strokeWidth={2.6} strokeLinecap="round" />
    </svg>
  );
}
