import { describe, expect, it } from 'vitest';
import { draftFromSpecs } from '@/components/dialogs/field-spec';
import { allFields, nodeFormGroups, type NodeFieldGroupId } from '@/components/dialogs/node-spec';
import { expandedNodeFormGroups, initialNodeFormGroups } from './node-form-visibility';

describe('node form group visibility', () => {
  it('shows each protocol connection path, including conditional Reality fields, on first open and protocol switch', () => {
    const vless = draftFromSpecs(allFields('vless'));
    expect([...initialNodeFormGroups('vless', vless)]).toEqual(['basic', 'transport']);
    const reality = { ...vless, sec: 'reality' };
    expect(nodeFormGroups('vless').find(group => group.id === 'transport')?.fields
      .some(field => field.k === 'pbk' && field.when?.(reality))).toBe(true);
    expect(initialNodeFormGroups('vless', reality).has('transport')).toBe(true);

    const openvpn = draftFromSpecs(allFields('openvpn-client'));
    expect([...initialNodeFormGroups('openvpn-client', openvpn)]).toEqual(['basic']);
    expect([...initialNodeFormGroups('hysteria2', draftFromSpecs(allFields('hysteria2')))])
      .toContain('transport');
  });

  it('opens configured edit groups without treating dormant conditional defaults as required', () => {
    const blank = draftFromSpecs(allFields('vless'));
    expect(initialNodeFormGroups('vless', { ...blank, engine: 'utls' }, { editing: true }).has('advanced'))
      .toBe(false); // TLS is off; the stale engine value is not visible.
    expect(initialNodeFormGroups('vless', { ...blank, sec: 'tls', ech: true }, { editing: true }).has('advanced'))
      .toBe(true);

    const mesh = draftFromSpecs(allFields('openvpn-client'));
    const open = initialNodeFormGroups('openvpn-client', {
      ...mesh, meshRoutes: '10.10.0.0/16', extraJson: '{"x":1}',
    }, { editing: true });
    expect(open.has('routing')).toBe(true);
    expect(open.has('advanced')).toBe(true);
  });

  it('keeps changed shared options visible after protocol switch and reopens a manually closed error group', () => {
    const changed = initialNodeFormGroups(
      'trojan', draftFromSpecs(allFields('trojan')),
      { detour: 'another-node', bindInterface: 'wlan0', bindInterfaceEditable: true },
    );
    expect(changed.has('advanced')).toBe(true);
    const manualFold = new Set<NodeFieldGroupId>(['basic']);
    const afterError = expandedNodeFormGroups(manualFold, 'advanced');
    expect(afterError.has('advanced')).toBe(true);
    expect(manualFold.has('advanced')).toBe(false);
  });
});
