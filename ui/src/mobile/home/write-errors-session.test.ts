import { describe, expect, it, vi } from 'vitest';
import type { TFunction } from 'i18next';
import type { WriteErrors } from './view-model';
const errors = vi.hoisted(() => vi.fn());
vi.mock('@/lib/error-handler', () => ({ toast: { error: errors } }));
import { createRunWrite } from './write-errors';
const t = ((key: string) => key) as TFunction;

describe('mobile write feedback session ownership', () => {
  it('old successful operation cannot clear a newer sheet error', async () => {
    let state: WriteErrors = { 'rule-add': 'new failure' };
    let session = 1;
    let finish!: () => void;
    const op = new Promise<void>((resolve) => { finish = resolve; });
    const run = createRunWrite((update) => { state = update(state); }, t);
    const pending = run('rule-add', () => op, () => session === 1);
    session = 2; finish(); await pending;
    expect(state).toEqual({ 'rule-add': 'new failure' });
  });
  it('old failed operation remains visible without poisoning the new sheet', async () => {
    let state: WriteErrors = {};
    let reject!: (error: Error) => void;
    const op = new Promise<void>((_, fail) => { reject = fail; });
    const run = createRunWrite((update) => { state = update(state); }, t);
    const pending = run('rule-add', () => op, () => false);
    reject(new Error('failed')); await pending;
    expect(state).toEqual({});
    expect(errors).toHaveBeenCalledWith('mobileHome.actionFailedWithReason');
  });
});
