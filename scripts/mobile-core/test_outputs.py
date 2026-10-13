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
from artifacts import check_buildinfo, component_output_identity, framework_entry_targets, inspect_framework, module, module_stream, objc_validation_getter_contract, tree_inventory, zip_entries
from android_sdk import prepare_view, revision_from_properties, observe_ndk_input
from budget import reservation
from ci_android import emit_outputs, prepare_source, prepare
from producer import compiler_fact, equivalent_binding, executable_driver_identity
from transaction import install, recover
import artifacts
from producer import write_json


class FormalOutputs(unittest.TestCase):
    def setUp(self):
        self.value = load()

    def test_large_apple_receipts_round_trip_original_inventory_and_fingerprint(self):
        # Model the real producer/consumer boundary, not native acceptance.
        # The full JSON is larger than the former 16MiB reader envelope.
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'receipt.json'
            for schema in ('polaris-mobile-component-v1', 'polaris-mobile-apple-link-input-v1'):
                value = {'schema': schema, 'platform': 'apple',
                         'originalInventoryObservation': 'x' * (16 * 1024 ** 2),
                         'AppAccepted': False, 'devicePassed': False}
                value['fingerprint'] = sha(canonical(value))
                write_json(path, value)
                self.assertGreater(path.stat().st_size, artifacts.MAX_RECEIPT)
                self.assertEqual(artifacts.read_receipt(path), value)
                self.assertEqual(value['fingerprint'], sha(canonical({k: v for k, v in value.items() if k != 'fingerprint'})))

    def test_receipt_budgets_refuse_unknown_android_oversize_and_preserve_prior_bytes(self):
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(artifacts, 'MAX_RECEIPT', 256), patch.object(artifacts, 'MAX_APPLE_RECEIPT', 1024):
            path = Path(temporary) / 'receipt.json'
            original = b'{"schema":"small-original"}\n'; path.write_bytes(original)
            for value in [{'schema': 'unknown'}, {'schema': 'polaris-mobile-component-v1', 'platform': 'android'},
                          {'schema': 'polaris-mobile-component-v1', 'platform': 'apple', 'payload': 'x' * 1024}]:
                value.setdefault('payload', 'x' * 256)
                with self.assertRaisesRegex(RuntimeError, 'before installation'):
                    write_json(path, value)
                self.assertEqual(path.read_bytes(), original)
                self.assertFalse(path.with_name('receipt.json.tmp').exists())
                path.write_text(json.dumps(value))
                with self.assertRaises(RuntimeError):
                    artifacts.read_receipt(path)
                path.write_bytes(original)
            for invalid in [b'[]', b'{"a":1,"a":2}', b'{"a":NaN}', b'']:
                path.write_bytes(invalid)
                with self.assertRaises(RuntimeError):
                    artifacts.read_receipt(path)
            path.write_bytes(original)
            link = path.with_name('link.json'); link.symlink_to(path.name)
            with self.assertRaisesRegex(RuntimeError, 'bounded regular'):
                artifacts.read_receipt(link)

    def validation_result_objc_header(self):
        # Declaration spelling/receiver/return type from the preserved original
        # ac6 Libbox.objc.h (129711B, e743b050...), not the former bare-name stub.
        selectors = ['getCleanup', 'getCleanupError', 'getConfigDigest', 'getContractVersion', 'getRequestID',
                     'getTailscaleStoreMembership', 'getTailscaleStoreRetirement', 'getValidation', 'getValidationError']
        return ('@interface LibboxConfigValidationResult : NSObject <goSeqRefInterface> {\n}\n'
                '@property(strong, readonly) _Nonnull id _ref;\n' +
                '\n'.join('- (NSString* _Nonnull)' + name + ';' for name in selectors) + '\n@end\n')

    def test_objc_validation_getters_require_exact_result_receiver_types_and_readonly_selectors(self):
        header = self.validation_result_objc_header()
        facts = objc_validation_getter_contract(header)
        self.assertEqual(facts['receiver'], 'LibboxConfigValidationResult')
        self.assertEqual(len(facts['getters']), 9)
        self.assertEqual(objc_validation_getter_contract(header.replace('\n', ' ')), facts,
                         'valid declaration formatting cannot depend on newlines')
        getter = '- (NSString* _Nonnull)getTailscaleStoreRetirement;'
        membership = '- (NSString* _Nonnull)getTailscaleStoreMembership;'
        for wrong in [
            header.replace(getter, ''), header.replace(membership, ''),
            header.replace(getter, '// ' + getter), header.replace(getter, '/* ' + getter + ' */'),
            header.replace(getter, getter.replace('getTailscaleStoreRetirement', 'tailscaleStoreRetirement')),
            header.replace(getter, getter.replace('NSString* _Nonnull', 'int64_t')),
            header.replace(getter, getter.replace('_Nonnull', '_Nullable')),
            header.replace(getter, getter.replace(';', ': (NSString*)argument;')),
            header.replace(getter, getter.replace('- ', '+ ')),
            header.replace(getter, getter + '\n' + getter),
            header.replace(getter, getter + ' ' + getter),
            header.replace(getter, getter + ' - (void)setTailscaleStoreRetirement:(NSString*)value;'),
            header.replace('LibboxConfigValidationResult', 'LibboxOtherReceiver'),
            header.replace('goSeqRefInterface', 'OtherProtocol'), header + header,
            header.replace('@end', '- (void)setTailscaleStoreRetirement:(NSString*)value;\n@end'),
            header.replace('@end', '@property(strong) NSString* tailscaleStoreMembership;\n@end'),
        ]:
            with self.subTest(header=wrong[:150]), self.assertRaises(RuntimeError):
                objc_validation_getter_contract(wrong)
        wrong_receiver = header.replace(getter, '') + ('@interface LibboxOtherReceiver : NSObject\n' + getter + '\n@end\n')
        with self.assertRaises(RuntimeError):
            objc_validation_getter_contract(wrong_receiver)

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

    def test_prepare_refusal_retains_ndk_inputs_without_success_outputs(self):
        properties = b'Pkg.Desc = Android NDK\nPkg.Revision = 30.0.16138531-beta3\nPkg.BaseRevision = 30.0.16138531\nPkg.ReleaseName = r30-beta3\n'
        # Execute the real prepare/observe path, with all external tool operations
        # controlled. Both admission refusal and a later source refusal happen
        # before emit_outputs, so the work output cannot be the evidence root.
        for admission_refuses in [True, False]:
            with self.subTest(admission_refuses=admission_refuses), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                ndk = root / 'ndk'; ndk.mkdir()
                raw = properties.replace(b'-beta3', b'-beta2') if admission_refuses else properties
                (ndk / 'source.properties').write_bytes(raw)
                jdk = root / 'jdk'; jdk.mkdir()
                sdk = root / 'sdk'; sdk.mkdir()
                output, environment = root / 'github-output', root / 'github-env'

                def controlled_run(args, **kwargs):
                    command = [str(arg) for arg in args]
                    if command == ['which', 'go']:
                        return '/controlled/go'
                    if command == ['/controlled/go', 'version']:
                        return 'go version go' + self.value['goVersion'] + ' linux/amd64'
                    if command == ['/controlled/go', 'install',
                            'github.com/sagernet/gomobile/cmd/gomobile@' + self.value['gomobileVersion'],
                            'github.com/sagernet/gomobile/cmd/gobind@' + self.value['gobindVersion']]:
                        return ''
                    if len(command) == 3 and command[1:] == [str(ROOT / 'scripts/libbox-patches/prepare-ndk-linux.py'), str(root)]:
                        return str(ndk)
                    self.fail('unexpected external operation: ' + repr(command))

                reason = 'revision/base/release differs' if admission_refuses else 'controlled source refusal'
                with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', 'JAVA_HOME_17_X64': str(jdk),
                        'ANDROID_SDK_ROOT': str(sdk), 'GITHUB_OUTPUT': str(output), 'GITHUB_ENV': str(environment)}, clear=True), \
                        patch('ci_android.location', return_value=root), \
                        patch('ci_android.producer_code', return_value={'files': []}), \
                        patch('ci_android.run', side_effect=controlled_run), \
                        patch('ci_android.prepare_source', side_effect=RuntimeError('controlled source refusal')) as source, \
                        patch('ci_android.emit_outputs') as emit, self.assertRaisesRegex(RuntimeError, reason):
                    prepare()
                emit.assert_not_called()
                if admission_refuses:
                    source.assert_not_called()
                else:
                    source.assert_called_once()
                self.assertFalse(output.exists())
                self.assertFalse(environment.exists())
                self.assertFalse((root / 'tools.json').exists())
                self.assertEqual((root / 'ndk-source.properties').read_bytes(), raw)
                observation = json.loads((root / 'sdk-input-observation.json').read_bytes())
                self.assertEqual(observation['sourceProperties']['sha256'], sha(raw))
                self.assertEqual(observation['revision'], revision_from_properties(raw.decode()))
                self.assertEqual(observation['requiredRevision'], self.value['android']['ndkSourceRevision'])
                self.assertFalse(observation['devicePassed'])
                self.assertFalse(observation['publicationEligible'])

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
                    'wakeNow', 'recordScreenState']
                (lib / 'Headers/Libbox.h').write_text('\n'.join(symbols) + '\n' + self.validation_result_objc_header())
            info = framework / 'Info.plist'; info.write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
            with patch('artifacts.check_buildinfo') as buildinfo:
                output = inspect_framework(framework, self.value, [], expected_id)
                self.assertEqual(set(output['slices']), set(self.value['apple']['slices']))
                self.assertEqual(buildinfo.call_count, 3)
                for row in output['slices'].values():
                    self.assertEqual(row['validationResultGetterContract']['receiver'], 'LibboxConfigValidationResult')
                original_header = framework / entries[0]['LibraryIdentifier'] / 'Libbox.framework/Headers/Libbox.h'
                text = original_header.read_text()
                original_header.write_text(text.replace('- (NSString* _Nonnull)getTailscaleStoreRetirement;',
                                                       '// - (NSString* _Nonnull)getTailscaleStoreRetirement;'))
                with self.assertRaisesRegex(RuntimeError, 'immutable getter'):
                    inspect_framework(framework, self.value, [], expected_id)
                original_header.write_text(text)
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

    def test_llvm_symlink_drivers_keep_invocation_name_and_bind_actual_physical_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            drivers = {'ld.lld': 'lld', 'clang++': 'clang', 'llvm-ar': 'llvm-ar-physical',
                       'llvm-readelf': 'llvm-readobj'}
            for name, physical_name in drivers.items():
                physical = root / physical_name
                physical.write_bytes(('inert physical ' + physical_name).encode())
                driver = root / name; driver.symlink_to(physical_name)
                calls = []
                def observe(command, *, env):
                    calls.append(command)
                    self.assertEqual(command, [str(driver), '--version'])
                    self.assertEqual(env, {'controlled': 'input'})
                    return name + ' controlled version'
                with patch('producer.run', side_effect=observe):
                    fact = compiler_fact(driver, {'controlled': 'input'})
                self.assertEqual(calls, [[str(driver), '--version']])
                self.assertEqual(fact['path'], str(physical))
                self.assertEqual(fact['driverPath'], str(driver))
                self.assertEqual(fact['driverLinkTarget'], physical_name)
                self.assertEqual(fact['sha256'], sha(physical.read_bytes()))
                self.assertEqual(fact['bytes'], physical.stat().st_size)
                self.assertEqual(fact['version'], name + ' controlled version')

    def test_compiler_observation_refuses_retargeted_alias_or_changed_physical_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            physical, replacement, driver = root / 'lld', root / 'other-lld', root / 'ld.lld'
            physical.write_bytes(b'inert same physical'); replacement.write_bytes(physical.read_bytes())
            driver.symlink_to('lld')
            def retarget(*args, **kwargs):
                driver.unlink(); driver.symlink_to('other-lld')
                return 'controlled version'
            with patch('producer.run', side_effect=retarget), self.assertRaisesRegex(RuntimeError, 'changed during observation'):
                compiler_fact(driver, {})
            driver.unlink(); driver.symlink_to('lld')
            def change(*args, **kwargs):
                physical.write_bytes(b'changed actual physical')
                return 'controlled version'
            with patch('producer.run', side_effect=change), self.assertRaisesRegex(RuntimeError, 'changed during observation'):
                compiler_fact(driver, {})
            driver.unlink(); driver.symlink_to('missing-lld')
            with self.assertRaises(OSError):
                executable_driver_identity(driver)

    def relocated_android_observations(self):
        name = 'jni/arm64-v8a/libbox.so'
        body = '\tpath\tgobind\n\tdep\texample.org/module\tv1.2.3\th1:fixed\n'
        original = {'format': 'AAR', 'artifact': {'bytes': 123, 'sha256': 'a' * 64},
                    'nativeLibraries': {name: {'bytes': 42, 'sha256': 'b' * 64,
                        'goBuildInfo': '/producer/scratch/jni_arm64-v8a_libbox.so: go1.25.5\n' + body,
                        'parsedBuildInfo': self.facts(), 'buildID': 'original-build-id',
                        'jniExports': ['JNI_OnLoad'], 'needed': ['libc.so']}},
                    'javaInterfaces': {'Libbox': 'original API'}}
        observed = deepcopy(original)
        observed['nativeLibraries'][name]['goBuildInfo'] = '/consumer/work/jni_arm64-v8a_libbox.so: go1.25.5\n' + body
        return name, original, observed

    def test_component_identity_ignores_only_the_exact_go_invocation_filename_and_preserves_raw(self):
        name, original, observed = self.relocated_android_observations()
        frozen = deepcopy(original), deepcopy(observed)
        self.assertNotEqual(original, observed)
        self.assertEqual(component_output_identity(original), component_output_identity(observed))
        self.assertEqual((original, observed), frozen, 'original stdout and receipt observations must stay intact')
        for mutate in [
            lambda v: v['artifact'].update(sha256='c' * 64),
            lambda v: v['nativeLibraries'][name].update(bytes=43),
            lambda v: v['nativeLibraries'][name].update(buildID='other-build-id'),
            lambda v: v['nativeLibraries'][name].update(jniExports=[]),
            lambda v: v['nativeLibraries'][name].update(needed=['libother.so']),
            lambda v: v['nativeLibraries'][name]['parsedBuildInfo']['settings'].update(GOARCH='amd64'),
            lambda v: v['nativeLibraries'][name]['parsedBuildInfo']['modules']['example.org/module'].update(sum='h1:other'),
            lambda v: v['nativeLibraries'][name].update(goBuildInfo=v['nativeLibraries'][name]['goBuildInfo'].replace('h1:fixed', 'h1:other')),
            lambda v: v['javaInterfaces'].update(Libbox='changed API'),
            lambda v: v.update(unboundNull=None),
        ]:
            wrong = deepcopy(observed); mutate(wrong)
            self.assertNotEqual(component_output_identity(original), component_output_identity(wrong))
        for header in ['relative/jni_arm64-v8a_libbox.so: go1.25.5',
                       '/consumer/work/other.so: go1.25.5',
                       '/consumer/work/jni_arm64-v8a_libbox.so: go1.25.6']:
            wrong = deepcopy(observed)
            wrong['nativeLibraries'][name]['goBuildInfo'] = header + '\n\tpath\tgobind\n'
            with self.subTest(header=header), self.assertRaises(RuntimeError):
                component_output_identity(wrong)
        bad = deepcopy(observed); bad['nativeLibraries'][name]['goBuildInfo'] += '\t=>\t./other-source\t(devel)\n'
        self.assertNotEqual(component_output_identity(original), component_output_identity(bad))
        framework = {'format': 'static-XCFramework', 'goBuildInfo': {'moduleRaw': 'raw original'}}
        self.assertEqual(component_output_identity(framework), framework)

    def test_consumer_and_producer_reinspection_keep_the_original_receipt_identity(self):
        from consume import verify_original
        from producer import verify_receipt
        name, original, observed = self.relocated_android_observations()
        binding = {'platform': 'android', 'sourceContract': self.value, 'sourceOverlay': 0,
                   'sourceObservation': {'sourceCommit': self.value['sourceCommit'], 'sourceTree': self.value['sourceTree']},
                   'moduleGraph': []}
        receipt = {'schema': 'polaris-mobile-component-v1', 'platform': 'android', 'input': binding,
                   'output': original, 'nativeInputs': {}, 'sourceOverlay': 0, 'AppAccepted': False, 'devicePassed': False}
        receipt['fingerprint'] = sha(canonical(receipt))
        frozen = deepcopy(receipt)
        tools = {'go': 'actual-go', 'javap': 'actual-javap', 'readelf': 'actual-readelf'}
        with patch('consume.load', return_value=self.value), patch('consume.read_receipt', return_value=receipt), \
                patch('consume.inspect_aar', return_value=observed), patch('native_inputs.verify'), \
                patch('producer.inspect_aar', return_value=observed):
            self.assertIs(verify_original('/component.aar', '/receipt.json', 'android', tools, Path('/consumer/work')), receipt)
            self.assertIs(verify_receipt('/component.aar', receipt, self.value, binding, tools, None, Path('/consumer/work')), receipt)
            self.assertEqual(receipt, frozen)
            observed['nativeLibraries'][name]['buildID'] = 'changed-native-id'
            with self.assertRaisesRegex(RuntimeError, 'native identity differ; fields=.*nativeLibraries.*originalRawHeaders='):
                verify_original('/component.aar', '/receipt.json', 'android', tools, Path('/consumer/work'))
            with self.assertRaisesRegex(RuntimeError, 'cached component native/content'):
                verify_receipt('/component.aar', receipt, self.value, binding, tools, None, Path('/consumer/work'))

    def test_source_owned_local_replacement_accepts_only_coherent_declared_literal_or_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source = root / 'fork'; directory = source / 'third_party/sing-tun'; directory.mkdir(parents=True)
            name = 'github.com/sagernet/sing-tun'
            version = 'v0.9.7-0.20261009022811-5c2edb183cc9'
            graph = [{'Path': 'example.org/module', 'Version': 'v1.2.3', 'Sum': 'h1:fixed'},
                     {'Path': self.value['goModule'], 'Main': True, 'Dir': str(source)},
                     {'Path': name, 'Version': version, 'Replace': {'Path': './third_party/sing-tun', 'Dir': str(directory)}}]
            facts = self.facts(); facts['modules'][self.value['goModule']]['replacement'] = str(source)
            def check(observed, declared=graph):
                check_buildinfo(observed, self.value, declared, os_name='android', arch='arm64', mode='c-shared',
                                expected_id='polaris-mobile-' + 'a' * 64, actual_id='polaris-mobile-' + 'a' * 64)
            for alias in ('./third_party/sing-tun', str(directory)):
                for replacement_version in ('', '(devel)'):
                    facts['modules'][name] = {'kind': 'dep', 'version': version, 'replacement': alias,
                                             'replacementVersion': replacement_version}
                    frozen = deepcopy(facts)
                    check(facts)
                    self.assertEqual(facts, frozen, 'native BuildInfo must remain its original observed values')
            for wrong in ('third_party/sing-tun', '../third_party/sing-tun', './third_party/sing-tun/',
                          './third_party/../third_party/sing-tun', str(root / 'elsewhere/sing-tun')):
                facts['modules'][name]['replacement'] = wrong
                with self.subTest(path=wrong), self.assertRaisesRegex(RuntimeError, 'observed=.*declaredPath=.*declaredDir='):
                    check(facts)
            facts['modules'][name]['replacement'] = './third_party/sing-tun'
            for change in [('version', 'v9.0.0'), ('replacementVersion', 'v0.1.0'),
                           ('replacementVersion', 'devel'), ('replacementSum', 'h1:undeclared')]:
                wrong = deepcopy(facts); wrong['modules'][name][change[0]] = change[1]
                with self.subTest(change=change), self.assertRaises(RuntimeError):
                    check(wrong)
            wrong = deepcopy(graph); wrong[-1]['Replace']['Dir'] = str(root / 'outside')
            with self.assertRaisesRegex(RuntimeError, 'Path/Dir disagree'):
                check(facts, wrong)
            for path in ('./../outside', './third_party/../sing-tun', '/absolute', './third_party//sing-tun'):
                wrong = deepcopy(graph); wrong[-1]['Replace']['Path'] = path
                with self.subTest(declared=path), self.assertRaises(RuntimeError):
                    check(facts, wrong)
            with self.assertRaisesRegex(RuntimeError, 'native module not in actual source graph'):
                check(facts, graph[:-1])
            outside = root / 'outside'; outside.mkdir()
            directory.rmdir(); directory.symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, 'escaped verified source root'):
                check(facts)

    def test_remote_versioned_replacement_retains_exact_path_version_and_h1(self):
        facts = self.facts()
        name = 'example.org/module'
        graph = [{'Path': name, 'Version': 'v1.2.3',
                  'Replace': {'Path': 'example.org/replaced', 'Version': 'v1.4.5', 'Sum': 'h1:replaced'}},
                 {'Path': self.value['goModule'], 'Main': True, 'Dir': '/verified/fork'}]
        facts['modules'][name].update(replacement='example.org/replaced', replacementVersion='v1.4.5',
                                      replacementSum='h1:replaced')
        def check(observed):
            check_buildinfo(observed, self.value, graph, os_name='android', arch='arm64', mode='c-shared',
                            expected_id='polaris-mobile-' + 'a' * 64, actual_id='polaris-mobile-' + 'a' * 64)
        check(facts)
        for key, bad in [('replacement', './example.org/replaced'), ('replacementVersion', 'v9.0.0'),
                         ('replacementSum', 'h1:other'), ('replacementVersion', '(devel)'), ('version', 'v9.0.0')]:
            wrong = deepcopy(facts); wrong['modules'][name][key] = bad
            with self.subTest(field=key), self.assertRaises(RuntimeError):
                check(wrong)


if __name__ == '__main__':
    unittest.main()
