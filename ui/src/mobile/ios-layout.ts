/** UIKit regions use physical WebView points. These helpers never partition the scroll/list DOM. */
export interface LayoutRect { x: number; y: number; width: number; height: number }
export interface ReservedRegion extends LayoutRect { kind: 'division' | 'occlusion' }
export interface IosViewportValue {
  width: number;
  height: number;
  top?: number;
  right?: number;
  bottom?: number;
  left?: number;
  keyboardVisibleBottom?: number;
  verticalBarSide: 'left' | 'right' | 'unspecified';
  reservedRegions: ReservedRegion[];
}
export interface IosViewportFrame extends LayoutRect { scale: number }

declare global {
  interface Window {
    __polarisIosViewport?: { value: IosViewportValue; sequence: number };
  }
}

const right = (r: LayoutRect): number => r.x + r.width;
const bottom = (r: LayoutRect): number => r.y + r.height;
const finite = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n);
const clamp = (n: number, low: number, high: number): number => Math.max(low, Math.min(high, n));

export function readIosViewport(value: unknown): IosViewportValue | null {
  if (!value || typeof value !== 'object') return null;
  const v = value as Record<string, unknown>;
  if (!finite(v.width) || !finite(v.height) || v.width <= 0 || v.height <= 0 || !Array.isArray(v.reservedRegions)) return null;
  const reservedRegions = v.reservedRegions.filter((r): r is ReservedRegion => {
    if (!r || typeof r !== 'object') return false;
    const a = r as Record<string, unknown>;
    return (a.kind === 'division' || a.kind === 'occlusion') &&
      finite(a.x) && finite(a.y) && finite(a.width) && finite(a.height) && a.width >= 0 && a.height >= 0 &&
      (a.kind === 'division' ? a.width > 0 || a.height > 0 : a.width > 0 && a.height > 0);
  });
  const insets = Object.fromEntries(['top', 'right', 'bottom', 'left', 'keyboardVisibleBottom']
    .filter((key) => finite(v[key]) && v[key] >= 0).map((key) => [key, v[key]]));
  return { width: v.width, height: v.height, ...insets,
    verticalBarSide: v.verticalBarSide === 'left' || v.verticalBarSide === 'right' ? v.verticalBarSide : 'unspecified',
    reservedRegions };
}

/** Recompute from raw UIKit points after WebKit settles or zooms, even at the
 * same native sequence. innerWidth can still hold the old rotation/zoom width. */
export function refreshNativeViewportCss(): { width: number; height: number } | null {
  const root = document.documentElement;
  if (root.dataset.mobileOs !== 'ios' || root.dataset.iosLayoutSource !== 'uikit-viewport') return null;
  const value = readIosViewport(window.__polarisIosViewport?.value);
  const width = root.clientWidth;
  if (!value || !finite(width) || width <= 0) return null;
  const factor = width / value.width;
  const height = value.height * factor;
  if (!finite(factor) || factor <= 0 || !finite(height) || height <= 0) return null;
  for (const [edge, key] of [['t', 'top'], ['r', 'right'], ['b', 'bottom'], ['l', 'left']] as const) {
    const inset = value[key];
    const converted = inset === undefined ? undefined : inset * factor;
    if (finite(converted)) root.style.setProperty(`--safe-${edge}`, `${converted}px`);
  }
  if (value.keyboardVisibleBottom !== undefined) {
    root.style.setProperty('--ios-keyboard-visible-bottom', `${Math.min(value.height, value.keyboardVisibleBottom) * factor}px`);
  }
  return { width, height };
}

/** offsetTop/Left locate the visible area in layout coordinates, just as the keyboard form layer does. */
export function iosRegionsInCss(value: IosViewportValue, layoutWidth: number, viewport: IosViewportFrame): ReservedRegion[] {
  const factor = layoutWidth / value.width / viewport.scale;
  if (!finite(factor) || factor <= 0) return [];
  return value.reservedRegions.map((r) => ({ kind: r.kind,
    x: viewport.x + r.x * factor, y: viewport.y + r.y * factor,
    width: r.width * factor, height: r.height * factor }));
}

/** A zero-thickness division is a barrier when a target straddles it, not an empty occlusion. */
export function intersectsReserved(rect: LayoutRect, region: ReservedRegion): boolean {
  const x = region.width === 0
    ? rect.x < region.x && right(rect) > region.x
    : rect.x < right(region) && right(rect) > region.x;
  const y = region.height === 0
    ? rect.y < region.y && bottom(rect) > region.y
    : rect.y < bottom(region) && bottom(rect) > region.y;
  return x && y;
}

export function intersectRects(a: LayoutRect, b: LayoutRect): LayoutRect | null {
  const x = Math.max(a.x, b.x), y = Math.max(a.y, b.y);
  const width = Math.min(right(a), right(b)) - x, height = Math.min(bottom(a), bottom(b)) - y;
  return width > 0 && height > 0 ? { x, y, width, height } : null;
}

function contains(a: LayoutRect, b: LayoutRect): boolean {
  return a.x <= b.x && a.y <= b.y && right(a) >= right(b) && bottom(a) >= bottom(b);
}

/** Candidate rectangles belong to one component's containing block, never to the shell as a whole. */
export function avoidReserved(
  target: LayoutRect, bounds: LayoutRect, regions: readonly ReservedRegion[], minimum = { width: 48, height: 48 },
): { frame: LayoutRect; available: LayoutRect } | null {
  if (![target, bounds].every((r) => [r.x, r.y, r.width, r.height].every(finite) && r.width > 0 && r.height > 0) ||
    !finite(minimum.width) || !finite(minimum.height) || minimum.width <= 0 || minimum.height <= 0) return null;
  if (contains(bounds, target) && !regions.some((r) => intersectsReserved(target, r))) return { frame: target, available: bounds };
  let free = [bounds];
  for (const region of regions) {
    const next = free.flatMap((r) => {
      if (!intersectsReserved(r, region)) return [r];
      return [
        { x: r.x, y: r.y, width: region.x - r.x, height: r.height },
        { x: right(region), y: r.y, width: right(r) - right(region), height: r.height },
        { x: r.x, y: r.y, width: r.width, height: region.y - r.y },
        { x: r.x, y: bottom(region), width: r.width, height: bottom(r) - bottom(region) },
      ].filter((part) => part.width >= minimum.width && part.height >= minimum.height && contains(r, part));
    });
    free = next.filter((r, index) => !next.some((other, i) => i !== index && contains(other, r) &&
      (other.width > r.width || other.height > r.height || i < index)));
    if (!free.length) return null;
  }
  const candidates = free.filter((r) => r.width >= minimum.width && r.height >= minimum.height).map((available) => {
    const width = Math.min(target.width, available.width), height = Math.min(target.height, available.height);
    const frame = { x: clamp(target.x, available.x, right(available) - width),
      y: clamp(target.y, available.y, bottom(available) - height), width, height };
    return { frame, available };
  });
  candidates.sort((a, b) => {
    const loss = (r: LayoutRect): number => 1 - r.width * r.height / (target.width * target.height);
    const distance = (r: LayoutRect): number => (r.x - target.x) ** 2 + (r.y - target.y) ** 2;
    return loss(a.frame) - loss(b.frame) || distance(a.frame) - distance(b.frame);
  });
  return candidates.find((c) => !regions.some((r) => intersectsReserved(c.frame, r))) ?? null;
}

const TARGETS = '.m-nav,.m-pending-slot,.m-toast-host,.h-sheet,.mn-sheet,.m-form-panel';
const OWNED = ['--ios-region-left', '--ios-region-top', '--ios-region-bottom', '--ios-region-width', '--ios-region-height', '--ios-region-align'];
const rectOf = (el: Element): LayoutRect => {
  const r = el.getBoundingClientRect();
  return { x: r.left, y: r.top, width: r.width, height: r.height };
};
const actionRect = (el: HTMLElement): LayoutRect => {
  const r = rectOf(el), style = getComputedStyle(el);
  const outline = document.activeElement === el && style.outlineStyle !== 'none'
    ? Math.max(0, (Number.parseFloat(style.outlineWidth) || 0) + (Number.parseFloat(style.outlineOffset) || 0)) : 0;
  return { x: r.x - outline, y: r.y - outline, width: r.width + outline * 2, height: r.height + outline * 2 };
};

/** Install only on the native iOS entry. All adjustment stays in existing nodes and containing blocks. */
export function installIosLayout(onApplied: () => void = () => {}): () => void {
  if (document.documentElement.dataset.mobileOs !== 'ios') return () => {};
  const root = document.documentElement;
  const tracked = new Set<HTMLElement>();
  let raf = 0, disposed = false, nativeSequence = -1, critical = false;
  let criticalRow: HTMLElement | null = null;
  let criticalAction: HTMLElement | null = null;
  const clear = (el: HTMLElement): void => {
    for (const key of OWNED) el.style.removeProperty(key);
    delete el.dataset.iosReservedAdjusted;
    delete el.dataset.iosReservedUnresolved;
    delete el.dataset.iosReservedShort;
    delete el.dataset.iosReservedTiny;
  };
  const schedule = (): void => {
    if (!disposed && !raf) raf = window.requestAnimationFrame(update);
  };
  const resize = new ResizeObserver(schedule);
  const refreshTargets = (): void => {
    const next = new Set(document.querySelectorAll<HTMLElement>(TARGETS));
    for (const el of tracked) if (!next.has(el)) { resize.unobserve(el); clear(el); tracked.delete(el); }
    for (const el of next) if (!tracked.has(el)) { tracked.add(el); resize.observe(el); }
  };
  const mutation = new MutationObserver((records) => {
    if (records.some((r) => r.type === 'childList' || r.attributeName === 'dir')) { refreshTargets(); schedule(); }
  });
  mutation.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ['dir'] });

  function update(): void {
    raf = 0;
    if (disposed) return;
    const snapshot = window.__polarisIosViewport;
    if (snapshot && snapshot.sequence < nativeSequence) return;
    const value = root.dataset.iosLayoutSource === 'uikit-viewport' ? readIosViewport(snapshot?.value) : null;
    refreshNativeViewportCss();
    if (snapshot && snapshot.sequence !== nativeSequence) { nativeSequence = snapshot.sequence; critical = true; }
    const visual = window.visualViewport;
    const scale = visual?.scale || 1;
    const nativeBottom = Number.parseFloat(getComputedStyle(root).getPropertyValue('--ios-keyboard-visible-bottom'));
    const viewport: IosViewportFrame = { x: visual?.offsetLeft ?? 0, y: visual?.offsetTop ?? 0,
      width: visual?.width ?? window.innerWidth,
      height: Math.max(0, Math.min(visual?.height ?? window.innerHeight, Number.isFinite(nativeBottom) ? nativeBottom / scale : Infinity)), scale };
    const regions = value ? iosRegionsInCss(value, root.clientWidth, viewport) : [];
    if (criticalAction && (!regions.length || !criticalAction.isConnected)) {
      delete criticalAction.dataset.iosReservedUnresolved;
      criticalAction = null;
    }
    if ((critical || !regions.length) && criticalRow) { clear(criticalRow); criticalRow = null; }
    refreshTargets();
    // Restore baseline in this coalesced frame before measuring; no intermediate state is painted.
    for (const el of tracked) clear(el);
    let changed = false;
    const appliedBounds = new Map<HTMLElement, LayoutRect>();
    const order = (el: HTMLElement): number => el.matches('.m-nav') ? 0 : el.matches('.m-pending-slot') ? 1 : 2;
    // Flow chrome reserves its space first; overlays then measure their real, updated owner.
    for (const el of [...tracked].sort((a, b) => order(a) - order(b))) {
      const target = rectOf(el);
      if (!target.width || !target.height || !regions.some((r) => intersectsReserved(target, r))) continue;
      const isNav = el.matches('.m-nav'), isPending = el.matches('.m-pending-slot');
      const rail = isNav && getComputedStyle(el).flexDirection === 'column';
      const owner = el.matches('.m-form-panel,.mn-sheet') ? el.parentElement
        : el.closest<HTMLElement>('.m-scroll') ?? document.querySelector<HTMLElement>('.m-scroll');
      let bounds = owner ? intersectRects(rectOf(owner), viewport) : null;
      if (bounds && el.matches('.mn-sheet')) {
        const scroll = el.closest<HTMLElement>('.m-scroll');
        if (scroll) bounds = intersectRects(bounds, rectOf(scroll));
      }
      if (isNav || isPending) {
        // Flow chrome can reserve vertical space, but cannot move into another column's text.
        bounds = { x: target.x, y: rail ? target.y : viewport.y,
          width: target.width, height: rail ? Math.min(target.height, bottom(viewport) - target.y)
            : Math.min(bottom(target), bottom(viewport)) - viewport.y };
      }
      if (!bounds) { el.dataset.iosReservedUnresolved = 'true'; continue; }
      if (el.matches('.m-form-panel')) {
        const style = getComputedStyle(owner!);
        const top = Number.parseFloat(style.paddingTop) || 0, low = Number.parseFloat(style.paddingBottom) || 0;
        bounds = { ...bounds, y: bounds.y + top, height: Math.max(0, bounds.height - top - low) };
      }
      const panel = el.matches('.m-form-panel');
      const panelStyle = getComputedStyle(el);
      const footer = panel ? el.querySelector<HTMLElement>('.m-form-foot') : null;
      const buttons = footer?.querySelectorAll('button').length ?? 0;
      const padding = (Number.parseFloat(panelStyle.paddingLeft) || 0) + (Number.parseFloat(panelStyle.paddingRight) || 0);
      const gap = footer ? Number.parseFloat(getComputedStyle(footer).gap) || 0 : 0;
      const minimumWidth = rail ? target.width : panel ? Math.min(target.width, padding + buttons * 48 + Math.max(0, buttons - 1) * gap) : Math.min(48, target.width);
      const minimumHeight = Math.max(Math.min(48, target.height), Number.parseFloat(panelStyle.minHeight) || 0);
      const result = avoidReserved(target, bounds, regions, { width: minimumWidth, height: minimumHeight });
      if (!result) { el.dataset.iosReservedUnresolved = 'true'; continue; }
      const { frame, available } = result;
      // A fixed home sheet may use the viewport rather than m-scroll as its
      // CSS containing block. Its baseline edges and resolved offsets reveal
      // the actual anchors without assuming offsetParent behavior in WebKit.
      const fixedOwner = el.matches('.h-sheet') && panelStyle.position === 'fixed';
      const origin = owner ? rectOf(owner) : viewport;
      const leftAnchor = fixedOwner ? target.x - (Number.parseFloat(panelStyle.left) || 0) : origin.x;
      const bottomAnchor = fixedOwner ? bottom(target) + (Number.parseFloat(panelStyle.bottom) || 0) : bottom(origin);
      el.style.setProperty('--ios-region-left', `${Math.max(0, frame.x - (isNav || isPending ? target.x : leftAnchor))}px`);
      el.style.setProperty('--ios-region-top', `${Math.max(0, frame.y - target.y)}px`);
      const anchorBottom = isNav || isPending ? bottom(target) : panel ? bottom(bounds) : bottomAnchor;
      el.style.setProperty('--ios-region-bottom', `${Math.max(0, anchorBottom - bottom(frame))}px`);
      el.style.setProperty('--ios-region-width', `${frame.width}px`);
      el.style.setProperty('--ios-region-height', `${available.height}px`);
      const shell = el.closest('.m-shell');
      el.style.setProperty('--ios-region-align', getComputedStyle(shell ?? owner ?? root).direction === 'rtl' ? 'end' : 'start');
      el.dataset.iosReservedAdjusted = rail ? 'rail' : 'true';
      appliedBounds.set(el, bounds);
      if (panel) {
        const font = Number.parseFloat(getComputedStyle(root).fontSize) || 16;
        if (available.height < 15 * font) el.dataset.iosReservedShort = 'true';
        if (available.height < 6.5 * font) el.dataset.iosReservedTiny = 'true';
      }
      changed = true;
    }
    if (critical && regions.length && !document.querySelector('.m-form-panel,.h-sheet,.mn-sheet')) {
      const action = document.querySelector<HTMLElement>('[data-write-control="connect"]');
      const scroller = action?.closest<HTMLElement>('.m-scroll');
      if (action && scroller) {
        if (criticalAction && criticalAction !== action) delete criticalAction.dataset.iosReservedUnresolved;
        criticalAction = action;
        const target = actionRect(action), bounds = intersectRects(rectOf(scroller), viewport);
        if (bounds && regions.some((r) => intersectsReserved(target, r))) {
          let result = avoidReserved(target, bounds, regions, { width: target.width, height: target.height });
          if (!result || result.frame.x !== target.x) {
            const row = action.closest<HTMLElement>('.h-acts,.h-header-actions');
            const rowRect = row ? rectOf(row.matches('.h-header-actions') ? row.parentElement! : row) : null;
            const local = rowRect ? { ...bounds, x: rowRect.x, width: rowRect.width } : null;
            const narrow = local ? avoidReserved(target, local, regions, { width: 48, height: target.height }) : null;
            if (row && rowRect && narrow && narrow.available.width < rowRect.width) {
              row.style.setProperty('--ios-region-left', `${Math.ceil(Math.max(0, narrow.available.x - rowRect.x))}px`);
              row.style.setProperty('--ios-region-width', `${Math.floor(narrow.available.width)}px`);
              row.dataset.iosReservedAdjusted = 'row';
              criticalRow = row;
              const adjusted = actionRect(action);
              result = avoidReserved(adjusted, bounds, regions, { width: adjusted.width, height: adjusted.height });
            }
          }
          // Only a vertical scroll within the same existing list; never translate a button over siblings.
          const current = actionRect(action);
          if (result && result.frame.x === current.x) {
            // A geometry repair must not inherit the list's optional smooth-scroll animation.
            const oldBehavior = scroller.style.scrollBehavior;
            scroller.style.scrollBehavior = 'auto';
            scroller.scrollTop += current.y - result.frame.y;
            if (oldBehavior) scroller.style.scrollBehavior = oldBehavior;
            else scroller.style.removeProperty('scroll-behavior');
            const actual = actionRect(action);
            if (regions.some((r) => intersectsReserved(actual, r)) || !contains(bounds, actual)) action.dataset.iosReservedUnresolved = 'true';
            else delete action.dataset.iosReservedUnresolved;
          }
          else action.dataset.iosReservedUnresolved = 'true';
        } else delete action.dataset.iosReservedUnresolved;
        critical = false;
      }
    }
    // CSS reflow can increase a component's intrinsic height. Report unresolved
    // geometry honestly instead of treating a predicted frame as proof.
    for (const el of tracked) {
      const rect = rectOf(el), ownerBounds = appliedBounds.get(el);
      if (regions.some((r) => intersectsReserved(rect, r)) || (ownerBounds && !contains(ownerBounds, rect))) el.dataset.iosReservedUnresolved = 'true';
    }
    if (!regions.length) critical = false;
    if (changed) onApplied();
  }
  const focus = (event: FocusEvent): void => {
    if (event.target instanceof Element && event.target.closest('[data-write-control="connect"]')) critical = true;
    schedule();
  };
  window.addEventListener('polaris-ios-viewport-change', schedule);
  window.addEventListener('resize', schedule);
  window.visualViewport?.addEventListener('resize', schedule);
  window.visualViewport?.addEventListener('scroll', schedule);
  document.addEventListener('focusin', focus);
  refreshTargets(); schedule();
  return () => {
    disposed = true;
    window.cancelAnimationFrame(raf);
    resize.disconnect(); mutation.disconnect();
    for (const el of tracked) clear(el);
    if (criticalRow) clear(criticalRow);
    if (criticalAction) delete criticalAction.dataset.iosReservedUnresolved;
    window.removeEventListener('polaris-ios-viewport-change', schedule);
    window.removeEventListener('resize', schedule);
    window.visualViewport?.removeEventListener('resize', schedule);
    window.visualViewport?.removeEventListener('scroll', schedule);
    document.removeEventListener('focusin', focus);
  };
}
