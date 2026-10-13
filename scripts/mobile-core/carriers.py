#!/usr/bin/env python3
"""Final App carrier/source predicates; never device or signing-trust acceptance."""
import argparse
import json
from pathlib import Path
import plistlib
import re
import stat
import zipfile

import os
import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, canonical, load, require, sha
from artifacts import receipt_digest, file_pin, module, zip_entries, check_buildinfo, read_receipt
from producer import lease, run, write_json
from consume import app_identity, apple_resources, verify_original


def android(apk, abi, r8, component, receipt_path, tools, scratch):
    receipt = verify_original(component, receipt_path, 'android', tools, scratch)
    input_path = ROOT / 'scripts/mobile-core/android-consumption.json'
    material = read_receipt(input_path)
    require(material['schema'] == 'polaris-mobile-app-input-v1' and material['platform'] == 'android' and
            material['componentFingerprint'] == receipt['fingerprint'] and
            material['componentReceiptSha256'] == file_pin(receipt_path)['sha256'] and
            material['output'] == receipt['output'] and material.get('sourceOverlay') == 0 and
            material.get('AppAccepted') is False and material.get('devicePassed') is False and
            material['fingerprint'] == receipt_digest({k: v for k, v in material.items() if k != 'fingerprint'}),
            'actual materialized App input changed')
    current = app_identity()
    require(current == material['App'], 'App candidate changed after SDK materialization')
    selected = ['arm64-v8a', 'armeabi-v7a'] if abi == 'universal' else [abi]
    require(set(selected) <= {'arm64-v8a', 'armeabi-v7a'}, 'published App architecture scope differs')
    run(['node', ROOT / 'scripts/release-assets.mjs', 'abis', apk, abi], cwd=ROOT)
    run(['node', ROOT / 'scripts/assert-r8-evidence.mjs', r8], cwd=ROOT)
    for target in selected:
        run(['node', ROOT / 'scripts/verify-apk.mjs', apk, '--abi', target], cwd=ROOT)
    with zipfile.ZipFile(apk) as archive:
        zip_entries(archive)
        embedded = {n.split('/')[1]: sha(archive.read(n)) for n in archive.namelist()
                    if re.fullmatch(r'lib/[^/]+/libbox\.so', n)}
        require(set(embedded) == set(selected), 'APK embedded SDK ABI inventory differs')
        for target in selected:
            require(embedded[target] == receipt['output']['nativeLibraries']['jni/' + target + '/libbox.so']['sha256'],
                    'APK native bytes differ from verified original AAR')
        runtime = {}
        for name, original in receipt['output']['nativeLibraries'].items():
            if name.split('/')[1] in selected:
                destination = name.replace('jni/', 'lib/', 1)
                require(destination in archive.namelist() and sha(archive.read(destination)) == original['sha256'],
                        'APK lost/changed a same-package SDK runtime input')
                runtime[destination] = original['sha256']
    require(app_identity() == current and read_receipt(input_path) == material,
            'consumer input changed during final carrier inspection')
    return {'schema': 'polaris-mobile-android-carrier-v1', 'App': current,
            'producerCandidate': receipt['input']['producer']['candidate'],
            'componentFingerprint': receipt['fingerprint'], 'componentReceiptSha256': file_pin(receipt_path)['sha256'],
            'apk': file_pin(apk), 'selectedABIs': selected, 'embeddedLibboxSha256': embedded,
            'embeddedRuntimeSha256': runtime,
            'r8Files': {p.relative_to(r8).as_posix(): file_pin(p) for p in Path(r8).rglob('*') if p.is_file()},
            'AppCarrierValidated': True, 'AppAccepted': False, 'devicePassed': False, 'signatureTrustValidated': False}


def static_linkmap(text, framework, selected, derived):
    require('# Object files:' in text and '# Sections:' in text and '# Symbols:' in text,
            'actual extension linker map required')
    binary = (Path(framework) / selected['libraryIdentifier'] / 'Libbox.framework/Libbox').resolve(strict=True)
    derived = Path(derived).resolve(strict=True)
    require(derived.is_dir(), 'actual fixed derived-data directory required')
    members = {}
    for member in selected['members']:
        members.setdefault(member['name'], []).append(member)
    body = text.split('# Object files:', 1)[1].split('# Sections:', 1)[0]
    consumed = []
    for line in body.splitlines():
        match = re.fullmatch(r'\[\s*(\d+)\]\s+(.+)\(([^()]+)\)', line.strip())
        if match is None:
            continue
        archive, name = match[2], match[3]
        if 'Libbox.framework' not in archive:
            continue
        linked = Path(archive).resolve(strict=True)
        require((linked == binary or linked.is_relative_to(derived)) and name in members and
                file_pin(linked) == selected['binary'],
                'extension link map used a different/unbound static SDK input')
        headers = Path(archive).parent / 'Headers'
        for header, digest in selected['headerSha256'].items():
            require(file_pin((headers / header).resolve(strict=True))['sha256'] == digest,
                    'actual selected framework headers changed')
        consumed.append({'linkOrdinal': int(match[1]), 'archive': str(linked), 'archivePin': file_pin(linked),
                         'memberName': name, 'originalMembers': members[name]})
    require(consumed and any(m['goCarrier'] for row in consumed for m in row['originalMembers']),
            'link map lacks original Go carrier member')
    require(any(not m['goCarrier'] for row in consumed for m in row['originalMembers']),
            'link map lacks original generated/native members')
    require(len({r['linkOrdinal'] for r in consumed}) == len(consumed), 'link map object ordinals duplicate')
    return consumed


def verify_apple_resources(archive, app_root, expected):
    prefix = app_root + 'assets/'
    actual = {name.removeprefix(prefix): name for name in archive.namelist()
              if name.startswith(prefix) and not name.endswith('/')}
    require(set(actual) == set(expected), 'final App resource inventory differs')
    observed = {}
    for name, row in expected.items():
        raw, info = archive.read(actual[name]), archive.getinfo(actual[name])
        pin = {'bytes': len(raw), 'sha256': sha(raw), 'mode': stat.S_IMODE(info.external_attr >> 16)}
        require(pin == {key: row[key] for key in ('bytes', 'sha256', 'mode')}, 'final App resource bytes/mode differ')
        observed[name] = pin
    return observed


def apple(ipa, link_path, link_sha, linkmap, receipt_path, scratch, derived, simulator_arch=None):
    require(file_pin(link_path)['sha256'] == link_sha, 'fixed Apple link input bytes differ')
    link = read_receipt(Path(link_path))
    require(link['schema'] == 'polaris-mobile-apple-link-input-v1' and link['staticOnly'] is True and
            link['fingerprint'] == receipt_digest({k: v for k, v in link.items() if k != 'fingerprint'}), 'Apple link input differs')
    require(file_pin(receipt_path)['sha256'] == link['componentReceiptSha256'], 'original Apple component receipt changed')
    receipt = verify_original(link['framework'], receipt_path, 'apple', {}, scratch)
    require(receipt['fingerprint'] == link['componentFingerprint'] and receipt['output'] == link['componentOutput'],
            'actual link component differs')
    current = app_identity()
    require(current == link['App'] and apple_resources() == link['resources'],
            'App source/resources changed after link context was frozen')
    value = load()
    helper = module('final_mobile_info', 'scripts/libbox-patches/android-source.py')
    parser = module('final_mobile_carrier', 'scripts/apple-carrier.py')
    with zipfile.ZipFile(ipa) as archive:
        zip_entries(archive)
        apps = [n for n in archive.namelist() if re.fullmatch(r'Payload/[^/]+\.app/Info\.plist', n)]
        require(len(apps) == 1, 'one actual host App required')
        app_root = apps[0].removesuffix('Info.plist')
        app = plistlib.loads(archive.read(apps[0]))
        resources = verify_apple_resources(archive, app_root, link['resources'])
        extensions = [n for n in archive.namelist() if n.startswith(app_root + 'PlugIns/') and n.endswith('.appex/Info.plist')]
        require(len(extensions) == 1, 'one actual PacketTunnel required')
        extension_root = extensions[0].removesuffix('Info.plist')
        extension = plistlib.loads(archive.read(extensions[0]))
        require(extension['CFBundleIdentifier'] == app['CFBundleIdentifier'] + '.PacketTunnel' and
                extension['NSExtension']['NSExtensionPointIdentifier'] == 'com.apple.networkextension.packet-tunnel' and
                app['PolarisAppGroup'] == extension['PolarisAppGroup'] and app['PolarisAppGroup'].startswith('group.') and
                '$(' not in app['PolarisAppGroup'], 'actual App/extension identities/group differ')
        rows = {}
        for name, root, info in [('app', app_root, app), ('extension', extension_root, extension)]:
            require(info['UIDeviceFamily'] == [1, 2] and info['CFBundleSupportedPlatforms'] ==
                    (['iPhoneSimulator'] if simulator_arch else ['iPhoneOS']) and
                    tuple(map(int, info['MinimumOSVersion'].split('.'))) >= (17, 0), 'actual device carrier scope differs')
            raw = archive.read(root + info['CFBundleExecutable'])
            parsed = parser.container(raw, helper)
            cpu = 0x1000007 if simulator_arch == 'amd64' else 0x100000c
            require(parsed['kind'] == 'Mach-O64' and parsed['cpu'] == cpu and parsed['filetype'] == 2,
                    'actual App binary target must be the selected thin MH_EXECUTE')
            rows[name] = {'bytes': len(raw), 'sha256': sha(raw), 'bundleIdentifier': info['CFBundleIdentifier']}
            if name == 'extension':
                infos = [s['buildInfo'] for s in parsed['sections'] if 'buildInfo' in s]
                ids = [n['value'] for s in parsed['sections'] for n in s.get('buildIDs', [])]
                require(len(infos) == len(ids) == 1, 'final extension must retain original Go source carrier')
                check_buildinfo(infos[0]['parsedBuildInfo'], value, receipt['input']['moduleGraph'],
                                os_name='ios', arch=simulator_arch or 'arm64', mode='c-archive', simulator=bool(simulator_arch),
                                expected_id='polaris-mobile-' + receipt_digest(receipt['input']), actual_id=ids[0])
                rows[name].update(goBuildInfo=infos[0], buildID=ids[0])
        require(not any(n.startswith(app_root) and (n.endswith('.a') or 'Libbox.framework/' in n) for n in archive.namelist()),
                'build-only static SDK must not be copied into the installed App')
    original_map = Path(linkmap).read_bytes()
    text = original_map.decode('utf-8')
    target = ('ios-x86_64-simulator' if simulator_arch == 'amd64' else 'ios-arm64-simulator') if simulator_arch else 'ios-arm64'
    selected = receipt['output']['slices'][target]
    consumed = static_linkmap(text, link['framework'], selected, derived)
    require(app_identity() == current and file_pin(link_path)['sha256'] == link_sha,
            'final App/link source context changed')
    return {'schema': 'polaris-mobile-apple-carrier-v1', 'App': current, 'ipa': file_pin(ipa), 'binaries': rows,
            'producerCandidate': receipt['input']['producer']['candidate'],
            'componentFingerprint': receipt['fingerprint'], 'linkInputSha256': link_sha,
            'selectedTarget': target, 'resources': resources, 'linkedOriginalStaticMembers': consumed,
            'linkmap': file_pin(linkmap), 'AppCarrierValidated': True,
            'AppAccepted': False, 'devicePassed': False, 'signatureTrustValidated': False,
            'scope': 'App/PacketTunnel compile/static source and original bytes; signing permissions/device lifecycle are separate'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('platform', choices=['android', 'apple'])
    for name in ('artifact', 'receipt', 'scratch', 'output'):
        parser.add_argument('--' + name, required=True, type=Path)
    for name in ('component', 'r8', 'link-input', 'linkmap', 'go', 'javap', 'readelf', 'derived-data'):
        parser.add_argument('--' + name, type=Path)
    parser.add_argument('--link-input-sha256')
    parser.add_argument('--simulator-arch', choices=['arm64', 'amd64'])
    parser.add_argument('--abi', choices=['arm64-v8a', 'armeabi-v7a', 'universal'], default='arm64-v8a')
    args = parser.parse_args()
    require(args.scratch.is_dir() and not args.scratch.is_symlink(), 'fixed owned scratch required')
    cache = Path(read_receipt(args.receipt)['nativeInputs']['cache']).resolve(strict=True)
    with lease(args.scratch, cache, load()):
        if args.platform == 'android':
            require(all([args.component, args.r8, args.go, args.javap, args.readelf]), 'Android original inputs/tools required')
            result = android(args.artifact, args.abi, args.r8, args.component, args.receipt,
                             {name: str(getattr(args, name)) for name in ('go', 'javap', 'readelf')}, args.scratch)
        else:
            require(all([args.link_input, args.link_input_sha256, args.linkmap, args.derived_data]), 'Apple original link context required')
            result = apple(args.artifact, args.link_input, args.link_input_sha256, args.linkmap, args.receipt, args.scratch,
                           args.derived_data, args.simulator_arch)
    result['fingerprint'] = receipt_digest(result)
    require(not args.output.exists(), 'final carrier receipt already frozen')
    write_json(args.output, result)
    print(json.dumps({'fingerprint': result['fingerprint'], 'devicePassed': False, 'signatureTrustValidated': False}))


if __name__ == '__main__':
    main()
