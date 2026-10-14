"""Resolved three-slot Xcode configuration admission; never compiles or signs."""
import json
from pathlib import Path
import re

from artifacts import file_pin
from fork_input import ROOT, require
from producer import run, write_json

SLOTS = (('device-arm64', 'iphoneos', 'arm64'),
         ('sim-arm64', 'iphonesimulator', 'arm64'),
         ('sim-amd64', 'iphonesimulator', 'x86_64'))
TARGETS = ('polaris_iOS', 'polaris_PacketTunnel')
SETTINGS_BYTES = 2 * 1024 ** 2


def checked_build_settings(rows, sdk, arch, minimum, *, expected_targets=TARGETS):
    require(sdk in ('iphoneos', 'iphonesimulator') and arch in ('arm64', 'x86_64') and
            (sdk != 'iphoneos' or arch == 'arm64'), 'invalid admitted Apple slot')
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
            require(isinstance(value, str), 'resolved Apple architecture settings required')
            fields[key] = value.split()
            require(all(item in ('arm64', 'arm64e', 'armv7', 'armv7s', 'i386', 'x86_64') for item in fields[key]) and
                    len(fields[key]) == len(set(fields[key])), 'unresolved Apple architecture setting')
        effective = (set(fields['ARCHS']) & set(fields['VALID_ARCHS'])) - set(fields['EXCLUDED_ARCHS'])
        require(fields['ARCHS'] == [arch] and effective == {arch},
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


def _read_rows(command, env, slot):
    raw = run(command, cwd=ROOT, env=env, observation_fd=1,
              observation_phase='build-settings-admission', observation_slot=slot)
    require(len(raw.encode()) <= SETTINGS_BYTES, 'Apple build-settings response exceeds observation bound')
    try:
        rows = json.loads(raw)
    except (ValueError, UnicodeError):
        raise RuntimeError('invalid resolved Apple settings JSON') from None
    return rows


def preflight(root, env):
    """Query the exact archive scheme/destination, plus explicit extension target.

    Main-only scheme output is expected on Xcode. The extension's explicit
    target query uses the identical project/configuration/SDK/ARCHS; it does
    not claim to be a second scheme or perform an archive.
    """
    project = ROOT / 'src-tauri/gen/apple/polaris.xcodeproj'
    minimum = json.loads((ROOT / 'src-tauri/tauri.ios.conf.json').read_bytes())['bundle']['iOS']['minimumSystemVersion']
    require(isinstance(minimum, str) and re.fullmatch(r'[0-9]+\.[0-9]+(?:\.[0-9]+)?', minimum),
            'explicit iOS deployment version required')
    inputs = [project / 'project.pbxproj', project / 'xcshareddata/xcschemes/polaris_iOS.xcscheme',
              ROOT / 'src-tauri/tauri.ios.conf.json']
    pins = [file_pin(path) for path in inputs]
    observations = {}
    for slot, sdk, arch in SLOTS:
        common = ['-project', project, '-configuration', 'release', '-sdk', sdk,
                  '-showBuildSettings', '-json', 'ARCHS=' + arch, 'ONLY_ACTIVE_ARCH=YES',
                  'CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO']
        rows = _read_rows(['xcodebuild', '-scheme', 'polaris_iOS', '-destination',
            'generic/platform=iOS Simulator' if sdk == 'iphonesimulator' else 'generic/platform=iOS',
            *common], env, slot)
        require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows),
                'actual Apple build-settings array required')
        targets = [row.get('target') for row in rows]
        if targets == ['polaris_iOS']:
            extension = _read_rows(['xcodebuild', '-target', 'polaris_PacketTunnel', *common], env, slot)
            checked_build_settings(extension, sdk, arch, minimum, expected_targets=('polaris_PacketTunnel',))
            rows += extension
        observations[slot] = checked_build_settings(rows, sdk, arch, minimum)
    require([file_pin(path) for path in inputs] == pins, 'Apple admission inputs changed during inspection')
    result = {'schema': 'polaris-apple-build-settings-admission-v1', 'slots': observations,
              'inputPins': pins, 'scope': 'resolved configuration only; no compiler/carrier/device acceptance'}
    write_json(Path(root) / 'build-settings-admission.json', result)
    return result
