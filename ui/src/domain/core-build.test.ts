import { expect, it } from 'vitest';
import { classifyCoreBuild, comparableCoreVersion, decideCoreOverride, reseedApplied } from './core-build';

it('managed builds preserve revision order and can return to newer official releases', () => {
  const bundled = '1.15.0-alpha.8.polaris.2';
  for (const [version, reseed] of [
    ['1.15.0-alpha.8', true], ['1.15.0-alpha.8.polaris.1', true],
    [bundled, false], ['1.15.0-alpha.9', false], ['1.15.0', false],
    ['1.15.0-alpha.7-other', false], ['unknown', false],
  ] as const) {
    expect(decideCoreOverride(classifyCoreBuild(version), comparableCoreVersion(version), bundled).reseed,
      version).toBe(reseed);
  }
  expect(classifyCoreBuild(`sing-box version ${bundled}`)).toBe('polaris');
  expect(classifyCoreBuild(`${bundled}-other`)).toBe('fork');
  expect(reseedApplied('1.15.0-alpha.8', bundled)).toBe(false);
  expect(reseedApplied(bundled, bundled)).toBe(true);
  expect(decideCoreOverride('polaris', bundled, '1.15.0-alpha.9').reseed).toBe(true);
});
