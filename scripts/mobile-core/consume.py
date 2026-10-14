#!/usr/bin/env python3
"""Verify original formal components, materialize Android input or freeze Apple link input."""
import argparse
import json
import os
from pathlib import Path
import shutil
import stat
import hashlib
import selectors
import signal
import subprocess
import time

import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, canonical, load, require, sha
from artifacts import receipt_digest, component_output_identity, file_pin, inspect_aar, inspect_framework, read_receipt
from producer import ACTIVE_FDS, lease, run, write_json
from budget import group_alive, register_group


def verify_original(component, receipt_path, platform, tools, scratch):
    value = load()
    receipt = read_receipt(Path(receipt_path))
    require(receipt.get('schema') == 'polaris-mobile-component-v1' and receipt.get('platform') == platform and
            type(receipt.get('sourceOverlay')) is int and receipt.get('sourceOverlay') == 0 and receipt.get('AppAccepted') is False and receipt.get('devicePassed') is False,
            'old/CLI/diagnostic mobile component receipt rejected')
    require(receipt['fingerprint'] == receipt_digest({k: v for k, v in receipt.items() if k != 'fingerprint'}), 'component receipt changed')
    from native_inputs import verify
    verify(receipt['nativeInputs'], log=Path(component).parent / 'original-bind.log', output=receipt['output'])
    binding = receipt['input']
    require(binding['sourceContract'] == value and binding['sourceObservation']['sourceCommit'] == value['sourceCommit'] and
            binding['sourceObservation']['sourceTree'] == value['sourceTree'] and binding['sourceOverlay'] == 0,
            'formal component source input differs')
    # Preserve producer candidate, original graph paths and actual BuildID.
    # The consuming App gets its own identity; it does not relabel the producer.
    expected_id = 'polaris-mobile-' + receipt_digest(binding)
    if platform == 'android':
        for name in ('go', 'javap', 'readelf'):
            require(name in tools, 'actual verifier tool missing: ' + name)
        observed = inspect_aar(component, value, binding['moduleGraph'], tools, run, scratch, expected_id)
    else:
        observed = inspect_framework(component, value, binding['moduleGraph'], expected_id)
    current_identity, producer_identity = component_output_identity(observed), component_output_identity(receipt['output'])
    missing = object()
    changed = sorted(key for key in set(current_identity) | set(producer_identity)
                     if current_identity.get(key, missing) != producer_identity.get(key, missing))
    headers = {}
    if platform == 'android':
        for name, row in observed['nativeLibraries'].items():
            if 'goBuildInfo' in row:
                headers[name] = {'observed': row['goBuildInfo'].split('\n', 1)[0],
                                 'producer': receipt['output'].get('nativeLibraries', {}).get(name, {}).get('goBuildInfo', '').split('\n', 1)[0]}
    require(current_identity == producer_identity, 'actual component bytes/API/native identity differ; fields=' + repr(changed) +
            '; originalRawHeaders=' + repr(headers))
    return receipt


STATUS_BYTES, STATUS_STDERR_BYTES, STATUS_PATHS, STATUS_PATH_BYTES = 16384, 1024, 64, 512
STATUS_HASH_BYTES, STATUS_REPORT_BYTES, STATUS_SECONDS = 1024 ** 2, 65536, 10
# Only these reviewed names may be displayed. Unknown names have opaque hashes.
STATUS_DISPLAY_PATHS = frozenset({
    'Cargo.toml', 'Cargo.lock', '.gitignore', 'src-tauri/tauri.ios.conf.json',
    'src-tauri/gen/apple/project.yml', 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj',
    'crates/tauri-plugin-polaris-ios/ios/Package.swift',
    'crates/tauri-plugin-polaris-ios/ios/Package.resolved',
})


def bounded_source_status(command, deadline):
    """Drain capped pipes while the child runs; never communicate/full-buffer it."""
    raw, stderr_bytes, process, reason = bytearray(), 0, None, None
    try:
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True, pass_fds=tuple(ACTIVE_FDS))
        register_group(process.pid)
        with selectors.DefaultSelector() as selected:
            for stream, label in [(process.stdout, 'stdout'), (process.stderr, 'stderr')]:
                os.set_blocking(stream.fileno(), False)
                selected.register(stream, selectors.EVENT_READ, label)
            while selected.get_map() and reason is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    reason = 'deadline'; break
                for key, _ in selected.select(min(0.2, remaining)):
                    used, maximum = (len(raw), STATUS_BYTES) if key.data == 'stdout' else (stderr_bytes, STATUS_STDERR_BYTES)
                    chunk = os.read(key.fd, min(1024, maximum - used + 1))
                    if not chunk:
                        selected.unregister(key.fileobj); continue
                    if key.data == 'stdout': raw.extend(chunk)
                    else: stderr_bytes += len(chunk)  # Content is never exported.
                    if used + len(chunk) > maximum:
                        reason = key.data + '-budget'; break
        if reason is None:
            try: process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired: reason = 'deadline'
        if reason is None and (process.returncode != 0 or group_alive(process.pid)):
            reason = 'git-exit-or-live-group'
    except (OSError, RuntimeError):
        reason = 'collector-error'  # Never copy arbitrary exception/stderr text.
    finally:
        if process is not None:
            try:
                if process.poll() is None or group_alive(process.pid):
                    try: os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError: pass
                    try: process.wait(timeout=1)
                    except subprocess.TimeoutExpired:
                        try: os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError: pass
                        process.wait(timeout=1)
                    if group_alive(process.pid):
                        try: os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError: pass
                register_group(process.pid, finished=True)
            except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired):
                reason = reason or 'cleanup-error'
            finally:
                for stream in (process.stdout, process.stderr): stream.close()
    return bytes(raw), {'statusComplete': reason is None, 'reason': reason,
                        'observedStdoutBytes': len(raw), 'observedStderrBytes': stderr_bytes,
                        'observedStatusPrefixSha256': sha(raw)}


def source_status_observation():
    deadline = time.monotonic() + STATUS_SECONDS
    raw, observation = bounded_source_status(
        ['git', 'status', '--porcelain=v1', '-z', '--untracked-files=all'], deadline)
    rows, hash_bytes, reason = [], 0, observation['reason']
    cursor = 0
    while cursor < len(raw) and len(rows) < STATUS_PATHS:
        if time.monotonic() >= deadline: reason = reason or 'deadline'; break
        end = raw.find(b'\0', cursor)
        if end < 0: reason = reason or 'partial-status'; break
        entry = raw[cursor:end]; cursor = end + 1
        if len(entry) < 4 or entry[2:3] != b' ' or any(v not in b' MADRCU?!T' for v in entry[:2]):
            reason = reason or 'invalid-status'; break
        paths = [('current', entry[3:])]
        if b'R' in entry[:2] or b'C' in entry[:2]:
            end = raw.find(b'\0', cursor)
            if end < 0: reason = reason or 'missing-rename-origin'; break
            paths.append(('rename-origin', raw[cursor:end])); cursor = end + 1
        for role, encoded in paths:
            if len(rows) >= STATUS_PATHS or len(encoded) > STATUS_PATH_BYTES:
                reason = reason or 'path-budget'; break
            row = {'status': entry[:2].decode('ascii'), 'role': role, 'pathBytes': len(encoded),
                   'pathSha256': sha(encoded), 'path': None, 'type': None, 'sha256': None}
            rows.append(row)
            try:
                name = encoded.decode('utf-8')
                rel = Path(name)
                if not name or rel.is_absolute() or '..' in rel.parts:
                    reason = reason or 'invalid-path'; continue
                row['path'] = name if name in STATUS_DISPLAY_PATHS else None
                path = ROOT / rel
                info = path.lstat()
                row.update(bytes=info.st_size, mode=stat.S_IMODE(info.st_mode))
                row['type'] = ('symlink' if stat.S_ISLNK(info.st_mode) else 'file' if stat.S_ISREG(info.st_mode)
                               else 'directory' if stat.S_ISDIR(info.st_mode) else 'other')
                if row['type'] != 'file': continue
                if time.monotonic() >= deadline or info.st_size > STATUS_HASH_BYTES - hash_bytes:
                    reason = reason or 'hash-budget-or-deadline'; continue
                if not path.resolve(strict=True).is_relative_to(ROOT.resolve(strict=True)):
                    reason = reason or 'redirected-path'; continue
                fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
                with os.fdopen(fd, 'rb') as stream:
                    actual = os.fstat(stream.fileno())
                    if (actual.st_dev, actual.st_ino) != (info.st_dev, info.st_ino):
                        reason = reason or 'changed-file'; continue
                    digest, count = hashlib.sha256(), 0
                    while True:
                        if time.monotonic() >= deadline:
                            reason = reason or 'deadline'; break
                        chunk = stream.read(min(8192, STATUS_HASH_BYTES - hash_bytes + 1))
                        if not chunk: break
                        count += len(chunk); hash_bytes += len(chunk)
                        if hash_bytes > STATUS_HASH_BYTES:
                            reason = reason or 'hash-budget'; break
                        digest.update(chunk)
                    after = os.fstat(stream.fileno())
                    if count == info.st_size and (after.st_size, after.st_mtime_ns) == (info.st_size, info.st_mtime_ns):
                        row['sha256'] = digest.hexdigest()
                    else: reason = reason or 'incomplete-or-changed-file'
            except FileNotFoundError:
                row['type'] = 'absent'
                if b'D' not in entry[:2]: reason = reason or 'metadata-missing'
            except (OSError, UnicodeError, ValueError):
                reason = reason or 'metadata-error'
    if cursor < len(raw): reason = reason or 'path-budget'
    observation.update(schema='polaris-app-source-status-observation-v1', complete=reason is None,
                       reason=reason, clean=not raw and observation['statusComplete'], paths=rows,
                       hashBytesRead=hash_bytes, rawStatusRetained=False, fileContentsExported=False,
                       processRSS=None,
                       scope='derived bounded metadata; exact reviewed names only; no raw stdout/stderr/diff/file bodies',
                       limits={'stdoutBytes': STATUS_BYTES, 'stderrBytes': STATUS_STDERR_BYTES,
                               'paths': STATUS_PATHS, 'pathBytes': STATUS_PATH_BYTES,
                               'hashBytes': STATUS_HASH_BYTES, 'reportBytes': STATUS_REPORT_BYTES,
                               'seconds': STATUS_SECONDS, 'pipeOverflowSentinelBytes': 1,
                               'hashOverflowSentinelBytes': 1, 'terminationGraceSeconds': 2})
    return observation


def app_identity(*, status_log=None):
    try:
        observation = source_status_observation()
    except (OSError, RuntimeError, ValueError):
        observation = {'schema': 'polaris-app-source-status-observation-v1', 'complete': False,
                       'clean': False, 'statusComplete': False, 'paths': [], 'reason': 'collector-error',
                       'rawStatusRetained': False, 'fileContentsExported': False, 'processRSS': None,
                       'scope': 'incomplete derived metadata; no raw error text or original source export'}
    if status_log is not None:
        status_log = Path(status_log)
        require(not status_log.exists() and not status_log.is_symlink(), 'source status observation already frozen')
        raw = json.dumps(observation, ensure_ascii=True, sort_keys=True).encode()
        if len(raw) > STATUS_REPORT_BYTES:
            observation.update(complete=False, reason='report-budget', paths=[], omittedPathCount=len(observation['paths']))
            raw = json.dumps(observation, ensure_ascii=True, sort_keys=True).encode()
        require(len(raw) <= STATUS_REPORT_BYTES, 'source status observation exceeds fixed report budget')
        try:
            with status_log.open('xb') as stream: stream.write(raw)
        except OSError:
            raise RuntimeError('incomplete source status observation could not be written') from None
    require(observation['complete'] and observation['clean'],
            'App consumer source is not frozen; observationComplete=' + str(observation['complete']) +
            '; observedPaths=' + str(len(observation['paths'])) + '; reason=' + str(observation['reason']))
    return {'candidate': run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip(),
            'consumerCode': {name: file_pin(ROOT / name) for name in
                ['scripts/mobile-core/consume.py', 'scripts/mobile-core/artifacts.py',
                 'scripts/mobile-core/fork_input.py', 'scripts/mobile-core/carriers.py',
                 'scripts/mobile-core/source-contract.json', 'scripts/mobile-core/transaction.py',
                 'scripts/mobile-core/native_inputs.py', 'src-tauri/gen/apple/project.yml',
                 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj',
                 'scripts/verify-apk.mjs', 'scripts/assert-r8-evidence.mjs',
                 'scripts/release-assets.mjs', 'scripts/mobile-core/apple_rust.py', 'scripts/mobile-core/ci_apple.py',
             'scripts/mobile-core/build_apple_app.py', 'scripts/mobile-core/apple_handoff.py',
                 'scripts/mobile-core/apple_admission.py']}}


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
                    existing.get('fingerprint') == receipt_digest({k: v for k, v in existing.items() if k != 'fingerprint'}) and
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
        facts['fingerprint'] = receipt_digest(facts)
        from transaction import install
        staged_receipt = work / 'stage-app-consumption.json'
        write_json(staged_receipt, facts)
        def installed():
            require(file_pin(dest) == receipt['output']['artifact'] and app_identity() == identity,
                    'installed input or consumer source changed')
            require(read_receipt(target_receipt) == facts, 'installed consumption receipt changed')
        install([(stage, dest), (staged_receipt, target_receipt)], work, installed)
        return facts


def apple_resource_inputs():
    """The actual iOS bundle config is the resource inventory, not a placeholder."""
    config = json.loads((ROOT / 'src-tauri/tauri.ios.conf.json').read_bytes())
    require(config['bundle']['resources'] == ['../resources/data/', '../THIRD-PARTY-LICENSES.md',
                                             '../NOTICE', '../LICENSE'], 'iOS resource contract changed')
    data = ROOT / 'resources/data'
    require(not data.is_symlink() and data.is_dir(), 'real geo resource directory required')
    geo = sorted(data.iterdir())
    require(len(geo) == 28 and all(p.suffix == '.srs' for p in geo), 'complete 28 geo resource inventory required')
    sources = geo + [ROOT / name for name in ('THIRD-PARTY-LICENSES.md', 'NOTICE', 'LICENSE')]
    rows = {}
    for source in sources:
        for parent in (source, *source.parents):
            require(not parent.is_symlink(), 'resource source redirected')
            if parent == ROOT:
                break
        require(source.is_file(), 'resource source must be regular')
        pin = file_pin(source)
        if source.suffix == '.srs':
            require(source.read_bytes().startswith(b'SRS'), 'resource geo bytes lack SRS magic')
        mode = stat.S_IMODE(source.stat().st_mode)
        require(mode == 0o644, 'resource source mode differs')
        name = '_up_/' + source.relative_to(ROOT).as_posix()
        rows[name] = {'source': source.relative_to(ROOT).as_posix(), **pin, 'mode': mode}
    return rows


def apple_resources():
    rows = apple_resource_inputs()
    assets = ROOT / 'src-tauri/gen/apple/assets'
    require(not assets.is_symlink() and assets.is_dir(), 'materialized Apple resources required before input freeze')
    actual = set()
    for path in assets.rglob('*'):
        require(not path.is_symlink(), 'Apple resource output redirected')
        if path.is_file():
            name = path.relative_to(assets).as_posix()
            require(name in rows and file_pin(path) == {k: rows[name][k] for k in ('bytes', 'sha256')} and
                    stat.S_IMODE(path.stat().st_mode) == rows[name]['mode'], 'materialized Apple resource differs')
            actual.add(name)
        else:
            require(path.is_dir() and any(name.startswith(path.relative_to(assets).as_posix() + '/') for name in rows),
                    'unbound Apple resource directory')
    require(actual == set(rows), 'materialized Apple resource inventory differs')
    return rows


def materialize_apple_resources():
    rows = apple_resource_inputs()
    assets = ROOT / 'src-tauri/gen/apple/assets'
    for parent in (assets, *assets.parents):
        require(not parent.is_symlink(), 'Apple resource destination redirected')
        if parent == ROOT:
            break
    if assets.exists():
        require(apple_resources() == rows, 'retained Apple resource output differs')
        return rows
    stage = assets.with_name('assets.formal-stage')
    require(not stage.exists(), 'unaccepted Apple resource stage retained')
    stage.mkdir(mode=0o700)
    for name, row in rows.items():
        dest = stage / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / row['source'], dest)
        dest.chmod(row['mode'])
        require(file_pin(dest) == {k: row[k] for k in ('bytes', 'sha256')}, 'resource copy differs')
    require(apple_resource_inputs() == rows, 'resource source changed during materialization')
    stage.chmod(0o755)
    os.rename(stage, assets)
    require(apple_resources() == rows, 'installed Apple resource output differs')
    return rows


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
                 'componentOutput': receipt['output'], 'resources': apple_resources(), 'staticOnly': True,
                 'AppAccepted': False, 'devicePassed': False}
        facts['fingerprint'] = receipt_digest(facts)
        require(not Path(output).exists(), 'fixed link input already exists; do not overwrite a frozen context')
        write_json(output, facts)
        return facts


def verify_link(path, expected_sha, scratch):
    require(file_pin(path)['sha256'] == expected_sha, 'fixed Apple link input changed')
    link = read_receipt(Path(path))
    require(link.get('schema') == 'polaris-mobile-apple-link-input-v1' and link.get('staticOnly') is True and
            link.get('fingerprint') == receipt_digest({k: v for k, v in link.items() if k != 'fingerprint'}), 'Apple link context differs')
    require(file_pin(link['componentReceipt'])['sha256'] == link['componentReceiptSha256'], 'original component receipt changed')
    receipt = verify_original(link['framework'], link['componentReceipt'], 'apple', {}, scratch)
    require(receipt['fingerprint'] == link['componentFingerprint'] and receipt['output'] == link['componentOutput'] and
            app_identity() == link['App'] and apple_resources() == link['resources'],
            'actual component/App/resource link context changed')
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
