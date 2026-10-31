/** Human presentation of a typed refusal; no diagnostic strings enter the view. */
export function coreAdmissionMessageKey(kind: unknown): string {
  switch (kind) {
    case 'UnsupportedExactObject': return 'helper.coreAdmission.unsupportedExactObject';
    case 'UnsupportedCapability': return 'helper.coreAdmission.unsupportedCapability';
    case 'Missing': case 'Mismatch': case 'Changed': return 'helper.coreAdmission.damaged';
    case 'Busy': return 'helper.coreAdmission.busy';
    default: return 'helper.coreAdmission.unavailable';
  }
}
export function coreAdmissionIsDamage(kind: unknown): boolean {
  return kind === 'Missing' || kind === 'Mismatch' || kind === 'Changed';
}
