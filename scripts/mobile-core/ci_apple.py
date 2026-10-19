#!/usr/bin/env python3
"""One hosted unsigned Apple SDK/App/PacketTunnel chain; no publish/device action."""
import argparse
import contextlib
import json
import hashlib
import os
from pathlib import Path
import shutil
import stat
import sys
import threading
import time
import zipfile

sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from artifacts import MAX_ARTIFACT, MAX_RECEIPT, file_pin, read_receipt, receipt_chunks
from ci_android import prepare_source
from consume import materialize_apple_resources
from fork_input import ROOT, canonical, decode, load, require, sha
from producer import lease, slot_locks, private_dir, producer_code, run, write_json

from apple_handoff import required_slots, scope_fields

CARRIERS = [('device-arm64', None), ('sim-arm64', 'arm64')]

# Existing selected-original and individual-artifact limits remain unchanged.
# Reserve new disk writes only: one bounded ZIP, one linked staging snapshot,
# and all index/sidecar/temp metadata. The shared ledger adds its normal 1 GiB
# filesystem headroom and still excludes other reservations/native descendants.
EVIDENCE_CONTENT_BYTES = 4 * 1024 ** 3
EVIDENCE_ENTRIES = 16384
EVIDENCE_PATH_BYTES = 4096
EVIDENCE_INDEX_BYTES = MAX_RECEIPT
EVIDENCE_METADATA_BYTES = 8 * MAX_RECEIPT
# Conservative deflate expansion, both ZIP names/headers, ZIP64 and end records.
# The writer also enforces this hard limit before each physical write.
EVIDENCE_ZIP_BYTES = ((EVIDENCE_CONTENT_BYTES + EVIDENCE_INDEX_BYTES) * 9 // 8 +
                      (EVIDENCE_ENTRIES + 1) * (2 * EVIDENCE_PATH_BYTES + 1024) + 1024)
EVIDENCE_RESERVE_BYTES = EVIDENCE_ZIP_BYTES + MAX_ARTIFACT + EVIDENCE_METADATA_BYTES


class BoundedEvidenceZip:
    """Keep even partial/failed ZIP writes inside the admitted disk envelope."""
    def __init__(self, stream):
        self.stream = stream

    def write(self, raw):
        require(self.stream.tell() + len(raw) <= EVIDENCE_ZIP_BYTES,
                'original evidence ZIP exceeds reserved byte budget')
        return self.stream.write(raw)

    def __getattr__(self, name):
        return getattr(self.stream, name)


def copy_pinned_evidence(path, output, pin):
    """Copy only the pinned regular bytes; growth cannot consume extra reserve."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        before = os.fstat(source.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_size == pin['bytes'] and
                0 <= before.st_size <= MAX_ARTIFACT, 'bounded regular original evidence required')
        digest, count = hashlib.sha256(), 0
        while True:
            raw = source.read(min(1024 ** 2, pin['bytes'] - count + 1))
            if not raw:
                break
            count += len(raw)
            require(count <= pin['bytes'], 'original evidence grew during preservation')
            output.write(raw)
            digest.update(raw)
        after = os.fstat(source.fileno())
    require({'bytes': count, 'sha256': digest.hexdigest()} == pin and
            (before.st_size, before.st_mtime_ns) == (after.st_size, after.st_mtime_ns),
            'original evidence changed during preservation')


@contextlib.contextmanager
def phase_timing(name):
    """Original elapsed observation only; never a carrier acceptance receipt."""
    started = time.monotonic()
    completed = False
    stopped = threading.Event()

    def report(event):
        print('APPLE_PHASE_PROGRESS=' + json.dumps({'phase': name, 'event': event,
              'elapsedSeconds': round(time.monotonic() - started, 3),
              'scope': 'parent wait observation only; not compiler liveness or carrier acceptance'},
              sort_keys=True), flush=True)

    def heartbeat():
        while not stopped.wait(30):
            report('waiting')

    report('start')
    monitor = threading.Thread(target=heartbeat, daemon=True)
    monitor.start()
    try:
        yield
        completed = True
    finally:
        stopped.set()
        monitor.join(timeout=1)
        report('end')
        print('APPLE_PHASE_TIMING=' + json.dumps({'phase': name,
              'elapsedSeconds': round(time.monotonic() - started, 3),
              'completed': completed, 'scope': 'elapsed observation only'},
              sort_keys=True), flush=True)


def location():
    require(os.environ.get('GITHUB_ACTIONS') == 'true' and os.uname().sysname == 'Darwin',
            'hosted Mac entry only; explicit local builder has separate admission')
    base = Path.home() / 'Code/polaris-mobile-apple-ci'
    run_id, attempt = os.environ.get('GITHUB_RUN_ID', ''), os.environ.get('GITHUB_RUN_ATTEMPT', '')
    require(run_id.isdigit() and attempt.isdigit(), 'hosted run/attempt required')
    return private_dir(base / (run_id + '-' + attempt))


def selected_apple_tools(value):
    """Exact source-owned selection, shared by producer and every carrier.

    A rolling hosted label is not a tool identity. Reject an older image before
    tool installation or component transport; never select another beta/RC.
    The later import still compares actual compiler/SDK bytes to the producer.
    """
    developer = Path('/Applications/Xcode_27.1.app/Contents/Developer').resolve(strict=True)
    env = dict(os.environ, DEVELOPER_DIR=str(developer))
    xcode = run(['xcodebuild', '-version'], env=env).strip()
    require(xcode == 'Xcode ' + value['apple']['xcodeVersion'] + '\nBuild version ' + value['apple']['xcodeBuild'],
            'runner does not provide the exact selected Xcode; no fallback/skip')
    for sdk, key in [('iphoneos', 'iPhoneSDK'), ('iphonesimulator', 'simulatorSDK')]:
        require(run(['xcrun', '--sdk', sdk, '--show-sdk-version'], env=env).strip() == value['apple'][key],
                'runner does not provide the exact selected SDK')
    return developer, env, xcode


def admit():
    value = load()
    expected = os.environ['POLARIS_APP_CANDIDATE']
    require(run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip() == expected, 'hosted App candidate differs')
    developer, _, xcode = selected_apple_tools(value)
    facts = {'schema': 'polaris-apple-runner-tool-admission-v1',
             **scope_fields(), 'candidate': expected, 'xcode': xcode, 'developer': str(developer),
             'iPhoneSDK': value['apple']['iPhoneSDK'], 'simulatorSDK': value['apple']['simulatorSDK'],
             'compilerByteIdentityChecked': False, 'carrierAccepted': False, 'devicePassed': False,
             'scope': 'exact version/build/SDK admission only; original compiler byte verification still mandatory'}
    write_json(location() / 'runner-tool-admission.json', facts)
    print('APPLE_RUNNER_TOOL_ADMISSION=' + json.dumps(facts, sort_keys=True), flush=True)


def prepare():
    value = load()
    identity = producer_code()
    root = location()
    expected = os.environ['POLARIS_APP_CANDIDATE']
    require(run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip() == expected, 'hosted App candidate differs')
    developer, env, xcode = selected_apple_tools(value)
    from apple_admission import preflight
    preflight(root, env, expected_slots=required_slots())
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
        log=root / 'original-generator-prepare.log', capture='none', observation_fd=sys.stdout.fileno(),
        observation_phase='tool-prepare')
    paths = {'go': str(go), 'gomobile': str(tools / 'gomobile'), 'gobind': str(tools / 'gobind'),
             'developer': str(developer), 'source': str(root / 'fork-source'),
             'component-work': str(private_dir(root / 'component')), 'cache': str(private_dir(root / 'cache')),
             'rust-target': str(private_dir(root / 'cache/rust-target'))}
    prepare_source(paths['source'], value)
    require(producer_code() == identity, 'App producer source changed during preparation')
    facts = {**scope_fields(), 'App': identity, 'paths': paths, 'xcode': xcode,
             'tools': {name: file_pin(paths[name]) for name in ('go', 'gomobile', 'gobind')},
             'rustc': run(['rustc', '-vV']), 'devicePassed': False}
    facts['fingerprint'] = sha(canonical(facts))
    write_json(root / 'tools.json', facts)
    with Path(os.environ['GITHUB_ENV']).open('a') as stream:
        stream.write('POLARIS_FORMAL_APPLE_ROOT=' + str(root) + '\nDEVELOPER_DIR=' + str(developer) + '\n')
    print('Exact Apple source/tool slots prepared; no compiler, signing or device action')


def build():
    root = location()
    tools = read_receipt(root / 'tools.json')
    require(all(tools.get(k) == v for k, v in scope_fields().items()),
            'prepared Apple carrier scope differs before build')
    paths = tools['paths']
    # The existing Tauri beforeBuild commands belong before source/link freeze.
    # Their outputs are ignored generated UI/resources, not mutations of PBX/Cargo.
    with phase_timing('frontend-resources'):
        with lease(private_dir(root / 'frontend-work'), Path(paths['cache']), load()):
            run(['node', ROOT / 'scripts/fetch-dashboard.mjs'], cwd=ROOT, log=root / 'original-dashboard.log', capture='none',
                observation_fd=sys.stdout.fileno(), observation_phase='frontend-dashboard')
            run(['pnpm', '--dir', ROOT / 'ui', 'build'], cwd=ROOT, log=root / 'original-frontend.log', capture='none',
                observation_fd=sys.stdout.fileno(), observation_phase='frontend-build')
            materialize_apple_resources()
    command = [sys.executable, ROOT / 'scripts/mobile-core/producer.py', 'apple']
    for name in ('source', 'go', 'gomobile', 'gobind', 'developer'):
        command += ['--' + name, paths[name]]
    command += ['--work', paths['component-work'], '--cache', paths['cache']]
    with phase_timing('shared-component'):
        progress_fd = os.dup(sys.stdout.fileno())
        try:
            command += ['--progress-fd', str(progress_fd)]
            run(command, cwd=ROOT, log=root / 'original-component-producer.log', capture='none',
                progress_fd=progress_fd, observation_phase='component-build-or-revalidate')
        finally:
            os.close(progress_fd)
    from apple_handoff import freeze
    with phase_timing('component-handoff-freeze'):
        freeze(root)


def import_carrier(folder):
    root = location()
    # Also enforce selection for direct invocation. Import's original-byte
    # comparison remains authoritative, rather than trusting this admission.
    selected_apple_tools(load())
    from apple_handoff import import_bundle
    with phase_timing('component-handoff-import'):
        tools = import_bundle(folder, root)
    paths = tools['paths']
    private_dir(paths['rust-target'])
    env = dict(os.environ, DEVELOPER_DIR=paths['developer'])
    from apple_admission import preflight
    preflight(root, env, expected_slots=required_slots())
    require(run(['rustc', '-vV']) == tools['rustc'], 'actual consumer Rust toolchain differs')
    with Path(os.environ['GITHUB_ENV']).open('a') as stream:
        stream.write('POLARIS_FORMAL_APPLE_ROOT=' + str(root) + '\nDEVELOPER_DIR=' + paths['developer'] + '\n')


def import_local():
    """Revalidate this job's frozen producer; never extract over its originals."""
    from apple_handoff import verify_import, context
    root = location()
    selected_apple_tools(load())
    tools = read_receipt(root / 'tools.json')
    paths = tools['paths']
    with phase_timing('component-local-verification'):
        with lease(private_dir(root / 'import-work'), Path(paths['cache']), load()):
            verify_import(Path(paths['component-work']), Path(paths['cache']),
                          *(paths[n] for n in ('source', 'go', 'gomobile', 'gobind', 'developer')))
    require(run(['rustc', '-vV']) == tools['rustc'], 'actual consumer Rust toolchain differs')
    write_json(root / 'import-observation.json', {'schema': 'polaris-apple-import-observation-v1',
               'context': context(), 'transport': read_receipt(root / 'component-transport.json'),
               'completed': True, 'devicePassed': False})


def carrier(slot):
    from apple_handoff import carrier_summary, context
    require(slot in required_slots(), 'unrequested Apple carrier slot')
    root = location()
    # Local verification precedes the common Rust cache restore. Every actual
    # carrier also rechecks the original component bytes before compilation.
    require((root / 'component-handoff.json').is_file(), 'verified component import required')
    imported = read_receipt(root / 'import-observation.json')
    require(imported['completed'] is True and imported['context'] == context(),
            'same-run complete component verification required')
    ensure_previous_staging_preserved(root, slot)
    paths = read_receipt(root / 'tools.json')['paths']
    # Work/derived-data/target triples are slot-specific. Checkout Externals
    # staging is shared, so the hosted workflow executes these calls in order.
    command = [sys.executable, ROOT / 'scripts/mobile-core/build_apple_app.py']
    for name, path in paths.items():
        command += ['--' + name, path]
    command += ['--work', private_dir(root / slot), '--slot', slot, '--imported-component']
    simulator = dict(CARRIERS)[slot]
    if simulator:
        command += ['--simulator', '--simulator-arch', simulator]
    marker = root / ('staging-preservation-' + slot + '.json')
    require(not marker.exists(), 'carrier staging scope already started')
    write_json(marker, {'slot': slot, 'context': context(), 'complete': False})
    with phase_timing(slot):
        progress_fd = os.dup(sys.stdout.fileno())
        try:
            command += ['--progress-fd', str(progress_fd)]
            run(command, cwd=ROOT, log=root / ('original-' + slot + '.log'), capture='none',
                progress_fd=progress_fd, observation_phase='carrier-command', observation_slot=slot)
        finally:
            os.close(progress_fd)
    carrier_summary(root, slot)


def ensure_previous_staging_preserved(root, slot):
    from apple_handoff import context, transport_pin
    require(slot in dict(CARRIERS), 'unknown Apple staging slot')
    for previous, _ in CARRIERS:
        if previous == slot:
            break
        # Both formal platforms share Externals/arm64 staging.
        marker = root / ('staging-preservation-' + previous + '.json')
        if marker.exists():
            facts = read_receipt(marker)
            require(facts['slot'] == previous and facts['context'] == context() and
                    facts['complete'] is True and
                    facts['archive'] == transport_pin(root / ('apple-original-' + previous + '-evidence.zip')),
                    'previous shared staging not safely preserved; replacement blocked')


def preserve_linked_archive(root, slot, receipt_path=None):
    """Freeze writable Externals staging before another arm64 platform uses it."""
    from apple_rust import target_for
    require(slot in dict(CARRIERS), 'unknown Apple staging slot')
    target = target_for('iphoneos' if slot == 'device-arm64' else 'iphonesimulator',
                        'arm64', 'release')
    original = ROOT / 'src-tauri/gen/apple/Externals/arm64/release/libapp.a'
    require(original.resolve(strict=False) == original, 'linked archive path cannot be redirected')
    work = private_dir(root / slot)
    if not original.exists():
        write_json(work / 'linked-rust-preservation.json', {
            'slot': slot, 'originalPath': str(original), 'originalAbsent': True,
            'preservationComplete': True, 'acceptedByReceipt': False,
            'receiptClaimedArchive': receipt_path is not None,
            'scope': 'fixed safe staging path absent; no accepted archive claimed'})
        require(receipt_path is None, 'claimed original linked Rust archive missing')
        return None
    # Observe and freeze the actual fixed-path bytes first. A bad receipt must
    # not cause these unaccepted bytes to disappear when the next slot runs.
    actual = file_pin(original, allow_empty=True)
    copy = work / 'original-linked-rust-archive.a'
    with copy.open('xb') as output:
        copy_pinned_evidence(original, output, actual)
    shutil.copystat(original, copy)
    require(file_pin(original, allow_empty=True) == actual and file_pin(copy, allow_empty=True) == actual,
            'linked Rust archive changed during preservation')
    expected, accepted, reason = None, False, 'accepted Rust receipt unavailable'
    if receipt_path is not None:
        try:
            receipt = read_receipt(receipt_path)
            expected = receipt['archive']
            accepted = receipt['target'] == target and receipt['linkedArchive'] == str(original) and expected == actual
            reason = 'original linked Rust archive differs before preservation'
        except (RuntimeError, ValueError, OSError, KeyError, TypeError):
            reason = 'accepted Rust receipt unreadable'
    write_json(work / 'linked-rust-preservation.json', {
        'schema': 'polaris-apple-linked-rust-preservation-v1', 'slot': slot,
        'originalPath': str(original), 'preservedPath': str(copy),
        'originalPin': actual, 'preservedPin': file_pin(copy, allow_empty=True), 'expectedPin': expected,
        'preservationComplete': True, 'acceptedByReceipt': accepted,
        'scope': 'actual staging bytes retained, accepted or unaccepted; not a new producer receipt'})
    require(accepted, reason)
    return copy


def evidence(*, publish_env=False, slot=None):
    root = location()
    require(slot is None or slot in required_slots(), 'unrequested Apple evidence slot')
    scoped_slot = slot or os.environ.get('POLARIS_APPLE_CARRIER_SLOT')
    require(scoped_slot is None or scoped_slot in required_slots(), 'unrequested Apple evidence slot')
    output = root / ('apple-original-' + slot + '-evidence.zip' if slot else 'apple-original-evidence.zip')
    # Even preparation/import failures may precede the normal GITHUB_ENV write.
    # Publish only the fixed owned evidence path for the following upload step.
    if publish_env:
        with Path(os.environ['GITHUB_ENV']).open('a') as stream:
            stream.write('POLARIS_FORMAL_APPLE_ROOT=' + str(root) + '\n')
    cache = private_dir(root / 'cache')
    # Both failed and successful builds retain original evidence. Package only
    # selected bytes, never an entire cache, repeated checkout or target tree.
    with lease(private_dir(root / 'evidence-work'), cache, load(),
               reserve_bytes=EVIDENCE_RESERVE_BYTES):
        require(not output.exists(), 'previous original evidence must not be overwritten')
        if slot:
            from apple_handoff import context
            marker = root / ('staging-preservation-' + slot + '.json')
            write_json(marker, {'slot': slot, 'context': context(), 'complete': False})
        imported = (read_receipt(root / 'import-observation.json').get('completed') is True
                    if (root / 'import-observation.json').is_file() else False)
        paths = (set(root.glob('*.json')) | set(root.glob('*.log')) | set(root.glob('*.raw'))
                 if not scoped_slot else
                 {root / name for name in ('tools.json', 'component-handoff.json',
                  'component-transport.json', 'import-observation.json', 'runner-tool-admission.json',
                  'original-' + scoped_slot + '.log',
                  'original-' + scoped_slot + '.log.stdout.raw',
                  'original-' + scoped_slot + '.log.stderr.raw') if (root / name).is_file()})
        component = root / 'component'
        paths.update(component.glob('*.json'))
        paths.update(component.glob('*.log'))
        paths.update(component.glob('*.raw'))
        frameworks = [component / 'Libbox.xcframework', component / 'staging/Libbox.xcframework']
        for framework in (frameworks if not scoped_slot else []):
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
        if origins is not None and not scoped_slot:
            for path in origins['files']:
                original = Path(path)
                if original.exists():
                    paths.add(original)
                else:
                    receipt_errors['native-input:' + str(len(receipt_errors))] = 'original input missing in failed partial work'
            if 'generatorOutputRoot' in origins:
                paths.update(path for path in Path(origins['generatorOutputRoot']).rglob('*') if path.is_file())
        timing_paths = []
        for current_slot in required_slots():
            if scoped_slot and current_slot != scoped_slot:
                continue
            work = root / current_slot
            paths.update(work.glob('*.json'))
            paths.update(work.glob('*.log'))
            paths.update(work.glob('*.raw'))
            paths.update(work.glob('*.ipa'))
            paths.update(work.glob('wire-*.a'))
            for name in ('PolarisTLSFingerprintLinker.args', 'PolarisTLSFingerprintLinker.json'):
                prepared_tls = work / 'tls-link-input' / name
                if prepared_tls.is_file():
                    paths.add(prepared_tls)
            paths.update((work / 'derived-data').rglob('*LinkMap*.txt'))
            # A failed symbol check has no accepted Rust receipt yet. Preserve
            # its actual archive as unaccepted evidence under the same budget.
            from apple_rust import target_for
            target = target_for('iphoneos' if current_slot == 'device-arm64' else 'iphonesimulator',
                                'arm64', 'release')
            timing_paths.append(work / ('diagnostic-cargo-timing-' + target + '.html'))
            archive = root / 'cache/rust-target' / target / 'release/libpolaris_lib.a'
            if archive.is_file():
                paths.add(archive)
            for receipt in work.glob('rust-*.json'):
                try:
                    target = read_receipt(receipt)['target']
                    require(isinstance(target, str) and target in
                            ('aarch64-apple-ios', 'aarch64-apple-ios-sim'),
                            'original Rust target differs')
                except (RuntimeError, ValueError, OSError, KeyError, TypeError) as error:
                    receipt_errors[receipt.relative_to(root).as_posix()] = str(error)
                    continue
                native = root / 'cache/rust-target' / target / 'release/libpolaris_lib.a'
                if native.is_file():
                    paths.add(native)
                else:
                    receipt_errors[receipt.relative_to(root).as_posix() + ':archive'] = 'original Rust archive missing'
        staging_safe = False
        if slot:
            work = root / slot
            from apple_rust import target_for
            target = target_for('iphoneos' if slot == 'device-arm64' else 'iphonesimulator',
                                'arm64', 'release')
            accepted = work / ('rust-' + target + '.json')
            try:
                preserved = preserve_linked_archive(root, slot, accepted if accepted.is_file() else None)
                if preserved is not None:
                    paths.add(preserved)
            except (RuntimeError, ValueError, OSError, KeyError, TypeError) as error:
                receipt_errors[slot + ':linked-preservation'] = str(error)
            for name in ('original-linked-rust-archive.a', 'linked-rust-preservation.json'):
                if (work / name).is_file():
                    paths.add(work / name)
            observation = work / 'linked-rust-preservation.json'
            if observation.is_file():
                observed = read_receipt(observation)
                staging_safe = observed.get('preservationComplete') is True
        require(len(paths) <= EVIDENCE_ENTRIES, 'selected original evidence entry budget exceeded')
        rows, total = {}, 0
        with output.open('xb') as raw_output, \
                zipfile.ZipFile(BoundedEvidenceZip(raw_output), 'w', compression=zipfile.ZIP_DEFLATED) as package:
            for path in sorted(paths):
                require(path.resolve(strict=True).is_relative_to(root),
                        'original Apple evidence escaped its fixed slots')
                name = path.relative_to(root).as_posix()
                require(len(name.encode('utf-8')) <= EVIDENCE_PATH_BYTES,
                        'original evidence ZIP path budget exceeded')
                if path.is_symlink():
                    require(any(path.is_relative_to(framework) for framework in frameworks),
                            'only actual component/staged framework links may be retained')
                    link = os.readlink(path)
                    raw_link = link.encode('utf-8')
                    total += len(raw_link)
                    require(total <= EVIDENCE_CONTENT_BYTES, 'selected original evidence exceeds 4GiB budget')
                    info = zipfile.ZipInfo(name)
                    info.create_system = 3
                    # Preserve the link itself: macOS framework links may be 0755.
                    # Synthesizing 0777 disagrees with the exact transport inventory.
                    info.external_attr = (stat.S_IFLNK | stat.S_IMODE(path.lstat().st_mode)) << 16
                    package.writestr(info, raw_link)
                    rows[name] = {'originalPath': str(path), 'link': link}
                    continue
                pin = file_pin(path, allow_empty=True)
                total += pin['bytes']
                require(total <= EVIDENCE_CONTENT_BYTES, 'selected original evidence exceeds 4GiB budget')
                info = zipfile.ZipInfo.from_file(path, arcname=name)
                info.compress_type = zipfile.ZIP_DEFLATED
                with package.open(info, 'w', force_zip64=True) as member:
                    copy_pinned_evidence(path, member, pin)
                rows[name] = {'originalPath': str(path), **pin}
            index = {'schema': 'polaris-apple-original-evidence-v2', **scope_fields(), 'files': rows,
                     'completeUnsignedChain': (root / 'apple-chain.json').exists() and not receipt_errors,
                     'componentAccepted': origins is not None and not receipt_errors and
                                          (not scoped_slot or imported),
                     'componentOriginalsInThisArchive': not bool(scoped_slot),
                     'componentOriginalsReferencedByTransport': bool(scoped_slot),
                     'slot': scoped_slot,
                     'sameRunComponentArtifact': 'apple-component-original-' + os.environ.get('GITHUB_RUN_ID', '') +
                                                 '-' + os.environ.get('GITHUB_RUN_ATTEMPT', ''),
                     'receiptReadErrors': receipt_errors,
                     'receiptBytes': {name: row['bytes'] for name, row in rows.items()
                                      if name.endswith('.json') and 'bytes' in row},
                     'stagedComponentIsAccepted': False,
                     'devicePassed': False, 'publicationEligible': False}
            # Use the same bounded receipt serializer as sidecars. Retirement
            # reads at most this index bound; never emit an unretirable index.
            require(sum(len(raw) for raw in receipt_chunks(index, pretty=True)) <= EVIDENCE_INDEX_BYTES,
                    'original evidence index exceeds bound')
            # Mandatory originals and their index have already passed unchanged.
            # Optional diagnostics use only remaining budgets; never enter paths
            # or receipt_errors, and never read again during ZIP serialization.
            from apple_rust import CARGO_TIMING_BYTES
            for timing in timing_paths:
                try:
                    name = timing.relative_to(root).as_posix()
                    require(len(rows) < EVIDENCE_ENTRIES and len(name.encode('utf-8')) <= EVIDENCE_PATH_BYTES and
                            timing.resolve(strict=True) == timing, 'no safe diagnostic entry budget')
                    fd = os.open(timing, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                    with os.fdopen(fd, 'rb') as stream:
                        before = os.fstat(stream.fileno())
                        require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= CARGO_TIMING_BYTES,
                                'bounded diagnostic required')
                        raw = stream.read(CARGO_TIMING_BYTES + 1)
                        after = os.fstat(stream.fileno())
                    require(len(raw) == before.st_size and
                            (before.st_size, before.st_mtime_ns) == (after.st_size, after.st_mtime_ns) and
                            total + len(raw) <= EVIDENCE_CONTENT_BYTES, 'no stable diagnostic content budget')
                    proposed = {**index, 'files': {**rows, name: {'originalPath': str(timing),
                                'bytes': len(raw), 'sha256': sha(raw), 'diagnosticOnly': True}}}
                    index_bytes = sum(len(chunk) for chunk in receipt_chunks(proposed, pretty=True))
                    require(index_bytes <= EVIDENCE_INDEX_BYTES and
                            (total + len(raw) + index_bytes) * 9 // 8 +
                            (len(rows) + 2) * (2 * EVIDENCE_PATH_BYTES + 1024) + 1024 <= EVIDENCE_ZIP_BYTES,
                            'no diagnostic index or ZIP budget')
                    info = zipfile.ZipInfo(name)
                    info.create_system = 3
                    info.external_attr = (stat.S_IFREG | stat.S_IMODE(before.st_mode)) << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                except (OSError, RuntimeError, ValueError):
                    with contextlib.suppress(OSError):
                        print('Cargo timing diagnostic omitted; mandatory originals unchanged', flush=True)
                    continue
                # Actual output-device/ZIP I/O failure keeps its existing strict
                # meaning; only optional source reads and validation are skipped.
                package.writestr(info, raw)
                index, rows, total = proposed, proposed['files'], total + len(raw)
            with package.open('EVIDENCE-INDEX.json', 'w', force_zip64=True) as member:
                for raw in receipt_chunks(index, pretty=True):
                    member.write(raw)
        if slot:
            from apple_handoff import context, transport_pin
            write_json(root / ('evidence-' + slot + '-pin.json'), {
                'slot': slot, 'context': context(), 'archive': transport_pin(output),
                'receiptReadErrors': receipt_errors, 'devicePassed': False})
            write_json(marker, {'slot': slot, 'context': context(), 'complete': staging_safe,
                               'archive': transport_pin(output)})
        print('Original selected bytes/paths preserved; unsigned chain is not device or signing acceptance')


def retire_slot(slot):
    """Retire only rebuildable slot intermediates after exact archive retention.

    The workflow invokes this only after upload succeeds. Borrow the same
    cache/work locks, so surviving native descendants refuse deletion. This
    operation allocates no native build and needs no second 16GiB reservation.
    """
    from apple_handoff import context, transport_pin
    from apple_rust import target_for
    require(slot in required_slots(), 'unrequested Apple retirement slot')
    root = location()
    target = target_for('iphoneos' if slot == 'device-arm64' else 'iphonesimulator',
                        'arm64', 'release')
    archive = root / ('apple-original-' + slot + '-evidence.zip')
    with slot_locks(private_dir(root / ('retirement-' + slot)), private_dir(root / 'cache')):
        pin = read_receipt(root / ('evidence-' + slot + '-pin.json'))
        require(pin['slot'] == slot and pin['context'] == context() and
                pin['archive'] == transport_pin(archive) and not pin['receiptReadErrors'],
                'slot originals are incomplete or changed; intermediates retained')
        target_dir = root / 'cache/rust-target' / target
        native = target_dir / 'release/libpolaris_lib.a'
        with zipfile.ZipFile(archive) as package:
            require(package.getinfo('EVIDENCE-INDEX.json').file_size <= 16 * 1024 ** 2,
                    'original evidence index exceeds bound')
            index = decode(package.read('EVIDENCE-INDEX.json'), maximum=16 * 1024 ** 2)
            require(index['slot'] == slot and not index['receiptReadErrors'], 'slot archive scope differs')
            if native.is_file():
                name = native.relative_to(root).as_posix()
                expected = file_pin(native)
                require({k: index['files'][name][k] for k in ('bytes', 'sha256')} == expected,
                        'native archive missing from preserved originals')
                import hashlib
                digest, size = hashlib.sha256(), 0
                with package.open(name) as original:
                    for chunk in iter(lambda: original.read(1024 * 1024), b''):
                        size += len(chunk)
                        require(size <= expected['bytes'], 'preserved native archive exceeds expected bytes')
                        digest.update(chunk)
                require({'bytes': size, 'sha256': digest.hexdigest()} == expected,
                        'preserved native archive bytes differ')
        before_space = os.statvfs(root)
        removed = []
        for path in (target_dir, root / slot / 'derived-data'):
            if path.exists() or path.is_symlink():
                require(not path.is_symlink() and path.resolve(strict=True) == path and path.is_dir() and
                        path.stat().st_uid == os.getuid(), 'retirement directory identity differs')
                shutil.rmtree(path)  # No component, cache root, receipts, logs, IPA or original ZIP.
                removed.append(str(path))
        write_json(root / ('retirement-' + slot + '.json'), {
            'slot': slot, 'context': context(), 'preservedArchive': pin['archive'],
            'removedRebuildableDirectories': removed,
            'filesystem': root.stat().st_dev,
            'freeBytesBefore': before_space.f_bavail * before_space.f_frsize,
            'freeBytesAfter': os.statvfs(root).f_bavail * before_space.f_frsize,
            'freeSpaceDeltaIsNotExclusiveReleaseMeasurement': True,
            'scope': 'uploaded and byte-verified originals retained; native descendants excluded by cache lock'})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['admit', 'prepare', 'build', 'import', 'import-local', 'carrier', 'aggregate', 'evidence', 'retire-slot'])
    parser.add_argument('--scope', choices=('device-only', 'device-and-simulator'))
    parser.add_argument('--handoff', type=Path)
    parser.add_argument('--slot', choices=[slot for slot, _ in CARRIERS])
    parser.add_argument('--summaries', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.scope is not None:
        require(scope_fields(args.scope) == scope_fields(), 'CLI/environment Apple scope differs')
    required_slots()  # Reject unknown environment scope before work or evidence.
    if args.action == 'import':
        require(args.handoff is not None, 'handoff folder required')
        import_carrier(args.handoff)
    elif args.action == 'carrier':
        require(args.slot is not None, 'exact slot required')
        carrier(args.slot)
    elif args.action == 'aggregate':
        require(args.summaries is not None and args.output is not None, 'aggregate inputs/output required')
        from apple_handoff import aggregate
        require(args.scope is not None, 'explicit aggregate scope required')
        aggregate(args.summaries, args.output, scope=args.scope)
    elif args.action == 'evidence':
        evidence(publish_env=True, slot=args.slot)
    elif args.action == 'retire-slot':
        require(args.slot is not None, 'exact slot required')
        retire_slot(args.slot)
    else:
        {'admit': admit, 'prepare': prepare, 'build': build, 'import-local': import_local}[args.action]()
