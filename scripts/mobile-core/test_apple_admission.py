"""Resolved Xcode JSON controls; never starts Xcode or compiles a carrier."""
import json
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

import apple_admission as admission


def rows(sdk='iphonesimulator', arch='arm64'):
    return [{'target': target, 'buildSettings': {'ARCHS': arch, 'VALID_ARCHS': 'arm64',
             'EXCLUDED_ARCHS': 'x86_64' if sdk == 'iphoneos' and target == 'polaris_iOS' else '',
             'PLATFORM_NAME': sdk, 'SDKROOT': sdk, 'CONFIGURATION': 'release',
             'IPHONEOS_DEPLOYMENT_TARGET': '17.0', 'UNRELATED_ENV': 'DO_NOT_EXPORT'}}
            for target in admission.TARGETS]


class AppleAdmission(unittest.TestCase):
    def test_extension_source_declares_same_two_slot_arm64_architecture_contract(self):
        project = (admission.ROOT / 'src-tauri/gen/apple/project.yml').read_text()
        target = project.split('  polaris_PacketTunnel:\n', 1)[1].split('    info:\n', 1)[0]
        self.assertIn('\n        VALID_ARCHS: arm64\n', target)
        self.assertIn('\n        VALID_ARCHS[sdk=iphonesimulator*]: arm64\n', target)
        native = (admission.ROOT / 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj').read_text()
        configurations = [(name, block) for name, block in re.findall(
            r'^\t\t[A-F0-9]{24} /\* (debug|release) \*/ = \{\n(.*?)^\t\t\};', native, re.M | re.S)
            if 'PRODUCT_BUNDLE_IDENTIFIER = "$(POLARIS_BUNDLE_ID).PacketTunnel";' in block]
        self.assertEqual(sorted(name for name, _ in configurations), ['debug', 'release'])
        for _, block in configurations:
            self.assertIn('\n\t\t\t\tVALID_ARCHS = arm64;\n', block)
            self.assertIn('\n\t\t\t\t"VALID_ARCHS[sdk=iphonesimulator*]" = arm64;\n', block)

    def test_rejected_architecture_reports_closed_exact_field_without_other_settings(self):
        for key in ('ARCHS', 'VALID_ARCHS', 'EXCLUDED_ARCHS'):
            for value, reason in [('$(inherited)', 'unknown-token'), ('arm64 arm64', 'duplicate-token')]:
                sample = rows(); sample[1]['buildSettings'][key] = value
                with self.subTest(field=key, value=value), self.assertRaises(RuntimeError) as rejected:
                    admission.checked_build_settings(sample, 'iphonesimulator', 'arm64', '17.0')
                detail = json.loads(str(rejected.exception).split(': ', 1)[1])
                self.assertEqual(detail['target'], 'polaris_PacketTunnel')
                self.assertEqual(detail['field'], key); self.assertEqual(detail['reason'], reason)
                self.assertEqual(detail['observed']['value'], value)
                self.assertNotIn('DO_NOT_EXPORT', str(rejected.exception))

    def test_failure_observation_keeps_architecture_before_rejection_and_withholds_paths(self):
        sample = rows('iphoneos', 'arm64')[1:]
        sample[0]['buildSettings']['VALID_ARCHS'] = '$(inherited)'
        saved = []
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(admission, 'run', return_value=json.dumps(sample)), \
                patch.object(admission, 'write_json', side_effect=lambda path, value: saved.append((path, value))):
            root = Path(temporary)
            observed = admission._read_rows(['xcodebuild'], {}, 'device-arm64', root, 'extension')
            with self.assertRaisesRegex(RuntimeError, 'unresolved Apple architecture setting'):
                admission.checked_build_settings(observed, 'iphoneos', 'arm64', '17.0',
                                                 expected_targets=('polaris_PacketTunnel',))
        self.assertEqual(saved[0][0].name, 'build-settings-device-arm64-extension.json')
        self.assertEqual(saved[0][1]['rows'][0]['architectureSettings']['VALID_ARCHS']['value'], '$(inherited)')
        self.assertFalse(saved[0][1]['componentAccepted']); self.assertFalse(saved[0][1]['devicePassed'])
        self.assertNotIn('DO_NOT_EXPORT', json.dumps([value for _, value in saved]))
        for value in ['/private/settings/path', 'x'*257, 'bad\x00value', 'bad\ud800value']:
            projection = admission.architecture_value(value)
            self.assertNotIn('value', projection); self.assertTrue(projection['valueWithheld'])
            self.assertEqual(projection['sha256'], admission.sha(value.encode('utf-8', errors='surrogatepass')))

    def test_only_arm64_platforms_are_admitted_and_intel_is_rejected(self):
        self.assertEqual(admission.SLOTS, (('device-arm64', 'iphoneos', 'arm64'),
                                          ('sim-arm64', 'iphonesimulator', 'arm64')))
        for sdk in ('iphoneos', 'iphonesimulator'):
            positive = admission.checked_build_settings(rows(sdk), sdk, 'arm64', '17.0')
            self.assertEqual(set(positive), set(admission.TARGETS))
            self.assertNotIn('DO_NOT_EXPORT', json.dumps(positive))
            self.assertTrue(all(row['effectiveArchitectures'] == ['arm64'] for row in positive.values()))
            with self.assertRaisesRegex(RuntimeError, 'invalid admitted Apple slot'):
                admission.checked_build_settings(rows(sdk, 'x86_64'), sdk, 'x86_64', '17.0')
        for validity in ('x86_64', 'arm64 x86_64'):
            original = rows(); original[0]['buildSettings']['VALID_ARCHS'] = validity
            with self.assertRaisesRegex(RuntimeError, 'architecture is excluded'):
                admission.checked_build_settings(original, 'iphonesimulator', 'arm64', '17.0')

    def test_targets_platform_sdk_deployment_and_device_exclusion_all_required(self):
        cases = []
        cases.append(rows()[:1]); cases.append(rows()+rows()[:1])
        for field, value in [('ARCHS', 'arm64 x86_64'), ('VALID_ARCHS', '$(inherited)'),
                             ('EXCLUDED_ARCHS', 'arm64'), ('PLATFORM_NAME', 'iphoneos'),
                             ('SDKROOT', '/wrong/iPhoneOS27.1.sdk'), ('CONFIGURATION', 'debug'),
                             ('IPHONEOS_DEPLOYMENT_TARGET', '15.0')]:
            sample = rows(); sample[1]['buildSettings'][field] = value; cases.append(sample)
        for sample in cases:
            with self.subTest(sample=sample), self.assertRaises(RuntimeError):
                admission.checked_build_settings(sample, 'iphonesimulator', 'arm64', '17.0')
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
            self.assertEqual(len(calls), 4)
            self.assertEqual(len(saved), 5); self.assertNotIn('DO_NOT_EXPORT', json.dumps(observed))
            self.assertEqual({path.name for path, _ in saved[:-1]},
                             {'build-settings-' + slot + '-' + query + '.json'
                              for slot, _, _ in admission.SLOTS for query in ('scheme', 'extension')})
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
            self.assertEqual(write.call_count, 4)
            self.assertNotIn('build-settings-admission.json', [call.args[0].name for call in write.call_args_list])

    def test_full_scheme_still_queries_extension_and_rejects_disagreement(self):
        calls = []
        differing = False
        def fake_run(command, **kwargs):
            calls.append(command)
            sdk = command[command.index('-sdk') + 1]
            sample = rows(sdk)
            if '-target' in command:
                sample = sample[1:]
                if differing:
                    sample[0]['buildSettings']['EXCLUDED_ARCHS'] = 'x86_64'
            return json.dumps(sample)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            conf = root / 'src-tauri/tauri.ios.conf.json'; conf.parent.mkdir(parents=True)
            conf.write_text(json.dumps({'bundle': {'iOS': {'minimumSystemVersion': '17.0'}}}))
            with patch.object(admission, 'ROOT', root), patch.object(admission, 'run', side_effect=fake_run), \
                    patch.object(admission, 'file_pin', return_value={'bytes': 1, 'sha256': 'same'}), \
                    patch.object(admission, 'write_json') as write:
                observed = admission.preflight(root, {})
                self.assertEqual(set(observed['slots']), {'device-arm64', 'sim-arm64'})
                self.assertEqual(len(calls), 4)
                self.assertEqual(sum('-target' in command for command in calls), 2)
                differing = True; write.reset_mock()
                with self.assertRaisesRegex(RuntimeError, 'scheme and explicit extension settings differ'):
                    admission.preflight(root, {})
                self.assertNotIn('build-settings-admission.json', [call.args[0].name for call in write.call_args_list])
