import { useEffect } from 'react';

const PROPERTIES = ['--m-vv-height', '--m-vv-width', '--m-vv-top', '--m-vv-left'] as const;

/** IME can resize only the visual viewport. Publish geometry once, without touching focus or state. */
export function installMobileVisibleViewport(win: Window, root: HTMLElement): () => void {
  const viewport = win.visualViewport;
  if (!viewport) return () => undefined;
  const previous = PROPERTIES.map((name) => ({
    name,
    value: root.style.getPropertyValue(name),
    priority: root.style.getPropertyPriority(name),
  }));
  const restore = (): void => {
    for (const { name, value, priority } of previous) {
      if (value) root.style.setProperty(name, value, priority);
      else root.style.removeProperty(name);
    }
  };
  const publish = (): void => {
    const { height, width, offsetTop, offsetLeft, scale } = viewport;
    if (
      ![height, width, offsetTop, offsetLeft, scale].every(Number.isFinite) ||
      height <= 0 || width <= 0 || Math.abs(scale - 1) > 0.01
    ) {
      // Pinch zoom keeps the original layout; CSS percentages remain the fallback.
      restore();
      return;
    }
    const values = [height, width, Math.max(0, offsetTop), Math.max(0, offsetLeft)];
    PROPERTIES.forEach((name, index) => {
      const value = `${values[index]}px`;
      if (root.style.getPropertyValue(name) !== value) root.style.setProperty(name, value);
    });
  };
  viewport.addEventListener('resize', publish);
  viewport.addEventListener('scroll', publish);
  win.addEventListener('resize', publish);
  publish();
  return () => {
    viewport.removeEventListener('resize', publish);
    viewport.removeEventListener('scroll', publish);
    win.removeEventListener('resize', publish);
    restore();
  };
}

export function useMobileVisibleViewport(): void {
  useEffect(() => installMobileVisibleViewport(window, document.documentElement), []);
}
