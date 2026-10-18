"""Resolved selected-slot arm64 Xcode configuration admission; never compiles or signs."""
import json
from pathlib import Path
import re

from artifacts import file_pin
from fork_input import ROOT, require, sha
from producer import run, write_json

SLOTS = (('device-arm64', 'iphoneos', 'arm64'),
         ('sim-arm64', 'iphonesimulator', 'arm64'))
TARGETS = ('polaris_iOS', 'polaris_PacketTunnel')
SETTINGS_BYTES = 2 * 1024 ** 2


def architecture_value(value):
    """Only bounded architecture identifiers; never unrelated settings/env."""
    if not isinstance(value, str):
        return {'type': type(value).__name__ if type(value) in (type(None), bool, int, float, list, dict) else 'other'}
    raw = value.encode('utf-8', errors='surrogatepass')
    result = {'type': 'str', 'encoding': 'utf8-surrogatepass', 'bytes': len(raw), 'sha256': sha(raw)}
    if re.fullmatch(r'[A-Za-z0-9_$(){}.+\-\s]{0,256}', value, re.ASCII):
        result['value'] = value
    else:
        result['valueWithheld'] = True
    return result


def architecture_rejection(target, key, value, reason):
    detail = {'target': target if target in TARGETS else 'other', 'field': key,
              'reason': reason, 'observed': architecture_value(value)}
    return 'unresolved Apple architecture setting: ' + json.dumps(detail, sort_keys=True)


def retain_architecture_rows(root, slot, query, rows, raw):
    require(slot in {s for s, _, _ in SLOTS} and query in ('scheme', 'extension'),
            'closed Apple settings observation required')
    observed = []
    for row in rows[:3] if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            observed.append({'rowShape': 'not-object'})
            continue
        settings = row.get('buildSettings')
        record = {'target': row.get('target') if row.get('target') in TARGETS else 'other'}
        if isinstance(settings, dict):
            record['architectureSettings'] = {key: {'present': key in settings,
                **architecture_value(settings.get(key))} for key in ('ARCHS', 'VALID_ARCHS', 'EXCLUDED_ARCHS')}
        else:
            record['settingsShape'] = 'not-object'
        observed.append(record)
    value = {'schema': 'polaris-apple-architecture-observation-v1', 'slot': slot, 'query': query,
             'rows': observed, 'rowCount': len(rows) if isinstance(rows, list) else None,
             'normalizedResponseText': {'bytes': len(raw.encode()), 'sha256': sha(raw.encode())},
             'componentAccepted': False, 'devicePassed': False,
             'scope': 'selected architecture fields before admission; full settings/environment not retained'}
    write_json(Path(root) / ('build-settings-' + slot + '-' + query + '.json'), value)


def checked_build_settings(rows, sdk, arch, minimum, *, expected_targets=TARGETS):
    require(sdk in ('iphoneos', 'iphonesimulator') and arch == 'arm64', 'invalid admitted Apple slot')
    require(isinstance(rows, list) and len(rows) == len(expected_targets),
            'actual Apple target set differs')
    selected = {}
    for row in rows:
        require(isinstance(row, dict) and row.get('target') in expected_targets and
                row['target'] not in selected, 'actual Apple target set differs')
        settings = row.get('buildSettings')
        require(isinstance(settings, dict), 'actual Apple target build settings required')
        fields = {}
        for key in ('ARCHS', 'VALID_ARCHS', 'EXCLUDED_ARCHS'):
            value = settings.get(key, '' if key == 'EXCLUDED_ARCHS' else None)
            if not isinstance(value, str):
                raise RuntimeError(architecture_rejection(row['target'], key, value, 'non-string'))
            fields[key] = value.split()
            if not all(item in ('arm64', 'arm64e', 'armv7', 'armv7s', 'i386', 'x86_64') for item in fields[key]):
                raise RuntimeError(architecture_rejection(row['target'], key, value, 'unknown-token'))
            if len(fields[key]) != len(set(fields[key])):
                raise RuntimeError(architecture_rejection(row['target'], key, value, 'duplicate-token'))
        effective = (set(fields['ARCHS']) & set(fields['VALID_ARCHS'])) - set(fields['EXCLUDED_ARCHS'])
        require(fields['ARCHS'] == [arch] and fields['VALID_ARCHS'] == ['arm64'] and effective == {arch},
                'selected Apple architecture is excluded by actual build settings')
        require(settings.get('PLATFORM_NAME') == sdk and settings.get('CONFIGURATION') == 'release',
                'actual Apple platform/configuration differs')
        sdkroot = settings.get('SDKROOT')
        # Resolved SDKROOT may be the logical identifier or the actual selected
        # Xcode SDK path. Project/environment values are never returned/logged.
        require(isinstance(sdkroot, str) and (sdkroot == sdk or
                (Path(sdkroot).is_absolute() and re.fullmatch(
                    r'iPhone(?:OS|Simulator)[0-9]+(?:\.[0-9]+)*\.sdk', Path(sdkroot).name) and
                 Path(sdkroot).name.startswith('iPhoneOS' if sdk == 'iphoneos' else 'iPhoneSimulator'))),
                'actual Apple SDK root differs')
        require(settings.get('IPHONEOS_DEPLOYMENT_TARGET') == minimum,
                'actual Apple deployment version differs')
        if sdk == 'iphoneos' and row['target'] == 'polaris_iOS':
            require('x86_64' in fields['EXCLUDED_ARCHS'], 'device Intel exclusion must remain')
        selected[row['target']] = {'target': row['target'], 'platform': sdk,
            'requestedArchitecture': arch, 'effectiveArchitectures': sorted(effective),
            'deploymentTarget': minimum, **fields}
    require(set(selected) == set(expected_targets), 'actual Apple target set differs')
    return selected


def _read_rows(command, env, slot, root, query):
    raw = run(command, cwd=ROOT, env=env, observation_fd=1,
              observation_phase='build-settings-admission', observation_slot=slot)
    require(len(raw.encode()) <= SETTINGS_BYTES, 'Apple build-settings response exceeds observation bound')
    try:
        rows = json.loads(raw)
    except (ValueError, UnicodeError):
        raise RuntimeError('invalid resolved Apple settings JSON') from None
    retain_architecture_rows(root, slot, query, rows, raw)
    return rows


def preflight(root, env, *, expected_slots):
    """Query the exact archive scheme/destination, plus explicit extension target.

    Main-only scheme output is expected on Xcode. The extension's explicit
    target query uses the identical project/configuration/SDK/ARCHS; it does
    not claim to be a second scheme or perform an archive.
    """
    from apple_handoff import SCOPES
    require(isinstance(expected_slots, tuple) and expected_slots in SCOPES.values(),
            'closed explicit Apple carrier slots required')
    project = ROOT / 'src-tauri/gen/apple/polaris.xcodeproj'
    minimum = json.loads((ROOT / 'src-tauri/tauri.ios.conf.json').read_bytes())['bundle']['iOS']['minimumSystemVersion']
    require(isinstance(minimum, str) and re.fullmatch(r'[0-9]+\.[0-9]+(?:\.[0-9]+)?', minimum),
            'explicit iOS deployment version required')
    inputs = [project / 'project.pbxproj', project / 'xcshareddata/xcschemes/polaris_iOS.xcscheme',
              ROOT / 'src-tauri/tauri.ios.conf.json']
    pins = [file_pin(path) for path in inputs]
    observations = {}
    for slot, sdk, arch in SLOTS:
        if slot not in expected_slots:
            continue
        common = ['-project', project, '-configuration', 'release', '-sdk', sdk,
                  '-showBuildSettings', '-json', 'ARCHS=' + arch, 'ONLY_ACTIVE_ARCH=YES',
                  'CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO']
        rows = _read_rows(['xcodebuild', '-scheme', 'polaris_iOS', '-destination',
            'generic/platform=iOS Simulator' if sdk == 'iphonesimulator' else 'generic/platform=iOS',
            *common], env, slot, root, 'scheme')
        require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows),
                'actual Apple build-settings array required')
        targets = [row.get('target') for row in rows]
        extension = _read_rows(['xcodebuild', '-target', 'polaris_PacketTunnel', *common], env, slot, root, 'extension')
        extension_settings = checked_build_settings(extension, sdk, arch, minimum,
                                                    expected_targets=('polaris_PacketTunnel',))
        if targets == ['polaris_iOS']:
            rows += extension
        observations[slot] = checked_build_settings(rows, sdk, arch, minimum)
        require(observations[slot]['polaris_PacketTunnel'] == extension_settings['polaris_PacketTunnel'],
                'scheme and explicit extension settings differ')
    require([file_pin(path) for path in inputs] == pins, 'Apple admission inputs changed during inspection')
    require(tuple(observations) == expected_slots, 'Apple admission observed slots differ')
    result = {'schema': 'polaris-apple-build-settings-admission-v2', 'slots': observations,
              'requiredSlots': list(expected_slots), 'observedSlots': list(observations),
              'inputPins': pins, 'scope': 'resolved configuration only; no compiler/carrier/device acceptance'}
    write_json(Path(root) / 'build-settings-admission.json', result)
    return result
