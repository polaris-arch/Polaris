#!/usr/bin/env python3
"""Build or revalidate one frozen formal mobile component in fixed private slots.

Never invokes upstream helper defaults, installs global tools, publishes a tag,
or issues device acceptance. Cache hits use the same native/content predicate
as fresh outputs. A verified component still needs final App carrier validation.
"""
import argparse
import codecs
import contextlib
import fcntl
import io
import json
import math
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

from fork_input import git_environment, require_git_root, ROOT, canonical, load, require, sha, verify_source, tool_environment
from android_sdk import prepare_view
from plan import plan
from artifacts import receipt_digest, file_pin, inspect_aar, inspect_framework, module_stream, tree_inventory, read_receipt, failure_note
from budget import group_alive, register_group, reservation, native_capacity, native_environment

ACTIVE_FDS = []
BORROWED_PROGRESS_FD = None
BORROWED_PROGRESS_PHASE = None


RUN_SECONDS = 3600
RUN_TERM_SECONDS = 10
RUN_HEARTBEAT_SECONDS = 30
RUN_CHUNK_BYTES = 65536
RUN_ERROR_CHARS = 8192
OBSERVATION_PHASES = frozenset((
    'component-build-or-revalidate', 'component-import-verification', 'component-link-input',
    'rust-target-build', 'host-swift-controls', 'app-extension-archive',
    'final-carrier-inspection', 'carrier-command', 'build-settings-admission',
    'tool-prepare', 'frontend-dashboard', 'frontend-build'))
OBSERVATION_SLOTS = frozenset(('device-arm64', 'sim-arm64'))


def progress_write(fd, prefix, row):
    """One fixed-schema small event. Callers never pass native text or env."""
    raw = (prefix + json.dumps(row, sort_keys=True) + '\n').encode('ascii')
    require(len(raw) <= 4096, 'progress event exceeds fixed bound')
    view = memoryview(raw)
    while view:
        written = os.write(fd, view)
        require(written > 0, 'incomplete tool progress output')
        view = view[written:]



def inspection_progress(stage, elapsed_seconds, file_reads, read_bytes):
    require(stage in ('verify-original', 'ipa-parse', 'static-linkmap', 'framework-inspection'), 'unknown inspection progress stage')
    require(isinstance(elapsed_seconds, (int, float)) and 0 <= elapsed_seconds < 86400 and
            all(value is None or (type(value) is int and value >= 0) for value in (file_reads, read_bytes)),
            'invalid inspection progress measurement')
    row = {'schema': 'polaris-apple-inspection-observation-v1', 'stage': stage,
           'elapsedSeconds': round(elapsed_seconds, 3), 'fileReads': file_reads,
           'readBytes': read_bytes, 'scope': 'local reads and elapsed only; no acceptance'}
    if BORROWED_PROGRESS_FD is None:
        print('APPLE_INSPECTION_PROGRESS=' + json.dumps(row, sort_keys=True), file=sys.stderr, flush=True)
    else:
        progress_write(BORROWED_PROGRESS_FD, 'APPLE_INSPECTION_PROGRESS=', row)


@contextlib.contextmanager
def _run_log(log, *, read_back=False):
    if log is None:
        yield None
        return
    resource = Path(log).open('a+b' if read_back else 'ab')
    sink = resource.__enter__()
    try:
        yield sink
    finally:
        original = sys.exc_info()
        try:
            resource.__exit__(*original)
        except BaseException as error:
            if original[0] is None:
                failure_note(error, 'original tool log incomplete; close failed')
                raise
            failure_note(original[1], 'original tool log incomplete; secondary close failed')


def _write_run_log(sink, raw):
    try:
        require(sink.write(raw) == len(raw), 'original tool log incomplete: short write')
    except BaseException as error:
        failure_note(error, 'original tool log incomplete; write failed')
        raise


def _project_run_logs(sink, raw_sinks, starts, counts, encoding, errors):
    """Append this call's stdout then stderr with bounded decode/error state."""
    tail, decode_error = '', None
    for stream in ('stdout', 'stderr'):
        source = raw_sinks[stream]
        source.flush()
        source.seek(starts[stream])
        remaining = counts[stream]
        decoder = io.IncrementalNewlineDecoder(codecs.getincrementaldecoder(encoding)(errors), True)
        while remaining:
            raw = source.read(min(RUN_CHUNK_BYTES, remaining))
            require(raw, 'original tool log incomplete: raw range ended early')
            remaining -= len(raw)
            _write_run_log(sink, raw)
            if decoder is not None:
                try:
                    text = decoder.decode(raw)
                    if stream == 'stderr':
                        tail = (tail + text)[-RUN_ERROR_CHARS:]
                except UnicodeError as error:
                    # Keep the complete original projection even when the
                    # historical strict text contract will reject these bytes.
                    if decode_error is None:
                        decode_error = error
                    decoder = None
        if decoder is not None:
            try:
                text = decoder.decode(b'', final=True)
                if stream == 'stderr':
                    tail = (tail + text)[-RUN_ERROR_CHARS:]
            except UnicodeError as error:
                if decode_error is None:
                    decode_error = error
    sink.flush()
    if decode_error is not None:
        raise decode_error
    return tail


def run(args, *, cwd=None, env=None, log=None, capture='full', progress_fd=None,
        observation_fd=None, observation_phase=None, observation_slot=None):
    if args and str(args[0]) == 'git':
        env = git_environment(env)
        if cwd is not None:
            require_git_root(cwd)
    require(capture in ('full', 'none'), 'unknown tool capture mode')
    require(capture != 'none' or log is not None, 'capture none requires complete original logs')
    if observation_phase is None and BORROWED_PROGRESS_FD is not None:
        observation_phase = BORROWED_PROGRESS_PHASE
    # Only the explicit carrier receives the borrowed progress descriptor.
    # Native grandchildren inherit reservation descriptors, never progress_fd.
    require(observation_phase is None or observation_phase in OBSERVATION_PHASES,
            'unknown tool observation phase')
    require(observation_slot is None or observation_slot in OBSERVATION_SLOTS,
            'unknown tool observation slot')
    extra_fds = () if progress_fd is None else (progress_fd,)
    event_fd = (observation_fd if observation_fd is not None else
                progress_fd if progress_fd is not None else BORROWED_PROGRESS_FD)
    require(BORROWED_PROGRESS_FD not in ACTIVE_FDS, 'progress descriptor cannot own a native lease')
    # Preserve each original stream separately while it is still growing.
    # The consumer projection remains complete stdout followed by stderr;
    # arbitrary pipe scheduling must never splice UTF-8/WORK/merge lines.
    stdout_log = None if log is None else str(log) + '.stdout.raw'
    stderr_log = None if log is None else str(log) + '.stderr.raw'
    with _run_log(log) as sink, _run_log(stdout_log, read_back=capture == 'none') as out_sink, \
            _run_log(stderr_log, read_back=capture == 'none') as err_sink:
        raw_sinks = {'stdout': out_sink, 'stderr': err_sink}
        starts = ({stream: stream_sink.tell() for stream, stream_sink in raw_sinks.items()}
                  if capture == 'none' else None)
        selected = selectors.DefaultSelector()
        try:
            process = subprocess.Popen([str(a) for a in args], cwd=cwd, env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                       start_new_session=True, pass_fds=tuple(ACTIVE_FDS) + extra_fds)
        except BaseException as original:
            try:
                selected.close()
            except BaseException:
                failure_note(original, 'secondary selector close failed after spawn failure')
            raise
        encoding, errors = process.stdout.encoding, process.stdout.errors
        buffers = {'stdout': [], 'stderr': []} if capture == 'full' else None
        counts = {'stdout': 0, 'stderr': 0}
        cleanup_log_failed = False
        started, next_observation = time.monotonic(), 0

        def observe(event):
            if event_fd is None:
                return
            try:
                load1 = os.getloadavg()[0]
                if not math.isfinite(load1) or load1 < 0:
                    load1 = None
            except (OSError, AttributeError):
                load1 = None
            progress_write(event_fd, 'APPLE_TOOL_PROGRESS=', {
                'event': event, 'phase': observation_phase, 'slot': observation_slot,
                'pid': process.pid, 'elapsedSeconds': round(time.monotonic() - started, 3),
                'stdoutBytes': counts['stdout'], 'stderrBytes': counts['stderr'],
                'processRunning': process.poll() is None, 'load1': load1, 'peakRSSBytes': None,
                'scope': 'direct process/pipe bytes only; no descendant liveness or acceptance'})

        def drain(deadline, *, cleanup=False):
            nonlocal next_observation, cleanup_log_failed
            while selected.get_map() or process.poll() is None:
                now = time.monotonic()
                if now >= deadline:
                    raise subprocess.TimeoutExpired(str(args[0]), RUN_SECONDS)
                if not cleanup and now >= next_observation:
                    observe('waiting')
                    next_observation = now + RUN_HEARTBEAT_SECONDS
                for key, _ in selected.select(min(1, max(0, deadline - now))):
                    raw = os.read(key.fileobj.fileno(), RUN_CHUNK_BYTES)
                    if not raw:
                        selected.unregister(key.fileobj)
                        key.fileobj.close()
                        continue
                    if buffers is not None:
                        buffers[key.data].append(raw)
                    counts[key.data] += len(raw)
                    stream_sink = raw_sinks[key.data]
                    if stream_sink is not None:
                        if cleanup:
                            # A secondary log failure must not interrupt pipe
                            # draining or replace the triggering exception.
                            try:
                                _write_run_log(stream_sink, raw)
                                stream_sink.flush()
                            except Exception:
                                cleanup_log_failed = True
                        else:
                            try:
                                _write_run_log(stream_sink, raw)
                                stream_sink.flush()
                            except BaseException as error:
                                failure_note(error, 'original tool log incomplete; raw storage failed')
                                raise
            process.wait()

        try:
            selected.register(process.stdout, selectors.EVENT_READ, 'stdout')
            selected.register(process.stderr, selectors.EVENT_READ, 'stderr')
            register_group(process.pid)
            observe('start')
            drain(started + RUN_SECONDS)
            require(not group_alive(process.pid),
                    'native session still owns descendants; original reservation retained')
            register_group(process.pid, finished=True)
            observe('end')
        except BaseException as original:
            # All failures, including output/observer and cancellation errors,
            # retain the original exception while stopping this session only.
            # No completed ledger entry is written while descendants survive.
            cleanup_failed = False
            try:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    drain(time.monotonic() + RUN_TERM_SECONDS, cleanup=True)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    drain(time.monotonic() + RUN_TERM_SECONDS, cleanup=True)
            except BaseException:
                cleanup_failed = True
                # A secondary I/O/selector failure still cannot leave the
                # direct child alive or prevent a bounded wait after KILL.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except OSError:
                    pass
                try:
                    process.wait(timeout=RUN_TERM_SECONDS)
                except BaseException:
                    pass
            try:
                if not group_alive(process.pid):
                    register_group(process.pid, finished=True)
                else:
                    cleanup_failed = True
            except BaseException:
                cleanup_failed = True
            if cleanup_failed:
                failure_note(original, 'native cleanup incomplete; original reservation/group retained')
            raise
        finally:
            original = sys.exc_info()[1]
            if cleanup_log_failed and original is not None:
                failure_note(original, 'original tool log incomplete; secondary raw storage failed during cleanup')
            try:
                if capture == 'none':
                    stderr_tail = _project_run_logs(sink, raw_sinks, starts, counts, encoding, errors)
                elif sink is not None:
                    for stream in ('stdout', 'stderr'):
                        for raw in buffers[stream]:
                            _write_run_log(sink, raw)
                    sink.flush()
            except BaseException as error:
                if not isinstance(error, UnicodeError):
                    failure_note(error, 'original tool log incomplete; projection failed')
                if original is None:
                    raise
                failure_note(original, 'secondary original-log projection or decoding failed')
            finally:
                closing_error = sys.exc_info()[1] or original
                first_close_error = None
                for resource in (selected, process.stdout, process.stderr):
                    try:
                        resource.close()
                    except BaseException as error:
                        if first_close_error is None:
                            first_close_error = error
                if first_close_error is not None:
                    if closing_error is None:
                        raise first_close_error
                    failure_note(closing_error, 'secondary selector/pipe close failed')
        if capture == 'none':
            require(process.returncode == 0, 'actual tool failed (' + str(args[0]) + '): ' + stderr_tail)
            return None
        # Decode only after EOF, so split multibyte sequences remain complete.

        # Preserve the original text=True universal-newline stdout contract.
        def text_output(stream):
            return b''.join(buffers[stream]).decode(encoding, errors).replace('\r\n', '\n').replace('\r', '\n')
        stdout, stderr = text_output('stdout'), text_output('stderr')
        require(process.returncode == 0, 'actual tool failed (' + str(args[0]) + '): ' + stderr[-RUN_ERROR_CHARS:])
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
            # Own cleanup immediately: identity/flock failures still close the
            # new descriptor. Only acquired locks belong in native pass_fds.
            handles.append(fd)
            info = os.fstat(fd)
            require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1,
                    'producer lock identity differs')
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            ACTIVE_FDS.append(fd)
        yield work, cache
    finally:
        for fd in reversed(handles):
            if fd in ACTIVE_FDS:
                ACTIVE_FDS.remove(fd)
            os.close(fd)


@contextlib.contextmanager
def lease(work, cache, value, *, reserve_bytes=None):
    with reservation(work, value, ACTIVE_FDS, reserve_bytes=reserve_bytes):
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


def generator_buildinfo(raw, name, value):
    """Validate the actual installed command, not a module-name substring."""
    lines = raw.splitlines()
    require(lines and re.fullmatch(r'.+: go' + re.escape(value['goVersion']), lines[0]),
            'actual mobile tool Go version differs: ' + name)
    rows = [line.split() for line in lines[1:] if line.strip()]
    paths = [row[1:] for row in rows if row[0] == 'path']
    modules = [row[1:] for row in rows if row[0] == 'mod']
    require(paths == [['github.com/sagernet/gomobile/cmd/' + name]] and
            len(modules) == 1 and len(modules[0]) == 3 and
            modules[0][0] == 'github.com/sagernet/gomobile' and
            modules[0][1] == value[name + 'Version'] and
            re.fullmatch(r'h1:[A-Za-z0-9+/]{43}=', modules[0][2]) and
            not any(row[0] == '=>' for row in rows),
            'actual mobile tool module differs: ' + name)


def tool_facts(args, value, env):
    paths = {name: str(Path(getattr(args, name)).resolve(strict=True)) for name in ('go', 'gomobile', 'gobind')}
    original_tools = {name: file_pin(path) for name, path in paths.items()}
    require(run([paths['go'], 'version'], env=env).strip().startswith('go version go' + value['goVersion'] + ' '),
            'actual Go tool version differs')
    observed = {}
    for name in ('gomobile', 'gobind'):
        raw = run([paths['go'], 'version', '-m', paths[name]], env=env)
        generator_buildinfo(raw, name, value)
        observed[name] = raw
    result = {'executables': original_tools, 'mobileBuildInfo': observed}
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
    require({name: file_pin(paths[name]) for name in original_tools} == original_tools,
            'mobile tool bytes changed during observation')
    return paths, result


PRODUCER_FILES = ('scripts/mobile-core/source-contract.json', 'scripts/mobile-core/fork_input.py', 'scripts/mobile-core/source_policy.py',
             'scripts/mobile-core/plan.py', 'scripts/mobile-core/android_sdk.py',
             'scripts/mobile-core/artifacts.py', 'scripts/mobile-core/producer.py',
             'scripts/mobile-core/budget.py', 'scripts/mobile-core/ci_android.py',
             'scripts/mobile-core/consume.py', 'scripts/mobile-core/carriers.py',
             'scripts/mobile-core/apple_rust.py', 'scripts/mobile-core/apple_tls_fingerprint.py', 'scripts/mobile-core/ci_apple.py',
             'scripts/mobile-core/apple_admission.py', 'scripts/mobile-core/apple_handoff.py',
             'scripts/mobile-core/apple_merge_reuse.py',
             'scripts/mobile-core/apple-cache.py',
             'scripts/mobile-core/build_apple_app.py', 'scripts/mobile-core/native_inputs.py',
             'scripts/mobile-core/transaction.py',
             'scripts/libbox-patches/prepare-ndk-linux.py', 'scripts/libbox-patches/ndk-linux-archive.json',
             'scripts/libbox-patches/android-source.py', 'scripts/apple-carrier.py')
ANDROID_BUILD_CODE_POLICY = 'polaris-android-build-code-v2'
ANDROID_UNRELATED_CODE = frozenset({
    'scripts/mobile-core/apple-cache.py', 'scripts/mobile-core/apple_merge_reuse.py',
    'scripts/apple-carrier.py', 'scripts/mobile-core/apple_rust.py', 'scripts/mobile-core/apple_tls_fingerprint.py',
    'scripts/mobile-core/ci_apple.py', 'scripts/mobile-core/build_apple_app.py',
    'scripts/mobile-core/apple_admission.py', 'scripts/mobile-core/apple_handoff.py',
})


def producer_code():
    require(not run(['git', 'status', '--porcelain=v1', '--untracked-files=all'], cwd=ROOT), 'App author source must be frozen')
    return {'candidate': run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip(),
            'files': {name: file_pin(ROOT / name) for name in PRODUCER_FILES}}


def android_build_code(identity, policy=ANDROID_BUILD_CODE_POLICY):
    """One reviewed projection for cache lookup and reuse, never provenance."""
    require(policy == ANDROID_BUILD_CODE_POLICY, 'unknown Android build code policy')
    files = identity.get('files') if isinstance(identity, dict) else None
    require(isinstance(files, dict) and set(PRODUCER_FILES) <= set(files),
            'complete original producer code inventory required')
    for name, pin in files.items():
        require(isinstance(name, str) and isinstance(pin, dict) and set(pin) == {'bytes', 'sha256'} and
                type(pin['bytes']) is int and pin['bytes'] > 0 and isinstance(pin['sha256'], str) and
                re.fullmatch('[0-9a-f]{64}', pin['sha256']), 'producer code file pin differs')
    # Unknown extra files stay relevant; only these four Apple-only entries are
    # omitted. The original nineteen-file identity is not edited or replaced.
    return {'policy': policy, 'files': {name: pin for name, pin in files.items()
                                      if name not in ANDROID_UNRELATED_CODE}}


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
    if value.get('platform') == 'android':
        projected = android_build_code(value['producer'])
        result['producer'] = {**result['producer'], 'files': projected['files'],
                              'comparisonPolicy': projected['policy']}
    return result


APPLE_COMPONENT_CACHE_POLICY = 'polaris-apple-complete-component-v1'


def apple_cache_binding(binding):
    """Comparison only: retain original graph/BuildID, normalize owned locations.

    The source/module bytes are independently admitted before this projection.
    Only Go's Dir/GoMod locations under the fixed source/module roots and the
    generator executable filename printed by `go version -m` are observational.
    Unknown fields, graph contents, SDK paths, executable bytes and producer
    files remain exact. No receipt is changed by this function.
    """
    require(binding.get('platform') == 'apple', 'Apple component cache binding required')
    result = json.loads(json.dumps(binding))
    graph = result['moduleGraph']
    mains = [row for row in graph if row.get('Main') is True]
    require(len(mains) == 1, 'one Apple source graph root required')
    source = Path(mains[0]['Dir'])
    require(source.is_absolute() and source.parts[-2:] == ('component', 'build-view'),
            'Apple graph source must use its fixed component slot')
    root = source.parent.parent
    prefixes = ((source, '$SOURCE'), (root / 'cache/go-mod', '$MODULES'))
    def normalize(row):
        require(isinstance(row, dict), 'module graph row required')
        for name in ('Dir', 'GoMod'):
            if name not in row:
                continue
            path = Path(row[name])
            require(path.is_absolute() and '..' not in path.parts, 'module location must be canonical')
            matches = [(base, label) for base, label in prefixes if path.is_relative_to(base)]
            require(len(matches) == 1, 'module location escaped admitted source/cache roots')
            base, label = matches[0]
            row[name] = label + '/' + path.relative_to(base).as_posix()
        if 'Replace' in row:
            normalize(row['Replace'])
    for row in graph:
        normalize(row)
    for name in ('gomobile', 'gobind'):
        raw = result['tools']['mobileBuildInfo'][name]
        prefix = str(root / 'tools' / name) + ': '
        require(isinstance(raw, str) and raw.startswith(prefix), 'generator observation path differs')
        result['tools']['mobileBuildInfo'][name] = '$TOOLS/' + name + ': ' + raw[len(prefix):]
    return comparison_binding(result)


def cacheable_native_environment(binding):
    """External include/library/config paths have no content closure here.

    Preserve custom execution inputs in the fresh producer receipt, but never
    reuse/export a component using them merely because their path strings match.
    Unrecognized pinned-Go defaults conservatively choose the fresh build.
    """
    native = binding.get('nativeEnvironment', {})
    if not isinstance(native, dict) or native.get('GODEBUG') != 'gocacheverify=1':
        return False
    effective, ambient = native.get('go'), native.get('ambient')
    if not isinstance(effective, dict) or not isinstance(ambient, dict) or any(ambient.values()):
        return False
    defaults = {
        'CGO_CFLAGS': ('-O2 -g',), 'CGO_CPPFLAGS': ('',), 'CGO_CXXFLAGS': ('-O2 -g',),
        'CGO_FFLAGS': ('-O2 -g',), 'CGO_LDFLAGS': ('-O2 -g',), 'CC': ('clang',),
        'CXX': ('clang++',), 'AR': ('ar',), 'PKG_CONFIG': ('pkg-config',), 'CGO_ENABLED': ('1',),
        'GOEXPERIMENT': ('',), 'GOAMD64': ('', 'v1'), 'GOARM': ('', '7', '7,hardfloat'),
        'GOARM64': ('', 'v8.0'), 'GO386': ('', 'sse2'), 'GOMIPS': ('', 'hardfloat'),
        'GOMIPS64': ('', 'hardfloat'), 'GOPPC64': ('', 'power8'), 'GORISCV64': ('', 'rva20u64'),
        'GO_EXTLINK_ENABLED': ('',), 'GOFIPS140': ('off',), 'GOFLAGS': ('-mod=readonly',),
        'GOTOOLCHAIN': ('local',),
    }
    return set(effective) == set(defaults) and all(effective[name] in values for name, values in defaults.items())


def equivalent_apple_cache_binding(original, current):
    return equivalent_binding(apple_cache_binding(original), apple_cache_binding(current))


def component_revalidation_observation(receipt, binding, receipt_pin, reused):
    """Current verifier identity alongside immutable original producer facts."""
    policy = (ANDROID_BUILD_CODE_POLICY if binding['platform'] == 'android'
              else 'polaris-full-producer-code-v1')
    result = {'schema': 'polaris-mobile-component-revalidation-v1', 'derivedObservation': True,
              'comparisonPolicy': policy, 'reused': reused,
              'currentVerifier': binding['producer'],
              'originalProducerCandidate': receipt['input']['producer']['candidate'],
              'originalReceipt': receipt_pin, 'originalComponentFingerprint': receipt['fingerprint'],
              'originalBuildID': 'polaris-mobile-' + receipt_digest(receipt['input']),
              'currentBuildInputFingerprint': receipt_digest(comparison_binding(binding)),
              'scope': 'current byte/origin revalidation; original receipt/BuildID remain unchanged; no App/device acceptance'}
    result['fingerprint'] = receipt_digest(result)
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


def apple_native_environment(go, env, source=None):
    # Read effective Go configuration (including GOENV), not just process
    # overrides. Keep all substantive values exact across cached producers.
    keys = ('CGO_CFLAGS CGO_CPPFLAGS CGO_CXXFLAGS CGO_FFLAGS CGO_LDFLAGS '
            'CC CXX AR PKG_CONFIG CGO_ENABLED GOEXPERIMENT GOAMD64 GOARM GOARM64 '
            'GO386 GOMIPS GOMIPS64 GOPPC64 GORISCV64 GO_EXTLINK_ENABLED GOFIPS140 '
            'GOFLAGS GOTOOLCHAIN').split()
    effective = json.loads(run([go, 'env', '-json', *keys], cwd=source, env=env))
    require(isinstance(effective, dict) and set(effective) == set(keys) and
            all(isinstance(v, str) for v in effective.values()), 'effective Go compilation inputs differ')
    ambient = ('CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH OBJC_INCLUDE_PATH LIBRARY_PATH '
               'COMPILER_PATH CCC_OVERRIDE_OPTIONS CLANG_CONFIG_FILE_SYSTEM_DIR '
               'CLANG_CONFIG_FILE_USER_DIR GCC_EXEC_PREFIX PKG_CONFIG_PATH PKG_CONFIG_LIBDIR '
               'PKG_CONFIG_SYSROOT_DIR PKG_CONFIG_TOP_BUILD_DIR PKG_CONFIG_ALLOW_SYSTEM_CFLAGS '
               'PKG_CONFIG_ALLOW_SYSTEM_LIBS PKG_CONFIG_SYSTEM_INCLUDE_PATH PKG_CONFIG_SYSTEM_LIBRARY_PATH '
               'PKG_CONFIG_DISABLE_UNINSTALLED').split()
    ambient += ['CGO_' + kind + '_' + mode for kind in ('CFLAGS', 'CPPFLAGS', 'CXXFLAGS', 'FFLAGS', 'LDFLAGS')
                for mode in ('ALLOW', 'DISALLOW')]
    return {'go': effective, 'ambient': {k: env.get(k) for k in ambient},
            'GODEBUG': 'gocacheverify=1'}


def admitted_binding(args, value, work, cache, producer_identity, initial_source):
    """Run the existing source/tool/module admission without invoking a compiler."""
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
    # Observe the runner without requiring a compiler's memory budget.
    # A cache hit still runs the complete byte/native/origin predicate under
    # its existing filesystem reservation and bounded metadata collectors.
    print(json.dumps({'phase': 'component-validation-resources', 'platform': args.platform,
                      'capacity': native_capacity(), 'compilerAdmissionRequired': False,
                      'budget': {'filesystemPeakReserveBytes': value['budget']['buildPeakReserveBytes'],
                                 'cacheMaxBytes': value['budget']['cacheMaxBytes']}}), flush=True)
    tools, identities = tool_facts(args, value, env)
    # init installs gobind@latest. The pinned generator only requires this
    # private GOPATH marker directory; create it without invoking init.
    private_dir(Path(env['GOPATH']) / 'pkg/gomobile')
    run([tools['go'], 'mod', 'verify'], cwd=source, env=env, log=work / 'original-module-verify.log', capture='none')
    graph = module_stream(run([tools['go'], 'list', '-m', '-json', 'all'], cwd=source, env=env))
    admit_module_cache(source, graph, tools['go'], env, work)
    require(verify_source(source, value, generator_outputs=True) == source_observation,
            'source changed during module cache admission')
    graph = module_stream(run([tools['go'], 'list', '-m', '-json', 'all'], cwd=source, env=env))
    run([tools['go'], 'mod', 'verify'], cwd=source, env=env, log=work / 'original-module-verify.log', capture='none')
    # Keep exact paths for native BuildInfo; portable source facts stay separate.
    binding = {'schema': 'polaris-mobile-producer-input-v1', 'platform': args.platform,
               'sourceObservation': source_observation, 'sourceContract': value, 'moduleGraph': graph,
               'tools': identities, 'producer': producer_identity, 'sourceOverlay': 0}
    if args.platform == 'apple':
        binding['nativeEnvironment'] = apple_native_environment(tools['go'], env, source)
    require(producer_code() == producer_identity, 'producer source changed during tool admission')
    return source, scratch, env, tools, identities, binding, jar


def execute(args):
    value = load()
    producer_identity = producer_code()  # Before any tool/cache/work mutation.
    initial_source = verify_source(args.source, value)
    with lease(args.work, args.cache, value) as (work, cache):
        source, scratch, env, tools, identities, binding, jar = admitted_binding(
            args, value, work, cache, producer_identity, initial_source)
        graph, source_observation = binding['moduleGraph'], binding['sourceObservation']
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
            # Sample and flush before validating explicit child concurrency.
            # Dynamic caps guide compilation, outside immutable binding/BuildID.
            observation = native_capacity()
            print(json.dumps({'phase': 'component-native-capacity-sample', 'platform': args.platform,
                              'capacity': observation}), flush=True)
            env, capacity = native_environment(env, observation)
            print(json.dumps({'phase': 'component-native-capacity', 'platform': args.platform,
                              'capacity': capacity, 'compilerCapacityApplied': True}), flush=True)
            recipe = plan(args.platform, value, gomobile=tools['gomobile'], output=staged, bootclasspath=jar)
            from fork_input import git_environment, require_git_root, bind_command
            command = bind_command(args.platform, value, tools['gomobile'], staged, fp, jar)
            # One direct bind; no helper API21/macOS/tvOS/git-describe side effects.
            log = work / 'stage-original-bind.log'
            require(not log.exists(), 'original bind log retained; archive its identity before another bind')
            # Actual compiler objects must remain observable on a fresh producer
            # even if the Go compiler cache already contains the target.
            env['GODEBUG'] = 'gocacheverify=1'
            run(command, cwd=source, env=env, log=log, capture='none')
            require(verify_source(source, value, generator_outputs=True) == source_observation, 'source changed during bind')
            require(tool_facts(args, value, env)[1] == identities and producer_code() == binding['producer'], 'producer/tool input changed')
            if args.platform == 'apple':
                require(apple_native_environment(tools['go'], env, source) == binding['nativeEnvironment'],
                        'effective Go/native inputs changed during bind')
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
        require(producer_code() == producer_identity, 'verifier source changed during component validation')
        print(json.dumps({'component': str(component), 'receipt': str(receipt_path),
                          'fingerprint': receipt['fingerprint'], 'AppAccepted': False, 'devicePassed': False,
                          'revalidation': component_revalidation_observation(
                              receipt, binding, file_pin(receipt_path), not rebuild)}))


def main():
    global BORROWED_PROGRESS_FD, BORROWED_PROGRESS_PHASE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('platform', choices=['android', 'apple'])
    for name in ('source', 'go', 'gomobile', 'gobind', 'work', 'cache'):
        parser.add_argument('--' + name, required=True, type=Path)
    for name in ('sdk', 'ndk', 'jdk', 'developer'):
        parser.add_argument('--' + name, type=Path)
    parser.add_argument('--progress-fd', type=int, help='Explicit parent descriptor for fixed Apple native observations')
    args = parser.parse_args()
    if args.progress_fd is not None:
        require(args.platform == 'apple' and args.progress_fd > 2, 'dedicated Apple progress descriptor required')
        require(str(args.progress_fd) != os.environ.get('POLARIS_BUILD_RESERVATION_FD'),
                'progress descriptor cannot own a native reservation')
        os.fstat(args.progress_fd)
        flags = fcntl.fcntl(args.progress_fd, fcntl.F_GETFD)
        fcntl.fcntl(args.progress_fd, fcntl.F_SETFD, flags | fcntl.FD_CLOEXEC)
        BORROWED_PROGRESS_FD = args.progress_fd
        BORROWED_PROGRESS_PHASE = 'component-build-or-revalidate'
    require((args.sdk is not None and args.ndk is not None and args.jdk is not None and args.developer is None)
            if args.platform == 'android' else
            (args.developer is not None and args.sdk is None and args.ndk is None and args.jdk is None), 'platform tools differ')
    execute(args)


if __name__ == '__main__':
    main()
