import { describe, expect, it, vi } from 'vitest';
import { installMobileVisibleViewport } from './visible-viewport';

function environment(withViewport = true) {
  const values = new Map<string, { value: string; priority: string }>();
  const style = {
    getPropertyValue: (name: string) => values.get(name)?.value ?? '',
    getPropertyPriority: (name: string) => values.get(name)?.priority ?? '',
    setProperty: (name: string, value: string, priority = '') => { values.set(name, { value, priority }); },
    removeProperty: (name: string) => { const value = values.get(name)?.value ?? ''; values.delete(name); return value; },
  };
  const viewport = Object.assign(new EventTarget(), {
    height: 844, width: 390, offsetTop: 0, offsetLeft: 0, scale: 1,
  });
  const input = { focus: vi.fn(), blur: vi.fn() };
  const win = Object.assign(new EventTarget(), {
    visualViewport: withViewport ? viewport : null,
    innerHeight: 844, innerWidth: 390,
    document: { activeElement: input },
  });
  const stop = () => installMobileVisibleViewport(win as unknown as Window, { style } as unknown as HTMLElement);
  return { style, viewport, input, win, start: stop };
}

describe('mobile visible viewport', () => {
  it('follows an IME visual resize while the layout viewport and focused input remain unchanged', () => {
    const { style, viewport, input, win, start } = environment();
    const stop = start();
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('0px');
    viewport.height = 360;
    viewport.offsetTop = 40;
    viewport.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-height')).toBe('360px');
    expect(style.getPropertyValue('--m-vv-top')).toBe('40px');
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('444px');
    expect([win.innerWidth, win.innerHeight]).toEqual([390, 844]);
    expect(win.document.activeElement).toBe(input);
    expect(input.focus).not.toHaveBeenCalled();
    expect(input.blur).not.toHaveBeenCalled();
    viewport.height = 844.33;
    viewport.offsetTop = 0;
    viewport.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('0px');
    expect(style.getPropertyValue('--m-vv-height')).toBe('844.33px');
    stop();
  });

  it('updates pan and window resize geometry, clamping rubber-band offsets to zero', () => {
    const { style, viewport, win, start } = environment();
    const stop = start();
    viewport.offsetLeft = 12;
    viewport.offsetTop = 20;
    viewport.dispatchEvent(new Event('scroll'));
    expect(style.getPropertyValue('--m-vv-left')).toBe('12px');
    expect(style.getPropertyValue('--m-vv-top')).toBe('20px');
    viewport.width = 320;
    viewport.offsetTop = -4;
    win.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-width')).toBe('320px');
    expect(style.getPropertyValue('--m-vv-top')).toBe('0px');
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('0px');
    stop();
  });

  it('keeps pinch zoom and invalid dimensions on the CSS fallback, then resumes at normal scale', () => {
    const { style, viewport, start } = environment();
    const stop = start();
    viewport.scale = 2;
    viewport.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-height')).toBe('');
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('');
    viewport.scale = 1;
    viewport.height = Number.NaN;
    viewport.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-height')).toBe('');
    viewport.height = 400;
    viewport.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-height')).toBe('400px');
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('444px');
    stop();
  });

  it('restores prior inline values and removes every listener on unmount', () => {
    const { style, viewport, win, start } = environment();
    style.setProperty('--m-vv-height', '77px', 'important');
    const stop = start();
    stop();
    expect(style.getPropertyValue('--m-vv-height')).toBe('77px');
    expect(style.getPropertyPriority('--m-vv-height')).toBe('important');
    expect(style.getPropertyValue('--m-vv-width')).toBe('');
    expect(style.getPropertyValue('--m-vv-occluded-bottom')).toBe('');
    viewport.height = 300;
    viewport.dispatchEvent(new Event('resize'));
    viewport.dispatchEvent(new Event('scroll'));
    win.dispatchEvent(new Event('resize'));
    expect(style.getPropertyValue('--m-vv-height')).toBe('77px');
    expect(style.getPropertyValue('--m-vv-width')).toBe('');
  });

  it('leaves existing percentages and inline values untouched without VisualViewport', () => {
    const { style, win, start } = environment(false);
    style.setProperty('--m-vv-height', '77px');
    const stop = start();
    win.dispatchEvent(new Event('resize'));
    stop();
    expect(style.getPropertyValue('--m-vv-height')).toBe('77px');
    expect(style.getPropertyValue('--m-vv-width')).toBe('');
  });
});
