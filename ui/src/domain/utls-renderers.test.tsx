import { beforeEach, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ModalContext } from '@/components/dialogs/Modal';
import { FieldRenderer } from '@/components/dialogs/FieldSpec';
import { MobileField } from '@/mobile/forms/FormFields';
import type { FieldSpec } from '@/components/dialogs/field-spec';
import type { SelectedTlsSupportDirectory } from '@/domain/utls';
import zh from '@/i18n/locales/zh-CN.json';

const capabilityState = vi.hoisted(() => ({ unavailableReason: null as string | null, directory: null as SelectedTlsSupportDirectory | null }));
vi.mock('@/hooks/use-tls-fingerprints', () => ({ useTlsFingerprints: () => capabilityState.directory ?? (capabilityState.unavailableReason ? {
  observationScope: 'unavailable', byteIdentityScope: 'unavailable', identity: null,
  binarySha256: null, document: null, unavailableReason: capabilityState.unavailableReason,
} : ({
  identity: 'verified-fixture', document: {
    schema: 'polaris-tls-fingerprint-capabilities-v1', enabled: true, os: 'darwin',
    profiles: [
      { id: 'random', displayName: 'Random browser', templateVersion: '', selection: 'process-browser' },
      { id: 'randomized', displayName: 'Randomized ClientHello', templateVersion: '', selection: 'process-seeded-generator' },
      { id: 'chrome', displayName: 'Chrome (Auto)', templateVersion: '155', selection: 'profile' },
    ],
  },
})) }));
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
  (value: string, field = spec) => renderToStaticMarkup(<ModalContext.Provider value={{ cselCloseRef: { current: null } }}><FieldRenderer spec={field} value={value} onChange={() => {}} /></ModalContext.Provider>),
  (value: string, field = spec) => renderToStaticMarkup(<MobileField spec={field} value={value} onChange={() => {}} t={translate} />),
];
beforeEach(() => { capabilityState.unavailableReason = null; capabilityState.directory = null; });
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

it('both renderers preserve saved choices and show a user status instead of native refusal details', () => {
  capabilityState.unavailableReason = 'CORE_ADMISSION_UnsupportedExactObject: exact object unavailable before execution';
  for (const render of renderers) {
    for (const field of [spec, { ...spec, k: 'stlsFp' }]) {
      for (const [value, label] of [['firefox_120', 'firefox_120'], ['@default', zh.node.utls.savedCoreDefault], ['randomized', zh.node.utls.savedRandomized]]) {
        const html = render(value, field);
        expect(html).toContain(zh.node.utls.unavailable);
        expect(html).toContain(label);
        expect(html).not.toContain('CORE_ADMISSION');
        expect(html).not.toContain('exact object unavailable');
        expect(html).not.toContain('{{reason}}');
      }
    }
  }
});

it('both renderers identify expected selected support without calling it a running-core observation', () => {
  capabilityState.directory = {
    evidenceKind: 'compiled-selected-support-v1', unavailableReason: null,
    selectedSpec: { platform: 'mac-arm64', target: 'darwin/arm64', sourceCommit: 'c'.repeat(40), sourceTree: 'd'.repeat(40),
      version: 'fixture', contractSha256: 'e'.repeat(64), coreBytes: 1, coreSha256: 'b'.repeat(64),
      dataOrigin: 'observed-cli-metadata', observationStdoutSha256: 'f'.repeat(64), sourceArchiveSha256: '1'.repeat(64), sourceIndexSha256: '2'.repeat(64), goVersion: 'fixture', requiredGoVersion: 'fixture', buildTags: 'with_utls' },
    document: { schema: 'polaris-tls-support-directory-v1', enabled: true, os: 'darwin', arch: 'arm64',
      defaultFingerprint: 'chrome', legacyChromeAliases: [], randomSemantics: 'per process',
      randomizedSemantics: 'process seed', negotiationEvidence: 'not a completed handshake', profiles: [
        { id: 'random', displayName: 'Random browser', selection: 'process-browser', client: null, templateVersion: null, templateOffersX25519MLKEM768: null },
        { id: 'chrome', displayName: 'Chrome (Auto)', selection: 'profile', client: null, templateVersion: null, templateOffersX25519MLKEM768: null },
      ] },
  };
  for (const render of renderers) {
    for (const [value, label] of [['random', zh.node.utls.random], ['chrome', 'Chrome']]) {
      const html = render(value);
      expect(html).toContain(label);
      expect(html).toContain(zh.node.utls.selectedSupport);
      expect(html).not.toContain(zh.node.utls.unavailable);
      expect(html).not.toContain('Random browser');
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
