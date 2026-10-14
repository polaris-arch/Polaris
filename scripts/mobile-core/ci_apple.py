#!/usr/bin/env python3
"""One hosted unsigned Apple SDK/App/PacketTunnel chain; no publish/device action."""
import argparse
import contextlib
import json
import os
from pathlib import Path
import shutil
import sys
import time
import zipfile

sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from artifacts import file_pin, read_receipt
from ci_android import prepare_source
from consume import materialize_apple_resources
from fork_input import ROOT, canonical, load, require, sha
from producer import lease, private_dir, producer_code, run, write_json

CARRIERS = [('device-arm64', None), ('sim-arm64', 'arm64'), ('sim-amd64', 'amd64')]


@contextlib.contextmanager
def phase_timing(name):
    """Original elapsed observation only; never a carrier acceptance receipt."""
    started = time.monotonic()
    completed = False
    try:
        yield
        completed = True
    finally:
        print('APPLE_PHASE_TIMING=' + json.dumps({'phase': name,
              'elapsedSeconds': round(time.monotonic() - started, 3),
              'completed': completed, 'scope': 'elapsed observation only'},
              sort_keys=True), flush=True)


def location():
    require(os.environ.get('GITHUB_ACTIONS') == 'true' and os.uname().sysname == 'Darwin',
            'hosted Mac entry only; explicit local builder has separate admission')
    return private_dir(Path.home() / 'Code/polaris-mobile-apple-ci')


def prepare():
    value = load()
    identity = producer_code()
    root = location()
    expected = os.environ['POLARIS_APP_CANDIDATE']
    require(run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip() == expected, 'hosted App candidate differs')
    developer = Path('/Applications/Xcode_27.1_beta.app/Contents/Developer').resolve(strict=True)
    env = dict(os.environ, DEVELOPER_DIR=str(developer))
    xcode = run(['xcodebuild', '-version'], env=env).strip()
    require(xcode == 'Xcode ' + value['apple']['xcodeVersion'] + '\nBuild version ' + value['apple']['xcodeBuild'],
            'runner does not provide the exact selected Xcode; no fallback/skip')
    for sdk, key in [('iphoneos', 'iPhoneSDK'), ('iphonesimulator', 'simulatorSDK')]:
        require(run(['xcrun', '--sdk', sdk, '--show-sdk-version'], env=env).strip() == value['apple'][key],
                'runner does not provide the exact selected SDK')
    tools = private_dir(root / 'tools')
    tool_cache = private_dir(root / 'tool-cache')
    go = Path(shutil.which('go')).resolve(strict=True)
    require(run([go, 'version']).startswith('go version go' + value['goVersion'] + ' '), 'actual Go differs')
    env.update(GOBIN=str(tools), GOTOOLCHAIN='local', GOWORK='off', GOFLAGS='-mod=readonly',
               GOPATH=str(tool_cache / 'go-path'), GOCACHE=str(tool_cache / 'go-build'),
               GOMODCACHE=str(tool_cache / 'go-mod'))
    from budget import native_capacity, native_environment
    observation = native_capacity()
    print(json.dumps({'phase': 'apple-tool-prepare-capacity-sample', 'capacity': observation}, sort_keys=True), flush=True)
    env, capacity = native_environment(env, observation)
    print('NATIVE_CAPACITY_OBSERVATION=' + json.dumps(capacity, sort_keys=True), flush=True)
    run([go, 'install', 'github.com/sagernet/gomobile/cmd/gomobile@' + value['gomobileVersion'],
         'github.com/sagernet/gomobile/cmd/gobind@' + value['gobindVersion']], env=env,
        log=root / 'original-generator-prepare.log')
    paths = {'go': str(go), 'gomobile': str(tools / 'gomobile'), 'gobind': str(tools / 'gobind'),
             'developer': str(developer), 'source': str(root / 'fork-source'),
             'component-work': str(private_dir(root / 'component')), 'cache': str(private_dir(root / 'cache')),
             'rust-target': str(private_dir(root / 'cache/rust-target'))}
    prepare_source(paths['source'], value)
    require(producer_code() == identity, 'App producer source changed during preparation')
    facts = {'App': identity, 'paths': paths, 'xcode': xcode,
             'tools': {name: file_pin(paths[name]) for name in ('go', 'gomobile', 'gobind')},
             'rustc': run(['rustc', '-vV']), 'devicePassed': False}
    facts['fingerprint'] = sha(canonical(facts))
    write_json(root / 'tools.json', facts)
    with Path(os.environ['GITHUB_ENV']).open('a') as stream:
        stream.write('POLARIS_FORMAL_APPLE_ROOT=' + str(root) + '\nDEVELOPER_DIR=' + str(developer) + '\n')
    print('Exact Apple source/tool slots prepared; no compiler, signing or device action')


def build():
    root = location()
    paths = read_receipt(root / 'tools.json')['paths']
    # The existing Tauri beforeBuild commands belong before source/link freeze.
    # Their outputs are ignored generated UI/resources, not mutations of PBX/Cargo.
    with phase_timing('frontend-resources'):
        with lease(private_dir(root / 'frontend-work'), Path(paths['cache']), load()):
            run(['node', ROOT / 'scripts/fetch-dashboard.mjs'], cwd=ROOT, log=root / 'original-dashboard.log')
            run(['pnpm', '--dir', ROOT / 'ui', 'build'], cwd=ROOT, log=root / 'original-frontend.log')
            materialize_apple_resources()
    for slot, simulator in CARRIERS:
        command = [sys.executable, ROOT / 'scripts/mobile-core/build_apple_app.py']
        for name, path in paths.items():
            command += ['--' + name, path]
        command += ['--work', private_dir(root / slot)]
        if simulator:
            command += ['--simulator', '--simulator-arch', simulator]
        with phase_timing(slot):
            run(command, cwd=ROOT, log=root / ('original-' + slot + '.log'))
    receipts = {slot: read_receipt(root / slot / 'app-carrier.json') for slot, _ in CARRIERS}
    require(all(row.get('devicePassed') is False for row in receipts.values()), 'unsigned CI cannot accept a device')
    result = {'schema': 'polaris-apple-unsigned-chain-v1', 'App': producer_code(),
              'carriers': {slot: file_pin(root / slot / 'app-carrier.json') for slot in receipts},
              'devicePassed': False, 'signatureTrustValidated': False, 'publicationEligible': False}
    result['fingerprint'] = sha(canonical(result))
    write_json(root / 'apple-chain.json', result)


def evidence():
    root = location()
    cache = private_dir(root / 'cache')
    # Both failed and successful builds retain original evidence. Package only
    # selected bytes, never an entire cache, repeated checkout or target tree.
    with lease(private_dir(root / 'evidence-work'), cache, load()):
        paths = set(root.glob('*.json')) | set(root.glob('*.log'))
        component = root / 'component'
        paths.update(component.glob('*.json'))
        paths.update(component.glob('*.log'))
        frameworks = [component / 'Libbox.xcframework', component / 'staging/Libbox.xcframework']
        for framework in frameworks:
            if framework.exists():
                # Failed inspection leaves the real generator output in staging.
                # Preserve those bytes as unaccepted evidence, never a final receipt.
                paths.update(path for path in framework.rglob('*') if path.is_file() or path.is_symlink())
        receipt = component / 'component-receipt.json'
        receipt_errors = {}
        origins = None
        if receipt.exists():
            try:
                origins = read_receipt(receipt)['nativeInputs']
                require(isinstance(origins, dict) and isinstance(origins.get('files'), dict),
                        'original native input inventory differs')
            except (RuntimeError, ValueError, OSError, KeyError, TypeError) as error:
                receipt_errors[receipt.relative_to(root).as_posix()] = str(error)
                origins = None
        if origins is not None:
            paths.update(Path(path) for path in origins['files'])
            if 'generatorOutputRoot' in origins:
                paths.update(path for path in Path(origins['generatorOutputRoot']).rglob('*') if path.is_file())
        for slot, _ in CARRIERS:
            work = root / slot
            paths.update(work.glob('*.json'))
            paths.update(work.glob('*.log'))
            paths.update(work.glob('*.ipa'))
            paths.update(work.glob('wire-*.a'))
            paths.update((work / 'derived-data').rglob('*LinkMap*.txt'))
            # A failed symbol check has no accepted Rust receipt yet. Preserve
            # its actual archive as unaccepted evidence under the same budget.
            from apple_rust import target_for
            target = target_for('iphoneos' if slot == 'device-arm64' else 'iphonesimulator',
                                'x86_64' if slot == 'sim-amd64' else 'arm64', 'release')
            archive = root / 'cache/rust-target' / target / 'release/libpolaris_lib.a'
            if archive.is_file():
                paths.add(archive)
            for receipt in work.glob('rust-*.json'):
                try:
                    target = read_receipt(receipt)['target']
                    require(isinstance(target, str) and target in
                            ('aarch64-apple-ios', 'aarch64-apple-ios-sim', 'x86_64-apple-ios'),
                            'original Rust target differs')
                except (RuntimeError, ValueError, OSError, KeyError, TypeError) as error:
                    receipt_errors[receipt.relative_to(root).as_posix()] = str(error)
                    continue
                paths.add(root / 'cache/rust-target' / target / 'release/libpolaris_lib.a')
        output = root / 'apple-original-evidence.zip'
        require(not output.exists(), 'previous original evidence must not be overwritten')
        rows, total = {}, 0
        with zipfile.ZipFile(output, 'x', compression=zipfile.ZIP_DEFLATED) as package:
            for path in sorted(paths):
                require(path.resolve(strict=True).is_relative_to(root),
                        'original Apple evidence escaped its fixed slots')
                name = path.relative_to(root).as_posix()
                if path.is_symlink():
                    require(any(path.is_relative_to(framework) for framework in frameworks),
                            'only actual component/staged framework links may be retained')
                    import stat
                    info = zipfile.ZipInfo(name)
                    info.create_system = 3
                    info.external_attr = (stat.S_IFLNK | 0o777) << 16
                    package.writestr(info, os.readlink(path))
                    rows[name] = {'originalPath': str(path), 'link': os.readlink(path)}
                    continue
                pin = file_pin(path, allow_empty=True)
                total += pin['bytes']
                require(total <= 4 * 1024 ** 3, 'selected original evidence exceeds 4GiB budget')
                package.write(path, name)
                rows[name] = {'originalPath': str(path), **pin}
            index = {'schema': 'polaris-apple-original-evidence-v1', 'files': rows,
                     'completeUnsignedChain': (root / 'apple-chain.json').exists() and not receipt_errors,
                     'componentAccepted': origins is not None,
                     'receiptReadErrors': receipt_errors,
                     'receiptBytes': {name: row['bytes'] for name, row in rows.items()
                                      if name.endswith('.json') and 'bytes' in row},
                     'stagedComponentIsAccepted': False,
                     'devicePassed': False, 'publicationEligible': False}
            package.writestr('EVIDENCE-INDEX.json', json.dumps(index, indent=2) + '\n')
        print('Original selected bytes/paths preserved; unsigned chain is not device or signing acceptance')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'build', 'evidence'])
    {'prepare': prepare, 'build': build, 'evidence': evidence}[parser.parse_args().action]()
