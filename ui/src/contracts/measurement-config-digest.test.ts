import { expect, it } from 'vitest';
import fixtures from './measurement-config-digest.fixture.json';
import { configContentDigest, configCanonicalText } from '@/lib/staged-config';

it.each(fixtures)('binds the actual canonical config bytes to independently generated SHA256 $sha256', async fixture => {
  expect(configCanonicalText(fixture.input)).toBe(fixture.canonical);
  expect(await configContentDigest(fixture.input)).toBe(fixture.sha256);
});
