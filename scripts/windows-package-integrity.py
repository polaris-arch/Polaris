#!/usr/bin/env python3
"""Windows release production and static carrier integrity. Never runs a product PE.

The normal prepare/build chain owns identity. This gate freezes its exact resource
inputs before bundling and compares every final member with those same bytes.
"""
import argparse
import codecs
import ctypes
from contextlib import ExitStack, contextmanager
import hashlib
import hmac
import importlib.util
import json
import ntpath
import os
from pathlib import Path, PureWindowsPath
import re
import queue
import shutil
import stat
import subprocess
import tempfile
import threading
import time
import zipfile

os.sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location('final_package', Path(__file__).with_name('final-package-receipt.py'))
final = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(final)
require = final.require
ROOT = Path(__file__).resolve().parents[1]
MAX_FILE = 512 * 1024 ** 2
MAX_TOTAL = 2 * 1024 ** 3
MAX_MEMBERS = 10000
MAX_LISTING = 8 * 1024 ** 2
MAX_STDERR = 256 * 1024
MAX_CHILD_DIAGNOSTIC = 256 * 1024
MAX_UNINSTALLER = 64 * 1024 ** 2
EXTRACT_TIMEOUT = 180
PUBLIC_LEGAL_FILES = ('LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md')
MARKER = ("Polaris portable edition marker - DO NOT DELETE.\n"
          "Polaris 便携版标记文件，请勿删除。\n\n"
          "This file tells Polaris it is running as the portable (zip) edition, so that\n"
          "'Check for updates' offers the portable zip instead of the NSIS installer.\n"
          "Deleting it makes Polaris think it was installed, and it will then offer an\n"
          "installer that creates a SECOND, separate copy of the app.\n").encode()


def io_path(path):
    """Use extended Windows names at I/O boundaries, never in logical maps."""
    path = Path(path)
    if os.name != 'nt':
        return path
    name = str(path)
    if name.startswith('\\\\?\\'):
        require(re.match(r'^\\\\\?\\(?:[A-Za-z]:\\|UNC\\[^\\]+\\[^\\]+(?:\\|$))', name, re.I),
                'unsupported Windows filesystem namespace')
        return Path(name)
    name = os.path.abspath(path)
    require(not name.startswith('\\\\.\\'), 'unsupported Windows filesystem namespace')
    if name.startswith('\\\\'):
        require(Path(name).is_absolute(), 'absolute UNC share required')
        return Path('\\\\?\\UNC\\' + name[2:])
    require(re.match(r'^[A-Za-z]:\\', name), 'absolute Windows drive required')
    return Path('\\\\?\\' + name)


def regular(path):
    path = Path(path)
    # Check ancestors as well: a regular leaf beneath a junction is an alias.
    for ancestor in [path, *path.parents]:
        s = io_path(ancestor).lstat()
        require(not stat.S_ISLNK(s.st_mode) and not (getattr(s, 'st_file_attributes', 0) & 0x400),
                'link/reparse object: ' + str(ancestor))
    s = io_path(path).stat()
    require(stat.S_ISREG(s.st_mode) and s.st_nlink == 1 and s.st_size <= MAX_FILE,
            'regular bounded single-link member required: ' + str(path))
    return path


def facts(path):
    path = regular(path); before = io_path(path).stat(); digest = hashlib.sha256()
    with io_path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    after = io_path(path).stat()
    require((before.st_size, before.st_mtime_ns, before.st_ino) ==
            (after.st_size, after.st_mtime_ns, after.st_ino), 'member changed while hashing')
    return {'bytes': before.st_size, 'sha256': digest.hexdigest()}


def byte_facts(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def key(name, directory=False):
    return final.windows_zip_key(name, directory)


def validate_names(rows):
    require(len(rows) <= MAX_MEMBERS, 'member count budget exceeded')
    seen = {}; ancestors = set()
    for name, directory in rows:
        normalized = key(name, directory)
        require(normalized not in seen, 'duplicate/alias member: ' + name)
        seen[normalized] = directory
        ancestors.update('/'.join(normalized.split('/')[:i]) for i in range(1, len(normalized.split('/'))))
    require(all(seen.get(parent, True) for parent in ancestors), 'file/directory alias collision')


def tree(directory):
    directory = Path(directory)
    for path in [directory, *directory.parents]:
        info = io_path(path).lstat()
        require(not stat.S_ISLNK(info.st_mode) and not (getattr(info, 'st_file_attributes', 0) & 0x400), 'directory link/reparse alias')
    result = {}
    native_directory = io_path(directory)
    for parent, directories, files in os.walk(native_directory, followlinks=False):
        for name in directories:
            path = Path(parent) / name
            s = path.lstat()
            require(not stat.S_ISLNK(s.st_mode) and not (getattr(s, 'st_file_attributes', 0) & 0x400), 'directory alias')
        for name in files:
            path = Path(parent) / name
            result[path.relative_to(native_directory).as_posix()] = facts(path)
    validate_names([(name, False) for name in result])
    require(len(result) <= MAX_MEMBERS and sum(row['bytes'] for row in result.values()) <= MAX_TOTAL,
            'payload budget exceeded')
    return result


def require_legal_resources(entries):
    require(isinstance(entries, list), 'resource declaration list required')
    for name in PUBLIC_LEGAL_FILES:
        require(entries.count('../' + name) == 1, 'required legal resource must occur once: ' + name)


def source_inventory(root):
    conf = json.loads(io_path(regular(root / 'src-tauri/tauri.windows.conf.json')).read_bytes())
    require_legal_resources(conf['bundle']['resources'])
    result = {}; sources = {}
    for entry in conf['bundle']['resources']:
        require(isinstance(entry, str) and not any(c in entry for c in '*?'), 'explicit resources required')
        source = root / 'src-tauri' / entry
        # Only the reviewed resource declaration paths, never arbitrary traversal.
        require(entry == 'core-manifest.json' or entry.startswith('../') and not entry[3:].startswith('../'),
                'unreviewed resource mapping')
        source = Path(os.path.abspath(source))
        require(source.is_relative_to(root), 'resource escaped repository')
        target = entry.replace('../', '_up_/', 1).rstrip('/')
        members = tree(source) if io_path(source).is_dir() else {'': facts(source)}
        for relative, value in members.items():
            name = target + ('/' + relative if relative else '')
            require(name not in result, 'resource declaration collision')
            result[name] = value; sources[name] = str(source / relative)
    validate_names([(name, False) for name in result])
    for name in ['core-manifest.json', '_up_/LICENSE', '_up_/NOTICE', '_up_/THIRD-PARTY-LICENSES.md',
                 '_up_/resources/win/core-manifest.json', '_up_/resources/win/polaris-helper.exe',
                 '_up_/resources/win/polaris-cleaner.exe', '_up_/resources/win/sing-box.exe',
                 '_up_/resources/win/libcronet.dll']:
        require(name in result and result[name]['bytes'] > 0, 'required source missing: ' + name)
    require(sum(row['bytes'] for row in result.values()) <= MAX_TOTAL, 'source inventory total budget exceeded')
    return result, sources


def verified_sources(root):
    subprocess.run(['node', str(root / 'scripts/prepare-core-identity.mjs'), '--platform=win', '--mode=verify'],
                   cwd=root, check=True, timeout=180)
    return source_inventory(root)


def find_sevenzip(*, timeout=30):
    executable = shutil.which('7zz') or shutil.which('7z')
    if not executable:
        candidate = Path(os.environ.get('ProgramFiles', 'C:/Program Files')) / '7-Zip/7z.exe'
        if io_path(candidate).is_file(): executable = str(candidate)
    require(executable, '7-Zip is required for static final NSIS inspection')
    info = bounded_command([executable, 'i', '-sccUTF-8'], 1024 * 1024, timeout=timeout).decode('utf-8-sig')
    version = re.search(r'7-Zip(?: \([^\n]*\))? ([0-9]+\.[0-9]+)', info)
    require(version and int(version[1].split('.')[0]) >= 24 and re.search(r'\bNsis\b', info, re.I),
            '7-Zip >=24 with the NSIS reader required')
    return str(executable), version[1]


def unique_installer(directory, version, product='Polaris'):
    candidates = [directory / path.name for path in io_path(directory).glob('*-setup.exe')]
    expected = f'{product}_{version}_x64-setup.exe'
    require(len(candidates) == 1 and candidates[0].name == expected,
            'expected unique current raw NSIS candidate: ' + expected)
    regular(candidates[0]); return candidates[0]


def cargo_directory(path):
    path = Path(path)
    require(path.is_absolute() and '..' not in path.parts, 'absolute canonical Cargo directory required')
    for part in path.parts[1:]: validate_names([(part, False)])
    for parent in [path, *path.parents]:
        try: value = io_path(parent).lstat()
        except FileNotFoundError: continue
        require(stat.S_ISDIR(value.st_mode) and not (getattr(value, 'st_file_attributes', 0) & 0x400),
                'link/reparse Cargo directory: ' + str(parent))
    return path


def directory_id(path):
    value = io_path(cargo_directory(path)).stat()
    return [value.st_dev, value.st_ino]


def cargo_output(argv, root, *, timeout=30, diagnostic_context=None, env=None, spool=None, transport=None):
    output = bytearray()
    value = os.environ.get('TAURI_CONFIG')
    config = None if value is None else value.encode('utf-8', errors='surrogatepass')
    context = {'firstActualChildObserved': False, **(diagnostic_context or {}),
               'TAURI_CONFIG': {'present': value is not None, 'empty': value == '',
                                'bytes': len(config) if config is not None else None,
                                'sha256': hashlib.sha256(config).hexdigest() if config is not None else None}}
    def consume(block):
        output.extend(block)
        if spool is not None:
            spool.write(block); os.sys.stdout.buffer.write(block); os.sys.stdout.buffer.flush()
    observed = stream_child(argv, consume, MAX_LISTING, timeout=timeout, cwd=root, env=env,
                            diagnostic_context=context, cargo_json=True)
    if transport is not None: transport.update(observed)
    return output.decode('utf-8', errors='strict')


def cargo_tool(path):
    # Rustup's normal Cargo proxy may have several links. This build-tool
    # identity does not relax the produced App's sole-known-alias rule.
    path = Path(path).absolute()
    before = app_identity(io_path(path).stat())
    require(stat.S_ISREG(before[2]) and 0 < before[4] <= MAX_FILE, 'regular bounded Cargo executable required')
    with io_path(path).open('rb') as stream:
        held = app_identity(os.fstat(stream.fileno()))
        require(app_object_identity(held) == app_object_identity(before), 'Cargo executable replaced before open')
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        require(app_identity(os.fstat(stream.fileno())) == held, 'Cargo executable changed while reading')
    require(app_identity(io_path(path).stat()) == before, 'Cargo executable changed')
    return {'path': str(path), 'identity': list(before), 'sha256': digest}


def cargo_inputs(root, manifest):
    inputs = {str(p): facts(p) for p in [root / 'Cargo.toml', root / 'Cargo.lock', manifest]}
    for parent in [root / 'src-tauri', root, *root.parents, Path(os.environ.get('CARGO_HOME') or Path.home() / '.cargo')]:
        for name in ('config', 'config.toml'):
            config = parent / name if parent == Path(os.environ.get('CARGO_HOME') or Path.home() / '.cargo') else parent / '.cargo' / name
            if io_path(config).exists() or io_path(config).is_symlink(): inputs[str(config)] = facts(config)
    return inputs


def cargo_environment():
    return {k: v for k, v in os.environ.items() if k.startswith(
            ('CARGO_PROFILE_', 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_')) or k in
            ('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN',
             'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'TAURI_CONFIG')}


def cargo_context(root, target=None, *, frozen=None):
    """Resolve Cargo's actual workspace/package/output, never guess an App path."""
    executable = os.environ.get('CARGO') or shutil.which('cargo')
    require(executable, 'normal Cargo executable missing')
    tool = cargo_tool(executable)
    if frozen is not None:
        # Readback reuses the completed producer's metadata, not a new Cargo
        # invocation or a compiler attestation. Every input to it stays bound.
        require(target == frozen['target'], 'foreign readback target')
        diff = subprocess.check_output(['git', '-C', str(root), 'diff', '--binary', 'HEAD', '--'], timeout=30)
        return {**frozen, 'tool': tool, 'inputs': cargo_inputs(root, root / 'src-tauri/Cargo.toml'),
                'environment': cargo_environment(),
                'sourceDiffSha256': hashlib.sha256(diff.encode() if isinstance(diff, str) else diff).hexdigest()}
    version = cargo_output([tool['path'], '-vV'], root)
    require(version.startswith('cargo ') and '\nhost: x86_64-pc-windows-msvc\n' in version,
            'native x64 Windows Cargo required')
    target = target or os.environ.get('CARGO_BUILD_TARGET')
    require(target in (None, 'x86_64-pc-windows-msvc'), 'only the actual native Windows target is supported')
    manifest = regular(root / 'src-tauri/Cargo.toml')
    metadata = json.loads(cargo_output([tool['path'], 'metadata', '--no-deps', '--format-version=1',
                                      '--locked', '--offline', '--manifest-path', str(manifest)], root / 'src-tauri'))
    packages = [p for p in metadata['packages'] if Path(p['manifest_path']) == manifest]
    require(Path(metadata['workspace_root']) == root and len(packages) == 1, 'foreign Cargo workspace/manifest')
    package = packages[0]
    require(package['id'] in metadata['workspace_members'] and package['name'] == 'polaris', 'foreign Cargo package')
    require(package['version'] == json.loads(io_path(regular(root / 'src-tauri/tauri.conf.json')).read_bytes())['version'],
            'Cargo/App version differs')
    binaries = [t for t in package['targets'] if t['name'] == 'polaris' and t['kind'] == ['bin']]
    require(len(binaries) == 1 and Path(binaries[0]['src_path']) == regular(root / 'src-tauri/src/main.rs'),
            'foreign Cargo binary target')
    directory = cargo_directory(metadata['target_directory'])
    require(directory != root and not root.is_relative_to(directory), 'Cargo target overlaps source root')
    # Check the configured spelling before a normalized metadata path could
    # conceal a junction. Ordinary relative CARGO_TARGET_DIR remains supported.
    configured = os.environ.get('CARGO_TARGET_DIR')
    if configured:
        raw = Path(configured)
        raw = raw if raw.is_absolute() else root / 'src-tauri' / raw
        for parent in [raw, *raw.parents]:
            try: value = io_path(parent).lstat()
            except FileNotFoundError: continue
            require(not stat.S_ISLNK(value.st_mode) and not (getattr(value, 'st_file_attributes', 0) & 0x400),
                    'link/reparse configured Cargo target')
    release = directory / target / 'release' if target else directory / 'release'
    inputs = cargo_inputs(root, manifest)
    diff = subprocess.check_output(['git', '-C', str(root), 'diff', '--binary', 'HEAD', '--'], timeout=30)
    source_diff = hashlib.sha256(diff.encode() if isinstance(diff, str) else diff).hexdigest()
    # Freeze the only supported native target's inputs too. Iterating existing
    # keys preserves absent versus present-empty, including TAURI_CONFIG.
    environment = cargo_environment()
    require(cargo_tool(tool['path']) == tool, 'Cargo tool changed during metadata')
    return {'tool': tool, 'version': version, 'packageId': package['id'], 'manifest': str(manifest),
            'binarySource': binaries[0]['src_path'], 'inputs': inputs, 'sourceDiffSha256': source_diff, 'environment': environment,
            'targetDirectory': str(directory), 'target': target, 'releaseDirectory': str(release), 'profile': 'release'}


def prepare(root):
    tool, version = find_sevenzip()
    inventory, sources = verified_sources(root)
    context = cargo_context(root, os.environ.get('POLARIS_WINDOWS_PACKAGE_TARGET'))
    release = cargo_directory(context['releaseDirectory']); io_path(release).mkdir(parents=True, exist_ok=True)
    require(directory_id(release)[0] == directory_id(root)[0], 'Cargo release/publication must share a filesystem for atomic retention')
    # Never delete somebody else's output. Quarantine only these fixed owned
    # workflow output namespaces, under a fresh same-filesystem temporary owner.
    owner = release / Path(tempfile.mkdtemp(prefix='.polaris-package-', dir=io_path(release))).name
    state = {'schema': 'polaris-windows-package-input-v2', 'owner': str(owner), 'root': str(root), 'cargo': context,
             'targetIdentity': directory_id(context['targetDirectory']), 'releaseIdentity': directory_id(release),
             'ownerIdentity': directory_id(owner),
             'sevenzip': tool, 'sevenzipVersion': version, 'resources': inventory, 'sources': sources,
             'version': json.loads(io_path(root / 'src-tauri/tauri.conf.json').read_bytes())['version'],
             'candidate': subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()}
    retained_bytes = 0
    for name, path in [('nsis', release / 'bundle/nsis'), ('dist-win', root / 'dist-win'),
                       ('evidence', root / 'windows-package-evidence')]:
        if io_path(path).exists() or io_path(path).is_symlink():
            retained = tree(path)  # reject links and bound retention before any move
            retained_bytes += sum(row['bytes'] for row in retained.values())
            require(retained_bytes <= MAX_TOTAL, 'combined old output retention budget exceeded')
            io_path(path).rename(io_path(owner / ('previous-' + name)))
        io_path(path).mkdir(parents=True)
    state['retainedPreviousOutputBytes'] = retained_bytes
    state_path = owner / 'state.json'
    io_path(state_path).write_text(json.dumps(state, indent=2) + '\n', encoding='utf-8')
    if os.environ.get('GITHUB_ENV'):
        with open(io_path(os.environ['GITHUB_ENV']), 'a', encoding='utf-8') as env:
            env.write('POLARIS_WINDOWS_PACKAGE_STATE=' + str(state_path) + '\n')
    print('POLARIS_WINDOWS_PACKAGE_STATE=' + str(state_path))
    print('static NSIS reader:', tool, version, '; fresh output namespaces; frozen resources:', len(inventory))


def load_state(root, *, readback=False):
    path = regular(os.environ['POLARIS_WINDOWS_PACKAGE_STATE'])
    state = json.loads(io_path(path).read_bytes())
    require(isinstance(state, dict) and state.get('schema') == 'polaris-windows-package-input-v2'
            and isinstance(state.get('cargo'), dict), 'foreign package state')
    context = cargo_context(root, state['cargo']['target'], frozen=state['cargo'] if readback else None)
    release = cargo_directory(context['releaseDirectory'])
    require(state['schema'] == 'polaris-windows-package-input-v2' and state['root'] == str(root)
            and path.parent == Path(state['owner']) and path.parent.parent == release
            and path.parent.name.startswith('.polaris-package-'), 'foreign package state')
    require(state['cargo'] == context, 'Cargo context changed')
    target_id = directory_id(context['targetDirectory'])
    release_id = directory_id(release)
    owner_id = directory_id(path.parent)
    require(target_id == state['targetIdentity'] and release_id == state['releaseIdentity'] and owner_id == state['ownerIdentity'],
            'Cargo context or owned namespace changed')
    current, sources = verified_sources(root)
    require(current == state['resources'] and sources == state['sources'], 'source resources changed after pre-build freeze')
    head = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
    require(head == state['candidate'], 'candidate changed during bundling')
    require(json.loads(io_path(root / 'src-tauri/tauri.conf.json').read_bytes())['version'] == state['version'], 'App version changed')
    return state


def carrier_inventory(state, app, kind):
    result = {(name[5:] if kind == 'zip' and name.startswith('_up_/') else name): row
              for name, row in state['resources'].items()}
    result['polaris.exe'] = byte_facts(final.expected_windows_app(app, kind, final.TAURI_CLI_VERSION))
    if kind == 'zip': result['portable.marker'] = byte_facts(MARKER)
    # The pinned CLI 2.12.1 sets its install destination through $R6. 7-Zip
    # preserves that variable root; only ZIP removes the existing _up_ prefix.
    if kind == 'nsis': result = {'$R6/' + name: row for name, row in result.items()}
    validate_names([(name, False) for name in result])
    return result


def exact(actual, expected):
    require(actual.keys() == expected.keys(), 'final member set differs; missing=' + str(sorted(expected.keys() - actual.keys()))
            + '; extra=' + str(sorted(actual.keys() - expected.keys())))
    for name in expected:
        require(actual[name] == expected[name], 'final member SHA/size differs: ' + name)


def zip_inventory(archive, expected):
    with zipfile.ZipFile(io_path(regular(archive))) as stream:
        members = stream.infolist()
        validate_names([(m.filename, m.is_dir()) for m in members])
        require(sum(m.file_size for m in members) <= MAX_TOTAL, 'ZIP expanded budget exceeded')
        directories = {'/'.join(name.split('/')[:i]) + '/' for name in expected for i in range(1, len(name.split('/')))}
        actual = {}
        for member in members:
            require(member.filename == member.orig_filename and '\0' not in member.orig_filename, 'ZIP NUL name')
            mode = member.external_attr >> 16
            require(not stat.S_ISLNK(mode) and (stat.S_IFMT(mode) in [0, stat.S_IFREG, stat.S_IFDIR]), 'ZIP special file')
            if member.is_dir():
                require(member.filename in directories and member.file_size == 0, 'unregistered ZIP directory')
                continue
            require(member.filename in expected and member.file_size <= MAX_FILE, 'unregistered/oversized ZIP member: ' + member.filename)
            digest = hashlib.sha256(); size = 0
            with stream.open(member) as content:
                for block in iter(lambda: content.read(1024 * 1024), b''):
                    size += len(block); require(size <= member.file_size, 'ZIP expanded size differs'); digest.update(block)
            actual[member.filename] = {'bytes': size, 'sha256': digest.hexdigest()}
        exact(actual, expected)
        return actual


def nsis_listing(text):
    require('Type = Nsis' in text and '----------' in text, 'actual artifact is not readable NSIS')
    records = []
    for block in text.split('----------', 1)[1].strip().split('\n\n'):
        row = {}
        for line in block.splitlines():
            if ' = ' in line:
                name, value = line.split(' = ', 1)
                require(name not in row, 'duplicate 7-Zip property'); row[name] = value
        if not row: continue
        require('Path' in row and 'Symbolic Link' not in row and 'Hard Link' not in row,
                'invalid NSIS member record')
        # 7-Zip on Windows prints native separators. No other alias is normalized.
        name = row['Path'].replace('\\', '/')
        require(row.get('Folder', '-') == '-' and not row.get('Attributes', '').startswith('D'), 'unexpected NSIS directory record')
        require(not re.search(r'\b[lrwx-]{10}\b', row.get('Attributes', '')), 'unexpected POSIX NSIS file type')
        size = row.get('Size')
        require(not size or size.isdecimal() and int(size) <= MAX_FILE, 'oversized NSIS member')
        records.append((name, int(size) if size else None))
    validate_names([(name, False) for name, _ in records])
    require(records, 'empty NSIS member listing')
    return records


def stream_child(argv, consume_stdout, stdout_limit, *, timeout, stderr_limit=MAX_STDERR, cwd=None,
                 diagnostic_context=None, cargo_json=False, env=None, output_guard=None):
    """Bounded pipe transport. Diagnostics never authorize an artifact or descendants.

    Preserve the execution deadline and exact-Popen cleanup. In particular the
    final kill/wait has no timeout: this is not a total cleanup wall-clock bound.
    """
    require(isinstance(stdout_limit, int) and stdout_limit >= 0 and timeout > 0,
            'invalid child output/deadline budget')
    started = time.monotonic(); started_wall = time.time(); deadline = started + timeout
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, bufsize=0, cwd=cwd, env=env)
    messages = queue.Queue(maxsize=4); stop = threading.Event(); stats_lock = threading.Lock()
    stderr = bytearray(); stdout_bytes = 0; delivered_hash = hashlib.sha256(); ended = set(); cause = None
    stats = {label: {'bytes': 0, 'hash': hashlib.sha256(), 'eof': False, 'lastReadElapsed': None}
             for label in ('stdout', 'stderr')}
    pending = bytearray(); line_bytes = 0; oversized = False
    events = {'counts': {}, 'lastEvents': [], 'invalidJsonLines': 0, 'oversizedLines': 0,
              'freshTrue': 0, 'freshFalse': 0, 'buildFinishedSeen': 0, 'lastBuildSuccess': None,
              'lastEventElapsed': None}
    decoder = codecs.getincrementaldecoder('utf-8')('strict') if cargo_json else None
    cleanup = {'terminate': None, 'kill': None, 'waits': [], 'readers': [], 'pipes': [], 'errors': []}
    def brief(error): return {'type': type(error).__name__, 'message': str(error)[:1024]}
    def wait_primary(seconds, phase):
        attempt = {'phase': phase, 'timeoutSeconds': seconds, 'startedElapsed': time.monotonic()-started}
        cleanup['waits'].append(attempt); begin = time.monotonic()
        try:
            code = process.wait(timeout=seconds) if seconds is not None else process.wait()
        except BaseException as error:
            attempt.update(returned=False, elapsedSeconds=time.monotonic()-begin, error=brief(error)); raise
        attempt.update(returned=True, code=code, elapsedSeconds=time.monotonic()-begin)
        return code
    def cargo_events(block):
        nonlocal line_bytes, oversized
        pieces = block.split(b'\n')
        for index, part in enumerate(pieces):
            line_bytes += len(part)
            if not oversized and len(pending) + len(part) <= 64 * 1024: pending.extend(part)
            else: oversized = True; pending.clear()
            if index == len(pieces) - 1: continue
            if oversized: events['oversizedLines'] += 1
            elif pending.strip():
                try: row = json.loads(pending)
                except (ValueError, UnicodeError): events['invalidJsonLines'] += 1
                else:
                    reason = row.get('reason') if isinstance(row, dict) else None
                    if reason in ('compiler-artifact', 'compiler-message', 'build-script-executed', 'build-finished'):
                        summary = {'reason': reason}
                        if isinstance(row.get('package_id'), str): summary['packageId'] = row['package_id'][:512]
                        target = row.get('target')
                        if isinstance(target, dict):
                            if isinstance(target.get('name'), str): summary['targetName'] = target['name'][:256]
                            if isinstance(target.get('kind'), list): summary['targetKind'] = [str(x)[:64] for x in target['kind'][:8]]
                        if type(row.get('fresh')) is bool:
                            summary['fresh'] = row['fresh']; events['freshTrue' if row['fresh'] else 'freshFalse'] += 1
                        if reason == 'build-finished':
                            events['buildFinishedSeen'] += 1
                            events['lastBuildSuccess'] = row.get('success') if type(row.get('success')) is bool else None
                            summary['success'] = events['lastBuildSuccess']
                        if reason == 'compiler-message' and isinstance(row.get('message'), dict):
                            level = row['message'].get('level')
                            if isinstance(level, str): summary['level'] = level[:64]
                        events['counts'][reason] = events['counts'].get(reason, 0) + 1
                        events['lastEvents'] = (events['lastEvents'] + [summary])[-16:]
                        events['lastEventElapsed'] = time.monotonic() - started
            pending.clear(); line_bytes = 0; oversized = False
    def observation(stage, error=None):
        with stats_lock:
            streams = {label: {'totalReadBytes': value['bytes'], 'readSha256': value['hash'].hexdigest(),
                                'eofRead': value['eof'], 'eofConsumed': label in ended,
                                'lastReadElapsed': value['lastReadElapsed']}
                       for label, value in stats.items()}
        streams['stdout'].update(retainedBytes=stdout_bytes, retainedSha256=delivered_hash.hexdigest(),
                                 truncated=streams['stdout']['totalReadBytes']>stdout_bytes,
                                 retentionBasis='blocks delivered successfully to consumer; failed callback retention unknown')
        tail = bytes(stderr[-4096:])
        streams['stderr'].update(retainedBytes=len(stderr), retainedSha256=hashlib.sha256(stderr).hexdigest(),
                                 truncated=streams['stderr']['totalReadBytes']>len(stderr),
                                 tail=tail.decode('utf-8', errors='replace'), tailTruncated=len(stderr)>len(tail),
                                 tailBytes=len(tail), tailOmittedBytes=max(0, len(stderr)-len(tail)),
                                 tailStartsAtLineBoundary=not tail or len(tail)==len(stderr) or stderr[-len(tail)-1] == 10,
                                 tailEndsAtLineBoundary=not stderr or stderr.endswith(b'\n'))
        try: poll = process.poll(); poll_error = None
        except Exception as failure: poll = None; poll_error = brief(failure)
        record = {'schema': 'polaris-owned-child-diagnostic-v1', 'diagnosticsOnly': True,
                  'artifactAuthority': False, 'allDescendantsObserved': False, 'stage': stage,
                  'argv': [str(x)[:512] for x in argv[:32]], 'argvTruncated': len(argv)>32 or any(len(str(x))>512 for x in argv),
                  'cwd': str(cwd if cwd is not None else os.getcwd())[:2048], 'tool': str(argv[0])[:2048],
                  'pid': process.pid, 'startedUnixSeconds': started_wall, 'startedMonotonic': started,
                  'deadlineMonotonic': deadline, 'timeoutSeconds': timeout, 'elapsedSeconds': time.monotonic()-started,
                  'primaryPoll': poll, 'pollError': poll_error, 'streams': streams,
                  'queueMessages': messages.qsize(),
                  'readerAlive': {label: reader.is_alive() for label, reader in zip(('stdout','stderr'), readers)},
                  'limits': {'stdoutBytes': stdout_limit, 'stderrBytes': stderr_limit, 'queueBlocks': 4, 'blockBytes': 64*1024},
                  'cause': brief(error) if error is not None else None, 'cleanup': cleanup}
        if cargo_json:
            record['cargo'] = {**events, 'incompleteLineBytes': line_bytes, 'retainedIncompleteLineBytes': len(pending)}
        if diagnostic_context is not None: record['context'] = diagnostic_context
        return record
    def emit(stage, error=None):
        record = observation(stage, error)
        encoded = json.dumps(record, ensure_ascii=True, separators=(',', ':'))
        if len(encoded.encode()) > MAX_CHILD_DIAGNOSTIC:
            # Caller context is never proof; keep the actual transport/failure facts
            # even if a future diagnostic field exceeds the closed record budget.
            record.pop('context', None); record.pop('argv', None); record.pop('cargo', None)
            record['diagnosticFieldsTruncated'] = True
            encoded = json.dumps(record, ensure_ascii=True, separators=(',', ':'))
        require(len(encoded.encode()) <= MAX_CHILD_DIAGNOSTIC, 'child diagnostic record byte limit exceeded')
        try: print('POLARIS_OWNED_CHILD_DIAGNOSTIC ' + encoded, file=os.sys.stderr, flush=True)
        except Exception as failure:
            if error is not None: error.add_note('child diagnostic write failed: ' + type(failure).__name__)
        return record
    def publish(message):
        while not stop.is_set():
            try: messages.put(message, timeout=0.05); return
            except queue.Full: pass
    def read_pipe(label, pipe):
        try:
            while not stop.is_set():
                block = pipe.read(64 * 1024)
                if not block:
                    with stats_lock: stats[label]['eof'] = True
                    break
                with stats_lock:
                    stats[label]['bytes'] += len(block); stats[label]['hash'].update(block)
                    stats[label]['lastReadElapsed'] = time.monotonic()-started
                publish((label, block))
        except OSError as error: publish(('reader-error', str(error)))
        finally: publish((label, None))
    readers = [threading.Thread(target=read_pipe, args=(label, pipe), daemon=True)
               for label, pipe in [('stdout', process.stdout), ('stderr', process.stderr)]]
    try:
        for reader in readers: reader.start()
        while len(ended) < 2:
            remaining = deadline - time.monotonic()
            require(remaining > 0, 'owned child output deadline exceeded')
            if output_guard is not None: output_guard()
            try: label, block = messages.get(timeout=min(remaining, 0.05))
            except queue.Empty: continue
            require(label != 'reader-error', 'owned child pipe read failed: ' + str(block))
            if block is None: ended.add(label); continue
            if label == 'stderr':
                require(len(stderr) + len(block) <= stderr_limit, 'owned child stderr byte limit exceeded')
                stderr.extend(block)
            else:
                require(stdout_bytes + len(block) <= stdout_limit, 'owned child stdout byte limit exceeded')
                if cargo_json: cargo_events(block); decoder.decode(block)
                consume_stdout(block)
                stdout_bytes += len(block); delivered_hash.update(block)
        if output_guard is not None:
            # EOF closes the pipes, not the child's ability to write named files.
            # Short waits keep the same guard/deadline active through actual exit;
            # do not append every timed poll to the bounded cleanup diagnostics.
            while True:
                output_guard()
                remaining = deadline - time.monotonic()
                require(remaining > 0, 'owned child exit deadline exceeded')
                try: process.wait(timeout=min(remaining, 0.05))
                except subprocess.TimeoutExpired: continue
                break
        remaining = deadline - time.monotonic()
        require(remaining > 0, 'owned child exit deadline exceeded')
        try: code = wait_primary(remaining, 'exit')
        except subprocess.TimeoutExpired as error: raise ValueError('owned child exit deadline exceeded') from error
        if output_guard is not None: output_guard()
        require(code == 0, 'owned child failed (' + str(code) + '): ' + stderr.decode('utf-8', errors='replace'))
        if decoder is not None: decoder.decode(b'', final=True)
        return {'stdoutBytes': stdout_bytes, 'stderrBytes': len(stderr), 'pid': process.pid,
                'birth': process_birth(process.pid, getattr(process, '_handle', None)), 'exitCode': code,
                'stdoutSha256': delivered_hash.hexdigest(), 'pipeEOF': sorted(ended),
                'actualWaitSeconds': time.monotonic() - started,
                'stderr': stderr.decode('utf-8', errors='replace')}
    except BaseException as error:
        cause = error
        emit('failure-before-cleanup', error)
        raise
    finally:
        stop.set()
        try:
            cleanup['pollBeforeAction'] = process.poll()
            if cleanup['pollBeforeAction'] is None:
                cleanup['terminate'] = {'attempted': True}; process.terminate(); cleanup['terminate']['returned'] = True
                try:
                    code = wait_primary(2, 'after-terminate')
                except subprocess.TimeoutExpired:
                    cleanup['kill'] = {'attempted': True}; process.kill(); cleanup['kill']['returned'] = True
                    code = wait_primary(None, 'after-kill')  # Existing unbounded final wait.
            else:
                code = wait_primary(None, 'already-exited')
        except BaseException as error: cleanup['errors'].append(brief(error)); cleanup_error = error
        for label, reader in zip(('stdout', 'stderr'), readers):
            try:
                begin = time.monotonic()
                if reader.ident is not None: reader.join(timeout=2)
                cleanup['readers'].append({'stream': label, 'started': reader.ident is not None, 'aliveAfterJoin': reader.is_alive(),
                                           'joinTimeoutSeconds': 2, 'joinElapsedSeconds': time.monotonic()-begin})
            except BaseException as error: cleanup['errors'].append(brief(error)); cleanup_error = error
        for label, pipe in [('stdout', process.stdout), ('stderr', process.stderr)]:
            try: pipe.close(); cleanup['pipes'].append({'stream': label, 'closed': pipe.closed})
            except BaseException as error: cleanup['errors'].append(brief(error)); cleanup_error = error
        if any(reader.is_alive() for reader in readers):
            cleanup_error = ValueError('owned child pipe readers did not join'); cleanup['errors'].append(brief(cleanup_error))
        if cleanup['errors']:
            if cause is not None: cause.add_note('owned child cleanup also failed: ' + json.dumps(cleanup['errors']))
            else: cause = cleanup_error
        if cause is not None: emit('failure-after-cleanup', cause)
        elif diagnostic_context is not None and diagnostic_context.get('candidate'):
            emit('transport-complete')
        if cleanup['errors'] and cause is cleanup_error: raise cleanup_error


def bounded_command(argv, limit, *, timeout):
    output = bytearray()
    stream_child(argv, output.extend, limit, timeout=timeout)
    return bytes(output)


def extract_nsis_member(tool, archive, name, path, allowed_bytes, budget, deadline):
    """One literal member per stdout stream: no concatenation/boundary guesses."""
    require(0 <= allowed_bytes <= MAX_FILE, 'invalid registered NSIS member limit')
    remaining_bytes = min(allowed_bytes, MAX_TOTAL - budget['bytes'])
    require(remaining_bytes >= 0, 'NSIS aggregate byte limit exceeded')
    digest = hashlib.sha256(); written = 0; created = False
    io_path(path.parent).mkdir(parents=True, exist_ok=True)
    try:
        with io_path(path).open('xb') as output:
            created = True
            def consume(block):
                nonlocal written
                # stream_child rejects the entire over-quota block before this
                # callback; no surplus member/aggregate bytes reach this file.
                output.write(block); digest.update(block)
                written += len(block); budget['bytes'] += len(block)
            observation = stream_child([tool, 'x', '-so', '-bd', '-y', '-spd', '--', str(archive), name],
                                       consume, remaining_bytes, timeout=deadline - time.monotonic())
        return {'bytes': written, 'sha256': digest.hexdigest()}, observation
    except BaseException:
        # This path was created exclusively here. The child has joined already.
        if created: io_path(path).unlink(missing_ok=True)
        raise


def extract_nsis_bound_files(tool, archive, destination, records, expected, actual, budget, deadline):
    """One named extraction into a fresh, pre-admitted private namespace.

    No concatenated stdout order or inferred member framing is authoritative.
    Unknown-size/content members have already been read by their literal name.
    """
    selected = {name: expected[name] for name, _ in records if name not in actual}
    require(sum(row['bytes'] for row in selected.values()) <= MAX_TOTAL - budget['bytes'],
            'NSIS aggregate byte limit exceeded')
    if not selected: return
    all_names = {name for name, _ in records}
    directories = {'/'.join(name.split('/')[:i]) for name in all_names for i in range(1, len(name.split('/')))}
    require(len(all_names) + len(directories) <= MAX_MEMBERS, 'NSIS extracted entry limit exceeded')
    maxima = {}; charged = 0

    def bounds():
        nonlocal charged
        pending = [destination]; seen = set()
        while pending:
            parent = pending.pop()
            with os.scandir(io_path(parent)) as entries:
                for entry in entries:
                    path = parent / entry.name; name = path.relative_to(destination).as_posix()
                    info = entry.stat(follow_symlinks=False)
                    require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400,
                            'NSIS extracted reparse or link')
                    if stat.S_ISDIR(info.st_mode):
                        require(name in directories, 'unregistered NSIS extracted directory'); pending.append(path)
                    else:
                        require(stat.S_ISREG(info.st_mode) and name in all_names,
                                'unregistered NSIS extracted member')
                        require(info.st_size <= expected[name]['bytes'] <= MAX_FILE, 'NSIS extracted member byte limit')
                        if name in selected: maxima[name] = max(maxima.get(name, 0), info.st_size)
                    seen.add(name)
                    require(len(seen) <= MAX_MEMBERS, 'NSIS extracted entry limit exceeded')
        observed = sum(maxima.values())
        require(observed - charged <= MAX_TOTAL - budget['bytes'], 'NSIS aggregate byte limit exceeded')
        budget['bytes'] += observed - charged; charged = observed

    # A list file avoids Windows command-length limits without wildcard selection.
    # Its contents are the already validated, exact logical names, one per line.
    with tempfile.TemporaryDirectory(prefix='nsis-list-', dir=io_path(destination.parent)) as tmp, ExitStack() as held:
        selector = Path(tmp) / 'members.txt'
        with io_path(selector).open('xb') as output:
            output.write(('\n'.join(selected) + '\n').encode('utf-8')); output.flush(); os.fsync(output.fileno())
        if os.name == 'nt':
            held.enter_context(hold_resource(selector))
            held.enter_context(hold_resource(destination, directory=True))
            for name in actual: held.enter_context(hold_resource(destination / name))
        stream_child([tool, 'x', '-bd', '-y', '-spd', '-scsUTF-8', '-o' + str(io_path(destination)),
                      '-i@' + str(io_path(selector)), '--', str(archive)],
                     lambda block: None, MAX_LISTING, timeout=deadline - time.monotonic(), output_guard=bounds)
        bounds()
        # The tool has actually exited and both pipes joined. Pin actual opened
        # output objects before hashing; ordinary single-link rules still apply.
        for name in all_names:
            path = regular(destination / name)
            if os.name == 'nt': held.enter_context(hold_resource(path))
            else: held.enter_context(io_path(path).open('rb'))
        observed = tree(destination)
        exact(observed, {name: expected[name] for name, _ in records})
        actual.update(observed)


def unpack_nsis(tool, archive, destination, allowed, *, budget=None, timeout=EXTRACT_TIMEOUT, expected=None):
    deadline = time.monotonic() + timeout
    archive = regular(archive)
    with ExitStack() as held:
        if os.name == 'nt': held.enter_context(hold_resource(archive))
        else: held.enter_context(io_path(archive).open('rb'))
        identity = app_identity(io_path(archive).stat()); original = facts(archive)
        result = unpack_nsis_held(tool, archive, destination, allowed, budget=budget,
                                  timeout=deadline - time.monotonic(), expected=expected)
        require(app_identity(io_path(regular(archive)).stat()) == identity and facts(archive) == original,
                'held NSIS archive changed')
        require(time.monotonic() < deadline, 'NSIS shared extraction deadline exceeded')
        return result


def unpack_nsis_held(tool, archive, destination, allowed, *, budget, timeout, expected):
    # One deadline covers listing AND all member streams. A solid archive can
    # require repeated decoder work; no member gets a fresh 180s allowance.
    deadline = time.monotonic() + timeout
    archive = regular(archive)
    listing = bounded_command([tool, 'l', '-slt', '-sccUTF-8', str(archive)], MAX_LISTING,
                              timeout=deadline - time.monotonic()).decode('utf-8-sig').replace('\r\n', '\n')
    records = nsis_listing(listing)
    require({name for name, _ in records} <= set(allowed), 'unregistered NSIS listed member')
    for name, size in records:
        require(isinstance(allowed[name], int) and 0 <= allowed[name] <= MAX_FILE,
                'invalid registered NSIS byte budget')
        require(size is None or size <= allowed[name], 'NSIS member exceeds registered size')
    known = {} if expected is None else {name: dict(row) for name, row in expected.items()}
    for name, row in known.items():
        require(name in allowed and set(row) == {'bytes', 'sha256'}
                and isinstance(row['bytes'], int) and 0 <= row['bytes'] <= allowed[name]
                and isinstance(row['sha256'], str) and re.fullmatch('[0-9a-f]{64}', row['sha256']),
                'invalid bound NSIS member facts')
    for name, size in records:
        require(name not in known or size is None or size == known[name]['bytes'],
                'listed NSIS size differs from bound member')
    require(sum(size if size is not None else allowed[name] for name, size in records) <= MAX_TOTAL,
            'NSIS expanded budget exceeded')
    if budget is None: budget = {'bytes': 0}
    require(not io_path(destination).exists() and io_path(destination.parent).is_dir(), 'new owned NSIS output directory required')
    io_path(destination).mkdir()
    actual = {}
    try:
        if expected is None:
            for name, size in records:
                value, _ = extract_nsis_member(tool, archive, name, destination / name, allowed[name], budget, deadline)
                require(size is None or value['bytes'] == size, 'extracted NSIS size differs')
                actual[name] = value
        else:
            # The uninstaller has no producer-side content hash. Read it by its
            # literal name first; never infer its length/content from adjacent data.
            for name, size in records:
                if name not in known:
                    value, _ = extract_nsis_member(tool, archive, name, destination / name, allowed[name], budget, deadline)
                    require(size is None or value['bytes'] == size, 'extracted NSIS size differs')
                    known[name] = value; actual[name] = value
            extract_nsis_bound_files(tool, archive, destination, records, known, actual, budget, deadline)
        require(time.monotonic() < deadline, 'NSIS shared extraction deadline exceeded')
        exact(tree(destination), actual)
        return actual, listing
    except BaseException:
        # Only this function's newly-created directory; no caller/user path is
        # recursively removed. Every failed child was joined by stream_child.
        shutil.rmtree(io_path(destination))
        raise


def plugin_inventory(root):
    # The pinned Tauri bundler uses this exact unsigned cached NSIS toolset.
    cache = Path(os.environ['LOCALAPPDATA']) / 'tauri/NSIS/Plugins/x86-unicode'
    result = {}
    for name in ['System.dll', 'nsDialogs.dll', 'StartMenu.dll', 'NSISdl.dll', 'LangDLL.dll', 'nsExec.dll', 'nsis_tauri_utils.dll']:
        source = cache / ('additional/' + name if name == 'nsis_tauri_utils.dll' else name)
        result['$PLUGINSDIR/' + name] = facts(source)
    # Current template has no custom header/sidebar. MUI2 welcome/finish embeds
    # this exact NSIS-owned bitmap, separate from application resources.
    conf = json.loads(io_path(regular(root / 'src-tauri/tauri.conf.json')).read_bytes())['bundle']['windows']['nsis']
    require(conf['headerImage'] is None and conf['sidebarImage'] is None and conf['template'] is None,
            'unreviewed NSIS UI resource mapping')
    result['$PLUGINSDIR/modern-wizard.bmp'] = facts(cache.parents[1] / 'Contrib/Graphics/Wizard/win.bmp')
    return result


def nsis_inventory(root, tool, archive, expected, scratch):
    # Keep the full $R6 payload and $PLUGINSDIR tool namespaces distinct.
    auxiliary = dict(plugin_inventory(root))
    cleaner = '$PLUGINSDIR/polaris-cleaner.exe'
    pinned_cleaner = expected['$R6/_up_/resources/win/polaris-cleaner.exe']
    require(cleaner not in auxiliary, 'cleaner must bind to the frozen application input')
    auxiliary[cleaner] = pinned_cleaner
    uninstaller = 'uninstall.exe'
    allowed = {name: row['bytes'] for name, row in {**expected, **auxiliary}.items()}
    allowed[uninstaller] = MAX_UNINSTALLER
    budget = {'bytes': 0}
    actual, listing = unpack_nsis(tool, archive, scratch / 'installer', allowed, budget=budget,
                                  expected={**expected, **auxiliary})
    require(set(actual) <= set(allowed) and uninstaller in actual, 'unregistered NSIS installer member')
    exact({name: row for name, row in actual.items() if name in expected}, expected)
    require(cleaner in actual, 'installer cleaner missing')
    for name in set(actual) & set(auxiliary):
        require(actual[name] == auxiliary[name], 'NSIS plugin differs from selected toolchain: ' + name)
    nested_allowed = {name: row['bytes'] for name, row in auxiliary.items()}
    nested, nested_listing = unpack_nsis(tool, scratch / 'installer' / uninstaller, scratch / 'uninstaller', nested_allowed, budget=budget,
                                         expected=auxiliary)
    require(cleaner in nested and set(nested) <= set(auxiliary) | {cleaner}, 'unregistered NSIS uninstaller member')
    require(nested[cleaner] == pinned_cleaner, 'embedded uninstall cleaner differs')
    for name in set(nested) & set(auxiliary):
        require(nested[name] == auxiliary[name], 'uninstaller plugin differs: ' + name)
    return {'members': actual, 'uninstallerMembers': nested, 'listing': listing, 'uninstallerListing': nested_listing,
            'streamedBytesIncludingNestedUninstaller': budget['bytes'],
            'extractionLimits': {'memberBytes': MAX_FILE, 'aggregateBytes': MAX_TOTAL, 'uninstallerBytes': MAX_UNINSTALLER,
                                 'listingBytes': MAX_LISTING, 'stderrBytes': MAX_STDERR, 'perUnpackSeconds': EXTRACT_TIMEOUT}}


def build_portable(root, state, app, destination):
    # Fresh standard-library owned staging; no overlay and no arbitrary rmtree.
    with tempfile.TemporaryDirectory(prefix='portable-', dir=io_path(state['owner'])) as tmp:
        staging = Path(tmp)
        for name, source in state['sources'].items():
            name = name[5:] if name.startswith('_up_/') else name
            path = staging / name; io_path(path.parent).mkdir(parents=True, exist_ok=True)
            shutil.copyfile(io_path(regular(source)), io_path(path))
        io_path(staging / 'polaris.exe').write_bytes(app)
        io_path(staging / 'portable.marker').write_bytes(MARKER)
        expected = carrier_inventory(state, app, 'zip'); exact(tree(staging), expected)
        with zipfile.ZipFile(io_path(destination), 'x', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
            for name in sorted(expected): archive.write(io_path(staging / name), name)
    zip_inventory(destination, expected)


def verify_generated_script(root, state, *, sources=None):
    sources = state['sources'] if sources is None else sources
    require(set(sources) == set(state['sources']), 'NSIS source member set differs')
    script = regular(Path(state['cargo']['releaseDirectory']) / 'nsis/x64/installer.nsi')
    text = io_path(script).read_text(encoding='utf-8-sig')
    def define(name):
        values = re.findall(r'^!define ' + name + r' "([^"\r\n]+)"\s*$', text, re.MULTILINE)
        require(len(values) == 1, 'generated NSIS define must be unique: ' + name)
        return values[0]
    require(define('VERSION') == state['version'] and define('MAINBINARYNAME') == 'polaris', 'NSIS source/version differs')
    app_source, _ = cargo_app_identity(root, state)
    require(Path(define('MAINBINARYSRCPATH')).absolute() == app_source.absolute(), 'NSIS main input differs')
    sections = re.findall(r'^Section Install\s*$(.*?)^SectionEnd\s*$', text, re.MULTILINE | re.DOTALL)
    require(len(sections) == 1, 'unique normal NSIS install section required')
    section = sections[0]
    require(len(re.findall(r'^\s*File "\$\{MAINBINARYSRCPATH\}"\s*$', section, re.MULTILINE)) == 1,
            'unique NSIS App File required')
    files = re.findall(r'^\s*File /a "/oname=([^"\r\n]+)" "([^"\r\n]+)"\s*$', section, re.MULTILINE)
    mapped = {}
    for name, source in files:
        name = name.replace('\\', '/')
        require(name not in mapped and name in state['sources'], 'extra/duplicate NSIS File mapping: ' + name)
        require(io_path(regular(source)).samefile(io_path(regular(sources[name]))), 'NSIS File source differs: ' + name)
        mapped[name] = facts(source)
    exact(mapped, state['resources'])
    require(len(re.findall(r'^\s*File\b', section, re.MULTILINE)) == len(files) + 1, 'unparsed NSIS File instruction')
    return facts(script)


def cargo_app_identity(root, state=None):
    """Admit only the fixed Cargo output and its sole known deps hardlink."""
    app_path = (Path(state['cargo']['releaseDirectory']) if state is not None else root / 'target/release') / 'polaris.exe'
    for path in [app_path, *app_path.parents]:
        value = io_path(path).lstat()
        require(not stat.S_ISLNK(value.st_mode) and not (getattr(value, 'st_file_attributes', 0) & 0x400),
                'link/reparse Cargo input: ' + str(path))
    value = io_path(app_path).stat()
    require(stat.S_ISREG(value.st_mode) and 0 < value.st_size <= MAX_FILE and value.st_nlink in (1, 2),
            'bounded Cargo App with at most its known deps link required')
    if value.st_nlink == 2:
        alias = app_path.parent / 'deps/polaris.exe'
        for path in [alias, alias.parent]:
            other = io_path(path).lstat()
            require(not stat.S_ISLNK(other.st_mode) and not (getattr(other, 'st_file_attributes', 0) & 0x400),
                    'link/reparse Cargo deps alias')
        require(io_path(alias).samefile(io_path(app_path)) and io_path(alias).stat().st_nlink == 2, 'unexpected Cargo App hardlink')
    return app_path, app_identity(value)


def app_identity(value):
    return (value.st_dev, value.st_ino, value.st_mode, value.st_nlink,
            value.st_size, value.st_mtime_ns, value.st_ctime_ns)


def app_object_identity(identity):
    # Windows pathname stat adds executable bits and reports birth/creation
    # ctime; fd fstat reports change time. Bind their common object facts,
    # while retaining complete before/after metadata within each API view.
    return (*identity[:2], stat.S_IFMT(identity[2]), *identity[3:6])


def compiled_app(root, *, staging=None, state=None):
    app_path, identity = cargo_app_identity(root, state)
    with io_path(app_path).open('rb') as stream:
        fd_identity = app_identity(os.fstat(stream.fileno()))
        require(app_object_identity(fd_identity) == app_object_identity(identity), 'Cargo App replaced before read')
        app = stream.read(MAX_FILE + 1)
        require(app_identity(os.fstat(stream.fileno())) == fd_identity, 'Cargo App changed while reading')
    require(cargo_app_identity(root, state)[1] == identity and len(app) == identity[4], 'Cargo App path changed while reading')
    pin = io_path(regular(root / 'resources/win/core-manifest.json')).read_bytes()
    require(app.count(pin) == 1, 'App must embed the exact frozen selected manifest once')
    if staging is not None:
        # A fresh producer-owned single-link snapshot, never an audit copy or
        # deletion of Cargo's alias. The original file identity stays guarded.
        app_path = Path(staging) / 'polaris.exe'
        with io_path(app_path).open('xb') as stream:
            stream.write(app); stream.flush(); os.fsync(stream.fileno())
        require(facts(app_path) == byte_facts(app), 'private compiled App snapshot differs')
        require(cargo_app_identity(root, state)[1] == identity, 'Cargo App changed during private staging')
    return app_path, app


def tauri_command():
    # CreateProcess does not perform the shell's PATHEXT resolution for the
    # bare name. Resolve the normal selected tool explicitly, including .cmd.
    executable = shutil.which('tauri')
    require(executable is not None, 'normal Tauri CLI entry is missing from PATH')
    if os.name == 'nt':
        require(Path(executable).suffix.lower() in ('.cmd', '.exe'), 'unsupported Tauri CLI entry')
    return executable


def tauri_identity(root):
    version = io_path(regular(root / 'scripts/tauri-cli.version')).read_text().strip()
    executable = tauri_command()
    actual = subprocess.check_output([executable, '--version'], timeout=30, text=True).strip()
    require(version == final.TAURI_CLI_VERSION and actual == 'tauri-cli ' + version, 'unreviewed App transform tool')
    return {'entry': executable, 'version': actual, 'tool': cargo_tool(executable)}


CAPTURE_ENV = 'POLARIS_WINDOWS_PACKAGE_CAPTURE'
CAPTURE_READY = b'POLARIS_WINDOWS_PACKAGE_CAPTURE_READY'
RUNNER_TEXT = ('@echo off\r\nsetlocal DisableDelayedExpansion\r\n'
               '"%POLARIS_PACKAGE_PYTHON%" -B "%POLARIS_PACKAGE_SCRIPT%" capture '
               '--state "%POLARIS_PACKAGE_STATE%" -- %*\r\nexit /b %ERRORLEVEL%\r\n')


def windows_api():
    require(os.name == 'nt', 'native Windows producer required')
    from ctypes import wintypes as wt
    api = ctypes.WinDLL('kernel32', use_last_error=True)
    for name, result, args in [
        ('OpenProcess', wt.HANDLE, [wt.DWORD, wt.BOOL, wt.DWORD]),
        ('CloseHandle', wt.BOOL, [wt.HANDLE]),
        ('GetProcessTimes', wt.BOOL, [wt.HANDLE, *([ctypes.POINTER(wt.FILETIME)] * 4)]),
        ('WaitForSingleObject', wt.DWORD, [wt.HANDLE, wt.DWORD]),
        ('CreateFileW', wt.HANDLE, [wt.LPCWSTR, wt.DWORD, wt.DWORD, wt.LPVOID, wt.DWORD, wt.DWORD, wt.HANDLE]),
        ('GetCurrentProcess', wt.HANDLE, []),
        ('GetFileInformationByHandleEx', wt.BOOL, [wt.HANDLE, ctypes.c_int, wt.LPVOID, wt.DWORD]),
        ('DuplicateHandle', wt.BOOL, [wt.HANDLE, wt.HANDLE, wt.HANDLE, ctypes.POINTER(wt.HANDLE), wt.DWORD, wt.BOOL, wt.DWORD])]:
        function = getattr(api, name); function.restype = result; function.argtypes = args
    return api, wt


def process_birth(pid, held=None):
    if os.name != 'nt': return None
    api, wt = windows_api(); handle = held if held is not None else api.OpenProcess(0x1000, False, pid)
    if not handle: raise ctypes.WinError(ctypes.get_last_error())
    try:
        values = [wt.FILETIME() for _ in range(4)]
        if not api.GetProcessTimes(handle, *(ctypes.byref(value) for value in values)):
            raise ctypes.WinError(ctypes.get_last_error())
        return values[0].dwHighDateTime << 32 | values[0].dwLowDateTime
    finally:
        if held is None: api.CloseHandle(handle)


def hold_app(path, *, write_share=True):
    # Allow Tauri's pinned in-place resource patch, but never a namespace swap.
    import msvcrt
    api, _ = windows_api()
    handle = api.CreateFileW(str(io_path(path)), 0x80000000, 1 | (2 if write_share else 0), None, 3, 0, None)
    if handle == ctypes.c_void_p(-1).value: raise ctypes.WinError(ctypes.get_last_error())
    try: fd = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
    except BaseException: api.CloseHandle(handle); raise
    return os.fdopen(fd, 'rb')


def duplicate_app(pid, birth, handle):
    import msvcrt
    api, wt = windows_api(); process = api.OpenProcess(0x1000 | 0x40 | 0x100000, False, pid)
    if not process: raise ctypes.WinError(ctypes.get_last_error())
    try:
        require(process_birth(pid, process) == birth and api.WaitForSingleObject(process, 0) == 258,
                'capture process exited or changed before handle transfer')
        duplicate = wt.HANDLE()
        if not api.DuplicateHandle(process, handle, api.GetCurrentProcess(), ctypes.byref(duplicate), 0, False, 2):
            raise ctypes.WinError(ctypes.get_last_error())
        try: fd = msvcrt.open_osfhandle(duplicate.value, os.O_RDONLY | os.O_BINARY)
        except BaseException: api.CloseHandle(duplicate); raise
        return os.fdopen(fd, 'rb')
    finally: api.CloseHandle(process)


@contextmanager
def hold_resource(path, *, directory=False):
    """Pin a derived resource object without CRT descriptor-count limits."""
    path = cargo_directory(path) if directory else regular(path)
    before = io_path(path).stat(); api, _ = windows_api()
    handle = api.CreateFileW(str(io_path(path)), 0x80000000, 3 if directory else 1, None, 3,
                             0x02200000 if directory else 0x00200000, None)
    if handle == ctypes.c_void_p(-1).value: raise ctypes.WinError(ctypes.get_last_error())
    try:
        class FileID(ctypes.Structure):
            _fields_ = [('volume', ctypes.c_ulonglong), ('id', ctypes.c_ubyte * 16)]
        value = FileID()
        if not api.GetFileInformationByHandleEx(handle, 18, ctypes.byref(value), ctypes.sizeof(value)):
            raise ctypes.WinError(ctypes.get_last_error())
        identity = (value.volume, int.from_bytes(value.id, 'little'))
        after = io_path(cargo_directory(path) if directory else regular(path)).stat()
        require(identity == (before.st_dev, before.st_ino) == (after.st_dev, after.st_ino),
                'derived resource handle/path identity differs')
        yield identity
    finally: api.CloseHandle(handle)


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode('ascii')


def signed_record(path, data, key):
    temporary = path.with_name(path.name + '.tmp')
    require(not io_path(path).exists(), 'capture record is single use')
    with io_path(temporary).open('xb') as stream:
        stream.write(encoded({'data': data, 'mac': hmac.new(key, encoded(data), 'sha256').hexdigest()}))
        stream.flush(); os.fsync(stream.fileno())
    io_path(temporary).rename(io_path(path))


def read_signed(path, key):
    before = facts(path); require(before['bytes'] <= MAX_LISTING, 'bounded capture record required')
    record = json.loads(io_path(regular(path)).read_bytes())
    require(facts(path) == before and hmac.compare_digest(record['mac'], hmac.new(key, encoded(record['data']), 'sha256').hexdigest()),
            'missing, foreign or modified live producer evidence')
    return record['data']


def release_arguments(state, args):
    # This is the actual normal Tauri 2.12.1 argv, not a second build recipe.
    require(args and args[0] == 'build', 'only the normal Tauri release build is supported')
    flags = []; values = {}; index = 1
    while index < len(args):
        arg = args[index]; index += 1
        if arg in ('--features', '--target'):
            require(arg not in values and index < len(args), 'duplicate or missing Cargo value')
            values[arg] = args[index]; index += 1
        else: flags.append(arg)
    require(sorted(flags) == sorted(['--locked', '--offline', '--bins', '--release', '--message-format=json'])
            and values.get('--features') == 'tauri/custom-protocol'
            and values.get('--target') == state['cargo']['target'], 'foreign Cargo release arguments/features/target')


def release_event(root, state, output):
    context = state['cargo']
    events = [json.loads(line) for line in output.splitlines() if line.strip()]
    require(events and len(events) <= MAX_MEMBERS and all(isinstance(row, dict) for row in events), 'bounded Cargo JSON events required')
    artifacts = [row for row in events if row.get('reason') == 'compiler-artifact'
                 and row.get('target', {}).get('name') == 'polaris' and row['target'].get('kind') == ['bin']]
    require(len(artifacts) == 1 and [row for row in events if row.get('reason') == 'build-finished'] == [{'reason': 'build-finished', 'success': True}]
            and events[-1] == {'reason': 'build-finished', 'success': True},
            'unique successful Cargo App artifact required')
    row = artifacts[0]; profile = row['profile']
    expected, identity = cargo_app_identity(root, state)
    require(row['package_id'] == context['packageId'] and Path(row['manifest_path']) == Path(context['manifest'])
            and Path(row['target']['src_path']) == Path(context['binarySource'])
            and Path(row['executable']) == expected and str(expected) in row['filenames']
            and profile['test'] is False and profile['debug_assertions'] is False
            and profile['opt_level'] in ('1', '2', '3', 's', 'z'), 'foreign/debug Cargo App artifact')
    return row, identity


def capture_context(root, state, scope):
    require(time.monotonic() < scope['deadline'] and process_birth(scope['parent']) == scope['parentBirth'], 'producer scope expired')
    require(cargo_tool(os.sys.executable) == scope['python'] and facts(Path(__file__)) == scope['script'], 'capture implementation changed')
    actual = cargo_context(root, state['cargo']['target']); expected = json.loads(json.dumps(state['cargo']))
    # Pinned Tauri merges precisely this explicit Windows config into its child.
    # Freeze the parent independently; accept only the observed normal child value.
    config = regular(Path(state['owner']) / 'native-windows.conf.json')
    proof = regular(Path(state['owner']) / 'resource-copies.json')
    require(facts(proof) == scope['copies'], 'derived resource proof changed')
    copies = json.loads(io_path(proof).read_bytes()); verify_resource_copies(root, state, copies)
    expected_config = native_windows_config(root, Path(copies['root']))
    require(facts(config) == scope['config'] and json.loads(io_path(config).read_bytes()) == expected_config,
            'native Tauri input configuration changed')
    require(json.loads(os.environ['TAURI_CONFIG']) == expected_config, 'foreign Tauri child configuration')
    expected['environment']['TAURI_CONFIG'] = os.environ['TAURI_CONFIG']
    require(actual == expected and os.environ.get('POLARIS_BUILD_ID', state['candidate']) == state['candidate'],
            'first producer effective Cargo context changed')
    require(state['root'] == str(root) and Path(state['owner']) == Path(scope['statePath']).parent
            and Path(state['owner']).parent == Path(actual['releaseDirectory'])
            and Path(state['owner']).name.startswith('.polaris-package-')
            and directory_id(actual['targetDirectory']) == state['targetIdentity']
            and directory_id(actual['releaseDirectory']) == state['releaseIdentity']
            and directory_id(state['owner']) == state['ownerIdentity'], 'first producer namespace changed')
    inventory, sources = verified_sources(root)
    require(inventory == state['resources'] and sources == state['sources']
            and subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip() == state['candidate']
            and json.loads(io_path(root / 'src-tauri/tauri.conf.json').read_bytes())['version'] == state['version'],
            'first producer source changed')
    return actual


def capture(root, args=None):
    """Private Tauri runner. The evidence channel is this exact Cargo Popen pipe."""
    if args is None:
        require('--' in os.sys.argv, 'private runner requires explicit Cargo arguments')
        args = os.sys.argv[os.sys.argv.index('--') + 1:]
    scope = json.loads(os.environ[CAPTURE_ENV]); key = bytes.fromhex(scope.pop('key'))
    require(os.environ['POLARIS_WINDOWS_PACKAGE_STATE'] == scope['statePath']
            and facts(scope['statePath']) == scope['stateFile'], 'input state changed')
    state = json.loads(io_path(regular(scope['statePath'])).read_bytes())
    release_arguments(state, args); actual = capture_context(root, state, scope)
    owner = Path(state['owner']); raw = owner / 'first-cargo.jsonl'; transport = {}
    child_env = {name: value for name, value in os.environ.items() if name not in
                 (CAPTURE_ENV, 'POLARIS_PACKAGE_PYTHON', 'POLARIS_PACKAGE_SCRIPT', 'POLARIS_PACKAGE_STATE')}
    argv = [state['cargo']['tool']['path'], *args]
    with io_path(raw).open('xb') as stream:
        output = cargo_output(argv, root / 'src-tauri', timeout=scope['deadline'] - time.monotonic(),
                              diagnostic_context={'candidate': state['candidate'], 'firstActualChildObserved': True},
                              env=child_env, spool=stream, transport=transport)
        stream.flush(); os.fsync(stream.fileno())
    artifact, identity = release_event(root, state, output)
    require(capture_context(root, state, scope) == actual, 'Cargo inputs changed during first build')
    require(transport['exitCode'] == 0 and transport['birth'] is not None and transport['pipeEOF'] == ['stderr', 'stdout']
            and transport['stdoutBytes'] == facts(raw)['bytes'] and transport['stdoutSha256'] == facts(raw)['sha256'],
            'incomplete first Cargo process/pipe evidence')
    path, _ = cargo_app_identity(root, state)
    with hold_app(path) as held:
        require(app_object_identity(app_identity(os.fstat(held.fileno()))) == app_object_identity(identity), 'first Cargo App changed before custody')
        before = app_identity(os.fstat(held.fileno()))
        original = held.read(MAX_FILE + 1)
        require(len(original) == before[4] <= MAX_FILE and app_identity(os.fstat(held.fileno())) == before
                and cargo_app_identity(root, state)[1] == identity, 'first Cargo App changed during original snapshot')
        with io_path(owner / 'first-app.bin').open('xb') as snapshot:
            snapshot.write(original); snapshot.flush(); os.fsync(snapshot.fileno())
        require(facts(owner / 'first-app.bin') == byte_facts(original), 'first Cargo snapshot differs')
        import msvcrt
        record = {'scope': scope, 'argv': argv, 'artifact': artifact, 'identity': list(identity),
                  'firstApp': byte_facts(original),
                  'transport': transport, 'stdout': facts(raw), 'pid': os.getpid(), 'birth': process_birth(os.getpid()),
                  'handle': msvcrt.get_osfhandle(held.fileno()),
                  'childConfig': {'present': True, 'bytes': len(os.environ['TAURI_CONFIG'].encode('utf-8')),
                                  'sha256': hashlib.sha256(os.environ['TAURI_CONFIG'].encode('utf-8')).hexdigest()}}
        signed_record(owner / 'capture.json', record, key)
        notify_capture_ready()
        while not io_path(owner / 'capture-ack.json').exists():
            require(time.monotonic() < scope['deadline'], 'live producer did not take App custody'); time.sleep(.01)
        require(read_signed(owner / 'capture-ack.json', key) == {'scope': scope, 'capture': facts(owner / 'capture.json')},
                'foreign App custody acknowledgement')


def notify_capture_ready():
    os.sys.stdout.buffer.write(b'\n' + CAPTURE_READY + b'\n'); os.sys.stdout.buffer.flush()


def run_tauri(root, argv, env, ready, timeout):
    pending = bytearray(); discard = False
    def consume(block):
        nonlocal discard
        os.sys.stdout.buffer.write(block); os.sys.stdout.buffer.flush()
        for part in block.splitlines(keepends=True):
            if not discard: pending.extend(part)
            if part.endswith(b'\n'):
                if not discard and pending.rstrip(b'\r\n') == CAPTURE_READY: ready()
                pending.clear(); discard = False
            elif len(pending) > 65536: pending.clear(); discard = True
    return stream_child(argv, consume, MAX_LISTING, timeout=timeout, cwd=root, env=env,
                        diagnostic_context={'candidate': env.get('POLARIS_BUILD_ID')})


def resource_source_key(consumer, physical):
    """Tauri source keys are relative to src-tauri, never drive-rooted paths."""
    require(consumer.is_absolute() and physical.is_absolute()
            and consumer.anchor.casefold() == physical.anchor.casefold(),
            'native resource stage must share the consumer drive/share anchor')
    paths = ntpath if isinstance(consumer, PureWindowsPath) else os.path
    name = paths.relpath(str(physical), str(consumer))
    require(not type(consumer)(name).anchor and not any(c in name for c in '*?')
            and paths.normcase(paths.normpath(paths.join(str(consumer), name)))
            == paths.normcase(paths.normpath(str(physical))), 'invalid relative native resource source')
    return name


def resource_source_path(root, stage, name):
    """Resolve only the frozen role roots against the actual Tauri/Cargo cwd."""
    require(isinstance(name, str) and not Path(name).anchor and not any(c in name for c in '*?'),
            'relative native resource source required')
    consumer = root / 'src-tauri'
    physical = Path(os.path.abspath(consumer / name))
    require(physical.parent == stage and resource_source_key(consumer, physical) == name,
            'native resource source escaped its stage or fixed consumer base')
    return physical


@contextmanager
def native_resource_stage(root, state):
    """Allocate one short owned stage on the consumer's existing drive/share.

    A default TEMP may be on another drive. Walk only existing source ancestors;
    keep the first short writable namespace, without links, drive maps or ACL edits.
    """
    root = cargo_directory(root)
    # Plan the exact short role/index suffixes, rather than charging their longer
    # logical destinations or assuming TEMP's random-name length.
    members = {}
    for source, target in native_windows_config(root, root)['bundle']['resources'].items():
        base = resource_source_path(root, root, source).relative_to(root)
        for name in state['resources']:
            if name == target or name.startswith(target + '/'):
                require(name not in members, 'ambiguous native resource role')
                members[name] = base if name == target else base / name[len(target) + 1:]
    require(set(members) == set(state['resources']), 'native resource role member set differs')
    for parent in root.parents:
        parent = cargo_directory(parent)
        if len(str(parent).encode('utf-16-le')) // 2 + 12 >= 260:
            continue
        try:
            directory = tempfile.TemporaryDirectory(prefix='r-', dir=parent)
        except PermissionError:
            continue
        with directory as temporary:
            stage = cargo_directory(Path(temporary))
            resource_source_key(root / 'src-tauri', stage / '0')
            if any(len(str(stage / suffix).encode('utf-16-le')) // 2 >= 260 for suffix in members.values()):
                continue
            yield stage
        return
    raise ValueError('no short writable native resource stage on the consumer drive/share')


def native_windows_config(root, resource_root):
    """The same eight declared logical roles, with owned short source paths."""
    config = json.loads(io_path(regular(root / 'src-tauri/tauri.windows.conf.json')).read_bytes())
    resource_root = cargo_directory(resource_root); resources = {}
    for index, entry in enumerate(config['bundle']['resources']):
        require(isinstance(entry, str) and not any(c in entry for c in '*?')
                and (entry == 'core-manifest.json' or entry.startswith('../') and not entry[3:].startswith('../')),
                'unreviewed native resource declaration')
        source = Path(os.path.abspath(root / 'src-tauri' / entry))
        require(source.is_relative_to(root), 'native resource escaped repository')
        name = resource_source_key(root / 'src-tauri', resource_root / str(index))
        target = entry.replace('../', '_up_/', 1).rstrip('/')
        require(name not in resources and target not in resources.values(), 'duplicate native resource declaration')
        resources[name] = target
    config['bundle']['resources'] = resources
    return config


def copy_resource_inputs(root, state, stage, owned):
    config = native_windows_config(root, stage); copies = {'root': str(stage), 'directories': {}, 'members': {}}
    # Parent namespace handles prevent replacement; directory writes remain
    # possible for unrelated ordinary temporary files. Every copied file is
    # separately held with neither WRITE nor DELETE sharing through Tauri.
    for directory in reversed([stage, *stage.parents]):
        copies['directories'][str(directory)] = list(owned.enter_context(hold_resource(directory, directory=True)))
    for short, target in config['bundle']['resources'].items():
        base = resource_source_path(root, stage, short)
        is_file = target in state['sources']
        if not is_file: io_path(base).mkdir()
        for name, source in state['sources'].items():
            if name != target and not name.startswith(target + '/'): continue
            destination = base if is_file else base / name[len(target) + 1:]
            require(len(str(destination).encode('utf-16-le')) // 2 < 260,
                    'owned stage is too deep for the native NSIS resource input')
            io_path(destination.parent).mkdir(parents=True, exist_ok=True)
            before = app_identity(io_path(regular(source)).stat()); expected = state['resources'][name]
            with hold_app(source, write_share=False) as stream, io_path(destination).open('xb') as output:
                held = app_identity(os.fstat(stream.fileno())); digest = hashlib.sha256(); count = 0
                require(app_object_identity(held) == app_object_identity(before), 'resource changed before copy')
                while block := stream.read(min(1024 * 1024, expected['bytes'] + 1 - count)):
                    count += len(block); require(count <= expected['bytes'], 'source resource grew during copy')
                    digest.update(block); output.write(block)
                output.flush(); os.fsync(output.fileno())
                require(app_identity(os.fstat(stream.fileno())) == held and app_identity(io_path(regular(source)).stat()) == before
                        and {'bytes': count, 'sha256': digest.hexdigest()} == expected, 'opened source bytes changed during copy')
            copy_id = owned.enter_context(hold_resource(destination))
            require(facts(destination) == expected, 'derived resource copy bytes differ')
            copies['members'][name] = {'sourceID': list(before[:2]), 'path': str(destination), 'copyID': list(copy_id)}
        if not is_file:
            for current, _, _ in os.walk(io_path(base), followlinks=False):
                directory = base / Path(current).relative_to(io_path(base))
                copies['directories'][str(directory)] = list(owned.enter_context(hold_resource(directory, directory=True)))
    verify_resource_copies(root, state, copies)
    return copies


def verify_resource_copies(root, state, copies):
    stage = cargo_directory(copies['root']); config = native_windows_config(root, stage)
    require(set(copies['members']) == set(state['sources']), 'derived resource member set differs')
    directories = {stage, *stage.parents}
    for short, target in config['bundle']['resources'].items():
        if target not in state['sources']: directories.add(resource_source_path(root, stage, short))
    for row in copies['members'].values():
        path = Path(row['path'])
        require(path.is_relative_to(stage), 'derived resource escaped its owner')
        directories.update(parent for parent in path.parents if parent.is_relative_to(stage))
    require(set(copies['directories']) == {str(path) for path in directories}, 'derived resource directory set differs')
    for directory, identity in copies['directories'].items():
        require(list(directory_id(directory)) == identity, 'derived resource ancestor changed')
    require(str(stage) in copies['directories'], 'derived resource root is not held')
    expected_tree = {}
    for name, source in state['sources'].items():
        declaration = [(short, target) for short, target in config['bundle']['resources'].items()
                       if name == target or name.startswith(target + '/')]
        require(len(declaration) == 1, 'ambiguous derived resource role')
        short, target = declaration[0]; base = resource_source_path(root, stage, short)
        path = base if name == target else base / name[len(target) + 1:]
        row = copies['members'][name]; original = io_path(regular(source)).stat(); value = io_path(regular(path)).stat()
        require(row == {'sourceID': [original.st_dev, original.st_ino], 'path': str(path), 'copyID': [value.st_dev, value.st_ino]}
                and facts(path) == state['resources'][name], 'derived resource identity or bytes changed')
        expected_tree[path.relative_to(stage).as_posix()] = state['resources'][name]
    exact(tree(stage), expected_tree)


def produce(root):
    state = load_state(root); cli = tauri_identity(root); owner = Path(state['owner'])
    require(not any(name in os.environ for name in (CAPTURE_ENV, 'POLARIS_PACKAGE_PYTHON', 'POLARIS_PACKAGE_SCRIPT', 'POLARIS_PACKAGE_STATE')),
            'nested or caller-selected capture scope')
    require(not any(io_path(owner / name).exists() for name in ('runner.cmd', 'resource-copies.json', 'native-windows.conf.json', 'capture.json', 'capture-ack.json', 'first-cargo.jsonl', 'first-app.bin', 'produced.json')),
            'producer owner is single use; prepare a fresh owner after a known failure')
    with ExitStack() as owned:
        stage = owned.enter_context(native_resource_stage(root, state))
        copies = copy_resource_inputs(root, state, stage, owned)
        proof = owner / 'resource-copies.json'
        with io_path(proof).open('xb') as stream:
            stream.write(encoded(copies)); stream.flush(); os.fsync(stream.fileno())
        config_path = owner / 'native-windows.conf.json'
        config_value = native_windows_config(root, stage)
        with io_path(config_path).open('xb') as stream:
            stream.write(encoded(config_value)); stream.flush(); os.fsync(stream.fileno())
        scope = {'parent': os.getpid(), 'parentBirth': process_birth(os.getpid()),
                 'statePath': os.environ['POLARIS_WINDOWS_PACKAGE_STATE'],
                 'stateFile': facts(os.environ['POLARIS_WINDOWS_PACKAGE_STATE']), 'script': facts(Path(__file__)),
                 'python': cargo_tool(os.sys.executable), 'config': facts(config_path), 'copies': facts(proof), 'deadline': time.monotonic() + 3600}
        key = os.urandom(32); runner = owner / 'runner.cmd'
        with io_path(runner).open('xb') as stream: stream.write(RUNNER_TEXT.encode('ascii'))
        runner_facts = facts(runner)
        env = dict(os.environ, **{CAPTURE_ENV: json.dumps({**scope, 'key': key.hex()}),
                   'POLARIS_PACKAGE_PYTHON': os.sys.executable, 'POLARIS_PACKAGE_SCRIPT': str(Path(__file__).absolute()),
                   'POLARIS_PACKAGE_STATE': os.environ['POLARIS_WINDOWS_PACKAGE_STATE']})
        require(all(not any(char in env[name] for char in '\r\n\x00"') for name in
                    ('POLARIS_PACKAGE_PYTHON', 'POLARIS_PACKAGE_SCRIPT', 'POLARIS_PACKAGE_STATE')), 'unsafe native runner path')
        require(not any(char in str(runner) for char in '%\r\n\x00"'), 'unsupported CMD runner path')
        argv = [cli['entry'], 'build', '--config', str(config_path), '--bundles', 'nsis', '--runner', str(runner)]
        if state['cargo']['target']: argv.extend(['--target', state['cargo']['target']])
        argv.extend(['--', '--locked', '--offline', '--message-format=json'])
        for frozen in (proof, runner, Path(__file__), Path(scope['statePath']), config_path, root / 'src-tauri/tauri.windows.conf.json'):
            owned.enter_context(hold_app(frozen, write_share=False))
        observed = []
        def ready():
            require(not observed and facts(runner) == runner_facts, 'duplicate or changed first producer')
            record = read_signed(owner / 'capture.json', key)
            require(record['scope'] == scope and record['stdout'] == facts(owner / 'first-cargo.jsonl'), 'foreign first producer scope/output')
            release_arguments(state, record['argv'][1:])
            require(record['argv'][0] == state['cargo']['tool']['path'], 'foreign Cargo executable')
            row, identity = release_event(root, state, io_path(regular(owner / 'first-cargo.jsonl')).read_text(encoding='utf-8'))
            require(row == record['artifact'] and list(identity) == record['identity'], 'first artifact differs')
            original = owned.enter_context(hold_app(owner / 'first-app.bin', write_share=False))
            original_bytes = original.read(MAX_FILE + 1)
            require(len(original_bytes) <= MAX_FILE and byte_facts(original_bytes) == record['firstApp'],
                    'original-byte-mismatch: authenticated first snapshot differs')
            held = owned.enter_context(duplicate_app(record['pid'], record['birth'], record['handle']))
            require(app_object_identity(app_identity(os.fstat(held.fileno()))) == app_object_identity(identity), 'transferred App differs')
            observed.append((record, held))
            signed_record(owner / 'capture-ack.json', {'scope': scope, 'capture': facts(owner / 'capture.json')}, key)
        run = run_tauri(root, argv, env, ready, scope['deadline'] - time.monotonic())
        require(len(observed) == 1 and run['exitCode'] == 0 and facts(runner) == runner_facts, 'Tauri did not finish one captured producer')
        record, held = observed[0]; require(load_state(root) == state and tauri_identity(root) == cli, 'producer inputs changed')
        verify_resource_copies(root, state, copies)
        path, identity = cargo_app_identity(root, state)
        require(identity[:2] == tuple(record['identity'][:2]) and
                app_object_identity(app_identity(os.fstat(held.fileno()))) == app_object_identity(identity), 'Tauri replaced the actual App')
        state['cargoArtifact'] = {'argv': record['argv'], 'artifact': record['artifact'], 'identity': list(identity),
                                  'firstApp': record['firstApp'], 'transport': record['transport'],
                                  'nativeResourceConfig': scope['config'], 'resourceCopies': copies, 'childConfig': record['childConfig'], 'tauri': run}
        with tempfile.TemporaryDirectory(prefix='compiled-app-', dir=io_path(owner)) as stage:
            app_path, app = compiled_app(root, staging=Path(stage), state=state)
            require(byte_facts(app) == record['firstApp'], 'original-byte-mismatch: Tauri did not restore the first Cargo App')
            assemble_carriers(root, state, app_path, app, path, identity, sources={name: row['path'] for name, row in copies['members'].items()})
            verify_resource_copies(root, state, copies)
        # This is a completed publication's readback binding, never an input
        # from which another producer or compiler attestation can be created.
        receipt = root / 'windows-package-evidence/static-carriers.json'
        with io_path(owner / 'produced.json').open('xb') as stream:
            stream.write(encoded({'schema': 'polaris-windows-produced-v1', 'candidate': state['candidate'],
                                  'receipt': facts(receipt), 'ownerIdentity': state['ownerIdentity']})); stream.flush(); os.fsync(stream.fileno())
        io_path(owner / 'produced.json').chmod(stat.S_IREAD)


def assemble(root):
    """Read back an already published first-producer result; never build or publish."""
    state = load_state(root, readback=True); produced = regular(Path(state['owner']) / 'produced.json')
    require(not io_path(produced).stat().st_mode & stat.S_IWRITE, 'mutable completed publication binding')
    completed = json.loads(io_path(produced).read_bytes())
    require(completed == {'schema': 'polaris-windows-produced-v1', 'candidate': state['candidate'],
                          'receipt': facts(root / 'windows-package-evidence/static-carriers.json'),
                          'ownerIdentity': state['ownerIdentity']}, 'foreign or modified publication readback')
    receipt = json.loads(io_path(regular(root / 'windows-package-evidence/static-carriers.json')).read_bytes())
    _, app = compiled_app(root, state=state); _, identity = cargo_app_identity(root, state)
    require(receipt['candidate'] == state['candidate'] and receipt['cargoContext'] == state['cargo']
            and receipt['sourceApp'] == byte_facts(app) and receipt['cargoArtifact']['identity'] == list(identity),
            'completed publication App/context changed')
    exact(tree(root / 'dist-win'), receipt['carriers'])
    portable = root / 'dist-win' / f"Polaris_{state['version']}_x64-win-Portable.zip"
    exact(zip_inventory(portable, carrier_inventory(state, app, 'zip')), receipt['portableObserved'])


def assemble_carriers(root, state, app_path, app, source_path, source_identity, *, sources=None):
    cli = tauri_identity(root)
    generated_script = verify_generated_script(root, state, sources=sources)
    raw = unique_installer(Path(state['cargo']['releaseDirectory']) / 'bundle/nsis', state['version'])
    artifact_before = facts(raw)
    with tempfile.TemporaryDirectory(prefix='nsis-', dir=io_path(state['owner'])) as tmp:
        observed = nsis_inventory(root, state['sevenzip'], raw, carrier_inventory(state, app, 'nsis'), Path(tmp))
    require(facts(raw) == artifact_before, 'NSIS changed during inspection')
    destination = root / 'dist-win'
    require(not any(io_path(destination).iterdir()), 'publication output namespace is not empty')
    installer = destination / f"Polaris_{state['version']}_x64-win-setup.exe"
    io_path(raw).rename(io_path(installer))
    portable = destination / f"Polaris_{state['version']}_x64-win-Portable.zip"
    build_portable(root, state, app, portable)
    load_state(root)
    require(facts(app_path) == byte_facts(app), 'App changed during carrier inspection')
    require(cargo_app_identity(root, state)[1] == source_identity, 'Cargo App changed during carrier inspection')
    require(byte_facts(compiled_app(root, state=state)[1]) == state['cargoArtifact']['firstApp']
            and cargo_app_identity(root, state)[1] == source_identity, 'original-byte-mismatch during carrier inspection')
    evidence = root / 'windows-package-evidence'; io_path(evidence).mkdir(exist_ok=True)
    receipt = {'schema': 'polaris-windows-static-carriers-v1', 'candidate': state['candidate'],
               'ownedStage': {'path': state['owner'], 'retainedPreviousOutputBytes': state['retainedPreviousOutputBytes'],
                              'retirement': 'preserved old outputs; fresh temporary extracts/staging removed; runner owns final retirement'},
               'sevenzip': {'path': state['sevenzip'], 'version': state['sevenzipVersion']},
               'sourceResources': state['resources'], 'sourceApp': byte_facts(app), 'generatedNSISScript': generated_script,
               'compiledAppInput': {'sourcePath': str(source_path), 'sourceIdentity': list(source_identity),
                                    'privateSingleLinkSnapshot': byte_facts(app)},
               'tauriCLI': cli, 'cargoContext': state['cargo'], 'cargoArtifact': state['cargoArtifact'],
               'nsisExpected': carrier_inventory(state, app, 'nsis'), 'nsisObserved': observed,
               'portableExpected': carrier_inventory(state, app, 'zip'),
               'portableObserved': zip_inventory(portable, carrier_inventory(state, app, 'zip')),
               'carriers': {p.name: facts(p) for p in [installer, portable]},
               'nativeInstallation': 'not performed by static carrier gate'}
    io_path(evidence / 'static-carriers.json').write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
    print('Actual final NSIS + portable complete member/SHA/size inventories verified')


def verify_existing_zip(root, archive):
    state = load_state(root, readback=True)
    _, app = compiled_app(root, state=state)
    _, identity = cargo_app_identity(root, state)
    receipt = json.loads(io_path(regular(root / 'windows-package-evidence/static-carriers.json')).read_bytes())
    require(receipt['candidate'] == state['candidate'] and receipt['cargoContext'] == state['cargo']
            and receipt['sourceApp'] == byte_facts(app)
            and receipt['cargoArtifact']['identity'] == list(identity),
            'Cargo release input changed after carrier assembly')
    return zip_inventory(archive, carrier_inventory(state, app, 'zip'))


def contract(root):
    """Read actual publication configuration; private source-shape audits live in tests."""
    conf = json.loads(io_path(regular(root / 'src-tauri/tauri.windows.conf.json')).read_bytes())
    resources = conf['bundle']['resources']; require_legal_resources(resources)
    required = ['core-manifest.json', '../resources/win/', '../resources/dashboard/', '../resources/data/']
    require(all(resources.count(name) == 1 for name in required), 'required Windows resources must occur once')
    require(len(resources) == len(set(resources)) and all(isinstance(name, str) and '*' not in name
            and (name == 'core-manifest.json' or name.startswith('../') and not name[3:].startswith('../'))
            for name in resources), 'explicit unique Windows resource mapping required')
    return {'schema': 'polaris-windows-package-config-v1', 'legalFiles': list(PUBLIC_LEGAL_FILES),
            'platform': 'win', 'resources': resources, 'formats': ['nsis', 'zip']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'produce', 'capture', 'assemble', 'contract'])
    parser.add_argument('--state', help='owned state printed by prepare; replaces the workflow environment lookup')
    parser.add_argument('--target', choices=['x86_64-pc-windows-msvc'], help='actual native Cargo build target used by Tauri')
    forwarded = []
    raw = os.sys.argv[1:]
    if '--' in raw:
        separator = raw.index('--'); forwarded = raw[separator + 1:]; raw = raw[:separator]
    args = parser.parse_args(raw)
    require(args.action == 'capture' or not forwarded, 'only the private Cargo runner accepts forwarded arguments')
    if args.state:
        require(args.action in ('produce', 'capture', 'assemble'), '--state requires an owned publication action')
        os.environ['POLARIS_WINDOWS_PACKAGE_STATE'] = args.state
    if args.target:
        require(args.action == 'prepare', '--target is only valid for prepare')
        os.environ['POLARIS_WINDOWS_PACKAGE_TARGET'] = args.target
    result = {'prepare': prepare, 'produce': produce, 'capture': capture, 'assemble': assemble, 'contract': contract}[args.action](ROOT)
    if args.action == 'contract': print(json.dumps(result, sort_keys=True))


if __name__ == '__main__': main()
