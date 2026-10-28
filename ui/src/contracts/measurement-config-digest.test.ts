import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { configContentDigest, configCanonicalText } from '@/lib/staged-config';

const fixtures = JSON.parse(readFileSync(new URL('./measurement-config-digest.fixture.json', import.meta.url), 'utf8')) as Array<{ input: Record<string, unknown>; canonical: string; sha256: string }>;

it.each(fixtures)('binds the actual canonical config bytes to independently generated SHA256 $sha256', async fixture => {
  if (fixture.canonical.includes('__proto__')) expect(Object.prototype.hasOwnProperty.call(fixture.input.a, '__proto__')).toBe(true);
  expect(configCanonicalText(fixture.input)).toBe(fixture.canonical);
  expect(await configContentDigest(fixture.input)).toBe(fixture.sha256);
});
