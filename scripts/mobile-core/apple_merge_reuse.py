#!/usr/bin/env python3
"""Read-only reuse of a fully bound PR Apple proof for an exact-tree main merge.

This never imports build products or rewrites producer receipts. The separate
bounded result references one original full producer, never another reuse.
Unavailable, ambiguous or incompatible evidence selects the normal full build.
"""
import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile

from artifacts import (MAX_ARTIFACT, MAX_RECEIPT, file_pin, read_receipt, receipt_digest,
                       receipt_lexical_budget, receipt_shape)
from fork_input import ROOT, canonical, decode, load, require, sha

REPOSITORY = 'polaris-arch/Polaris'
REPOSITORY_ID = 1347028220
POLICY = 'polaris-apple-exact-tree-merge-reuse-v1'
CONTRACT_SCHEMA = 'polaris-apple-producer-execution-v1'
RESULT_SCHEMA = 'polaris-apple-merge-equivalence-v1'
SNAPSHOT_SCHEMA = 'polaris-apple-execution-snapshot-v2'
WORKFLOWS = ('.github/workflows/release-risk.yml', '.github/workflows/apple.yml')
COMPONENT_JOB = 'apple unsigned App and PacketTunnel gate / component'
CHAIN_JOB = 'apple unsigned App and PacketTunnel gate / unsigned-chain'
MAX_JSON = 64 * 1024
MAX_API = 8 * 1024 ** 2
MAX_ZIP = 5 * 1024 ** 3
MAX_MEMBERS = 65536
MAX_EXPANDED = 5 * 1024 ** 3
MAX_DOWNLOAD_TOTAL = 16 * 1024 ** 3
FALSE_CLAIMS = {'devicePassed': False, 'signatureTrustValidated': False,
                'publicationEligible': False}
ARTIFACT_ROLES = ('component', 'component-manifest', 'component-original',
                  'carrier-summary-device-arm64', 'carrier-original-device-arm64', 'chain')
PHASES = ('frontend-before', 'frontend-after', 'carrier-before', 'carrier-after')


def signed(value):
    require('fingerprint' not in value, 'fingerprint already supplied')
    return {**value, 'fingerprint': receipt_digest(value)}


def fingerprint(value):
    require(isinstance(value, dict) and value.get('fingerprint') ==
            receipt_digest({k: v for k, v in value.items() if k != 'fingerprint'}),
            'receipt fingerprint differs')
    return value


def safe_json(raw, maximum=MAX_RECEIPT):
    require(len(raw) <= maximum, 'JSON byte budget exceeded')
    receipt_lexical_budget(raw)
    result = decode(raw, maximum=maximum)
    receipt_shape(result, maximum)
    return result


def pin_bytes(raw):
    return {'bytes': len(raw), 'sha256': sha(raw)}


def pin_shape(pin):
    require(isinstance(pin, dict) and set(pin) == {'bytes', 'sha256'} and
            type(pin['bytes']) is int and 0 <= pin['bytes'] <= MAX_ZIP and
            isinstance(pin['sha256'], str) and re.fullmatch('[0-9a-f]{64}', pin['sha256']),
            'invalid byte pin')
    return pin


def sha40(value):
    require(isinstance(value, str) and re.fullmatch('[0-9a-f]{40}', value), 'exact Git SHA required')
    return value


def command(args):
    result = subprocess.run([str(a) for a in args], cwd=ROOT, check=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
    require(len(result.stdout) <= MAX_RECEIPT, 'command output budget')
    return result.stdout.decode().strip()


def write_new(path, value, maximum=MAX_JSON):
    path = Path(path)
    raw = canonical(value) + b'\n'
    require(len(raw) <= maximum, 'reference-only receipt byte budget')
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('xb') as stream:
        stream.write(raw)
    return file_pin(path)


def current_context():
    from apple_handoff import context
    ctx = context()
    require(ctx['repository'] == REPOSITORY, 'repository differs')
    require(os.environ.get('GITHUB_REPOSITORY_ID') == str(REPOSITORY_ID), 'repository ID differs')
    sha40(ctx['candidate'])
    return ctx


def source_bindings():
    from producer import producer_code
    from consume import app_identity
    producer, app = producer_code(), app_identity()
    require(producer['candidate'] == app['candidate'], 'current source identities differ')
    tree = sha40(command(['git', 'rev-parse', 'HEAD^{tree}']))
    return {'candidate': producer['candidate'], 'tree': tree,
            'producer': producer, 'app': app,
            'contract': load(), 'sourceContract': file_pin(ROOT / 'scripts/mobile-core/source-contract.json'),
            'producerCode': sha(canonical(producer['files'])),
            'consumerCode': sha(canonical(app['consumerCode'])),
            'verifier': file_pin(Path(__file__)),
            'workflows': {name: file_pin(ROOT / name) for name in WORKFLOWS}}


def source_summary(source):
    return {key: source[key] for key in ('tree', 'sourceContract', 'producerCode',
                                        'consumerCode', 'verifier', 'workflows')}


def tool(path, args):
    path = Path(path).resolve(strict=True)
    require(path.is_file() and not path.is_symlink(), 'actual regular tool required')
    return {'path': str(path), **file_pin(path), 'version': command([path, *args])}


class UnsupportedCargoInputs(RuntimeError):
    """A full build remains valid, but its environment cannot qualify for reuse."""


def admit_cargo(condition, reason):
    if not condition:
        raise UnsupportedCargoInputs(reason)


UNAVAILABLE_SCHEMA = 'polaris-apple-execution-unavailable-v1'
UNAVAILABLE_REASON = 'unsupported-cargo-inputs'


# Cargo config has environment aliases for every supported scalar/array key.
# Snapshot the complete CARGO_* namespace, plus direct Rust/native aliases.
# Runtime-only concurrency/FD handles are deliberately outside semantic identity:
# budget.native_environment changes jobs, and Cargo sets its own jobserver handles.
CARGO_RUNTIME_ENV = frozenset({'CARGO_BUILD_JOBS', 'CARGO_MAKEFLAGS', 'MAKEFLAGS', 'MFLAGS'})
CARGO_UNSUPPORTED_ENV = frozenset({'CARGO', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
    'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER',
    'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTDOC', 'RUSTDOC', 'RUST_TARGET_PATH', 'RUSTC_BOOTSTRAP', 'CARGO_RESOLVER_LOCKFILE_PATH', 'CARGO_BUILD_TARGET'})
CARGO_FIXED_CHILD_ENV = frozenset({'CARGO_TARGET_DIR', 'RUSTC'})


def environment_key(key):
    return key.startswith(('CARGO_', 'RUST', 'CFLAGS', 'CXXFLAGS', 'OBJC_INCLUDE_PATH')) or key == 'CARGO'


def unsupported_cargo_key(key):
    return key in CARGO_UNSUPPORTED_ENV or key.startswith(('CARGO_UNSTABLE_', 'CARGO_ALIAS_', 'CARGO_SOURCE_')) or \
        (key.startswith('CARGO_TARGET_') and key.endswith(('_LINKER', '_RUNNER'))) or \
        (key.startswith(('CARGO_REGISTRY_', 'CARGO_REGISTRIES_')) and key.endswith('_INDEX'))


def cargo_secret_key(key):
    return key.startswith(('CARGO_CREDENTIAL_ALIAS_', 'CARGO_HTTP_')) or \
        (key.startswith(('CARGO_REGISTRY_', 'CARGO_REGISTRIES_')) and ('TOKEN' in key or 'CREDENTIAL' in key))


def environment_pins(env):
    pins = {}
    for key, value in env.items():
        if not environment_key(key) or key in CARGO_RUNTIME_ENV:
            continue
        admit_cargo(re.fullmatch('[A-Za-z_][A-Za-z0-9_]*', key) and len(key) <= 256,
                'unsupported execution environment key')
        # Credentials are transport authority, never compiler-input evidence.
        # Even hashes of credential values are not exported.
        if cargo_secret_key(key):
            continue
        admit_cargo(not unsupported_cargo_key(key), 'unsupported Cargo wrapper/compiler/config override')
        # Arbitrary flags can point at unretained sysroots, response files or
        # link inputs. This initial reuse policy admits tracked config flags;
        # nonempty ambient flags refuse, without clearing/changing the build.
        admit_cargo(not (key.endswith(('RUSTFLAGS', 'RUSTDOCFLAGS')) and value),
                'unsupported ambient Cargo compiler flags')
        raw = value.encode()
        admit_cargo(len(raw) <= 16384, 'execution environment value budget')
        pins[key] = pin_bytes(raw)
    admit_cargo(len(pins) <= 128 and len(canonical(pins)) <= 32768, 'execution environment contract budget')
    return pins


def cargo_configuration(env, cwd=None, source_root=None):
    """Observe Cargo's actual hierarchy, never read credential files/contents."""
    try:
        import tomllib
    except ImportError:
        raise UnsupportedCargoInputs('Cargo config parser unavailable') from None
    source_root = Path(source_root or ROOT).resolve(strict=True)
    cwd = Path(cwd or source_root / 'src-tauri').resolve(strict=True)
    admit_cargo(cwd == source_root / 'src-tauri', 'Cargo invocation directory differs')
    home = Path(env.get('CARGO_HOME') or str(Path(env.get('HOME') or str(Path.home())) / '.cargo'))
    if not home.is_absolute():
        home = cwd / home
    directories = [cwd, *cwd.parents]
    candidates = [directory / '.cargo' / name for directory in directories for name in ('config', 'config.toml')]
    candidates += [home / name for name in ('config', 'config.toml')]
    allowed = {source_root / '.cargo' / name for name in ('config', 'config.toml')} | \
              {cwd / '.cargo' / name for name in ('config', 'config.toml')}
    result = {}
    for path in dict.fromkeys(candidates):
        if not path.exists() and not path.is_symlink():
            continue
        admit_cargo(path in allowed and not path.is_symlink() and path.is_file() and
                path.resolve(strict=True) == path, 'unsupported external or redirected Cargo configuration')
        raw = path.read_bytes()
        admit_cargo(len(raw) <= 65536, 'Cargo configuration byte budget')
        try:
            parsed = tomllib.loads(raw.decode())
        except (ValueError, UnicodeError):
            raise UnsupportedCargoInputs('unsupported Cargo configuration syntax') from None
        # Includes, env injection, local source/path overrides and tool selectors
        # need a separate complete contract. Do not follow those external edges.
        admit_cargo(set(parsed) <= {'target', 'profile', 'build', 'term'}, 'unsupported Cargo configuration section')
        admit_cargo(isinstance(parsed.get('build', {}), dict) and set(parsed.get('build', {})) <=
                {'jobs', 'incremental', 'warnings'}, 'unsupported Cargo build configuration')
        targets = parsed.get('target', {})
        admit_cargo(isinstance(targets, dict) and all(isinstance(row, dict) and
                set(row) <= {'rustflags', 'rustdocflags'} for row in targets.values()),
                'unsupported Cargo target/tool/link configuration')
        for target in targets.values():
            for name in ('rustflags', 'rustdocflags'):
                flags = target.get(name, [])
                admit_cargo(flags == [] or (name == 'rustflags' and flags in (
                    ['-C', 'target-feature=+crt-static'], ['-C', 'strip=debuginfo'])),
                    'unsupported source Cargo compiler flags')
        profiles = parsed.get('profile', {})
        admit_cargo(isinstance(profiles, dict), 'unsupported source Cargo profile shape')
        scalar_profile = {'opt-level', 'debug', 'split-debuginfo', 'strip', 'debug-assertions',
                          'overflow-checks', 'lto', 'panic', 'incremental', 'codegen-units', 'rpath', 'inherits'}
        for profile in profiles.values():
            admit_cargo(isinstance(profile, dict) and set(profile) <= scalar_profile | {'build-override', 'package'},
                        'unsupported source Cargo profile configuration')
            packages = profile.get('package', {})
            admit_cargo(isinstance(packages, dict), 'unsupported source Cargo package profile')
            nested = [profile.get('build-override', {}), *packages.values()]
            admit_cargo(all(isinstance(row, dict) and set(row) <= scalar_profile for row in nested),
                        'unsupported source Cargo nested profile configuration')
        result[path.relative_to(source_root).as_posix()] = pin_bytes(raw)
    return result


def cargo_execution_inputs(env):
    return {'overrides': environment_pins(env), 'cargoConfig': cargo_configuration(env)}


def observe_cargo_execution_inputs(env):
    try:
        return cargo_execution_inputs(env)
    except UnsupportedCargoInputs:
        return {'unavailable': UNAVAILABLE_REASON}


def validate_environment_pins(pins):
    require(isinstance(pins, dict) and len(pins) <= 128, 'execution environment shape differs')
    for key, pin in pins.items():
        require(isinstance(key, str) and re.fullmatch('[A-Za-z_][A-Za-z0-9_]*', key) and
                len(key) <= 256 and environment_key(key) and key not in CARGO_RUNTIME_ENV and
                not unsupported_cargo_key(key) and not cargo_secret_key(key), 'unsupported recorded execution override')
        pin_shape(pin)
        require(pin['bytes'] <= 16384, 'execution environment pin budget')
        if key.endswith(('RUSTFLAGS', 'RUSTDOCFLAGS')):
            require(pin == pin_bytes(b''), 'unsupported recorded ambient compiler flags')


def validate_cargo_configuration(pins):
    require(isinstance(pins, dict) and set(pins) <= {'.cargo/config', '.cargo/config.toml',
            'src-tauri/.cargo/config', 'src-tauri/.cargo/config.toml'}, 'recorded Cargo configuration scope differs')
    expected = {path for path in ('.cargo/config', '.cargo/config.toml', 'src-tauri/.cargo/config',
                'src-tauri/.cargo/config.toml') if (ROOT / path).exists() or (ROOT / path).is_symlink()}
    require(set(pins) == expected, 'Cargo configuration source inventory differs')
    for path, pin in pins.items():
        require(not (ROOT / path).is_symlink() and pin_shape(pin) == file_pin(ROOT / path),
                'Cargo configuration source bytes differ')


def snapshot(phase, slot=None):
    from ci_apple import location, selected_apple_tools
    from producer import private_dir
    require(phase in PHASES and os.environ.get('POLARIS_APPLE_SCOPE') == 'device-only',
            'only declared device-only snapshots supported')
    require((phase.startswith('carrier-') and slot == 'device-arm64') or
            (phase.startswith('frontend-') and slot is None), 'snapshot slot differs')
    root = location()
    # A before-snapshot is the first writer in the carrier work slot. Admit
    # it through the producer boundary, never generic receipt-parent mkdir.
    target = private_dir(root / slot) if slot else root
    selected_apple_tools(load())
    prepared = read_receipt(root / 'tools.json')
    paths = {name: prepared['paths'][name] for name in ('go', 'gomobile', 'gobind')}
    paths.update({name: shutil.which(name) for name in ('node', 'pnpm', 'xcodebuild', 'xcrun')})
    paths.update({name: command(['rustup', 'which', name]) for name in ('cargo', 'rustc')})
    paths['swiftc'] = command(['xcrun', '--find', 'swiftc'])
    paths['clang'] = command(['xcrun', '--sdk', 'iphoneos', '--find', 'clang'])
    if slot:
        paths['protoc'] = os.environ.get('PROTOC') or shutil.which('protoc')
    require(all(paths.values()), 'tool snapshot is incomplete')
    args = {'go': ['version'], 'gomobile': ['version'], 'gobind': ['-h'],
            'node': ['--version'], 'pnpm': ['--version'], 'xcodebuild': ['-version'],
            'xcrun': ['--version'], 'cargo': ['-V'], 'rustc': ['-vV'],
            'swiftc': ['--version'], 'clang': ['--version'], 'protoc': ['--version']}
    # Generator version/build information is already validated by producer.py;
    # invoking gobind -h is not a stable successful version command.
    facts = {name: ({'path': str(Path(path).resolve(strict=True)), **file_pin(path),
                     'version': 'source-contract:' + load()[name + 'Version']}
                    if name in ('gomobile', 'gobind') else tool(path, args[name]))
             for name, path in paths.items()}
    sdk = {}
    for name in ('iphoneos', 'iphonesimulator'):
        sdk_path = Path(command(['xcrun', '--sdk', name, '--show-sdk-path'])).resolve(strict=True)
        sdk[name] = {'version': command(['xcrun', '--sdk', name, '--show-sdk-version']),
                     'settings': file_pin(sdk_path / 'SDKSettings.json')}
    inputs = observe_cargo_execution_inputs(os.environ)
    if 'unavailable' in inputs:
        row = signed({'schema': UNAVAILABLE_SCHEMA, 'context': current_context(), 'phase': phase,
                      'slot': slot, 'workflowSha': sha40(os.environ.get('GITHUB_WORKFLOW_SHA')),
                      'source': source_summary(source_bindings()), 'reason': UNAVAILABLE_REASON, **FALSE_CLAIMS})
        write_new(target / ('execution-' + phase + '.json'), row)
        print('Cargo inputs cannot qualify for reuse; continuing the full native build.', flush=True)
        return
    row = signed({'schema': SNAPSHOT_SCHEMA, 'context': current_context(), 'phase': phase,
                  'slot': slot, 'workflowSha': sha40(os.environ.get('GITHUB_WORKFLOW_SHA')),
                  'source': source_summary(source_bindings()), 'tools': facts, 'sdk': sdk,
                  **inputs, **FALSE_CLAIMS})
    validate_snapshot(row, phase, current_context(), source_summary(source_bindings()), load())
    write_new(target / ('execution-' + phase + '.json'), row)


def validate_snapshot(row, phase, ctx, source, contract, *, collect_unavailable=False):
    fingerprint(row)
    if row.get('schema') == UNAVAILABLE_SCHEMA:
        require(set(row) == {'schema', 'context', 'phase', 'slot', 'workflowSha', 'source',
                'reason', 'fingerprint', *FALSE_CLAIMS} and row['context'] == ctx and row['phase'] == phase and
                row['slot'] == ('device-arm64' if phase.startswith('carrier-') else None) and
                row['source'] == source and row['reason'] == UNAVAILABLE_REASON and
                all(row[k] is False for k in FALSE_CLAIMS), 'invalid unavailable execution snapshot')
        sha40(row['workflowSha'])
        if not collect_unavailable:
            raise UnsupportedCargoInputs('original Cargo inputs do not qualify for reuse')
        return row
    require(set(row) == {'schema', 'context', 'phase', 'slot', 'workflowSha', 'source', 'tools',
                         'sdk', 'overrides', 'cargoConfig', 'fingerprint', *FALSE_CLAIMS}, 'snapshot field contract differs')
    require(row['schema'] == SNAPSHOT_SCHEMA and row['context'] == ctx and row['phase'] == phase and
            row['slot'] == ('device-arm64' if phase.startswith('carrier-') else None) and
            row['source'] == source and all(row[k] is False for k in FALSE_CLAIMS), 'snapshot identity differs')
    sha40(row['workflowSha'])
    expected = {'node', 'pnpm', 'go', 'gomobile', 'gobind', 'cargo', 'rustc', 'xcodebuild', 'xcrun', 'swiftc', 'clang'}
    if phase.startswith('carrier-'):
        expected.add('protoc')
    require(set(row['tools']) == expected, 'tool snapshot set differs')
    for value in row['tools'].values():
        require(set(value) == {'path', 'bytes', 'sha256', 'version'} and isinstance(value['path'], str) and
                value['path'].startswith('/') and isinstance(value['version'], str) and value['version'],
                'concrete tool identity required')
        pin_shape({k: value[k] for k in ('bytes', 'sha256')})
        require(value['bytes'] > 0, 'empty tool identity')
    tools = row['tools']
    require(re.fullmatch(r'v26\.\d+\.\d+', tools['node']['version']) and tools['pnpm']['version'] == '11.24.0',
            'frontend tool policy differs')
    require(tools['go']['version'].startswith('go version go' + contract['goVersion'] + ' ') and
            tools['gomobile']['version'] == 'source-contract:' + contract['gomobileVersion'] and
            tools['gobind']['version'] == 'source-contract:' + contract['gobindVersion'], 'Go tool policy differs')
    require(tools['xcodebuild']['version'] == 'Xcode ' + contract['apple']['xcodeVersion'] +
            '\nBuild version ' + contract['apple']['xcodeBuild'], 'Xcode policy differs')
    require(set(row['sdk']) == {'iphoneos', 'iphonesimulator'}, 'SDK inventory differs')
    for sdk, key in [('iphoneos', 'iPhoneSDK'), ('iphonesimulator', 'simulatorSDK')]:
        require(set(row['sdk'][sdk]) == {'version', 'settings'} and
                row['sdk'][sdk]['version'] == contract['apple'][key], 'SDK version differs')
        pin_shape(row['sdk'][sdk]['settings'])
    if 'protoc' in tools:
        version = re.search(r"const PROTOC_VERSION = '([^']+)';", (ROOT / 'scripts/fetch-protoc.mjs').read_text())
        require(version and tools['protoc']['version'] == 'libprotoc ' + version[1], 'protobuf compiler differs')
    validate_environment_pins(row['overrides'])
    validate_cargo_configuration(row['cargoConfig'])
    return row


def snapshot_pair(before, after):
    require({k: v for k, v in before.items() if k not in ('phase', 'fingerprint')} ==
            {k: v for k, v in after.items() if k not in ('phase', 'fingerprint')},
            'actual tools/SDK/source changed during execution')


def safe_member(name):
    require(isinstance(name, str) and name and len(name.encode()) <= 4096 and '\\' not in name and
            not name.startswith('/') and not re.match(r'^[A-Za-z]:', name) and
            all(part not in ('', '.', '..') for part in (name[:-1] if name.endswith('/') else name).split('/')),
            'unsafe archive member')
    return name


def zip_inventory(package):
    rows = package.infolist()
    require(len(rows) <= MAX_MEMBERS and len({row.filename for row in rows}) == len(rows) and
            sum(row.file_size for row in rows) <= MAX_EXPANDED, 'archive member/byte budget or duplicates')
    for row in rows:
        safe_member(row.filename)
        require(not row.flag_bits & 1 and row.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED),
                'encrypted/unsupported archive')
        mode = row.external_attr >> 16
        require(stat.S_IFMT(mode) in (0, stat.S_IFREG, stat.S_IFDIR, stat.S_IFLNK), 'special archive member')
    return {row.filename: row for row in rows}


def member_pin(package, name, maximum=MAX_EXPANDED):
    row = package.getinfo(name)
    require(row.file_size <= maximum, 'member byte budget')
    digest, count = hashlib.sha256(), 0
    with package.open(row) as stream:
        while True:
            raw = stream.read(min(1024 ** 2, maximum - count + 1))
            if not raw:
                break
            count += len(raw)
            require(count <= maximum, 'member grew beyond budget')
            digest.update(raw)
    require(count == row.file_size, 'member length differs')
    return {'bytes': count, 'sha256': digest.hexdigest()}


def json_member(package, name, maximum=MAX_RECEIPT):
    require(package.getinfo(name).file_size <= maximum, 'receipt member exceeds budget')
    return safe_json(package.read(name), maximum)


@contextlib.contextmanager
def nested_zip(outer, name):
    info = outer.getinfo(name)
    require(0 < info.file_size <= MAX_ZIP and
            shutil.disk_usage(tempfile.gettempdir()).free >= info.file_size + 1024 ** 3,
            'nested evidence exceeds available bounded disk')
    with tempfile.TemporaryFile() as stream:
        total = 0
        with outer.open(name) as source:
            while True:
                raw = source.read(1024 ** 2)
                if not raw:
                    break
                total += len(raw)
                require(total <= info.file_size, 'nested evidence exceeds declared bytes')
                stream.write(raw)
        require(total == info.file_size, 'nested evidence incomplete')
        stream.seek(0)
        with zipfile.ZipFile(stream) as package:
            zip_inventory(package)
            yield package


def check_original(package, ctx, slot):
    rows = zip_inventory(package)
    index = json_member(package, 'EVIDENCE-INDEX.json')
    require(index.get('schema') == 'polaris-apple-original-evidence-v2' and
            index.get('carrierScope') == 'device-only' and index.get('requiredSlots') == ['device-arm64'] and
            index.get('slot') == slot and index.get('receiptReadErrors') == {} and
            index.get('componentAccepted') is True and index.get('devicePassed') is False and
            index.get('publicationEligible') is False and
            index.get('sameRunComponentArtifact') == artifact_name('component-original', ctx),
            'original evidence scope/acceptance differs')
    require(index.get('componentOriginalsInThisArchive') is (slot is None) and
            index.get('componentOriginalsReferencedByTransport') is (slot is not None),
            'original component retention differs')
    files = index['files']
    require(isinstance(files, dict) and set(rows) == set(files) | {'EVIDENCE-INDEX.json'},
            'original evidence has missing/extra members')
    for name, wanted in files.items():
        safe_member(name)
        require(isinstance(wanted.get('originalPath'), str) and wanted['originalPath'].startswith('/'),
                'original path missing')
        if 'link' in wanted:
            require(stat.S_ISLNK(rows[name].external_attr >> 16) and
                    isinstance(wanted['link'], str) and not wanted['link'].startswith('/') and
                    '..' not in PurePosixPath(wanted['link']).parts and
                    rows[name].file_size == len(wanted['link'].encode()) <= 4096 and
                    package.read(name) == wanted['link'].encode(), 'original link differs')
        else:
            pin = pin_shape({key: wanted[key] for key in ('bytes', 'sha256')})
            require(not stat.S_ISLNK(rows[name].external_attr >> 16) and
                    member_pin(package, name, pin['bytes']) == pin, 'original bytes differ')
    return index


def original_relative(path, root):
    require(isinstance(path, str) and path.startswith(root + '/'), 'original path escaped exact producer root')
    relative = path[len(root) + 1:]
    safe_member(relative)
    require(not relative.endswith('/'), 'original path is not canonical')
    return relative


def validate_native_closure(package, index, manifest, receipt):
    """Check receipt -> payload -> original edges without touching original paths."""
    from native_inputs import reference_layout, verify_mapping_shape, source_table, bind_origins, origin_identity
    root, transported = manifest['root'], manifest['files']['root']
    original = index['files']
    entries = zip_inventory(package)

    def retained(name, pin=None):
        safe_member(name)
        require(name in original and name in transported, 'required original or transported input missing')
        row, wanted, info = original[name], transported[name], entries[name]
        require(row['originalPath'] == root + '/' + name, 'original input path relabelled')
        mode = info.external_attr >> 16
        require(stat.S_IMODE(mode) == wanted['mode'], 'original/transport input mode differs')
        if 'link' in wanted:
            require(pin is None and stat.S_ISLNK(mode) and row.get('link') == wanted['link'],
                    'original/transport input link differs')
        else:
            require(stat.S_ISREG(mode) and not wanted.get('directory') and 'link' not in row and
                    {k: row[k] for k in ('bytes', 'sha256')} ==
                    {k: wanted[k] for k in ('bytes', 'sha256')}, 'original/transport input bytes or type differ')
            if pin is not None:
                require(pin_shape(pin) == {k: row[k] for k in ('bytes', 'sha256')}, 'native input receipt bytes differ')
        return row

    # Compare every overlapping retained item, not merely XCFramework binaries.
    for name in original.keys() & transported.keys():
        retained(name)

    def inventory(directory, rows, *, retain):
        prefix = original_relative(directory, root) + '/'
        require(isinstance(rows, list) and 0 < len(rows) <= MAX_MEMBERS, 'original inventory missing or excessive')
        expected = {}
        for row in rows:
            require(isinstance(row, dict), 'original inventory row differs')
            name = safe_member(row['path'])
            require(name not in expected and type(row['mode']) is int and 0 <= row['mode'] <= 0o777,
                    'original inventory duplicate/mode differs')
            kind = row['type']
            if kind == 'file':
                require(set(row) == {'path', 'mode', 'type', 'bytes', 'sha256'}, 'file inventory fields differ')
                wanted = {**pin_shape({k: row[k] for k in ('bytes', 'sha256')}), 'mode': row['mode']}
            elif kind == 'directory':
                require(set(row) == {'path', 'mode', 'type'}, 'directory inventory fields differ')
                wanted = {'directory': True, 'mode': row['mode']}
            else:
                require(kind == 'symlink' and set(row) == {'path', 'mode', 'type', 'target'}, 'link inventory fields differ')
                wanted = {'link': row['target'], 'mode': row['mode']}
            expected[name] = wanted
            require(transported.get(prefix + name) == wanted, 'receipt/transport inventory differs')
            if retain and kind != 'directory':
                retained(prefix + name, {k: row[k] for k in ('bytes', 'sha256')} if kind == 'file' else None)
        require({name[len(prefix):] for name in transported if name.startswith(prefix)} == set(expected),
                'receipt/transport inventory missing or extra edge')
        if retain:
            require({name[len(prefix):] for name in original if name.startswith(prefix)} ==
                    {name for name, row in expected.items() if not row.get('directory')},
                    'original inventory missing or extra edge')

    native = receipt['nativeInputs']
    require(isinstance(native, dict) and set(native) == {'schema', 'scratch', 'cache', 'workRoots',
            'originalBindLog', 'files', 'staticMemberOrigins', 'originsByDigest',
            'generatorOutputRoot', 'generatorOutputInventory'} and
            native['schema'] == 'polaris-mobile-native-inputs-v2', 'complete native input schema required')
    scratch = original_relative(native['scratch'], root)
    cache = original_relative(native['cache'], root)
    require(scratch.startswith('component/') and cache == 'cache', 'native owned roots differ')
    roots = native['workRoots']
    require(isinstance(roots, list) and 0 < len(roots) <= 32 and len(set(roots)) == len(roots) and
            all(original_relative(path, root).startswith(scratch + '/') for path in roots), 'native work roots differ')
    retained('component/original-bind.log', native['originalBindLog'])
    files = native['files']
    require(isinstance(files, dict) and 0 < len(files) <= MAX_MEMBERS, 'native input inventory bounds')
    for path, pin in files.items():
        name = original_relative(path, root)
        require(isinstance(pin, dict) and set(pin) == {'kind', 'bytes', 'sha256'} and
                pin['kind'] in ('compiler-object', 'external-static-archive'), 'native input kind/fields differ')
        require(name.startswith((scratch if pin['kind'] == 'compiler-object' else cache) + '/'),
                'native input outside owned scope')
        if pin['kind'] == 'compiler-object':
            require(any(path.startswith(work + '/') for work in roots), 'native object outside recorded work roots')
        retained(name, {key: pin[key] for key in ('bytes', 'sha256')})
    # A dropped compiler-object edge cannot hide a retained object under WORK.
    objects = {root + '/' + name for name in original.keys() | transported.keys()
               if name.endswith('.o') and any((root + '/' + name).startswith(work + '/') for work in roots)}
    require(objects == {path for path, pin in files.items() if pin['kind'] == 'compiler-object'},
            'native compiler object inventory missing/extra edge')
    archives = {root + '/' + name for name in original.keys() | transported.keys()
                if name.startswith(cache + '/') and name.endswith('.a')}
    require(archives == {path for path, pin in files.items() if pin['kind'] == 'external-static-archive'},
            'native external archive inventory missing/extra edge')
    output = receipt['output']
    refs = reference_layout(output)
    verify_mapping_shape(native, refs)
    def archived_reader(path):
        name = original_relative(path, root)
        require(entries[name].file_size <= MAX_ARTIFACT, 'external archive read budget')
        raw = package.read(name)
        return raw, pin_bytes(raw)
    actual = bind_origins(refs, source_table(files, reader=archived_reader))
    require(native['originsByDigest'] == actual and native['staticMemberOrigins'] == refs,
            'native source/member origin closure differs')
    for choices in actual.values():
        for row in choices:
            origin_identity(row)
    inventory(root + '/component/Libbox.xcframework', output['inventory'], retain=True)
    generated = original_relative(native['generatorOutputRoot'], root)
    require(generated.startswith('component/') and generated.endswith('/build-view/build'), 'generator output root differs')
    inventory(native['generatorOutputRoot'], native['generatorOutputInventory'], retain=True)

    graph = receipt['input']['moduleGraph']
    require(isinstance(graph, list) and 0 < len(graph) <= 65536 and all(isinstance(row, dict) for row in graph),
            'native module graph missing')
    mains = [row for row in graph if row.get('Main') is True]
    require(len(mains) == 1 and original_relative(mains[0]['Dir'], root).startswith('component/'),
            'native source view differs')
    source_root = mains[0]['Dir']
    expected = []
    for row in graph:
        replacement = row.get('Replace')
        if not replacement or replacement.get('Version'):
            continue
        literal = replacement['Path']
        require(isinstance(literal, str) and literal.startswith('./'), 'source replacement path differs')
        relative = safe_member(literal[2:])
        require(replacement['Dir'] == source_root + '/' + relative, 'source replacement root relabelled')
        expected.append({'module': row['Path'], 'path': literal, 'sourceRoot': source_root,
                         'directory': replacement['Dir']})
    snapshots = manifest['localReplacementSources']
    require(isinstance(snapshots, list) and len(expected) == len(snapshots) <= 1024,
            'source replacement inventory missing/extra edge')
    for row, wanted in zip(snapshots, expected):
        require(set(row) == {*wanted, 'inventory', 'goMod', 'goSum'} and
                {key: row[key] for key in wanted} == wanted, 'source replacement identity differs')
        inventory(row['directory'], row['inventory'], retain=False)
        for name, key, contract_key in [('go.mod', 'goMod', 'goModSha256'), ('go.sum', 'goSum', 'goSumSha256')]:
            member = original_relative(source_root + '/' + name, root)
            require(pin_shape(row[key]) == {k: transported[member][k] for k in ('bytes', 'sha256')} and
                    row[key]['sha256'] == receipt['input']['sourceContract'][contract_key],
                    'source module receipt/transport bytes differ')


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class GitHub:
    """GET-only provider access. Never forward the token to artifact storage."""
    def __init__(self, token, work):
        require(token, 'read-only GitHub token unavailable')
        self.token, self.work, self.downloaded = token, Path(work), 0
        self.opener = urllib.request.build_opener(NoRedirect())

    def request(self, path):
        routes = (
            r'/git/commits/[0-9a-f]{40}',
            r'/commits/[0-9a-f]{40}/pulls\?per_page=100',
            r'/pulls/[1-9][0-9]*',
            r'/actions/runs/[1-9][0-9]*/attempts/[1-9][0-9]*(?:/jobs\?per_page=100)?',
            r'/actions/runs/[1-9][0-9]*/artifacts\?per_page=100',
            r'/actions/artifacts/[1-9][0-9]*/zip',
            r'/actions/workflows/release-risk\.yml/runs\?event=pull_request&status=success&head_sha=[0-9a-f]{40}&per_page=100',
        )
        require(isinstance(path, str) and any(re.fullmatch(route, path) for route in routes),
                'invalid provider route')
        return urllib.request.Request('https://api.github.com/repos/' + REPOSITORY + path,
            headers={'Authorization': 'Bearer ' + self.token, 'Accept': 'application/vnd.github+json',
                     'X-GitHub-Api-Version': '2022-11-28'}, method='GET')

    def get(self, path):
        with self.opener.open(self.request(path), timeout=30) as response:
            raw = response.read(MAX_API + 1)
        return safe_json(raw, MAX_API)

    def artifact(self, row):
        identifier = row['id']
        require(shutil.disk_usage(self.work).free >= row['size_in_bytes'] + 1024 ** 3,
                'artifact download exceeds available bounded disk')
        require(type(identifier) is int and identifier > 0, 'artifact ID differs')
        try:
            response = self.opener.open(self.request('/actions/artifacts/' + str(identifier) + '/zip'), timeout=30)
        except urllib.error.HTTPError as error:
            require(error.code in (301, 302, 303, 307, 308), 'artifact provider download unavailable')
            url = error.headers.get('Location', '')
            parsed = urllib.parse.urlsplit(url)
            host = parsed.hostname or ''
            require(parsed.scheme == 'https' and parsed.port in (None, 443) and
                    parsed.username is None and parsed.password is None and
                    (host.endswith('.blob.core.windows.net') or host.endswith('.actions.githubusercontent.com')),
                    'artifact redirect is not an admitted provider')
            # No Authorization header and no follow-up redirects to arbitrary hosts.
            response = self.opener.open(urllib.request.Request(url, method='GET'), timeout=60)
        target = self.work / (str(identifier) + '.zip')
        digest, count = hashlib.sha256(), 0
        with response, target.open('xb') as stream:
            while True:
                raw = response.read(1024 ** 2)
                if not raw:
                    break
                count += len(raw)
                self.downloaded += len(raw)
                require(count <= MAX_ZIP and self.downloaded <= MAX_DOWNLOAD_TOTAL,
                        'artifact download byte budget')
                stream.write(raw)
                digest.update(raw)
        require(count == row['size_in_bytes'] and 'sha256:' + digest.hexdigest() == row['digest'],
                'provider artifact digest/size differs')
        return target


def artifact_name(role, ctx):
    return 'apple-' + role + '-' + ctx['runId'] + '-' + ctx['attempt']


def artifact_reference(row):
    return {key: row[key] for key in ('id', 'name', 'size_in_bytes', 'digest')}


def artifact_rows(api, ctx, *, contract=False):
    data = api.get('/actions/runs/' + ctx['runId'] + '/artifacts?per_page=100')
    rows = data['artifacts']
    require(data['total_count'] == len(rows) and len(rows) <= 100, 'artifact listing incomplete')
    roles = (*ARTIFACT_ROLES, 'execution-contract') if contract else ARTIFACT_ROLES
    selected = {}
    for role in roles:
        matches = [row for row in rows if row.get('name') == artifact_name(role, ctx)]
        require(len(matches) == 1, 'missing/duplicate exact-attempt artifact: ' + role)
        row = matches[0]
        run = row.get('workflow_run', {})
        require(row.get('expired') is False and type(row.get('id')) is int and row['id'] > 0 and
                type(row.get('size_in_bytes')) is int and 0 < row['size_in_bytes'] <= MAX_ZIP and
                re.fullmatch(r'sha256:[0-9a-f]{64}', row.get('digest') or '') and
                run.get('id') == int(ctx['runId']) and run.get('repository_id') == REPOSITORY_ID and
                run.get('head_repository_id') == REPOSITORY_ID and run.get('head_sha') == ctx['candidate'],
                'artifact owner/run/hash differs')
        selected[role] = row
    require(len({row['id'] for row in selected.values()}) == len(selected), 'artifact ID reused across roles')
    return selected


def run_contract(api, ctx, source, *, completed):
    run = api.get('/actions/runs/' + ctx['runId'] + '/attempts/' + ctx['attempt'])
    require(run.get('id') == int(ctx['runId']) and run.get('run_attempt') == int(ctx['attempt']) and
            run.get('head_sha') == ctx['candidate'] and run.get('path') == WORKFLOWS[0] and
            run.get('repository', {}).get('id') == REPOSITORY_ID and
            run.get('head_repository', {}).get('id') == REPOSITORY_ID and
            run.get('event') == 'pull_request', 'original run identity differs')
    if completed:
        require(run.get('event') == 'pull_request' and run.get('status') == 'completed' and
                run.get('conclusion') == 'success', 'original PR attempt did not succeed')
    else:
        require(run.get('status') == 'in_progress' and run.get('conclusion') is None,
                'producer seal requires its active PR run')
    references = [row for row in run.get('referenced_workflows', []) if
                  row.get('path', '').startswith(REPOSITORY + '/' + WORKFLOWS[1] + '@')]
    require(len(references) == 1, 'actual reusable workflow identity missing/ambiguous')
    workflow_sha = sha40(references[0]['sha'])
    require(references[0]['path'] == REPOSITORY + '/' + WORKFLOWS[1] + '@' + workflow_sha,
            'actual reusable workflow path differs')
    # A local reusable workflow executes from the caller's workflow commit.
    workflow_commit = api.get('/git/commits/' + workflow_sha)
    require(workflow_commit['sha'] == workflow_sha and workflow_commit['tree']['sha'] == source['tree'],
            'executed workflow tree differs from source tree')
    jobs_data = api.get('/actions/runs/' + ctx['runId'] + '/attempts/' + ctx['attempt'] + '/jobs?per_page=100')
    require(jobs_data['total_count'] == len(jobs_data['jobs']) and len(jobs_data['jobs']) <= 100,
            'exact-attempt jobs listing incomplete')
    jobs = {}
    for label in (COMPONENT_JOB, CHAIN_JOB):
        matches = [job for job in jobs_data['jobs'] if job.get('name') == label]
        require(len(matches) == 1, 'original Apple job identity missing/ambiguous')
        job = matches[0]
        require(job.get('head_sha') == ctx['candidate'] and job.get('run_attempt') == int(ctx['attempt']) and
                job.get('run_id') == int(ctx['runId']), 'original job run/attempt/head differs')
        if label == COMPONENT_JOB or completed:
            require(job.get('status') == 'completed' and job.get('conclusion') == 'success',
                    'original Apple job did not succeed')
        else:
            require(job.get('status') == 'in_progress' and job.get('conclusion') is None,
                    'producer seal must run in the active aggregate job')
        jobs[label] = job['id']
    return {'workflowId': run['workflow_id'], 'workflowPath': run['path'],
            'workflowSha': workflow_sha, 'workflowRef': references[0].get('ref'),
            'headBranch': run.get('head_branch'), 'jobs': jobs}


def validate_merge_proof(proof, target, tree):
    require(isinstance(proof, dict) and set(proof) == {
        'mergeCommit', 'parents', 'producerHead', 'tree', 'pullRequest', 'headBranch', 'merged'},
        'reference-only merged PR proof required')
    require(sha40(proof['mergeCommit']) == target and sha40(proof['tree']) == tree and
            isinstance(proof['parents'], list) and len(proof['parents']) in (1, 2),
            'merged PR target/tree/ancestry differs')
    parents = [sha40(parent) for parent in proof['parents']]
    head = sha40(proof['producerHead'])
    require(len(set(parents)) == len(parents) and target not in parents and
            (head == target or (len(parents) == 2 and head == parents[1])) and
            type(proof['pullRequest']) is int and proof['pullRequest'] > 0 and
            isinstance(proof['headBranch'], str) and bool(proof['headBranch']) and
            proof['merged'] is True, 'merged PR producer relationship differs')
    return proof


def merge_contract(api, target, source):
    commit = api.get('/git/commits/' + target)
    require(sha40(target) == source['candidate'] and commit.get('sha') == target and
            commit.get('tree', {}).get('sha') == source['tree'] and
            isinstance(commit.get('parents'), list) and len(commit['parents']) in (1, 2),
            'target is not an exact supported source commit')
    parents = [sha40(parent['sha']) for parent in commit['parents']]
    require(len(set(parents)) == len(parents) and target not in parents, 'target ancestry differs')
    prs = api.get('/commits/' + target + '/pulls?per_page=100')
    require(isinstance(prs, list) and len(prs) < 100, 'associated PR listing incomplete')
    # A normal fast-forward keeps the reviewed head itself. Its immediate
    # parent need not be the PR's old main base (the PR may contain many commits).
    # The other supported relationship remains the exact two-parent merge.
    matched = [pr for pr in prs if pr.get('merged_at') and pr.get('merge_commit_sha') == target and
               (pr.get('head', {}).get('sha') == target or
                (len(parents) == 2 and pr.get('head', {}).get('sha') == parents[1] and
                 pr.get('base', {}).get('sha') == parents[0])) and
               pr.get('base', {}).get('ref') == 'main' and
               pr.get('head', {}).get('repo', {}).get('id') == REPOSITORY_ID and
               pr.get('base', {}).get('repo', {}).get('id') == REPOSITORY_ID]
    require(len(matched) == 1, 'merged PR ancestry missing/ambiguous')
    selected = matched[0]
    require(type(selected.get('number')) is int and selected['number'] > 0, 'exact merged PR number required')
    producer_head = sha40(selected['head']['sha'])
    base = sha40(selected['base']['sha'])
    pr = api.get('/pulls/' + str(selected['number']))
    require(pr.get('merged') is True and pr.get('number') == selected['number'] and
            pr.get('merge_commit_sha') == target and pr.get('head', {}).get('sha') == producer_head and
            pr.get('base', {}).get('sha') == base and pr.get('base', {}).get('ref') == 'main' and
            pr.get('head', {}).get('repo', {}).get('id') == REPOSITORY_ID and
            pr.get('base', {}).get('repo', {}).get('id') == REPOSITORY_ID and
            isinstance(pr.get('head', {}).get('ref'), str) and pr['head']['ref'],
            'authoritative merged PR identity differs')
    head = commit if producer_head == target else api.get('/git/commits/' + producer_head)
    require(head.get('sha') == producer_head and head.get('tree', {}).get('sha') == source['tree'],
            'PR head and merge trees differ')
    # Human review and authorization remain in the existing merge process.
    # This receipt makes no machine claim about a person's review decision.
    return validate_merge_proof({'mergeCommit': target, 'parents': parents, 'producerHead': producer_head,
            'tree': source['tree'], 'pullRequest': pr['number'], 'headBranch': pr['head']['ref'],
            'merged': True}, target, source['tree'])


def find_producer(api, head):
    data = api.get('/actions/workflows/release-risk.yml/runs?event=pull_request&status=success&head_sha=' +
                   head + '&per_page=100')
    require(data['total_count'] == len(data['workflow_runs']) and len(data['workflow_runs']) <= 100,
            'producer run listing incomplete')
    rows = [run for run in data['workflow_runs'] if run.get('head_sha') == head and
            run.get('event') == 'pull_request' and run.get('status') == 'completed' and
            run.get('conclusion') == 'success' and run.get('path') == WORKFLOWS[0]]
    require(rows, 'successful exact-head PR producer unavailable')
    row = max(rows, key=lambda value: value['id'])
    require(type(row['id']) is int and type(row['run_attempt']) is int and row['run_attempt'] > 0,
            'producer run/attempt missing')
    return {'repository': REPOSITORY, 'candidate': head, 'runId': str(row['id']), 'attempt': str(row['run_attempt'])}


def validate_payload(package, manifest, transport):
    rows = zip_inventory(package)
    require(set(manifest['files']) == {'root', 'checkout'}, 'component payload namespaces differ')
    expected = {'component-handoff.json'}
    require(member_pin(package, 'component-handoff.json', MAX_RECEIPT) == transport['manifest'],
            'transport manifest bytes differ')
    for namespace, entries in manifest['files'].items():
        for name, wanted in entries.items():
            safe_member(name)
            member = namespace + '/' + name
            expected.add(member)
            require(member in rows and stat.S_IMODE(rows[member].external_attr >> 16) == wanted['mode'],
                    'payload member/mode differs')
            mode = rows[member].external_attr >> 16
            if wanted.get('directory') is True:
                require(stat.S_ISDIR(mode) and rows[member].file_size == 0, 'payload directory differs')
            elif 'link' in wanted:
                require(stat.S_ISLNK(mode) and isinstance(wanted['link'], str) and
                        not wanted['link'].startswith('/') and '..' not in PurePosixPath(wanted['link']).parts and
                        rows[member].file_size == len(wanted['link'].encode()) <= 4096 and
                        package.read(member) == wanted['link'].encode(), 'payload symlink differs')
            else:
                require(stat.S_ISREG(mode) and member_pin(package, member, wanted['bytes']) ==
                        {k: wanted[k] for k in ('bytes', 'sha256')}, 'payload file bytes differ')
    require(set(rows) == expected, 'component payload has missing/extra members')


def read_evidence(api, artifacts, ctx, source, work):
    """Authenticate all provider archives before interpreting their receipts."""
    expected = {
        'component': {'component-handoff.zip', 'component-transport.json'},
        'component-manifest': {'component-handoff.json', 'component-transport.json'},
        'component-original': {'apple-original-evidence.zip'},
        'carrier-summary-device-arm64': {'carrier-device-arm64.json', 'summary-app-carrier.json', 'summary-rust-receipt.json'},
        'carrier-original-device-arm64': {'apple-original-device-arm64-evidence.zip'},
        'chain': {'apple-chain.json'},
    }
    with contextlib.ExitStack() as stack:
        packages = {}
        for role in ARTIFACT_ROLES:
            package = stack.enter_context(zipfile.ZipFile(api.artifact(artifacts[role])))
            require(set(zip_inventory(package)) == expected[role], 'provider archive file set differs: ' + role)
            packages[role] = package
        folder = Path(work) / 'summaries'
        (folder / 'component').mkdir(parents=True)
        (folder / 'device-arm64').mkdir()
        for role, target in [('component-manifest', 'component'), ('carrier-summary-device-arm64', 'device-arm64')]:
            for name in expected[role]:
                require(packages[role].getinfo(name).file_size <= MAX_RECEIPT, 'summary exceeds bound')
                raw = packages[role].read(name)
                safe_json(raw)
                (folder / target / name).write_bytes(raw)
        manifest = read_receipt(folder / 'component/component-handoff.json')
        transport = read_receipt(folder / 'component/component-transport.json')
        require(packages['component'].getinfo('component-transport.json').file_size <= MAX_RECEIPT,
                'transport sidecar exceeds bound')
        require(packages['component'].read('component-transport.json') ==
                packages['component-manifest'].read('component-transport.json'), 'transport sidecars differ')
        from apple_handoff import validate_aggregate
        producer = {**source['producer'], 'candidate': ctx['candidate']}
        app = {**source['app'], 'candidate': ctx['candidate']}
        chain = validate_aggregate(folder, scope='device-only', expected_context=ctx,
                                   expected_producer=producer, expected_app=app,
                                   expected_source_contract=source['contract'])
        require(chain['schema'] == 'polaris-apple-unsigned-chain-v3' and
                json_member(packages['chain'], 'apple-chain.json') == chain, 'original final chain differs')
        require(member_pin(packages['component'], 'component-handoff.zip') == transport['archive'],
                'component transport archive differs')
        payload = stack.enter_context(nested_zip(packages['component'], 'component-handoff.zip'))
        validate_payload(payload, manifest, transport)
        component = stack.enter_context(nested_zip(packages['component-original'], 'apple-original-evidence.zip'))
        carrier = stack.enter_context(nested_zip(packages['carrier-original-device-arm64'], 'apple-original-device-arm64-evidence.zip'))
        component_index = check_original(component, ctx, None)
        carrier_index = check_original(carrier, ctx, 'device-arm64')
        # Original absolute paths remain data; no extraction into those locations.
        root = manifest['root']
        require(isinstance(root, str) and root.endswith('/' + ctx['runId'] + '-' + ctx['attempt']) and
                isinstance(manifest['checkout'], str) and manifest['checkout'].startswith('/'),
                'producer canonical paths differ')
        def original_pin(index, name):
            row = index['files'][name]
            require(row['originalPath'] == root + '/' + name, 'original path identity changed')
            return {k: row[k] for k in ('bytes', 'sha256')}
        for name, row in manifest['files']['root'].items():
            if name.startswith('component/Libbox.xcframework/') and 'sha256' in row:
                require(original_pin(component_index, name) == {k: row[k] for k in ('bytes', 'sha256')},
                        'original component bytes differ from transported component')
        component_receipt = fingerprint(json_member(component, 'component/component-receipt.json'))
        require(original_pin(component_index, 'component/component-receipt.json') == manifest['componentReceipt'] and
                component_receipt['fingerprint'] == manifest['componentFingerprint'] and
                component_receipt['input']['producer'] == producer and
                component_receipt['input']['sourceContract'] == source['contract'] and
                component_receipt.get('platform') == 'apple' and
                component_receipt.get('sourceOverlay') == 0 and
                component_receipt.get('AppAccepted') is False and component_receipt.get('devicePassed') is False,
                'original component producer/source differs')
        validate_native_closure(component, component_index, manifest, component_receipt)
        prepared = json_member(component, 'tools.json')
        require(prepared['App'] == producer and prepared.get('carrierScope') == 'device-only' and
                prepared.get('requiredSlots') == ['device-arm64'], 'original prepared tool/source scope differs')
        source_ref = source_summary(source)
        snapshots = {}
        snapshot_refs = {}
        for phase in PHASES:
            archive = carrier if phase.startswith('carrier-') else component
            name = ('device-arm64/' if phase.startswith('carrier-') else '') + 'execution-' + phase + '.json'
            snapshots[phase] = validate_snapshot(json_member(archive, name, MAX_JSON), phase, ctx, source_ref,
                                                 source['contract'], collect_unavailable=True)
            snapshot_refs[phase] = member_pin(archive, name, MAX_JSON)
        # Qualification is data here. Check every snapshot and every remaining
        # byte/identity edge before seal decides whether a sidecar may be emitted.
        supported = {phase: row for phase, row in snapshots.items() if row['schema'] == SNAPSHOT_SCHEMA}
        for stage in ('frontend', 'carrier'):
            if stage + '-before' in supported and stage + '-after' in supported:
                snapshot_pair(supported[stage + '-before'], supported[stage + '-after'])
        workflow_sha = snapshots['frontend-before']['workflowSha']
        require(all(row['workflowSha'] == workflow_sha for row in snapshots.values()),
                'workflow identity changed across stages')
        observations = list(supported.values())
        if observations:
            observed = observations[0]
            for row in observations[1:]:
                require(row['sdk'] == observed['sdk'] and row['overrides'] == observed['overrides'] and
                        row['cargoConfig'] == observed['cargoConfig'] and
                        {k: v for k, v in row['tools'].items() if k != 'protoc'} ==
                        {k: v for k, v in observed['tools'].items() if k != 'protoc'},
                        'actual tool/input contract drifted between producer and carrier')
        actual_tools = component_receipt['input']['tools']
        require(actual_tools['executables'] == prepared['tools'] == manifest['tools'], 'component tool binding differs')
        require(actual_tools['xcode'].strip() == 'Xcode ' + source['contract']['apple']['xcodeVersion'] +
                '\nBuild version ' + source['contract']['apple']['xcodeBuild'], 'native Xcode binding differs')
        for name in ('go', 'gomobile', 'gobind'):
            pin_shape(actual_tools['executables'][name])
        for sdk, key in [('iphoneos', 'iPhoneSDK'), ('iphonesimulator', 'simulatorSDK')]:
            sdk_tools = actual_tools['sdk'][sdk]
            require(sdk_tools['version'] == source['contract']['apple'][key], 'native SDK version differs')
            pin_shape(sdk_tools['settings'])
            pin_shape(sdk_tools['compiler'])
        for observed in observations:
            for name in ('go', 'gomobile', 'gobind'):
                require(actual_tools['executables'][name] == {k: observed['tools'][name][k] for k in ('bytes', 'sha256')},
                        'snapshot and native generator tools differ')
            require(actual_tools['xcode'].strip() == observed['tools']['xcodebuild']['version'], 'native Xcode binding differs')
            for sdk in ('iphoneos', 'iphonesimulator'):
                sdk_tools = actual_tools['sdk'][sdk]
                require(sdk_tools['settings'] == observed['sdk'][sdk]['settings'] and
                        sdk_tools['version'] == observed['sdk'][sdk]['version'] and
                        sdk_tools['compiler'] == {k: observed['tools']['clang'][k] for k in ('bytes', 'sha256')},
                        'native compiler/SDK byte binding differs')
        carrier_receipt = json_member(carrier, 'device-arm64/app-carrier.json')
        rust_receipt = json_member(carrier, 'device-arm64/rust-aarch64-apple-ios.json')
        summary = chain['carriers']['device-arm64']
        require(carrier_receipt == summary['receipt'] and
                original_pin(carrier_index, 'device-arm64/app-carrier.json') == summary['receiptPin'] and
                original_pin(carrier_index, 'device-arm64/rust-aarch64-apple-ios.json') ==
                {k: carrier_receipt['rustInput'][k] for k in ('bytes', 'sha256')}, 'carrier original/summary differs')
        require(carrier_receipt['rustInput']['receipt'] == root + '/device-arm64/rust-aarch64-apple-ios.json',
                'original Rust path relabelled')
        require(rust_receipt == read_receipt(folder / 'device-arm64/summary-rust-receipt.json') and
                rust_receipt['rustc'].strip() == prepared['rustc'].strip(), 'Rust version/receipt differs')
        rust_tools = rust_receipt['tools']
        require(set(rust_tools) == {'cargo', 'rustc'}, 'actual Rust tool inventory differs')
        for tool_row in rust_tools.values():
            require(set(tool_row) == {'path', 'bytes', 'sha256'} and isinstance(tool_row['path'], str) and
                    tool_row['path'].startswith('/') and tool_row['bytes'] > 0, 'actual Rust tool identity differs')
            pin_shape({key: tool_row[key] for key in ('bytes', 'sha256')})
        for observed in observations:
            require(rust_receipt['rustc'].strip() == observed['tools']['rustc']['version'], 'Rust version/receipt differs')
            for name in ('cargo', 'rustc'):
                require(rust_tools[name] == {k: observed['tools'][name][k] for k in ('path', 'bytes', 'sha256')},
                        'actual Rust tool bytes differ')
        require(rust_receipt['sdkVersion'] == source['contract']['apple']['iPhoneSDK'] and
                rust_receipt['cargoLock'] == file_pin(ROOT / 'Cargo.lock') and
                rust_receipt['iosConfig'] == file_pin(ROOT / 'src-tauri/tauri.ios.conf.json'),
                'Rust input contract differs')
        require(rust_receipt['command'] == [rust_tools['cargo']['path'], 'rustc', '--locked', '--release',
                '--timings', '--lib', '--package', 'polaris', '--target', 'aarch64-apple-ios', '--features',
                'tauri/custom-protocol', '--crate-type', 'staticlib'], 'Rust command contract differs')
        execution = rust_receipt['executionInputs']
        execution_supported = execution != {'unavailable': UNAVAILABLE_REASON}
        if execution_supported:
            require(isinstance(execution, dict) and set(execution) == {'overrides', 'cargoConfig'},
                    'actual Cargo execution input contract missing')
            validate_environment_pins(execution['overrides'])
            validate_cargo_configuration(execution['cargoConfig'])
            fixed_native = {prefix + '_' + target for prefix in ('CFLAGS', 'CXXFLAGS')
                            for target in ('aarch64_apple_ios', 'aarch64_apple_darwin', 'x86_64_apple_darwin')}
            fixed_native.add('OBJC_INCLUDE_PATH_aarch64_apple_ios')
            fixed = CARGO_FIXED_CHILD_ENV | fixed_native
            for observed in observations:
                require(execution['cargoConfig'] == observed['cargoConfig'], 'actual Cargo configuration drifted')
                require({k: v for k, v in execution['overrides'].items() if k not in fixed} ==
                        {k: v for k, v in observed['overrides'].items() if k not in fixed},
                        'actual Cargo environment differs from admitted carrier input')
            for key, value in {'RUSTC': rust_tools['rustc']['path'],
                               'CARGO_TARGET_DIR': root + '/cache/rust-target'}.items():
                require(execution['overrides'].get(key) == pin_bytes(value.encode()), 'actual Cargo fixed input differs')
        archive_name = 'cache/rust-target/aarch64-apple-ios/release/libpolaris_lib.a'
        require(original_pin(carrier_index, archive_name) == rust_receipt['archive'] and
                original_pin(carrier_index, 'device-arm64/original-linked-rust-archive.a') == rust_receipt['archive'],
                'original linked Rust bytes missing/different')
        ipa_name = 'device-arm64/polaris-box-' + source['contract']['version'] + '-app-link-probe.ipa'
        require(original_pin(carrier_index, ipa_name) == carrier_receipt['ipa'], 'original IPA bytes differ')
        link = fingerprint(json_member(carrier, 'device-arm64/apple-link-input.json'))
        require(original_pin(carrier_index, 'device-arm64/apple-link-input.json')['sha256'] ==
                carrier_receipt['linkInputSha256'] == rust_receipt['linkInputSha256'] and
                link['App'] == app and link['componentFingerprint'] == component_receipt['fingerprint'] and
                link['producerInput'] == component_receipt['input'] and link['resources'] == manifest['resources'],
                'actual final link/input/resource contract differs')
        eligible = len(supported) == len(PHASES) and execution_supported
        tool_contract = None
        if eligible:
            front, native = snapshots['frontend-before'], snapshots['carrier-before']
            tool_contract = sha(canonical({'frontend': front['tools'], 'carrier': native['tools'],
                'sdk': front['sdk'], 'overrides': front['overrides'],
                'cargoConfig': front['cargoConfig'], 'cargoExecution': execution}))
        return {'eligible': eligible, 'workflowSha': workflow_sha, 'source': source_ref,
                'snapshots': snapshot_refs, 'chain': member_pin(packages['chain'], 'apple-chain.json'),
                'summaries': {target + '/' + name: member_pin(packages[role], name, MAX_RECEIPT)
                    for role, target in [('component-manifest', 'component'), ('carrier-summary-device-arm64', 'device-arm64')]
                    for name in sorted(expected[role])},
                'componentReceipt': manifest['componentReceipt'], 'carrierReceipt': summary['receiptPin'],
                'rustReceipt': {k: carrier_receipt['rustInput'][k] for k in ('bytes', 'sha256')},
                'toolContract': tool_contract,
                'inputContract': sha(canonical({'source': source_ref, 'resources': manifest['resources'],
                                               'linkInput': carrier_receipt['linkInputSha256'],
                                               'componentInput': component_receipt['input']}))}


def validate_execution_contract(row, ctx, source, provider, evidence, artifacts):
    require(evidence['eligible'] is True, 'complete eligible execution evidence required')
    fingerprint(row)
    require(set(row) == {'schema', 'policy', 'context', 'repositoryId', 'scope', 'origin', 'provider',
                         'evidence', 'artifacts', 'fingerprint', *FALSE_CLAIMS}, 'execution contract fields differ')
    require(row['schema'] == CONTRACT_SCHEMA and row['policy'] == POLICY and row['context'] == ctx and
            row['repositoryId'] == REPOSITORY_ID and row['scope'] == 'device-only' and
            row['origin'] == 'full-native-build' and all(row[k] is False for k in FALSE_CLAIMS),
            'execution contract identity/scope differs')
    require(row['provider'] == provider and row['evidence'] == evidence and
            row['evidence']['source'] == source_summary(source) and
            row['artifacts'] == {role: artifact_reference(artifacts[role]) for role in ARTIFACT_ROLES},
            'execution contract workflow/tool/input/artifact binding differs')
    require(evidence['workflowSha'] == provider['workflowSha'], 'snapshot and provider workflow differ')
    require(len(canonical(row)) <= MAX_JSON, 'execution contract byte budget')
    return row


def seal(summaries, chain, output):
    require(os.environ.get('POLARIS_APPLE_SCOPE') == 'device-only' and
            os.environ.get('GITHUB_JOB') == 'unsigned-chain' and
            os.environ.get('GITHUB_EVENT_NAME') == 'pull_request' and
            re.fullmatch(re.escape(REPOSITORY + '/' + WORKFLOWS[0]) + r'@refs/pull/[1-9][0-9]*/merge',
                         os.environ.get('GITHUB_WORKFLOW_REF', '')),
            'execution seal requires the native Release Risk device-only PR aggregate')
    ctx, source = current_context(), source_bindings()
    with tempfile.TemporaryDirectory(prefix='apple-producer-seal-') as work:
        api = GitHub(os.environ.get('GITHUB_TOKEN'), work)
        provider = run_contract(api, ctx, source, completed=False)
        artifacts = artifact_rows(api, ctx)
        evidence = read_evidence(api, artifacts, ctx, source, work)
        require(file_pin(chain) == evidence['chain'], 'uploaded chain differs from just-built chain')
        for name, pin in evidence['summaries'].items():
            require(file_pin(Path(summaries) / name) == pin, 'local aggregate inputs changed')
        require(evidence['workflowSha'] == provider['workflowSha'], 'snapshot and provider workflow differ')
        # Only now, after provider, ALL archived evidence and current local
        # aggregate inputs are validated, may qualification decline harmlessly.
        if not evidence['eligible']:
            raise UnsupportedCargoInputs('original Cargo inputs do not qualify for reuse')
        row = signed({'schema': CONTRACT_SCHEMA, 'policy': POLICY, 'context': ctx,
                      'repositoryId': REPOSITORY_ID, 'scope': 'device-only', 'origin': 'full-native-build',
                      'provider': provider, 'evidence': evidence,
                      'artifacts': {role: artifact_reference(artifacts[role]) for role in ARTIFACT_ROLES}, **FALSE_CLAIMS})
        validate_execution_contract(row, ctx, source, provider, evidence, artifacts)
        write_new(output, row)


def eligible_environment():
    return (os.environ.get('POLARIS_APPLE_ALLOW_MERGE_REUSE') == 'true' and
            os.environ.get('GITHUB_EVENT_NAME') == 'push' and os.environ.get('GITHUB_REF') == 'refs/heads/main' and
            os.environ.get('POLARIS_APPLE_SCOPE') == 'device-only' and
            os.environ.get('POLARIS_APP_CANDIDATE') == os.environ.get('GITHUB_SHA'))


def resolve_evidence(api, ctx, source, work):
    proof = merge_contract(api, ctx['candidate'], source)
    producer_ctx = find_producer(api, proof['producerHead'])
    provider = run_contract(api, producer_ctx, source, completed=True)
    require(provider['workflowRef'] == 'refs/pull/' + str(proof['pullRequest']) + '/merge' and
            provider['headBranch'] == proof['headBranch'], 'original run does not belong to the merged PR')
    artifacts = artifact_rows(api, producer_ctx, contract=True)
    # Reject old/missing sidecars before downloading large original archives.
    with zipfile.ZipFile(api.artifact(artifacts['execution-contract'])) as package:
        require(set(zip_inventory(package)) == {'apple-execution-contract.json'}, 'sidecar archive differs')
        contract_pin = member_pin(package, 'apple-execution-contract.json', MAX_JSON)
        contract = json_member(package, 'apple-execution-contract.json', MAX_JSON)
    fingerprint(contract)
    require(contract.get('schema') == CONTRACT_SCHEMA and contract.get('context') == producer_ctx and
            contract.get('origin') == 'full-native-build', 'old/reused/incomplete producer contract')
    evidence = read_evidence(api, artifacts, producer_ctx, source, work)
    validate_execution_contract(contract, producer_ctx, source, provider, evidence, artifacts)
    return signed({'schema': RESULT_SCHEMA, 'policy': POLICY, 'context': ctx,
                   'repositoryId': REPOSITORY_ID, 'scope': 'device-only', 'result': 'verified-reuse',
                   'proof': proof, 'producer': producer_ctx, 'provider': provider,
                   'source': source_summary(source), 'executionContract': contract_pin,
                   'artifacts': {role: artifact_reference(row) for role, row in artifacts.items()},
                   **FALSE_CLAIMS})


def output_fields(**fields):
    path = os.environ.get('GITHUB_OUTPUT')
    require(path, 'GitHub output path required')
    # Validate every field before publishing any success output. The receipt's
    # literal sha256 key is part of the workflow contract, including its digits.
    require(all(name in {'reused', 'receipt_sha256', 'eligible', 'reason'} and
                '\n' not in str(value) and '\r' not in str(value) for name, value in fields.items()),
            'unsafe workflow output')
    with Path(path).open('a') as stream:
        for name, value in fields.items():
            stream.write(name + '=' + str(value) + '\n')


def resolve(output):
    # Explicit default first; partial work must never publish reused=true.
    if not eligible_environment():
        output_fields(reused='false', reason='ineligible-event-or-scope')
        return
    try:
        ctx, source = current_context(), source_bindings()
        with tempfile.TemporaryDirectory(prefix='apple-merge-reuse-') as work:
            row = resolve_evidence(GitHub(os.environ.get('GITHUB_TOKEN'), work), ctx, source, work)
            pin = write_new(output, row)
        output_fields(reused='true', receipt_sha256=pin['sha256'], reason='verified-exact-tree-producer')
    except (RuntimeError, ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError,
            zipfile.BadZipFile, RecursionError):
        # Do not expose provider URLs, token-bearing redirects, untrusted text or
        # log false success. Detailed downloaded evidence never escapes temp.
        output_fields(reused='false', reason='evidence-unavailable-or-incompatible')
        print('Apple evidence is unavailable or incompatible; the full native build is required.', flush=True)


def check_result(path, expected_sha):
    require(re.fullmatch('[0-9a-f]{64}', expected_sha or ''), 'resolver receipt hash required')
    require(eligible_environment(), 'reuse result context is ineligible')
    require(file_pin(path)['sha256'] == expected_sha and Path(path).stat().st_size <= MAX_JSON,
            'same-run resolver receipt bytes differ')
    row = fingerprint(safe_json(Path(path).read_bytes(), MAX_JSON))
    require(set(row) == {'schema', 'policy', 'context', 'repositoryId', 'scope', 'result', 'proof',
                         'producer', 'provider', 'source', 'executionContract', 'artifacts',
                         'fingerprint', *FALSE_CLAIMS}, 'reference-only result fields differ')
    require(row['schema'] == RESULT_SCHEMA and row['policy'] == POLICY and row['result'] == 'verified-reuse' and
            row['context'] == current_context() and row['repositoryId'] == REPOSITORY_ID and
            row['scope'] == 'device-only' and all(row[k] is False for k in FALSE_CLAIMS),
            'same-run resolver receipt context differs')
    source = source_bindings()
    validate_merge_proof(row['proof'], source['candidate'], source['tree'])
    require(row['source'] == source_summary(source) and
            row['proof']['producerHead'] == row['producer']['candidate'] and row['proof']['merged'] is True and
            row['provider']['workflowRef'] == 'refs/pull/' + str(row['proof']['pullRequest']) + '/merge' and
            row['provider']['headBranch'] == row['proof']['headBranch'] and
            row['producer']['repository'] == REPOSITORY, 'resolver tree/source/merged-PR binding differs')
    pin_shape(row['executionContract'])
    require(set(row['artifacts']) == {*ARTIFACT_ROLES, 'execution-contract'}, 'resolver artifact references incomplete')
    for role, reference in row['artifacts'].items():
        require(set(reference) == {'id', 'name', 'size_in_bytes', 'digest'} and
                type(reference['id']) is int and reference['id'] > 0 and
                reference['name'] == artifact_name(role, row['producer']) and
                re.fullmatch('sha256:[0-9a-f]{64}', reference['digest']), 'resolver artifact reference differs')
    print('Verified prior unsigned device-arm64 App + PacketTunnel evidence for this exact-tree merge. '
          'No fresh native build, signing, device execution or publication acceptance.', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    snap = commands.add_parser('snapshot')
    snap.add_argument('--phase', choices=PHASES, required=True)
    snap.add_argument('--slot', choices=['device-arm64'])
    sealing = commands.add_parser('seal')
    sealing.add_argument('--summaries', type=Path, required=True)
    sealing.add_argument('--chain', type=Path, required=True)
    sealing.add_argument('--output', type=Path, required=True)
    resolving = commands.add_parser('resolve')
    resolving.add_argument('--output', type=Path, required=True)
    checking = commands.add_parser('check-result')
    checking.add_argument('--receipt', type=Path, required=True)
    checking.add_argument('--expected-sha', required=True)
    args = parser.parse_args()
    if args.action == 'snapshot':
        snapshot(args.phase, args.slot)
    elif args.action == 'seal':
        try:
            seal(args.summaries, args.chain, args.output)
        except UnsupportedCargoInputs:
            require(not args.output.exists(), 'unavailable inputs must not produce a reuse contract')
            output_fields(eligible='false', reason=UNAVAILABLE_REASON)
            print('Full build retained; Cargo inputs do not qualify for reuse.', flush=True)
        else:
            output_fields(eligible='true', reason='complete-execution-contract')
    elif args.action == 'resolve':
        resolve(args.output)
    else:
        check_result(args.receipt, args.expected_sha)


if __name__ == '__main__':
    main()
