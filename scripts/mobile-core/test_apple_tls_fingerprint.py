import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import apple_tls_fingerprint as subject


class OptionalTLSCapabilityLink(unittest.TestCase):
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
            self.assertIn('apple-admission.mjs', text)
            self.assertIn('--tls-link verify-installed', text)
            self.assertIn('--tls-link install', text)
            self.assertNotIn(' prepare', text)


if __name__ == '__main__':
    unittest.main()
