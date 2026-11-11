import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const source = readFileSync(
  fileURLToPath(new URL('./SettingsAbout.tsx', import.meta.url)),
  'utf8',
);

describe('SettingsAbout external links have a single navigation owner', () => {
  it('does not retain a second browser-owned navigation or fallback opener', () => {
    expect(source).not.toMatch(/<a[\s>]/);
    expect(source).not.toContain('target="_blank"');
    expect(source).not.toContain('window.open(');
  });
});
