export interface IdentityConflictRecord { index: number; name: string; protocol: string; address: string | null; port: number | null }
export interface IdentityConflictGroup { indices: number[]; repairable: boolean; records: IdentityConflictRecord[] }
export interface IdentityConflicts { version: string; groups: IdentityConflictGroup[] }
export interface IdentityRepairChoice { indices: number[]; keeperIndex: number }
export interface IdentityRepairRequest { version: string; choices: IdentityRepairChoice[] }
export interface IdentityRepairReceipt { status: 'saved'; requiresApply: boolean }

/** No first-record default: each group needs the user's explicit choice. */
export function identityRepairRequest(preview: IdentityConflicts | null, keepers: Record<number, number>): IdentityRepairRequest | null {
  if (!preview?.groups.length) return null;
  const choices: IdentityRepairChoice[] = [];
  for (const [groupIndex, group] of preview.groups.entries()) {
    const keeperIndex = keepers[groupIndex];
    if (!group.repairable || keeperIndex === undefined || !group.indices.includes(keeperIndex)) return null;
    choices.push({ indices: [...group.indices], keeperIndex });
  }
  return { version: preview.version, choices };
}

export function hasDuplicateServerIds(servers: readonly { id: string }[]): boolean {
  const ids = new Set<string>();
  return servers.some(({ id }) => { if (ids.has(id)) return true; ids.add(id); return false; });
}

export function identityRepairErrorKey(code: string | undefined): string {
  switch (code) {
    case 'serverIdentityRevisionChanged': return 'identityRepair.stale';
    case 'serverIdentityRepairUnsupported': return 'identityRepair.unsupported';
    case 'serverIdentityRepairInvalidChoice': return 'identityRepair.invalidChoice';
    default: return 'identityRepair.failed';
  }
}
