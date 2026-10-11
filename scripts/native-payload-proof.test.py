import importlib.util
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import shutil
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
    @staticmethod
    def inert_app(token=final.BUNDLE_UNK):
        data = bytearray(1024); data[:2] = b'MZ'; struct.pack_into('<I', data, 0x3c, 0x80)
        data[0x80:0x84] = b'PE\0\0'
        struct.pack_into('<HHIIIHH', data, 0x84, 0x8664, 1, 0, 0, 0, 240, 0x22)
        struct.pack_into('<H', data, 0x98, 0x20b); struct.pack_into('<I', data, 0x98 + 108, 16)
        data[512:512+len(token)] = token
        return bytes(data)

    def test_exact_nsis_bundle_transform_and_zip_identity(self):
        source = self.inert_app(); expected = final.expected_windows_app(source, 'nsis', '2.12.1')
        self.assertEqual(len(source), len(expected))
        self.assertEqual(expected, source[:512] + final.BUNDLE_NSS + source[512+len(final.BUNDLE_UNK):])
        self.assertEqual(final.expected_windows_app(source, 'zip', '2.12.1'), source)
        self.assertEqual(final.byte_differences(source, expected), [{'offset': 536, 'endExclusive': 539, 'beforeHex': '554e4b', 'afterHex': '4e5353'}])
        for data in [self.inert_app(b''), self.inert_app(final.BUNDLE_NSS), self.inert_app(final.BUNDLE_PREFIX+b'MSI'),
                     source+final.BUNDLE_UNK]:
            with self.subTest(source=data[-25:]), self.assertRaisesRegex(ValueError, 'unique unpatched'):
                final.expected_windows_app(data, 'nsis', '2.12.1')
        for kind in ['nsis','zip']:
            with self.assertRaisesRegex(ValueError, 'unreviewed Tauri CLI'):
                final.expected_windows_app(source, kind, '2.11.4')

    def test_existing_other_bundle_tokens_survive_and_each_byte_change_is_rejected(self):
        # Runtime comparison strings are not additional patch targets. Include
        # NSS/MSI and opaque same-prefix bytes before and after the unique UNK.
        source=bytearray(self.inert_app())
        other=[(400,final.BUNDLE_NSS),(600,final.BUNDLE_PREFIX+b'MSI'),(700,final.BUNDLE_PREFIX+b'BAD')]
        for offset,token in other: source[offset:offset+len(token)]=token
        source=bytes(source)
        nsis=source[:512]+b'__TAURI_BUNDLE_TYPE_VAR_NSS'+source[539:]
        self.assertEqual(final.expected_windows_app(source,'nsis','2.12.1'),nsis)
        self.assertEqual(final.expected_windows_app(source,'zip','2.12.1'),source)
        signature={'status':'NotSigned','signerThumbprint':None}
        with tempfile.TemporaryDirectory() as tmp, patch.object(final,'windows_signature',return_value=signature):
            root=Path(tmp);app=root/'polaris.exe'
            for kind,expected in [('nsis',nsis),('zip',source)]:
                app.write_bytes(expected)
                final.verify_windows_app(app,source,expected,root/kind,{'kind':kind,'source':{'Authenticode':signature}})
                for offset,token in other:
                    # Mutate every byte in each other constant, including its
                    # shared prefix. None of these offsets is exempted.
                    for position in range(offset,offset+len(token)):
                        changed=bytearray(expected);changed[position]^=1;app.write_bytes(changed)
                        evidence={'kind':kind,'source':{'Authenticode':signature}}
                        with self.subTest(kind=kind,position=position), self.assertRaisesRegex(ValueError,'exact carrier-stage'):
                            final.verify_windows_app(app,source,expected,root/kind,evidence)
                        self.assertEqual((root/kind/'actual.exe').read_bytes(),bytes(changed))

    def test_app_byte_gate_rejects_other_mutations_and_retains_complete_failure_bytes(self):
        source = self.inert_app(); expected = final.expected_windows_app(source, 'nsis', '2.12.1')
        signature = {'status':'NotSigned','signerThumbprint':None}
        with tempfile.TemporaryDirectory() as tmp, patch.object(final, 'windows_signature', return_value=signature):
            root=Path(tmp); app=root/'polaris.exe'
            for kind, wanted, actual in [
                ('nsis',expected,expected), ('zip',source,source),
                ('nsis',expected,source), ('nsis',expected,self.inert_app(final.BUNDLE_PREFIX+b'MSI')),
                ('nsis',expected,expected[:20]+b'X'+expected[21:]),
                ('nsis',expected,expected+b'x'), ('nsis',expected,expected[:-1]),
                ('zip',source,expected),
            ]:
                with self.subTest(kind=kind, actualSHA=final.hashlib.sha256(actual).hexdigest()):
                    directory=root/('case'+str(len(list(root.iterdir())))); directory.mkdir(); app.write_bytes(actual)
                    evidence={'kind':kind,'source':{'Authenticode':signature}}
                    if actual==wanted:
                        result=final.verify_windows_app(app,source,wanted,directory,evidence)
                        self.assertEqual(result['expectedSha256'],result['sha256'])
                    else:
                        with self.assertRaisesRegex(ValueError,'exact carrier-stage'):
                            final.verify_windows_app(app,source,wanted,directory,evidence)
                    self.assertEqual((directory/'actual.exe').read_bytes(),actual)
                    saved=json.loads((directory/'app-byte-evidence.json').read_text(encoding='utf-8'))
                    self.assertEqual(saved['completeFileBytesEqualExpected'],actual==wanted)
                    self.assertEqual(saved['actual']['bytes'],len(actual))
                    self.assertEqual(saved['sourceToActualDifferences'],final.byte_differences(source,actual))
            app.write_bytes(expected)
            with patch.object(final,'windows_signature',return_value={'status':'Valid','signerThumbprint':'bad'}):
                with self.assertRaisesRegex(ValueError,'unsigned/no-certificate'):
                    final.verify_windows_app(app,source,expected,root/'signed',{'kind':'nsis','source':{'Authenticode':signature}})
            # Even a NotSigned observation cannot excuse a PE certificate table.
            certificate=bytearray(source); struct.pack_into('<II',certificate,0x98+144,512,8)
            certificate=bytes(certificate); app.write_bytes(certificate)
            with self.assertRaisesRegex(ValueError,'unsigned/no-certificate'):
                final.verify_windows_app(app,certificate,certificate,root/'certificate',{'kind':'zip','source':{'Authenticode':signature}})

    def test_app_baseline_binds_runtime_version_generated_nsis_input_and_portable_member(self):
        source=self.inert_app()+final.BUNDLE_NSS+final.BUNDLE_PREFIX+b'MSI'; signature={'status':'NotSigned','signerThumbprint':None}
        with tempfile.TemporaryDirectory() as tmp, patch.object(final,'windows_signature',return_value=signature), \
                patch.object(final,'pwsh',return_value=b'tauri-cli 2.12.1\n') as cli:
            root=Path(tmp); (root/'target/release/nsis/x64').mkdir(parents=True)
            (root/'scripts').mkdir(); (root/'scripts/tauri-cli.version').write_text('2.12.1\n')
            app=root/'target/release/polaris.exe'; app.write_bytes(source)
            script=root/'target/release/nsis/x64/installer.nsi'
            script.write_text(f'!define MAINBINARYSRCPATH "{app}"\n  File "${{MAINBINARYSRCPATH}}"\n',encoding='utf-8-sig')
            (root/'dist-win').mkdir(); archive=root/'dist-win/Polaris-test-Portable.zip'
            with zipfile.ZipFile(archive,'w') as z: z.writestr('polaris.exe',source)
            evidence={}; directory=root/'evidence'
            observed, expected=final.windows_app_baseline(root,'nsis',directory,evidence)
            self.assertEqual(observed,source); self.assertEqual(expected,final.expected_windows_app(source,'nsis','2.12.1'))
            self.assertEqual((directory/'source.exe').read_bytes(),source)
            self.assertEqual((directory/'portable.exe').read_bytes(),source)
            self.assertEqual((directory/'installer.nsi').read_bytes(),script.read_bytes())
            self.assertEqual(evidence['generatedNSISMainFileInput']['inputPath'],str(app))
            for runtime in [b'tauri-cli 2.11.4\n',b'tauri-cli 2.12.1\nextra']:
                cli.return_value=runtime
                with self.assertRaisesRegex(ValueError,'actual Tauri CLI'):
                    final.windows_app_baseline(root,'nsis',directory,{})
            cli.return_value=b'tauri-cli 2.12.1\n'
            (root/'scripts/tauri-cli.version').write_text('2.11.4\n')
            with self.assertRaisesRegex(ValueError,'unreviewed Tauri CLI'):
                final.windows_app_baseline(root,'nsis',directory,{})
            (root/'scripts/tauri-cli.version').write_text('2.12.1\n')
            script.write_text('!define MAINBINARYSRCPATH "different.exe"\n  File "${MAINBINARYSRCPATH}"\n')
            with self.assertRaises(ValueError): final.windows_app_baseline(root,'nsis',directory,{})

    def test_final_nsis_actual_failure_retains_bytes_before_temporary_cleanup_and_blocks_native(self):
        source=self.inert_app(); expected=final.expected_windows_app(source,'nsis','2.12.1')
        signature={'status':'NotSigned','signerThumbprint':None}
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); (root/'target/release/nsis/x64').mkdir(parents=True); (root/'scripts').mkdir()
            (root/'scripts/tauri-cli.version').write_text('2.12.1\n')
            app=root/'target/release/polaris.exe'; app.write_bytes(source)
            (root/'target/release/nsis/x64/installer.nsi').write_text(
                f'!define MAINBINARYSRCPATH "{app}"\n  File "${{MAINBINARYSRCPATH}}"\n',encoding='utf-8-sig')
            (root/'dist-win').mkdir()
            with zipfile.ZipFile(root/'dist-win/Polaris-test-Portable.zip','w') as z: z.writestr('polaris.exe',source)
            artifact=root/'dist-win/inert-setup.exe'; artifact.write_bytes(b'not executed')
            output=root/'native-payload-evidence/final-nsis.json'; destinations=[]
            def install_fixture(artifact,destination,root):
                destinations.append(destination); destination.mkdir()
                (destination/'polaris.exe').write_bytes(expected+b'unrelated mutation')
                (destination/'sing-box.exe').write_bytes(b'not executed')
                (destination/'libcronet.dll').write_bytes(b'not loaded')
            with patch.object(final,'ci_context',return_value={'candidate':'c'*40,'runId':'900','attempt':'1'}), \
                    patch.object(final,'windows_signature',return_value=signature), \
                    patch.object(final,'pwsh',return_value=b'tauri-cli 2.12.1\n'), \
                    patch.object(final,'nsis_install',side_effect=install_fixture) as installer, \
                    patch.object(final,'prove',side_effect=AssertionError('native proof must not run')) as native_proof:
                with self.assertRaisesRegex(ValueError,'exact carrier-stage'):
                    final.final_proof(root,'win',artifact,'nsis',output)
                installer.assert_called_once(); native_proof.assert_not_called()
            self.assertFalse(destinations[0].exists()); self.assertFalse(output.exists())
            directory=output.parent/'app-bytes/nsis'
            self.assertEqual((directory/'actual.exe').read_bytes(),expected+b'unrelated mutation')
            self.assertEqual((directory/'source.exe').read_bytes(),source)
            self.assertEqual((directory/'expected.exe').read_bytes(),expected)
            self.assertEqual((directory/'portable.exe').read_bytes(),source)
            evidence=json.loads((directory/'app-byte-evidence.json').read_text(encoding='utf-8'))
            self.assertFalse(evidence['completeFileBytesEqualExpected'])
            self.assertIn('exact carrier-stage',evidence['failure']['message'])

    def test_windows_failure_upload_keeps_exact_evidence_without_new_installation(self):
        workflow=(ROOT/'.github/workflows/package.yml').read_text(encoding='utf-8')
        block=workflow.split('- name: Preserve Windows App stage bytes and failure evidence')[1].split('# ── 上传产物')[0]
        self.assertIn("!cancelled() && inputs.native_validation == true && runner.os == 'Windows'",block)
        for path in ['native-payload-evidence/app-bytes/**','target/release/polaris.exe',
                     'target/release/nsis/x64/installer.nsi','dist-win/*.exe','dist-win/*-Portable.zip']:
            self.assertIn(path,block)
        self.assertNotIn('run:',block)

    def test_nsis_policy_reads_utf8_with_windows_legacy_default_before_native_actions(self):
        raw = (ROOT / 'src-tauri/tauri.conf.json').read_bytes()
        # The actual repository config contains Chinese text that CP1252 rejects.
        with self.assertRaises(UnicodeDecodeError): raw.decode('cp1252')
        original_read_text = Path.read_text

        def windows_default_read_text(path, encoding=None, errors=None, **kwargs):
            # Simulate the Windows default at the public text-read boundary.
            # Path.read_text may resolve None before calling Path.open.
            effective = 'cp1252' if encoding is None else encoding
            return original_read_text(path, encoding=effective, errors=errors, **kwargs)

        class GuardReached(Exception): pass

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); (root / 'src-tauri').mkdir()
            config = root / 'src-tauri/tauri.conf.json'
            config.write_bytes(raw)
            with patch.object(Path, 'read_text', windows_default_read_text), \
                    patch.object(final, 'ci_context', return_value={}), \
                    patch.object(final, 'pwsh', side_effect=GuardReached) as guard, \
                    patch.object(final.subprocess, 'run', side_effect=AssertionError('installer must not run')) as installer:
                with self.assertRaises(GuardReached):
                    final.nsis_install(root / 'inert.exe', root / 'payload', root)
                guard.assert_called_once(); installer.assert_not_called()
                # Explicit UTF-8 must retain both reviewed install/signature policy checks.
                for field, value in [('installMode', 'perMachine'), ('certificateThumbprint', 'unreviewed')]:
                    bad = json.loads(raw.decode('utf-8'))
                    if field == 'installMode': bad['bundle']['windows']['nsis'][field] = value
                    else: bad['bundle']['windows'][field] = value
                    config.write_bytes(json.dumps(bad, ensure_ascii=False).encode('utf-8'))
                    guard.reset_mock()
                    with self.assertRaisesRegex(ValueError, 'install/signature policy differs'):
                        final.nsis_install(root / 'inert.exe', root / 'payload', root)
                    guard.assert_not_called(); installer.assert_not_called()
                # UTF-8 remains strict: invalid input must stop before any native action.
                config.write_bytes(raw + b'\xff')
                guard.reset_mock()
                with self.assertRaises(UnicodeDecodeError):
                    final.nsis_install(root / 'inert.exe', root / 'payload', root)
                guard.assert_not_called(); installer.assert_not_called()

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
                [('./polaris.exe', b'first'), ('polaris.exe', b'second')],
                [('resources//win/sing-box.exe', b'first'), ('resources/win/sing-box.exe', b'second')],
                [('polaris.exe.', b'first'), ('polaris.exe', b'second')],
                [('polaris.exe ', b'first'), ('polaris.exe', b'second')],
                [('resources./win/sing-box.exe', b'first')], [('resources /win/sing-box.exe', b'first')],
                [('resources', b'file'), ('resources/win/libcronet.dll', b'library')],
                [('CON.exe', b'device')], [('x/NUL', b'device')],
                [('e\u0301.dll', b'non-NFC')],
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

    def test_windows_zip_normalized_keys_and_directory_aliases(self):
        self.assertEqual(final.windows_zip_key('Resources/Win/CORE.exe', False), 'resources/win/core.exe')
        self.assertEqual(final.windows_zip_key('resources/win/', True), 'resources/win')
        for name in ['./polaris.exe', 'resources//win/core.exe', 'polaris.exe.', 'polaris.exe ',
                     'resources /core.exe', 'file:stream', 'CON.exe', 'LPT1.txt', 'COM¹.exe', 'a/../b']:
            with self.subTest(name=name), self.assertRaises(ValueError): final.windows_zip_key(name, False)

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
