import importlib.util
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / filename)
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module); return module


native = load('native', 'native-payload-proof.py')
final = load('final', 'final-package-receipt.py')
download = load('download', 'desktop-core/validation-download.py')
ENV = {'POLARIS_NATIVE_PAYLOAD_VALIDATION': '1', 'GITHUB_ACTIONS': 'true', 'GITHUB_EVENT_NAME': 'workflow_dispatch',
       'GITHUB_REPOSITORY': 'polaris-arch/Polaris', 'GITHUB_SHA': 'c' * 40, 'GITHUB_RUN_ID': '900', 'GITHUB_RUN_ATTEMPT': '1',
       'GITHUB_REF': 'refs/heads/collab/fk02-native-payload-validation-20261010',
       'GITHUB_WORKFLOW_REF': 'polaris-arch/Polaris/.github/workflows/release-risk.yml@refs/heads/collab/fk02-native-payload-validation-20261010'}


class NativeContract(unittest.TestCase):
    def test_exact_native_ci_contract(self):
        for key, system in [('linux', 'Linux'), ('win', 'Windows')]:
            self.assertEqual(native.ci_context(key, ENV, system, 'AMD64')['candidate'], 'c' * 40)

    def test_context_rejects_each_scope_drift(self):
        for name in ENV:
            with self.subTest(name=name):
                with self.assertRaises(ValueError): native.ci_context('linux', {**ENV, name: ''}, 'Linux', 'x86_64')
        for change in [{'POLARIS_NO_KERNEL_RUN': '1'}, {'GITHUB_REF': 'refs/tags/v1'}, {'GITHUB_EVENT_NAME': 'push'}]:
            with self.assertRaises(ValueError): native.ci_context('linux', {**ENV, **change}, 'Linux', 'x86_64')
        for key, system, arch in [('win', 'Linux', 'amd64'), ('linux', 'Linux', 'arm64'), ('mac-x64', 'Darwin', 'x86_64')]:
            with self.assertRaises(ValueError): native.ci_context(key, ENV, system, arch)

    def test_no_kernel_override_blocks_before_any_native_action(self):
        with patch.dict(os.environ, {**ENV, 'POLARIS_NO_KERNEL_RUN': '1'}, clear=True), \
                patch.object(native.ctypes, 'CDLL', side_effect=AssertionError('loader must not run')), \
                patch.object(native.subprocess, 'Popen', side_effect=AssertionError('core must not run')):
            with self.assertRaises(ValueError): native.capi_smoke('missing', 'linux', 'a' * 64)
            with self.assertRaises(ValueError): native.core_probe('missing', 'missing', 'linux', 'a' * 64)
        for filename, args in [
            ('native-payload-proof.py', ['--platform', 'linux', '--library', '/missing', '--capi-only', '--expected-sha256', 'a' * 64]),
            ('final-package-receipt.py', ['--platform', 'linux', '--kind', 'deb', '--artifact', '/missing', '--output', '/missing']),
        ]:
            run = subprocess.run([sys.executable, str(ROOT / 'scripts' / filename), *args], capture_output=True,
                                 env={**os.environ, 'POLARIS_NO_KERNEL_RUN': '1'}, timeout=10)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn(b'exact non-tag manual CI', run.stderr)

    def test_linux_maps_reads_actual_paths_deduplicates_and_decodes(self):
        text = 'a-b r-xp 0 00:01 1 /tmp/with\\040space/libcronet.so\n' \
               'b-c rw-p 0 00:01 1 /tmp/with\\040space/libcronet.so\n' \
               'd-e r-xp 0 00:01 2 /lib/libc.so.6\n'
        self.assertEqual(native.linux_maps(text), ['/tmp/with space/libcronet.so'])
        with self.assertRaises(ValueError): native.linux_maps('a-b r-xp 0 00:01 1 /tmp/libcronet.so (deleted)')

    def test_mapped_library_binds_path_and_complete_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            lib = Path(tmp) / 'libcronet.so'; other = Path(tmp) / 'other.so'
            lib.write_bytes(b'canonical'); other.write_bytes(b'canonical')
            h = native.sha(lib)
            self.assertEqual(native.match_mapped([str(lib)], lib, h)['sha256'], h)
            for paths in [[], [str(lib), str(other)], [str(other)]]:
                with self.assertRaises(ValueError): native.match_mapped(paths, lib, h)
            lib.write_bytes(b'otherdata')
            with self.assertRaises(ValueError): native.match_mapped([str(lib)], lib, h)

    def test_final_payload_rejects_wrong_platform_duplicates_split_and_links(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); core = root / 'sing-box'; lib = root / 'libcronet.so'
            core.write_bytes(b'core'); lib.write_bytes(b'library')
            self.assertEqual(final.payload_files(root, 'linux'), (core, lib))
            with self.assertRaises(ValueError): final.payload_files(root, 'win')
            second = root / 'nested'; second.mkdir(); (second / 'sing-box').write_bytes(b'core')
            with self.assertRaises(ValueError): final.payload_files(root, 'linux')
            (second / 'sing-box').unlink(); lib.rename(second / lib.name)
            with self.assertRaises(ValueError): final.payload_files(root, 'linux')
            (second / lib.name).rename(lib); core.unlink(); core.symlink_to(lib)
            with self.assertRaises(ValueError): final.payload_files(root, 'linux')

    def test_zip_inventory_rejects_traversal_links_and_case_alias(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for index, members in enumerate([
                [('..//escaped', b'x')], [('C:/absolute', b'x')], [('x\\bad', b'x')],
                [('polaris.exe', b'x'), ('POLARIS.EXE', b'y')], [('link', b'target', stat.S_IFLNK | 0o777)],
            ]):
                archive = root / f'{index}.zip'; dest = root / f'dest{index}'; dest.mkdir()
                with zipfile.ZipFile(archive, 'w') as z:
                    for name, content, *mode in members:
                        info = zipfile.ZipInfo(name)
                        if mode: info.external_attr = mode[0] << 16
                        z.writestr(info, content)
                with self.assertRaises(ValueError): final.extract_zip(archive, dest)
                self.assertEqual(list(dest.iterdir()), [])

    def test_final_zip_opens_members_and_requires_marker_and_licenses(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); archive = root / 'portable.zip'; dest = root / 'dest'; dest.mkdir()
            with zipfile.ZipFile(archive, 'w') as z:
                for name in ['polaris.exe', 'portable.marker', 'LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md']:
                    z.writestr(name, b'present')
                z.writestr('resources/win/sing-box.exe', b'core'); z.writestr('resources/win/libcronet.dll', b'library')
            final.extract_zip(archive, dest)
            self.assertEqual(final.payload_files(dest, 'win')[0].read_bytes(), b'core')

    def test_squashfs_static_offset_rejects_magic_decoy_and_ambiguity(self):
        block = bytearray(96)
        block[:4] = b'hsqs'; struct.pack_into('<I', block, 4, 1); struct.pack_into('<I', block, 12, 131072)
        struct.pack_into('<HH', block, 28, 4, 0); struct.pack_into('<Q', block, 40, 96)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'final.AppImage'
            path.write_bytes(b'ELFhsqsdecoy' + block)
            self.assertEqual(final.squashfs_offset(path), 12)
            path.write_bytes(block + block)
            with self.assertRaises(ValueError): final.squashfs_offset(path)
            path.write_bytes(b'hsqsdecoy')
            with self.assertRaises(ValueError): final.squashfs_offset(path)

    def test_origin_zip_requires_exact_two_original_members_and_never_overwrites(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); archive = root / 'origin.zip'; destination = root / 'dest'; destination.mkdir()
            with zipfile.ZipFile(archive, 'w') as z:
                z.writestr('linux/sing-box', b'core'); z.writestr('linux/sing-box.source-receipt.json', b'original receipt bytes')
            download.extract_archive(archive, destination, 'linux')
            self.assertEqual((destination / 'linux/sing-box.source-receipt.json').read_bytes(), b'original receipt bytes')
            with self.assertRaises(ValueError): download.extract_archive(archive, destination, 'linux')
            with zipfile.ZipFile(archive, 'a') as z: z.writestr('extra', b'x')
            with self.assertRaises(ValueError): download.extract_archive(archive, root / 'other', 'linux')


if __name__ == '__main__':
    unittest.main(verbosity=2)
