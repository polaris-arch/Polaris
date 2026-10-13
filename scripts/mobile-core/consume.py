#!/usr/bin/env python3
"""Verify original formal components, materialize Android input or freeze Apple link input."""
import argparse
import json
import os
from pathlib import Path
import shutil

import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, canonical, load, require, sha
from artifacts import file_pin, inspect_aar, inspect_framework, read_receipt
from producer import lease, run, write_json


def verify_original(component, receipt_path, platform, tools, scratch):
    value = load()
    receipt = read_receipt(Path(receipt_path))
    require(receipt.get('schema') == 'polaris-mobile-component-v1' and receipt.get('platform') == platform and
            type(receipt.get('sourceOverlay')) is int and receipt.get('sourceOverlay') == 0 and receipt.get('AppAccepted') is False and receipt.get('devicePassed') is False,
            'old/CLI/diagnostic mobile component receipt rejected')
    require(receipt['fingerprint'] == sha(canonical({k: v for k, v in receipt.items() if k != 'fingerprint'})), 'component receipt changed')
    from native_inputs import verify
    verify(receipt['nativeInputs'], log=Path(component).parent / 'original-bind.log')
    binding = receipt['input']
    require(binding['sourceContract'] == value and binding['sourceObservation']['sourceCommit'] == value['sourceCommit'] and
            binding['sourceObservation']['sourceTree'] == value['sourceTree'] and binding['sourceOverlay'] == 0,
            'formal component source input differs')
    # Preserve producer candidate, original graph paths and actual BuildID.
    # The consuming App gets its own identity; it does not relabel the producer.
    expected_id = 'polaris-mobile-' + sha(canonical(binding))
    if platform == 'android':
        for name in ('go', 'javap', 'readelf'):
            require(name in tools, 'actual verifier tool missing: ' + name)
        observed = inspect_aar(component, value, binding['moduleGraph'], tools, run, scratch, expected_id)
    else:
        observed = inspect_framework(component, value, binding['moduleGraph'], expected_id)
    require(observed == receipt['output'], 'actual component bytes/API/native identity differ')
    return receipt


def app_identity():
    require(not run(['git', 'status', '--porcelain=v1', '--untracked-files=all'], cwd=ROOT), 'App consumer source is not frozen')
    return {'candidate': run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip(),
            'consumerCode': {name: file_pin(ROOT / name) for name in
                ['scripts/mobile-core/consume.py', 'scripts/mobile-core/artifacts.py',
                 'scripts/mobile-core/fork_input.py', 'scripts/mobile-core/carriers.py',
                 'scripts/mobile-core/source-contract.json', 'scripts/mobile-core/transaction.py',
                 'scripts/mobile-core/native_inputs.py', 'src-tauri/gen/apple/project.yml',
                 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj',
                 'scripts/verify-apk.mjs', 'scripts/assert-r8-evidence.mjs',
                 'scripts/release-assets.mjs', 'scripts/mobile-core/apple_rust.py', 'scripts/mobile-core/ci_apple.py',
             'scripts/mobile-core/build_apple_app.py']}}


def materialize_android(component, receipt_path, tools, work, cache):
    value = load()
    with lease(work, cache, value) as (work, cache):
        require(Path(read_receipt(receipt_path)['nativeInputs']['cache']).resolve(strict=True) == cache,
                'consumer must hold the actual producer cache lease')
        receipt = verify_original(component, receipt_path, 'android', tools, work)
        identity = app_identity()
        dest = ROOT / 'src-tauri/gen/android/app/libs/libbox.aar'
        target_receipt = ROOT / 'scripts/mobile-core/android-consumption.json'
        for parent in dest.parents:
            require(not parent.is_symlink(), 'App input parent redirected')
            if parent == ROOT:
                break
        dest.parent.mkdir(parents=True, exist_ok=True)
        stage = work / 'app-libbox.aar'
        from transaction import recover
        recover([(stage, dest), (work / 'stage-app-consumption.json', target_receipt)], work)
        if dest.exists() and target_receipt.exists():
            existing = read_receipt(target_receipt)
            if (existing.get('componentFingerprint') == receipt['fingerprint'] and existing.get('App') == identity and
                    existing.get('fingerprint') == sha(canonical({k: v for k, v in existing.items() if k != 'fingerprint'})) and
                    existing.get('output') == receipt['output'] and existing.get('AppAccepted') is False and
                    existing.get('devicePassed') is False and existing.get('sourceOverlay') == 0 and
                    file_pin(dest) == receipt['output']['artifact']):
                return existing  # Identical input cache hit, with original receipt identity.
        require(not stage.exists(), 'unaccepted materialization stage retained')
        shutil.copyfile(component, stage)
        require(file_pin(stage) == receipt['output']['artifact'], 'materialization copy changed')
        facts = {'schema': 'polaris-mobile-app-input-v1', 'platform': 'android', 'App': identity,
                 'producerCandidate': receipt['input']['producer']['candidate'],
                 'componentReceiptSha256': file_pin(receipt_path)['sha256'],
                 'componentFingerprint': receipt['fingerprint'], 'output': receipt['output'],
                 'AppAccepted': False, 'devicePassed': False, 'sourceOverlay': 0}
        facts['fingerprint'] = sha(canonical(facts))
        from transaction import install
        staged_receipt = work / 'stage-app-consumption.json'
        write_json(staged_receipt, facts)
        def installed():
            require(file_pin(dest) == receipt['output']['artifact'] and app_identity() == identity,
                    'installed input or consumer source changed')
            require(read_receipt(target_receipt) == facts, 'installed consumption receipt changed')
        install([(stage, dest), (staged_receipt, target_receipt)], work, installed)
        return facts


def apple_link(component, receipt_path, output, work, cache):
    value = load()
    with lease(work, cache, value) as (work, cache):
        require(Path(read_receipt(receipt_path)['nativeInputs']['cache']).resolve(strict=True) == cache,
                'consumer must hold the actual producer cache lease')
        receipt = verify_original(component, receipt_path, 'apple', {}, work)
        facts = {'schema': 'polaris-mobile-apple-link-input-v1', 'App': app_identity(),
                 'framework': str(Path(component).resolve(strict=True)),
                 'componentReceipt': str(Path(receipt_path).resolve(strict=True)),
                 'componentReceiptSha256': file_pin(receipt_path)['sha256'],
                 'componentFingerprint': receipt['fingerprint'], 'producerInput': receipt['input'],
                 'componentOutput': receipt['output'], 'staticOnly': True,
                 'AppAccepted': False, 'devicePassed': False}
        facts['fingerprint'] = sha(canonical(facts))
        require(not Path(output).exists(), 'fixed link input already exists; do not overwrite a frozen context')
        write_json(output, facts)
        return facts


def verify_link(path, expected_sha, scratch):
    require(file_pin(path)['sha256'] == expected_sha, 'fixed Apple link input changed')
    link = read_receipt(Path(path))
    require(link.get('schema') == 'polaris-mobile-apple-link-input-v1' and link.get('staticOnly') is True and
            link.get('fingerprint') == sha(canonical({k: v for k, v in link.items() if k != 'fingerprint'})), 'Apple link context differs')
    require(file_pin(link['componentReceipt'])['sha256'] == link['componentReceiptSha256'], 'original component receipt changed')
    receipt = verify_original(link['framework'], link['componentReceipt'], 'apple', {}, scratch)
    require(receipt['fingerprint'] == link['componentFingerprint'] and receipt['output'] == link['componentOutput'] and
            app_identity() == link['App'], 'actual component/App link context changed')
    return link


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['verify-android', 'materialize-android', 'apple-link', 'verify-link'])
    for name in ('component', 'receipt', 'work', 'cache', 'link-input', 'scratch'):
        parser.add_argument('--' + name, type=Path)
    parser.add_argument('--link-input-sha256')
    for name in ('go', 'javap', 'readelf', 'output'):
        parser.add_argument('--' + name, type=Path)
    args = parser.parse_args()
    tools = {name: str(getattr(args, name)) for name in ('go', 'javap', 'readelf') if getattr(args, name) is not None}
    if args.action == 'verify-link':
        # Xcode's build service does not transport compiler reservation fds.
        # This CLI phase only reads frozen metadata; no compiler is launched.
        os.environ.pop('POLARIS_BUILD_RESERVATION_FD', None)
        require(args.link_input is not None and args.link_input_sha256 is not None and args.scratch is not None,
                'original fixed Apple link context and scratch required')
        result = verify_link(args.link_input, args.link_input_sha256, args.scratch)
    elif args.action == 'apple-link':
        require(all([args.component, args.receipt, args.work, args.cache]), 'Apple original inputs/owned slots required')
        require(args.output is not None and not tools, 'Apple link consumes static component and fixed link context only')
        result = apple_link(args.component, args.receipt, args.output, args.work, args.cache)
    elif args.action == 'materialize-android':
        require(all([args.component, args.receipt, args.work, args.cache]), 'Android original inputs/owned slots required')
        result = materialize_android(args.component, args.receipt, tools, args.work, args.cache)
    else:
        require(all([args.component, args.receipt, args.work, args.cache]), 'Android original inputs/owned slots required')
        with lease(args.work, args.cache, load()) as (work, cache):
            require(Path(read_receipt(args.receipt)['nativeInputs']['cache']).resolve(strict=True) == cache,
                    'verifier must hold the actual producer cache lease')
            result = verify_original(args.component, args.receipt, 'android', tools, work)
    print(json.dumps({'fingerprint': result['fingerprint'], 'AppAccepted': False, 'devicePassed': False}))


if __name__ == '__main__':
    main()
