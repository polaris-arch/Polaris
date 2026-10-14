#!/usr/bin/env python3
"""Build or revalidate one frozen formal mobile component in fixed private slots.

Never invokes upstream helper defaults, installs global tools, publishes a tag,
or issues device acceptance. Cache hits use the same native/content predicate
as fresh outputs. A verified component still needs final App carrier validation.
"""
import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import time

import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, canonical, load, require, sha, verify_source, tool_environment
from android_sdk import prepare_view
from plan import plan
from artifacts import receipt_digest, file_pin, inspect_aar, inspect_framework, module_stream, tree_inventory, read_receipt
from budget import group_alive, register_group, reservation

ACTIVE_FDS = []


def run(args, *, cwd=None, env=None, log=None):
    process = subprocess.Popen([str(a) for a in args], cwd=cwd, env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               start_new_session=True, pass_fds=tuple(ACTIVE_FDS))
    try:
        register_group(process.pid)
        stdout, stderr = process.communicate(timeout=3600)
    except BaseException:
        # Only this newly created compiler/tool session is stopped. Keep the
        # producer reservation until its original direct child has been waited.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            stdout, stderr = process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate()
        if log is not None:
            with Path(log).open('a') as stream:
                stream.write(stdout + stderr)
        register_group(process.pid, finished=True)
        raise
    register_group(process.pid, finished=True)
    require(not group_alive(process.pid), 'native session still owns descendants; original reservation retained')
    if log is not None:
        with Path(log).open('a') as stream:
            stream.write(stdout + stderr)
    require(process.returncode == 0, 'actual tool failed (' + str(args[0]) + '): ' + stderr[-8192:])
    return stdout


def private_dir(path):
    path = Path(path).absolute()
    for parent in (path, *path.parents):
        require(not parent.is_symlink(), 'producer slot parent cannot be redirected')
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and
            stat.S_IMODE(info.st_mode) == 0o700, 'producer slot must be current-user private')
    return path


@contextlib.contextmanager
def slot_locks(work, cache):
    work, cache = private_dir(work), private_dir(cache)
    handles = []
    try:
        for root in sorted({work, cache}):
            fd = os.open(root / '.mobile-producer.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
            info = os.fstat(fd)
            require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1,
                    'producer lock identity differs')
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            handles.append(fd)
            ACTIVE_FDS.append(fd)
        yield work, cache
    finally:
        for fd in reversed(handles):
            ACTIVE_FDS.remove(fd)
            os.close(fd)


@contextlib.contextmanager
def lease(work, cache, value):
    with reservation(work, value, ACTIVE_FDS):
        with slot_locks(work, cache) as paths:
            require(paths[0].stat().st_dev == paths[1].stat().st_dev, 'work/cache must use one reserved filesystem')
            try:
                yield paths
            finally:
                # Same observation on success, failure and cancellation. No
                # unique object/log is silently deleted to make the budget pass.
                report = {}
                for name, root in zip(('work', 'cache'), paths):
                    seen, total = set(), 0
                    for p in root.rglob('*'):
                        if p.is_symlink():
                            continue
                        info = p.stat()
                        if stat.S_ISREG(info.st_mode) and (info.st_dev, info.st_ino) not in seen:
                            seen.add((info.st_dev, info.st_ino))
                            total += info.st_size
                    report[name + 'Bytes'] = total
                space = os.statvfs(paths[0])
                report.update(filesystem=paths[0].stat().st_dev, freeBytes=space.f_bavail * space.f_frsize,
                              cacheMaxBytes=value['budget']['cacheMaxBytes'],
                              cleanupPerformed=False, retainedReason='original components, native inputs and recovery evidence')
                temporary = paths[0] / '.budget-observation.tmp'
                require(not temporary.exists(), 'interrupted budget observation retained')
                with temporary.open('x') as stream:
                    json.dump(report, stream, sort_keys=True)
                temporary.replace(paths[0] / 'budget-observation.json')
                require(report['cacheBytes'] <= value['budget']['cacheMaxBytes'], 'fixed mobile cache budget exceeded')


def executable_driver_identity(path):
    """Bind physical bytes while retaining the name used by LLVM's argv[0] dispatch."""
    driver = Path(path).absolute()
    info = driver.lstat()
    require(stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode), 'compiler driver must be a file or explicit symlink')
    physical = driver.resolve(strict=True)
    return {'path': str(physical), **file_pin(physical),
            'driverPath': str(driver), 'driverMode': stat.S_IMODE(info.st_mode),
            'driverLinkTarget': os.readlink(driver) if stat.S_ISLNK(info.st_mode) else None}


def compiler_fact(path, env):
    identity = executable_driver_identity(path)
    # Resolving ld.lld to lld (or clang++ to clang) changes the actual driver.
    # Call the original bound name, then verify both link and physical input.
    version = run([identity['driverPath'], '--version'], env=env)
    require(executable_driver_identity(path) == identity, 'compiler driver/physical bytes changed during observation')
    return {**identity, 'version': version}


def tool_facts(args, value, env):
    paths = {name: str(Path(getattr(args, name)).resolve(strict=True)) for name in ('go', 'gomobile', 'gobind')}
    require(run([paths['go'], 'version'], env=env).strip().startswith('go version go' + value['goVersion'] + ' '),
            'actual Go tool version differs')
    observed = {}
    for name in ('gomobile', 'gobind'):
        raw = run([paths['go'], 'version', '-m', paths[name]], env=env)
        require(re.search(r'\bgithub.com/sagernet/gomobile\s+' + re.escape(value[name + 'Version']) + r'\b', raw),
                'actual mobile tool module differs: ' + name)
        observed[name] = raw
    result = {'executables': {name: file_pin(path) for name, path in paths.items()}, 'mobileBuildInfo': observed}
    if args.platform == 'android':
        jdk, ndk = Path(args.jdk).resolve(strict=True), Path(args.ndk).resolve(strict=True)
        java = run([jdk / 'bin/java', '--version'], env=env)
        require(re.search(r'^(?:openjdk|java) 17[. ]', java), 'formal JNI requires actual Java17')
        javac = Path(run(['which', 'javac'], env=env).strip()).resolve(strict=True)
        require(javac == (jdk / 'bin/javac').resolve(strict=True), 'actual javac resolution differs from pinned JDK')
        javac_version = run([javac, '--version'], env=env).strip()
        require(re.fullmatch(r'javac 17(?:[.][0-9]+)*(?:[-+].*)?', javac_version), 'actual javac must be Java17')
        paths['javap'] = str(jdk / 'bin/javap')
        host = 'linux-x86_64' if os.uname().sysname == 'Linux' else 'darwin-x86_64'
        binaries = ndk / 'toolchains/llvm/prebuilt' / host / 'bin'
        readelf = executable_driver_identity(binaries / 'llvm-readelf')
        paths['readelf'] = readelf['driverPath']
        compiler_paths = {name: binaries / name for name in
                          ['clang', 'clang++', 'ld.lld', 'llvm-ar']}
        result.update(java={'path': str((jdk / 'bin/java').resolve(strict=True)),
                            **file_pin((jdk / 'bin/java').resolve(strict=True)), 'version': java}, javac={'path': str(javac), **file_pin(javac), 'version': javac_version},
                      ndkProperties=(ndk / 'source.properties').read_text(),
                      readelf={key: readelf[key] for key in ('bytes', 'sha256')}, readelfDriver=readelf, javap=file_pin(paths['javap']),
                      compilers={name: compiler_fact(path, env) for name, path in compiler_paths.items()})
        require(executable_driver_identity(binaries / 'llvm-readelf') == readelf,
                'readelf driver/physical bytes changed during observation')
    else:
        xcode = run(['xcodebuild', '-version'], env=env)
        require('Xcode ' + value['apple']['xcodeVersion'] in xcode and
                'Build version ' + value['apple']['xcodeBuild'] in xcode, 'selected Xcode version/build differs')
        sdk = {}
        for name, key in [('iphoneos', 'iPhoneSDK'), ('iphonesimulator', 'simulatorSDK')]:
            version = run(['xcrun', '--sdk', name, '--show-sdk-version'], env=env).strip()
            require(version == value['apple'][key], 'actual Apple SDK version differs')
            compiler = run(['xcrun', '--sdk', name, '--find', 'clang'], env=env).strip()
            sdk_path = Path(run(['xcrun', '--sdk', name, '--show-sdk-path'], env=env).strip()).resolve(strict=True)
            sdk[name] = {'version': version, 'path': str(sdk_path), 'settings': file_pin(sdk_path / 'SDKSettings.json'),
                         'compiler': file_pin(Path(compiler).resolve(strict=True)),
                         'compilerVersion': run([compiler, '--version'], env=env)}
        result.update(xcode=xcode, sdk=sdk)
    return paths, result


def producer_code():
    files = ['scripts/mobile-core/source-contract.json', 'scripts/mobile-core/fork_input.py',
             'scripts/mobile-core/plan.py', 'scripts/mobile-core/android_sdk.py',
             'scripts/mobile-core/artifacts.py', 'scripts/mobile-core/producer.py',
             'scripts/mobile-core/budget.py', 'scripts/mobile-core/ci_android.py',
             'scripts/mobile-core/consume.py', 'scripts/mobile-core/carriers.py',
             'scripts/mobile-core/apple_rust.py', 'scripts/mobile-core/ci_apple.py',
             'scripts/mobile-core/build_apple_app.py', 'scripts/mobile-core/native_inputs.py',
             'scripts/mobile-core/transaction.py',
             'scripts/libbox-patches/prepare-ndk-linux.py', 'scripts/libbox-patches/ndk-linux-archive.json',
             'scripts/libbox-patches/android-source.py', 'scripts/apple-carrier.py']
    require(not run(['git', 'status', '--porcelain=v1', '--untracked-files=all'], cwd=ROOT), 'App author source must be frozen')
    return {'candidate': run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip(),
            'files': {name: file_pin(ROOT / name) for name in files}}


def verify_receipt(component, receipt, value, binding, tools, run_tool, scratch):
    require(receipt.get('schema') == 'polaris-mobile-component-v1' and receipt.get('platform') == binding['platform'] and
            receipt.get('input') == binding and type(receipt.get('sourceOverlay')) is int and receipt.get('sourceOverlay') == 0 and
            receipt.get('devicePassed') is False and receipt.get('AppAccepted') is False,
            'old/CLI/diagnostic component receipt rejected')
    fp = receipt_digest({k: v for k, v in receipt.items() if k != 'fingerprint'})
    require(fp == receipt.get('fingerprint'), 'component receipt fingerprint differs')
    from native_inputs import verify
    verify(receipt['nativeInputs'], log=Path(component).parent / 'original-bind.log', output=receipt['output'])
    expected = 'polaris-mobile-' + receipt_digest(binding)
    if binding['platform'] == 'android':
        observed = inspect_aar(component, value, binding['moduleGraph'], tools, run_tool, scratch, expected)
    else:
        observed = inspect_framework(component, value, binding['moduleGraph'], expected)
    from artifacts import component_output_identity
    require(component_output_identity(observed) == component_output_identity(receipt['output']),
            'cached component native/content predicate differs')
    return receipt


def equivalent_binding(original, current):
    # Reusing unchanged SDK producer bytes does not relabel its original commit.
    return comparison_binding(original) == comparison_binding(current)


def comparison_binding(value):
    require(isinstance(value, dict) and isinstance(value.get('producer', {}), dict),
            'component input/producer structure differs')
    result = dict(value)
    result['producer'] = {key: item for key, item in value.get('producer', {}).items() if key != 'candidate'}
    return result


def cache_binding_observation(receipt, binding, fingerprint):
    """Fixed field labels/counts only; never export input values or module URLs."""
    left, right = comparison_binding(receipt['input']), comparison_binding(binding)
    known = ('schema', 'platform', 'sourceObservation', 'sourceContract', 'moduleGraph', 'tools', 'producer', 'sourceOverlay')
    def graph(value):
        rows = value.get('moduleGraph')
        if not isinstance(rows, list): return {'rows': None, 'directories': None, 'sums': None}
        return {'rows': len(rows), 'directories': sum(isinstance(row, dict) and bool(row.get('Dir')) for row in rows),
                'sums': sum(isinstance(row, dict) and bool(row.get('Sum')) for row in rows)}
    require(re.fullmatch('[0-9a-f]{64}', fingerprint) and
            re.fullmatch('[0-9a-f]{64}', receipt['fingerprint']), 'actual binding/receipt fingerprint required')
    return {'schema': 'polaris-mobile-cache-binding-observation-v1', 'derivedObservation': True,
            'changedFields': [name for name in known if left.get(name) != right.get(name)],
            'unknownFieldCount': len((set(left) | set(right)) - set(known)),
            'retainedReceiptFingerprint': receipt['fingerprint'], 'currentInputFingerprint': fingerprint,
            'originalModuleGraph': graph(left), 'currentModuleGraph': graph(right),
            'inputValuesExported': False, 'fileContentsExported': False,
            'scope': 'actual comparison before refusal; not component, recovery or App acceptance'}


DOWNLOAD_STDOUT_BYTES, DOWNLOAD_STDERR_BYTES = 1024 ** 2, 4096
DOWNLOAD_LINE_BYTES, DOWNLOAD_LINES, DOWNLOAD_ITEMS = 8192, 32768, 1024
DOWNLOAD_STRING_BYTES, DOWNLOAD_DEPTH, DOWNLOAD_TOKENS = 4096, 8, 65536
DOWNLOAD_SECONDS, DOWNLOAD_CLEANUP_SECONDS = 3600, 2


def bounded_module_download(command, *, cwd, env, expected_count, observation):
    """Only this admission call: capped pipes/JSON, safe failure, no raw log."""
    require(type(expected_count) is int and 0 < expected_count <= DOWNLOAD_ITEMS,
            'bounded module download item count required')
    observation = Path(observation)
    require(not observation.exists() and not observation.is_symlink(), 'retained module download observation')
    deadline = time.monotonic() + DOWNLOAD_SECONDS
    raw, stderr_bytes, process, reason, rows = bytearray(), 0, None, None, []
    lines, line_bytes, string_bytes, depth, tokens, items = 0, 0, 0, 0, 0, 0
    quoted = escaped = False
    cleanup_complete = True
    try:
        process = subprocess.Popen([str(arg) for arg in command], cwd=cwd, env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True, pass_fds=tuple(ACTIVE_FDS))
        register_group(process.pid)
        with selectors.DefaultSelector() as selected:
            for stream, label in [(process.stdout, 'stdout'), (process.stderr, 'stderr')]:
                os.set_blocking(stream.fileno(), False)
                selected.register(stream, selectors.EVENT_READ, label)
            while selected.get_map() and reason is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0: reason = 'deadline'; break
                for key, _ in selected.select(min(0.2, remaining)):
                    used, maximum = (len(raw), DOWNLOAD_STDOUT_BYTES) if key.data == 'stdout' else (stderr_bytes, DOWNLOAD_STDERR_BYTES)
                    chunk = os.read(key.fd, min(4096, maximum - used + 1))
                    if not chunk: selected.unregister(key.fileobj); continue
                    if key.data == 'stderr': stderr_bytes += len(chunk)
                    else: raw.extend(chunk)
                    if used + len(chunk) > maximum: reason = key.data + '-budget'; break
                    if key.data == 'stderr': continue  # Discard contents immediately.
                    for byte in chunk:
                        line_bytes += 1
                        if byte == 10: lines += 1; line_bytes = 0
                        if lines > DOWNLOAD_LINES or line_bytes > DOWNLOAD_LINE_BYTES:
                            reason = 'line-budget'; break
                        if quoted:
                            string_bytes += 1
                            if string_bytes > DOWNLOAD_STRING_BYTES: reason = 'string-budget'; break
                            if escaped: escaped = False
                            elif byte == 92: escaped = True
                            elif byte == 34: quoted = False
                        elif byte == 34: quoted = True; string_bytes = 0
                        elif byte in (123, 91):
                            depth += 1; tokens += 1
                            if depth == 1:
                                items += 1
                                if byte != 123: reason = 'download-json-structure'; break
                            if depth > DOWNLOAD_DEPTH or items > expected_count:
                                reason = 'structure-or-item-budget'; break
                        elif byte in (125, 93): depth -= 1
                        elif byte in (44, 58): tokens += 1
                        if tokens > DOWNLOAD_TOKENS or depth < 0: reason = 'structure-budget'; break
                    if reason is not None: break
        if reason is None:
            try: process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired: reason = 'deadline'
        if reason is None and (process.returncode != 0 or group_alive(process.pid)):
            reason = 'download-exit-or-live-group'
        if reason is None:
            def pairs(entries):
                value = {}
                for key, item in entries:
                    if key in value: raise ValueError()
                    value[key] = item
                return value
            def invalid_constant(_value): raise ValueError()
            decoder = json.JSONDecoder(object_pairs_hook=pairs, parse_constant=invalid_constant)
            text, cursor = raw.decode('utf-8'), 0
            # Go 1.25.5 ModuleJSON also reports the exact version Query.
            allowed = {'Path', 'Version', 'Query', 'Info', 'GoMod', 'Zip', 'Dir', 'Sum', 'GoModSum', 'Origin', 'Reuse'}
            while cursor < len(text):
                if time.monotonic() >= deadline: reason = 'deadline'; break
                while cursor < len(text) and text[cursor].isspace(): cursor += 1
                if cursor == len(text): break
                if len(rows) >= expected_count: reason = 'item-budget'; break
                row, cursor = decoder.raw_decode(text, cursor)
                if not isinstance(row, dict) or 'Error' in row: reason = 'download-json-error'; break
                if not set(row) <= allowed or not all(isinstance(row.get(k), str) for k in ('Path', 'Version', 'Sum', 'Dir')):
                    reason = 'download-json-structure'; break
                if any(not isinstance(value, str) for key, value in row.items() if key not in ('Origin', 'Reuse')):
                    reason = 'download-json-structure'; break
                if 'Reuse' in row and type(row['Reuse']) is not bool:
                    reason = 'download-json-structure'; break
                if 'Origin' in row and (not isinstance(row['Origin'], dict) or
                        not set(row['Origin']) <= {'VCS', 'URL', 'Subdir', 'Hash', 'TagPrefix', 'TagSum', 'Ref', 'RepoSum'} or
                        not all(isinstance(value, str) for value in row['Origin'].values())):
                    reason = 'download-json-structure'; break
                rows.append(row)
            if reason is None and len(rows) != expected_count: reason = 'download-item-count'
    except (OSError, RuntimeError, ValueError, RecursionError):
        reason = reason or 'download-collection-or-json-error'
    except BaseException:
        reason = 'download-interrupted'
    finally:
        if process is not None:
            end = time.monotonic() + DOWNLOAD_CLEANUP_SECONDS
            try:
                if process.poll() is None or group_alive(process.pid):
                    try: os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError: pass
                    try: process.wait(timeout=min(1, max(0.001, end - time.monotonic())))
                    except subprocess.TimeoutExpired: pass
                    if process.poll() is None or group_alive(process.pid):
                        try: os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError: pass
                    if process.poll() is None:
                        process.wait(timeout=max(0.001, end - time.monotonic()))
                    while group_alive(process.pid) and time.monotonic() < end:
                        time.sleep(min(0.02, max(0, end - time.monotonic())))
                cleanup_complete = process.poll() is not None and not group_alive(process.pid)
                if cleanup_complete: register_group(process.pid, finished=True)
                else: reason = 'download-cleanup-incomplete'  # Keep ledger ownership.
            except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired):
                cleanup_complete = False; reason = 'download-cleanup-incomplete'
            finally:
                for stream in (process.stdout, process.stderr): stream.close()
    report = {'schema': 'polaris-module-download-observation-v1', 'derivedObservation': True,
              'complete': reason is None, 'reason': reason, 'cleanupComplete': cleanup_complete,
              'stdoutBytesObserved': len(raw), 'stderrBytesObserved': stderr_bytes,
              'linesObserved': lines, 'itemsObserved': items, 'itemsDecoded': len(rows),
              'limits': {'stdoutBytes': DOWNLOAD_STDOUT_BYTES, 'stderrBytes': DOWNLOAD_STDERR_BYTES,
                         'lineBytes': DOWNLOAD_LINE_BYTES, 'lines': DOWNLOAD_LINES, 'items': expected_count,
                         'stringBytes': DOWNLOAD_STRING_BYTES, 'depth': DOWNLOAD_DEPTH, 'tokens': DOWNLOAD_TOKENS,
                         'seconds': DOWNLOAD_SECONDS, 'cleanupSeconds': DOWNLOAD_CLEANUP_SECONDS},
              'rawOutputRetained': False, 'inputValuesExported': False, 'processRSS': None,
              'scope': 'bounded pipe/JSON collection only; module checksum/cache admission separately required'}
    try: write_json(observation, report)
    except (OSError, RuntimeError, ValueError): raise RuntimeError('module download observation could not be saved') from None
    if reason is not None: raise RuntimeError('module download rejected: ' + reason) from None
    return rows


def admit_module_cache(source, graph, go, env, work):
    """Make already source-summed module directories observable before freezing.

    go list's remote Dir fields depend on DownloadDir availability. Download only
    exact versions already covered by the source go.sum, with an alternate mod
    file in one fixed scratch slot. Never edit the frozen source's mod/sum files.
    """
    source = Path(source)
    candidates = []
    for row in graph:
        node = row.get('Replace', row)
        require(isinstance(node, dict), 'module replacement structure differs')
        if not node.get('Main') and not node.get('Dir') and node.get('Version'): candidates.append(node)
    if not candidates: return
    def source_bytes(name):
        path = source / name
        require(path.stat().st_size <= 1024 ** 2, 'bounded source module input required')
        with path.open('rb') as stream: raw = stream.read(1024 ** 2 + 1)
        require(len(raw) <= 1024 ** 2, 'source module input grew beyond budget')
        return raw
    originals = {name: source_bytes(name) for name in ('go.mod', 'go.sum')}
    sums = {}
    for line in originals['go.sum'].decode().splitlines():
        fields = line.split()
        require(len(fields) == 3, 'source sum record differs')
        path, version, checksum = fields
        if not version.endswith('/go.mod'):
            require((path, version) not in sums or sums[(path, version)] == checksum,
                    'conflicting source module checksum')
            sums[(path, version)] = checksum
    pending = {}
    for node in candidates:
        key = (node.get('Path'), node['Version'])
        if key in sums: pending[key] = sums[key]
    if not pending: return
    require(len(pending) <= 1024 and all(isinstance(path, str) and isinstance(version, str) and
            len(path.encode()) <= 512 and len(version.encode()) <= 128 and
            not any(c.isspace() or c == '@' for c in path + version) for path, version in pending),
            'bounded exact source-summed module arguments required')
    require(sum(len((path + '@' + version).encode()) + 1 for path, version in pending) <= 128 * 1024,
            'module download argument byte budget exceeded')
    view = private_dir(Path(work) / 'module-cache-admission')
    modfile, sumfile = view / 'admission.mod', view / 'admission.sum'
    require(not modfile.exists() and not modfile.is_symlink() and not sumfile.exists() and not sumfile.is_symlink(),
            'prior module admission inputs retained; explicit recovery required')
    for path, raw in [(modfile, originals['go.mod']), (sumfile, originals['go.sum'])]:
        with path.open('xb') as stream: stream.write(raw)
    # -modfile belongs to the download subcommand, not go's global flags.
    downloaded = bounded_module_download([go, 'mod', 'download', '-modfile=' + str(modfile), '-json',
                                         *[path + '@' + version for path, version in sorted(pending)]],
                                        cwd=source, env=env, expected_count=len(pending),
                                        observation=Path(work) / 'module-cache-admission-observation.json')
    require(len(downloaded) == len(pending) and
            {(row['Path'], row.get('Version')) for row in downloaded} == set(pending) and
            all(row.get('Sum') == pending[(row['Path'], row.get('Version'))] and row.get('Dir') and not row.get('Error')
                for row in downloaded), 'actual downloaded module identity differs from frozen source sum')
    require(all(source_bytes(name) == raw for name, raw in originals.items()),
            'source module bytes changed during cache admission')


def write_json(path, value):
    from artifacts import receipt_limit, receipt_chunks, receipt_field_bytes
    # First measure a bounded stream, then write the same stream. Never hold
    # a full canonical string plus a full pretty string/bytes beside the DOM.
    try:
        size = sum(len(raw) for raw in receipt_chunks(value, pretty=True))
    except (RuntimeError, ValueError) as error:
        print(json.dumps({'schema': 'polaris-receipt-size-observation-v1', 'phase': 'bounded-serialization-rejected',
                          'path': str(path), 'maximumBytes': receipt_limit(value), 'error': str(error),
                          'scope': 'no full serialized bytes produced or installed'}), file=sys.stderr, flush=True)
        raise RuntimeError('receipt rejected before installation: ' + str(error)) from error
    print(json.dumps({'schema': 'polaris-receipt-size-observation-v1', 'phase': 'serialize-before-install',
                      'path': str(path), 'bytes': size, 'maximumBytes': receipt_limit(value),
                      'receiptSchema': value.get('schema'),
                      'canonicalValueBytesByTopLevelField': receipt_field_bytes(value),
                      'scope': 'actual serialized size; not installation or carrier acceptance'}), file=sys.stderr, flush=True)
    path = Path(path)
    temporary = path.with_name(path.name + '.tmp')
    created = False
    try:
        with temporary.open('xb') as stream:
            created = True
            written = 0
            for raw in receipt_chunks(value, pretty=True):
                stream.write(raw); written += len(raw)
            require(written == size, 'receipt changed during serialization')
            stream.flush(); os.fsync(stream.fileno())
        temporary.replace(path)
    except BaseException:
        if created: temporary.unlink(missing_ok=True)
        raise


def execute(args):
    value = load()
    producer_identity = producer_code()  # Before any tool/cache/work mutation.
    initial_source = verify_source(args.source, value)
    with lease(args.work, args.cache, value) as (work, cache):
        scratch = private_dir(work / 'scratch')
        # The fixed generator writes source/build/<arch>, despite TMPDIR.
        # One fixed shared-object view per work slot contains that mutation;
        # distinct producers never share its generated directories.
        require(work.is_relative_to(Path.home() / 'Code') or not work.is_relative_to(Path.home()),
                'producer Git build view must be under Code or outside home')
        source = work / 'build-view'
        if not source.exists():
            run(['git', 'clone', '--no-checkout', '--shared', Path(args.source).resolve(strict=True), source])
            run(['git', 'checkout', '--detach', value['sourceCommit']], cwd=source)
        source_observation = verify_source(source, value, generator_outputs=True)
        require(source_observation == initial_source, 'source changed during admission')
        if args.platform == 'android':
            sdk, jar = prepare_view(work, args.sdk, args.ndk, value['android'])
            env = tool_environment(args.go, args.gomobile, args.gobind, cache, scratch,
                                   sdk=sdk, ndk=Path(args.ndk).resolve(), jdk=Path(args.jdk).resolve())
            env['CGO_LDFLAGS'] = '-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384'
        else:
            require(os.uname().sysname == 'Darwin', 'formal Apple bind requires a Mac host')
            jar = None
            env = tool_environment(args.go, args.gomobile, args.gobind, cache, scratch,
                                   developer=Path(args.developer).resolve(strict=True))
        tools, identities = tool_facts(args, value, env)
        # init installs gobind@latest. The pinned generator only requires this
        # private GOPATH marker directory; create it without invoking init.
        private_dir(Path(env['GOPATH']) / 'pkg/gomobile')
        run([tools['go'], 'mod', 'verify'], cwd=source, env=env, log=work / 'original-module-verify.log')
        graph = module_stream(run([tools['go'], 'list', '-m', '-json', 'all'], cwd=source, env=env))
        admit_module_cache(source, graph, tools['go'], env, work)
        require(verify_source(source, value, generator_outputs=True) == source_observation,
                'source changed during module cache admission')
        graph = module_stream(run([tools['go'], 'list', '-m', '-json', 'all'], cwd=source, env=env))
        run([tools['go'], 'mod', 'verify'], cwd=source, env=env, log=work / 'original-module-verify.log')
        # Keep exact paths for native BuildInfo; portable source facts stay separate.
        binding = {'schema': 'polaris-mobile-producer-input-v1', 'platform': args.platform,
                   'sourceObservation': source_observation, 'sourceContract': value, 'moduleGraph': graph,
                   'tools': identities, 'producer': producer_identity, 'sourceOverlay': 0}
        require(producer_code() == producer_identity, 'producer source changed during tool admission')
        fp = receipt_digest(binding)
        component = work / (value['android']['outputName'] if args.platform == 'android' else 'Libbox.xcframework')
        receipt_path = work / 'component-receipt.json'
        from transaction import recover
        stage_root = private_dir(work / 'staging')
        stage_component = stage_root / ('stage.aar' if args.platform == 'android' else 'Libbox.xcframework')
        recover([(stage_component, component),
                 (work / 'stage-component-receipt.json', receipt_path),
                 (work / 'stage-original-bind.log', work / 'original-bind.log')], work)
        run_tool = lambda command: run(command, cwd=source, env=env)
        previous = work / 'previous-pair'
        rebuild = not component.exists()
        if component.exists() or receipt_path.exists():
            require(component.exists() and receipt_path.is_file(), 'incomplete retained output; preserve for explicit recovery')
            receipt = read_receipt(receipt_path)
            if equivalent_binding(receipt.get('input', {}), binding):
                verify_receipt(component, receipt, value, receipt['input'], tools, run_tool, scratch)
            else:
                # One prior output/receipt slot, never per-run source/SDK copies.
                # An occupied prior slot requires explicit ownership/recovery,
                # not a recursive delete of potentially unique evidence.
                require(not previous.exists(), 'prior component recovery slot is occupied')
                require(receipt.get('fingerprint') == receipt_digest({k: v for k, v in receipt.items() if k != 'fingerprint'}),
                        'retained old receipt is corrupt; preserve without replacement')
                if args.platform == 'android':
                    require(file_pin(component) == receipt['output']['artifact'], 'retained old component bytes differ')
                else:
                    require(tree_inventory(component) == receipt['output']['inventory'], 'retained old framework bytes differ')
                observation = work / 'cache-binding-observation.json'
                require(not observation.exists() and not observation.is_symlink(),
                        'prior cache binding observation retained; explicit recovery required')
                write_json(observation, cache_binding_observation(receipt, binding, fp))
                rebuild = True
        if rebuild:
            require(not (source / 'build').exists(),
                    'original generator outputs retained; explicit recovery required before rebinding')
            staged = stage_component
            require(not staged.exists(), 'unaccepted prior stage retained; inspect before recovery')
            recipe = plan(args.platform, value, gomobile=tools['gomobile'], output=staged, bootclasspath=jar)
            from fork_input import bind_command
            command = bind_command(args.platform, value, tools['gomobile'], staged, fp, jar)
            # One direct bind; no helper API21/macOS/tvOS/git-describe side effects.
            log = work / 'stage-original-bind.log'
            require(not log.exists(), 'original bind log retained; archive its identity before another bind')
            # Actual compiler objects must remain observable on a fresh producer
            # even if the Go compiler cache already contains the target.
            env['GODEBUG'] = 'gocacheverify=1'
            run(command, cwd=source, env=env, log=log)
            require(verify_source(source, value, generator_outputs=True) == source_observation, 'source changed during bind')
            require(tool_facts(args, value, env)[1] == identities and producer_code() == binding['producer'], 'producer/tool input changed')
            expected = 'polaris-mobile-' + fp
            output = (inspect_aar(staged, value, graph, tools, run_tool, scratch, expected) if args.platform == 'android'
                      else inspect_framework(staged, value, graph, expected))
            from native_inputs import collect, verify
            origins = collect(scratch, log, args.platform, output, cache)
            if (source / 'build').exists():
                origins['generatorOutputRoot'] = str(source / 'build')
                origins['generatorOutputInventory'] = tree_inventory(source / 'build')
            verify(origins, scratch, log, output=output)
            receipt = {'schema': 'polaris-mobile-component-v1', 'platform': args.platform, 'input': binding,
                       'command': command, 'declaredPlan': recipe['facts'], 'sourceOverlay': 0, 'output': output,
                       'nativeInputs': origins,
                       'AppAccepted': False, 'devicePassed': False, 'publicationEligible': False,
                       'scope': 'actual component bytes/API/source/tool identity only; final App/carrier/device pending'}
            receipt['fingerprint'] = receipt_digest(receipt)
            # Both receipt and log are staged before any accepted bytes move.
            from transaction import install
            staged_receipt = work / 'stage-component-receipt.json'
            write_json(staged_receipt, receipt)
            install([(staged, component), (staged_receipt, receipt_path),
                     (log, work / 'original-bind.log')], work,
                    lambda: verify_receipt(component, receipt, value, binding, tools, run_tool, scratch))
        require(os.statvfs(work).f_bavail * os.statvfs(work).f_frsize >= 1024 ** 3, 'post-build filesystem reserve exhausted')
        print(json.dumps({'component': str(component), 'receipt': str(receipt_path),
                          'fingerprint': receipt['fingerprint'], 'AppAccepted': False, 'devicePassed': False}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('platform', choices=['android', 'apple'])
    for name in ('source', 'go', 'gomobile', 'gobind', 'work', 'cache'):
        parser.add_argument('--' + name, required=True, type=Path)
    for name in ('sdk', 'ndk', 'jdk', 'developer'):
        parser.add_argument('--' + name, type=Path)
    args = parser.parse_args()
    require((args.sdk is not None and args.ndk is not None and args.jdk is not None and args.developer is None)
            if args.platform == 'android' else
            (args.developer is not None and args.sdk is None and args.ndk is None and args.jdk is None), 'platform tools differ')
    execute(args)


if __name__ == '__main__':
    main()
