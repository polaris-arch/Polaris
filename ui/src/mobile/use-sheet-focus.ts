import { useLayoutEffect, useRef, type RefObject } from 'react';

const FOCUSABLE = 'button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';
// Independent from the back stack: this stack controls focus only, while callers keep their close semantics.
const focusLayers: object[] = [];

/** One focus implementation for body portals and nested screen sheets. Only the top layer traps focus. */
export function useSheetFocus(panel: RefObject<HTMLDivElement | null>, active: boolean, onClose: () => void, options?: {
  readonly restoreFocusTo?: RefObject<HTMLButtonElement | null>;
  readonly initialFocus?: string;
}): void {
  const latest = useRef(onClose);
  latest.current = onClose;
  const restoreRef = options?.restoreFocusTo;
  const initialFocus = options?.initialFocus;
  useLayoutEffect(() => {
    if (!active || !panel.current) return;
    const root = panel.current;
    const previous = restoreRef?.current ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const layer = {};
    focusLayers.push(layer);
    const isTop = (): boolean => focusLayers[focusLayers.length - 1] === layer;
    const locked = (): boolean => document.querySelector('[data-overlay="privacy-lock"]') !== null;
    const targets = (): HTMLElement[] => [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(el => el.getClientRects().length > 0);
    const first = (): HTMLElement => targets()[0] ?? root;
    // Grouped selectors reset their expanded groups after opening. Wait for that commit
    // before locating the selected option, so focus and reveal use the current group.
    const frame = requestAnimationFrame(() => {
      if (!isTop() || locked()) return;
      const candidate = initialFocus ? root.querySelector<HTMLElement>(initialFocus) : null;
      const initial = candidate?.getClientRects().length ? candidate : null;
      (initial ?? first()).focus({ preventScroll: true });
      initial?.scrollIntoView({ block: 'nearest' });
    });
    const containFocus = (event: FocusEvent): void => {
      if (isTop() && !locked() && event.target instanceof Node && !root.contains(event.target)) first().focus({ preventScroll: true });
    };
    const keyboard = (event: KeyboardEvent): void => {
      if (!isTop() || locked()) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); latest.current(); return; }
      if (event.key !== 'Tab') return;
      const elements = targets();
      const head = elements[0];
      const tail = elements[elements.length - 1];
      if (!head) { event.preventDefault(); root.focus(); return; }
      if (!root.contains(document.activeElement)) { event.preventDefault(); (event.shiftKey ? tail : head)?.focus(); }
      else if (event.shiftKey && document.activeElement === head) { event.preventDefault(); tail?.focus(); }
      else if (!event.shiftKey && document.activeElement === tail) { event.preventDefault(); head.focus(); }
    };
    document.addEventListener('focusin', containFocus);
    // Close is ready in the commit, even before the deferred selected-row focus.
    // Capture only the top registered sheet, so Escape cannot reach its parent.
    document.addEventListener('keydown', keyboard, true);
    return () => {
      cancelAnimationFrame(frame);
      const top = isTop();
      const index = focusLayers.indexOf(layer);
      if (index >= 0) focusLayers.splice(index, 1);
      document.removeEventListener('focusin', containFocus);
      document.removeEventListener('keydown', keyboard, true);
      if (top && previous?.isConnected && !locked()) previous.focus({ preventScroll: true });
    };
  }, [active, panel, restoreRef, initialFocus]);
}
