#!/usr/bin/env python3
"""Explicit PRB00/VAL01 producer and App+PacketTunnel link entry; no publish/device action."""
import argparse
import os
from pathlib import Path
import zipfile

import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, load, require
from producer import lease, private_dir, run
from artifacts import file_pin


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('source', 'go', 'gomobile', 'gobind', 'developer', 'work', 'cache'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--simulator', action='store_true')
    parser.add_argument('--simulator-arch', choices=['arm64', 'amd64'], default='arm64')
    parser.add_argument('--signing-team', help='Explicit device signing identity; omit for unsigned compile/link probe')
    parser.add_argument('--component-work', type=Path, help='One fixed shared three-slice component slot')
    parser.add_argument('--rust-target', type=Path, help='One fixed Rust target shared under the actual cache lease')
    args = parser.parse_args()
    value = load()
    require(os.uname().sysname == 'Darwin', 'Apple App producer requires the selected Mac host')
    root, cache = private_dir(args.work), private_dir(args.cache)
    component_work = private_dir(args.component_work or root / 'component')
    rust_target = private_dir(args.rust_target or root / 'rust-target')
    require(root.stat().st_dev == component_work.stat().st_dev == rust_target.stat().st_dev == cache.stat().st_dev,
            'App/component/Rust/cache slots must share the reserved filesystem')
    # The component producer owns its own reservation. Do not sell a second
    # nested peak while waiting for it; the App lease below covers real linking.
    producer_args = [sys_python(), ROOT / 'scripts/mobile-core/producer.py', 'apple']
    for name in ('source', 'go', 'gomobile', 'gobind', 'developer'):
        producer_args += ['--' + name, getattr(args, name)]
    producer_args += ['--work', component_work, '--cache', cache]
    run(producer_args, log=root / 'original-component-runner.log')
    link = root / 'apple-link-input.json'
    # A retained link context cannot be overwritten as a new App identity.
    if not link.exists():
        run([sys_python(), ROOT / 'scripts/mobile-core/consume.py', 'apple-link',
            '--component', component_work / 'Libbox.xcframework', '--receipt', component_work / 'component-receipt.json',
            '--work', private_dir(root / 'link-work'), '--cache', cache, '--output', link],
            log=root / 'original-link-input.log')
    with lease(root, cache, value):
        env = dict(os.environ, DEVELOPER_DIR=str(args.developer.resolve(strict=True)),
                   POLARIS_LIBBOX_GENERATION_ROOT=str(component_work), POLARIS_MOBILE_LINK_INPUT=str(link),
                   POLARIS_MOBILE_LINK_INPUT_SHA256=file_pin(link)['sha256'],
                   POLARIS_MOBILE_SCRATCH=str(private_dir(root / 'link-check')),
                   POLARIS_FORMAL_APP_BUILD='1', POLARIS_APP_WORK=str(root),
                   POLARIS_APP_RUST_TARGET=str(rust_target))
        env['PATH'] = str(ROOT / 'ui/node_modules/.bin') + os.pathsep + env.get('PATH', '')
        platform = 'iphonesimulator' if args.simulator else 'iphoneos'
        env.update(PLATFORM_NAME=platform, CONFIGURATION='release',
                   ARCHS='x86_64' if args.simulator and args.simulator_arch == 'amd64' else 'arm64')
        env['SDKROOT'] = run(['xcrun', '--sdk', platform, '--show-sdk-path'], env=env).strip()
        # Compile before Xcode starts its build service so the reservation fd,
        # registered compiler group and child lifetime remain observable.
        run([sys_python(), ROOT / 'scripts/mobile-core/apple_rust.py', 'build'], env=env,
            log=root / 'original-rust-entry.log')
        # Authored pure Swift controls compile on the same real toolchain as the App.
        # This entry runs only in the eventual single uniform Mac validation phase.
        unit = root / 'measurement-conditions-test'
        if not args.simulator:
            run(['xcrun', 'swiftc', ROOT / 'crates/tauri-plugin-polaris-ios/ios/Sources/MeasurementDeviceState.swift',
                 ROOT / 'crates/tauri-plugin-polaris-ios/ios/Tests/DeviceConditionsTests.swift', '-o', unit], env=env,
                log=root / 'original-swift-control-build.log')
            run([unit], env=env, log=root / 'original-swift-control-run.log')
        derived = private_dir(root / 'derived-data')
        archive = root / 'Polaris.xcarchive'
        require(not archive.exists(), 'existing actual archive must not be overwritten')
        command = ['xcodebuild', '-project', ROOT / 'src-tauri/gen/apple/polaris.xcodeproj',
                   '-scheme', 'polaris_iOS', '-configuration', 'release', '-derivedDataPath', derived,
                   '-destination', 'generic/platform=iOS Simulator' if args.simulator else 'generic/platform=iOS',
                   '-archivePath', archive, 'archive', 'LD_GENERATE_MAP_FILE=YES']
        if args.simulator:
            command += ['ARCHS=' + ('x86_64' if args.simulator_arch == 'amd64' else 'arm64'), 'ONLY_ACTIVE_ARCH=YES']
        if args.signing_team:
            require(not args.simulator and args.signing_team.isalnum(), 'explicit device signing team differs')
            command += ['DEVELOPMENT_TEAM=' + args.signing_team]
        else:
            command += ['CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO']
        run(command, env=env, log=root / 'original-app-extension-link.log')
        apps = list((archive / 'Products/Applications').glob('*.app'))
        require(len(apps) == 1 and not apps[0].is_symlink(), 'actual archived host App required')
        ipa = root / ('polaris-box-' + value['version'] + '-app-link-probe.ipa')
        require(not ipa.exists(), 'existing carrier must not be overwritten')
        with zipfile.ZipFile(ipa, 'x', compression=zipfile.ZIP_DEFLATED) as package:
            for path in sorted(apps[0].rglob('*')):
                require(not path.is_symlink(), 'archived App contains a redirected entry')
                if path.is_file():
                    package.write(path, 'Payload/' + apps[0].name + '/' + path.relative_to(apps[0]).as_posix())
        maps = list(derived.rglob('PolarisPacketTunnel-LinkMap.txt'))
        require(len(maps) == 1, 'one actual extension link map required')
        # Invoke the same carrier predicate under this held cache/root lease;
        # do not open a competing lock description in a child verifier.
        from carriers import apple
        from fork_input import canonical, sha
        from producer import write_json
        final_receipt = apple(ipa, link, file_pin(link)['sha256'], maps[0],
                              component_work / 'component-receipt.json', private_dir(root / 'final-check'),
                              derived, args.simulator_arch if args.simulator else None)
        final_receipt['fingerprint'] = sha(canonical(final_receipt))
        rust_receipts = list(root.glob('rust-*.json'))
        require(len(rust_receipts) == 1, 'one actual Rust target receipt required')
        from artifacts import read_receipt
        rust = read_receipt(rust_receipts[0])
        require(rust['App'] == final_receipt['App'] and rust['devicePassed'] is False and
                rust['fingerprint'] == sha(canonical({k: v for k, v in rust.items() if k != 'fingerprint'})),
                'actual Rust/App source receipt differs')
        final_receipt['rustInput'] = {'receipt': str(rust_receipts[0]), **file_pin(rust_receipts[0])}
        final_receipt['fingerprint'] = sha(canonical({k: v for k, v in final_receipt.items() if k != 'fingerprint'}))
        require(not (root / 'app-carrier.json').exists(), 'final carrier receipt already frozen')
        write_json(root / 'app-carrier.json', final_receipt)
        # Neither ad-hoc nor a selected Team here means device/entitlement trust passed.
        print('Actual App/PacketTunnel carrier/source predicate completed; device/signature trust remains separate')


def sys_python():
    import sys
    return sys.executable


if __name__ == '__main__':
    main()
