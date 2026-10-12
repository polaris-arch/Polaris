"""Pure format, provenance, cache and rollback controls; no compiler/device runs."""
from copy import deepcopy
import io
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from fork_input import ROOT, canonical, load, sha
from artifacts import check_buildinfo, module_stream, tree_inventory, zip_entries
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


if __name__ == '__main__':
    unittest.main()
