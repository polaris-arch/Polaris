import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CoexWindowsSources } from './CoexWindowsSources';
import { decodeCoexSnapshot } from '@/contracts/coex-snapshot';
import fixture from '@/contracts/coex-windows.rust.fixture.json';
import en from '@/i18n/locales/en-US.json';
const t = (key: string) => key.split('.').reduce<unknown>((v, k) => (v as Record<string, unknown>)[k], en) as string;
describe('Windows five-source settings projection', () => {
  it.each(Object.entries(fixture.snapshots))('renders %s collapsed with explicit uncertainty and no automatic rows or safety verdict', (_name, input) => {
    const snapshot = decodeCoexSnapshot(input); if (snapshot.schemaVersion !== 2) throw Error('fixture');
    const html = renderToStaticMarkup(<CoexWindowsSources snapshot={snapshot} t={t} />);
    for (const id of ['adapters', 'addresses', 'routes4', 'routes6', 'ras']) expect(html).toContain(`data-coex-source="${id}"`);
    expect(html).toContain(en.settings.coex.windowsHint); expect(html).toContain(en.settings.coex.unknown);
    expect(html).not.toContain(en.settings.coex.sourceUnavailable); expect(html).not.toContain('12025550123');
    expect(html).not.toContain('<ol'); expect(html).not.toContain('class="good"');
  });
});
