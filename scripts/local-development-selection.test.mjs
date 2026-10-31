import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LOCAL_MARKER, rejectLocalDistribution, prepareLocalInstallation } from './desktop-core/local-development.mjs';
import { selectedCoreManifest, writeSelectedCoreManifest } from './lib/core-identity.mjs';
import { createHash } from 'node:crypto';
const hash = (value) => createHash('sha256').update(value).digest('hex');
function fixture(action) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-local-selection-'));
  try { action(root); } finally { rmSync(root, { recursive: true, force: true }); }
}
test('Formal package and publisher reject local input slots, including a receipt without binaries', () => fixture((root) => {
  rejectLocalDistribution(root);
  mkdirSync(join(root, 'resources/.local-development/linux'), { recursive: true });
  writeFileSync(join(root, 'resources/.local-development/linux/selection.json'), '{}');
  assert.throws(() => rejectLocalDistribution(root), /Formal distribution rejects/);
}));
test('removing local receipts cannot turn a compiled local Helper into a Formal distribution', () => fixture((root) => {
  const stage = join(root, 'stage'); mkdirSync(stage);
  writeFileSync(join(stage, 'polaris-helper'), Buffer.concat([Buffer.from('owned-negative-fixture\0'), LOCAL_MARKER]));
  assert.throws(() => rejectLocalDistribution(root, stage), /LocalDevelopmentSelection executable/);
}));
test('an alternate directory cannot borrow local installation semantics', () => fixture((root) => {
  assert.throws(() => prepareLocalInstallation(root, join(root, 'resources/linux'), 'prepare'), /Fixed local development/);
}));
test('the existing final installation collector freezes actual Helper bytes without a CoreId self-reference', () => fixture((root) => {
  const core = Buffer.from('owned core fixture'), sidecar = Buffer.from('owned sidecar fixture');
  const contract = { version: '1.15.0-polaris-dev.control', platforms: { linux: { providerFingerprint: 'a'.repeat(64),
    core: { member: 'sing-box', bytes: core.length, sha256: hash(core) }, sidecar: { member: 'libcronet.so', bytes: sidecar.length, sha256: hash(sidecar) } } } };
  writeFileSync(join(root, 'sing-box'), core); writeFileSync(join(root, 'libcronet.so'), sidecar);
  const pure = selectedCoreManifest(contract, 'linux', '2.12.3');
  writeFileSync(join(root, 'polaris-helper'), 'first actual owned Helper');
  const first = writeSelectedCoreManifest(root, contract, 'linux', '2.12.3');
  writeFileSync(join(root, 'polaris-helper'), 'second actual owned Helper');
  const second = writeSelectedCoreManifest(root, contract, 'linux', '2.12.3');
  assert.equal(first.id, pure.id); assert.equal(second.id, pure.id);
  assert.notEqual(first.core.helperSha256, second.core.helperSha256);
  assert.equal(second.core.helperSha256, hash(Buffer.from('second actual owned Helper')));
}));
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
test('local compiler, artifact and DMG controls reject malformed selection inputs', () => {
  execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', fileURLToPath(new URL('./desktop-core/test-local-development.py', import.meta.url))], { stdio: 'pipe' });
  execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', fileURLToPath(new URL('./desktop-core/test-dmg-selection.py', import.meta.url))], { stdio: 'pipe' });
});

test('Linux containers preserve exact namespaces without weakening carrier controls', { skip: process.platform !== 'linux' }, (t) => fixture((root) => {
  // Real, small deb/SquashFS containers containing inert opaque bytes only.
  // None of these App/Helper names is a native payload or execution receipt.
  const result = spawnSync('python3', ['-B', '-c', String.raw`
import hashlib, importlib.util, io, os, shutil, subprocess, sys, tarfile, tempfile, unittest, zipfile
from pathlib import Path
spec = importlib.util.spec_from_file_location('namespace_package', sys.argv[1])
package = importlib.util.module_from_spec(spec); spec.loader.exec_module(package)
scratch = Path(sys.argv[2])
APP = b'inert namespace App'; HELPER = b'inert namespace Helper'
HELPER_PATH = 'usr/lib/Polaris/_up_/resources/linux/polaris-helper'
CONTROL = b'Package: polaris-namespace-control\nVersion: 1.0\nArchitecture: amd64\nMaintainer: Polaris <test@example.invalid>\nDescription: inert namespace regression\n'
MARKER = b'POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1'
def stage(root, app='polaris', body=APP):
    for name, data in [('usr/bin/'+app, body), (HELPER_PATH, HELPER)]:
        p=root/name; p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(data)
    return root
def containers(root, source):
    control=source/'DEBIAN'; control.mkdir(); (control/'control').write_bytes(CONTROL)
    deb=root/'fixture.deb'; image=root/'fixture.AppImage'
    subprocess.run(['dpkg-deb','--build','--root-owner-group',str(source),str(deb)], check=True, capture_output=True, timeout=30)
    subprocess.run(['mksquashfs',str(source),str(image),'-noappend','-processors','1','-quiet'], check=True, capture_output=True, timeout=30)
    return [deb,image]
def deb_records(root, rows):
    # A normal ar/deb with a real tar member table can express duplicate and
    # conflicting entries that no on-disk staging tree can represent.
    work=root/'deb-members'; work.mkdir(); (work/'debian-binary').write_bytes(b'2.0\n')
    with tarfile.open(work/'control.tar.gz','w:gz') as archive:
        item=tarfile.TarInfo('control'); item.size=len(CONTROL); archive.addfile(item,io.BytesIO(CONTROL))
    with tarfile.open(work/'data.tar','w:') as archive:
        for name,kind,value in rows:
            item=tarfile.TarInfo(name); item.mode=0o755
            if kind=='file': item.size=len(value); archive.addfile(item,io.BytesIO(value))
            else:
                item.type=tarfile.LNKTYPE if kind=='hardlink' else tarfile.SYMTYPE; item.linkname=value; archive.addfile(item)
    deb=root/'fixture.deb'
    subprocess.run(['ar','rc',str(deb),'debian-binary','control.tar.gz','data.tar'], cwd=work, check=True, capture_output=True, timeout=30)
    return deb
BASE=[('usr/bin/polaris','file',APP),(HELPER_PATH,'file',HELPER)]
class LinuxNamespace(unittest.TestCase):
    def test_real_case_distinct_icons_ancestors_and_exact_executable_links(self):
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            root=Path(tmp); source=stage(root/'stage',app='Polaris')
            (source/'usr/bin/polaris').symlink_to('Polaris')
            (source/'Polaris.png').write_bytes(b'icon')
            (source/'.DirIcon').symlink_to('Polaris.png')
            (source/'icons').mkdir(); (source/'icons/polaris.png').write_bytes(b'icon')
            (source/'polaris.png').symlink_to('icons/polaris.png')
            (source/'Share').write_bytes(b'case-distinct file')
            (source/'share').mkdir(); (source/'share/config').write_bytes(b'case-distinct ancestor')
            for asset in containers(root,source):
                with self.subTest(container=asset.suffix):
                    result=package.reject_local_selection(asset)
                    self.assertEqual(result['artifactSha256'],hashlib.sha256(asset.read_bytes()).hexdigest())
                    self.assertEqual({row['member']:row['sha256'] for row in result['checkedExecutables']}, {
                        'usr/bin/Polaris':hashlib.sha256(APP).hexdigest(),
                        'usr/bin/polaris':hashlib.sha256(APP).hexdigest(),
                        HELPER_PATH:hashlib.sha256(HELPER).hexdigest()})
                    self.assertEqual(result['expandedExecutableBytes'],len(APP)*2+len(HELPER))
    def test_case_mismatched_executable_target_is_absent_in_both_linux_formats(self):
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            root=Path(tmp); source=stage(root/'stage',app='Polaris')
            (source/'usr/bin/polaris').symlink_to('POLARIS')
            for asset in containers(root,source):
                with self.subTest(container=asset.suffix),self.assertRaisesRegex(ValueError,'link target absent'):
                    package.reject_local_selection(asset)
    def test_executable_link_escape_and_cycles_remain_rejected(self):
        for fault in ['escape','cycle']:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory(dir=scratch) as tmp:
                root=Path(tmp); source=stage(root/'stage'); (source/'usr/bin/polaris').unlink()
                (source/'usr/bin/polaris').symlink_to('../../../outside' if fault=='escape' else 'Polaris')
                if fault=='cycle': (source/'usr/bin/Polaris').symlink_to('polaris')
                for asset in containers(root,source):
                    with self.subTest(container=asset.suffix),self.assertRaisesRegex(ValueError,'link escapes archive' if fault=='escape' else 'link cycle'):
                        package.reject_local_selection(asset)
    def test_tar_exact_duplicates_and_file_directory_ancestors_remain_rejected(self):
        for name,extra,message in [
            ('duplicate',('usr/bin/polaris','file',b'other'),'duplicate/member count'),
            ('ancestor',('usr/bin/polaris/child','file',b'child'),'file/directory alias collision')]:
            with self.subTest(fault=name),tempfile.TemporaryDirectory(dir=scratch) as tmp:
                asset=deb_records(Path(tmp),BASE+[extra])
                with self.assertRaisesRegex(ValueError,message): package.reject_local_selection(asset)
    def test_tar_hardlinks_keep_exact_targets_escape_and_cycle_controls(self):
        for target in ['usr/bin/Polaris','usr/bin/POLARIS','USR/bin/Polaris','../../../outside',r'usr\bin\Polaris',str(Path.cwd()/'usr/bin/Polaris')]:
            with self.subTest(target=target),tempfile.TemporaryDirectory(dir=scratch) as tmp:
                asset=deb_records(Path(tmp),[('usr/bin/Polaris','file',APP),(HELPER_PATH,'file',HELPER),('usr/bin/polaris','hardlink',target)])
                if target=='usr/bin/Polaris':
                    self.assertEqual(len(package.reject_local_selection(asset)['checkedExecutables']),3)
                else:
                    absent=target in ['usr/bin/POLARIS','USR/bin/Polaris']
                    with self.assertRaisesRegex(ValueError,'link target absent' if absent else 'link escapes archive'):
                        package.reject_local_selection(asset)
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            asset=deb_records(Path(tmp),[(HELPER_PATH,'file',HELPER),('usr/bin/polaris','hardlink','usr/bin/Polaris'),('usr/bin/Polaris','hardlink','usr/bin/polaris')])
            with self.assertRaisesRegex(ValueError,'link cycle'): package.reject_local_selection(asset)
    def test_linux_retains_every_single_name_safety_constraint(self):
        for name in ['../outside','usr//bad','bad:name',r'bad\name','bad\x01name','trailing.','CON','e\u0301']:
            with self.subTest(name=repr(name)),tempfile.TemporaryDirectory(dir=scratch) as tmp:
                asset=deb_records(Path(tmp),BASE+[(name,'file',b'inert')])
                with self.assertRaises(ValueError): package.reject_local_selection(asset)
    def test_local_marker_across_64k_boundary_rejected_in_both_formats(self):
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            root=Path(tmp); source=stage(root/'stage',body=b'x'*65530+MARKER+b'end')
            for asset in containers(root,source):
                with self.subTest(container=asset.suffix),self.assertRaisesRegex(ValueError,'LocalDevelopmentSelection executable'):
                    package.reject_local_selection(asset)
    def test_windows_zip_case_alias_remains_rejected_on_this_linux_host(self):
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            asset=Path(tmp)/'windows.zip'
            with zipfile.ZipFile(asset,'w') as archive:
                for name,body in [('Polaris.exe',APP),('polaris.exe',APP),('polaris-helper.exe',HELPER)]: archive.writestr(name,body)
            with self.assertRaisesRegex(ValueError,'duplicate/alias member'): package.reject_local_selection(asset)
for tool in ['dpkg-deb','mksquashfs','unsquashfs','ar']: assert shutil.which(tool), tool+' is required'
unittest.main(argv=['linux-container-namespace'],verbosity=2)
`, fileURLToPath(new URL('./final-package-receipt.py', import.meta.url)), root], { encoding: 'utf8', timeout: 180_000 });
  t.diagnostic(result.stdout + result.stderr);
  assert.equal(result.status, 0, result.stdout + result.stderr);
}));
