import { describe, expect, it } from 'vitest';
import { resourceUpdateOutcome } from './resource-update-outcome';

describe('resource update receipts', () => {
  it('rejects missing, empty and short responses', () => {
    expect(resourceUpdateOutcome(null, 1).status).toBe('empty');
    expect(resourceUpdateOutcome([], 1).status).toBe('empty');
    expect(resourceUpdateOutcome([{ ok: true }], 2)).toMatchObject({ status: 'partial', succeeded: 1, failed: 1 });
    expect(resourceUpdateOutcome([{ ok: true }], 0)).toMatchObject({ status: 'success', succeeded: 1 });
    expect(resourceUpdateOutcome([{ resource: {} }], 1).status).toBe('failed');
  });

  it('counts success, failure and cancellation separately', () => {
    expect(resourceUpdateOutcome([
      { ok: true, id: 'builtin:geosite-cn' },
      { ok: false, errorCode: 'RULE_RESOURCE_DOWNLOAD_FAILED' },
      { ok: false, errorCode: 'RULE_RESOURCE_CANCELLED' },
    ], 3)).toEqual({ status: 'partial', succeeded: 1, failed: 1, cancelled: 1 });
    expect(resourceUpdateOutcome([{ ok: false, errorCode: 'RULE_RESOURCE_CANCELLED' }], 1).status).toBe('cancelled');
  });
});
