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

    def test_preflight_queries_app_and_extension_for_exact_single_or_double_scope(self):
        for slots in (('device-arm64',), ('device-arm64', 'sim-arm64')):
            calls, saved = [], []
            def fake_run(command, **kwargs):
                calls.append((command, kwargs)); sdk = command[command.index('-sdk') + 1]
                arch = next(value.split('=', 1)[1] for value in command if str(value).startswith('ARCHS='))
                sample = rows(sdk, arch)
                return json.dumps(sample[1:] if '-target' in command else sample[:1])
            with self.subTest(slots=slots), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                conf = root / 'src-tauri/tauri.ios.conf.json'; conf.parent.mkdir(parents=True)
                conf.write_text(json.dumps({'bundle': {'iOS': {'minimumSystemVersion': '17.0'}}}))
                with patch.object(admission, 'ROOT', root), patch.object(admission, 'run', side_effect=fake_run), \
                        patch.object(admission, 'file_pin', return_value={'bytes': 1, 'sha256': 'same'}), \
                        patch.object(admission, 'write_json', side_effect=lambda *args: saved.append(args)):
                    observed = admission.preflight(root, {'DEVELOPER_DIR': 'selected'}, expected_slots=slots)
                self.assertEqual(observed['schema'], 'polaris-apple-build-settings-admission-v2')
                self.assertEqual(tuple(observed['slots']), slots)
                self.assertEqual(observed['requiredSlots'], list(slots))
                self.assertEqual(observed['observedSlots'], list(slots))
                for settings in observed['slots'].values():
                    self.assertEqual(set(settings), {'polaris_iOS', 'polaris_PacketTunnel'})
                self.assertEqual(len(calls), 2 * len(slots))
                self.assertEqual(len(saved), 2 * len(slots) + 1)
                self.assertNotIn('DO_NOT_EXPORT', json.dumps(observed))
                self.assertEqual({path.name for path, _ in saved[:-1]},
                                 {'build-settings-' + slot + '-' + query + '.json'
                                  for slot in slots for query in ('scheme', 'extension')})
                self.assertEqual([command[command.index('-sdk') + 1] for command, _ in calls],
                                 ['iphoneos', 'iphoneos'] +
                                 (['iphonesimulator', 'iphonesimulator'] if len(slots) == 2 else []))
                for command, kwargs in calls:
                    self.assertIn('-showBuildSettings', command); self.assertNotIn('archive', command)
                    self.assertEqual(command[command.index('-configuration') + 1], 'release')
                    self.assertEqual(kwargs['env'], {'DEVELOPER_DIR': 'selected'})
                    if '-scheme' in command: self.assertIn('-destination', command)
                    else: self.assertEqual(command[command.index('-target') + 1], 'polaris_PacketTunnel')
                with patch.object(admission, 'ROOT', root), patch.object(admission, 'run', side_effect=fake_run), \
                        patch.object(admission, 'file_pin', side_effect=[{'bytes': 1}] * 3 + [{'bytes': 2}] * 3), \
                        patch.object(admission, 'write_json') as write, self.assertRaisesRegex(RuntimeError, 'inputs changed'):
                    admission.preflight(root, {}, expected_slots=slots)
                self.assertEqual(write.call_count, 2 * len(slots))
                self.assertNotIn('build-settings-admission.json', [call.args[0].name for call in write.call_args_list])

    def test_preflight_requires_explicit_closed_ordered_slots_before_reading_or_querying(self):
        invalid = ((), ('sim-arm64',), ('sim-arm64', 'device-arm64'),
                   ('device-arm64', 'device-arm64'), ('device-arm64', 'sim-amd64'),
                   ('device-arm64', 'sim-arm64', 'sim-amd64'), ('unknown',),
                   ['device-arm64'], 'device-arm64', None)
        with patch.object(admission, 'run') as query, patch.object(admission, 'file_pin') as pin, \
                patch.object(admission, 'write_json') as write:
            for slots in invalid:
                with self.subTest(slots=slots), self.assertRaisesRegex(RuntimeError, 'closed explicit Apple carrier slots'):
                    admission.preflight(Path('/inert'), {}, expected_slots=slots)
            with self.assertRaises(TypeError):
                admission.preflight(Path('/inert'), {})
        query.assert_not_called(); pin.assert_not_called(); write.assert_not_called()

    def test_each_selected_scope_requires_the_explicit_packet_tunnel_query(self):
        for slots in (('device-arm64',), ('device-arm64', 'sim-arm64')):
            for rejected_sdk in ('iphoneos', 'iphonesimulator') if len(slots) == 2 else ('iphoneos',):
                def fake_run(command, **kwargs):
                    sdk = command[command.index('-sdk') + 1]
                    if '-target' in command:
                        return json.dumps([] if sdk == rejected_sdk else rows(sdk)[1:])
                    return json.dumps(rows(sdk)[:1])
                with self.subTest(slots=slots, sdk=rejected_sdk), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    conf = root / 'src-tauri/tauri.ios.conf.json'; conf.parent.mkdir(parents=True)
                    conf.write_text(json.dumps({'bundle': {'iOS': {'minimumSystemVersion': '17.0'}}}))
                    with patch.object(admission, 'ROOT', root), patch.object(admission, 'run', side_effect=fake_run), \
                            patch.object(admission, 'file_pin', return_value={'bytes': 1, 'sha256': 'same'}), \
                            patch.object(admission, 'write_json') as write, \
                            self.assertRaisesRegex(RuntimeError, 'actual Apple target set differs'):
                        admission.preflight(root, {}, expected_slots=slots)
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
                observed = admission.preflight(root, {}, expected_slots=('device-arm64', 'sim-arm64'))
                self.assertEqual(set(observed['slots']), {'device-arm64', 'sim-arm64'})
                self.assertEqual(len(calls), 4)
                self.assertEqual(sum('-target' in command for command in calls), 2)
                differing = True; write.reset_mock()
                with self.assertRaisesRegex(RuntimeError, 'scheme and explicit extension settings differ'):
                    admission.preflight(root, {}, expected_slots=('device-arm64', 'sim-arm64'))
                self.assertNotIn('build-settings-admission.json', [call.args[0].name for call in write.call_args_list])
