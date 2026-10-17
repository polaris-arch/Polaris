#!/usr/bin/env python3
"""Explicit PRB00/VAL01 producer and App+PacketTunnel link entry; no publish/device action."""
import argparse
import fcntl
import json
import os
import plistlib
from pathlib import Path
import re
import time
import threading
import zipfile

import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, load, require
import producer
from producer import lease, private_dir, run as native_run, progress_write
from artifacts import receipt_digest, file_pin

APPLE_PHASES = frozenset(('component-build-or-revalidate', 'component-import-verification', 'component-link-input',
                         'rust-target-build', 'host-swift-controls',
                         'app-extension-archive', 'final-carrier-inspection'))
PROGRESS_FD = None
CURRENT_PHASE = None
CURRENT_SLOT = None


def run(*args, **kwargs):
    # Observation is never inherited as a descriptor by native grandchildren.
    kwargs.setdefault('observation_fd', PROGRESS_FD)
    kwargs.setdefault('observation_phase', CURRENT_PHASE)
    kwargs.setdefault('observation_slot', CURRENT_SLOT)
    return native_run(*args, **kwargs)


def phase_progress(name, event, elapsed, completed=None):
    require(name in APPLE_PHASES and event in ('start', 'waiting', 'end'), 'unknown Apple progress phase')
    row = {'phase': name, 'slot': CURRENT_SLOT, 'event': event, 'pid': os.getpid(),
           'elapsedSeconds': round(elapsed, 3), 'completed': completed,
           'scope': 'phase wait observation only; not compiler liveness or carrier acceptance'}
    if PROGRESS_FD is None:
        print('APPLE_PHASE_PROGRESS=' + json.dumps(row, sort_keys=True), flush=True)
    else:
        progress_write(PROGRESS_FD, 'APPLE_PHASE_PROGRESS=', row)


def timed_phase(name, operation, *args, **kwargs):
    global CURRENT_PHASE
    previous, CURRENT_PHASE = CURRENT_PHASE, name
    started, completed = time.monotonic(), False
    stopped, observer_errors = threading.Event(), []

    def heartbeat():
        while not stopped.wait(30):
            try:
                phase_progress(name, 'waiting', time.monotonic() - started)
            except BaseException as error:
                observer_errors.append(error)
                return

    monitor = threading.Thread(target=heartbeat, daemon=True)
    try:
        phase_progress(name, 'start', 0)
        monitor.start()
        result = operation(*args, **kwargs)
        if observer_errors:
            raise RuntimeError('Apple phase observation failed') from observer_errors[0]
        completed = True
        return result
    finally:
        stopped.set()
        if monitor.ident is not None:
            monitor.join(timeout=1)
        CURRENT_PHASE = previous
        # Keep an operation failure primary if its output channel also broke.
        failing = sys.exc_info()[0] is not None
        try:
            phase_progress(name, 'end', time.monotonic() - started, completed)
        except BaseException:
            if not failing:
                raise


def host_swift_controls(root, app_env):
    """The pure Foundation executable runs on the host, not an iOS slice."""
    host = os.uname().machine
    require(host in ('arm64', 'x86_64'), 'unsupported actual Mac control architecture')
    minimum = json.loads((ROOT / 'src-tauri/tauri.conf.json').read_bytes())['bundle']['macOS']['minimumSystemVersion']
    require(isinstance(minimum, str) and re.fullmatch(r'[0-9]+\.[0-9]+(?:\.[0-9]+)?', minimum),
            'explicit Mac control deployment target required')
    env = dict(app_env)
    env.pop('IPHONEOS_DEPLOYMENT_TARGET', None)
    sdk = run(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], env=env).strip()
    require(Path(sdk).is_absolute() and Path(sdk).is_dir(), 'actual host Mac SDK required')
    env.update(SDKROOT=sdk, PLATFORM_NAME='macosx', ARCHS=host, MACOSX_DEPLOYMENT_TARGET=minimum)
    unit = root / 'measurement-conditions-test'
    run(['xcrun', '--sdk', 'macosx', 'swiftc', '-sdk', sdk,
         '-target', host + '-apple-macosx' + minimum, '-parse-as-library',
         ROOT / 'crates/tauri-plugin-polaris-ios/ios/Sources/MeasurementDeviceState.swift',
         ROOT / 'crates/tauri-plugin-polaris-ios/ios/Tests/DeviceConditionsTests.swift', '-o', unit], env=env,
        log=root / 'original-swift-control-build.log', capture='none')
    run([unit], env=env, log=root / 'original-swift-control-run.log', capture='none')


def main():
    global PROGRESS_FD, CURRENT_SLOT
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('source', 'go', 'gomobile', 'gobind', 'developer', 'work', 'cache'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--simulator', action='store_true')
    parser.add_argument('--simulator-arch', choices=['arm64'], default='arm64')
    parser.add_argument('--signing-team', help='Explicit device signing identity; omit for unsigned compile/link probe')
    parser.add_argument('--component-work', type=Path, help='One fixed shared two-slice arm64 component slot')
    parser.add_argument('--rust-target', type=Path, help='One fixed Rust target shared under the actual cache lease')
    parser.add_argument('--imported-component', action='store_true', help='Verify same-run frozen component handoff; never reproduce it')
    parser.add_argument('--slot', choices=['device-arm64', 'sim-arm64'])
    parser.add_argument('--progress-fd', type=int, help='Explicit parent stdout descriptor for fixed phase events')
    args = parser.parse_args()
    value = load()
    CURRENT_SLOT = 'sim-' + args.simulator_arch if args.simulator else 'device-arm64'
    require(args.slot is None or args.slot == CURRENT_SLOT, 'selected carrier slot differs')
    require(os.uname().sysname == 'Darwin', 'Apple App producer requires the selected Mac host')
    root, cache = private_dir(args.work), private_dir(args.cache)
    if args.progress_fd is not None:
        require(args.progress_fd > 2, 'dedicated Apple progress descriptor required')
        require(str(args.progress_fd) != os.environ.get('POLARIS_BUILD_RESERVATION_FD'),
                'progress descriptor cannot own a native reservation')
        os.fstat(args.progress_fd)
        # This borrowed stdout copy belongs only to this carrier process.
        # It must not reach compiler/native grandchildren or lease state.
        flags = fcntl.fcntl(args.progress_fd, fcntl.F_GETFD)
        fcntl.fcntl(args.progress_fd, fcntl.F_SETFD, flags | fcntl.FD_CLOEXEC)
        PROGRESS_FD = args.progress_fd
        producer.BORROWED_PROGRESS_FD = PROGRESS_FD
    component_work = private_dir(args.component_work or root / 'component')
    rust_target = private_dir(args.rust_target or root / 'rust-target')
    require(root.stat().st_dev == component_work.stat().st_dev == rust_target.stat().st_dev == cache.stat().st_dev,
            'App/component/Rust/cache slots must share the reserved filesystem')
    from consume import materialize_apple_resources
    with lease(root, cache, value):
        materialize_apple_resources()
    # The component producer owns its own reservation. Do not sell a second
    # nested peak while waiting for it; the App lease below covers real linking.
    if args.imported_component:
        from apple_handoff import verify_import
        with lease(root, cache, value):
            timed_phase('component-import-verification', verify_import,
                        component_work, cache, args.source, args.go, args.gomobile,
                        args.gobind, args.developer)
    else:
        producer_args = [sys_python(), ROOT / 'scripts/mobile-core/producer.py', 'apple']
        for name in ('source', 'go', 'gomobile', 'gobind', 'developer'):
            producer_args += ['--' + name, getattr(args, name)]
        producer_args += ['--work', component_work, '--cache', cache]
        timed_phase('component-build-or-revalidate', run, producer_args,
                    log=root / 'original-component-runner.log', capture='none')
    link = root / 'apple-link-input.json'
    # A retained link context cannot be overwritten as a new App identity.
    if not link.exists():
        timed_phase('component-link-input', run, [sys_python(), ROOT / 'scripts/mobile-core/consume.py', 'apple-link',
            '--component', component_work / 'Libbox.xcframework', '--receipt', component_work / 'component-receipt.json',
            '--work', private_dir(root / 'link-work'), '--cache', cache, '--output', link],
            log=root / 'original-link-input.log', capture='none')
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
                   ARCHS='arm64')
        env['SDKROOT'] = run(['xcrun', '--sdk', platform, '--show-sdk-path'], env=env).strip()
        from budget import native_capacity, native_environment
        observation = native_capacity()
        print(json.dumps({'phase': 'apple-app-build-capacity-sample', 'capacity': observation}, sort_keys=True), flush=True)
        env, capacity = native_environment(env, observation)
        print('NATIVE_CAPACITY_OBSERVATION=' + json.dumps(capacity, sort_keys=True), flush=True)
        # Compile before Xcode starts its build service so the reservation fd,
        # registered compiler group and child lifetime remain observable.
        timed_phase('rust-target-build', run, [sys_python(), ROOT / 'scripts/mobile-core/apple_rust.py', 'build'], env=env,
            log=root / 'original-rust-entry.log', capture='none')
        # Authored pure Swift controls compile on the same real toolchain as the App.
        # This entry runs only in the eventual single uniform Mac validation phase.
        if not args.simulator:
            timed_phase('host-swift-controls', host_swift_controls, root, env)
        # Xcode build-service hooks cannot inherit our reservation descriptor.
        # Inspect optional TLS ABI under the admitted parent, then pass pinned
        # data only to Xcode; final native checks run here after archive.
        tls_root = private_dir(root / 'tls-link-input')
        tls_env = dict(env, DERIVED_FILE_DIR=str(tls_root))
        run([sys_python(), ROOT / 'scripts/mobile-core/apple_tls_fingerprint.py', 'prepare'],
            env=tls_env, log=root / 'original-tls-prepare.log', capture='none')
        env.update(POLARIS_TLS_PREPARED_ROOT=str(tls_root),
                   POLARIS_TLS_PREPARED_SHA256=file_pin(tls_root / 'PolarisTLSFingerprintLinker.json')['sha256'])
        derived = private_dir(root / 'derived-data')
        archive = root / 'Polaris.xcarchive'
        require(not archive.exists(), 'existing actual archive must not be overwritten')
        command = ['xcodebuild', '-project', ROOT / 'src-tauri/gen/apple/polaris.xcodeproj',
                   '-scheme', 'polaris_iOS', '-configuration', 'release', '-derivedDataPath', derived,
                   '-jobs', str(capacity['appliedEnvironment']['CARGO_BUILD_JOBS']),
                   '-destination', 'generic/platform=iOS Simulator' if args.simulator else 'generic/platform=iOS',
                   '-archivePath', archive, 'archive', 'LD_GENERATE_MAP_FILE=YES']
        if args.simulator:
            command += ['ARCHS=arm64', 'ONLY_ACTIVE_ARCH=YES']
        if args.signing_team:
            require(not args.simulator and args.signing_team.isalnum(), 'explicit device signing team differs')
            command += ['DEVELOPMENT_TEAM=' + args.signing_team]
        else:
            command += ['CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO']
        timed_phase('app-extension-archive', run, command, env=env,
                    log=root / 'original-app-extension-link.log', capture='none')
        apps = list((archive / 'Products/Applications').glob('*.app'))
        require(len(apps) == 1 and not apps[0].is_symlink(), 'actual archived host App required')
        extensions = list((apps[0] / 'PlugIns').glob('*.appex'))
        require(len(extensions) == 1 and not extensions[0].is_symlink(), 'one actual PacketTunnel archive required')
        extension_info = plistlib.loads((extensions[0] / 'Info.plist').read_bytes())
        executable_name = extension_info['CFBundleExecutable']
        require(isinstance(executable_name, str) and executable_name and
                Path(executable_name).name == executable_name, 'PacketTunnel executable name differs')
        final_tls_env = dict(env, DERIVED_FILE_DIR=str(tls_root),
                             TARGET_BUILD_DIR=str(extensions[0]), EXECUTABLE_PATH=executable_name)
        run([sys_python(), ROOT / 'scripts/mobile-core/apple_tls_fingerprint.py', 'verify'],
            env=final_tls_env, log=root / 'original-tls-final-verify.log', capture='none')
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
        final_receipt = timed_phase('final-carrier-inspection', apple, ipa, link, file_pin(link)['sha256'], maps[0],
                              component_work / 'component-receipt.json', private_dir(root / 'final-check'),
                              derived, args.simulator_arch if args.simulator else None)
        final_receipt['fingerprint'] = receipt_digest(final_receipt)
        rust_receipts = list(root.glob('rust-*.json'))
        require(len(rust_receipts) == 1, 'one actual Rust target receipt required')
        from artifacts import read_receipt
        rust = read_receipt(rust_receipts[0])
        require(rust['App'] == final_receipt['App'] and rust['devicePassed'] is False and
                rust['fingerprint'] == receipt_digest({k: v for k, v in rust.items() if k != 'fingerprint'}),
                'actual Rust/App source receipt differs')
        final_receipt['rustInput'] = {'receipt': str(rust_receipts[0]), **file_pin(rust_receipts[0])}
        final_receipt['fingerprint'] = receipt_digest({k: v for k, v in final_receipt.items() if k != 'fingerprint'})
        require(not (root / 'app-carrier.json').exists(), 'final carrier receipt already frozen')
        write_json(root / 'app-carrier.json', final_receipt)
        # Neither ad-hoc nor a selected Team here means device/entitlement trust passed.
        print('Actual App/PacketTunnel carrier/source predicate completed; device/signature trust remains separate')


def sys_python():
    import sys
    return sys.executable


if __name__ == '__main__':
    main()
