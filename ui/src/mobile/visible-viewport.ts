import { useEffect } from 'react';

const PROPERTIES = ['--m-vv-height', '--m-vv-width', '--m-vv-top', '--m-vv-left', '--m-vv-occluded-bottom'] as const;

/** IME can resize only the visual viewport. Publish geometry once, without touching focus or state. */
export function installMobileVisibleViewport(win: Window, root: HTMLElement): () => void {
  if (root.dataset?.mobileOs === 'ios') return () => {};
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
    // Native --safe-b describes the layout viewport's gesture area. When IME
    // obscures its bottom, an overlay docked to the visual viewport must not
    // reserve that same area above the keyboard a second time.
    const visibleTop = Math.max(0, offsetTop);
    const layoutBottom = Number.isFinite(win.innerHeight) && win.innerHeight > 0
      ? win.innerHeight : visibleTop + height;
    const occludedBottom = Math.max(0, layoutBottom - (visibleTop + height));
    const values = [height, width, visibleTop, Math.max(0, offsetLeft), occludedBottom];
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
