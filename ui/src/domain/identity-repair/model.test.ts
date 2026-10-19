import { describe, expect, it } from 'vitest';
import { hasDuplicateServerIds, identityRepairErrorKey, identityRepairRequest, type IdentityConflicts } from './model';
const preview: IdentityConflicts = { version: 'full-config-version', groups: [
  { indices: [2, 4], repairable: true, records: [] }, { indices: [5, 8], repairable: true, records: [] },
] };
describe('explicit duplicate identity recovery', () => {
  it('never defaults a keeper and requires every group', () => {
    expect(identityRepairRequest(preview, {})).toBeNull();
    expect(identityRepairRequest(preview, { 0: 2 })).toBeNull();
    expect(identityRepairRequest(preview, { 0: 3, 1: 5 })).toBeNull();
    expect(identityRepairRequest(preview, { 0: 4, 1: 5 })).toEqual({ version: preview.version, choices: [
      { indices: [2,4], keeperIndex: 4 }, { indices: [5,8], keeperIndex: 5 },
    ] });
  });
  it('rejects protected groups and an empty or missing preview', () => {
    expect(identityRepairRequest({ ...preview, groups: [{ ...preview.groups[0]!, repairable:false }] }, { 0:2 })).toBeNull();
    expect(identityRepairRequest({ ...preview, groups:[] }, {})).toBeNull();
    expect(identityRepairRequest(null, {})).toBeNull();
  });
  it('shows the recovery entry for duplicate IDs, including unrelated selected nodes', () => {
    expect(hasDuplicateServerIds([{id:'a'},{id:'a'},{id:'selected'}])).toBe(true);
    expect(hasDuplicateServerIds([{id:'a'},{id:'b'}])).toBe(false);
  });
  it('maps only stable safe error codes', () => {
    expect(identityRepairErrorKey('serverIdentityRevisionChanged')).toBe('identityRepair.stale');
    expect(identityRepairErrorKey('serverIdentityRepairUnsupported')).toBe('identityRepair.unsupported');
    expect(identityRepairErrorKey('secret raw diagnostic')).toBe('identityRepair.failed');
  });
});
