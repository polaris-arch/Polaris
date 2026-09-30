import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  assertInstalledAndroidPackage,
  DEBUG_APP,
  parseAndroidQaTarget,
  RELEASE_APP,
} from './android-cdp.mjs';

test('Android QA defaults to Debug and requires an explicit Release flag', () => {
  assert.deepEqual(parseAndroidQaTarget(['emulator-5554']),
    { serial: 'emulator-5554', app: DEBUG_APP });
  assert.deepEqual(parseAndroidQaTarget(['emulator-5554', '--release']),
    { serial: 'emulator-5554', app: RELEASE_APP });
  for (const args of [[], ['--release'], ['emulator-5554', '--debug'],
    ['emulator-5554', '--release', '--release'], ['emulator-5554', 'another-device']]) {
    assert.throws(() => parseAndroidQaTarget(args));
  }
});

test('installed-package preflight checks exact IDs before an Activity can launch', () => {
  const fakeAdb = output => (...args) => {
    assert.deepEqual(args, ['shell', 'pm', 'list', 'packages', 'com.polaris2.app']);
    return output;
  };
  const both = fakeAdb(`package:${RELEASE_APP}\npackage:${DEBUG_APP}\n`);
  assert.doesNotThrow(() => assertInstalledAndroidPackage(both, DEBUG_APP));
  assert.doesNotThrow(() => assertInstalledAndroidPackage(both, RELEASE_APP));
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(`package:${RELEASE_APP}\n`), DEBUG_APP),
    /com\.polaris2\.app\.debug is not installed.*pass --release/);
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(`package:${DEBUG_APP}\n`), RELEASE_APP),
    /com\.polaris2\.app is not installed.*omit --release/);
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(''), DEBUG_APP),
    /Neither Debug nor Release package is installed/);
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(`package:${DEBUG_APP}.other\n`), DEBUG_APP),
    /com\.polaris2\.app\.debug is not installed/);
  assert.throws(() => assertInstalledAndroidPackage(both, 'com.example.other'),
    /Unknown Android QA package/);
});

test('Android QA entrypoints reject the wrong variant and launch the native Activity class', () => {
  const sdk = mkdtempSync(join(tmpdir(), 'polaris-qa-adb-'));
  const calls = join(sdk, 'calls');
  try {
    mkdirSync(join(sdk, 'platform-tools'));
    writeFileSync(join(sdk, 'platform-tools', 'adb'),
      '#!/bin/sh\nprintf "%s\\n" "$*" >> "$ANDROID_QA_CALLS"\n'
      + 'if [ "$4" = "am" ] && [ "$5" = "start" ]; then exit 66; fi\n'
      + 'printf "%s\\n" "$ANDROID_QA_PACKAGES"\n',
      { mode: 0o755 });
    for (const script of ['test-android-mobile.mjs', 'test-android-mobile-layout.mjs',
      'test-android-vpn-permission.mjs']) {
      for (const { option, installed, missing } of [
        { option: [], installed: RELEASE_APP, missing: DEBUG_APP },
        { option: ['--release'], installed: DEBUG_APP, missing: RELEASE_APP },
      ]) {
        writeFileSync(calls, '');
        const result = spawnSync(process.execPath,
          [fileURLToPath(new URL(script, import.meta.url)), 'emulator-5554', ...option], {
            encoding: 'utf8',
            env: { ...process.env, ANDROID_HOME: sdk, ANDROID_QA_CALLS: calls,
              ANDROID_QA_PACKAGES: `package:${installed}` },
          });
        assert.equal(result.status, 1, `${script}: ${result.stderr}`);
        assert.ok(result.stderr.includes(`${missing} is not installed`), `${script}: ${result.stderr}`);
        assert.equal(readFileSync(calls, 'utf8'),
          '-s emulator-5554 shell pm list packages com.polaris2.app\n');

        const selected = option.length ? RELEASE_APP : DEBUG_APP;
        writeFileSync(calls, '');
        const launch = spawnSync(process.execPath,
          [fileURLToPath(new URL(script, import.meta.url)), 'emulator-5554', ...option], {
            encoding: 'utf8',
            env: { ...process.env, ANDROID_HOME: sdk, ANDROID_QA_CALLS: calls,
              ANDROID_QA_PACKAGES: `package:${selected}` },
          });
        assert.equal(launch.status, 1, `${script}: ${launch.stderr}`);
        const observed = readFileSync(calls, 'utf8').trim().split('\n');
        assert.deepEqual(observed, [
          '-s emulator-5554 shell pm list packages com.polaris2.app',
          ...(script === 'test-android-vpn-permission.mjs'
            ? [`-s emulator-5554 shell appops get ${selected} ACTIVATE_VPN`] : []),
          `-s emulator-5554 shell am start -W -n ${selected}/com.polaris2.app.MainActivity`,
        ]);
      }
    }
  } finally {
    rmSync(sdk, { recursive: true, force: true });
  }
});
