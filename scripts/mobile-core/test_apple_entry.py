"""Pure controls only: authored for the eventual whole-candidate Node gate."""
import unittest
import tempfile
import json
import struct
import io
import zipfile
from pathlib import Path
from unittest import mock

import apple_rust
import build_apple_app
import ci_apple
import consume
from carriers import verify_apple_resources
from artifacts import module


def object_archive(cpu, subtype, platform):
    obj = struct.pack('<8I', 0xfeedfacf, cpu, subtype, 1, 1, 24, 0, 0)
    obj += struct.pack('<6I', 0x32, 24, platform, 15 << 16, 27 << 16, 0)
    body = b'go.o' + obj
    header = f'{"#1/4":<16}{0:<12}{0:<6}{0:<6}{"644":<8}{len(body):<10}`\n'.encode()
    return b'!<arch>\n' + header + body


def single_fat_archive(raw, cpu=0x100000c, subtype=0):
    header = b'\xca\xfe\xba\xbe' + struct.pack('>I', 1)
    header += struct.pack('>5I', cpu, subtype, 32, len(raw), 4)
    return header + bytes(32 - len(header)) + raw


def fat_archive(arm_platform=7, intel_platform=7):
    arm = object_archive(0x100000c, 0, arm_platform)
    intel = object_archive(0x1000007, 3, intel_platform)
    header = b'\xca\xfe\xba\xbe' + struct.pack('>I', 2)
    header += struct.pack('>5I', 0x100000c, 0, 64, len(arm), 4)
    header += struct.pack('>5I', 0x1000007, 3, 64 + len(arm), len(intel), 4)
    return header + bytes(64 - len(header)) + arm + intel, arm, intel


class AppleEntryTests(unittest.TestCase):
    def test_host_swift_controls_select_mac_sdk_target_without_changing_app_environment(self):
        from types import SimpleNamespace
        for host in ('arm64', 'x86_64'):
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); sdk = root / 'MacOSX.sdk'; sdk.mkdir()
                config = root / 'src-tauri/tauri.conf.json'; config.parent.mkdir()
                config.write_text(json.dumps({'bundle': {'macOS': {'minimumSystemVersion': '13.0'}}}))
                env = {'SDKROOT': '/selected/iPhoneOS.sdk', 'PLATFORM_NAME': 'iphoneos',
                       'ARCHS': 'arm64', 'IPHONEOS_DEPLOYMENT_TARGET': '17.0',
                       'DEVELOPER_DIR': '/selected/Xcode/Contents/Developer'}
                original = dict(env); calls = []
                def run(command, **kwargs):
                    calls.append(([str(x) for x in command], kwargs))
                    return str(sdk) if '--show-sdk-path' in command else ''
                with mock.patch.object(build_apple_app, 'ROOT', root), \
                        mock.patch.object(build_apple_app.os, 'uname', return_value=SimpleNamespace(machine=host)), \
                        mock.patch.object(build_apple_app, 'run', side_effect=run):
                    build_apple_app.host_swift_controls(root, env)
                command, options = calls[1]
                self.assertEqual(command[:4], ['xcrun', '--sdk', 'macosx', 'swiftc'])
                self.assertEqual(command[command.index('-sdk') + 1], str(sdk))
                self.assertEqual(command[command.index('-target') + 1], host + '-apple-macosx13.0')
                self.assertIn('-parse-as-library', command)
                self.assertEqual(options['env']['SDKROOT'], str(sdk))
                self.assertEqual(options['env']['ARCHS'], host)
                self.assertNotIn('IPHONEOS_DEPLOYMENT_TARGET', options['env'])
                self.assertEqual(calls[2][0], [str(root / 'measurement-conditions-test')])
                self.assertEqual(env, original)
                self.assertEqual(options['env']['DEVELOPER_DIR'], env['DEVELOPER_DIR'])

    def test_host_swift_control_rejects_unknown_host_before_compilation(self):
        from types import SimpleNamespace
        with mock.patch.object(build_apple_app.os, 'uname', return_value=SimpleNamespace(machine='unknown')), \
                mock.patch.object(build_apple_app, 'run') as run, \
                self.assertRaisesRegex(RuntimeError, 'unsupported actual Mac'):
            build_apple_app.host_swift_controls(Path('/not-used'), {})
        run.assert_not_called()

    def controlled_status(self, root, raw):
        facts = {'statusComplete': True, 'reason': None, 'observedStdoutBytes': len(raw),
                 'observedStderrBytes': 0, 'observedStatusPrefixSha256': consume.sha(raw)}
        with mock.patch.object(consume, 'ROOT', root), \
                mock.patch.object(consume, 'bounded_source_status', return_value=(raw, facts)):
            return consume.source_status_observation()

    def test_clean_derived_status_preserves_identity_without_original_export(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            log = root / 'source-status-before-rust.json'
            observed = self.controlled_status(root, b'')
            with mock.patch.object(consume, 'source_status_observation', return_value=observed), \
                    mock.patch.object(consume, 'run', return_value='a' * 40 + '\n'), \
                    mock.patch.object(consume, 'file_pin', return_value={'bytes': 1, 'sha256': 'b' * 64}):
                identity = consume.app_identity(status_log=log)
            self.assertEqual(identity['candidate'], 'a' * 40)
            saved = json.loads(log.read_bytes())
            self.assertTrue(saved['complete'] and saved['clean'])
            self.assertFalse(saved['rawStatusRetained'] or saved['fileContentsExported'])
            self.assertEqual(list(root.glob('*.log')), [])

    def test_dirty_generated_or_tracked_metadata_rejects_without_credentials_or_bodies(self):
        generated = 'crates/tauri-plugin-polaris-ios/ios/Package.resolved'
        tracked = 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj'
        secret = 'DO_NOT_EXPORT_CREDENTIAL_OR_SOURCE_BODY'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in (generated, tracked):
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('https://user:' + secret + '@example.invalid/private\n')
            raw = b'?? ' + generated.encode() + b'\0 M ' + tracked.encode() + b'\0'
            saved = self.controlled_status(root, raw)
            self.assertTrue(saved['complete']); self.assertFalse(saved['clean'])
            self.assertEqual([row['path'] for row in saved['paths']], [generated, tracked])
            self.assertTrue(all(row['sha256'] for row in saved['paths']))
            self.assertNotIn(secret, json.dumps(saved))
            self.assertNotIn('https://', json.dumps(saved))
            log = root / 'source-status-after-rust.json'
            with mock.patch.object(consume, 'source_status_observation', return_value=saved), \
                    self.assertRaisesRegex(RuntimeError, 'App consumer source is not frozen'):
                consume.app_identity(status_log=log)
            self.assertNotIn(secret, log.read_text())
            self.assertEqual(list(root.glob('*.log')), [])
            unknown = root / ('private-' + secret); unknown.write_bytes(b'inert')
            redacted = self.controlled_status(root, b'?? ' + unknown.name.encode() + b'\0')
            self.assertIsNone(redacted['paths'][0]['path'])
            self.assertNotIn(secret, json.dumps(redacted))

    def test_path_count_length_and_total_hash_budgets_are_incomplete_not_clean(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for i in range(consume.STATUS_PATHS + 1): (root / ('entry-' + str(i))).write_bytes(b'')
            many = b''.join(b'?? entry-' + str(i).encode() + b'\0' for i in range(consume.STATUS_PATHS + 1))
            for raw in (many, b'?? ' + b'x' * (consume.STATUS_PATH_BYTES + 1) + b'\0', b'?? partial'):
                observed = self.controlled_status(root, raw)
                self.assertFalse(observed['complete'] or observed['clean'])
                self.assertLessEqual(len(observed['paths']), consume.STATUS_PATHS)
            large = root / 'Cargo.lock'; large.write_bytes(b'x' * (consume.STATUS_HASH_BYTES + 1))
            observed = self.controlled_status(root, b' M Cargo.lock\0')
            self.assertFalse(observed['complete'])
            self.assertEqual(observed['hashBytesRead'], 0)
            self.assertIsNone(observed['paths'][0]['sha256'])

    def test_pipe_stdout_stderr_and_deadline_are_capped_during_collection(self):
        import sys
        import time
        cases = [
            ('import os; os.write(1, b"x" * 65536)', 'stdout-budget', consume.STATUS_BYTES + 1),
            ('import os; os.write(2, b"DO_NOT_EXPORT_SECRET" * 2048)', 'stderr-budget', consume.STATUS_STDERR_BYTES + 1),
            ('import time; time.sleep(2)', 'deadline', None),
        ]
        for code, reason, expected in cases:
            with self.subTest(reason=reason):
                raw, observed = consume.bounded_source_status(
                    [sys.executable, '-c', code], time.monotonic() + (0.1 if reason == 'deadline' else 5))
                self.assertFalse(observed['statusComplete'])
                self.assertEqual(observed['reason'], reason)
                self.assertLessEqual(len(raw), consume.STATUS_BYTES + 1)
                self.assertNotIn('DO_NOT_EXPORT_SECRET', json.dumps(observed))
                if expected is not None:
                    self.assertEqual(observed['observedStdoutBytes' if reason == 'stdout-budget' else 'observedStderrBytes'], expected)

    def test_collection_error_writes_incomplete_safe_metadata_and_rejects(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / 'source-status-before-rust.json'
            with mock.patch.object(consume, 'source_status_observation', side_effect=OSError('DO_NOT_EXPORT_SECRET')), \
                    self.assertRaisesRegex(RuntimeError, 'App consumer source is not frozen'):
                consume.app_identity(status_log=log)
            observed = json.loads(log.read_bytes())
            self.assertFalse(observed['complete'] or observed['clean'])
            self.assertEqual(observed['reason'], 'collector-error')
            self.assertNotIn('DO_NOT_EXPORT_SECRET', log.read_text())
            with mock.patch.object(consume.subprocess, 'Popen', side_effect=OSError('DO_NOT_EXPORT_SECRET')):
                raw, observed = consume.bounded_source_status(['git'], float('inf'))
            self.assertEqual(raw, b'')
            self.assertFalse(observed['statusComplete'])
            self.assertNotIn('DO_NOT_EXPORT_SECRET', json.dumps(observed))

    def test_report_budget_and_write_failure_cannot_accept_or_overwrite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            observed = self.controlled_status(root, b'')
            observed['paths'] = [{'pathSha256': 'a' * 64}] * 64
            observed.update(clean=False, complete=False, reason='controlled-dirty')
            log = root / 'source-status-after-rust.json'
            with mock.patch.object(consume, 'source_status_observation', return_value=observed), \
                    mock.patch.object(consume, 'STATUS_REPORT_BYTES', 1024), \
                    self.assertRaisesRegex(RuntimeError, 'App consumer source is not frozen'):
                consume.app_identity(status_log=log)
            self.assertEqual(json.loads(log.read_bytes())['reason'], 'report-budget')
            self.assertLessEqual(log.stat().st_size, 1024)
            original = log.read_bytes()
            with mock.patch.object(consume, 'source_status_observation', return_value=self.controlled_status(root, b'')), \
                    self.assertRaisesRegex(RuntimeError, 'already frozen'):
                consume.app_identity(status_log=log)
            self.assertEqual(log.read_bytes(), original)
            with mock.patch.object(consume, 'source_status_observation', return_value=self.controlled_status(root, b'')), \
                    self.assertRaisesRegex(RuntimeError, 'incomplete source status observation could not be written'):
                consume.app_identity(status_log=root / 'absent/observation.json')

    def test_preexisting_dirty_rejects_before_link_verification_after_slot_admission(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); target = root / 'target'; target.mkdir()
            env = {'POLARIS_FORMAL_APP_BUILD': '1', 'PLATFORM_NAME': 'iphoneos', 'ARCHS': 'arm64',
                   'CONFIGURATION': 'release', 'POLARIS_APP_WORK': str(root), 'POLARIS_APP_RUST_TARGET': str(target)}
            with mock.patch.dict(apple_rust.os.environ, env), \
                    mock.patch.object(apple_rust.os, 'uname', return_value=SimpleNamespace(sysname='Darwin')), \
                    mock.patch.object(apple_rust, 'load', return_value={}), \
                    mock.patch.object(apple_rust, 'private_dir', side_effect=lambda value: Path(value)), \
                    mock.patch.object(apple_rust, 'app_identity', side_effect=RuntimeError('controlled dirty')) as identity, \
                    mock.patch.object(apple_rust, 'verify_link') as link, \
                    self.assertRaisesRegex(RuntimeError, 'controlled dirty'):
                apple_rust.build_reserved()
            identity.assert_called_once_with(status_log=root / 'source-status-before-rust.json')
            link.assert_not_called()

    def test_exact_three_actual_target_mappings(self):
        self.assertEqual(apple_rust.target_for('iphoneos', 'arm64', 'release'), 'aarch64-apple-ios')
        self.assertEqual(apple_rust.target_for('iphonesimulator', 'arm64', 'release'), 'aarch64-apple-ios-sim')
        self.assertEqual(apple_rust.target_for('iphonesimulator', 'x86_64', 'release'), 'x86_64-apple-ios')

    def test_host_arch_does_not_replace_explicit_intel_simulator(self):
        with mock.patch.dict(apple_rust.os.environ, {'HOST_ARCH': 'arm64'}):
            self.assertEqual(apple_rust.target_for('iphonesimulator', 'x86_64', 'release'), 'x86_64-apple-ios')

    def test_partial_or_wrong_platform_arch_profile_refuses(self):
        for platform, arch, profile in [('iphoneos', 'x86_64', 'release'), ('macosx', 'arm64', 'release'),
                                        ('iphonesimulator', 'arm64 x86_64', 'release'), ('iphoneos', 'arm64', 'debug')]:
            with self.subTest(platform=platform, arch=arch, profile=profile), self.assertRaises(RuntimeError):
                apple_rust.target_for(platform, arch, profile)

    def test_candidate_chain_has_all_three_finite_carriers(self):
        self.assertEqual(ci_apple.CARRIERS, [('device-arm64', None), ('sim-arm64', 'arm64'), ('sim-amd64', 'amd64')])

    def test_local_entry_does_not_claim_hosted_admission(self):
        with mock.patch.dict(ci_apple.os.environ, {'GITHUB_ACTIONS': 'false'}), self.assertRaises(RuntimeError):
            ci_apple.location()

    def test_native_start_app_definition_is_required_and_tool_errors_propagate(self):
        archive, log = Path('/inert/libpolaris_lib.a'), Path('/inert/symbols.log')
        with mock.patch.object(apple_rust, 'run', return_value='00000000 T _start_app\n') as tool:
            apple_rust.verify_start_app(archive, log)
        tool.assert_called_once_with(['xcrun', 'nm', '--no-llvm-bc', '-gU', archive], log=log)
        for symbols in ['', '                 U _start_app\n', '00000000 T _start_app_extra\n']:
            with self.subTest(symbols=symbols), \
                    mock.patch.object(apple_rust, 'run', return_value=symbols), \
                    self.assertRaisesRegex(RuntimeError, '^actual Rust start_app definition missing$'):
                apple_rust.verify_start_app(archive, log)
        with mock.patch.object(apple_rust, 'run', side_effect=RuntimeError('invalid native object')), \
                self.assertRaisesRegex(RuntimeError, '^invalid native object$'):
            apple_rust.verify_start_app(archive, log)

    def test_all_eight_native_global_swift_entries_are_required_before_staging_app_archive(self):
        entries = apple_rust.SWIFT_BRIDGES
        complete = ''.join('00000000 T ' + name + '\n' for name in entries)
        self.assertEqual(apple_rust.verify_swift_bridges(complete), list(entries))
        # Each missing, undefined or local/private entry is a real link gap.
        for name in entries:
            for replacement in ('', '                 U ' + name + '\n',
                                '00000000 t ' + name + '\n', '00000000 T ' + name + '_extra\n'):
                with self.subTest(name=name, replacement=replacement), \
                        self.assertRaisesRegex(RuntimeError, 'actual global Swift bridge definition missing'):
                    apple_rust.verify_swift_bridges(complete.replace('00000000 T ' + name + '\n', replacement))
        original_partial_log = '00000000 T _start_app\n'
        with self.assertRaisesRegex(RuntimeError, 'actual global Swift bridge definition missing'):
            apple_rust.verify_swift_bridges(original_partial_log)

    def test_hosted_selection_rejects_default_xcode_wrong_build_and_either_old_sdk_before_tool_install(self):
        value = ci_apple.load()
        apple = value['apple']
        expected_xcode = 'Xcode ' + apple['xcodeVersion'] + '\nBuild version ' + apple['xcodeBuild']
        developer = Path('/selected/Xcode_27.1_Release_Candidate.app/Contents/Developer')
        candidate = 'a' * 40
        admitted = 'control: exact selected tools admitted'
        cases = [
            ('Xcode 27.0\nBuild version 27A266a', apple['iPhoneSDK'], apple['simulatorSDK'],
             'runner does not provide the exact selected Xcode; no fallback/skip'),
            ('Xcode ' + apple['xcodeVersion'] + '\nBuild version 27A266a', apple['iPhoneSDK'], apple['simulatorSDK'],
             'runner does not provide the exact selected Xcode; no fallback/skip'),
            ('Xcode ' + apple['xcodeVersion'] + '\nBuild version 27A9269', apple['iPhoneSDK'], apple['simulatorSDK'],
             'runner does not provide the exact selected Xcode; no fallback/skip'),
            (expected_xcode, '27.0', apple['simulatorSDK'], 'runner does not provide the exact selected SDK'),
            (expected_xcode, apple['iPhoneSDK'], '27.0', 'runner does not provide the exact selected SDK'),
            (expected_xcode, apple['iPhoneSDK'], apple['simulatorSDK'], admitted),
        ]
        for xcode, device_sdk, simulator_sdk, reason in cases:
            with self.subTest(xcode=xcode, device_sdk=device_sdk, simulator_sdk=simulator_sdk):
                answers = {('git', 'rev-parse', 'HEAD'): candidate, ('xcodebuild', '-version'): xcode,
                    ('xcrun', '--sdk', 'iphoneos', '--show-sdk-version'): device_sdk,
                    ('xcrun', '--sdk', 'iphonesimulator', '--show-sdk-version'): simulator_sdk}
                selected_path = mock.Mock()
                selected_path.resolve.return_value = developer
                with mock.patch.object(ci_apple, 'producer_code', return_value={}), \
                        mock.patch.object(ci_apple, 'location', return_value=Path('/isolated/entry')), \
                        mock.patch.dict(ci_apple.os.environ, {'POLARIS_APP_CANDIDATE': candidate}), \
                        mock.patch.object(ci_apple, 'Path', return_value=selected_path) as path, \
                        mock.patch.object(ci_apple, 'run', side_effect=lambda args, **_: answers[tuple(args)]) as run, \
                        mock.patch('apple_admission.preflight', return_value={}) as admission, \
                        mock.patch.object(ci_apple, 'private_dir', side_effect=RuntimeError(admitted)) as tools, \
                        self.assertRaisesRegex(RuntimeError, '^' + reason + '$'):
                    ci_apple.prepare()
                path.assert_called_once_with('/Applications/Xcode_27.1.app/Contents/Developer')
                selected_path.resolve.assert_called_once_with(strict=True)
                for call in run.call_args_list[1:]:
                    self.assertEqual(call.kwargs['env']['DEVELOPER_DIR'], str(developer))
                self.assertEqual(tools.call_count, int(reason == admitted))
                self.assertEqual(admission.call_count, int(reason == admitted))

    def test_actual_rust_link_verifier_refuses_changed_archive_and_drops_xcode_fd(self):
        from artifacts import file_pin
        from fork_input import canonical, sha
        from producer import write_json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sdk = root / 'sdk'; sdk.mkdir()
            work = root / 'work'; work.mkdir(mode=0o700)
            archive = root / 'src-tauri/gen/apple/Externals/arm64/release/libapp.a'
            archive.parent.mkdir(parents=True); archive.write_bytes(b'!<arch>\ninert control')
            for name in ['Cargo.lock', 'src-tauri/tauri.ios.conf.json', *apple_rust.SWIFT_EXPORT_POLICY]:
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'inert source')
            tool = root / 'tool'; tool.write_bytes(b'inert tool')
            framework = root / 'Libbox.xcframework'
            original = framework / 'ios-arm64/Libbox.framework/Libbox'
            original.parent.mkdir(parents=True); original.write_bytes(object_archive(0x100000c, 0, 2))
            selected = {'libraryIdentifier': 'ios-arm64', 'binary': file_pin(original)}
            link = {'App': {'candidate': 'a' * 40}, 'framework': str(framework),
                    'componentOutput': {'slices': {'ios-arm64': selected}}}
            thin, selection = apple_rust.selected_wire_bytes(original.read_bytes(), 'aarch64-apple-ios')
            wire = work / 'wire-aarch64-apple-ios.a'; wire.write_bytes(thin)
            receipt = {'schema': 'polaris-formal-apple-rust-v1', 'target': 'aarch64-apple-ios',
                       'App': link['App'], 'linkInputSha256': 'b' * 64, 'sdk': str(sdk),
                       'linkedArchive': str(archive), 'archive': file_pin(archive),
                       'cargoLock': file_pin(root / 'Cargo.lock'),
                       'iosConfig': file_pin(root / 'src-tauri/tauri.ios.conf.json'),
                       'swiftBridgeDefinitions': list(apple_rust.SWIFT_BRIDGES),
                       'swiftExportPolicy': {name: file_pin(root / name) for name in apple_rust.SWIFT_EXPORT_POLICY},
                       'tools': {'cargo': {'path': str(tool), **file_pin(tool)}}, 'devicePassed': False,
                       'originalWireArchive': {'path': str(original), **file_pin(original)},
                       'wireArchive': {'path': str(wire), **file_pin(wire)}, 'wireSelection': selection}
            receipt['fingerprint'] = sha(canonical(receipt))
            write_json(work / 'rust-aarch64-apple-ios.json', receipt)
            env = {'POLARIS_FORMAL_APP_BUILD': '1', 'POLARIS_APP_WORK': str(work),
                   'PLATFORM_NAME': 'iphoneos', 'ARCHS': 'arm64', 'CONFIGURATION': 'release',
                   'SDKROOT': str(sdk), 'POLARIS_MOBILE_LINK_INPUT': str(work / 'link.json'),
                   'POLARIS_MOBILE_LINK_INPUT_SHA256': 'b' * 64, 'POLARIS_BUILD_RESERVATION_FD': '999999'}
            with mock.patch.object(apple_rust, 'ROOT', root), mock.patch.dict(apple_rust.os.environ, env), \
                    mock.patch.object(apple_rust, 'verify_link', return_value=link):
                apple_rust.verify()
                self.assertNotIn('POLARIS_BUILD_RESERVATION_FD', apple_rust.os.environ)
                for field in ('swiftBridgeDefinitions', 'swiftExportPolicy'):
                    changed = {key: val for key, val in receipt.items() if key not in (field, 'fingerprint')}
                    changed['fingerprint'] = sha(canonical(changed))
                    with mock.patch.object(apple_rust, 'read_receipt', return_value=changed), \
                            self.assertRaisesRegex(RuntimeError, 'input differs'):
                        apple_rust.verify()
                policy = root / apple_rust.SWIFT_EXPORT_POLICY[0]
                policy.write_bytes(b'changed export policy')
                with self.assertRaisesRegex(RuntimeError, 'input differs'):
                    apple_rust.verify()
                policy.write_bytes(b'inert source')
                archive.write_bytes(b'!<arch>\nchanged')
                with self.assertRaisesRegex(RuntimeError, 'input differs'):
                    apple_rust.verify()

    def test_fat_simulator_selects_exact_cargo_architecture_not_first_descriptor(self):
        raw, arm, intel = fat_archive()
        actual, selection = apple_rust.selected_wire_bytes(raw, 'x86_64-apple-ios')
        self.assertEqual(actual, intel)
        self.assertEqual(selection['offset'], 64 + len(arm))
        self.assertEqual(apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios-sim')[0], arm)
        with self.assertRaisesRegex(RuntimeError, 'exact Cargo'):
            apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios')
        wrong_header = bytearray(raw); struct.pack_into('>I', wrong_header, 8, 0x1000007)
        with self.assertRaises(RuntimeError):
            apple_rust.selected_wire_bytes(bytes(wrong_header), 'x86_64-apple-ios')
        overlap = bytearray(raw); struct.pack_into('>I', overlap, 36, 64)
        with self.assertRaises(RuntimeError):
            apple_rust.selected_wire_bytes(bytes(overlap), 'x86_64-apple-ios')
        for invalid in [raw[:-1], b'!<arch>\n', b'\xcf\xfa\xed\xfe']:
            with self.subTest(raw=invalid[:8]), self.assertRaises(RuntimeError):
                apple_rust.selected_wire_bytes(invalid, 'x86_64-apple-ios')

    def test_real_resource_materialization_and_final_carrier_require_28_geo_and_licenses(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / 'resources/data'; data.mkdir(parents=True)
            for n in range(28):
                (data / f'geo-{n}.srs').write_bytes(b'SRS' + bytes([n]))
            for name in ('LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'):
                (root / name).write_bytes(('actual ' + name).encode())
            config = root / 'src-tauri/tauri.ios.conf.json'; config.parent.mkdir(parents=True)
            config.write_text(json.dumps({'bundle': {'resources': ['../resources/data/',
                '../THIRD-PARTY-LICENSES.md', '../NOTICE', '../LICENSE']}}))
            (root / 'src-tauri/gen/apple').mkdir(parents=True)
            with mock.patch.object(consume, 'ROOT', root):
                expected = consume.materialize_apple_resources()
                self.assertEqual(len(expected), 31)
                self.assertEqual(consume.materialize_apple_resources(), expected)
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, 'w') as package:
                    for name, row in expected.items():
                        info = zipfile.ZipInfo('Payload/Polaris.app/assets/' + name)
                        info.create_system = 3; info.external_attr = (0o100000 | row['mode']) << 16
                        package.writestr(info, (root / row['source']).read_bytes())
                with zipfile.ZipFile(io.BytesIO(stream.getvalue())) as package:
                    observed = verify_apple_resources(package, 'Payload/Polaris.app/', expected)
                    self.assertEqual(len(observed), 31)
                    changed = {key: dict(row) for key, row in expected.items()}
                    key = next(iter(changed)); changed[key]['sha256'] = '0' * 64
                    with self.assertRaisesRegex(RuntimeError, 'bytes/mode'):
                        verify_apple_resources(package, 'Payload/Polaris.app/', changed)
                    with self.assertRaisesRegex(RuntimeError, 'inventory'):
                        verify_apple_resources(package, 'Payload/Polaris.app/', {})
                output = root / 'src-tauri/gen/apple/assets/_up_/NOTICE'
                output.write_bytes(b'wrong retained output')
                with self.assertRaisesRegex(RuntimeError, 'resource differs'):
                    consume.materialize_apple_resources()
                self.assertEqual(output.read_bytes(), b'wrong retained output')
                (data / 'geo-0.srs').unlink()
                with self.assertRaisesRegex(RuntimeError, '28 geo'):
                    consume.apple_resource_inputs()

    def test_resource_unknown_stage_is_preserved_and_never_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / 'src-tauri/gen/apple/assets.formal-stage'; stage.mkdir(parents=True)
            retained = stage / 'unknown'; retained.write_bytes(b'retained')
            with mock.patch.object(consume, 'ROOT', root), \
                    mock.patch.object(consume, 'apple_resource_inputs', return_value={}):
                with self.assertRaisesRegex(RuntimeError, 'stage retained'):
                    consume.materialize_apple_resources()
            self.assertEqual(retained.read_bytes(), b'retained')

    def test_single_fat_device_wrapper_selects_the_same_actual_target_bytes(self):
        device = object_archive(0x100000c, 0, 2)
        raw = single_fat_archive(device)
        thin, selection = apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios')
        self.assertEqual(thin, device)
        self.assertEqual(selection, {'target': 'aarch64-apple-ios', 'cpu': 0x100000c, 'platform': 2,
            'offset': 32, 'bytes': len(device), 'container': 'fat-ar'})
        with self.assertRaisesRegex(RuntimeError, 'exact Cargo'):
            apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios-sim')
        invalid_counts = []
        for count in (0, 3):
            changed = bytearray(raw); struct.pack_into('>I', changed, 4, count); invalid_counts.append(bytes(changed))
        for invalid in [raw[:-1], raw + b'\0', single_fat_archive(device, cpu=0x1000007, subtype=3), *invalid_counts]:
            with self.subTest(raw=invalid[:32]), self.assertRaises(RuntimeError):
                apple_rust.selected_wire_bytes(invalid, 'aarch64-apple-ios')

    def test_failed_staging_sdk_original_bytes_are_retained_without_acceptance(self):
        from contextlib import contextmanager
        @contextmanager
        def held(*args):
            yield
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework = root / 'component/staging/Libbox.xcframework'
            library = framework / 'ios-arm64/Libbox.framework'
            library.mkdir(parents=True)
            raw = single_fat_archive(object_archive(0x100000c, 0, 2))
            (library / 'Libbox').write_bytes(raw)
            (root / 'cache').mkdir(mode=0o700)
            rust_archive = root / 'cache/rust-target/aarch64-apple-ios/release/libpolaris_lib.a'
            rust_archive.parent.mkdir(parents=True)
            rust_archive.write_bytes(b'!<arch>\nunaccepted archive fixture')
            with mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', held), mock.patch('builtins.print'):
                ci_apple.evidence()
            with zipfile.ZipFile(root / 'apple-original-evidence.zip') as archive:
                member = 'component/staging/Libbox.xcframework/ios-arm64/Libbox.framework/Libbox'
                self.assertEqual(archive.read(member), raw)
                self.assertEqual(archive.read(rust_archive.relative_to(root).as_posix()), rust_archive.read_bytes())
                index = json.loads(archive.read('EVIDENCE-INDEX.json'))
                self.assertFalse(index['componentAccepted'])
                self.assertFalse(index['stagedComponentIsAccepted'])
                self.assertFalse(index['completeUnsignedChain'])
                self.assertEqual(index['files'][member]['sha256'], __import__('hashlib').sha256(raw).hexdigest())

    def test_unreadable_component_receipt_does_not_block_original_evidence_packaging(self):
        from contextlib import contextmanager
        @contextmanager
        def held(*args):
            yield
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            component = root / 'component'; component.mkdir()
            raw = b'{"schema":"polaris-mobile-component-v1","nativeInputs":'
            (component / 'component-receipt.json').write_bytes(raw)
            (component / 'original-bind.log').write_bytes(b'original tool output')
            with mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', held), mock.patch('builtins.print'):
                ci_apple.evidence()
            with zipfile.ZipFile(root / 'apple-original-evidence.zip') as archive:
                self.assertEqual(archive.read('component/component-receipt.json'), raw)
                self.assertEqual(archive.read('component/original-bind.log'), b'original tool output')
                index = json.loads(archive.read('EVIDENCE-INDEX.json'))
                self.assertIn('component/component-receipt.json', index['receiptReadErrors'])
                self.assertEqual(index['receiptBytes']['component/component-receipt.json'], len(raw))
                self.assertFalse(index['componentAccepted'])
                self.assertFalse(index['completeUnsignedChain'])
                self.assertFalse(index['publicationEligible'])

    def test_actual_bsd_unsorted_and_sorted_symbol_tables_bind_real_code_offsets(self):
        fixtures = module('formal_bsd_symbol_table_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_bsd_symbol_table_parser', 'scripts/apple-carrier.py')
        helper = module('formal_bsd_symbol_table_info', 'scripts/libbox-patches/android-source.py')
        for table_name in ('__.SYMDEF', '__.SYMDEF SORTED'):
            # For the unsorted table intentionally preserve zeta before alpha.
            # These are structural fixtures, not executed native/SDK proof.
            strings = b'zeta\0alpha\0'
            offsets = (0, 5) if table_name == '__.SYMDEF' else (5, 0)
            table = struct.pack('<6I', 16, offsets[0], 0, offsets[1], 0, len(strings)) + strings
            first = fixtures.archive([(table_name, table)])
            code_offset = len(first)
            table = struct.pack('<6I', 16, offsets[0], code_offset, offsets[1], code_offset, len(strings)) + strings
            code = fixtures.mach_object('ios-arm64')
            raw = fixtures.archive([(table_name, table), ('actual-code.o', code)])
            observed = parser.archive(raw, helper)
            self.assertEqual(observed['members'][0]['name'], table_name)
            self.assertEqual(observed['members'][0]['classification'], 'symbol-table')
            self.assertEqual(observed['members'][1]['headerOffset'], code_offset)
            wrapped = single_fat_archive(raw)
            thin, selected = apple_rust.selected_wire_bytes(wrapped, 'aarch64-apple-ios')
            self.assertEqual(thin, raw)
            self.assertEqual(selected['bytes'], len(raw))
            for bad_table in [table[:-1],
                    struct.pack('<I', 15) + table[4:],
                    table[:4] + struct.pack('<I', len(strings)) + table[8:],
                    table[:8] + struct.pack('<I', code_offset + 1) + table[12:],
                    table[:8] + struct.pack('<I', 8) + table[12:],
                    table[:-1] + b'x', table + bytes(8), table + b'bad']:
                with self.subTest(name=table_name, table=bad_table[:24]), self.assertRaises(RuntimeError):
                    parser.archive(fixtures.archive([(table_name, bad_table), ('actual-code.o', code)]), helper)
            with self.assertRaisesRegex(RuntimeError, 'position/payload'):
                parser.archive(fixtures.archive([('actual-code.o', code), (table_name, table)]), helper)
            with self.assertRaisesRegex(RuntimeError, 'position/payload'):
                parser.archive(fixtures.archive([(table_name, table), (table_name, table), ('actual-code.o', code)]), helper)
            with self.assertRaisesRegex(RuntimeError, 'unsupported code member format'):
                parser.archive(fixtures.archive([(table_name + ' UNKNOWN', table), ('actual-code.o', code)]), helper)

    def test_actual_literal_and_tlv_section_kinds_keep_width_and_zero_fill_bounds(self):
        fixtures = module('formal_section_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_section_parser', 'scripts/apple-carrier.py')
        helper = module('formal_section_info', 'scripts/libbox-patches/android-source.py')
        # All seven kinds occur in the exact ac6 three-slice SDK inventory.
        layouts = [(3, '__TEXT', '__literal4', 4), (4, '__TEXT', '__literal8', 8),
                   (5, '__DATA', '__objc_selrefs', 8), (14, '__TEXT', '__literal16', 16),
                   (17, '__DATA', '__thread_data', 9), (18, '__DATA', '__thread_bss', 8),
                   (19, '__DATA', '__thread_vars', 24)]
        section_header = 32 + 72 + 80  # second section in the anonymous MH_OBJECT segment
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            for kind, segment, section, length in layouts:
                raw = bytearray(fixtures.mach_object(target, extra=[(segment, section, bytes(length))]))
                struct.pack_into('<I', raw, section_header + 64, kind)
                if kind == 18:
                    struct.pack_into('<I', raw, section_header + 48, 0)
                observed = parser.macho(bytes(raw), helper)['sections'][1]
                self.assertEqual(observed['flags'] & 255, kind)
                self.assertEqual(observed['zeroFill'], kind == 18)
                self.assertEqual('sha256' in observed, kind != 18)
                bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 68, 1)
                with self.subTest(target=target, kind=kind, bad='reserved'), self.assertRaises(RuntimeError):
                    parser.macho(bytes(bad), helper)
                bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 64, 0x7f)
                with self.assertRaisesRegex(RuntimeError, 'unsupported section type'):
                    parser.macho(bytes(bad), helper)
                if kind in (3, 4, 5, 14, 19):
                    bad = bytearray(raw); struct.pack_into('<Q', bad, section_header + 40, length - 1)
                    with self.assertRaisesRegex(RuntimeError, 'element width'):
                        parser.macho(bytes(bad), helper)
                if kind == 18:
                    bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 48, 1)
                    with self.assertRaisesRegex(RuntimeError, 'zero-fill file offset'):
                        parser.macho(bytes(bad), helper)
                    bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 60, 1)
                    with self.assertRaisesRegex(RuntimeError, 'TLV zero-fill relocation'):
                        parser.macho(bytes(bad), helper)

    def test_observed_optimization_hint_is_object_only_bounded_disjoint_and_recorded(self):
        fixtures = module('formal_optimization_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_optimization_parser', 'scripts/apple-carrier.py')
        helper = module('formal_optimization_info', 'scripts/libbox-patches/android-source.py')
        payload = bytes(8)  # structural fixture, not a compiler/linker run
        for target in ('ios-arm64', 'ios-arm64-simulator'):
            raw = bytearray(fixtures.mach_object(target, load_commands=[struct.pack('<4I', 0x2e, 16, 0, len(payload))]))
            command = 32 + 152 + 24  # one-section segment and LC_BUILD_VERSION
            struct.pack_into('<I', raw, command + 8, len(raw))
            raw += payload
            observed = [r for r in parser.macho(bytes(raw), helper)['loadCommands'] if r['command'] == 0x2e][0]
            self.assertEqual(observed['dataBytes'], len(payload))
            self.assertEqual(observed['payloadSha256'], helper.digest(payload))
            for field, value in [(4, 24), (8, 1), (8, len(raw) + 1), (12, len(raw)), (0, 0x8000002e)]:
                bad = bytearray(raw); struct.pack_into('<I', bad, command + field, value)
                with self.subTest(field=field, value=value), self.assertRaises(RuntimeError):
                    parser.macho(bytes(bad), helper)
            bad = bytearray(raw); struct.pack_into('<I', bad, 12, 2)
            with self.assertRaisesRegex(RuntimeError, 'optimization hint object'):
                parser.macho(bytes(bad), helper)
            bad = bytearray(raw)
            text_offset = struct.unpack_from('<I', bad, 32 + 72 + 48)[0]
            struct.pack_into('<I', bad, command + 8, text_offset)
            with self.assertRaisesRegex(RuntimeError, 'overlap'):
                parser.macho(bytes(bad), helper)
        duplicate = struct.pack('<4I', 0x2e, 16, 0, 0)
        with self.assertRaisesRegex(RuntimeError, 'duplicate'):
            parser.macho(fixtures.mach_object('ios-arm64', load_commands=[duplicate, duplicate]), helper)

    @staticmethod
    def code_signature_fixture(fixtures, target, *, kind=2, duplicate=False, overlap=False,
                               duplicate_linkedit=False):
        # Structural metadata only: the blob is opaque and is not a valid
        # authenticated signature or a compiler-generated CodeDirectory.
        payload = struct.pack('>7I', 0xfade0cc0, 28, 1, 0, 20, 0xfade0c02, 8)
        text = b'fixture!'
        cpu, subcpu, platform, _, _, _ = fixtures.carrier.TARGETS[target]
        data_commands = 1 + int(duplicate) + int(overlap)
        commands_bytes = 152 + 72 + int(duplicate_linkedit) * 72 + 24 + data_commands * 16
        text_offset = 32 + commands_bytes
        signature_offset = text_offset + len(text)
        section = struct.pack('<16s16sQQ8I', b'__text', b'__TEXT', text_offset,
                              len(text), text_offset, 0, 0, 0, 0, 0, 0, 0)
        text_segment = struct.pack('<II16s4Q4I', 0x19, 152, b'__TEXT', 0,
                                   signature_offset, 0, signature_offset, 5, 5, 1, 0) + section
        linkedit = struct.pack('<II16s4Q4I', 0x19, 72, b'__LINKEDIT', 0x1000,
                               len(payload), signature_offset, len(payload), 1, 1, 0, 0)
        build_version = struct.pack('<6I', 0x32, 24, platform, 17 << 16, (27 << 16) | (1 << 8), 0)
        signature = struct.pack('<4I', 0x1d, 16, signature_offset, len(payload))
        commands = text_segment + linkedit
        if duplicate_linkedit:
            commands += struct.pack('<II16s4Q4I', 0x19, 72, b'__LINKEDIT', 0x2000,
                                    0, signature_offset, 0, 1, 1, 0, 0)
        commands += build_version + signature
        if duplicate:
            commands += signature
        if overlap:
            commands += struct.pack('<4I', 0x26, 16, signature_offset, len(payload))
        header = struct.pack('<8I', 0xfeedfacf, cpu, subcpu, kind,
                             3 + data_commands + int(duplicate_linkedit), commands_bytes, 0, 0)
        return (header + commands + text + payload,
                32 + 152 + 72 + int(duplicate_linkedit) * 72 + 24, signature_offset, payload)

    def test_linked_code_signature_records_opaque_bytes_without_trust(self):
        fixtures = module('formal_signature_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_signature_parser', 'scripts/apple-carrier.py')
        helper = module('formal_signature_info', 'scripts/libbox-patches/android-source.py')
        # The command was observed in the actual8795 sim-arm64 App and
        # PacketTunnel; other targets here are structural controls, not builds.
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            raw, _, offset, payload = self.code_signature_fixture(fixtures, target)
            parsed = parser.macho(raw, helper)
            row = next(row for row in parsed['loadCommands'] if row['command'] == 0x1d)
            self.assertEqual((row['dataOffset'], row['dataBytes']), (offset, len(payload)))
            self.assertEqual(row['payloadSha256'], helper.digest(payload))
            self.assertEqual(row['payloadKind'], 'opaque-code-signature')
            self.assertIs(row['trustValidated'], False)
            changed = raw[:-1] + bytes([raw[-1] ^ 1])
            observed = next(row for row in parser.macho(changed, helper)['loadCommands'] if row['command'] == 0x1d)
            self.assertNotEqual(observed['payloadSha256'], row['payloadSha256'])
            self.assertIs(observed['trustValidated'], False)

    def test_code_signature_refuses_invalid_locus_duplicate_size_and_overlap(self):
        fixtures = module('formal_signature_negative_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_signature_negative_parser', 'scripts/apple-carrier.py')
        helper = module('formal_signature_negative_info', 'scripts/libbox-patches/android-source.py')
        raw, command, offset, payload = self.code_signature_fixture(fixtures, 'ios-arm64-simulator')
        bads = []
        for field, value in [(command + 4, 8), (command + 8, 0),
                             (command + 8, len(raw) + 1), (command + 12, 0),
                             (command + 12, len(payload) + 1)]:
            bad = bytearray(raw); struct.pack_into('<I', bad, field, value); bads.append(bytes(bad))
        missing = bytearray(raw); missing[32 + 152 + 8:32 + 152 + 24] = b'__MISSING'.ljust(16, b'\0'); bads.append(bytes(missing))
        short = bytearray(raw); struct.pack_into('<Q', short, 32 + 152 + 48, len(payload) - 1); bads.append(bytes(short))
        outside = bytearray(raw); struct.pack_into('<2I', outside, command + 8, offset - 8, 8); bads.append(bytes(outside))
        for bad in bads:
            with self.subTest(control=bad[command:command + 16].hex()), self.assertRaises(RuntimeError):
                parser.macho(bad, helper)
        for flags, reason in [({'kind': 1}, 'executable'), ({'duplicate': True}, 'duplicate'),
                              ({'overlap': True}, 'overlap'), ({'duplicate_linkedit': True}, 'unique')]:
            bad, _, _, _ = self.code_signature_fixture(fixtures, 'ios-arm64-simulator', **flags)
            with self.assertRaisesRegex(RuntimeError, reason):
                parser.macho(bad, helper)
        unknown = bytearray(raw); struct.pack_into('<I', unknown, command, 0x1e)
        with self.assertRaisesRegex(RuntimeError, 'unsupported load command'):
            parser.macho(bytes(unknown), helper)

    def test_actual_linked_weak_swift_libraries_and_rpaths_are_structurally_recorded(self):
        fixtures = module('formal_dyld_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_dyld_parser', 'scripts/apple-carrier.py')
        helper = module('formal_dyld_info', 'scripts/libbox-patches/android-source.py')
        # Exact device-arm64 App/PacketTunnel command bytes from ab8cf's IPA.
        weak = bytes.fromhex('18000080480000001800000002000000014d9209000001002f7573722f6c69622f73776966742f6c696273776966744156466f756e646174696f6e2e64796c696200000000000000')
        rpaths = [bytes.fromhex(raw) for raw in (
            '1c000080200000000c0000002f7573722f6c69622f7377696674000000000000',
            '1c000080280000000c0000004065786563757461626c655f706174682f4672616d65776f726b7300',
            '1c000080300000000c0000004065786563757461626c655f706174682f2e2e2f2e2e2f4672616d65776f726b73000000',
        )]
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            observed = parser.macho(fixtures.mach_object(target, kind=2, load_commands=[weak, *rpaths]), helper)
            rows = [r for r in observed['loadCommands'] if r['command'] in (0x80000018, 0x8000001c)]
            self.assertEqual([r['payloadHex'] for r in rows], [c.hex() for c in [weak, *rpaths]])
            self.assertEqual(rows[0]['name'], '/usr/lib/swift/libswiftAVFoundation.dylib')
            self.assertTrue(rows[0]['weak'])
            self.assertEqual((rows[0]['timestamp'], rows[0]['currentVersionRaw'], rows[0]['compatibilityVersionRaw']),
                             struct.unpack_from('<3I', weak, 12))
            self.assertEqual([r['name'] for r in rows[1:]],
                             ['/usr/lib/swift', '@executable_path/Frameworks', '@executable_path/../../Frameworks'])

    def test_weak_dylib_and_rpath_refuse_malformed_strings_versions_kind_and_unknown_commands(self):
        fixtures = module('formal_dyld_negative_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_dyld_negative_parser', 'scripts/apple-carrier.py')
        helper = module('formal_dyld_negative_info', 'scripts/libbox-patches/android-source.py')
        def command(kind, name, offset=None):
            base = 24 if kind == 0x80000018 else 12
            size = (base + len(name) + 1 + 7) // 8 * 8
            head = struct.pack('<3I', kind, size, base if offset is None else offset)
            if kind == 0x80000018:
                head += struct.pack('<3I', 2, 0x10000, 0x10000)
            return head + name + bytes(size - base - len(name))
        for kind, valid in [(0x80000018, b'/usr/lib/swift/libswiftCoreFoundation.dylib'),
                            (0x8000001c, b'@executable_path/Frameworks')]:
            good = command(kind, valid)
            bads = [command(kind, valid, 0), command(kind, valid, len(good)), command(kind, valid, 32),
                    command(kind, valid + bytes(160)), command(kind, b''),
                    command(kind, b'\xff'), command(kind, b'/private/evil.dylib'),
                    command(kind, b'/usr/lib/swift/../evil.dylib'), command(kind, b'@rpath/evil.dylib')]
            no_nul = bytearray(good); no_nul[24 if kind == 0x80000018 else 12:] = b'x' * (len(good) - (24 if kind == 0x80000018 else 12)); bads.append(bytes(no_nul))
            padding = bytearray(good + bytes(8)); struct.pack_into('<I', padding, 4, len(padding)); bads.append(bytes(padding))
            trailing = bytearray(command(kind, valid + b'\0x')); bads.append(bytes(trailing))
            undersized = struct.pack('<4I', kind, 16, 12, 0); bads.append(undersized)
            unknown = bytearray(good); struct.pack_into('<I', unknown, 0, 0x80000017); bads.append(bytes(unknown))
            for bad in bads:
                with self.subTest(kind=kind, bad=bad.hex()), self.assertRaises(RuntimeError):
                    parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[bad]), helper)
            with self.assertRaisesRegex(RuntimeError, 'linked executable'):
                parser.macho(fixtures.mach_object('ios-arm64', load_commands=[good]), helper)
            if kind == 0x80000018:
                wrong = bytearray(good); struct.pack_into('<I', wrong, 16, 0)
                with self.assertRaisesRegex(RuntimeError, 'version relationship'):
                    parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[bytes(wrong)]), helper)
            else:
                with self.assertRaisesRegex(RuntimeError, 'duplicate executable rpath'):
                    parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[good, good]), helper)

    def test_observed_linker_option_command_bytes_are_parsed_and_recorded_for_object_only(self):
        fixtures = module('formal_linker_option_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_linker_option_parser', 'scripts/apple-carrier.py')
        helper = module('formal_linker_option_info', 'scripts/libbox-patches/android-source.py')
        # Exact command bytes observed in the original three slices (efe artifact).
        commands = {
            'Foundation': '2d00000028000000020000002d6672616d65776f726b00466f756e646174696f6e00000000000000',
            'CFNetwork': '2d00000028000000020000002d6672616d65776f726b0043464e6574776f726b0000000000000000',
            'Security': '2d00000020000000020000002d6672616d65776f726b00536563757269747900',
            'CoreFoundation': '2d00000028000000020000002d6672616d65776f726b00436f7265466f756e646174696f6e000000',
        }
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            raw = fixtures.mach_object(target, load_commands=[bytes.fromhex(value) for value in commands.values()])
            observed = parser.macho(raw, helper)
            actual = [row for row in observed['loadCommands'] if row['command'] == 0x2d]
            self.assertEqual(len(actual), 4)
            for row, (framework, payload) in zip(actual, commands.items()):
                self.assertEqual(row['count'], 2)
                self.assertEqual(row['options'], ['-framework', framework])
                self.assertEqual(row['payloadHex'], payload)
                self.assertLess(row['paddingBytes'], 8)
        foundation = bytes.fromhex(commands['Foundation'])
        controls = []
        for count in (0, 1, 3, 0xffffffff):
            bad = bytearray(foundation); struct.pack_into('<I', bad, 8, count); controls.append(bytes(bad))
        bad = bytearray(foundation); bad[-1] = 1; controls.append(bytes(bad))
        bad = bytearray(foundation); bad[12] = 0xff; controls.append(bytes(bad))
        controls.append(struct.pack('<3I', 0x2d, 40, 2) + b'x' * 28)  # no NUL
        for option in [b'-L\0Foundation\0', b'-framework\0/private/Foundation\0', b'-framework\0Unknown\0',
                       b'-framework\0../Foundation\0', b'-framework\0\0']:
            size = (12 + len(option) + 7) // 8 * 8
            controls.append(struct.pack('<3I', 0x2d, size, 2) + option + bytes(size - 12 - len(option)))
        controls.append(foundation + bytes(8))
        enlarged = bytearray(foundation + bytes(8)); struct.pack_into('<I', enlarged, 4, 48)
        controls.append(bytes(enlarged))
        for command in controls:
            with self.subTest(command=command.hex()), self.assertRaises(RuntimeError):
                parser.macho(fixtures.mach_object('ios-arm64', load_commands=[command]), helper)
        with self.assertRaisesRegex(RuntimeError, 'object/size'):
            parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[foundation]), helper)
        unknown = bytearray(foundation); struct.pack_into('<I', unknown, 0, 0x7ffffffe)
        with self.assertRaisesRegex(RuntimeError, 'unsupported load command'):
            parser.macho(fixtures.mach_object('ios-arm64', load_commands=[bytes(unknown)]), helper)


if __name__ == '__main__':
    unittest.main()
