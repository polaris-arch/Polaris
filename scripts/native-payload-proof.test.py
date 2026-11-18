import importlib.util
import os
from pathlib import Path
import struct
import subprocess
import sys
import shutil
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / filename)
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module); return module


native = load('native', 'native-payload-proof.py')
windows = load('windows_integrity', 'windows-package-integrity.py')


class NativeContract(unittest.TestCase):
    @staticmethod
    def inert_app(token=windows.BUNDLE_UNK):
        data = bytearray(1024); data[:2] = b'MZ'; struct.pack_into('<I', data, 0x3c, 0x80)
        data[0x80:0x84] = b'PE\0\0'
        struct.pack_into('<HHIIIHH', data, 0x84, 0x8664, 1, 0, 0, 0, 240, 0x22)
        struct.pack_into('<H', data, 0x98, 0x20b); struct.pack_into('<I', data, 0x98 + 108, 16)
        data[512:512+len(token)] = token
        return bytes(data)

    def test_exact_nsis_bundle_transform_and_zip_identity(self):
        source = self.inert_app(); expected = windows.expected_windows_app(source, 'nsis', '2.12.5')
        self.assertEqual(len(source), len(expected))
        self.assertEqual(expected, source[:512] + windows.BUNDLE_NSS + source[512+len(windows.BUNDLE_UNK):])
        self.assertEqual(windows.expected_windows_app(source, 'zip', '2.12.5'), source)
        for data in [self.inert_app(b''), self.inert_app(windows.BUNDLE_NSS), self.inert_app(windows.BUNDLE_PREFIX+b'MSI'),
                     source+windows.BUNDLE_UNK]:
            with self.subTest(source=data[-25:]), self.assertRaisesRegex(ValueError, 'unique unpatched'):
                windows.expected_windows_app(data, 'nsis', '2.12.5')
        for kind in ['nsis','zip']:
            with self.assertRaisesRegex(ValueError, 'unreviewed Tauri CLI'):
                windows.expected_windows_app(source, kind, '2.11.4')

    def test_signature_uses_fixed_file_and_literal_path_argv(self):
        path = r'C:\inert payload with spaces\unsigned payload.exe'
        with patch.object(native.subprocess, 'check_output', return_value=b'{"status":"NotSigned","signerThumbprint":null}') as call:
            self.assertEqual(native.observe_unsigned_windows(path)['status'], 'NotSigned')
            args = call.call_args.args[0]
            self.assertEqual(args, ['pwsh', '-NoProfile', '-NonInteractive', '-File',
                str(ROOT / 'scripts/observe-unsigned-windows.ps1'), '-Path', path])
            self.assertNotIn('-Command', args)
        script = (ROOT / 'scripts/observe-unsigned-windows.ps1').read_text()
        self.assertIn('[string]$Path', script)
        self.assertIn('Get-AuthenticodeSignature -LiteralPath $Path', script)
        self.assertNotIn('$args', script)

    @unittest.skipUnless(os.name == 'nt' and shutil.which('pwsh'), 'native PowerShell inert PE path proof requires Windows')
    def test_native_powershell_reads_inert_unsigned_pe_with_spaces(self):
        # A valid PE header with no entry point/code. Never execute or load it.
        data = bytearray(1024); data[:2] = b'MZ'; struct.pack_into('<I', data, 0x3c, 0x80)
        data[0x80:0x84] = b'PE\0\0'
        struct.pack_into('<HHIIIHH', data, 0x84, 0x8664, 1, 0, 0, 0, 240, 0x22)
        opt = 0x98; struct.pack_into('<H', data, opt, 0x20b)
        struct.pack_into('<Q', data, opt + 24, 0x140000000)
        struct.pack_into('<II', data, opt + 32, 4096, 512)
        struct.pack_into('<II', data, opt + 56, 8192, 512)
        struct.pack_into('<HH', data, opt + 68, 3, 0x140)
        struct.pack_into('<I', data, opt + 108, 16)
        sec = opt + 240; data[sec:sec + 8] = b'.text\0\0\0'
        struct.pack_into('<IIII', data, sec + 8, 1, 4096, 512, 512)
        struct.pack_into('<I', data, sec + 36, 0x60000020)
        with tempfile.TemporaryDirectory(prefix='polaris inert PE with spaces ') as tmp:
            path = Path(tmp) / 'unsigned inert payload.exe'; path.write_bytes(data)
            before = native.sha(path)
            observed = native.observe_unsigned_windows(path)
            self.assertEqual(observed, {'status': 'NotSigned', 'signerThumbprint': None})
            self.assertEqual(native.sha(path), before)

    def test_context_rejects_wrong_platform_or_host(self):
        for key, system, arch in [('win', 'Linux', 'amd64'), ('linux', 'Linux', 'arm64'), ('mac-x64', 'Darwin', 'x86_64')]:
            with self.subTest(key=key, system=system, arch=arch), \
                    self.assertRaisesRegex(ValueError, 'native amd64 host/platform differs'):
                native.ci_context(key, {}, system, arch)

    def test_no_kernel_override_blocks_before_any_native_action(self):
        with patch.dict(os.environ, {'POLARIS_NO_KERNEL_RUN': '1'}, clear=True), \
                patch.object(native.ctypes, 'CDLL', side_effect=AssertionError('loader must not run')), \
                patch.object(native.subprocess, 'Popen', side_effect=AssertionError('core must not run')):
            with self.assertRaises(ValueError): native.capi_smoke('missing', 'linux', 'a' * 64)
            with self.assertRaises(ValueError): native.core_probe('missing', 'missing', 'linux', 'a' * 64)
        args = ['--platform', 'linux', '--library', '/missing', '--capi-only', '--expected-sha256', 'a' * 64]
        run = subprocess.run([sys.executable, str(ROOT / 'scripts/native-payload-proof.py'), *args],
                             capture_output=True, env={**os.environ, 'POLARIS_NO_KERNEL_RUN': '1'}, timeout=10)
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

    def test_windows_zip_normalized_keys_and_directory_aliases(self):
        self.assertEqual(windows.key('Resources/Win/CORE.exe', False), 'resources/win/core.exe')
        self.assertEqual(windows.key('resources/win/', True), 'resources/win')
        for name in ['./polaris.exe', 'resources//win/core.exe', 'polaris.exe.', 'polaris.exe ',
                     'resources /core.exe', 'file:stream', 'CON.exe', 'LPT1.txt', 'COM¹.exe', 'a/../b']:
            with self.subTest(name=name), self.assertRaises(ValueError): windows.key(name, False)

if __name__ == '__main__':
    unittest.main(verbosity=2)
