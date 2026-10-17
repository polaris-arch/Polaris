"""Inert controls for per-invocation original-byte reuse; no native build."""
from collections import Counter
from contextlib import contextmanager
from copy import deepcopy
from pathlib import Path
import json
import os
import plistlib
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import artifacts
import carriers
from fork_input import load, sha
from artifacts import file_pin, module, read_pinned


class AppleReadReuse(unittest.TestCase):
    def test_fixed_failure_notes_do_not_require_a_newer_exception_api(self):
        class LegacyFailure(RuntimeError):
            def add_note(self, message):
                raise AssertionError('newer exception API must not be required')
        primary = LegacyFailure('original parse error')
        primary.__notes__ = ['existing fixed observation']
        notes = primary.__notes__
        artifacts.failure_note(primary, 'fixed follow-up observation')
        self.assertIs(primary.__notes__, notes)
        self.assertEqual(notes, ['existing fixed observation', 'fixed follow-up observation'])
        self.assertEqual(str(primary), 'original parse error')
        # Even malformed pre-existing note metadata cannot mask the failure.
        primary.__notes__ = 'opaque existing metadata'
        artifacts.failure_note(primary, 'fixed follow-up observation')
        self.assertEqual(primary.__notes__, 'opaque existing metadata')
        self.assertEqual(str(primary), 'original parse error')

    def test_parse_failure_remains_primary_when_observation_channel_breaks(self):
        primary = RuntimeError('controlled parse failure')
        observer = BrokenPipeError('controlled observer failure')
        with patch('producer.inspection_progress', side_effect=observer) as progress:
            with self.assertRaises(RuntimeError) as raised:
                with artifacts.inspection_stage('ipa-parse'):
                    raise primary
        self.assertIs(raised.exception, primary)
        self.assertEqual(raised.exception.__notes__,
                         ['Apple inspection observation failed; original failure preserved'])
        self.assertNotIn('controlled observer failure', str(raised.exception.__notes__))
        progress.assert_called_once()

    def test_cancellation_remains_primary_when_observation_channel_breaks(self):
        for primary in (KeyboardInterrupt(), SystemExit(130)):
            with self.subTest(kind=type(primary).__name__), \
                    patch('producer.inspection_progress', side_effect=BrokenPipeError('controlled observer failure')):
                with self.assertRaises(type(primary)) as raised:
                    with artifacts.inspection_stage('static-linkmap', {'fileReads': 1, 'readBytes': 7}):
                        raise primary
                self.assertIs(raised.exception, primary)
                self.assertEqual(raised.exception.__notes__,
                                 ['Apple inspection observation failed; original failure preserved'])

    def test_observation_only_failure_preserves_existing_failure_policy(self):
        observer = BrokenPipeError('controlled observer failure')
        with patch('producer.inspection_progress', side_effect=observer):
            with self.assertRaises(BrokenPipeError) as raised:
                with artifacts.inspection_stage('framework-inspection', {'fileReads': 2, 'readBytes': 9}):
                    pass
        self.assertIs(raised.exception, observer)
        self.assertFalse(hasattr(raised.exception, '__notes__'))

    def test_handled_outer_exception_does_not_suppress_observation_only_failure(self):
        observer = BrokenPipeError('controlled observer failure')
        try:
            raise ValueError('already handled outside inspection')
        except ValueError:
            with patch('producer.inspection_progress', side_effect=observer):
                with self.assertRaises(BrokenPipeError) as raised:
                    with artifacts.inspection_stage('ipa-parse'):
                        pass
            self.assertIs(raised.exception, observer)
            self.assertFalse(hasattr(raised.exception, '__notes__'))

    def link_fixture(self, root):
        framework = root / 'Libbox.xcframework'
        lib = framework / 'ios-arm64/Libbox.framework'
        (lib / 'Headers').mkdir(parents=True)
        binary = lib / 'Libbox'; binary.write_bytes(b'controlled archive bytes')
        header = lib / 'Headers/Libbox.h'; header.write_bytes(b'controlled header bytes')
        derived = root / 'derived'; derived.mkdir()
        selected = {'libraryIdentifier': 'ios-arm64', 'binary': file_pin(binary),
                    'headerSha256': {'Libbox.h': sha(header.read_bytes())},
                    'members': [{'name': 'go.o', 'ordinal': 1, 'goCarrier': True},
                                {'name': 'native.o', 'ordinal': 2, 'goCarrier': False}]}
        return framework, binary, header, derived, selected

    def link_text(self, rows):
        return '# Object files:\n' + '\n'.join(
            f'[ {ordinal}] {binary}({name})' for ordinal, binary, name in rows
        ) + '\n# Sections:\n# Symbols:\n'

    def test_members_share_only_canonical_file_reads_with_final_boundary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework, binary, header, derived, selected = self.link_fixture(root)
            text = self.link_text([(i + 1, binary, 'go.o' if i % 2 else 'native.o') for i in range(80)])
            calls = Counter()
            def pin(path, **kwargs):
                calls[Path(path)] += 1
                return file_pin(path, **kwargs)
            with patch('carriers.file_pin', side_effect=pin):
                rows = carriers.static_linkmap(text, framework, selected, derived)
            self.assertEqual(len(rows), 80)
            self.assertEqual(calls, Counter({binary: 2, header: 2}))
            self.assertEqual(rows[0]['archivePin'], selected['binary'])
            self.assertEqual([r['linkOrdinal'] for r in rows], list(range(1, 81)))

    def test_same_basename_in_distinct_derived_frameworks_never_reuses_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework, binary, header, derived, selected = self.link_fixture(root)
            other = derived / 'different/Libbox.framework/Libbox'
            other.parent.mkdir(parents=True)
            (other.parent / 'Headers').mkdir()
            other.write_bytes(b'different archive bytes')
            (other.parent / 'Headers/Libbox.h').write_bytes(header.read_bytes())
            text = self.link_text([(1, binary, 'go.o'), (2, other, 'native.o')])
            with self.assertRaisesRegex(RuntimeError, 'different/unbound'):
                carriers.static_linkmap(text, framework, selected, derived)
            other.write_bytes(binary.read_bytes())
            (other.parent / 'Headers/Libbox.h').write_bytes(b'different header bytes')
            with self.assertRaisesRegex(RuntimeError, 'headers changed'):
                carriers.static_linkmap(text, framework, selected, derived)

    def test_final_boundary_rejects_archive_or_header_changed_after_first_use(self):
        for changed in ('binary', 'header'):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temporary:
                framework, binary, header, derived, selected = self.link_fixture(Path(temporary).resolve())
                text = self.link_text([(1, binary, 'go.o'), (2, binary, 'native.o')])
                mutated = False
                def pin(path, **kwargs):
                    nonlocal mutated
                    result = file_pin(path, **kwargs)
                    if Path(path) == header and not mutated:
                        mutated = True
                        (binary if changed == 'binary' else header).write_bytes(b'changed after first read')
                    return result
                with patch('carriers.file_pin', side_effect=pin), \
                        self.assertRaisesRegex(RuntimeError, 'bytes changed during member'):
                    carriers.static_linkmap(text, framework, selected, derived)

    def test_reuse_preserves_per_line_bounds_members_coverage_and_ordinals(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework, binary, header, derived, selected = self.link_fixture(root)
            outside = root / 'unowned/Libbox.framework/Libbox'
            outside.parent.mkdir(parents=True); outside.write_bytes(binary.read_bytes())
            controls = [
                ([(1, binary, 'go.o'), (2, outside, 'native.o')], 'different/unbound'),
                ([(1, binary, 'go.o'), (2, binary, 'unknown.o')], 'different/unbound'),
                ([(1, binary, 'go.o'), (1, binary, 'native.o')], 'ordinals duplicate'),
                ([(1, binary, 'go.o')], 'generated/native'),
                ([(1, binary, 'native.o')], 'Go carrier'),
            ]
            for rows, message in controls:
                with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                    carriers.static_linkmap(self.link_text(rows), framework, selected, derived)

    def framework_fixture(self, root):
        fixtures = module('read_reuse_framework_fixture', 'scripts/apple-carrier.test.py')
        from test_apple_entry import single_fat_archive
        framework = root / 'Libbox.xcframework'; framework.mkdir()
        expected_id = 'polaris-mobile-' + 'a' * 64
        def archive(target):
            return fixtures.archive([
                ('go.o', fixtures.mach_object(target, fixtures.inline('path\tsynthetic/main\nmod\tgithub.com/sagernet/sing-box\t(devel)\t\n'), expected_id)),
                ('native.o', fixtures.mach_object(target))])
        device = single_fat_archive(archive('ios-arm64'))
        sim = single_fat_archive(archive('ios-arm64-simulator'))
        entries, binaries, headers = [], [], []
        symbols = ['NewManagedStrictCommandServer', 'exportTailscaleDirectoryCapability',
                   'activateTailscaleDirectoryDomain', 'exportTailscaleDirectoryRetirement',
                   'NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
                   'CheckConfigWithResult', 'ConfigValidationContractVersion', 'exportTailscaleStoreRetirement',
                   'wakeNow', 'recordScreenState']
        getters = ['Cleanup', 'CleanupError', 'ConfigDigest', 'ContractVersion', 'RequestID',
                   'TailscaleStoreMembership', 'TailscaleStoreRetirement', 'Validation', 'ValidationError']
        header = '\n'.join(symbols) + '\n@interface LibboxConfigValidationResult : NSObject <goSeqRefInterface> {}\n'
        header += '\n'.join('- (NSString* _Nonnull)get' + name + ';' for name in getters) + '\n@end\n'
        for identifier, variant, arches, raw in [
                ('ios-arm64', None, ['arm64'], device),
                ('ios-arm64-simulator', 'simulator', ['arm64'], sim)]:
            entry = {'LibraryIdentifier': identifier, 'LibraryPath': 'Libbox.framework',
                     'SupportedPlatform': 'ios', 'SupportedArchitectures': arches}
            if variant: entry['SupportedPlatformVariant'] = variant
            entries.append(entry)
            lib = framework / identifier / 'Libbox.framework'
            (lib / 'Resources').mkdir(parents=True); (lib / 'Headers').mkdir()
            (lib / 'Resources/Info.plist').write_bytes(plistlib.dumps({}))
            binary = lib / 'Libbox'; binary.write_bytes(raw); binaries.append(binary)
            h = lib / 'Headers/Libbox.h'; h.write_text(header); headers.append(h)
        (framework / 'Info.plist').write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
        return framework, binaries, headers, expected_id

    def test_framework_fat_parse_and_pin_share_raw_and_final_inventory_is_fresh(self):
        with tempfile.TemporaryDirectory() as temporary:
            framework, binaries, headers, expected_id = self.framework_fixture(Path(temporary).resolve())
            reads = Counter()
            def read(path, **kwargs):
                reads[Path(path)] += 1
                return read_pinned(path, **kwargs)
            with patch('artifacts.read_pinned', side_effect=read), patch('artifacts.check_buildinfo'):
                result = artifacts.inspect_framework(framework, load(), [], expected_id)
            for path in binaries + headers:
                self.assertEqual(reads[path], 2, 'first parse/pin plus final fresh boundary, not per-slice hashing')
            sim_arm = result['slices']['ios-arm64-simulator']['binary']
            self.assertEqual(set(result['slices']), {'ios-arm64', 'ios-arm64-simulator'})
            self.assertEqual(sim_arm, file_pin(binaries[1]))
            inventory = {r['path']: r for r in result['inventory']}
            self.assertEqual(sim_arm, {k: inventory[binaries[1].relative_to(framework).as_posix()][k]
                                      for k in ('bytes', 'sha256')})

    def test_framework_final_boundary_rejects_binary_header_mode_or_layout_change(self):
        for changed in ('binary', 'header', 'mode', 'layout'):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temporary:
                framework, binaries, headers, expected_id = self.framework_fixture(Path(temporary).resolve())
                mutated = False
                def buildinfo(*args, **kwargs):
                    nonlocal mutated
                    if mutated: return
                    mutated = True
                    if changed == 'binary': binaries[0].write_bytes(b'changed binary')
                    elif changed == 'header': headers[0].write_bytes(b'changed header')
                    elif changed == 'mode': os.chmod(binaries[0], binaries[0].stat().st_mode ^ 0o100)
                    else: (framework / 'unexpected').write_bytes(b'added')
                with patch('artifacts.check_buildinfo', side_effect=buildinfo), \
                        self.assertRaisesRegex(RuntimeError, 'framework .*changed during'):
                    artifacts.inspect_framework(framework, load(), [], expected_id)

    def test_bounded_read_does_not_pin_growing_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'artifact'; path.write_bytes(b'first')
            original = Path.open
            def opened(p, *args, **kwargs):
                with original(path, 'wb') as stream:
                    stream.write(b'grew after size observation')
                return original(p, *args, **kwargs)
            with patch.object(Path, 'open', new=opened), \
                    self.assertRaisesRegex(RuntimeError, 'size changed'):
                read_pinned(path)

    @contextmanager
    def final_carrier_fixture(self, root, target, *, sdk_version=None):
        """Real ZIP/plist/Mach-O/linkmap predicates; source/component boundary is isolated."""
        value = load()
        simulator = target != 'ios-arm64'
        arch = 'arm64'
        if sdk_version is not None:
            value['apple']['simulatorSDK' if simulator else 'iPhoneSDK'] = sdk_version
        sdk = carriers.apple_version_raw(value['apple']['simulatorSDK' if simulator else 'iPhoneSDK'])
        fixtures = module('final_native_platform_fixture', 'scripts/apple-carrier.test.py')
        framework, binary, header, derived, selected = self.link_fixture(root)
        current = {'candidate': 'a' * 40}
        binding = {'producer': current, 'moduleGraph': [{'Path': value['goModule'], 'Main': True}]}
        build_id = 'polaris-mobile-' + artifacts.receipt_digest(binding)
        tags = set(value['apple']['tags']) | {'ios'} | ({'iossimulator'} if simulator else set())
        raw_info = 'path\tsynthetic/main\nmod\t' + value['goModule'] + '\t(devel)\t\n'
        raw_info += ''.join('build\t' + key + '=' + val + '\n' for key, val in {
            'GOOS': 'ios', 'GOARCH': arch, 'CGO_ENABLED': '1', '-buildmode': 'c-archive',
            '-trimpath': 'true', '-tags': ','.join(sorted(tags))}.items())
        def native_binary(role, *, load_commands=()):
            return fixtures.mach_object(target,
                fixtures.inline(raw_info) if role == 'extension' else None,
                build_id if role == 'extension' else None,
                kind=2, minimum=17 << 16, sdk=sdk, load_commands=load_commands)
        native = {role: native_binary(role) for role in ('app', 'extension')}
        receipt = {'input': binding, 'output': {'slices': {target: selected}}, 'fingerprint': 'b' * 64}
        receipt_path = root / 'component-receipt.json'; receipt_path.write_text(json.dumps(receipt))
        link = {'schema': 'polaris-mobile-apple-link-input-v1', 'staticOnly': True,
                'App': current, 'resources': {}, 'framework': str(framework),
                'componentFingerprint': receipt['fingerprint'], 'componentOutput': receipt['output'],
                'componentReceiptSha256': file_pin(receipt_path)['sha256']}
        link['fingerprint'] = artifacts.receipt_digest(link)
        link_path = root / 'link.json'; link_path.write_text(json.dumps(link))
        link_sha = file_pin(link_path)['sha256']
        linkmap = root / 'linkmap.txt'
        linkmap.write_text(self.link_text([(1, binary, 'go.o'), (2, binary, 'native.o')]))
        app_root = 'Payload/Polaris.app/'
        extension_root = app_root + 'PlugIns/PacketTunnel.appex/'
        common = {'UIDeviceFamily': [1, 2], 'MinimumOSVersion': '17.0',
                  'CFBundleSupportedPlatforms': ['iPhoneSimulator' if simulator else 'iPhoneOS'],
                  'PolarisAppGroup': 'group.example.polaris'}
        plists = {app_root + 'Info.plist': plistlib.dumps({**common, 'CFBundleIdentifier': 'example.polaris',
                                                        'CFBundleExecutable': 'Polaris'}),
                  extension_root + 'Info.plist': plistlib.dumps({**common,
                      'CFBundleIdentifier': 'example.polaris.PacketTunnel', 'CFBundleExecutable': 'PacketTunnel',
                      'NSExtension': {'NSExtensionPointIdentifier': 'com.apple.networkextension.packet-tunnel'}})}
        paths = {'app': app_root + 'Polaris', 'extension': extension_root + 'PacketTunnel'}
        ipa = root / 'final.ipa'
        def inspect(binaries=None, info=None):
            with zipfile.ZipFile(ipa, 'w') as package:
                for path, raw in (plists if info is None else info).items(): package.writestr(path, raw)
                for role, raw in (native if binaries is None else binaries).items(): package.writestr(paths[role], raw)
            return carriers.apple(ipa, link_path, link_sha, linkmap, receipt_path, root, derived,
                                  arch if simulator else None)
        with patch('carriers.verify_original', return_value=receipt) as original, \
                patch('carriers.app_identity', return_value=current), \
                patch('carriers.apple_resources', return_value={}), patch('carriers.load', return_value=value):
            yield {'inspect': inspect, 'native': native, 'plists': plists, 'ipa': ipa, 'original': original,
                   'receipt': receipt, 'link': link, 'target': target, 'native_binary': native_binary}

    def test_final_carrier_refuses_unsupported_arch_before_reading_any_input(self):
        for arch in ('amd64', 'x86_64', 'armv7', '', 'ARM64'):
            with self.subTest(arch=arch), patch('carriers.file_pin') as pin, \
                    patch('carriers.verify_original') as original, \
                    self.assertRaisesRegex(RuntimeError, 'only device-arm64 and sim-arm64'):
                carriers.apple(None, None, None, None, None, None, None, arch)
            pin.assert_not_called()
            original.assert_not_called()

    def test_final_native_admission_accepts_exact_two_slots_and_preserves_scope(self):
        self.assertEqual(set(artifacts.APPLE), {'ios-arm64', 'ios-arm64-simulator'})
        for target in artifacts.APPLE:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary, \
                    self.final_carrier_fixture(Path(temporary).resolve(), target) as fixture:
                observed = fixture['inspect']()
                self.assertEqual(observed['selectedTarget'], target)
                self.assertTrue(observed['AppCarrierValidated'])
                self.assertFalse(observed['AppAccepted'] or observed['devicePassed'] or observed['signatureTrustValidated'])
                self.assertEqual(set(observed['binaries']), {'app', 'extension'})
                self.assertEqual(len(observed['linkedOriginalStaticMembers']), 2)
                for role, raw in fixture['native'].items():
                    self.assertEqual(observed['binaries'][role]['sha256'], sha(raw))
                fixture['original'].assert_called_once()

    def test_final_app_and_extension_reject_native_only_platform_minimum_and_sdk_mutations(self):
        parser = module('final_native_platform_parser', 'scripts/apple-carrier.py')
        helper = module('final_native_platform_info', 'scripts/libbox-patches/android-source.py')
        for target, (_, platform, _, _) in artifacts.APPLE.items():
            with tempfile.TemporaryDirectory() as temporary, \
                    self.final_carrier_fixture(Path(temporary).resolve(), target) as fixture:
                frozen = deepcopy((fixture['receipt'], fixture['link']))
                for role, raw in fixture['native'].items():
                    parsed = parser.container(raw, helper)
                    offset = next(row['offset'] for row in parsed['loadCommands'] if row['command'] == 0x32)
                    controls = [('platform', 8, 7 if platform == 2 else 2),
                                ('missing', 0, 0x1b),  # Same-width LC_UUID keeps all section/Go bytes intact.
                                ('component-minimum', 12, 15 << 16), ('higher-minimum', 12, 18 << 16),
                                ('minimum-patch', 12, (17 << 16) | 1),
                                ('missing-sdk', 16, 0), ('older-sdk', 16, 27 << 16),
                                ('sdk-patch', 16, (27 << 16) | (1 << 8) | 1)]
                    for label, field, changed in controls:
                        mutated = bytearray(raw); struct.pack_into('<I', mutated, offset + field, changed)
                        binaries = dict(fixture['native'], **{role: bytes(mutated)})
                        # Confirm this control changed native metadata alone,
                        # preserving the extension's source metadata and BuildID.
                        self.assertEqual(parser.container(bytes(mutated), helper)['sections'], parsed['sections'])
                        with self.subTest(target=target, role=role, mutation=label), \
                                self.assertRaisesRegex(RuntimeError, 'actual ' + role + ' native platform/deployment/SDK differs'):
                            fixture['inspect'](binaries)
                        with zipfile.ZipFile(fixture['ipa']) as package:
                            self.assertEqual({name: package.read(name) for name in fixture['plists']}, fixture['plists'])
                    # Every rejection, including reusing this call boundary,
                    # still goes through original component verification.
                self.assertEqual(fixture['original'].call_count, 2 * len(controls))
                self.assertEqual((fixture['receipt'], fixture['link']), frozen)

    def test_final_app_and_extension_refuse_intel_native_bytes_with_arm64_declarations(self):
        for target in artifacts.APPLE:
            with tempfile.TemporaryDirectory() as temporary, \
                    self.final_carrier_fixture(Path(temporary).resolve(), target) as fixture:
                for role, raw in fixture['native'].items():
                    changed = bytearray(raw)
                    struct.pack_into('<2I', changed, 4, 0x1000007, 3)
                    binaries = dict(fixture['native'], **{role: bytes(changed)})
                    with self.subTest(target=target, role=role), \
                            self.assertRaisesRegex(RuntimeError, 'selected thin MH_EXECUTE'):
                        fixture['inspect'](binaries)
                self.assertEqual(fixture['original'].call_count, 2)

    def test_final_app_and_extension_refuse_duplicate_native_platform_commands(self):
        for target, (_, platform, _, _) in artifacts.APPLE.items():
            with tempfile.TemporaryDirectory() as temporary, \
                    self.final_carrier_fixture(Path(temporary).resolve(), target) as fixture:
                duplicate = struct.pack('<6I', 0x32, 24, platform, 17 << 16, (27 << 16) | (1 << 8), 0)
                for role in fixture['native']:
                    binaries = dict(fixture['native'], **{
                        role: fixture['native_binary'](role, load_commands=[duplicate])})
                    with self.subTest(target=target, role=role), self.assertRaisesRegex(RuntimeError, 'duplicate platform command'):
                        fixture['inspect'](binaries)

    def test_final_plist_minimum_and_platform_still_must_match_current_admission(self):
        for target in artifacts.APPLE:
            with tempfile.TemporaryDirectory() as temporary, \
                    self.final_carrier_fixture(Path(temporary).resolve(), target) as fixture:
                for path, raw in fixture['plists'].items():
                    for field, bad in [('MinimumOSVersion', '15.0'), ('MinimumOSVersion', '18.0'),
                                       ('CFBundleSupportedPlatforms', ['iPhoneSimulator' if target == 'ios-arm64' else 'iPhoneOS'])]:
                        changed = plistlib.loads(raw); changed[field] = bad
                        info = dict(fixture['plists'], **{path: plistlib.dumps(changed)})
                        with self.subTest(target=target, path=path, field=field, bad=bad), \
                                self.assertRaisesRegex(RuntimeError, 'actual device carrier scope differs'):
                            fixture['inspect'](info=info)

    def test_final_native_sdk_uses_selected_current_contract_including_patch(self):
        # Deliberately distinguish device/simulator SDK declarations, rather
        # than letting today's equal versions hide a wrong field selection.
        for target in artifacts.APPLE:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary, \
                    self.final_carrier_fixture(Path(temporary).resolve(), target, sdk_version='27.2.3') as fixture:
                self.assertTrue(fixture['inspect']()['AppCarrierValidated'])

    def test_apple_version_encoding_keeps_minor_patch_and_rejects_unrepresentable_values(self):
        self.assertEqual(carriers.apple_version_raw('17.0'), 17 << 16)
        self.assertEqual(carriers.apple_version_raw('17.0.0'), 17 << 16)
        self.assertEqual(carriers.apple_version_raw('27.2.3'), (27 << 16) | (2 << 8) | 3)
        for value in ('17', '17.0.0.1', '17.-1', '17.256', '65536.0', '0.0', '17.0beta', None):
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                carriers.apple_version_raw(value)


if __name__ == '__main__':
    unittest.main()
