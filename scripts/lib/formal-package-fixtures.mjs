// Shared inert fixtures. Linux/ZIP containers are real; 7-Zip-only formats use
// an explicit controlled backend, never native package/signature evidence.
import { cpSync, mkdirSync, copyFileSync, writeFileSync, mkdtempSync, rmSync, symlinkSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { crc32, deflateRawSync, gzipSync } from 'node:zlib';

export function fixtureZip(entries) {
  const localEntries = [], centralEntries = [];
  let offset = 0;
  for (const { name, data, mode = 0o100644, method = 0, flags = 0 } of entries) {
    const filename = Buffer.from(name), payload = Buffer.from(data);
    const packed = method === 8 ? deflateRawSync(payload) : payload;
    const crc = crc32(payload);
    const local = Buffer.alloc(30), central = Buffer.alloc(46);
    local.writeUInt32LE(0x04034b50);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(flags, 6);
    local.writeUInt16LE(method, 8);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(packed.length, 18);
    local.writeUInt32LE(payload.length, 22);
    local.writeUInt16LE(filename.length, 26);
    central.writeUInt32LE(0x02014b50);
    central.writeUInt16LE(3 << 8 | 20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(flags, 8);
    central.writeUInt16LE(method, 10);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(packed.length, 20);
    central.writeUInt32LE(payload.length, 24);
    central.writeUInt16LE(filename.length, 28);
    central.writeUInt32LE((mode << 16) >>> 0, 38);
    central.writeUInt32LE(offset, 42);
    localEntries.push(local, filename, packed);
    centralEntries.push(central, filename);
    offset += local.length + filename.length + packed.length;
  }
  const central = Buffer.concat(centralEntries), eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(central.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...localEntries, central, eocd]);
}
export function fixtureTar(entries) {
  const { Header, Pax } = createRequire(new URL('../../ui/package.json', import.meta.url))('tar');
  const blocks = [];
  for (const { name, data = '', mode = 0o644, type = 'File', linkpath = '', pax, globalPax } of entries) {
    const payload = Buffer.from(data);
    const header = new Header({ path: name, size: payload.length, mode, type, linkpath });
    if (globalPax) blocks.push(new Pax(globalPax, true).encode());
    if (header.encode() || pax) blocks.push(new Pax({ path: name, size: payload.length, mode, linkpath, ...pax }).encode());
    blocks.push(header.block, payload, Buffer.alloc((512 - payload.length % 512) % 512));
  }
  return gzipSync(Buffer.concat([...blocks, Buffer.alloc(1024)]));
}
export function copyPackagingSource(original, root) {
  mkdirSync(join(root, 'scripts'), { recursive: true });
  for (const dir of ['desktop-core', 'lib']) cpSync(join(original, 'scripts', dir), join(root, 'scripts', dir), { recursive: true, filter: p => !p.includes('__pycache__') && !p.includes('node_modules') });
  for (const name of ['verify-packaging.mjs', 'postprocess-appimage.mjs', 'release-assets.mjs']) {
    copyFileSync(join(original, 'scripts', name), join(root, 'scripts', name));
  }
  if (process.platform === 'linux') {
    const require = createRequire(join(original, 'ui/package.json'));
    const modules = join(root, 'ui/node_modules');
    mkdirSync(modules, { recursive: true });
    symlinkSync(dirname(require.resolve('tar/package.json')), join(modules, 'tar'), 'dir');
  }
  mkdirSync(join(root,'src-tauri'),{recursive:true});
  for (const name of ['core-manifest.json','build.rs','tauri.conf.json','tauri.linux.conf.json','tauri.windows.conf.json','tauri.macos-x64.conf.json','tauri.macos-arm64.conf.json']) copyFileSync(join(original,'src-tauri',name),join(root,'src-tauri',name));
}
export function writeLinuxFixture(stage, path) {
  if (path.endsWith('.deb')) {
    mkdirSync(join(stage, 'DEBIAN'), { recursive: true });
    writeFileSync(join(stage, 'DEBIAN/control'), 'Package: polaris-control\nVersion: 1.0\nArchitecture: amd64\nMaintainer: Polaris <test@example.invalid>\nDescription: inert source regression\n');
    execFileSync('dpkg-deb', ['--build', '--root-owner-group', stage, path], { stdio: 'pipe', timeout: 30_000 });
  } else {
    execFileSync('mksquashfs', [stage, path, '-noappend', '-processors', '1', '-quiet'], { stdio: 'pipe', timeout: 30_000 });
  }
}
export function writeFormalAssets(directory, names) {
  mkdirSync(directory, { recursive: true });
  for (const name of names) {
    const path = join(directory, name);
    if (/\.(dmg|exe)$/.test(name)) {
      const windows = name.endsWith('.exe');
      const files = windows
        ? { 'polaris.exe': 'ordinary App', '_up_/resources/win/polaris-helper.exe': 'ordinary Helper' }
        : { 'Polaris.app/Contents/MacOS/polaris': 'ordinary App', 'Polaris.app/Contents/Resources/_up_/resources/mac-x64/polaris-helper': 'ordinary Helper' };
      writeFileSync(path, JSON.stringify({ kind: windows ? 'Nsis' : 'HFS', files }));
    } else if (/\.(zip|apk)$/.test(name)) {
      const files = name.endsWith('.apk')
        ? { 'lib/arm64-v8a/libpolaris_lib.so': 'ordinary App' }
        : { 'polaris.exe': 'ordinary App', '_up_/resources/win/polaris-helper.exe': 'ordinary Helper' };
      writeFileSync(path, fixtureZip(Object.entries(files).map(([name, data]) => ({ name, data, method: 8 }))));
    } else {
      const stage = mkdtempSync(join(tmpdir(), 'polaris-package-fixture-'));
      try {
        for (const [name, data] of Object.entries({ 'usr/bin/polaris': 'ordinary App', 'usr/lib/Polaris/_up_/resources/linux/polaris-helper': 'ordinary Helper' })) {
          const path = join(stage, name);
          mkdirSync(dirname(path), { recursive: true });
          writeFileSync(path, data);
        }
        writeLinuxFixture(stage, path);
      } finally {
        rmSync(stage, { recursive: true, force: true });
      }
    }
  }
  if (!names.some(n=>/\.(exe|dmg)$/.test(n))) return {...process.env};
  const tools=join(dirname(directory),'.fixture-tools');mkdirSync(tools,{recursive:true});
  writeFileSync(join(tools, '7zz'), `#!/usr/bin/env node
const { readFileSync } = require('node:fs');
const args = process.argv.slice(2);
if (args[0] === 'i') { console.log('7-Zip 24.09\\nNsis'); process.exit(0); }
const record = JSON.parse(readFileSync(args[0] === 'l' ? args.at(-1) : args.at(-2), 'utf8'));
if (args[0] === 'l') {
  if (record.listing !== undefined) process.stdout.write(record.listing);
  else console.log('Type = ' + record.kind + '\\n----------\\n' + Object.entries(record.files)
    .map(([name, value]) => 'Path = ' + name + '\\nSize = ' + Buffer.byteLength(value)).join('\\n\\n'));
} else {
  const isNsis = record.kind === 'Nsis' || record.listing?.includes('Type = Nsis');
  if (!args.includes('-spd') || (!isNsis && !args.includes('-sns-'))) process.exit(22);
  process.stdout.write(record.files[args.at(-1)]);
}
`, { mode: 0o700 });
  return { ...process.env, PATH: tools + ':' + process.env.PATH };
}
