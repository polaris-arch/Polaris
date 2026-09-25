// Windows-only DNS refresh fix, built from pinned upstream source. Other targets
// keep official release assets. No sing-box run command is used here.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
export function verifyHash(file, expected) {
  if (!/^[a-f0-9]{64}$/.test(expected ?? '') || sha256(file) !== expected) {
    throw new Error(`SHA-256 mismatch: ${file}`);
  }
}

export function buildWindowsCore(root, manifest, dest, force) {
  const spec = manifest.windowsBuild;
  if (!spec || !/^[a-f0-9]{40}$/.test(spec.sourceCommit)
      || !/^\d+\.\d+\.\d+$/.test(spec.goVersion)
      || !/^[a-f0-9]{64}$/.test(spec.binarySha256 ?? '')
      || !spec.version.startsWith(`${manifest.bundledCoreVersion}.polaris.`)
      || !/^[1-9][0-9]*$/.test(spec.version.slice(`${manifest.bundledCoreVersion}.polaris.`.length))) {
    throw new Error('Invalid pinned Windows build manifest');
  }
  const patch = join(root, 'scripts/core-patches/windows-dns-refresh.patch');
  verifyHash(patch, spec.patchSha256);
  if (!force && existsSync(dest) && sha256(dest) === spec.binarySha256) {
    console.log(`skip (verified): Windows core @ ${spec.version}`);
    return;
  }
  const work = mkdtempSync(join(tmpdir(), 'polaris-windows-core-'));
  const tempDest = `${dest}.tmp`;
  try {
    const archive = join(work, 'source.tar.gz');
    const source = join(work, 'source');
    mkdirSync(source);
    const run = (cmd, args, options = {}) => execFileSync(cmd, args, { stdio: 'inherit', ...options });
    run('curl', ['-fL', '--retry', '3', '-o', archive,
      `https://codeload.github.com/SagerNet/sing-box/tar.gz/${spec.sourceCommit}`]);
    verifyHash(archive, spec.sourceSha256);
    run('tar', ['xzf', archive, '-C', source, '--strip-components=1']);
    run('git', ['apply', '--check', patch], { cwd: source });
    run('git', ['apply', patch], { cwd: source });
    const env = { ...process.env, GOTOOLCHAIN: `go${spec.goVersion}`, GOOS: 'windows',
      GOARCH: 'amd64', GOAMD64: 'v1', CGO_ENABLED: '0', GOFLAGS: '', GOEXPERIMENT: '', GOWORK: 'off' };
    const options = { cwd: source, env };
    // Native Windows runners exercise the cache/race regression. Cross builders
    // compile the same tests without executing a Windows binary.
    run('go', process.platform === 'win32'
      ? ['test', '-count=20', './dns/transport/local/systemconfig']
      : ['test', '-c', '-o', join(work, 'systemconfig.test.exe'), './dns/transport/local/systemconfig'], options);
    const binary = join(work, 'sing-box.exe');
    const tags = readFileSync(join(source, 'release/DEFAULT_BUILD_TAGS_WINDOWS'), 'utf8').trim();
    const flags = readFileSync(join(source, 'release/LDFLAGS'), 'utf8').trim();
    run('go', ['build', '-trimpath', '-buildvcs=false', '-tags', tags, '-ldflags',
      `${flags} -X github.com/sagernet/sing-box/constant.Version=${spec.version} -s -w -buildid=`,
      '-o', binary, './cmd/sing-box'], options);
    verifyHash(binary, spec.binarySha256);
    mkdirSync(join(dest, '..'), { recursive: true });
    copyFileSync(binary, tempDest);
    renameSync(tempDest, dest);
    console.log(`verified Windows core @ ${spec.version}: ${spec.binarySha256}`);
  } finally {
    rmSync(tempDest, { force: true });
    rmSync(work, { recursive: true, force: true });
  }
}
