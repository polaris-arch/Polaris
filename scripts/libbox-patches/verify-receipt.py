#!/usr/bin/env python3
"""Consume the same Android source/input/artifact predicate as the fresh builder."""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

PATCH_DIR = Path(__file__).resolve().parent
ROOT = PATCH_DIR.parent.parent
AAR = ROOT / 'src-tauri/gen/android/app/libs/libbox.aar'

_spec = importlib.util.spec_from_file_location('android_libbox_builder', PATCH_DIR / 'build.py')
builder = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(builder)
android = builder.android


def consumption(apk, abi, r8, receipt, receipt_path):
    """Bind actual package bytes to its clean App candidate without relabeling its component."""
    selected = ['arm64-v8a', 'armeabi-v7a'] if abi == 'universal' else [abi]
    builder.require(all(item in android.ABIS for item in selected), 'Unknown APK ABI')
    builder.require(json.loads(receipt_path.read_bytes()) == receipt, 'Component receipt changed before APK consumption')
    current = android.candidate(builder.run)
    builder.run(['node', str(ROOT / 'scripts/release-assets.mjs'), 'abis', str(apk), abi], cwd=ROOT)
    for item in selected:
        builder.run(['node', str(ROOT / 'scripts/verify-apk.mjs'), str(apk), '--abi', item], cwd=ROOT)
    builder.run(['node', str(ROOT / 'scripts/assert-r8-evidence.mjs'), str(r8)], cwd=ROOT)
    with zipfile.ZipFile(apk) as package:
        builder.require(len(package.namelist()) == len(set(package.namelist())), 'Duplicate APK entries')
        embedded = {name.split('/')[1]: android.digest(package.read(name)) for name in package.namelist()
                    if name.startswith('lib/') and name.endswith('/libbox.so')}
        builder.require(set(embedded) == set(selected), 'APK selected libbox ABI differs')
        for item in selected:
            builder.require(embedded[item] == receipt['nativeLibraries'][f'jni/{item}/libbox.so']['sha256'], 'APK embedded libbox differs from verified AAR')
    builder.require(android.candidate(builder.run) == current, 'App candidate changed during APK verification')
    builder.require(json.loads(receipt_path.read_bytes()) == receipt, 'Component receipt changed during APK verification')
    facts = {'schema': 'polaris-android-source-consumption-v2', 'currentAppCandidate': current,
             'componentProducerCandidate': receipt['componentProducerCandidate'],
             'componentReceiptSha256': android.file_hash(receipt_path),
             'androidInputFingerprint': receipt['androidInput']['fingerprint'],
             'sourceReceiptFingerprint': receipt['sourceReceipt']['fingerprint'],
             'aarSha256': receipt['aar']['sha256'], 'selectedABIs': selected, 'embeddedLibboxSha256': embedded,
             'apk': {'sha256': android.file_hash(apk), 'bytes': apk.stat().st_size},
             'r8Evidence': android.directory_identity(r8),
             'verifiers': {name: android.file_hash(ROOT / 'scripts' / name)
                           for name in ('verify-apk.mjs', 'assert-r8-evidence.mjs', 'release-assets.mjs')}}
    facts['fingerprint'] = android.digest(android.canonical(facts))
    destination = Path(str(apk) + '.source-consumption.json')
    staged = Path(str(destination) + '.tmp')
    try:
        staged.write_text(json.dumps(facts, indent=2, sort_keys=True) + '\n')
        staged.replace(destination)
    finally:
        staged.unlink(missing_ok=True)
    return facts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apk', type=Path)
    parser.add_argument('--abi', default='arm64-v8a')
    parser.add_argument('--r8', type=Path)
    args = parser.parse_args()
    # Admission is deliberately before even reading an old cache receipt/AAR.
    source, core, policy = android.admit()
    tool = android.tools(source, policy, builder.run)
    identity = android.input_identity(source, core, policy, tool['identity'])
    receipt_path = PATCH_DIR / 'build-receipt.json'
    receipt = json.loads(receipt_path.read_bytes())
    with tempfile.TemporaryDirectory(prefix='polaris-aar-verification-') as scratch:
        builder.verify_component(AAR, receipt, source, core, policy, tool, identity, Path(scratch))
    if args.apk is not None:
        builder.require(args.r8 is not None, 'APK source consumption requires actual R8 evidence')
        consumption(args.apk.resolve(), args.abi, args.r8.resolve(), receipt, receipt_path)
    else:
        builder.require(args.r8 is None, '--r8 requires --apk')
    print(f"libbox receipt verified: {receipt['aar']['sha256']}")


if __name__ == '__main__':
    try:
        main()
    except (OSError, KeyError, ValueError, TypeError, RuntimeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print(f'libbox receipt verification failed: {error}', file=sys.stderr)
        sys.exit(1)
