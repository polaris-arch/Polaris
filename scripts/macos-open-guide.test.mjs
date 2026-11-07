import assert from 'node:assert/strict';
import { appendFileSync, chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statfsSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { macOpenGuideViolations } from './lib/macos-open-guide.mjs';
const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const guide = read('packaging/macos-dmg-open-guide.txt');
const names = ['README.md', 'README.en.md', 'README.zh-TW.md', 'README.ru.md', 'README.fa.md'];
const readmes = Object.fromEntries(names.map((name) => [name, read(name)]));
test('all five README translations match the reviewed safe guide', () => {
  assert.deepEqual(macOpenGuideViolations(guide, readmes), []);
});
test('each missing translation or mismatched README fails closed', () => {
  const sections = [...guide.matchAll(/═+\n  ([^\n]+)\n═+\n\n([\s\S]*?)(?=\n\n═+|$)/gu)];
  assert.equal(sections.length, 5);
  for (const section of sections) assert.ok(macOpenGuideViolations(guide.replace(section[0], ''), readmes).length > 0);
  for (const name of names) assert.ok(macOpenGuideViolations(guide, { ...readmes, [name]: '' }).length > 0);
});
test('removing stop guidance fails even if README is changed to match', () => {
  const changed = guide.replace('Do not clear quarantine attributes or', 'You may clear quarantine attributes or');
  const changedReadmes = { ...readmes, 'README.en.md': readmes['README.en.md'].replace('Do not clear quarantine attributes or', 'You may clear quarantine attributes or') };
  assert.ok(macOpenGuideViolations(changed, changedReadmes).some((e) => e.includes('lost safety instruction')));
});
test('bypass command cannot return in the guide or any README', () => {
  for (const command of ['xattr -cr /Applications/Polaris.app', 'spctl --master-disable']) {
    assert.ok(macOpenGuideViolations(`${guide}\n${command}`, readmes).some((e) => e.includes('security-bypass')));
    for (const name of names) assert.ok(macOpenGuideViolations(guide, { ...readmes, [name]: `${readmes[name]}\n${command}` }).some((e) => e.includes('security-bypass')));
  }
});
test('final DMG compares the entire canonical guide and keeps signature gates', () => {
  const workflow = read('.github/workflows/package.yml');
  assert.ok(workflow.includes('if ! cmp -s packaging/macos-dmg-open-guide.txt "$mnt/$guide_name"; then'));
  assert.equal(workflow.split('codesign --verify --deep --strict --verbose=2 "${apps[0]}"').length - 1, 2);
  assert.ok(workflow.includes('if ! bash scripts/macos-nested-code.sh verify "${apps[0]}" "$core_ref"; then'));
});

const workflow = read('.github/workflows/package.yml');
const guideSteps = ['Embed macOS first-open guide and seal app bundle', 'Verify guide is actually inside the dmg'];
function guideStep(name) {
  const step = workflow.split(`      - name: ${name}\n`)[1]?.split('\n      - name: ')[0];
  assert.ok(step, `missing workflow step: ${name}`);
  return step.split('        run: |\n')[1].split('\n').filter((line) => line.startsWith('          ') || line === '')
    .map((line) => line.slice(10)).join('\n')
    .replaceAll('${{ matrix.rust_target }}', 'fixture-target').replaceAll('${{ matrix.mac_arch_tag }}', 'mac-arm64');
}

function assertOwnedGuideScript(name, script) {
  assert.ok(script);
  const create = script.indexOf('set targetWindow to make new Finder window to targetFolder');
  const id = script.indexOf('set targetWindowID to (get id of targetWindow)', create);
  const target = script.indexOf('if ((get target of targetWindow) as alias) is not targetFolder then error', id);
  const closeable = script.indexOf('if not (get closeable of targetWindow) then error', target);
  const firstUse = script.indexOf(name === guideSteps[0] ? 'set current view of targetWindow' : 'set guidePosition to position of item guideName of targetFolder');
  const readback = script.indexOf('set guidePosition to position of item guideName of targetFolder', closeable);
  const recheck = script.indexOf('if ((get target of closingWindow) as alias) is not targetFolder then error', readback);
  const closingCloseable = script.indexOf('if not (get closeable of closingWindow) then error', recheck);
  const close = script.indexOf('close Finder window id targetWindowID', recheck);
  assert.ok(create >= 0 && id > create && target > id && closeable > target && firstUse > closeable && readback >= firstUse && recheck > readback && close > recheck);
  assert.ok(script.includes('if not (exists targetWindow) then error'));
  assert.ok(script.includes('if not (exists closingWindow) then error'));
  assert.ok(closingCloseable >= 0 && closingCloseable > recheck && closingCloseable < close,
    'closing closeable proof must exist after target recheck and before the owned close');
  assert.doesNotMatch(script, /open targetFolder|container window|front window|close (?:every|all|targetWindow|closingWindow)/);
}
test('both Finder scripts create their own viewer and validate the same ID/target before use and close', () => {
  for (const name of guideSteps) {
    const script = guideStep(name).split("<<'APPLESCRIPT'\n")[1]?.split('\nAPPLESCRIPT')[0];
    assertOwnedGuideScript(name, script);
    const lines = script.split('\n');
    const proof = lines.find(line => line.includes('if not (get closeable of closingWindow) then error'));
    const recheck = lines.find(line => line.includes('if ((get target of closingWindow) as alias) is not targetFolder then error'));
    const close = lines.find(line => line.includes('close Finder window id targetWindowID'));
    for (const line of [proof, recheck, close]) assert.equal(script.split(line).length - 1, 1);
    const missing = script.replace(proof, '');
    for (const changed of [missing,
      missing.replace(recheck, `${proof}\n${recheck}`),
      missing.replace(close, `${close}\n${proof}`),
    ]) {
      assert.notEqual(changed, script, 'closing closeable negative must really change the source');
      assert.throws(() => assertOwnedGuideScript(name, changed), /closing closeable proof must exist/);
    }
  }
});

// Execute the actual two workflow shell bodies with command stubs. No AppleScript,
// Finder, hdiutil or codesign process runs: all native commands below are functions.
const posix = process.platform !== 'win32';
const stubCommands = String.raw`
hdiutil() {
  case "$1" in
    convert)
      printf 'convert:%s\n' "$4" >> "$STUB_LOG"
      while [ "$#" -gt 0 ]; do
        if [ "$1" = -o ]; then shift; : > "$1"; break; fi
        shift
      done ;;
    resize) printf 'resize:%s\n' "$3" >> "$STUB_LOG" ;;
    attach)
      [ "$STUB_ATTACH_FAIL" = 0 ] || return 21
      while [ "$1" != -mountpoint ]; do shift; done
      shift
      printf 'attach:%s\n' "$1" >> "$STUB_LOG"
      mkdir -p "$1/Polaris.app"
      cp packaging/macos-dmg-open-guide.txt "$1/README First.txt"
      [ "$STUB_BAD_GUIDE" = 0 ] || printf corrupted > "$1/README First.txt" ;;
    detach)
      printf 'detach:%s\n' "$2" >> "$STUB_LOG"
      [ "$STUB_DETACH_FAIL" = 0 ] || return 22 ;;
    *) return 23 ;;
  esac
}
osascript() {
  cat > "$STUB_APPLESCRIPT"
  printf 'osascript:%s\n' "$2" >> "$STUB_LOG"
  count=$(sed -n '/^osascript:/p' "$STUB_LOG" | wc -l | tr -d ' ')
  if [ "$STUB_SCRIPT_FAIL_ON" = "$count" ]; then
    echo 'Finder stub: close event failed (-1708)' >&2
    return 24
  fi
  printf '%s\n' "$STUB_LAYOUT"
}
codesign() {
  printf 'codesign:%s\n' "$*" >> "$STUB_LOG"
  [ "$STUB_CODESIGN_FAIL" = 0 ]
}
stat() {
  [ "$1" = -f ] && [ "$2" = %z ] || return 25
  printf '%s\n' "$STUB_IMAGE_BYTES"
}
sleep() { :; }
`;

function runGuideStep(name, options = {}) {
  const root = mkdtempSync(join(tmpdir(), "polaris guide 'owned' $x-"));
  try {
    const bundle = join(root, 'target/fixture-target/release/bundle/dmg');
    mkdirSync(bundle, { recursive: true });
    for (let n = 0; n < (options.images ?? 1); n++) writeFileSync(join(bundle, `${n}.dmg`), 'fixture');
    mkdirSync(join(root, 'packaging'));
    writeFileSync(join(root, 'packaging/macos-dmg-open-guide.txt'), guide);
    mkdirSync(join(root, 'scripts'));
    const nested = join(root, 'scripts/macos-nested-code.sh');
    writeFileSync(nested, 'printf "nested:%s\\n" "$1" >> "$STUB_LOG"\n[ "${STUB_NESTED_FAIL:-0}" = 0 ]\n');
    chmodSync(nested, 0o700);
    const log = join(root, 'calls.log');
    writeFileSync(log, '');
    const result = spawnSync('bash', ['-c', `${stubCommands}\n${guideStep(name)}`], {
      cwd: root, encoding: 'utf8', env: { ...process.env, TMPDIR: `${root}/`, STUB_LOG: log,
        STUB_ATTACH_FAIL: '0', STUB_BAD_GUIDE: '0', STUB_DETACH_FAIL: '0', STUB_SCRIPT_FAIL_ON: '0', STUB_CODESIGN_FAIL: '0',
        STUB_IMAGE_BYTES: '41943057',
        STUB_APPLESCRIPT: join(root, 'script.applescript'), STUB_LAYOUT: name === guideSteps[0] ? '330,330' : '330,330;500', ...options.env },
    });
    assert.ifError(result.error);
    return { ...result, calls: readFileSync(log, 'utf8').trim().split('\n').filter(Boolean) };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function assertOwnMountCleanup(result) {
  const mounts = result.calls.filter((line) => line.startsWith('attach:')).map((line) => line.slice(7));
  const detached = result.calls.filter((line) => line.startsWith('detach:')).map((line) => line.slice(7));
  assert.ok(mounts.length > 0);
  assert.deepEqual(detached, mounts, 'detach must target each successfully attached mount once, without foreign mounts');
}

test('guide shell failures stop the pipeline and clean only the current owned mount, including the second image', { skip: !posix }, () => {
  for (const name of guideSteps) {
    const result = runGuideStep(name, { images: 2, env: { STUB_SCRIPT_FAIL_ON: '2' } });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Finder stub: close event failed \(-1708\)/);
    assertOwnMountCleanup(result);
    const failure = result.calls.findLastIndex((line) => line.startsWith('osascript:'));
    assert.deepEqual(result.calls.slice(failure + 1).map((line) => line.split(':')[0]), ['detach']);
  }
});

test('successful guide shell steps keep positioning, seal/strict verification and final readonly comparison', { skip: !posix }, () => {
  for (const name of guideSteps) {
    const result = runGuideStep(name);
    assert.equal(result.status, 0, result.stderr);
    assertOwnMountCleanup(result);
    assert.ok(result.calls.includes(`nested:${name === guideSteps[0] ? 'seal' : 'verify'}`));
    assert.ok(result.calls.some((line) => line.startsWith('codesign:') && line.includes('--verify --deep --strict')));
    if (name === guideSteps[0]) assert.equal(result.calls.at(-1), 'convert:UDZO');
  }
});

test('seal workspace uses logical image bytes, rounds up, and rejects empty images before attaching', { skip: !posix }, () => {
  for (const [bytes, expected] of [['8388609', '38m'], ['41943057', '102m']]) {
    const result = runGuideStep(guideSteps[0], { env: { STUB_IMAGE_BYTES: bytes } });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.calls.filter((line) => line.startsWith('resize:')), [`resize:${expected}`]);
    assertOwnMountCleanup(result);
  }
  for (const bytes of ['0', 'invalid']) {
    const result = runGuideStep(guideSteps[0], { env: { STUB_IMAGE_BYTES: bytes } });
    assert.notEqual(result.status, 0);
    assert.match(result.stdout + result.stderr, /Empty writable disk image/);
    assert.equal(result.calls.filter((line) => line.startsWith('attach:')).length, 0);
  }
});

test('native x64 seal needs rewrite space and preserves frozen resources with deep strict verification', {
  skip: process.platform === 'darwin' ? false : 'native clang/codesign/HFS+ regression requires macOS',
}, (t) => {
  const directory = mkdtempSync(join(tmpdir(), "polaris seal 'space' $x-"));
  const source = join(directory, 'source'), appName = 'SealFixture.app';
  const app = join(source, appName), resources = join(app, 'Contents/Resources/_up_/resources/mac-x64');
  const main = join(app, 'Contents/MacOS/inert'), image = join(directory, 'fixture.dmg');
  const mount = join(directory, 'mount'), calls = [];
  let attached = false, completed = false;
  const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
  function run(command, args, extra = {}) {
    const result = spawnSync(command, args, { encoding: 'utf8', timeout: 30_000, ...extra });
    assert.ifError(result.error);
    calls.push({ argv: [command, ...args], pid: result.pid, actualwait: result.status, signal: result.signal,
      stdout: result.stdout, stderr: result.stderr });
    return result;
  }
  function checked(command, args, extra) {
    const result = run(command, args, extra);
    assert.equal(result.status, 0, result.stderr);
    return result;
  }
  const script = guideStep(guideSteps[0]), lines = script.split('\n');
  const first = lines.findIndex((line) => line.trim().startsWith('image_bytes='));
  const last = lines.findIndex((line) => line.trim().startsWith('hdiutil resize -size'));
  assert.ok(first >= 0 && last > first, 'real workflow image-sizing commands missing');
  const sizing = lines.slice(first, last + 1).join('\n');
  const seal = lines.find((line) => line.trim().startsWith('codesign --force '));
  const verify = lines.find((line) => line.trim().startsWith('codesign --verify '));
  assert.ok(seal && verify, 'real workflow seal/verification commands missing');
  const nativeApp = join(mount, appName);
  const invoke = (body) => run('/bin/bash', ['-c', `set -euo pipefail\napps=("$NATIVE_APP")\n${body}`], {
    env: { ...process.env, NATIVE_APP: nativeApp, rw: image },
  });
  const contents = () => Object.fromEntries(['Contents/MacOS/inert',
    'Contents/Resources/_up_/resources/mac-x64/sing-box',
    'Contents/Resources/_up_/resources/mac-x64/polaris-helper'].map((name) => [name, hash(join(nativeApp, name))]));
  try {
    mkdirSync(resources, { recursive: true });
    mkdirSync(join(app, 'Contents/MacOS'));
    mkdirSync(mount);
    writeFileSync(join(source, '.metadata_never_index'), '');
    writeFileSync(join(directory, 'tiny.c'), 'int main(void) { return 0; }\n');
    writeFileSync(join(directory, 'large.c'),
      'volatile const unsigned char data[32*1024*1024]={1}; int main(int n,char**v){return data[(unsigned)n%sizeof(data)];}\n');
    for (const [input, output] of [['large.c', main], ['tiny.c', join(resources, 'sing-box')]]) {
      checked('/usr/bin/xcrun', ['clang', '-target', 'x86_64-apple-macos11.0', '-Wl,-no_adhoc_codesign',
        join(directory, input), '-o', output]);
    }
    copyFileSync(join(resources, 'sing-box'), join(resources, 'polaris-helper'));
    chmodSync(join(resources, 'polaris-helper'), 0o755);
    checked('/usr/bin/codesign', ['--sign', '-', '--timestamp=none', join(resources, 'polaris-helper')]);
    writeFileSync(join(app, 'Contents/Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
      <plist version="1.0"><dict><key>CFBundleExecutable</key><string>inert</string>
      <key>CFBundleIdentifier</key><string>test.polaris.seal-space</string>
      <key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>1</string></dict></plist>`);
    checked('/usr/bin/hdiutil', ['create', '-size', '42m', '-fs', 'HFS+', '-layout', 'NONE',
      '-srcfolder', source, '-format', 'UDRW', '-o', image, '-quiet']);
    // Counterfactual: the previous production du + 20MiB allocation really fails,
    // not a mocked codesign response. The same unsigned bytes are kept for repair.
    const oldSizing = 'need=$(( $(du -m "$rw" | cut -f1) + 20 ))\nhdiutil resize -size "${need}m" "$rw" >/dev/null';
    assert.equal(invoke(oldSizing).status, 0);
    checked('/usr/bin/hdiutil', ['attach', image, '-mountpoint', mount, '-nobrowse', '-noautoopen', '-quiet']);
    attached = true;
    const before = contents(), fs = statfsSync(mount), freeBefore = fs.bavail * fs.bsize;
    assert.ok(freeBefore < statSync(join(nativeApp, 'Contents/MacOS/inert')).size);
    const failed = invoke(seal);
    assert.notEqual(failed.status, 0, 'old allocation unexpectedly had sufficient signing workspace');
    assert.match(failed.stderr, /internal error in Code Signing subsystem/);
    assert.deepEqual(contents(), before, 'failed allocation must leave the original input bytes intact');
    checked('/usr/bin/hdiutil', ['detach', mount, '-quiet']);
    attached = false;
    const resized = invoke(sizing);
    assert.equal(resized.status, 0, resized.stderr);
    checked('/usr/bin/hdiutil', ['attach', image, '-mountpoint', mount, '-nobrowse', '-noautoopen', '-quiet']);
    attached = true;
    assert.deepEqual(contents(), before);
    const signed = invoke(seal);
    assert.equal(signed.status, 0, signed.stderr);
    const verified = invoke(verify);
    assert.equal(verified.status, 0, verified.stderr);
    const after = contents();
    for (const [name, bytes] of Object.entries(before)) {
      if (name.endsWith('/inert')) assert.notEqual(after[name], bytes, 'root executable must actually be signed');
      else assert.equal(after[name], bytes, 'frozen Core/helper must never be re-signed');
    }
    checked('/usr/bin/codesign', ['--verify', '--strict', join(nativeApp,
      'Contents/Resources/_up_/resources/mac-x64/polaris-helper')]);
    const unsigned = run('/usr/bin/codesign', ['--verify', '--strict', join(nativeApp,
      'Contents/Resources/_up_/resources/mac-x64/sing-box')]);
    assert.notEqual(unsigned.status, 0);
    assert.match(unsigned.stderr, /code object is not signed at all/);
    appendFileSync(join(nativeApp, 'Contents/Resources/_up_/resources/mac-x64/sing-box'), 'changed fixture');
    assert.notEqual(invoke(verify).status, 0, 'deep strict verification must still reject changed resources');
    t.diagnostic(JSON.stringify({ freeBefore, before, after, calls }));
    completed = true;
  } finally {
    if (attached) checked('/usr/bin/hdiutil', ['detach', mount, '-quiet']);
    if (completed) rmSync(directory, { recursive: true, force: true });
    else t.diagnostic(JSON.stringify({ retainedFailureDirectory: directory, calls }));
  }
});

test('layout, nested-code, signature and full-guide failures retain their gates and owned-mount cleanup', { skip: !posix }, () => {
  for (const name of guideSteps) {
    for (const env of [{ STUB_LAYOUT: '1,2;499' }, { STUB_NESTED_FAIL: '1' }, { STUB_CODESIGN_FAIL: '1' },
      ...(name === guideSteps[1] ? [{ STUB_LAYOUT: '330,330;499' }, { STUB_BAD_GUIDE: '1' }] : [])]) {
      const result = runGuideStep(name, { env });
      assert.notEqual(result.status, 0, JSON.stringify(env));
      assertOwnMountCleanup(result);
      if (name === guideSteps[0]) assert.equal(result.calls.filter((line) => line === 'convert:UDZO').length, 0);
    }
    const attachFailure = runGuideStep(name, { env: { STUB_ATTACH_FAIL: '1' } });
    assert.notEqual(attachFailure.status, 0);
    assert.equal(attachFailure.calls.filter((line) => line.startsWith('detach:')).length, 0);
  }
});

test('cleanup failure is visible and cannot turn a failed Finder operation into success', { skip: !posix }, () => {
  for (const name of guideSteps) {
    const result = runGuideStep(name, { env: { STUB_SCRIPT_FAIL_ON: '1', STUB_DETACH_FAIL: '1' } });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Finder stub: close event failed/);
    assert.match(result.stderr, /Failed to detach owned guide mount/);
    const attached = result.calls.find((line) => line.startsWith('attach:')).slice(7);
    assert.deepEqual(result.calls.filter((line) => line.startsWith('detach:')), [`detach:${attached}`, `detach:${attached}`]);
  }
});
