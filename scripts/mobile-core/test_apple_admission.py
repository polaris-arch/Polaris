"""Resolved Xcode JSON controls; never starts Xcode or compiles a carrier."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import apple_admission as admission


def rows(sdk='iphonesimulator', arch='x86_64'):
    return [{'target': target, 'buildSettings': {'ARCHS': arch, 'VALID_ARCHS': 'arm64 x86_64',
             'EXCLUDED_ARCHS': 'x86_64' if sdk == 'iphoneos' and target == 'polaris_iOS' else '',
             'PLATFORM_NAME': sdk, 'SDKROOT': sdk, 'CONFIGURATION': 'release',
             'IPHONEOS_DEPLOYMENT_TARGET': '17.0', 'UNRELATED_ENV': 'DO_NOT_EXPORT'}}
            for target in admission.TARGETS]


class AppleAdmission(unittest.TestCase):
    def test_original_intel_arm64_only_negative_and_single_target_selection(self):
        original = rows(); original[0]['buildSettings']['VALID_ARCHS'] = 'arm64'
        with self.assertRaisesRegex(RuntimeError, 'architecture is excluded'):
            admission.checked_build_settings(original, 'iphonesimulator', 'x86_64', '17.0')
        positive = admission.checked_build_settings(rows(), 'iphonesimulator', 'x86_64', '17.0')
        self.assertEqual(set(positive), set(admission.TARGETS))
        self.assertNotIn('DO_NOT_EXPORT', json.dumps(positive))
        self.assertTrue(all(row['effectiveArchitectures'] == ['x86_64'] for row in positive.values()))

    def test_targets_platform_sdk_deployment_and_device_exclusion_all_required(self):
        cases = []
        cases.append(rows()[:1]); cases.append(rows()+rows()[:1])
        for field, value in [('ARCHS', 'arm64 x86_64'), ('VALID_ARCHS', '$(inherited)'),
                             ('EXCLUDED_ARCHS', 'x86_64'), ('PLATFORM_NAME', 'iphoneos'),
                             ('SDKROOT', '/wrong/iPhoneOS27.1.sdk'), ('CONFIGURATION', 'debug'),
                             ('IPHONEOS_DEPLOYMENT_TARGET', '15.0')]:
            sample = rows(); sample[1]['buildSettings'][field] = value; cases.append(sample)
        for sample in cases:
            with self.subTest(sample=sample), self.assertRaises(RuntimeError):
                admission.checked_build_settings(sample, 'iphonesimulator', 'x86_64', '17.0')
        device = rows('iphoneos', 'arm64')
        admission.checked_build_settings(device, 'iphoneos', 'arm64', '17.0')
        device[0]['buildSettings']['EXCLUDED_ARCHS'] = ''
        with self.assertRaisesRegex(RuntimeError, 'device Intel exclusion'):
            admission.checked_build_settings(device, 'iphoneos', 'arm64', '17.0')
        with self.assertRaises(RuntimeError):
            admission.checked_build_settings(rows('iphoneos'), 'iphoneos', 'x86_64', '17.0')

    def test_preflight_explicit_extension_queries_full_slot_set_and_source_stability(self):
        calls, saved = [], []
        def fake_run(command, **kwargs):
            calls.append((command, kwargs)); sdk = command[command.index('-sdk')+1]
            arch = next(value.split('=',1)[1] for value in command if str(value).startswith('ARCHS='))
            sample = rows(sdk, arch)
            return json.dumps(sample[1:] if '-target' in command else sample[:1])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            conf = root/'src-tauri/tauri.ios.conf.json'; conf.parent.mkdir(parents=True)
            conf.write_text(json.dumps({'bundle': {'iOS': {'minimumSystemVersion':'17.0'}}}))
            with patch.object(admission, 'ROOT', root), patch.object(admission, 'run', side_effect=fake_run), \
                    patch.object(admission, 'file_pin', return_value={'bytes':1,'sha256':'same'}), \
                    patch.object(admission, 'write_json', side_effect=lambda *args: saved.append(args)):
                observed = admission.preflight(root, {'DEVELOPER_DIR':'selected'})
            self.assertEqual(set(observed['slots']), {slot for slot,_,_ in admission.SLOTS})
            self.assertEqual(len(calls), 6)
            self.assertEqual(len(saved), 1); self.assertNotIn('DO_NOT_EXPORT', json.dumps(observed))
            for command, kwargs in calls:
                self.assertIn('-showBuildSettings', command); self.assertNotIn('archive', command)
                self.assertEqual(command[command.index('-configuration')+1], 'release')
                self.assertEqual(kwargs['env'], {'DEVELOPER_DIR':'selected'})
                if '-scheme' in command: self.assertIn('-destination', command)
                else: self.assertEqual(command[command.index('-target')+1], 'polaris_PacketTunnel')
            with patch.object(admission, 'ROOT', root), patch.object(admission, 'run', side_effect=fake_run), \
                    patch.object(admission, 'file_pin', side_effect=[{'bytes':1}]*3+[{'bytes':2}]*3), \
                    patch.object(admission, 'write_json') as write, self.assertRaisesRegex(RuntimeError,'inputs changed'):
                admission.preflight(root, {})
            write.assert_not_called()
