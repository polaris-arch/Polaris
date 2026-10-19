import { describe, expect, it } from 'vitest';
import { createInputRequest } from './use-input-request';

describe('input-scoped preview ownership', () => {
  it('rejects stale A after edits, including returning to A', async () => {
    const owner = createInputRequest('A');
    const a = owner.begin();
    owner.update('B');
    expect(a()).toBe(false);
    owner.update('A');
    expect(a()).toBe(false);
    const current = owner.begin();
    expect(current()).toBe(true);
    owner.update('A');
    expect(current()).toBe(true);
  });
  it('supersedes same-input requests and never revives disposed replies', () => {
    const owner = createInputRequest('A');
    const first = owner.begin();
    const second = owner.begin();
    expect(first()).toBe(false);
    expect(second()).toBe(true);
    owner.dispose(); owner.mount();
    expect(second()).toBe(false);
    expect(owner.begin()()).toBe(true);
  });
});
