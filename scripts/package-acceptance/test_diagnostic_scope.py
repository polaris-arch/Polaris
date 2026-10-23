"""Local-only diagnostic writer contracts; no product/system/network operations."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('diagnostic_scope', Path(__file__).with_name('diagnostic_scope.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def environment():
    return {'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '39947115011',
            'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_JOB': 'linux_package_acceptance'}


class DiagnosticScopeTests(unittest.TestCase):
    def test_all_lanes(self):
        for lane, kind in (('plain', 'both'), ('tun', 'deb'), ('tun', 'AppImage')):
            with self.subTest(lane=lane, kind=kind):
                value = m.build_scope(environment(), lane, kind)
                self.assertEqual(value['schema'], m.SCHEMA)
                self.assertFalse(value['acceptance'])
                self.assertFalse(value['publicationEligible'])
                self.assertEqual(value['cleanup'], 'not-assessed')

    def test_preparation_failure_runtime_skipped(self):
        env = environment() | {'DIAGNOSTIC_TEST_TOOLS_OUTCOME': 'failure',
                               'DIAGNOSTIC_RUNTIME_OUTCOME': 'skipped'}
        value = m.build_scope(env, 'plain', 'both')
        self.assertEqual(value['runtime'], 'not-started')
        self.assertEqual(value['stepOutcomes']['test_tools'], 'failure')
        self.assertNotIn('result', value)

    def test_runtime_not_inferred_from_missing_or_failed_preparation(self):
        for outcome in ('', 'success', 'failure', 'cancelled'):
            value = m.build_scope(environment() | {'DIAGNOSTIC_RUNTIME_OUTCOME': outcome}, 'tun', 'deb')
            self.assertEqual(value['runtime'], 'attempted-or-unknown')
            self.assertEqual(value['cleanup'], 'not-assessed')
            self.assertFalse(value['acceptance'])

    def test_missing_steps_are_not_passes(self):
        value = m.build_scope(environment(), 'plain', 'both')
        self.assertEqual(set(value['stepOutcomes'].values()), {'not-recorded'})
        self.assertEqual(tuple(value['stepOutcomes']), m.STEPS)

    def test_reject_unrecognized_outcome(self):
        for bad in ('PASS', 'success\nsecret', {}, None, 'not-recorded'):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                m.build_scope(environment() | {'DIAGNOSTIC_RUNTIME_OUTCOME': bad}, 'plain', 'both')

    def test_no_unlisted_environment_export(self):
        env = environment() | {'GH_TOKEN': 'secret-value', 'GITHUB_EVENT_PATH': '/private/event',
                               'HOME': '/private/home', 'DIAGNOSTIC_EXTRA_OUTCOME': 'secret-value'}
        data = json.dumps(m.build_scope(env, 'plain', 'both'))
        for forbidden in ('secret-value', '/private', 'GH_TOKEN', 'GITHUB_EVENT_PATH', 'EXTRA'):
            self.assertNotIn(forbidden, data)

    def test_bad_identity_rejected_without_value_echo(self):
        for key in environment():
            for bad in ('secret\nvalue', '', None):
                with self.subTest(key=key, bad=bad), self.assertRaises(ValueError) as caught:
                    m.build_scope(environment() | {key: bad}, 'plain', 'both')
                self.assertNotIn('secret', str(caught.exception))

    def test_lane_kind_cross_product_rejected(self):
        for lane, kind in (('plain', 'deb'), ('tun', 'both'), ('unknown', 'both'), ('tun', '../deb')):
            with self.subTest(lane=lane, kind=kind), self.assertRaises(ValueError):
                m.build_scope(environment(), lane, kind)

    def test_file_does_not_create_runtime_receipt(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = m.build_scope(environment(), 'plain', 'both')
            output = m.write_scope(temp, scope)
            self.assertEqual(output.relative_to(temp).as_posix(), 'linux-package-evidence/scope.json')
            self.assertEqual(json.loads(output.read_text()), scope)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            self.assertEqual([p.name for p in output.parent.iterdir()], ['scope.json'])
            self.assertLess(output.stat().st_size, 8192)

    def test_existing_scope_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = m.build_scope(environment(), 'tun', 'AppImage')
            output = m.write_scope(temp, scope)
            before = output.read_bytes()
            with self.assertRaises(FileExistsError):
                m.write_scope(temp, scope)
            self.assertEqual(output.read_bytes(), before)

    def test_directory_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); (root / 'outside').mkdir()
            (root / 'linux-tun-evidence').symlink_to(root / 'outside', target_is_directory=True)
            with self.assertRaises(ValueError):
                m.write_scope(root, m.build_scope(environment(), 'tun', 'deb'))
            self.assertEqual(list((root / 'outside').iterdir()), [])

    def test_output_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); directory = root / 'linux-package-evidence'; directory.mkdir()
            (directory / 'scope.json').symlink_to(root / 'must-not-exist')
            with self.assertRaises(FileExistsError):
                m.write_scope(root, m.build_scope(environment(), 'plain', 'both'))
            self.assertFalse((root / 'must-not-exist').exists())


if __name__ == '__main__':
    unittest.main()
