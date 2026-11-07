import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import apple_tls_fingerprint as subject


class OptionalTLSCapabilityLink(unittest.TestCase):
    def test_old_core_and_matched_new_c_abi(self):
        self.assertFalse(subject.capability_present(['NSString* LibboxVersion(void);'], 'T _LibboxVersion\n'))
        header = 'NSString* _Nonnull LibboxTLSFingerprintCapabilities(NSError* _Nullable* _Nullable error);'
        self.assertTrue(subject.capability_present([header], '000000001 T ' + subject.SYMBOL + '\n'))
        for headers, symbols in [([header], ''), ([], 'T ' + subject.SYMBOL + '\n'),
                                 (['void LibboxTLSFingerprintCapabilities(void);'], 'T ' + subject.SYMBOL + '\n')]:
            with self.assertRaises(RuntimeError):
                subject.capability_present(headers, symbols)

    def test_comments_and_conflicting_real_declarations_cannot_attest_abi(self):
        good = 'NSString* _Nonnull LibboxTLSFingerprintCapabilities(NSError* _Nullable* _Nullable error);'
        bad = 'void LibboxTLSFingerprintCapabilities(void);'
        symbols = 'T ' + subject.SYMBOL + '\n'
        for header in [bad + '/* ' + good + ' */', bad + '// ' + good,
                       good + '\n' + bad, good + '\n' + good, good + good]:
            with self.assertRaises(RuntimeError):
                subject.capability_present([header], symbols)
        self.assertFalse(subject.capability_present(['/* ' + good + ' */'], ''))
        self.assertTrue(subject.capability_present(['// irrelevant\n' + good], symbols))

    def test_final_definition_without_dynamic_export_is_not_accepted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            executable = root / 'extension'
            executable.write_bytes(b'fixture native bytes')
            flags = root / 'PolarisTLSFingerprintLinker.args'
            flags.write_text('-Xlinker\n-u\n-Xlinker\n' + subject.SYMBOL + '\n')
            env = {'DERIVED_FILE_DIR': tmp, 'TARGET_BUILD_DIR': tmp, 'EXECUTABLE_PATH': 'extension'}
            receipt = {'capabilityPresent': True, 'flags': subject.file_pin(flags)}
            with patch.dict(os.environ, env), patch.object(subject, 'read_receipt', return_value=receipt):
                with patch.object(subject, 'run', side_effect=['T ' + subject.SYMBOL + '\n', 'unrelated export']):
                    with self.assertRaisesRegex(RuntimeError, 'dynamically discoverable'):
                        subject.verify()
                with patch.object(subject, 'run', return_value=''):
                    with self.assertRaisesRegex(RuntimeError, 'stripped'):
                        subject.verify()


    @staticmethod
    def versioned_framework(root, platform, present):
        # Actual gomobile layout, inert bytes and mocked nm/dyld only: no SDK.
        identifier = 'ios-arm64' if platform == 'iphoneos' else 'ios-arm64-simulator'
        xcframework = root / 'Libbox.xcframework'
        framework = xcframework / identifier / 'Libbox.framework'
        version = framework / 'Versions/A'
        (version / 'Headers').mkdir(parents=True)
        binary = version / 'Libbox'
        binary.write_bytes(b'inert selected static archive')
        header = version / 'Headers/Libbox.h'
        header.write_text('NSString* _Nonnull LibboxTLSFingerprintCapabilities('
                          'NSError* _Nullable* _Nullable error);' if present else
                          'NSString* LibboxVersion(void);')
        (framework / 'Versions/Current').symlink_to('A', target_is_directory=True)
        (framework / 'Libbox').symlink_to('Versions/Current/Libbox')
        (framework / 'Headers').symlink_to('Versions/Current/Headers', target_is_directory=True)
        selected = {'libraryIdentifier': identifier, 'binary': subject.file_pin(binary)}
        context = root / 'link.json'
        context.write_text(json.dumps({'framework': str(xcframework),
            'componentOutput': {'slices': {identifier: selected}}}))
        executable = root / 'extension'
        executable.write_bytes(b'inert final PacketTunnel executable')
        env = {'POLARIS_MOBILE_LINK_INPUT': str(context),
               'POLARIS_MOBILE_LINK_INPUT_SHA256': subject.sha(context.read_bytes()),
               'PLATFORM_NAME': platform, 'ARCHS': 'arm64',
               'DERIVED_FILE_DIR': str(root / 'derived'),
               'TARGET_BUILD_DIR': str(root), 'EXECUTABLE_PATH': executable.name}
        return framework, binary, header, selected, context, env

    def test_versioned_selected_framework_prepare_then_verify_both_targets(self):
        for platform in ('iphoneos', 'iphonesimulator'):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                framework, binary, header, selected, _, env = self.versioned_framework(root, platform, True)
                symbols = '000000001 T ' + subject.SYMBOL + '\n'
                with patch.dict(os.environ, env), patch.object(subject, 'run',
                        side_effect=[symbols, symbols, subject.SYMBOL + '\n']) as native, \
                        patch('builtins.print') as printed:
                    subject.prepare()
                    flags, receipt_path = subject.paths()
                    receipt = json.loads(receipt_path.read_text())
                    self.assertTrue(receipt['capabilityPresent'])
                    self.assertEqual(receipt['binary'], selected['binary'])
                    self.assertEqual(receipt['headers'], {header.name: subject.file_pin(header)})
                    self.assertEqual(flags.read_text(), '-Xlinker\n-u\n-Xlinker\n' + subject.SYMBOL + '\n')
                    self.assertEqual(native.call_args_list[0].args[0][-1], binary.resolve(strict=True))
                    self.assertTrue((framework / 'Libbox').is_symlink())
                    subject.verify()
                    self.assertEqual(native.call_count, 3)
                    output = json.loads(printed.call_args.args[0])
                    self.assertTrue(output['capabilityPresent'])
                    self.assertFalse(output['loadedQueryExecuted'] or output['devicePassed'])

    def test_versioned_old_core_empty_flags_prepare_then_verify_both_targets(self):
        for platform in ('iphoneos', 'iphonesimulator'):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                _, _, _, selected, _, env = self.versioned_framework(root, platform, False)
                with patch.dict(os.environ, env), patch.object(subject, 'run',
                        return_value='T _LibboxVersion\n') as native, patch('builtins.print') as printed:
                    subject.prepare()
                    flags, receipt_path = subject.paths()
                    receipt = json.loads(receipt_path.read_text())
                    self.assertFalse(receipt['capabilityPresent'])
                    self.assertEqual(receipt['binary'], selected['binary'])
                    self.assertEqual(flags.read_bytes(), b'')
                    self.assertEqual(receipt['flags'], {'bytes': 0, 'sha256': subject.sha(b'')})
                    with self.assertRaisesRegex(RuntimeError, 'bounded regular artifact required'):
                        subject.file_pin(flags)  # Global default still refuses empty files.
                    subject.verify()
                    self.assertEqual(native.call_count, 1)  # No optional query/export invented.
                    self.assertFalse(json.loads(printed.call_args.args[0])['capabilityPresent'])

    def test_prepare_rejects_framework_escape_bad_chain_wrong_slice_and_changed_bytes(self):
        reasons = {'relative-escape': 'escaped selected framework', 'absolute-escape': 'escaped selected framework',
                   'bad-chain': 'input missing or unresolvable', 'loop': 'input missing or unresolvable',
                   'wrong-slice': 'selected libbox slice differs', 'changed-bytes': 'actual libbox link bytes differ',
                   'empty-binary': 'bounded regular artifact required', 'empty-header': 'bounded regular artifact required',
                   'header-escape': 'escaped selected framework', 'framework-redirect': 'framework root redirected/missing'}
        for case in ('relative-escape', 'absolute-escape', 'bad-chain', 'loop',
                     'wrong-slice', 'changed-bytes', 'empty-binary', 'empty-header',
                     'header-escape', 'framework-redirect'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                framework, binary, header, selected, context, env = self.versioned_framework(root, 'iphoneos', True)
                alias = framework / 'Libbox'
                if case in ('relative-escape', 'absolute-escape'):
                    outside = root / 'outside'
                    outside.write_bytes(binary.read_bytes())  # Matching bytes do not authorize an escape.
                    alias.unlink()
                    alias.symlink_to(os.path.relpath(outside, framework) if case == 'relative-escape' else outside)
                elif case == 'bad-chain':
                    (framework / 'Versions/Current').unlink()
                    (framework / 'Versions/Current').symlink_to('missing')
                elif case == 'loop':
                    alias.unlink(); alias.symlink_to('Libbox')
                elif case == 'wrong-slice':
                    link = json.loads(context.read_text())
                    link['componentOutput']['slices']['ios-arm64']['libraryIdentifier'] = 'ios-arm64-simulator'
                    other = framework.parent.parent / 'ios-arm64-simulator/Libbox.framework'
                    other.mkdir(parents=True); (other / 'Libbox').write_bytes(binary.read_bytes())
                    context.write_text(json.dumps(link))
                    env['POLARIS_MOBILE_LINK_INPUT_SHA256'] = subject.sha(context.read_bytes())
                elif case == 'changed-bytes':
                    binary.write_bytes(b'changed after selected pin')
                elif case == 'empty-binary':
                    binary.write_bytes(b'')
                elif case == 'empty-header':
                    header.write_bytes(b'')
                elif case == 'header-escape':
                    outside = root / 'outside-headers'; outside.mkdir()
                    (outside / header.name).write_bytes(header.read_bytes())
                    (framework / 'Headers').unlink()
                    (framework / 'Headers').symlink_to(outside, target_is_directory=True)
                else:
                    relocated = framework.with_name('relocated.framework')
                    framework.rename(relocated)
                    framework.symlink_to(relocated, target_is_directory=True)
                with patch.dict(os.environ, env), patch.object(subject, 'run', return_value='T ' + subject.SYMBOL + '\n'), \
                        self.assertRaisesRegex(RuntimeError, reasons[case]):
                    subject.prepare()

    def test_verify_still_rejects_changed_or_redirected_flags_and_empty_executable(self):
        for case in ('changed-flags', 'redirected-flags', 'empty-executable'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                _, _, _, _, _, env = self.versioned_framework(root, 'iphoneos', False)
                with patch.dict(os.environ, env), patch.object(subject, 'run', return_value='T _LibboxVersion\n'), \
                        patch('builtins.print'):
                    subject.prepare()
                    flags, _ = subject.paths()
                    if case == 'changed-flags':
                        flags.write_text('unreviewed linker flags')
                    elif case == 'redirected-flags':
                        other = root / 'empty-flags'; other.write_bytes(b'')
                        flags.unlink(); flags.symlink_to(other)
                    else:
                        (root / env['EXECUTABLE_PATH']).write_bytes(b'')
                    with self.assertRaisesRegex(RuntimeError, 'flags changed|bounded regular artifact required'):
                        subject.verify()

    def test_xcode_hooks_use_pinned_bytes_with_stale_reservation_fd_and_no_tools(self):
        for platform in ('iphoneos', 'iphonesimulator'):
            for present in (False, True):
                with self.subTest(platform=platform, present=present), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    _, _, _, _, _, env = self.versioned_framework(root, platform, present)
                    prepared = root / 'parent-prepared'
                    with patch.dict(os.environ, dict(env, DERIVED_FILE_DIR=str(prepared))), \
                            patch.object(subject, 'run', return_value='T ' + (subject.SYMBOL if present else '_LibboxVersion') + '\n'):
                        subject.prepare()
                    env.update(POLARIS_TLS_PREPARED_ROOT=str(prepared),
                               POLARIS_TLS_PREPARED_SHA256=subject.file_pin(prepared / 'PolarisTLSFingerprintLinker.json')['sha256'],
                               POLARIS_BUILD_RESERVATION_FD='999999')
                    with patch.dict(os.environ, env), \
                            patch.object(subject, 'run', side_effect=AssertionError('Xcode must not start native sessions')), \
                            patch('budget.register_group', side_effect=AssertionError('Xcode must not access inherited fd')):
                        subject.install()
                        subject.verify_installed()
                        flags, receipt = subject.paths()
                        self.assertEqual(flags.read_bytes(), (prepared / flags.name).read_bytes())
                        self.assertEqual(receipt.read_bytes(), (prepared / receipt.name).read_bytes())
                        flags.write_text('tampered installed flags')
                        with self.assertRaisesRegex(RuntimeError, 'installed TLS linker inputs changed'):
                            subject.verify_installed()

    def test_prepared_capability_receipt_flags_and_selected_bytes_cannot_drift(self):
        for case in ('receipt', 'flags', 'binary', 'header', 'link'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                _, binary, header, _, context, env = self.versioned_framework(root, 'iphoneos', True)
                prepared = root / 'prepared'
                with patch.dict(os.environ, dict(env, DERIVED_FILE_DIR=str(prepared))), \
                        patch.object(subject, 'run', return_value='T ' + subject.SYMBOL + '\n'):
                    subject.prepare()
                env.update(POLARIS_TLS_PREPARED_ROOT=str(prepared),
                           POLARIS_TLS_PREPARED_SHA256=subject.file_pin(prepared / 'PolarisTLSFingerprintLinker.json')['sha256'])
                changed = {'receipt': prepared / 'PolarisTLSFingerprintLinker.json',
                           'flags': prepared / 'PolarisTLSFingerprintLinker.args',
                           'binary': binary, 'header': header, 'link': context}[case]
                changed.write_bytes(changed.read_bytes() + b'changed')
                with patch.dict(os.environ, env), patch.object(subject, 'run') as native:
                    with self.assertRaises(RuntimeError):
                        subject.install()
                    native.assert_not_called()

    def test_native_actions_refuse_missing_or_closed_parent_reservation(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {'POLARIS_APP_WORK': str(Path(tmp) / 'work'),
                   'POLARIS_BUILD_BUDGET_ROOT': str(Path(tmp) / 'budget'),
                   'POLARIS_FORMAL_APP_BUILD': '1'}
            with patch.dict(os.environ, env, clear=True), patch.object(subject, 'prepare') as prepare:
                with self.assertRaisesRegex(RuntimeError, 'admitted parent reservation'):
                    subject.native_action('prepare')
                os.environ['POLARIS_BUILD_RESERVATION_FD'] = '999999'
                with self.assertRaises(OSError):
                    subject.native_action('prepare')
                prepare.assert_not_called()

    def test_native_action_keeps_real_parent_reservation_for_inert_child(self):
        import sys
        from budget import reservation
        from producer import ACTIVE_FDS, private_dir, run
        with tempfile.TemporaryDirectory() as tmp:
            root = private_dir(Path(tmp) / 'work')
            value = {'budget': {'buildPeakReserveBytes': 1}}
            env = {'POLARIS_APP_WORK': str(root), 'POLARIS_FORMAL_APP_BUILD': '1',
                   'POLARIS_BUILD_BUDGET_ROOT': str(Path(tmp) / 'budget')}
            with patch.dict(os.environ, env, clear=True), patch('fork_input.load', return_value=value), \
                    reservation(root, value, ACTIVE_FDS):
                original_fd = int(os.environ['POLARIS_BUILD_RESERVATION_FD'])
                def inert_prepare():
                    self.assertIn(original_fd, ACTIVE_FDS)
                    self.assertEqual(run([sys.executable, '-c', 'print("inert native-tool control")']).strip(),
                                     'inert native-tool control')
                with patch.object(subject, 'prepare', side_effect=inert_prepare):
                    subject.native_action('prepare')
                row = json.loads(os.pread(original_fd, 8192, 0))
                self.assertEqual(row['groups'], [])
                self.assertIn(original_fd, ACTIVE_FDS)

    def test_parent_runs_native_tls_checks_around_archive_before_carrier_acceptance(self):
        root = Path(__file__).resolve().parents[2]
        builder = (root / 'scripts/mobile-core/build_apple_app.py').read_text()
        before = builder.index("'original-tls-prepare.log'")
        archive = builder.index("timed_phase('app-extension-archive'")
        after = builder.index("'original-tls-final-verify.log'")
        accept = builder.index("timed_phase('final-carrier-inspection'")
        self.assertLess(before, archive)
        self.assertLess(archive, after)
        self.assertLess(after, accept)
        for name in ('project.yml', 'polaris.xcodeproj/project.pbxproj'):
            text = (root / 'src-tauri/gen/apple' / name).read_text()
            self.assertIn('apple_tls_fingerprint.py', text)
            self.assertIn(' verify-installed', text)
            self.assertIn(' install', text)
            self.assertNotIn(' prepare', text)



# Interface declarations match the fixed gomobile contract, including nullable
# direct return instead of ret0_. Inert fixtures never import or execute Libbox.
NATIVE_PLATFORM_METHODS = """
- (int32_t)runtimeTunVersion;
- (id<LibboxRuntimeTunSession> _Nullable)newTunSession:(id<LibboxTunOptions> _Nullable)options previous:(id<LibboxRuntimeTunSession> _Nullable)previous receipt:(LibboxRuntimeTunReceipt* _Nullable)receipt error:(NSError* _Nullable* _Nullable)error;
"""
NATIVE_SESSION_METHODS = """
- (NSString* _Nonnull)ownerID;
- (BOOL)prepare:(NSError* _Nullable* _Nullable)error;
- (int32_t)fileDescriptor;
- (BOOL)validate:(NSError* _Nullable* _Nullable)error;
- (BOOL)abort:(NSError* _Nullable* _Nullable)error;
- (BOOL)retire:(NSError* _Nullable* _Nullable)error;
"""
NATIVE_HEADER = ('@protocol LibboxRuntimeTunPlatform;\n'
    '@protocol LibboxRuntimeTunSession;\n'
    '@class LibboxRuntimeTunReceipt;\n'
    '@protocol LibboxRuntimeTunPlatform <NSObject>\n' + NATIVE_PLATFORM_METHODS + '@end\n'
    '@protocol LibboxRuntimeTunSession <NSObject>\n' + NATIVE_SESSION_METHODS + '@end\n'
    '@interface LibboxRuntimeTunPlatform : NSObject <goSeqRefInterface, LibboxRuntimeTunPlatform> {\n}\n' +
    NATIVE_PLATFORM_METHODS + '@end\n'
    '@interface LibboxRuntimeTunSession : NSObject <goSeqRefInterface, LibboxRuntimeTunSession> {\n}\n' +
    NATIVE_SESSION_METHODS + '@end\n'
    '@interface LibboxRuntimeTunReceipt : NSObject <goSeqRefInterface> {\n}\n'
    '- (NSString* _Nonnull)ownerID;\n- (BOOL)published;\n@end\n'
    '@interface LibboxCommandServer : NSObject <goSeqRefInterface> {\n}\n'
    '- (BOOL)setRuntimeTunPlatform:(id<LibboxRuntimeTunPlatform> _Nullable)platform error:(NSError* _Nullable* _Nullable)error;\n@end\n')


class OptionalNativeTunCompilerABI(unittest.TestCase):
    def test_old_complete_and_comment_only_headers(self):
        self.assertFalse(subject.native_tun_abi_present(['NSString* LibboxVersion(void);']))
        self.assertTrue(subject.native_tun_abi_present([NATIVE_HEADER]))
        self.assertTrue(subject.native_tun_abi_present(['#ifndef __Libbox_H__\n#define __Libbox_H__\n' +
                                                       NATIVE_HEADER + '#endif\n']))
        self.assertFalse(subject.native_tun_abi_present(['/*' + NATIVE_HEADER + '*/']))
        self.assertEqual(subject.compiler_args(True), b'-D\nPOLARIS_NATIVE_TUN_V1\n')
        self.assertEqual(subject.compiler_args(False), b'')

    def test_partial_conflicting_duplicate_disabled_and_wrong_return_fail_closed(self):
        bad_headers = [
            '@protocol LibboxRuntimeTunPlatform;',
            NATIVE_HEADER.replace('- (BOOL)published;', ''),
            NATIVE_HEADER.replace('newTunSession:', 'newTunSessionWrong:', 1),
            NATIVE_HEADER.replace('id<LibboxRuntimeTunSession> _Nullable)newTunSession:',
                                  'BOOL)newTunSession:', 1),
            NATIVE_HEADER.replace('- (int32_t)fileDescriptor;', '- (int64_t)fileDescriptor;', 1),
            NATIVE_HEADER.replace('- (BOOL)prepare:(NSError* _Nullable* _Nullable)error;',
                                  '- (BOOL)prepare:(NSError* _Nullable* _Nullable)error;\n'
                                  '- (void)prepare:(NSError* _Nullable* _Nullable)error;', 1),
            NATIVE_HEADER + '@protocol LibboxRuntimeTunSession <NSObject>\n' + NATIVE_SESSION_METHODS + '@end\n',
            '#if 0\n' + NATIVE_HEADER + '#endif\n',
            '#ifdef POLARIS_NATIVE_TUN_V1\n' + NATIVE_HEADER + '#endif\n',
        ]
        for header in bad_headers:
            with self.subTest(header=header[:80]), self.assertRaises(RuntimeError):
                subject.native_tun_abi_present([header])

    @staticmethod
    def prepared(root, platform, new_abi):
        _, _, header, _, _, env = OptionalTLSCapabilityLink.versioned_framework(root, platform, False)
        if new_abi:
            header.write_text(NATIVE_HEADER)
        prepared = root / 'parent-prepared'
        with patch.dict(os.environ, dict(env, DERIVED_FILE_DIR=str(prepared))), \
                patch.object(subject, 'run', return_value='T _LibboxVersion\n'):
            subject.prepare()
        env.update(POLARIS_TLS_PREPARED_ROOT=str(prepared),
                   POLARIS_TLS_PREPARED_SHA256=subject.file_pin(prepared / 'PolarisTLSFingerprintLinker.json')['sha256'])
        return header, prepared, env

    def test_actual_selected_slice_prepare_install_and_verify_recompute_abi(self):
        for platform in ('iphoneos', 'iphonesimulator'):
            for present in (False, True):
                with self.subTest(platform=platform, present=present), tempfile.TemporaryDirectory() as tmp:
                    _, prepared, env = self.prepared(Path(tmp), platform, present)
                    receipt = json.loads((prepared / 'PolarisTLSFingerprintLinker.json').read_text())
                    self.assertEqual(receipt['nativeTun']['present'], present)
                    with patch.dict(os.environ, env), \
                            patch.object(subject, 'run', side_effect=AssertionError('Xcode hook ran native tools')):
                        subject.install()
                        self.assertEqual(subject.compiler_args_path().read_bytes(), subject.compiler_args(present))
                        subject.verify_installed()
                        subject.compiler_args_path().write_bytes(b'-D\nFORGED\n')
                        with self.assertRaisesRegex(RuntimeError, 'native TUN compiler input changed'):
                            subject.verify_installed()

    def test_stale_native_args_are_overwritten_for_actual_old_headers(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, _, env = self.prepared(Path(tmp), 'iphoneos', False)
            with patch.dict(os.environ, env):
                subject.compiler_args_path().write_bytes(subject.compiler_args(True))
                subject.install()
                self.assertEqual(subject.compiler_args_path().read_bytes(), b'')
                subject.verify_installed()

    def test_legacy_receipt_requires_old_abi_and_cannot_authorize_stale_flags(self):
        for new_abi in (False, True):
            with self.subTest(new_abi=new_abi), tempfile.TemporaryDirectory() as tmp:
                _, prepared, env = self.prepared(Path(tmp), 'iphoneos', new_abi)
                receipt = prepared / 'PolarisTLSFingerprintLinker.json'
                value = json.loads(receipt.read_text()); value.pop('nativeTun')
                receipt.write_text(json.dumps(value))
                (prepared / subject.NATIVE_TUN_ARGS).unlink()
                env['POLARIS_TLS_PREPARED_SHA256'] = subject.file_pin(receipt)['sha256']
                with patch.dict(os.environ, env):
                    if new_abi:
                        with self.assertRaisesRegex(RuntimeError, 'legacy TLS receipt cannot attest'):
                            subject.install()
                    else:
                        subject.install()
                        self.assertEqual(subject.compiler_args_path().read_bytes(), b'')
                        (prepared / subject.NATIVE_TUN_ARGS).write_bytes(subject.compiler_args(True))
                        with self.assertRaisesRegex(RuntimeError, 'unexpected native TUN compiler arguments'):
                            subject.install()

    def test_parent_argument_and_actual_header_drift_rejected_before_compile(self):
        for case in ('parent-args', 'header', 'receipt-abi', 'receipt-shape', 'redirected-output'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                header, prepared, env = self.prepared(Path(tmp), 'iphoneos', True)
                if case == 'parent-args':
                    (prepared / subject.NATIVE_TUN_ARGS).write_bytes(b'')
                elif case == 'header':
                    header.write_text(header.read_text().replace('- (BOOL)published;', ''))
                elif case in ('receipt-abi', 'receipt-shape'):
                    receipt = prepared / 'PolarisTLSFingerprintLinker.json'
                    value = json.loads(receipt.read_text())
                    if case == 'receipt-abi':
                        value['nativeTun']['present'] = False
                    else:
                        value['nativeTun']['unrecognized'] = True
                    receipt.write_text(json.dumps(value))
                    env['POLARIS_TLS_PREPARED_SHA256'] = subject.file_pin(receipt)['sha256']
                with patch.dict(os.environ, env):
                    if case == 'redirected-output':
                        elsewhere = Path(tmp) / 'foreign-args'; elsewhere.write_bytes(b'')
                        subject.compiler_args_path().symlink_to(elsewhere)
                    with self.assertRaises(RuntimeError):
                        subject.install()

    def test_project_templates_and_existing_gui_cli_ci_target_consume_response_file(self):
        root = Path(__file__).resolve().parents[2]
        for name in ('project.yml', 'polaris.xcodeproj/project.pbxproj'):
            text = (root / 'src-tauri/gen/apple' / name).read_text()
            self.assertIn('OTHER_SWIFT_FLAGS', text)
            self.assertIn('@$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args', text)
            self.assertNotIn('-D POLARIS_NATIVE_TUN_V1', text)
        pbx = (root / 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj').read_text()
        target = pbx.split('78552A5C57D5DDA9797F1E67 /* polaris_PacketTunnel */ =', 1)[1].split('buildRules', 1)[0]
        self.assertLess(target.index('Verify iOS Core'), target.index('Sources'))
        phase = pbx.split('B395AB4CF19394B13456AAB7 /* Verify iOS Core */ =', 1)[1].split('6BA4F5A019704F91829A51D3', 1)[0]
        self.assertIn('alwaysOutOfDate = 1', phase)
        self.assertIn('$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args', phase)
        self.assertEqual(pbx.count('OTHER_SWIFT_FLAGS = ("$(inherited)", "@$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args",);'), 2)
        builder = (root / 'scripts/mobile-core/build_apple_app.py').read_text()
        self.assertLess(builder.index("'original-tls-prepare.log'"), builder.index("timed_phase('app-extension-archive'"))
        swift = (root / 'src-tauri/gen/apple/PacketTunnel/TunnelPlatform.swift').read_text()
        self.assertIn('#if POLARIS_NATIVE_TUN_V1', swift)
        native_tun = swift.split('#if POLARIS_NATIVE_TUN_V1', 1)[1].split('#endif', 1)[0]
        self.assertIn('extension TunnelPlatform: LibboxRuntimeTunPlatformProtocol', native_tun)
        provider = (root / 'src-tauri/gen/apple/PacketTunnel/PacketTunnelProvider.swift').read_text()
        guard_start = provider.index('#if POLARIS_NATIVE_TUN_V1')
        guard_end = provider.index('#endif', guard_start)
        registration = provider[guard_start:guard_end]
        self.assertIn('try server.setRuntimeTunPlatform(self.platform)', registration)
        self.assertLess(guard_end, provider.index('try server.start()'))


if __name__ == '__main__':
    unittest.main()
