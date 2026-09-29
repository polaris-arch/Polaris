import { afterAll, afterEach, describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync } from 'node:fs';
import { measure, closeOracle, type Measured } from '@/styles/css-oracle.test-support';
import { inMobileShell } from '@/styles/mount.test-support';
import { clearDismissables } from './back-stack';
import { FormSheet } from './forms/FormSheet';
import { SelectSheetPanel } from './screens/rules/Primitives';

afterAll(closeOracle);
afterEach(clearDismissables);

const roles = ['bg', 'surface', 'surface-2', 'surface-3', 'fg', 'fg-dim', 'fg-faint', 'flow', 'flow-hi', 'flow-weak', 'ok', 'warn', 'err'] as const;
const probes = Object.fromEntries(roles.map(role => [role, `hsl(var(--${role}))`]));
const html = () => inMobileShell(renderToStaticMarkup(<FormSheet title="Form" closeLabel="Close" cancelLabel="Cancel" submitLabel="Save" onRequestClose={() => undefined}>Fields</FormSheet>));
const linear = (n: number): number => n <= .04045 ? n / 12.92 : ((n + .055) / 1.055) ** 2.4;
const luminance = (rgb: string): number => {
  const channels = rgb.match(/[\d.]+/g)?.slice(0, 3).map(n => linear(Number(n) / 255));
  if (!channels || channels.length !== 3) throw new Error('CSS_COLOR_UNREADABLE');
  return .2126 * channels[0] + .7152 * channels[1] + .0722 * channels[2];
};
const contrast = (a: string, b: string): number => {
  const one = luminance(a), two = luminance(b);
  return (Math.max(one, two) + .05) / (Math.min(one, two) + .05);
};
const dark = (system = false): Promise<Measured> => system
  ? measure({ctx:'mobile', html:html(), probes, media:{'prefers-color-scheme':'dark'}}, [{select:'body',props:['color']}])
  : measure({ctx:'mobile', html:html(), probes, rootAttrs:{'data-theme':'dark'}, media:{'prefers-color-scheme':'light'}}, [{select:'body',props:['color']}]);

describe('mobile theme and shared selection presentation', () => {
  it('explicit and system dark resolve alike; all small text roles remain readable on their actual surfaces', async () => {
    const explicit = await dark();
    const system = await dark(true);
    for (const role of roles) expect(system.probe(role)).toBe(explicit.probe(role));
    for (const foreground of ['fg', 'fg-dim', 'fg-faint', 'flow', 'flow-hi', 'ok', 'warn', 'err']) {
      for (const background of ['surface', 'surface-2', 'surface-3', 'flow-weak']) {
        expect(contrast(explicit.probe(foreground), explicit.probe(background)), `${foreground} on ${background}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it('explicit light keeps the shared desktop baseline even when the system is dark', async () => {
    const mobile = await measure({ctx:'mobile',html:html(),probes,rootAttrs:{'data-theme':'light'},media:{'prefers-color-scheme':'dark'}}, [{select:'body',props:['color']}]);
    const baseline = await measure({css:[readFileSync(new URL('../styles/tokens.resolved.css',import.meta.url),'utf8')],html:'',probes,
      rootAttrs:{'data-theme':'light'},media:{'prefers-color-scheme':'dark'}}, [{select:'body',props:['color']}]);
    for (const role of roles) expect(mobile.probe(role)).toBe(baseline.probe(role));
  });

  it('selection headings and values use the common type scale; danger and disabled reasons keep their semantics', async () => {
    const markup = inMobileShell(renderToStaticMarkup(<SelectSheetPanel label="Choose" closeLabel="Close" open onClose={() => undefined} onSelect={() => undefined}
      value="bad" note="Disabled reason remains visible" options={[{id:'plain',label:'Ordinary'},{id:'bad',label:'Block',danger:true},{id:'off',label:'Unavailable',disabled:true}]} />));
    const selected = '.mr-sel-opt[aria-selected="true"]';
    const m = await measure({ctx:'mobile',html:markup,rootAttrs:{'data-theme':'dark'},probes:{...probes,errWeak:'hsl(var(--err-weak))'}},[
      {select:'.m-sheet-title',props:['font-size','font-weight','color']},
      {select:'.mr-sel-opt',props:['font-size','font-weight'],many:'all'},
      {select:selected,props:['color','background-color']},
      {select:'.mr-sel-opt:disabled',props:['opacity']},
    ]);
    expect(m.get('.m-sheet-title','font-size')).toBe('16px');
    expect(m.get('.m-sheet-title','font-weight')).toBe('600');
    expect(m.get('.m-sheet-title','color')).toBe(m.probe('fg'));
    expect(m.getAll('.mr-sel-opt','font-size')).toEqual(['14px','14px','14px']);
    expect(m.getAll('.mr-sel-opt','font-weight')).toEqual(['400','400','400']);
    expect(m.get(selected,'color')).toBe(m.probe('err'));
    expect(m.get(selected,'background-color')).toBe(m.probe('errWeak'));
    expect(m.get('.mr-sel-opt:disabled','opacity')).toBe('1');
    expect((markup.match(/aria-label="Close"/g) ?? []).length).toBe(1);
  });
});
