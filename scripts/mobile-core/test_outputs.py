"""Pure format, provenance, cache and rollback controls; no compiler/device runs."""
from copy import deepcopy
import io
import json
import os
from pathlib import Path
import struct
import plistlib
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from fork_input import ROOT, canonical, load, sha
from artifacts import check_buildinfo, framework_entry_targets, inspect_framework, module, module_stream, tree_inventory, zip_entries
from android_sdk import prepare_view, revision_from_properties, observe_ndk_input
from budget import reservation
from ci_android import emit_outputs, prepare_source
from producer import equivalent_binding
from transaction import install, recover


class FormalOutputs(unittest.TestCase):
    def setUp(self):
        self.value = load()

    def facts(self, apple=False, simulator=False):
        tags = self.value['apple' if apple else 'android']['tags'][:]
        if apple:
            tags += ['ios', *(['iossimulator'] if simulator else [])]
        return {'goVersion': '1.25.5', 'settings': {'GOOS': 'ios' if apple else 'android',
            'GOARCH': 'arm64', 'CGO_ENABLED': '1', '-buildmode': 'c-archive' if apple else 'c-shared',
            '-trimpath': 'true', '-tags': ','.join(tags)}, 'modules': {'example.org/module':
            {'kind': 'dep', 'version': 'v1.2.3', 'sum': 'h1:fixed'}, self.value['goModule']:
            {'kind': 'dep', 'version': 'v0.0.0-00010101000000-000000000000', 'replacement': '/verified/fork'}}}

    def check(self, facts, **kwargs):
        check_buildinfo(facts, self.value, [{'Path': 'example.org/module', 'Version': 'v1.2.3', 'Sum': 'h1:fixed'},
                                         {'Path': self.value['goModule'], 'Main': True, 'Dir': '/verified/fork'}],
                        os_name=kwargs.pop('os_name', 'android'), arch='arm64', mode=kwargs.pop('mode', 'c-shared'),
                        expected_id='polaris-mobile-' + 'a' * 64, actual_id=kwargs.pop('actual_id', 'polaris-mobile-' + 'a' * 64),
                        **kwargs)

    def test_trimpath_does_not_invent_absent_ldflags(self):
        self.check(self.facts())

    def test_actual_buildid_cannot_reuse_cli_or_other_input(self):
        for bad in ['', 'CLI-BuildID', 'polaris-mobile-' + 'b' * 64]:
            with self.subTest(id=bad), self.assertRaises(RuntimeError):
                self.check(self.facts(), actual_id=bad)

    def test_all_native_settings_and_required_feature_tags_are_checked(self):
        for field, bad in [('GOOS', 'linux'), ('GOARCH', 'amd64'), ('CGO_ENABLED', '0'),
                           ('-buildmode', 'exe'), ('-trimpath', 'false'), ('-tags', 'android')]:
            value = self.facts()
            value['settings'][field] = bad
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.check(value)

    def test_simulator_tag_cannot_appear_in_device_or_android(self):
        self.check(self.facts(True), os_name='ios', mode='c-archive')
        self.check(self.facts(True, True), os_name='ios', mode='c-archive', simulator=True)
        for apple, sim, kwargs in [(True, True, {'os_name': 'ios', 'mode': 'c-archive'}),
                                   (True, False, {'os_name': 'ios', 'mode': 'c-archive', 'simulator': True}),
                                   (False, False, {})]:
            facts = self.facts(apple, sim)
            if not apple:
                facts['settings']['-tags'] += ',android'
            with self.subTest(apple=apple, simulator=sim), self.assertRaises(RuntimeError):
                self.check(facts, **kwargs)

    def test_module_version_h1_and_replacements_cannot_drift(self):
        for key, bad in [('version', 'v9.9.9'), ('sum', 'h1:other'), ('replacement', '/unowned/source')]:
            facts = self.facts()
            facts['modules']['example.org/module'][key] = bad
            with self.subTest(field=key), self.assertRaises(RuntimeError):
                self.check(facts)
        facts = self.facts()
        facts['modules']['undeclared/module'] = {'kind': 'dep', 'version': 'v1.0.0'}
        with self.assertRaises(RuntimeError):
            self.check(facts)

    def test_module_stream_requires_unique_actual_rows(self):
        self.assertEqual(module_stream('{"Path":"a"}\n {"Path":"b"}'), [{'Path': 'a'}, {'Path': 'b'}])
        for bad in ['', '{"Path":"a"}{"Path":"a"}', '[]', '{"Version":"v1"}']:
            with self.subTest(raw=bad), self.assertRaises(RuntimeError):
                module_stream(bad)

    def test_framework_internal_links_and_empty_resources_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'Versions/A').mkdir(parents=True)
            (root / 'Versions/A/Libbox').write_bytes(b'static original')
            (root / 'Versions/A/empty').write_bytes(b'')
            (root / 'Versions/Current').symlink_to('A', target_is_directory=True)
            (root / 'Libbox').symlink_to('Versions/Current/Libbox')
            rows = tree_inventory(root)
            self.assertTrue(any(r.get('target') == 'Versions/Current/Libbox' for r in rows))
            (root / 'escape').symlink_to('/etc/passwd')
            with self.assertRaises(RuntimeError):
                tree_inventory(root)

    def test_zip_rejects_duplicates_escape_and_links(self):
        for names, mode in [(['../escape'], None), (['/absolute'], None), (['a', 'a'], None), (['link'], 0o120777)]:
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as bundle:
                for name in names:
                    item = zipfile.ZipInfo(name)
                    if mode:
                        item.external_attr = mode << 16
                    bundle.writestr(item, b'fixture')
            with zipfile.ZipFile(io.BytesIO(stream.getvalue())) as bundle, self.assertRaises(RuntimeError):
                zip_entries(bundle)

    def test_cache_reuse_preserves_original_producer_commit_only(self):
        original = {'producer': {'candidate': 'old', 'files': {'code': {'sha256': 'same'}}}, 'tools': {'go': 'fixed'}}
        current = deepcopy(original)
        current['producer']['candidate'] = 'new'
        self.assertTrue(equivalent_binding(original, current))
        current['tools']['go'] = 'different'
        self.assertFalse(equivalent_binding(original, current))
        current = deepcopy(original)
        current['producer']['files']['code']['sha256'] = 'changed'
        self.assertFalse(equivalent_binding(original, current))

    def test_pair_validation_failure_restores_both_original_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            pairs = []
            for name in ('component', 'receipt'):
                a, b = root / ('stage-' + name), root / name
                a.write_bytes(('new-' + name).encode())
                b.write_bytes(('old-' + name).encode())
                pairs.append((a, b))
            def fail():
                raise RuntimeError('late actual content rejection')
            with self.assertRaisesRegex(RuntimeError, 'late actual'):
                install(pairs, root, fail)
            for _, path in pairs:
                self.assertEqual(path.read_bytes(), ('old-' + path.name).encode())
            self.assertTrue((root / 'rejected-pair/component').exists())
            self.assertTrue((root / 'recovered-transaction.json').exists())

    def test_interrupted_pair_recovery_does_not_touch_equal_unmoved_old_object(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage, dest = root / 'stage', root / 'component'
            stage.write_bytes(b'same')
            dest.write_bytes(b'same')
            pin = {'bytes': 4, 'sha256': sha(b'same')}
            (root / 'component-transaction.json').write_bytes(canonical({'schema': 'polaris-mobile-pair-transaction-v1',
                'entries': [{'stage': str(stage), 'destination': str(dest), 'name': dest.name,
                             'stagePin': pin, 'originalPin': pin}]}))
            recover([(stage, dest)], root)
            self.assertEqual(dest.read_bytes(), b'same')
            self.assertFalse((root / 'rejected-pair').exists())

    def test_occupied_recovery_slot_is_preserved_without_new_install(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'previous-pair').mkdir()
            unique = root / 'previous-pair/unique-object'
            unique.write_bytes(b'original')
            stage = root / 'stage'
            stage.write_bytes(b'new')
            with self.assertRaises(RuntimeError):
                install([(stage, root / 'component')], root, lambda: None)
            self.assertEqual(unique.read_bytes(), b'original')
            self.assertEqual(stage.read_bytes(), b'new')

    def test_source_fetch_is_fixed_tag_and_checked_before_handoff(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'source'
            calls = []
            with patch('ci_android.run', side_effect=lambda args: calls.append([str(x) for x in args])), \
                    patch('ci_android.verify_source', return_value={'sourceOverlay': 0}) as verify:
                prepare_source(source, self.value)
            ref = 'refs/tags/' + self.value['sourceTag']
            self.assertEqual(calls[1], ['git', '-C', str(source), 'fetch', '--depth=1', '--no-tags',
                                      self.value['sourceURL'], ref + ':' + ref])
            verify.assert_called_once_with(source, self.value)
            for error in ['tag', 'object', 'commit', 'tree', 'module', 'dirty', 'overlay', 'generator']:
                with self.subTest(error=error), patch('ci_android.run'), \
                        patch('ci_android.verify_source', side_effect=RuntimeError(error)), self.assertRaises(RuntimeError):
                    prepare_source(source, self.value)

    def test_cache_output_refuses_invalid_identity_before_any_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            out, env = root / 'output', root / 'env'
            for p in (out, env):
                p.write_text('retained\n')
            with patch.dict(os.environ, {'GITHUB_OUTPUT': str(out), 'GITHUB_ENV': str(env)}):
                for bad in ['', 'A' * 64, 'g' * 64, 'a' * 63, 'a' * 65]:
                    with self.subTest(fp=bad), self.assertRaises(RuntimeError):
                        emit_outputs({'work': '/work', 'cache': '/cache'}, bad, root)
                    self.assertEqual(out.read_text(), 'retained\n')
                with self.assertRaises(RuntimeError):
                    emit_outputs({'work': '/work\nINJECT=1', 'cache': '/cache'}, 'a' * 64, root)
                self.assertEqual(env.read_text(), 'retained\n')

    def test_shared_budget_does_not_sell_the_same_free_bytes_twice(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            value = deepcopy(self.value)
            value['budget']['buildPeakReserveBytes'] = 1024
            active = []
            with patch.dict(os.environ, {'GITHUB_ACTIONS': '', 'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                    patch('budget.os.statvfs', return_value=type('Space', (), {'f_bavail': 1, 'f_frsize': 2048})()), \
                    self.assertRaises(RuntimeError):
                with reservation(root / 'work', value, active):
                    self.fail('no peak/system margin')
            self.assertEqual(active, [])

    def test_hosted_admission_has_real_descriptor_and_nested_child_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); root.chmod(0o700)
            value = deepcopy(self.value); value['budget']['buildPeakReserveBytes'] = 1024
            active, nested = [], []
            with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true',
                    'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                    patch('budget.os.statvfs', return_value=type('Space', (), {'f_bavail': 10**12, 'f_frsize': 1})()):
                with reservation(root / 'work', value, active):
                    fd = int(os.environ['POLARIS_BUILD_RESERVATION_FD'])
                    self.assertEqual(active, [fd])
                    self.assertTrue(os.get_inheritable(fd))
                    row = json.loads(os.pread(fd, 8192, 0))
                    self.assertEqual(row['reserveBytes'], 1024)
                    self.assertEqual(row['filesystem'], (root / 'work').stat().st_dev)
                    with reservation(root / 'child-work', value, nested):
                        self.assertEqual(nested, [fd])
                        self.assertEqual(int(os.environ['POLARIS_BUILD_RESERVATION_FD']), fd)
                    self.assertEqual(nested, [])
                    lock = root / 'budget' / row['slot']; lock.chmod(0o644)
                    try:
                        with self.assertRaisesRegex(RuntimeError, 'identity differs'):
                            with reservation(root / 'child-work', value, nested):
                                self.fail('unprotected inherited descriptor admitted')
                    finally:
                        lock.chmod(0o600)
                self.assertEqual(active, [])
                self.assertNotIn('POLARIS_BUILD_RESERVATION_FD', os.environ)
                with self.assertRaises(OSError):
                    os.fstat(fd)

    def test_actual_ndk_properties_spacing_keeps_exact_pin_and_rejects_duplicate_or_other_revision(self):
        # Matches the observed fixed archive's full revision and distinct base revision.
        self.assertEqual(revision_from_properties('Pkg.Desc = Android NDK\nPkg.Revision = 30.0.16138531-beta3\n'),
                         self.value['android']['ndkSourceRevision'])
        self.assertEqual(revision_from_properties('Pkg.Revision=30.0.16138531\n'), '30.0.16138531')
        for bad in ['Pkg.Revision = 30.0.16138531\nPkg.Revision=30.0.16138531',
                    'Pkg.Revision=30.0.16138531\n Pkg.Revision = 27.3.13750724',
                    'Pkg.Revision = 30.0.16138531 trailing', 'Pkg.Desc = Android NDK']:
            with self.subTest(properties=bad), self.assertRaises(RuntimeError):
                revision_from_properties(bad)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            work = root / 'work'; work.mkdir(mode=0o700)
            sdk, ndk = root / 'sdk', root / 'ndk'; ndk.mkdir()
            profile = deepcopy(self.value['android'])
            for name in ['platforms/' + profile['bootclasspath']['platform'], 'build-tools', 'platform-tools']:
                (sdk / name).mkdir(parents=True)
            jar = sdk / 'platforms' / profile['bootclasspath']['platform'] / 'android.jar'
            jar.write_bytes(b'actual controlled bootclasspath')
            profile['bootclasspath']['sha256'] = sha(jar.read_bytes())
            properties = b'Pkg.Desc = Android NDK\nPkg.Revision = 30.0.16138531-beta3\nPkg.BaseRevision = 30.0.16138531\nPkg.ReleaseName = r30-beta3\n'
            (ndk / 'source.properties').write_bytes(properties)
            observation = observe_ndk_input(root, ndk, profile)
            self.assertEqual((root / 'ndk-source.properties').read_bytes(), properties)
            self.assertEqual(observation['revision'], profile['ndkSourceRevision'])
            self.assertEqual(observation['baseRevision'], profile['ndkRevision'])
            self.assertFalse(observation['publicationEligible'])
            view, actual_jar = prepare_view(work, sdk, ndk, profile)
            self.assertEqual(actual_jar, jar)
            self.assertEqual((view / 'ndk' / profile['ndkRevision']).resolve(), ndk)
            (ndk / 'source.properties').write_text('Pkg.Revision = 27.3.13750724\n')
            with self.assertRaisesRegex(RuntimeError, 'revision/base/release differs'):
                prepare_view(work, sdk, ndk, profile)
            with self.assertRaisesRegex(RuntimeError, 'observation differs'):
                observe_ndk_input(root, ndk, profile)
            self.assertEqual((root / 'ndk-source.properties').read_bytes(), properties)
            for bad in [properties.replace(b'-beta3', b'-beta2'),
                        properties.replace(b'Pkg.BaseRevision = 30.0.16138531', b'Pkg.BaseRevision = 30.0.0'),
                        properties.replace(b'r30-beta3', b'r30-rc1'),
                        properties.replace(b'Pkg.Revision = 30.0.16138531-beta3', b'Pkg.Revision = 30.0.16138531')]:
                (ndk / 'source.properties').write_bytes(bad)
                with self.assertRaisesRegex(RuntimeError, 'revision/base/release differs'):
                    prepare_view(work, sdk, ndk, profile)
            (ndk / 'source.properties').write_bytes(properties)
            jar.write_bytes(b'wrong bootclasspath')
            with self.assertRaisesRegex(RuntimeError, 'bootclasspath differs'):
                prepare_view(work, sdk, ndk, profile)

    def test_framework_declares_exact_device_and_simulator_architectures(self):
        device = {'SupportedPlatform': 'ios', 'SupportedArchitectures': ['arm64']}
        simulator = {**device, 'SupportedPlatformVariant': 'simulator', 'SupportedArchitectures': ['x86_64', 'arm64']}
        self.assertEqual(framework_entry_targets(device), {'ios-arm64'})
        self.assertEqual(framework_entry_targets(simulator), {'ios-arm64-simulator', 'ios-x86_64-simulator'})
        for bad in [{**device, 'SupportedArchitectures': []}, {**device, 'SupportedArchitectures': ['arm64', 'x86_64']},
                    {**simulator, 'SupportedArchitectures': ['arm64']},
                    {**simulator, 'SupportedArchitectures': ['arm64', 'arm64']},
                    {**simulator, 'SupportedArchitectures': ['arm64', 'x86_64', 'armv7']},
                    {**device, 'SupportedPlatform': 'macos'}]:
            with self.subTest(entry=bad), self.assertRaises(RuntimeError):
                framework_entry_targets(bad)

    def test_framework_content_predicate_accepts_single_device_fat_and_exact_two_simulator_targets(self):
        fixtures = module('formal_mobile_framework_fixtures', 'scripts/apple-carrier.test.py')
        from test_apple_entry import single_fat_archive
        raw_info = 'path\tsynthetic/main\nmod\tgithub.com/sagernet/sing-box\t(devel)\t\n'
        expected_id = 'polaris-mobile-' + 'a' * 64
        def archive(target):
            return fixtures.archive([('go.o', fixtures.mach_object(target, fixtures.inline(raw_info), expected_id)),
                                     ('native.o', fixtures.mach_object(target))])
        device = single_fat_archive(archive('ios-arm64'))
        simulator = fixtures.fat(archive('ios-x86_64-simulator'), archive('ios-arm64-simulator'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework = root / 'Libbox.xcframework'; framework.mkdir()
            entries = []
            for identifier, variant, arches, raw in [('ios-arm64', None, ['arm64'], device),
                    ('ios-arm64_x86_64-simulator', 'simulator', ['arm64', 'x86_64'], simulator)]:
                entry = {'LibraryIdentifier': identifier, 'LibraryPath': 'Libbox.framework',
                         'SupportedPlatform': 'ios', 'SupportedArchitectures': arches}
                if variant: entry['SupportedPlatformVariant'] = variant
                entries.append(entry)
                lib = framework / identifier / 'Libbox.framework'; (lib / 'Resources').mkdir(parents=True)
                (lib / 'Headers').mkdir()
                (lib / 'Libbox').write_bytes(raw)
                (lib / 'Resources/Info.plist').write_bytes(plistlib.dumps({}))
                symbols = ['NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
                    'CheckConfigWithResult', 'ConfigValidationContractVersion', 'exportTailscaleStoreRetirement',
                    'tailscaleStoreRetirement', 'tailscaleStoreMembership', 'wakeNow', 'recordScreenState']
                (lib / 'Headers/Libbox.h').write_text('\n'.join(symbols))
            info = framework / 'Info.plist'; info.write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
            with patch('artifacts.check_buildinfo') as buildinfo:
                output = inspect_framework(framework, self.value, [], expected_id)
                self.assertEqual(set(output['slices']), set(self.value['apple']['slices']))
                self.assertEqual(buildinfo.call_count, 3)
                entries[1]['SupportedArchitectures'] = ['arm64']
                info.write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
                with self.assertRaisesRegex(RuntimeError, 'declared architecture inventory'):
                    inspect_framework(framework, self.value, [], expected_id)
                entries[1]['SupportedArchitectures'] = ['arm64', 'x86_64']
                info.write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
                sim = framework / entries[1]['LibraryIdentifier'] / 'Libbox.framework/Libbox'
                sim.write_bytes(single_fat_archive(archive('ios-arm64-simulator')))
                with self.assertRaisesRegex(RuntimeError, 'static slice inventory'):
                    inspect_framework(framework, self.value, [], expected_id)
                sim.write_bytes(simulator)
                bad = bytearray(device); struct.pack_into('>I', bad, 8, 0x1000007)
                (framework / entries[0]['LibraryIdentifier'] / 'Libbox.framework/Libbox').write_bytes(bad)
                with self.assertRaisesRegex(RuntimeError, 'header/payload CPU'):
                    inspect_framework(framework, self.value, [], expected_id)
                bad_count = bytearray(device); struct.pack_into('>I', bad_count, 4, 3)
                device_path = framework / entries[0]['LibraryIdentifier'] / 'Libbox.framework/Libbox'
                device_path.write_bytes(bad_count)
                with self.assertRaisesRegex(RuntimeError, 'fatCount=3') as rejected:
                    inspect_framework(framework, self.value, [], expected_id)
                message = str(rejected.exception)
                self.assertIn('path=' + str(device_path), message)
                self.assertIn('header=' + bytes(bad_count[:16]).hex(), message)
                self.assertIn("declaredArchitectures=['arm64']", message)


if __name__ == '__main__':
    unittest.main()
