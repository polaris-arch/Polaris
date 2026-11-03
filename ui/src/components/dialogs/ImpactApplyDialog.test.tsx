import { describe, expect, it, vi } from 'vitest';
import type { ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ImpactPrompt } from './use-impact-confirmation';
import type { ModalProps } from './Modal';

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
// The browser suite covers the actual Modal. Here invoke the real disclosure/footer handlers without a DOM.
vi.mock('./Modal', () => ({ Modal: () => null }));
import { ImpactApplyDialog, ImpactApplySummary } from './ImpactApplyDialog';

function prompt(classes: string[], updateClass = 2, reconnectRequired = true): ImpactPrompt {
  return { token: 'private-token', generation: 7, draft: [], version: null, hostEffects: [],
    coreImpacts: [{ path: 'private-path', reason: 'private-reason', updateClass, reconnectRequired,
      affectedSessionClasses: classes }] };
}
function dialog(value: ImpactPrompt, onConfirm = vi.fn(), onDismiss = vi.fn()) {
  const modal = ImpactApplyDialog({ prompt: value, onConfirm, onDismiss }) as ReactElement<ModalProps>;
  const footer = modal.props.footer as ReactElement<{ children: ReactElement<{ disabled?: boolean; onClick: () => void }>[] }>;
  return { confirm: footer.props.children[1]!, cancel: footer.props.children[0]!, onConfirm, onDismiss,
    html: renderToStaticMarkup(<ImpactApplySummary prompt={value} />) };
}
function privateFieldsStayHidden(html: string) {
  for (const raw of ['private-token', 'private-path', 'private-reason', 'private-session', 'api_generation_handoff_v1']) expect(html).not.toContain(raw);
}

describe('desktop impact consent disclosure', () => {
  it.each([
    { name: 'TUN', classes: ['existing TUN TCP flows', 'existing TUN UDP associations', 'changed ingress admissions'],
      expected: ['home.impactApplyTunTcpReset', 'home.impactApplyTunUdpReset', 'home.impactApplyTunRollback'], absent: 'home.impactApplyApiStreams' },
    { name: 'API/dashboard', classes: ['API streams on the replaced ingress', 'api_generation_handoff_v1', 'dashboard updater'],
      expected: ['home.impactApplyApiStreams', 'home.impactApplyDashboard'], absent: 'home.impactApplyTunRollback' },
    { name: 'listener/UDP', classes: ['changed ingress admissions', 'affected UDP associations', 'dependent listener handoff'],
      expected: ['home.impactApplyListenerAdmissions', 'home.impactApplyListenerUdp', 'home.impactApplyListenerHandoff'], absent: 'home.impactApplyTunRollback' },
  ])('discloses $name scope and confirms only the retained callback', ({ classes, expected, absent }) => {
    const view = dialog(prompt(classes));
    for (const key of expected) expect(view.html).toContain(key);
    expect(view.html).not.toContain(absent);
    expect(view.html).toContain('home.impactApplySavedPending');
    expect(view.html).not.toContain('home.impactApplyReconnect');
    expect(view.confirm.props.disabled).toBe(false);
    view.confirm.props.onClick();
    expect(view.onConfirm).toHaveBeenCalledOnce();
    privateFieldsStayHidden(view.html);
  });

  it.each([
    { classes: ['private-session'], updateClass: 2 },
    { classes: ['existing TUN TCP flows'], updateClass: 1 },
    ...[0, 4, 5, 99].map(updateClass => ({ classes: ['API streams on the replaced ingress'], updateClass })),
    { classes: [], updateClass: 2 },
    { classes: ['new admissions'], updateClass: 3 },
  ])('blocks unknown or undisclosed risk even if the footer handler is invoked: %j', ({ classes, updateClass }) => {
    const view = dialog(prompt(classes, updateClass));
    expect(view.html).toContain('role="alert"');
    expect(view.html).toContain('home.impactApplyUnknown');
    expect(view.confirm.props.disabled).toBe(true);
    view.confirm.props.onClick();
    expect(view.onConfirm).not.toHaveBeenCalled();
    view.cancel.props.onClick();
    expect(view.onDismiss).toHaveBeenCalledOnce();
    privateFieldsStayHidden(view.html);
  });

  it('keeps known metadata and host-only choices finite, and rejects unknown host kinds', () => {
    const metadata = dialog(prompt(['new admissions', 'log emissions', 'new TLS admissions',
      'captured provider consumers', 'new clock consumers'], 1, false));
    expect(metadata.confirm.props.disabled).toBe(false);
    for (const key of ['NewAdmissions', 'LogEmissions', 'TlsConsumers', 'ClockConsumers']) expect(metadata.html).toContain('home.impactApply' + key);
    const value = { ...prompt([], 2, false), coreImpacts: [], hostEffects: [{ kind: 'dns' as const,
      scopeDigest: 'private-scope', beforeDigest: 'private-before', targetDigest: 'private-target' }] };
    const host = dialog(value);
    expect(host.confirm.props.disabled).toBe(false);
    expect(host.html).toContain('home.impactApplyDns');
    expect(host.html).not.toContain('private-scope');
    const unknown = dialog({ ...value, hostEffects: [{ ...value.hostEffects[0]!, kind: 'private-session' }] } as unknown as ImpactPrompt);
    expect(unknown.confirm.props.disabled).toBe(true);
    expect(unknown.html).toContain('home.impactApplyUnknown');
    privateFieldsStayHidden(unknown.html);
  });
});
