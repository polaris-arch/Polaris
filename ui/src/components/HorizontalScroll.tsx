import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ChevronLeftIcon } from './Icons';
import { horizontalPageTarget } from '@/lib/horizontal-scroll';

const useIsomorphicLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

/** Native scrolling stays in charge: no wheel, touch or pointer gesture interception. */
export function useHorizontalScroll(itemSelector: string, enabled = true) {
  const containerRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ overflow: false, previous: false, next: false });
  const measure = useCallback(() => {
    const scroller = scrollRef.current;
    const content = contentRef.current;
    if (!scroller || !content) return;
    const max = Math.max(0, scroller.scrollWidth - scroller.clientWidth);
    const sign = getComputedStyle(scroller).direction === 'rtl' ? -1 : 1;
    const offset = Math.max(0, Math.min(max, sign * scroller.scrollLeft));
    const next = {
      // A tab strip reserves arrow slots inside the full container. Compare its intrinsic
      // content with that full width, otherwise the slots can prevent overflow clearing.
      overflow: content.scrollWidth > (containerRef.current ?? scroller).clientWidth + 1,
      previous: offset > 1,
      next: max - offset > 1,
    };
    setEdges((old) => old.overflow === next.overflow && old.previous === next.previous && old.next === next.next
      ? old : next);
  }, []);

  useIsomorphicLayoutEffect(() => {
    if (!enabled) return;
    measure();
    const observer = new ResizeObserver(measure);
    for (const element of [containerRef.current, scrollRef.current, contentRef.current]) {
      if (element) observer.observe(element);
    }
    return () => observer.disconnect();
  }, [enabled, measure]);

  function scrollPage(direction: -1 | 1): void {
    const scroller = scrollRef.current;
    const content = contentRef.current;
    if (!scroller || !content) return;
    const rtl = getComputedStyle(scroller).direction === 'rtl';
    const viewportStart = scroller.getBoundingClientRect().left + scroller.clientLeft;
    const viewportEnd = viewportStart + scroller.clientWidth;
    const offset = Math.max(0, Math.min((rtl ? -1 : 1) * scroller.scrollLeft, scroller.scrollWidth - scroller.clientWidth));
    const items = Array.from(content.querySelectorAll(itemSelector), (item) => {
      const rect = item.getBoundingClientRect();
      return rtl
        ? { start: viewportEnd - rect.right + offset, end: viewportEnd - rect.left + offset }
        : { start: rect.left - viewportStart + offset, end: rect.right - viewportStart + offset };
    });
    scroller.scrollTo({
      left: (rtl ? -1 : 1) * horizontalPageTarget(offset, scroller.clientWidth, scroller.scrollWidth, items, direction),
      behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth',
    });
  }

  return { ...edges, containerRef, scrollRef, contentRef, measure, scrollPage };
}

/** Keep the unavailable edge's slot while overflowing; remove both slots when everything fits. */
export function HorizontalScrollArrow({ navigation, direction, label, controls }: {
  navigation: ReturnType<typeof useHorizontalScroll>;
  direction: -1 | 1;
  label: string;
  controls: string;
}) {
  if (!navigation.overflow) return null;
  const available = direction === -1 ? navigation.previous : navigation.next;
  return (
    <button
      type="button"
      className={`horizontal-scroll-arrow${direction === 1 ? ' scroll-next' : ''}`}
      aria-label={label}
      data-tip={label}
      aria-controls={controls}
      aria-hidden={!available}
      disabled={!available}
      onClick={() => navigation.scrollPage(direction)}
    >
      <ChevronLeftIcon aria-hidden="true" />
    </button>
  );
}
