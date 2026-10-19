import { expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ModalContext } from '@/components/dialogs/Modal';
import { FieldRenderer } from '@/components/dialogs/FieldSpec';
import { MobileField } from '@/mobile/forms/FormFields';
import type { FieldSpec } from '@/components/dialogs/field-spec';
import zh from '@/i18n/locales/zh-CN.json';

vi.mock('@/hooks/use-tls-fingerprints', () => ({ useTlsFingerprints: () => ({
  identity: 'verified-fixture', document: {
    schema: 'polaris-tls-fingerprint-capabilities-v1', enabled: true, os: 'darwin',
    profiles: [
      { id: 'random', displayName: 'Random browser', templateVersion: '', selection: 'process-browser' },
      { id: 'randomized', displayName: 'Randomized ClientHello', templateVersion: '', selection: 'process-seeded-generator' },
      { id: 'chrome', displayName: 'Chrome (Auto)', templateVersion: '155', selection: 'profile' },
    ],
  },
}) }));
vi.mock('react-i18next', async (importOriginal) => ({
  ...await importOriginal<typeof import('react-i18next')>(),
  useTranslation: () => ({ t: translate }),
}));
function translate(key: string): string {
  return key.split('.').reduce<unknown>((value, part) => (value as Record<string, unknown>)?.[part], zh) as string || key;
}
await (await import('@/i18n')).i18nReady;
const spec: FieldSpec = { t: 'select', k: 'fp', label: 'uTLS', options: [] };
const renderers = [
  (value: string) => renderToStaticMarkup(<ModalContext.Provider value={{ cselCloseRef: { current: null } }}><FieldRenderer spec={spec} value={value} onChange={() => {}} /></ModalContext.Provider>),
  (value: string) => renderToStaticMarkup(<MobileField spec={spec} value={value} onChange={() => {}} t={translate} />),
];
it('both renderers translate default, disabled and Random and simplify Chrome', () => {
  for (const render of renderers) {
    for (const [value, label] of [['', '默认'], ['none', '关闭 uTLS'], ['random', '随机'], ['chrome', 'Chrome']]) {
      const html = render(value);
      expect(html).toContain(label);
      expect(html).not.toContain('Random browser');
      expect(html).not.toContain('Chrome (Auto)');
      expect(html).not.toContain('Randomized ClientHello');
      expect(html).not.toContain('已保存：');
    }
  }
});
it('both renderers label saved removed settings and disclose preservation', () => {
  for (const render of renderers) {
    for (const [value, label] of [['@default', zh.node.utls.savedCoreDefault], ['randomized', zh.node.utls.savedRandomized]]) {
      const html = render(value);
      expect(html).toContain(label);
      expect(html).toContain(zh.node.utls.savedHint);
    }
  }
});
it('mobile preserves the requested menu order and disables off for REALITY', () => {
  const html = renderToStaticMarkup(<MobileField spec={spec} value="" reality onChange={() => {}} t={translate} />);
  expect(html.indexOf('value=""')).toBeLessThan(html.indexOf('value="none"'));
  expect(html.indexOf('value="none"')).toBeLessThan(html.indexOf('value="random"'));
  expect(html.indexOf('value="random"')).toBeLessThan(html.indexOf('value="chrome"'));
  expect(html).toMatch(/<option[^>]*value="none"[^>]*disabled=""/);
  expect(html).toContain(zh.node.utls.realityRequired);
});

it('shows an engine-specific conflict and prevents incompatible browser selection', () => {
  const html = renderToStaticMarkup(<MobileField spec={spec} value="" tlsEngine="apple" onChange={() => {}} t={translate} />);
  expect(html).toContain(zh.node.utls.engineUnsupported);
  expect(html).toMatch(/<option[^>]*value="random"[^>]*disabled=""/);
  expect(html).toMatch(/<option[^>]*value="chrome"[^>]*disabled=""/);
  expect(html).not.toMatch(/<option[^>]*value=""[^>]*disabled=""/);
  const desktop = renderToStaticMarkup(<ModalContext.Provider value={{ cselCloseRef: { current: null } }}>
    <FieldRenderer spec={spec} value="chrome" tlsEngine="apple" onChange={() => {}} />
  </ModalContext.Provider>);
  expect(desktop).toContain(zh.node.utls.engineUnsupported);
});
