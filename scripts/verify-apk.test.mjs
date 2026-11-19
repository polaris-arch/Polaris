import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempDisposableSync, readFileSync, readdirSync, realpathSync, renameSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fixtureZip } from './lib/formal-package-fixtures.mjs';
import { emitAndroidInputs, prepareAndroid } from './mobile-core/android-ci.mjs';
import { openComponentZip, withComponentZip } from './mobile-core/component-admission.mjs';
import { observeNdkInput, prepareSdkView } from './mobile-core/prepare-ndk-linux.mjs';

import {
  ABIS,
  APK_DECLARED_PERMISSIONS,
  APK_PERMISSION_REGISTRY,
  ASSET_UP,
  DEFAULT_ABI,
  LICENSE_FILES,
  NATIVE_MARKERS,
  REQUIRED_ELF_SECTIONS,
  SRS_DIR,
  ZIP,
  apkViolations,
  countOccurrences,
  elfSectionNames,
  elfPageAlignment,
  isDebugSection,
  parseAapt2Permissions,
  parseArgs,
  parseZipEntries,
  permissionViolations,
  strippedReport,
  verifyAarInterfaces,
  verifySdkPayload,
  zipOverheadBound,
} from './verify-apk.mjs';

/** 夹具默认造缺省 ABI（arm64-v8a）的包；`ABI` 这个名字保留下来只是为了少改一堆现有用例。 */
const ABI = DEFAULT_ABI;

test('hosted Android cache outputs reject an invalid identity or multiline path before writing', () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-android-hosted-outputs-'));
  const output = join(fixture.path, 'github-output');
  const environment = join(fixture.path, 'github-env');
  const env = { GITHUB_OUTPUT: output, GITHUB_ENV: environment };
  const paths = Object.fromEntries(['go', 'gomobile', 'gobind', 'ndk', 'jdk', 'sdk', 'source',
    'work', 'cache', 'consume', 'verify', 'carrier'].map(name => [name, join(fixture.path, name)]));
  for (const file of [output, environment]) writeFileSync(file, 'retained\n');
  for (const bad of ['', 'A'.repeat(64), 'g'.repeat(64), 'a'.repeat(63), 'a'.repeat(65), 'a'.repeat(64) + '\n']) {
    assert.throws(() => emitAndroidInputs(env, paths, bad, 'libbox.aar'), /cache fingerprint/);
  }
  assert.throws(() => emitAndroidInputs(env, { ...paths, work: '/work\nINJECT=1' }, 'a'.repeat(64),
    'libbox.aar'), /line controls/);
  assert.equal(readFileSync(output, 'utf8'), 'retained\n');
  assert.equal(readFileSync(environment, 'utf8'), 'retained\n');
  const fingerprint = 'a'.repeat(64);
  emitAndroidInputs(env, paths, fingerprint, 'libbox.aar');
  assert.equal(readFileSync(output, 'utf8'),
    `retained\nfingerprint=${fingerprint}\nwork=${paths.work}\ncache=${paths.cache}\n`);
  assert.ok(readFileSync(environment, 'utf8').includes('POLARIS_ANDROID_COMPONENT=' + join(paths.work, 'libbox.aar') + '\n'));
});

test('hosted Android preparation emits one bound handoff and halts on preparation faults', {
  skip: process.platform !== 'linux',
}, async () => {
  const source = JSON.parse(readFileSync(new URL('./mobile-core/source-contract.json', import.meta.url)));
  for (const failure of ['capacity', 'NDK', 'source', null]) {
    using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-android-hosted-prep-'));
    const [sdk, ndk, jdk, output, environment] = ['sdk', 'ndk', 'jdk', 'github-output', 'github-env']
      .map(name => join(fixture.path, name));
    mkdirSync(join(sdk, 'platforms', source.android.bootclasspath.platform), { recursive: true });
    mkdirSync(ndk);
    mkdirSync(join(jdk, 'bin'), { recursive: true });
    const go = join(fixture.path, 'go');
    for (const file of [go, join(jdk, 'bin/java'), join(jdk, 'bin/javac'),
      join(ndk, 'source.properties'), join(sdk, 'platforms', source.android.bootclasspath.platform, 'android.jar')]) {
      writeFileSync(file, 'controlled ' + file);
    }
    for (const file of [output, environment]) writeFileSync(file, 'retained\n');
    const env = { GITHUB_ACTIONS: 'true', ANDROID_SDK_ROOT: sdk, JAVA_HOME_17_X64: jdk,
      GITHUB_OUTPUT: output, GITHUB_ENV: environment, ...(failure === 'capacity' ? { GOMAXPROCS: 'auto' } : {}) };
    const steps = [], observations = [];
    const preparation = prepareAndroid({
      env, home: fixture.path, observeCapacity: () => ({ jobCap: 1 }), report: row => observations.push(row.phase),
      run: async ([program, ...args], options) => {
        if (program === 'git') return args.includes('status') ? '' : '1'.repeat(40);
        if (program === 'node' && args.includes('--check-license-only')) return '';
        if (program === 'which' && args[0] === 'go') return go;
        if (program === go && args[0] === 'version') return 'go version go' + source.goVersion + ' linux/amd64';
        if (program === go && args[0] === 'install') {
          assert.deepEqual(args, ['install', 'github.com/sagernet/gomobile/cmd/gomobile@' + source.gomobileVersion,
            'github.com/sagernet/gomobile/cmd/gobind@' + source.gobindVersion]);
          assert.equal(options.env.GOMAXPROCS, '1');
          assert.ok(options.env.GOMODCACHE.startsWith(fixture.path));
          for (const tool of ['gomobile', 'gobind']) writeFileSync(join(options.env.GOBIN, tool), 'controlled ' + tool);
          steps.push('install');
          return '';
        }
        if (program === 'node' && args[0].endsWith('/prepare-ndk-linux.mjs')) {
          steps.push('NDK');
          if (failure === 'NDK') throw new Error('controlled NDK refusal');
          return ndk;
        }
        if ([join(jdk, 'bin/java'), join(jdk, 'bin/javac')].includes(program)) {
          assert.deepEqual(args, ['--version']);
          return 'controlled JDK 17';
        }
        assert.fail('unexpected hosted operation: ' + program);
      },
      prepare: async (checkout, declaration) => {
        steps.push('source');
        assert.ok(checkout.startsWith(fixture.path));
        assert.equal(declaration.sourceCommit, source.sourceCommit);
        if (failure === 'source') throw new Error('controlled source refusal');
      },
    });
    if (!failure) {
      await preparation;
      assert.deepEqual(observations, ['android-tool-prepare-capacity-sample', 'android-tool-prepare-capacity']);
      assert.deepEqual(steps, ['install', 'NDK', 'source']);
      assert.match(readFileSync(output, 'utf8'), /\nfingerprint=[0-9a-f]{64}\nwork=.+\ncache=.+\n$/);
      assert.match(readFileSync(environment, 'utf8'), /\nPOLARIS_ANDROID_RECEIPT=.+\/component-receipt\.json\n/);
      continue;
    }
    await assert.rejects(preparation, failure === 'capacity' ? /positive integer/ : new RegExp('controlled ' + failure + ' refusal'));
    assert.equal(observations[0], 'android-tool-prepare-capacity-sample');
    assert.deepEqual(steps, failure === 'capacity' ? [] : failure === 'NDK' ? ['install', 'NDK'] : ['install', 'NDK', 'source']);
    assert.equal(readFileSync(output, 'utf8'), 'retained\n');
    assert.equal(readFileSync(environment, 'utf8'), 'retained\n');
  }
});

test('formal Android SDK preparation requires existing license and exact archive bytes', {
  skip: process.platform !== 'linux',
}, () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-ndk-admission-'));
  const profile = fileURLToPath(new URL('./mobile-core/ndk-r30-linux.json', import.meta.url));
  const policy = JSON.parse(readFileSync(profile));
  const script = fileURLToPath(new URL('./mobile-core/prepare-ndk-linux.mjs', import.meta.url));
  const sdk = join(fixture.path, 'sdk'), work = join(fixture.path, 'work');
  const licenses = join(sdk, 'licenses'), license = join(licenses, policy.licenseId);
  mkdirSync(licenses, { recursive: true });
  mkdirSync(work, { mode: 0o700 });
  const run = (...args) => {
    const child = spawnSync(process.execPath, [script, '--profile', profile, '--sdk', sdk, ...args],
      { encoding: 'utf8', timeout: 5000 });
    assert.ifError(child.error);
    return child;
  };
  const licenseOnly = '--check-license-only';
  assert.equal(run(licenseOnly).status, 1);
  assert.deepEqual(readdirSync(licenses), []);
  writeFileSync(license, 'different-license\n');
  assert.match(run(licenseOnly).stderr, /license is not already accepted/);
  const accepted = policy.licenseSha1 + '\r\n';
  writeFileSync(license, accepted);
  assert.equal(run(licenseOnly).status, 0);
  assert.equal(run(licenseOnly, work).status, 1);
  assert.deepEqual(readdirSync(work), []);
  const archive = join(fixture.path, 'unexpected.zip');
  writeFileSync(archive, 'unexpected bytes');
  const failed = run(work, archive);
  assert.equal(failed.status, 1);
  assert.equal(failed.stdout, '');
  assert.match(failed.stderr, /archive size\/type differs/);
  const target = join(work, 'polaris-mobile-ndk-' + policy.ndkRevision);
  assert.deepEqual(readdirSync(target), []);
  assert.match(run(work, archive).stderr, /retained formal NDK slot refused/);
  const alias = join(fixture.path, 'work-alias');
  symlinkSync(work, alias);
  assert.match(run(alias, archive).stderr, /cannot be redirected/);
  assert.equal(readFileSync(license, 'utf8'), accepted);
});

test('formal NDK evidence retains original bytes and the SDK view admits only the reviewed inputs', {
  skip: process.platform !== 'linux',
}, () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-ndk-input-'));
  const profileBytes = readFileSync(new URL('./mobile-core/ndk-r30-linux.json', import.meta.url));
  const profile = JSON.parse(profileBytes);
  const pin = bytes => createHash('sha256').update(bytes).digest('hex');
  const root = fixture.path, work = join(root, 'work'), sdk = join(root, 'sdk'), ndk = join(root, 'ndk');
  const bootclasspath = { platform: 'android-36', sha256: pin('controlled bootclasspath') };
  const jar = join(sdk, 'platforms', bootclasspath.platform, 'android.jar');
  const file = join(ndk, 'source.properties');
  const stable = `Pkg.Desc = Android NDK\nPkg.Revision = ${profile.ndkSourceRevision}\n`
    + `Pkg.BaseRevision = ${profile.ndkRevision}\nPkg.ReleaseName = r30\n`;
  assert.equal(Buffer.byteLength(stable), profile.sourcePropertiesBytes);
  assert.equal(pin(stable), profile.sourcePropertiesSha256);
  mkdirSync(work, { mode: 0o700 });
  mkdirSync(ndk);
  mkdirSync(join(sdk, 'platforms', bootclasspath.platform), { recursive: true });
  mkdirSync(join(sdk, 'build-tools'));
  mkdirSync(join(sdk, 'platform-tools'));
  mkdirSync(join(sdk, 'ndk', '28.0.0'), { recursive: true });
  writeFileSync(jar, 'controlled bootclasspath');
  writeFileSync(file, stable);
  const observed = observeNdkInput(root, ndk, profile, profileBytes);
  assert.deepEqual(observeNdkInput(root, ndk, profile, profileBytes), observed);
  assert.deepEqual(JSON.parse(readFileSync(join(root, 'sdk-input-observation.json'))), observed);
  assert.equal(readFileSync(join(root, 'ndk-source.properties'), 'utf8'), stable);
  assert.deepEqual(observed.sourceProperties, { bytes: Buffer.byteLength(stable), sha256: pin(stable) });
  assert.deepEqual(observed.archiveContract, { bytes: profileBytes.length, sha256: pin(profileBytes) });
  assert.equal(observed.revision, profile.ndkSourceRevision);
  assert.equal(observed.baseRevision, profile.ndkRevision);
  assert.equal(observed.devicePassed, false);
  assert.equal(observed.publicationEligible, false);

  const view = prepareSdkView(work, sdk, ndk, profile, bootclasspath);
  assert.equal(view[1], jar);
  assert.deepEqual(prepareSdkView(work, sdk, ndk, profile, bootclasspath), view);
  const selected = join(view[0], 'ndk');
  assert.deepEqual(readdirSync(selected), [profile.ndkRevision]);
  assert.equal(realpathSync(join(selected, profile.ndkRevision)), ndk);
  const preview = stable.replace(profile.ndkSourceRevision, '30.0.16138531-beta3');
  writeFileSync(file, preview);
  assert.throws(() => observeNdkInput(root, ndk, profile, profileBytes), /previous NDK source observation differs/);
  assert.equal(readFileSync(join(root, 'ndk-source.properties'), 'utf8'), stable);
  const refused = join(root, 'refused');
  mkdirSync(refused);
  assert.throws(() => observeNdkInput(refused, ndk, profile, profileBytes), /revision\/base\/release differs/);
  assert.equal(readFileSync(join(refused, 'ndk-source.properties'), 'utf8'), preview);
  const refusedReceipt = JSON.parse(readFileSync(join(refused, 'sdk-input-observation.json')));
  assert.equal(refusedReceipt.revision, '30.0.16138531-beta3');
  assert.equal(refusedReceipt.requiredRevision, profile.ndkSourceRevision);
  assert.equal(refusedReceipt.publicationEligible, false);

  for (const [raw, error] of [
    [preview, /revision\/base\/release differs/],
    [stable.replace(profile.ndkSourceRevision, profile.ndkSourceRevision + '-beta3'), /revision\/base\/release differs/],
    [stable.replace(`Pkg.BaseRevision = ${profile.ndkRevision}`, 'Pkg.BaseRevision = 30.0.0'), /revision\/base\/release differs/],
    [stable.replace('Pkg.ReleaseName = r30', 'Pkg.ReleaseName = r30-rc1'), /revision\/base\/release differs/],
    [stable + `Pkg.Revision=${profile.ndkSourceRevision}\n`, /duplicate\/empty key/],
    [stable.replace(`Pkg.Revision = ${profile.ndkSourceRevision}`, 'Pkg.Revision = 30.0.16138531 trailing'), /revision malformed\/missing/],
  ]) {
    writeFileSync(file, raw);
    assert.throws(() => prepareSdkView(work, sdk, ndk, profile, bootclasspath), error);
  }
  writeFileSync(file, stable);
  mkdirSync(join(selected, '28.0.0'));
  assert.throws(() => prepareSdkView(work, sdk, ndk, profile, bootclasspath), /only the pinned NDK/);
  writeFileSync(jar, 'wrong bootclasspath');
  assert.throws(() => prepareSdkView(work, sdk, ndk, profile, bootclasspath), /bootclasspath differs/);
});

test('Android build entry passes the verified launcher and preserves config overrides', {
  skip: process.platform !== 'linux',
}, () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-android-config-'));
  const script = fileURLToPath(new URL('./build-android-apk.sh', import.meta.url));
  const config = fileURLToPath(new URL('../src-tauri/tauri.android.conf.json', import.meta.url));
  const ndk = join(fixture.path, 'ndk');
  const revision = readFileSync(new URL('./android-rust-ndk.version', import.meta.url), 'utf8').trim();
  mkdirSync(join(ndk, 'build/cmake'), { recursive: true });
  writeFileSync(join(ndk, 'source.properties'), `Pkg.Revision = ${revision}\n`);
  writeFileSync(join(ndk, 'build/cmake/android.toolchain.cmake'), '');
  const version = readFileSync(new URL('./tauri-cli.version', import.meta.url), 'utf8').trim();
  writeFileSync(join(fixture.path, 'tauri'), `#!/usr/bin/env bash
if [[ "$1" == --version ]]; then
  printf '%s\n' "tauri-cli $TEST_TAURI_VERSION"
else
  printf '%s\n' "$POLARIS_TAURI_CLI" "$@"
fi
`, { mode: 0o700 });
  const env = { ...process.env, PATH: fixture.path + delimiter + process.env.PATH,
    ANDROID_NDK_HOME: ndk, NDK_HOME: ndk, ANDROID_NDK_ROOT: ndk, TEST_TAURI_VERSION: version };
  const args = ['--apk', '--split-per-abi', '--target', 'aarch64', '--ci'];
  const override = '--config=custom.json';
  for (const [input, expected] of [
    [args, ['--config', config, ...args]],
    [[...args, override], [...args, override]],
    [[...args, '-c', 'custom.json'], [...args, '-c', 'custom.json']],
    [[...args, '--', override], ['--config', config, ...args, '--', override]],
  ]) {
    const child = spawnSync('bash', [script, ...input], { env, cwd: fixture.path, encoding: 'utf8', timeout: 5000 });
    assert.ifError(child.error);
    assert.equal(child.status, 0, child.stderr);
    assert.deepEqual(child.stdout.trimEnd().split('\n'), [
      realpathSync(join(fixture.path, 'tauri')), 'android', 'build', ...expected,
    ]);
  }
});

/**
 * `verify-apk.mjs` 的判据本体测试。
 *
 * 这里喂的是**合成输入**，不是真 APK：真 APK 只能证明「这一次的产物是对的」，
 * 证明不了「判据在产物出错时会红」。两者都要 —— 真 APK 的收据在 CI 腿与设计文档里，
 * 判据有没有牙由本文件负责。
 *
 * 每条否定用例都配一条**反向对照**（第一条 `未变异的合成 APK 全绿`）：没有它，
 * 「判据恒红」与「判据抓到了这次变异」在测试结果上完全一样。
 */

const ELF = Buffer.from([0x7f, 0x45, 0x4c, 0x46]);

/**
 * 造一份**结构上真的能被解析**的 ELF：头 + 载荷 + 节名字符串表 + 节表。
 *
 * 判据 ⑥ 读的是节表；拿「4 字节魔数 + 一坨字节」当夹具的话，每条用例都会红在
 * 「节表读不出来」上 —— 那样测到的是解析器的报错路径，不是判据本身。
 *
 * 两种 ELFCLASS 都能造：本仓 armv7/i686 是 ELF32、arm64/x86_64 是 ELF64，
 * 解析器要在两侧都对，夹具就得能喂出两侧。
 *
 * @param {object} o
 * @param {Array<string|[string, number]>} o.sections 节名（或 `[节名, sh_size]`）；
 *        索引 0 的 NULL 节与末尾的 `.shstrtab` 由本函数自己补
 * @param {Buffer} o.payload 夹在头与节表之间的字节（用来塞 cronet/naive 针）
 * @param {32|64}  o.cls
 * @param {number} o.machine `e_machine`（缺省 EM_AARCH64；判据 ② 拿它与 ABI 对拍）
 */
function fakeElf({
  sections = [...REQUIRED_ELF_SECTIONS],
  payload = Buffer.alloc(0),
  cls = 64,
  machine = ABIS[DEFAULT_ABI].machine,
} = {}) {
  const is64 = cls === 64;
  const EHDR = is64 ? 64 : 52;
  const SHENT = is64 ? 64 : 40;
  const PHENT = is64 ? 56 : 32;
  const payloadOffset = EHDR + 2 * PHENT;

  const entries = [['', 0], ...sections.map((x) => (Array.isArray(x) ? x : [x, 16])), ['.shstrtab', 0]];
  const strParts = [];
  const nameOffs = [];
  let cur = 0;
  for (const [name] of entries) {
    nameOffs.push(cur);
    strParts.push(Buffer.from(`${name}\0`, 'latin1'));
    cur += Buffer.byteLength(name, 'latin1') + 1;
  }
  const strtab = Buffer.concat(strParts);

  const strtabOff = payloadOffset + payload.length;
  const shoff = strtabOff + strtab.length;
  const shnum = entries.length;
  const buf = Buffer.alloc(shoff + shnum * SHENT);

  ELF.copy(buf, 0);
  buf[4] = is64 ? 2 : 1; // EI_CLASS
  buf[5] = 1; // EI_DATA = 小端
  buf[6] = 1; // EI_VERSION
  buf.writeUInt16LE(3, 16); // ET_DYN
  buf.writeUInt16LE(machine, 18); // e_machine（e_ident 16 + e_type 2）
  payload.copy(buf, payloadOffset);
  if (is64) {
    buf.writeBigUInt64LE(BigInt(EHDR), 32);
  } else {
    buf.writeUInt32LE(EHDR, 28);
  }
  buf.writeUInt16LE(PHENT, is64 ? 54 : 42);
  buf.writeUInt16LE(2, is64 ? 56 : 44);
  for (let i = 0; i < 2; i++) {
    const row = EHDR + i * PHENT;
    buf.writeUInt32LE(1, row);
    if (is64) buf.writeBigUInt64LE(16384n, row + 48);
    else buf.writeUInt32LE(16384, row + 28);
  }
  strtab.copy(buf, strtabOff);

  if (is64) {
    buf.writeBigUInt64LE(BigInt(shoff), 40);
    buf.writeUInt16LE(SHENT, 58);
    buf.writeUInt16LE(shnum, 60);
    buf.writeUInt16LE(shnum - 1, 62); // `.shstrtab` 是最后一条
  } else {
    buf.writeUInt32LE(shoff, 32);
    buf.writeUInt16LE(SHENT, 46);
    buf.writeUInt16LE(shnum, 48);
    buf.writeUInt16LE(shnum - 1, 50);
  }

  entries.forEach(([name, size], i) => {
    const base = shoff + i * SHENT;
    buf.writeUInt32LE(nameOffs[i], base);
    buf.writeUInt32LE(i === 0 ? 0 : 1, base + 4); // sh_type：非 NULL 一律 PROGBITS
    if (name === '.shstrtab') {
      if (is64) {
        buf.writeBigUInt64LE(BigInt(strtabOff), base + 24);
        buf.writeBigUInt64LE(BigInt(strtab.length), base + 32);
      } else {
        buf.writeUInt32LE(strtabOff, base + 16);
        buf.writeUInt32LE(strtab.length, base + 20);
      }
    } else if (is64) {
      buf.writeBigUInt64LE(BigInt(size), base + 32);
    } else {
      buf.writeUInt32LE(size, base + 20);
    }
  });
  return buf;
}

/**
 * 造一个含 `cronet` × cronet 次、`naive` × naive 次的假 `.so`。
 * 默认节表是「已剥过」的形态（无任何 `.debug_*`），与真库一致。
 */
function fakeSo({ cronet = 500, naive = 100, elf = true, sections, cls, machine } = {}) {
  const so = fakeElf({
    cls,
    machine,
    sections: sections ?? [...REQUIRED_ELF_SECTIONS, '.rodata', '.bss'],
    payload: Buffer.concat([
      Buffer.from('cronet'.repeat(cronet)),
      Buffer.from('naive'.repeat(naive)),
    ]),
  });
  if (!elf) Buffer.from('NOPE').copy(so, 0);
  return so;
}

/** 应用自己那份 `.so`（不带 cronet/naive 针）。 */
function fakeAppSo({ sections, cls, machine } = {}) {
  return fakeElf({
    cls,
    machine,
    sections: sections ?? [...REQUIRED_ELF_SECTIONS, '.rodata'],
    payload: Buffer.from('app'),
  });
}

const SRS_COUNT = 28;

/** 「什么都对」的合成 APK。用 `patch` 做定点变异；`abi` 决定原生库铺在哪、造成什么架构。 */
function goodApk(patch = {}, abi = DEFAULT_ABI) {
  const licenses = new Map(
    LICENSE_FILES.map((name) => [name, Buffer.from(`${name} 的真实内容\n`)]),
  );
  const members = new Map();
  for (const [name, bytes] of licenses) members.set(`${ASSET_UP}${name}`, bytes);
  const { elfClass, machine } = ABIS[abi];
  const cls = elfClass === 2 ? 64 : 32;
  members.set(`lib/${abi}/libbox.so`, fakeSo({ cls, machine }));
  members.set(`lib/${abi}/libpolaris_lib.so`, fakeAppSo({ cls, machine }));
  members.set(`lib/${abi}/libc++_shared.so`, fakeAppSo({ cls, machine }));
  for (let i = 0; i < SRS_COUNT; i += 1) {
    members.set(`${ASSET_UP}${SRS_DIR}/geosite-${i}.srs`, Buffer.from(`SRS\x03geo-${i}`));
  }
  // 填充到真实 APK 的量级（classes.dex / res / META-INF …），让扫描面自检那条不误红。
  for (let i = 0; i < 200; i += 1) members.set(`res/filler/${i}.xml`, Buffer.from('x'));

  // 死字节判据的两个新输入。合成 APK 里成员按「压缩后 = 原字节」算（真实 APK 的
  // `.so` / `resources.arsc` 也确实是 `stor` 不压缩），文件大小 = 条目和 + 一份**真实形状**
  // 的结构开销（本地头 30 + 中央头 46 + 文件名两遍 + EOCD 22）。
  // 刻意**不**用 zipOverheadBound() 来造 fileSize：那会让夹具与判据共用同一个算式，
  // 算式写错时两边一起错、测试照绿（判据被自己污染）。这里手写一遍最小真实开销。
  //
  // 这两个值在 `patch` **之前**算定，之后不再随 `_members` 的增删重算 —— 它们是两个独立旋钮，
  // 由死字节那组用例直接赋值。成员级变异（删一份 .srs / 一份许可文本）因此会让这两个数
  // 偏大几十字节，落在每条目 4096 字节的对齐余量里，够不到带的上沿，不会误红。
  const entriesCompressedTotal = [...members.values()].reduce((a, b) => a + b.length, 0);
  const realisticOverhead =
    members.size * (ZIP.LOCAL_HEADER + ZIP.CENTRAL_HEADER) +
    2 * [...members.keys()].reduce((a, n) => a + Buffer.byteLength(n), 0) +
    ZIP.EOCD;

  const input = {
    names: [...members.keys()],
    fileSize: entriesCompressedTotal + realisticOverhead,
    entriesCompressedTotal,
    zipEntries: new Map([...members].map(([name, bytes]) => [name,
      { method: 8, compressedSize: bytes.length, uncompressedSize: bytes.length, dataOffset: 123 }])),
    readMember: (name) => {
      const hit = members.get(name);
      if (hit === undefined) throw new Error(`测试夹具里没有成员 ${name}`);
      return hit;
    },
    repoLicense: (name) => licenses.get(name) ?? null,
    repoSrs: new Map([...members].filter(([name]) => name.endsWith('.srs')).map(([name, bytes]) => [name.slice(`${ASSET_UP}${SRS_DIR}/`.length), Buffer.from(bytes)])),
    abi,
    // 判据 ⑦ 的取材面：默认造一份与登记表恰等的权限集（`patch` 可以就地改它）。
    permissions: {
      uses: APK_PERMISSION_REGISTRY.map((e) => e.name),
      declared: APK_DECLARED_PERMISSIONS.map((e) => e.name),
      // 独立建模编译 manifest 的 signature 值，不从被测登记表复制保护级别。
      declaredProtectionLevels: { 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION': 2 },
    },
    // 变异钩子（不是 apkViolations 的参数，仅供本文件内改夹具用）
    _members: members,
    _licenses: licenses,
  };
  patch(input);
  input.names = [...members.keys()];
  return input;
}

function run(patch = () => {}, { abi = DEFAULT_ABI, judgeAs = abi } = {}) {
  const { names, readMember, repoLicense, repoSrs, fileSize, entriesCompressedTotal, permissions, zipEntries } =
    goodApk(patch, abi);
  // `judgeAs` 与 `abi` 分开：造包用一个 ABI、判它用另一个，正是「参数没真的进判据」这条变异要的形状。
  return apkViolations({
    names,
    readMember,
    repoLicense,
    repoSrs,
    fileSize,
    entriesCompressedTotal,
    abi: judgeAs,
    permissions,
    zipEntries,
  });
}

/** 违反项里恰有 n 条提到 `needle`。 */
function hits(violations, needle) {
  return violations.filter((v) => v.includes(needle)).length;
}

function oneEntryZip(name, method = 8) {
  return fixtureZip([{ name, data: 'native-library-payload'.repeat(8), method }]);
}

test('formal APK SDK payload matches selected AAR bytes and preserves the carrier receipt fields', () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-apk-sdk-'));
  const apk = join(fixture.path, 'app.apk');
  const selectedABIs = ['arm64-v8a', 'armeabi-v7a'];
  const libraries = ['libbox.so', 'libpolaris_lib.so', 'libc++_shared.so', 'libsupport.so'];
  const entries = selectedABIs.flatMap((abi) => libraries.map((library) => {
    const name = `lib/${abi}/${library}`;
    return { name, data: name, method: 8 };
  }));
  const embeddedRuntimeSha256 = Object.fromEntries(entries
    .filter(({ name }) => /\/(libbox|libsupport)\.so$/.test(name))
    .map(({ name, data }) => [name, createHash('sha256').update(data).digest('hex')]));
  const embeddedLibboxSha256 = Object.fromEntries(selectedABIs.map((abi) => [abi, embeddedRuntimeSha256[`lib/${abi}/libbox.so`]]));
  const pins = Object.fromEntries(Object.entries(embeddedRuntimeSha256).map(([name, sha256]) => [
    name.replace(/^lib\//, 'jni/'), { bytes: Buffer.byteLength(name), sha256 },
  ]));
  pins['jni/x86/libbox.so'] = { bytes: 12, sha256: '0'.repeat(64) };
  writeFileSync(apk, fixtureZip(entries));
  const script = fileURLToPath(new URL('./verify-apk.mjs', import.meta.url));
  const child = spawnSync(process.execPath, [script, '--sdk-payload', apk, 'universal', JSON.stringify(pins)], { encoding: 'utf8' });
  assert.ifError(child.error);
  assert.equal(child.status, 0, child.stderr);
  assert.deepEqual(JSON.parse(child.stdout), { selectedABIs, embeddedLibboxSha256, embeddedRuntimeSha256 });

  const verify = (rows, flavor = 'universal') => {
    writeFileSync(apk, fixtureZip(rows));
    return verifySdkPayload(apk, flavor, pins);
  };
  assert.throws(() => verify(entries, 'arm64-v8a'), /actual ABI set differs/);
  assert.throws(() => verify(entries.filter(({ name }) => !name.endsWith('/libsupport.so'))), /missing.*libsupport/);
  const changed = entries.map((entry) => entry.name === 'lib/armeabi-v7a/libsupport.so'
    ? { ...entry, data: 'x'.repeat(entry.data.length) } : entry);
  assert.throws(() => verify(changed), /native bytes differ.*libsupport/);
  assert.throws(() => verify([...entries, entries[0]]), /duplicate entry/);
  assert.throws(() => verify([...entries, { name: '../outside', data: 'fixture' }]), /unsafe APK ZIP member/);

  const badCrc = fixtureZip(entries);
  const central = badCrc.readUInt32LE(badCrc.length - 6);
  badCrc.writeUInt32LE(0, central + 16);
  writeFileSync(apk, badCrc);
  assert.throws(() => verifySdkPayload(apk, 'universal', pins), /size or CRC differs/);
});

test('mobile carrier reads the App and extension from one original archive across an awaited plist read', async () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-mobile-zip-'));
  const path = join(fixture.path, 'app.ipa'), replacement = join(fixture.path, 'replacement.ipa');
  const app = 'Payload/Polaris.app/Polaris', extension = 'Payload/Polaris.app/PlugIns/Tunnel.appex/Tunnel';
  writeFileSync(path, fixtureZip([{ name: app, data: 'original host App' }, { name: extension, data: 'original extension' }]));
  writeFileSync(replacement, fixtureZip([{ name: app, data: 'replacement host App' }, { name: extension, data: 'replacement extension' }]));
  const archive = openComponentZip(path);
  try {
    assert.equal(archive.member(app).toString(), 'original host App');
    await Promise.resolve().then(() => renameSync(replacement, path));
    assert.equal(archive.member(extension).toString(), 'original extension');
    assert.equal(withComponentZip(path, current => current.member(extension).toString()), 'replacement extension');
  } finally {
    archive.close();
  }
});

test('native ZIP metadata accepts DEFLATE at an unaligned data offset and rejects STORE', () => {
  const name = `lib/${ABI}/libpolaris_lib.so`;
  const compressed = parseZipEntries(oneEntryZip(name)).get(name);
  assert.equal(compressed.method, 8);
  assert.notEqual(compressed.dataOffset % 16384, 0);
  assert.ok(compressed.compressedSize < compressed.uncompressedSize);
  assert.equal(parseZipEntries(oneEntryZip(name, 0)).get(name).method, 0);
  for (const lib of ['libpolaris_lib.so', 'libbox.so', 'libc++_shared.so']) {
    const violations = run((input) => { input.zipEntries.get(`lib/${ABI}/${lib}`).method = 0; });
    assert.equal(violations.length, 1, violations.join(' | '));
    assert.match(violations[0], /必须用 DEFLATE/);
  }
});

test('native compression fails closed on missing metadata and differing ZIP headers', () => {
  const violations = run((input) => { input.zipEntries = null; });
  assert.equal(violations.length, 1);
  assert.match(violations[0], /ZIP 压缩方式读不出来/);
  const zip = oneEntryZip(`lib/${ABI}/libbox.so`);
  zip.writeUInt16LE(0, 8);
  assert.throws(() => parseZipEntries(zip), /local and central headers differ/);
  assert.throws(() => parseZipEntries(zip.subarray(0, zip.length - 3)), /EOCD missing/);
});

test('non-runtime .symtab/.strtab are rejected while dynamic export sections stay required', () => {
  const violations = run((input) => input._members.set(`lib/${ABI}/libpolaris_lib.so`,
    fakeAppSo({ sections: [...REQUIRED_ELF_SECTIONS, '.symtab', '.strtab'] })));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /非运行静态符号表 \.symtab \/ \.strtab/);
  for (const missing of ['.dynamic', '.dynsym', '.dynstr']) {
    const errors = run((input) => input._members.set(`lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: [...REQUIRED_ELF_SECTIONS.filter((name) => name !== missing), '.rodata'] })));
    assert.equal(errors.length, 1, errors.join(' | '));
    assert.ok(errors[0].includes(missing));
  }
});

test('libc++ runtime cannot disappear from the package', () => {
  const violations = run((input) => input._members.delete(`lib/${ABI}/libc++_shared.so`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libc\+\+_shared\.so 不在 APK/);
});

test('反向对照：未变异的合成 APK 全绿', () => {
  assert.deepEqual(run(), []);
});

test('compiled permission evidence remains mandatory for every ABI', () => {
  for (const abi of Object.keys(ABIS)) {
    for (const evidence of [undefined, null, 'signature']) {
      const violations = run((input) => {
        input.permissions.declaredProtectionLevels = evidence;
      }, { abi });
      assert.equal(violations.length, 1, `${abi}: ${violations.join(' | ')}`);
      assert.match(violations[0], /Missing\/unparseable compiled permission protectionLevel evidence/);
    }
  }
});

test('compiled permission must remain exactly signature, independent of ABI and permission names', () => {
  for (const abi of Object.keys(ABIS)) {
    for (const level of [0, 1, 3, 0x12, '2', undefined]) {
      const violations = run((input) => {
        input.permissions.declaredProtectionLevels[
          'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'
        ] = level;
      }, { abi });
      assert.equal(violations.length, 1, `${abi}: ${violations.join(' | ')}`);
      assert.match(violations[0], /must have exact signature protectionLevel=2/);
    }
  }
});

test('M1 形态：包里缺 NOTICE ⇒ 红，且恰红在 NOTICE 那一条', () => {
  const violations = run((input) => input._members.delete(`${ASSET_UP}NOTICE`));
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.equal(hits(violations, 'NOTICE'), 1);
  assert.equal(hits(violations, 'LICENSE'), 0);
  assert.equal(hits(violations, 'THIRD-PARTY'), 0);
});

test('M2 形态：三份许可文本全不在包里（`_up_` 整条逃逸）⇒ 三条都红', () => {
  const violations = run((input) => {
    for (const name of LICENSE_FILES) input._members.delete(`${ASSET_UP}${name}`);
  });
  assert.equal(violations.length, 3, violations.join(' | '));
  for (const name of LICENSE_FILES) assert.equal(hits(violations, `${ASSET_UP}${name} 不在`), 1);
});

test('许可文本被截断（在包里但与工作树不一致）⇒ 红', () => {
  const violations = run((input) =>
    input._members.set(`${ASSET_UP}LICENSE`, Buffer.from('MIT')),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /与工作树 LICENSE 不一致/);
});

test('工作树里的真值缺失或为空 ⇒ 红在真值那一侧，不冒充「APK 有问题」', () => {
  const missing = run((input) => input._licenses.delete('NOTICE'));
  assert.equal(missing.length, 1);
  assert.match(missing[0], /工作树里没有 NOTICE/);

  const empty = run((input) => input._licenses.set('NOTICE', Buffer.alloc(0)));
  assert.equal(empty.length, 1);
  assert.match(empty[0], /0 字节/);
});

test('M3 形态：libbox.so 里没有 naive ⇒ 红（判据本体有牙）', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ cronet: 500, naive: 0 })),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /'naive' 只出现 0 次/);
  assert.match(violations[0], /naive\/H3|naive outbound/);
});

test('cronet 被摘掉（with_naive_outbound 掉了）⇒ 两条都红', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ cronet: 0, naive: 0 })),
  );
  assert.equal(violations.length, 2, violations.join(' | '));
  assert.equal(hits(violations, "'cronet'"), 1);
  assert.equal(hits(violations, "'naive'"), 1);
});

test('阈值是严格大于：恰等于下限判红，下限 +1 判绿', () => {
  for (const { needle, min } of NATIVE_MARKERS) {
    const atLimit = run((input) =>
      input._members.set(
        `lib/${ABI}/libbox.so`,
        fakeSo(needle === 'cronet' ? { cronet: min, naive: 999 } : { cronet: 999, naive: min }),
      ),
    );
    assert.equal(hits(atLimit, `'${needle}' 只出现 ${min} 次`), 1, `${needle} 恰等于下限应判红`);

    const overLimit = run((input) =>
      input._members.set(
        `lib/${ABI}/libbox.so`,
        fakeSo(needle === 'cronet' ? { cronet: min + 1, naive: 999 } : { cronet: 999, naive: min + 1 }),
      ),
    );
    assert.deepEqual(overLimit, [], `${needle} 超出下限 1 应判绿`);
  }
});

test('libbox.so 整个缺席 ⇒ 红，且构建 tag 判据不在缺席的输入上假绿', () => {
  const violations = run((input) => input._members.delete(`lib/${ABI}/libbox.so`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libbox\.so 不在 APK 里/);
});

test('应用自己的 libpolaris_lib.so 缺席 ⇒ 红（libbox 在场不构成放行）', () => {
  const violations = run((input) => input._members.delete(`lib/${ABI}/libpolaris_lib.so`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libpolaris_lib\.so 不在 APK 里/);
});

test('读到的 .so 不是 ELF ⇒ 红（不拿一坨字节冒充共享库），且判据 ② 与 ⑥ 都说话', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ elf: false })),
  );
  // ② 魔数那条
  assert.equal(hits(violations, '不是 ELF（前四字节'), 1, violations.join(' | '));
  // ⑥ 节表那条：**这一条才是关键** —— 剥符号判据绝不能在「读到的根本不是 ELF」的输入上
  // 因为「没扫到 .debug_*」而判绿。两条各红一次，恰好 2 条。
  assert.equal(hits(violations, '节表读不出来'), 1, violations.join(' | '));
  assert.equal(violations.length, 2, violations.join(' | '));
});

test('.srs 份数与工作树不等 ⇒ 红', () => {
  const violations = run((input) => input._members.delete(`${ASSET_UP}${SRS_DIR}/geosite-0.srs`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], new RegExp(`有 ${SRS_COUNT - 1} 份 \\.srs`));
});

test('工作树 .srs 份数为零 ⇒ 红（`0 === 0` 不许判绿）', () => {
  const g = goodApk((input) => {
    for (const name of [...input._members.keys()]) {
      if (name.includes(`${SRS_DIR}/`)) input._members.delete(name);
    }
  });
  const { names, readMember, repoLicense } = g;
  const violations = apkViolations({
    names,
    readMember,
    repoLicense,
    permissions: g.permissions,
    zipEntries: g.zipEntries,
    repoSrs: new Map(),
    fileSize: g.fileSize,
    entriesCompressedTotal: g.entriesCompressedTotal,
    abi: DEFAULT_ABI,
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /真值为零/);
});

test('成员清单塌了（空 / 过短）⇒ 红在扫描面，不报成「许可文本不在包里」', () => {
  const violations = apkViolations({
    names: [],
    readMember: () => Buffer.alloc(0),
    repoLicense: () => Buffer.from('x'),
    repoSrs: new Map(),
    fileSize: 1,
    entriesCompressedTotal: 1,
    abi: DEFAULT_ABI,
  });
  assert.equal(violations.length, 1);
  assert.match(violations[0], /成员清单只有 0 条/);
});

test('countOccurrences 与 Python bytes.count 同语义（重叠不计）', () => {
  assert.equal(countOccurrences(Buffer.from('aaaa'), 'aa'), 2);
  assert.equal(countOccurrences(Buffer.from('abcabc'), 'abc'), 2);
  assert.equal(countOccurrences(Buffer.from('abc'), 'x'), 0);
});

// ───────────────────────── 死字节判据（判据 ⑤）─────────────────────────
//
// 反向对照沿用文件开头那条「未变异的合成 APK 全绿」：它现在也覆盖死字节这一条，
// 没有它，下面每一条「变异后判红」都与「判据恒红」不可分。

test('M4 形态：文件里多出一大段没有条目指向的字节 ⇒ 红，且恰红在死字节那一条', () => {
  // 越界幅度取真实脏包的量级：实测非条目字节 388058998 是上界 4295570 的 90 倍。
  // **不**用「比值 ×2.77」来驱动：合成夹具的条目总量只有几 KB，而上界含每条目一页
  // 对齐余量（231 条目 ≈ 0.9 MB），比值的带在这个尺度上宽得没有分辨力 —— 那是判据
  // 在小包上的真实性质（见 verify-apk.mjs 里 zipOverheadBound 的「射程与灵敏度下限」段），
  // 不是夹具的毛病。真实尺度上的红/绿对照由本文件最后一条用实测数字覆盖。
  const violations = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names) * 90;
  });
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /没有任何条目指向/);
  assert.match(violations[0], /超出带 \[1\.000000, /);
  // 报文必须点名：读的人要一眼看出差多少、往哪个方向修。
  assert.match(violations[0], /删掉这个 APK 再打一次/);
});

test('死字节变异**不**牵连其它四条判据（变异是定点的，不是把夹具打坏了）', () => {
  const violations = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names) * 90;
  });
  assert.equal(hits(violations, '不在 APK 里'), 0, violations.join(' | '));
  assert.equal(hits(violations, '只出现'), 0, violations.join(' | '));
  assert.equal(hits(violations, '.srs'), 0, violations.join(' | '));
});

test('带的上沿是严格大于：恰落在上沿判绿，上沿 +1 字节判红', () => {
  // hi = 1 + 上界/条目和  ⇔  fileSize = 条目和 + 上界。判据是 `ratio > hi` 才红。
  const atEdge = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names);
  });
  assert.deepEqual(atEdge, [], `恰在上沿应判绿，实为：${atEdge.join(' | ')}`);

  const overEdge = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names) + 1;
  });
  assert.equal(overEdge.length, 1, overEdge.join(' | '));
  assert.match(overEdge[0], /没有任何条目指向/);
});

test('带的下沿：文件比自己条目的压缩和还小 ⇒ 红在取材面，不冒充「包里有死字节」', () => {
  // 纯上限判据在这种输入上是**恒绿**的——下沿存在的全部理由就是这个。
  const violations = run((input) => {
    input.fileSize = input.entriesCompressedTotal - 1;
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /物理上不可能/);
  assert.match(violations[0], /取材面错了，不是包有问题/);
  assert.equal(hits(violations, '没有任何条目指向'), 0);
});

test('条目压缩和读不出来（null / 0）⇒ 红在取材面，不许空跑绿', () => {
  for (const bad of [null, 0, undefined, NaN]) {
    const violations = run((input) => {
      input.entriesCompressedTotal = bad;
    });
    assert.equal(violations.length, 1, `${bad} 应恰 1 条违反，实为：${violations.join(' | ')}`);
    assert.match(violations[0], /没能从 zip 读出条目尺寸/);
    assert.match(violations[0], /恒真，必须红在取材面/);
  }
});

test('文件大小读不出来（0 / NaN）⇒ 红在取材面', () => {
  for (const bad of [0, NaN, -1]) {
    const violations = run((input) => {
      input.fileSize = bad;
    });
    assert.equal(violations.length, 1, `${bad} 应恰 1 条违反，实为：${violations.join(' | ')}`);
    assert.match(violations[0], /取材面塌了/);
  }
});

test('zipOverheadBound 与逐项手算逐字对拍（算式本身没有被自己的实现证明）', () => {
  const names = ['a.txt', 'lib/arm64-v8a/libbox.so', '中文名.srs'];
  const nameBytes = names.reduce((a, n) => a + Buffer.byteLength(n, 'utf8'), 0);
  // 手写一遍：本地头 30 + 中央头 46 + data descriptor 24 + 本地 extra 4096，每条目一份；
  // 文件名存两遍；整包一个 EOCD 22 + zip64 的 56 + 20。
  const byHand = names.length * (30 + 46 + 24 + 4096) + 2 * nameBytes + 22 + 56 + 20;
  assert.equal(zipOverheadBound(names), byHand);
  // 名字按**字节**算，不是按字符：'中文名.srs' 是 7 个字符、13 个字节。
  assert.equal(Buffer.byteLength('中文名.srs', 'utf8'), 13); // 3 个 CJK ×3 + '.srs' 4
  assert.ok(nameBytes > names.join('').length, '含非 ASCII 名字时字节数必须大于字符数');
});

test('真实尺度的红/绿对照：本机实测的两份产物喂进判据，判定与实测一致', () => {
  // 干净包与脏包**只差 fileSize** —— 同一次剥符号改动、同一份条目内容（条目压缩和逐字节相同），
  // 一个删了旧 APK 重打，一个没删。这是这条判据要抓的形态在真实尺度上的样子；
  // 上面那些合成夹具的用例证明它有牙，这一条证明带宽在真实数字上也是对的（不会被悄悄放松）。
  const base = goodApk(() => {}); // goodApk 的默认形参是 {} 不是函数，须显式给一个空 patch
  const names = [
    ...base.names,
    ...Array.from({ length: 1001 - base.names.length }, (_, i) => `res/pad/${i}.xml`),
  ];
  const common = {
    names,
    readMember: base.readMember,
    repoLicense: base.repoLicense,
    repoSrs: base.repoSrs,
    abi: DEFAULT_ABI,
    permissions: base.permissions,
    zipEntries: base.zipEntries,
  };
  const entriesCompressedTotal = 218812042; // 两份包相同

  const clean = apkViolations({ ...common, fileSize: 219011984, entriesCompressedTotal });
  assert.deepEqual(clean, [], `干净包（删旧包重打）应全绿，实为：${clean.join(' | ')}`);

  const dirty = apkViolations({ ...common, fileSize: 606871040, entriesCompressedTotal });
  assert.equal(dirty.length, 1, `脏包应恰 1 条违反，实为：${dirty.join(' | ')}`);
  assert.match(dirty[0], /死字节占整包 63\.\d%/);
  assert.match(dirty[0], /比值 2\.7734\d+ 超出带/);
});

// ───────────────────── 原生库剥符号判据（判据 ⑥）─────────────────────
//
// 这条判据存在的理由：把 `.cargo/config.toml` 的四段 android rustflags 删掉之后，
// 判据 ①②③④⑤ **全绿**（死字节那条也不红——文件大小与条目压缩和是一起变大的），
// 而 APK 从 219011984 弹回 606871040。反向对照仍是文件开头那条「未变异的合成 APK 全绿」。

test('M5 形态：libpolaris_lib.so 留着 DWARF ⇒ 红，点名节区、字节数与改动点', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({
        sections: [
          ...REQUIRED_ELF_SECTIONS,
          ['.debug_info', 165020847],
          ['.debug_str', 159873331],
        ],
      }),
    ),
  );
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /libpolaris_lib\.so 里还留着 2 个调试信息节/);
  assert.match(violations[0], /\.debug_info \.debug_str/);
  assert.match(violations[0], /324894178 字节/); // 两个节的 sh_size 之和，正面报出实测量
  // 报文要把人送到改动点，而不是让他自己去找
  assert.match(violations[0], /\.cargo\/config\.toml/);
  assert.match(violations[0], /strip=debuginfo/);
});

test('M5 变异是定点的：不牵连许可文本 / 符号计数 / .srs / 死字节四条', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: [...REQUIRED_ELF_SECTIONS, '.debug_info'] }),
    ),
  );
  assert.equal(hits(violations, '不在 APK 里'), 0, violations.join(' | '));
  assert.equal(hits(violations, '只出现'), 0, violations.join(' | '));
  assert.equal(hits(violations, '.srs'), 0, violations.join(' | '));
  assert.equal(hits(violations, '没有任何条目指向'), 0, violations.join(' | '));
});

test('libbox.so 也在射程内，且报文指向 build-mobile-core.sh 而不是 rustflags', () => {
  // 实测：三份真库（libbox 30 节 / libc++_shared 31 节 / libpolaris_lib 28 节）当下
  // 都是 0 个 debug 节，故把它们全纳入不会造出假红。而 libbox 由 Go/gomobile 构建、
  // 不经 rustc —— 出处不同，报文就必须指不同的地方，否则红了也修不到点上。
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ sections: [...REQUIRED_ELF_SECTIONS, '.debug_line'] })),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libbox\.so 里还留着 1 个调试信息节/);
  assert.match(violations[0], /build-mobile-core\.sh/);
  assert.doesNotMatch(violations[0], /cargo\/config\.toml/);
});

test('压缩 DWARF 的旧拼写 `.zdebug_*` 同样算 ⇒ 红（读回形态≠写入形态）', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: [...REQUIRED_ELF_SECTIONS, ['.zdebug_info', 4096]] }),
    ),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /\.zdebug_info/);
  assert.ok(isDebugSection('.zdebug_info') && isDebugSection('.debug_info'));
  assert.ok(!isDebugSection('.dynsym') && !isDebugSection('.text'));
});

test('取材面：节表读不出来 ⇒ 红在取材面，绝不因「没扫到 .debug_*」判绿', () => {
  // e_shoff = 0：一份没有节表的 ELF。纯否定式判据在它上面是**恒绿**的。
  const violations = run((input) => {
    const so = fakeAppSo();
    so.writeBigUInt64LE(0n, 40);
    input._members.set(`lib/${ABI}/libpolaris_lib.so`, so);
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /节表读不出来/);
  assert.match(violations[0], /e_shoff = 0/);
});

test('取材面：节表越过文件末尾（读到的字节不完整）⇒ 红在取材面', () => {
  const violations = run((input) => {
    const full = fakeAppSo();
    input._members.set(`lib/${ABI}/libpolaris_lib.so`, full.subarray(0, full.length - 200));
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /节表读不出来/);
  assert.match(violations[0], /超出文件长度/);
});

test('正面那一半有牙：节表解析得出但缺 .text/.dynsym ⇒ 红（不靠「什么都没解析到」判绿）', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: ['.rodata', '.bss', '.data', '.comment', '.note'] }),
    ),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /没有 \.text \/ \.dynamic \/ \.dynsym \/ \.dynstr/);
  assert.match(violations[0], /不构成任何证据/);
});

test('取材面：lib/<ABI>/ 下一个 .so 都没扫到 ⇒ 红（空集上恒真不许判绿）', () => {
  const violations = run((input) => {
    for (const name of [...input._members.keys()]) {
      if (name.startsWith(`lib/${ABI}/`)) input._members.delete(name);
    }
  });
  assert.equal(hits(violations, '一个 .so 都没扫到'), 1, violations.join(' | '));
  assert.match(violations.find((v) => v.includes('一个 .so 都没扫到')), /恒真，不许判绿/);
});

test('ELF32 也要解析对（armv7 / i686 两个 ABI 是 32 位）', () => {
  const stripped = fakeElf({ cls: 32, sections: [...REQUIRED_ELF_SECTIONS, '.rodata'] });
  const ok = strippedReport(stripped);
  assert.equal(ok.why, null, `ELF32 应解析成功，实为：${ok.why}`);
  assert.equal(ok.ok, true);
  assert.equal(ok.sections, 7); // NULL + four runtime sections + .rodata + .shstrtab

  const dirty = fakeElf({ cls: 32, sections: [...REQUIRED_ELF_SECTIONS, ['.debug_info', 999]] });
  const bad = strippedReport(dirty);
  assert.equal(bad.ok, false);
  assert.deepEqual(bad.debug, ['.debug_info']);
  assert.equal(bad.debugBytes, 999);
  // 反向对照：把同一份 ELF32 当 ELF64 读会读到垃圾——这正是「写死一种 class」的后果。
  const mislabeled = Buffer.from(dirty);
  mislabeled[4] = 2;
  assert.notEqual(strippedReport(mislabeled).why, null, '按错的 class 解析必须失败，而不是静默给个干净结论');
});

test('elfSectionNames 报出的是**实际读到的**节名与字节数（正面断言的取值口）', () => {
  const buf = fakeElf({ sections: [['.text', 100], ['.dynsym', 200], ['.debug_info', 300], '.dynamic', '.dynstr'] });
  const parsed = elfSectionNames(buf);
  assert.equal(parsed.ok, true);
  assert.deepEqual(parsed.names, ['', '.text', '.dynsym', '.debug_info', '.dynamic', '.dynstr', '.shstrtab']);
  assert.equal(parsed.bytes[1], 100);
  assert.equal(parsed.bytes[2], 200);
  assert.equal(parsed.bytes[3], 300);
  assert.equal(strippedReport(buf).debugBytes, 300);
});

// ───────────────────── ABI 参数化（2026-09-05）─────────────────────
//
// 起因：`ABI` 曾是写死的 `arm64-v8a`，而收尾序列要出 arm64 与 x86_64 两个 debug 包，
// **x86_64 恰恰是模拟器上跑的那个**——它此前在本仓一条产物级判据都没有。
// 下面这组用例分两半：一半钉参数取值口（缺省不变 / 未知取值必须抛），
// 一半钉「参数真的进了判据」（换 ABI 造的包按另一个 ABI 判必须红，不许静默判绿）。

test('parseArgs：不带 --abi 时缺省仍是 arm64-v8a（既有调用逐字不变）', () => {
  assert.deepEqual(parseArgs(['out/app-arm64-debug.apk']), {
    apk: 'out/app-arm64-debug.apk',
    abi: 'arm64-v8a',
  });
  assert.equal(DEFAULT_ABI, 'arm64-v8a');
});

test('parseArgs：--abi 取得到，且与路径的先后顺序无关', () => {
  assert.deepEqual(parseArgs(['a.apk', '--abi', 'x86_64']), { apk: 'a.apk', abi: 'x86_64' });
  assert.deepEqual(parseArgs(['--abi', 'x86_64', 'a.apk']), { apk: 'a.apk', abi: 'x86_64' });
});

test('parseArgs：未知 ABI ⇒ 抛，且报文列出全部支持的取值（不静默按缺省走）', () => {
  // 静默回落的后果不是「少验一点」：`--abi x86-64`（该写下划线）会让判据去量 arm64 那半个包，
  // 而它在 x86_64 产物里根本不存在 —— 要么红在「取材面塌了」（离真因两跳远），要么恰好判绿。
  assert.throws(() => parseArgs(['a.apk', '--abi', 'x86-64']), (e) => {
    assert.match(e.message, /未知 ABI："x86-64"/);
    for (const known of Object.keys(ABIS)) assert.ok(e.message.includes(known), `报文里应列出 ${known}`);
    return true;
  });
  assert.throws(() => parseArgs(['a.apk', '--abi']), /--abi 后面要跟一个 ABI 名/);
  // 反向对照：四个已知取值一个都不许被这条拦下来。
  for (const known of Object.keys(ABIS)) {
    assert.equal(parseArgs(['a.apk', '--abi', known]).abi, known);
  }
});

test('反向对照：四个 ABI 各造一个包、各按自己判 ⇒ 全绿', () => {
  for (const abi of Object.keys(ABIS)) {
    assert.deepEqual(run(() => {}, { abi }), [], `${abi} 的合成包应全绿`);
  }
});

test('ABI 真的进了判据：x86_64 的包按缺省 arm64 判 ⇒ 红（不许静默判绿）', () => {
  // 这条是「参数化只改了签名、判据还挂在旧常量上」的变异。若 abi 是装饰品，
  // 下面这次判定会去看 lib/arm64-v8a/（包里没有）却依然判绿。
  const violations = run(() => {}, { abi: 'x86_64', judgeAs: 'arm64-v8a' });
  assert.equal(hits(violations, 'lib/arm64-v8a/libbox.so 不在 APK 里'), 1, violations.join(' | '));
  assert.equal(hits(violations, 'lib/arm64-v8a/libpolaris_lib.so 不在 APK 里'), 1, violations.join(' | '));
  assert.equal(hits(violations, 'lib/arm64-v8a/libc++_shared.so 不在 APK 里'), 1, violations.join(' | '));
  assert.equal(hits(violations, '一个 .so 都没扫到'), 1, violations.join(' | '));
  assert.equal(violations.length, 4, violations.join(' | '));
});

test('ELF 机器类型对不上 ABI ⇒ 红（路径前缀不校验内容，只有这条看得见）', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ machine: ABIS.x86_64.machine })),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /e_machine=0x3e/);
  assert.match(violations[0], /应为 .*e_machine=0xb7（EM_AARCH64）/);
});

test('ELF 位宽对不上 ABI ⇒ 红（armeabi-v7a 是 ELF32，塞进一份 ELF64）', () => {
  const abi = 'armeabi-v7a';
  const violations = run(
    (input) =>
      input._members.set(
        `lib/${abi}/libpolaris_lib.so`,
        fakeAppSo({ cls: 64, machine: ABIS[abi].machine }),
      ),
    { abi },
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /读到 EI_CLASS=2（ELF64）/);
  assert.match(violations[0], /应为 EI_CLASS=1（ELF32）/);
});

test('换了 ABI 之后，构建 tag 与剥符号两条判据照样有牙', () => {
  const abi = 'x86_64';
  const { machine } = ABIS[abi];

  const noNaive = run(
    (input) => input._members.set(`lib/${abi}/libbox.so`, fakeSo({ machine, naive: 0 })),
    { abi },
  );
  assert.equal(noNaive.length, 1, noNaive.join(' | '));
  assert.match(noNaive[0], new RegExp(`lib/${abi}/libbox\\.so 里 'naive' 只出现 0 次`));

  const withDwarf = run(
    (input) =>
      input._members.set(
        `lib/${abi}/libpolaris_lib.so`,
        fakeAppSo({ machine, sections: [...REQUIRED_ELF_SECTIONS, ['.debug_info', 4096]] }),
      ),
    { abi },
  );
  assert.equal(withDwarf.length, 1, withDwarf.join(' | '));
  assert.match(withDwarf[0], /还留着 1 个调试信息节/);
  assert.match(withDwarf[0], /\.cargo\/config\.toml/);
});

// ───────────────────── 出厂权限集判据（判据 ⑦）─────────────────────
//
// 这条判据存在的理由（2026-09-06 实证）：本仓自有的 AndroidManifest.xml 不是 APK 的权限集，
// APK 带的是 manifest merger 合并库 manifest 之后的集合。本仓 8 条、合并后 11 条，两者今天就已经不等。
// 于是「升一版 tauri 插件 ⇒ APK 多一条敏感权限」这一形，在本判据之前没有任何一处会说话。
//
// 下面这段是**本机真 aapt2 的真实输出**（build-tools 36.0.0，喂的是由本仓 packaged manifest
// 现场 link 出来的 APK），逐字记下来当夹具。解析器的形状因此不是从文档抄的：
// `permission:`（自己声明的）与 `uses-permission:`（请求的）只差一个词，行首锚定是承重的。
const AAPT2_DUMP = [
  'package: com.polaris2.app',
  'permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION',
  "uses-permission: name='android.permission.INTERNET'",
  "uses-permission: name='android.permission.FOREGROUND_SERVICE'",
  "uses-permission: name='android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED'",
  "uses-permission: name='android.permission.POST_NOTIFICATIONS'",
  "uses-permission: name='android.permission.ACCESS_NETWORK_STATE'",
  "uses-permission: name='android.permission.CHANGE_NETWORK_STATE'",
  "uses-permission: name='android.permission.ACCESS_WIFI_STATE'",
  "uses-permission: name='android.permission.REQUEST_INSTALL_PACKAGES'",
  "uses-permission: name='android.permission.RECEIVE_BOOT_COMPLETED'",
  "uses-permission: name='android.permission.WAKE_LOCK'",
  "uses-permission: name='com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'",
  '',
].join('\n');

test('反向对照：真 aapt2 输出解析出 11 条请求 + 1 条声明，且判据全绿', () => {
  const parsed = parseAapt2Permissions(AAPT2_DUMP);
  assert.equal(parsed.packageName, 'com.polaris2.app');
  assert.equal(parsed.uses.length, 11);
  assert.equal(parsed.declared.length, 1);
  parsed.declaredProtectionLevels = { 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION': 2 };
  assert.deepEqual(permissionViolations(parsed), []);
});

test('Debug 包只重基 applicationId 权限，Release 身份与其余权限仍严格验收', () => {
  const debug = parseAapt2Permissions(AAPT2_DUMP.replaceAll('com.polaris2.app', 'com.polaris2.app.debug'));
  assert.equal(debug.packageName, 'com.polaris2.app.debug');
  debug.declaredProtectionLevels = { 'com.polaris2.app.debug.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION': 2 };
  assert.deepEqual(permissionViolations(debug), []);
  const wrongPermission = parseAapt2Permissions(
    AAPT2_DUMP.replace('package: com.polaris2.app', 'package: com.polaris2.app.debug'),
  );
  assert.ok(permissionViolations(wrongPermission).some((v) => v.includes('DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION')));
  const unexpectedId = parseAapt2Permissions(
    AAPT2_DUMP.replace('package: com.polaris2.app', 'package: com.polaris2.app.other'),
  );
  assert.match(permissionViolations(unexpectedId)[0], /applicationId/);
});

test('解析器按行首锚定：`permission:` 那一行不许被算成一次请求', () => {
  const parsed = parseAapt2Permissions(AAPT2_DUMP);
  // 同一个名字既被声明也被请求（androidx 那条就是这样）——两个集合各算各的，不许串。
  assert.ok(parsed.declared.includes('com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'));
  assert.equal(
    parsed.uses.filter((n) => n === 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION').length,
    1,
    '请求集里那条应恰好来自 uses-permission 行，声明行不许再算一次',
  );
});

test('M11 形态：依赖往合并 manifest 注入 QUERY_ALL_PACKAGES ⇒ 红，且点名那一条', () => {
  const parsed = parseAapt2Permissions(
    AAPT2_DUMP.replace(
      "uses-permission: name='android.permission.INTERNET'",
      "uses-permission: name='android.permission.INTERNET'\nuses-permission: name='android.permission.QUERY_ALL_PACKAGES'",
    ),
  );
  parsed.declaredProtectionLevels = { 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION': 2 };
  const violations = permissionViolations(parsed);
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /没登记/);
  assert.match(violations[0], /QUERY_ALL_PACKAGES/);
});

test('M12 形态：某条腿的权限被误删 ⇒ 红，且点名那一条', () => {
  const parsed = parseAapt2Permissions(
    AAPT2_DUMP.replace("uses-permission: name='android.permission.REQUEST_INSTALL_PACKAGES'\n", ''),
  );
  parsed.declaredProtectionLevels = { 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION': 2 };
  const violations = permissionViolations(parsed);
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /不存在/);
  assert.match(violations[0], /REQUEST_INSTALL_PACKAGES/);
});

test('M13 形态：库悄悄多声明一条自建 permission ⇒ 红（声明面与请求面各判各的）', () => {
  const parsed = parseAapt2Permissions(
    AAPT2_DUMP.replace(
      'permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION',
      'permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION\npermission: com.polaris2.app.SOMETHING_NEW',
    ),
  );
  parsed.declaredProtectionLevels = { 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION': 2 };
  const violations = permissionViolations(parsed);
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /自建 permission/);
  assert.match(violations[0], /SOMETHING_NEW/);
});

test('取材面塌陷：aapt2 缺席 / 输出读不出 ⇒ 红在取材面，不判绿也不报成「少了 11 条」', () => {
  // `null` = 工具找不到或调用失败。
  const missing = permissionViolations(null);
  assert.equal(missing.length, 1);
  assert.match(missing[0], /读不出出厂 APK 的权限集/);
  // 没有 `package:` 行 ⇒ 解析器自己就返回 null（而不是一个空集）。
  assert.equal(parseAapt2Permissions('Error: failed to open apk\n'), null);
  assert.equal(parseAapt2Permissions(''), null);
});

test('正面断言有牙：解析器只读到零星几条 ⇒ 红在下限，不去逐条报差集', () => {
  const violations = permissionViolations({ uses: ['android.permission.INTERNET'], declared: [] });
  assert.equal(violations.length, 1, `应恰 1 条（下限那条），实为：${violations.join(' | ')}`);
  assert.match(violations[0], /解析口径塌了/);
});

test('登记表自身：本仓自有恰 9 条、库注入恰 2 条，且每条都写了 from 与 why', () => {
  // 2026-09-25：RECEIVE_BOOT_COMPLETED 由「库注入」改为本仓自有（开机自动连接真的消费它），8+3 → 9+2。
  // 「今天 9 + 2」这件事本身要被钉住：把一条自有权限改标成「库注入」来消一个红，
  // 或反过来把库注入的那三条抹掉当自有的，都会在这里红。
  const self = APK_PERMISSION_REGISTRY.filter((e) => e.from === 'self');
  const injected = APK_PERMISSION_REGISTRY.filter((e) => e.from !== 'self');
  assert.equal(self.length, 9, '本仓自有 uses-permission 的条数变了 —— 权限画像变了，来这里答一次题');
  assert.equal(injected.length, 2, '库注入的条数变了 —— 依赖带进来的权限变了，逐条登记来源与理由');
  for (const entry of [...APK_PERMISSION_REGISTRY, ...APK_DECLARED_PERMISSIONS]) {
    assert.ok(entry.from && entry.from.length > 0, `${entry.name} 没写 from`);
    assert.ok(
      entry.why && [...entry.why].length >= 20,
      `${entry.name} 的理由太短：${entry.why} —— 要说清「缺了它哪条腿会怎样失败」`,
    );
    assert.ok(!entry.why.includes(entry.name), `${entry.name} 的理由只是把权限名重复了一遍`);
  }
  // 名字不许重复：重复会让「集合恰等」在一条被删掉时仍然绿。
  const names = APK_PERMISSION_REGISTRY.map((e) => e.name);
  assert.equal(new Set(names).size, names.length, '登记表里有重复条目');
});


test('16 KB alignment checks every LOAD, including dependency libraries', () => {
  const so = fakeElf();
  assert.equal(elfPageAlignment(so).ok, true);
  so.writeBigUInt64LE(4096n, 64 + 56 + 48);
  assert.equal(elfPageAlignment(so).ok, false);
  const result = run(input => input._members.set(`lib/${ABI}/libextra.so`, so));
  assert.ok(result.some(message => message.includes('libextra.so') && message.includes('16 KB')));
  assert.equal(elfPageAlignment(so.subarray(0, 70)).ok, false);
});

function androidNote(api = 24) {
  const note = Buffer.alloc(24);
  note.writeUInt32LE(8, 0);
  note.writeUInt32LE(4, 4);
  note.writeUInt32LE(1, 8);
  note.write('Android', 12);
  note.writeUInt32LE(api, 20);
  return note;
}

function aarElf(abi, notes) {
  const { elfClass, machine } = ABIS[abi];
  const wide = elfClass === 2;
  const raw = fakeElf({ cls: wide ? 64 : 32, machine, payload: notes });
  const secondRow = wide ? 120 : 84;
  const payload = wide ? 176 : 116;
  if (notes) {
    raw.writeUInt32LE(4, secondRow);
    if (wide) {
      raw.writeBigUInt64LE(BigInt(payload), secondRow + 8);
      raw.writeBigUInt64LE(BigInt(notes.length), secondRow + 32);
    } else {
      raw.writeUInt32LE(payload, secondRow + 4);
      raw.writeUInt32LE(notes.length, secondRow + 16);
    }
  }
  return raw;
}

test('AAR ELF CLI preserves all ABI receipt rows and rejects malformed member bytes', () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-aar-elf-'));
  const script = fileURLToPath(new URL('./verify-apk.mjs', import.meta.url));
  const path = join(fixture.path, 'member.so');
  const inspect = (raw, abi, library = 'libbox', { size = raw.length, sha, file = path } = {}) => {
    writeFileSync(path, raw);
    const pin = sha ?? createHash('sha256').update(raw).digest('hex');
    const child = spawnSync(process.execPath, [script, '--aar-elf', file, abi, size, pin, library],
      { encoding: 'utf8', timeout: 5000 });
    assert.ifError(child.error);
    return child;
  };
  const rejected = (raw, abi, why, library, pin) => {
    const child = inspect(raw, abi, library, pin);
    assert.equal(child.status, 1, child.stdout);
    assert.match(child.stderr, why);
  };
  const load = { offset: 0, virtualAddress: 0, alignment: 16384, bytes: 0 };
  for (const abi of Object.keys(ABIS)) {
    const raw = aarElf(abi, Buffer.concat([androidNote(), Buffer.alloc(5)]));
    const child = inspect(raw, abi);
    assert.equal(child.status, 0, child.stderr);
    assert.deepEqual(JSON.parse(child.stdout), { ptLoad: [load], androidAPI: 24 });
    const dependency = inspect(aarElf(abi), abi, 'dependency');
    assert.equal(dependency.status, 0, dependency.stderr);
    assert.deepEqual(JSON.parse(dependency.stdout), { ptLoad: [load, load] });
  }

  const abi = DEFAULT_ABI;
  rejected(aarElf(abi, androidNote(23)), abi, /Android API differs/);
  rejected(aarElf(abi), abi, /one actual Android ident API note required/);
  rejected(aarElf(abi, Buffer.concat([androidNote(), androidNote()])), abi, /one actual Android ident API note required/);
  const foreign = androidNote();
  foreign.write('AndroidX', 12);
  rejected(aarElf(abi, foreign), abi, /one actual Android ident API note required/);
  rejected(aarElf(abi, androidNote().subarray(0, 23)), abi, /Android note payload bounds/);
  rejected(aarElf(abi, Buffer.concat([androidNote(), Buffer.from([1])])), abi, /Android note header bounds/);
  rejected(aarElf('x86', androidNote()), abi, /ELF shared-library ABI differs/);
  rejected(aarElf('x86_64', androidNote()), abi, /ELF shared-library ABI differs/);

  const raw = aarElf(abi, androidNote());
  rejected(raw, abi, /bounded original AAR ELF member pin required/, 'libbox', { size: 2 ** 31 + 1 });
  rejected(raw, abi, /AAR ELF member size differs/, 'libbox', { size: raw.length + 1 });
  rejected(raw, abi, /AAR ELF member pin differs/, 'libbox', { sha: '0'.repeat(64) });
  if (process.platform !== 'win32') {
    const link = join(fixture.path, 'link.so');
    symlinkSync(path, link);
    rejected(raw, abi, /ELOOP/, 'libbox', { file: link });
  }
  raw.writeUInt16LE(2, 16);
  rejected(raw, abi, /ELF shared-library ABI differs/);
  raw.writeUInt16LE(3, 16);
  raw[5] = 2;
  rejected(raw, abi, /ELF shared-library ABI differs/);
  raw[5] = 1;
  raw.writeUInt16LE(55, 54);
  rejected(raw, abi, /ELF program header bounds/);
  raw.writeUInt16LE(56, 54);
  raw.writeBigUInt64LE(BigInt(raw.length + 1), 120 + 32);
  rejected(raw, abi, /Android note bounds/);
  raw.writeBigUInt64LE(24n, 120 + 32);
  raw.writeBigUInt64LE(2n ** 63n, 64 + 16);
  const checked = inspect(raw, abi);
  assert.equal(checked.status, 0, checked.stderr);
  assert.match(checked.stdout, /"virtualAddress":9223372036854775808[,}]/);
  raw.writeBigUInt64LE(1n, 64 + 16);
  rejected(raw, abi, /16KiB PT_LOAD required/);
  raw.writeUInt32LE(0, 64);
  rejected(raw, abi, /ELF has no load segments/);

  const dependency = aarElf('armeabi-v7a');
  dependency.writeUInt32LE(4096, 84 + 28);
  rejected(dependency, 'armeabi-v7a', /16KiB PT_LOAD required/, 'dependency');
  dependency.writeUInt32LE(16384, 84 + 28);
  dependency.writeUInt32LE(dependency.length + 1, 84 + 16);
  rejected(dependency, 'armeabi-v7a', /ELF PT_LOAD bounds/, 'dependency');
});


// Exact retained javap -s stdout from diagnostic run 39562122098, result.json.
// SHA-256 cab4336ea2975412264b4eaab68b44a714b8e2fe20fdb4a66cbbe077ab7abeb1.
const RETAINED_AAR_HOT_JAVA = {
  "CommandServer": `Compiled from "CommandServer.java"
public final class io.nekohasekai.libbox.CommandServer implements go.Seq$Proxy {
  public final int refnum;
    descriptor: I
  public final int incRefnum();
    descriptor: ()I

  public io.nekohasekai.libbox.CommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface);
    descriptor: (Lio/nekohasekai/libbox/CommandServerHandler;Lio/nekohasekai/libbox/PlatformInterface;)V

  io.nekohasekai.libbox.CommandServer(int);
    descriptor: (I)V

  public native java.lang.String acknowledgeRuntimePersistence(java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;)Ljava/lang/String;

  public native void activateTailscaleDirectoryDomain(java.lang.String, java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;Ljava/lang/String;)V

  public native java.lang.String applyRuntimeConfig(java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;)Ljava/lang/String;

  public native void cancelNotification(java.lang.String, int) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;I)V

  public native void close();
    descriptor: ()V

  public native void closeService() throws java.lang.Exception;
    descriptor: ()V

  public native java.lang.String exportTailscaleDirectoryCapability();
    descriptor: ()Ljava/lang/String;

  public native java.lang.String exportTailscaleDirectoryRetirement();
    descriptor: ()Ljava/lang/String;

  public native java.lang.String exportTailscaleStoreRetirement();
    descriptor: ()Ljava/lang/String;

  public native boolean needFindProcess();
    descriptor: ()Z

  public native boolean needWIFIState();
    descriptor: ()Z

  public native void pause();
    descriptor: ()V

  public native java.lang.String planRuntimeConfig(java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;)Ljava/lang/String;

  public native void recordScreenState(boolean);
    descriptor: (Z)V

  public native void resetNetwork();
    descriptor: ()V

  public native java.lang.String runtimeCapabilities() throws java.lang.Exception;
    descriptor: ()Ljava/lang/String;

  public native java.lang.String runtimeConfigStatus(java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;)Ljava/lang/String;

  public native void setError(java.lang.String);
    descriptor: (Ljava/lang/String;)V

  public native void start() throws java.lang.Exception;
    descriptor: ()V

  public native void startOrReloadService(java.lang.String, io.nekohasekai.libbox.OverrideOptions) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;Lio/nekohasekai/libbox/OverrideOptions;)V

  public native void startOrReloadServiceWithRetryableRuntimePersistence(java.lang.String, io.nekohasekai.libbox.OverrideOptions, io.nekohasekai.libbox.RuntimePersistenceHandler) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;Lio/nekohasekai/libbox/OverrideOptions;Lio/nekohasekai/libbox/RuntimePersistenceHandler;)V

  public native void startOrReloadServiceWithRuntimePersistence(java.lang.String, io.nekohasekai.libbox.OverrideOptions, io.nekohasekai.libbox.RuntimePersistenceHandler) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;Lio/nekohasekai/libbox/OverrideOptions;Lio/nekohasekai/libbox/RuntimePersistenceHandler;)V

  public native void updateWIFIState();
    descriptor: ()V

  public native void wake();
    descriptor: ()V

  public native void wakeNow();
    descriptor: ()V

  public native void writeMessage(int, java.lang.String);
    descriptor: (ILjava/lang/String;)V

  public boolean equals(java.lang.Object);
    descriptor: (Ljava/lang/Object;)Z

  public int hashCode();
    descriptor: ()I

  public java.lang.String toString();
    descriptor: ()Ljava/lang/String;

  static {};
    descriptor: ()V
}
`,
  "Libbox$proxyRuntimePersistenceHandler": `Compiled from "Libbox.java"
final class io.nekohasekai.libbox.Libbox$proxyRuntimePersistenceHandler implements go.Seq$Proxy,io.nekohasekai.libbox.RuntimePersistenceHandler {
  public final int refnum;
    descriptor: I
  public final int incRefnum();
    descriptor: ()I

  io.nekohasekai.libbox.Libbox$proxyRuntimePersistenceHandler(int);
    descriptor: (I)V

  public native void persistRuntimeConfiguration(java.lang.String, long, java.lang.String, java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;JLjava/lang/String;Ljava/lang/String;)V

  public native java.lang.String runtimePersistenceMode();
    descriptor: ()Ljava/lang/String;
}
`,
  "OverrideOptions": `Compiled from "OverrideOptions.java"
public final class io.nekohasekai.libbox.OverrideOptions implements go.Seq$Proxy {
  public final int refnum;
    descriptor: I
  public final int incRefnum();
    descriptor: ()I

  io.nekohasekai.libbox.OverrideOptions(int);
    descriptor: (I)V

  public io.nekohasekai.libbox.OverrideOptions();
    descriptor: ()V

  public final native boolean getAutoRedirect();
    descriptor: ()Z

  public final native void setAutoRedirect(boolean);
    descriptor: (Z)V

  public final native io.nekohasekai.libbox.StringIterator getIncludePackage();
    descriptor: ()Lio/nekohasekai/libbox/StringIterator;

  public final native void setIncludePackage(io.nekohasekai.libbox.StringIterator);
    descriptor: (Lio/nekohasekai/libbox/StringIterator;)V

  public final native io.nekohasekai.libbox.StringIterator getExcludePackage();
    descriptor: ()Lio/nekohasekai/libbox/StringIterator;

  public final native void setExcludePackage(io.nekohasekai.libbox.StringIterator);
    descriptor: (Lio/nekohasekai/libbox/StringIterator;)V

  public boolean equals(java.lang.Object);
    descriptor: (Ljava/lang/Object;)Z

  public int hashCode();
    descriptor: ()I

  public java.lang.String toString();
    descriptor: ()Ljava/lang/String;

  static {};
    descriptor: ()V
}
`,
  "RuntimePersistenceHandler": `Compiled from "RuntimePersistenceHandler.java"
public interface io.nekohasekai.libbox.RuntimePersistenceHandler {
  public abstract void persistRuntimeConfiguration(java.lang.String, long, java.lang.String, java.lang.String) throws java.lang.Exception;
    descriptor: (Ljava/lang/String;JLjava/lang/String;Ljava/lang/String;)V

  public abstract java.lang.String runtimePersistenceMode();
    descriptor: ()Ljava/lang/String;
}
`,
};

const AAR_JAVA_OBSERVATION = {
  Libbox: `newTransientCommandServer(
newStrictCommandServer(
newManagedStrictCommandServer(
interfaceUpdateListenerIdentity(
checkConfigWithResult(
java.lang.String version()
ConfigValidationContractVersion = "polaris-validation-v1"
`,
  CommandServer: `startOrReloadService(
public native java.lang.String exportTailscaleStoreRetirement();
public native java.lang.String exportTailscaleDirectoryCapability();
public native java.lang.String exportTailscaleDirectoryRetirement();
activateTailscaleDirectoryDomain(
void wakeNow()
void recordScreenState(boolean)
`,
  PlatformInterface: `void bindInterfaceControl(int, java.lang.String)
`,
  ConfigValidationResult: `getRequestID()
getConfigDigest()
getContractVersion()
getValidation()
getCleanup()
getValidationError()
getCleanupError()
getTailscaleStoreRetirement()
getTailscaleStoreMembership()
`,
  ExchangeContext: `void errnoCode(int)
void errorCode(int)
void onCancel(
void rawSuccess(byte[])
void success(java.lang.String)
`,
  LocalDNSTransport: `void exchange(
void lookup(
boolean raw()
`,
  Func: `void invoke()
`,
};

const AAR_DEFINED_JNI = [
  'JNI_OnLoad',
  'Java_io_nekohasekai_libbox_CommandServer_runtimeCapabilities',
  'Java_io_nekohasekai_libbox_CommandServer_startOrReloadServiceWithRetryableRuntimePersistence',
  'Java_io_nekohasekai_libbox_Libbox_00024proxyRuntimePersistenceHandler_persistRuntimeConfiguration',
  'Java_io_nekohasekai_libbox_Libbox_00024proxyRuntimePersistenceHandler_runtimePersistenceMode',
  'Java_go_Seq_incRef',
];


const AAR_HOT_METHOD_NAMES = {
  CommandServer: ['startOrReloadServiceWithRetryableRuntimePersistence', 'runtimeCapabilities'],
  RuntimePersistenceHandler: ['runtimePersistenceMode', 'persistRuntimeConfiguration'],
  'Libbox$proxyRuntimePersistenceHandler': ['runtimePersistenceMode', 'persistRuntimeConfiguration'],
  OverrideOptions: ['io.nekohasekai.libbox.OverrideOptions'],
};

function aarInterfaceObservation() {
  const definitions = [...AAR_DEFINED_JNI, 'unrelated'].map(name => '1: 0 16 FUNC GLOBAL DEFAULT 13 ' + name);
  return {
    javaInterfaces: { ...AAR_JAVA_OBSERVATION },
    hotJavaInterfaces: { ...RETAINED_AAR_HOT_JAVA },
    jni: Object.fromEntries(Object.keys(ABIS).map(abi => [abi, definitions.join('\n')])),
  };
}

test('AAR Java observes the retained receivers, exact method/error descriptors, and read-only validation API', () => {
  const observation = aarInterfaceObservation();
  const expected = [...AAR_DEFINED_JNI].sort();
  assert.deepEqual(verifyAarInterfaces(observation), Object.fromEntries(Object.keys(ABIS).map(abi => [abi, expected])));
  const mutate = (name, text, expectedError) => {
    const hotJavaInterfaces = { ...observation.hotJavaInterfaces, [name]: text };
    assert.throws(() => verifyAarInterfaces({ ...observation, hotJavaInterfaces }), expectedError);
  };
  for (const [name, source] of Object.entries(RETAINED_AAR_HOT_JAVA)) {
    const candidates = [...source.matchAll(/^  (.+;)\n    descriptor: (.+)$/gm)]
      .filter(([, decl]) => AAR_HOT_METHOD_NAMES[name].some(method => decl.includes(' ' + method + '(')))
      .filter(([, decl]) => name !== 'OverrideOptions' || decl.includes('()'));
    assert.equal(candidates.length, AAR_HOT_METHOD_NAMES[name].length, name);
    for (const [pair, decl, descriptor] of candidates) {
      const variations = [
        decl.replace('public ', 'public static '), decl.replace('public ', 'protected '), decl.replace('public ', ''),
        decl.replace('(', 'Wrong('), decl.replace(/ (native|abstract) /, ' '), '',
      ].filter(other => other !== decl).map(other => pair.replace(decl, other));
      if (decl.includes(' throws java.lang.Exception;')) {
        variations.push(pair.replace(' throws java.lang.Exception', ''), pair.replace('java.lang.Exception', 'java.lang.Error'));
      } else {
        variations.push(pair.replace(';', ' throws java.lang.Exception;'));
      }
      variations.push(pair.replace('descriptor: ' + descriptor, 'descriptor: ()I'), pair + '\n' + pair);
      if (name !== 'OverrideOptions') {
        variations.push(pair + '\n' + pair.replace('(', '(int, '));
      }
      for (const mutation of variations) {
        mutate(name, source.replace(pair, mutation), /HOT Java method\/descriptor\/error differs/);
      }
    }
    mutate(name, source.replace('io.nekohasekai.libbox.' + name, 'io.nekohasekai.libbox.ForeignReceiver'),
      /HOT Java receiver\/interface differs/);
  }
  const proxy = 'Libbox$proxyRuntimePersistenceHandler';
  mutate(proxy, observation.hotJavaInterfaces[proxy].replace(',io.nekohasekai.libbox.RuntimePersistenceHandler', ''),
    /HOT Java receiver\/interface differs/);
  mutate('OverrideOptions', observation.hotJavaInterfaces.OverrideOptions.replace(
    '  public io.nekohasekai.libbox.OverrideOptions();\n    descriptor: ()V\n', ''),
  /HOT Java method\/descriptor\/error differs/);
  mutate('CommandServer', observation.hotJavaInterfaces.CommandServer.replace('public final class', 'public\uFEFFfinal class'),
    /HOT Java receiver\/interface differs/);
  mutate('CommandServer', observation.hotJavaInterfaces.CommandServer.replace(
    '\npublic final class', '\u2028public final class'), /HOT Java receiver\/interface differs/);
  assert.deepEqual(verifyAarInterfaces({
    ...observation,
    hotJavaInterfaces: {
      ...observation.hotJavaInterfaces,
      CommandServer: observation.hotJavaInterfaces.CommandServer.replace('public final class', 'public\u0085final class'),
    },
  }), verifyAarInterfaces(observation));
  assert.throws(() => verifyAarInterfaces({ ...observation, hotJavaInterfaces: {} }), /HOT Java class inventory missing/);

  for (const [name, source] of Object.entries(AAR_JAVA_OBSERVATION)) {
    for (const required of source.trimEnd().split('\n')) {
      const change = { ...observation.javaInterfaces, [name]: source.replace(required, '') };
      assert.throws(() => verifyAarInterfaces({ ...observation, javaInterfaces: change }),
        /generated Java API differs|validation API contract differs/);
    }
  }
  for (const setter of ['setRequestID', 'setConfigDigest', 'setValidation', 'setCleanup', 'setTailscaleStoreRetirement', 'setTailscaleStoreMembership']) {
    const javaInterfaces = { ...observation.javaInterfaces };
    javaInterfaces.ConfigValidationResult += 'public void ' + setter + '();\n';
    assert.throws(() => verifyAarInterfaces({ ...observation, javaInterfaces }), /validation result must be readonly/);
  }
});

test('AAR JNI requires all defined bridges on each ABI and retains only sorted JNI exports', () => {
  const observation = aarInterfaceObservation();
  for (const abi of Object.keys(ABIS)) {
    for (const bridge of AAR_DEFINED_JNI) {
      const source = observation.jni[abi];
      const mutations = [
        source.replace('DEFAULT 13 ' + bridge, 'DEFAULT UND ' + bridge),
        source.replace('DEFAULT 13 ' + bridge, 'PROTECTED 13 ' + bridge),
        source.replace('GLOBAL DEFAULT 13 ' + bridge, 'WEAK DEFAULT 13 ' + bridge),
        source.replace(bridge, bridge.replace('CommandServer', 'CommandClient') +
          (bridge.includes('CommandServer') ? '' : bridge.startsWith('Java_go_Seq_') ? 'OtherPrefix' : 'Wrong')),
      ];
      // A different go.Seq method still meets the original prefix-only predicate.
      if (bridge.startsWith('Java_go_Seq_')) mutations.pop();
      for (const text of mutations) {
        const jni = { ...observation.jni, [abi]: text };
        assert.throws(() => verifyAarInterfaces({ ...observation, jni }), /actual (generated JNI loader\/sequence|HOT JNI) exports missing/);
      }
    }
  }
});

test('AAR interface commands bind observations to the exact tool captures', () => {
  using fixture = mkdtempDisposableSync(join(tmpdir(), 'polaris-aar-interface-'));
  const script = fileURLToPath(new URL('./verify-apk.mjs', import.meta.url));
  const run = (...args) => {
    const result = spawnSync(process.execPath, [script, ...args], { encoding: 'utf8', timeout: 5000 });
    assert.ifError(result.error);
    return result;
  };
  const captures = run('--aar-interface-classes');
  assert.equal(captures.status, 0, captures.stderr);
  const plan = JSON.parse(captures.stdout);
  assert.deepEqual(plan.javaInterfaces, Object.keys(AAR_JAVA_OBSERVATION));
  assert.deepEqual([...plan.hotJavaInterfaces].sort(), Object.keys(RETAINED_AAR_HOT_JAVA).sort());
  const source = readFileSync(new URL('./mobile-core/component-admission.mjs', import.meta.url), 'utf8');
  const owner = source.split('function inspectAar(', 2)[1].split('function parsePlist(', 1)[0];
  for (const requirement of ['formal AAR must contain all four ABIs', 'verifyNativeBuildInfo({ raw, source, graph',
    'aarElfFacts(bytes, abi, libbox)', "'--dyn-syms', '--wide'", 'verifyAarInterfaces(observations)', 'aarInterfaceClasses()']) {
    assert.ok(owner.includes(requirement), requirement);
  }
  const path = join(fixture.path, 'observations.json');
  const observation = aarInterfaceObservation();
  const raw = Buffer.from(JSON.stringify(observation));
  const pin = createHash('sha256').update(raw).digest('hex');
  writeFileSync(path, raw);
  const checked = run('--aar-interfaces', path, raw.length, pin);
  assert.equal(checked.status, 0, checked.stderr);
  assert.deepEqual(JSON.parse(checked.stdout), verifyAarInterfaces(observation));
  const oversized = run('--aar-interfaces', path, 16 * 1024 ** 2 + 1, pin);
  assert.equal(oversized.status, 1);
  assert.match(oversized.stderr, /bounded original AAR interface observation pin required/);
  const bad = Buffer.from(raw);
  bad[1] ^= 1;
  writeFileSync(path, bad);
  const tampered = run('--aar-interfaces', path, raw.length, pin);
  assert.equal(tampered.status, 1);
  assert.match(tampered.stderr, /AAR interface observation pin differs/);
  if (process.platform !== 'win32') {
    const link = join(fixture.path, 'link.json');
    writeFileSync(path, raw);
    symlinkSync(path, link);
    const redirected = run('--aar-interfaces', link, raw.length, pin);
    assert.equal(redirected.status, 1);
    assert.match(redirected.stderr, /ELOOP/);
  }
});

test('geo exact inventory rejects same-count renames and nested replacements', () => {
  for (const replacement of ['renamed.srs', 'nested/geosite-0.srs']) {
    const violations = run((input) => {
      const old = `${ASSET_UP}${SRS_DIR}/geosite-0.srs`;
      const bytes = input._members.get(old);
      input._members.delete(old);
      input._members.set(`${ASSET_UP}${SRS_DIR}/${replacement}`, bytes);
    });
    assert.equal(violations.length, 1, violations.join(' | '));
    assert.match(violations[0], /路径集不一致/);
    assert.ok(violations[0].includes(replacement));
  }
});

test('geo exact bytes reject empty, truncated, and same-size changed content for every ABI', () => {
  for (const abi of Object.keys(ABIS)) {
    for (const mutation of [() => Buffer.alloc(0), (bytes) => bytes.subarray(0, -1),
      (bytes) => { const changed = Buffer.from(bytes); changed[changed.length - 1] ^= 1; return changed; }]) {
      const violations = run((input) => {
        const key = `${ASSET_UP}${SRS_DIR}/geosite-0.srs`;
        input._members.set(key, mutation(input._members.get(key)));
      }, { abi });
      assert.equal(violations.length, 1, violations.join(' | '));
      assert.match(violations[0], /geosite-0.srs.*字节不一致/);
    }
  }
});

test('geo truth must be supplied and contain valid nonempty SRS seeds', () => {
  const missing = run((input) => { input.repoSrs = undefined; });
  assert.equal(missing.length, 1);
  assert.match(missing[0], /资源真值缺失/);
  for (const bytes of [Buffer.alloc(0), Buffer.from('SRS'), Buffer.from('bad!'), Buffer.from([0xd3, 0xd2, 0xd3, 3])]) {
    const violations = run((input) => {
      input.repoSrs.set('geosite-0.srs', bytes);
      input._members.set(`${ASSET_UP}${SRS_DIR}/geosite-0.srs`, bytes);
    });
    assert.equal(violations.length, 1, violations.join(' | '));
    assert.match(violations[0], /工作树.*不是有效 SRS/);
  }
});

test('geo byte check reads each expected member once and ignores ZIP mode/compression metadata', () => {
  const reads = new Map();
  const violations = run((input) => {
    const read = input.readMember;
    input.readMember = (name) => { reads.set(name, (reads.get(name) ?? 0) + 1); return read(name); };
    for (const [name, row] of input.zipEntries) {
      if (name.endsWith('.srs')) { row.method = 0; row.externalAttributes = 0; }
    }
  });
  assert.deepEqual(violations, []);
  const geoReads = [...reads].filter(([name]) => name.endsWith('.srs'));
  assert.equal(geoReads.length, SRS_COUNT);
  assert.ok(geoReads.every(([, count]) => count === 1));
});
