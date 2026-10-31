#!/usr/bin/env python3
"""Static Windows carrier integrity. Does not execute an installer or product PE.

The normal prepare/build chain owns identity. This gate freezes its exact resource
inputs before bundling and compares every final member with those same bytes.
"""
import argparse
import ast
import hashlib
import importlib.util
import json
import os
from pathlib import Path
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


def prepare(root):
    tool, version = find_sevenzip()
    inventory, sources = verified_sources(root)
    release = root / 'target/release'; io_path(release).mkdir(parents=True, exist_ok=True)
    # Never delete somebody else's output. Quarantine only these two fixed owned
    # workflow output namespaces, under a fresh same-filesystem temporary owner.
    owner = release / Path(tempfile.mkdtemp(prefix='.polaris-package-', dir=io_path(release))).name
    state = {'schema': 'polaris-windows-package-input-v1', 'owner': str(owner), 'root': str(root),
             'sevenzip': tool, 'sevenzipVersion': version, 'resources': inventory, 'sources': sources,
             'version': json.loads(io_path(root / 'src-tauri/tauri.conf.json').read_bytes())['version'],
             'candidate': subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()}
    retained_bytes = 0
    for name, path in [('nsis', release / 'bundle/nsis'), ('dist-win', root / 'dist-win')]:
        if io_path(path).exists() or io_path(path).is_symlink():
            retained = tree(path)  # reject links and bound retention before any move
            retained_bytes += sum(row['bytes'] for row in retained.values())
            require(retained_bytes <= MAX_TOTAL, 'combined old output retention budget exceeded')
            io_path(path).rename(io_path(owner / ('previous-' + name)))
        io_path(path).mkdir(parents=True)
    state['retainedPreviousOutputBytes'] = retained_bytes
    state_path = owner / 'state.json'
    io_path(state_path).write_text(json.dumps(state, indent=2) + '\n', encoding='utf-8')
    require(os.environ.get('GITHUB_ENV'), 'workflow environment output required')
    with open(io_path(os.environ['GITHUB_ENV']), 'a', encoding='utf-8') as env:
        env.write('POLARIS_WINDOWS_PACKAGE_STATE=' + str(state_path) + '\n')
    print('static NSIS reader:', tool, version, '; fresh output namespaces; frozen resources:', len(inventory))


def load_state(root):
    path = regular(os.environ['POLARIS_WINDOWS_PACKAGE_STATE'])
    state = json.loads(io_path(path).read_bytes())
    require(state['schema'] == 'polaris-windows-package-input-v1' and state['root'] == str(root)
            and path.parent == Path(state['owner']) and path.parent.parent == root / 'target/release'
            and path.parent.name.startswith('.polaris-package-'), 'foreign package state')
    current, _ = verified_sources(root)
    require(current == state['resources'], 'source resources changed after pre-build freeze')
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


def stream_child(argv, consume_stdout, stdout_limit, *, timeout, stderr_limit=MAX_STDERR):
    """Drain both pipes with bounded queues; the caller sees only quota-safe blocks.

    Threads only read pipes. The parent alone checks budgets/writes files. All
    failure paths terminate and wait for this exact Popen child before returning.
    A queue carries at most four 64KiB blocks, never an unbounded child output.
    """
    require(isinstance(stdout_limit, int) and stdout_limit >= 0 and timeout > 0,
            'invalid child output/deadline budget')
    deadline = time.monotonic() + timeout
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, bufsize=0)
    messages = queue.Queue(maxsize=4); stop = threading.Event()
    stderr = bytearray(); stdout_bytes = 0; ended = set()
    def publish(message):
        while not stop.is_set():
            try: messages.put(message, timeout=0.05); return
            except queue.Full: pass
    def read_pipe(label, pipe):
        try:
            while not stop.is_set():
                block = pipe.read(64 * 1024)
                if not block: break
                publish((label, block))
        except OSError as error:
            publish(('reader-error', str(error)))
        finally: publish((label, None))
    readers = [threading.Thread(target=read_pipe, args=(label, pipe), daemon=True)
               for label, pipe in [('stdout', process.stdout), ('stderr', process.stderr)]]
    try:
        for reader in readers: reader.start()
        while len(ended) < 2:
            remaining = deadline - time.monotonic()
            require(remaining > 0, 'owned child output deadline exceeded')
            try: label, block = messages.get(timeout=min(remaining, 0.05))
            except queue.Empty: continue
            require(label != 'reader-error', 'owned child pipe read failed: ' + str(block))
            if block is None: ended.add(label); continue
            if label == 'stderr':
                require(len(stderr) + len(block) <= stderr_limit, 'owned child stderr byte limit exceeded')
                stderr.extend(block)
            else:
                require(stdout_bytes + len(block) <= stdout_limit, 'owned child stdout byte limit exceeded')
                consume_stdout(block)
                stdout_bytes += len(block)
        remaining = deadline - time.monotonic()
        require(remaining > 0, 'owned child exit deadline exceeded')
        try: code = process.wait(timeout=remaining)
        except subprocess.TimeoutExpired as error: raise ValueError('owned child exit deadline exceeded') from error
        require(code == 0, 'owned child failed (' + str(code) + '): ' + stderr.decode('utf-8', errors='replace'))
        return {'stdoutBytes': stdout_bytes, 'stderrBytes': len(stderr), 'pid': process.pid,
                'exitCode': code, 'stderr': stderr.decode('utf-8', errors='replace')}
    finally:
        # Never address a guessed PID/group or leave the 7z child writing while
        # TemporaryDirectory removes its files. Popen owns this exact process.
        stop.set()
        if process.poll() is None:
            process.terminate()
            try: process.wait(timeout=2)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
        else: process.wait()
        for reader in readers:
            if reader.ident is not None: reader.join(timeout=2)
        process.stdout.close(); process.stderr.close()
        require(not any(reader.is_alive() for reader in readers), 'owned child pipe readers did not join')


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


def unpack_nsis(tool, archive, destination, allowed, *, budget=None, timeout=EXTRACT_TIMEOUT):
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
    require(sum(size if size is not None else allowed[name] for name, size in records) <= MAX_TOTAL,
            'NSIS expanded budget exceeded')
    if budget is None: budget = {'bytes': 0}
    require(not io_path(destination).exists() and io_path(destination.parent).is_dir(), 'new owned NSIS output directory required')
    io_path(destination).mkdir()
    actual = {}
    try:
        for name, size in records:
            value, _ = extract_nsis_member(tool, archive, name, destination / name, allowed[name], budget, deadline)
            require(size is None or value['bytes'] == size, 'extracted NSIS size differs')
            actual[name] = value
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
    actual, listing = unpack_nsis(tool, archive, scratch / 'installer', allowed, budget=budget)
    require(set(actual) <= set(allowed) and uninstaller in actual, 'unregistered NSIS installer member')
    exact({name: row for name, row in actual.items() if name in expected}, expected)
    require(cleaner in actual, 'installer cleaner missing')
    for name in set(actual) & set(auxiliary):
        require(actual[name] == auxiliary[name], 'NSIS plugin differs from selected toolchain: ' + name)
    nested_allowed = {name: row['bytes'] for name, row in auxiliary.items()}
    nested, nested_listing = unpack_nsis(tool, scratch / 'installer' / uninstaller, scratch / 'uninstaller', nested_allowed, budget=budget)
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


def verify_generated_script(root, state):
    script = regular(root / 'target/release/nsis/x64/installer.nsi')
    text = io_path(script).read_text(encoding='utf-8-sig')
    def define(name):
        values = re.findall(r'^!define ' + name + r' "([^"\r\n]+)"\s*$', text, re.MULTILINE)
        require(len(values) == 1, 'generated NSIS define must be unique: ' + name)
        return values[0]
    require(define('VERSION') == state['version'] and define('MAINBINARYNAME') == 'polaris', 'NSIS source/version differs')
    app_source, _ = cargo_app_identity(root)
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
        require(io_path(regular(source)).samefile(io_path(regular(state['sources'][name]))), 'NSIS File source differs: ' + name)
        mapped[name] = facts(source)
    exact(mapped, state['resources'])
    require(len(re.findall(r'^\s*File\b', section, re.MULTILINE)) == len(files) + 1, 'unparsed NSIS File instruction')
    return facts(script)


def cargo_app_identity(root):
    """Admit only the fixed Cargo output and its sole known deps hardlink."""
    app_path = root / 'target/release/polaris.exe'
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


def compiled_app(root, *, staging=None):
    app_path, identity = cargo_app_identity(root)
    with io_path(app_path).open('rb') as stream:
        fd_identity = app_identity(os.fstat(stream.fileno()))
        require(app_object_identity(fd_identity) == app_object_identity(identity), 'Cargo App replaced before read')
        app = stream.read(MAX_FILE + 1)
        require(app_identity(os.fstat(stream.fileno())) == fd_identity, 'Cargo App changed while reading')
    require(cargo_app_identity(root)[1] == identity and len(app) == identity[4], 'Cargo App path changed while reading')
    pin = io_path(regular(root / 'resources/win/core-manifest.json')).read_bytes()
    require(app.count(pin) == 1, 'App must embed the exact frozen selected manifest once')
    if staging is not None:
        # A fresh producer-owned single-link snapshot, never an audit copy or
        # deletion of Cargo's alias. The original file identity stays guarded.
        app_path = Path(staging) / 'polaris.exe'
        with io_path(app_path).open('xb') as stream:
            stream.write(app); stream.flush(); os.fsync(stream.fileno())
        require(facts(app_path) == byte_facts(app), 'private compiled App snapshot differs')
        require(cargo_app_identity(root)[1] == identity, 'Cargo App changed during private staging')
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
    return {'entry': executable, 'version': actual}


def assemble(root):
    state = load_state(root)
    source_path, source_identity = cargo_app_identity(root)
    with tempfile.TemporaryDirectory(prefix='compiled-app-', dir=io_path(state['owner'])) as stage:
        app_path, app = compiled_app(root, staging=Path(stage))
        require(cargo_app_identity(root)[1] == source_identity, 'Cargo input changed before inspection')
        assemble_carriers(root, state, app_path, app, source_path, source_identity)


def assemble_carriers(root, state, app_path, app, source_path, source_identity):
    cli = tauri_identity(root)
    generated_script = verify_generated_script(root, state)
    raw = unique_installer(root / 'target/release/bundle/nsis', state['version'])
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
    require(cargo_app_identity(root)[1] == source_identity, 'Cargo App changed during carrier inspection')
    evidence = root / 'windows-package-evidence'; io_path(evidence).mkdir(exist_ok=True)
    receipt = {'schema': 'polaris-windows-static-carriers-v1', 'candidate': state['candidate'],
               'ownedStage': {'path': state['owner'], 'retainedPreviousOutputBytes': state['retainedPreviousOutputBytes'],
                              'retirement': 'preserved old outputs; fresh temporary extracts/staging removed; runner owns final retirement'},
               'sevenzip': {'path': state['sevenzip'], 'version': state['sevenzipVersion']},
               'sourceResources': state['resources'], 'sourceApp': byte_facts(app), 'generatedNSISScript': generated_script,
               'compiledAppInput': {'sourcePath': str(source_path), 'sourceIdentity': list(source_identity),
                                    'privateSingleLinkSnapshot': byte_facts(app)},
               'tauriCLI': cli,
               'nsisExpected': carrier_inventory(state, app, 'nsis'), 'nsisObserved': observed,
               'portableExpected': carrier_inventory(state, app, 'zip'),
               'portableObserved': zip_inventory(portable, carrier_inventory(state, app, 'zip')),
               'carriers': {p.name: facts(p) for p in [installer, portable]},
               'nativeInstallation': 'not performed by static carrier gate'}
    io_path(evidence / 'static-carriers.json').write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
    print('Actual final NSIS + portable complete member/SHA/size inventories verified')


def verify_existing_zip(root, archive):
    resources, _ = verified_sources(root)
    _, app = compiled_app(root)
    return zip_inventory(archive, carrier_inventory({'resources': resources}, app, 'zip'))


def contract(root, *, source_text=None, workflow_text=None):
    """Static confs consumer: actual workflow/config and actual function edges.

    This reads no downloaded/native resources and executes no carrier/tool. The
    fixture suite separately exercises these functions with real inert ZIP bytes.
    Only eager function bodies count: a call hidden in a new if/loop/try does not
    satisfy a mandatory pipeline edge.
    """
    require(PUBLIC_LEGAL_FILES == ('LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'), 'legal file registry differs')
    conf = json.loads(io_path(regular(root / 'src-tauri/tauri.windows.conf.json')).read_bytes())
    require_legal_resources(conf['bundle']['resources'])
    source_text = io_path(__file__).read_text(encoding='utf-8') if source_text is None else source_text
    workflow_text = io_path(root / '.github/workflows/package.yml').read_text(encoding='utf-8') if workflow_text is None else workflow_text
    module = ast.parse(source_text)
    definitions = {node.name: node for node in module.body if isinstance(node, ast.FunctionDef)}
    def transfers(node, kinds):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
            return False  # A nested scope does not transfer control in this body.
        return isinstance(node, kinds) or any(transfers(child, kinds) for child in ast.iter_child_nodes(node))
    def direct(nodes, *, loop_body=False):
        for node in nodes:
            yield node
            if isinstance(node, ast.With):
                # These existing stdlib contexts propagate failed verification;
                # arbitrary contexts (notably suppress) cannot make a gate eager.
                contexts = {'tempfile.TemporaryDirectory', 'zipfile.ZipFile', 'open'}
                if all(isinstance(item.context_expr, ast.Call)
                       and ast.unparse(item.context_expr.func) in contexts for item in node.items):
                    yield from direct(node.body, loop_body=loop_body)
            exits = (ast.Return, ast.Break, ast.Continue) if loop_body else (ast.Return,)
            if isinstance(node, (ast.Raise, ast.Return, ast.Break, ast.Continue)) or transfers(node, exits):
                break  # Do not count calls after even a conditional early exit.
    def expressions(node):
        if node is None or isinstance(node, (ast.Lambda, ast.GeneratorExp, ast.ListComp, ast.SetComp, ast.DictComp)):
            return  # Deferred or iteration-dependent bodies are not mandatory.
        if isinstance(node, ast.BoolOp):
            yield from expressions(node.values[0])  # Only the first operand is unconditional.
        elif isinstance(node, ast.IfExp):
            yield from expressions(node.test)
        elif isinstance(node, ast.Compare):
            yield from expressions(node.left)
            yield from expressions(node.comparators[0])  # Chained later comparisons short-circuit.
        else:
            if isinstance(node, ast.Call):
                yield node
            # Dict.keys/values and List/Tuple.elts are AST lists, not mapping
            # methods. iter_child_nodes also skips None keys in dict unpacking.
            for child in ast.iter_child_nodes(node):
                yield from expressions(child)
    def eager(nodes, *, loop_body=False):
        for node in direct(nodes, loop_body=loop_body):
            if isinstance(node, (ast.Assign, ast.AnnAssign, ast.Expr, ast.Return)):
                yield from expressions(node.value)
    def calls(name):
        require(name in definitions, 'package contract function missing: ' + name)
        definition = definitions[name]
        require(not definition.decorator_list and not any(transfers(node, (ast.Yield, ast.YieldFrom)) for node in definition.body),
                'mandatory package function must execute eagerly: ' + name)
        return list(eager(definition.body))
    required_edges = {
        'source_inventory': {'require_legal_resources'},
        'verified_sources': {'source_inventory'},
        'prepare': {'verified_sources', 'find_sevenzip'},
        'load_state': {'verified_sources'},
        'assemble': {'load_state', 'cargo_app_identity', 'compiled_app', 'assemble_carriers'},
        'assemble_carriers': {'tauri_identity', 'nsis_inventory', 'carrier_inventory', 'build_portable', 'zip_inventory', 'cargo_app_identity'},
        'tauri_identity': {'tauri_command'},
        'build_portable': {'carrier_inventory', 'exact', 'zip_inventory'},
        'zip_inventory': {'exact'},
    }
    for owner, targets in required_edges.items():
        actual = {node.func.id for node in calls(owner) if isinstance(node.func, ast.Name)}
        require(targets <= actual, 'mandatory package call missing from ' + owner + ': ' + str(sorted(targets - actual)))
    build = definitions['build_portable']
    source_loops = [node for node in direct(build.body) if isinstance(node, ast.For)
                    and ast.dump(node.iter) == ast.dump(ast.parse("state['sources'].items()", mode='eval').body)]
    require(len(source_loops) == 1, 'portable must copy the complete frozen source map once')
    require(ast.dump(source_loops[0].target) == ast.dump(ast.parse('for name, source in (): pass').body[0].target),
            'portable source-map name/source binding differs')
    copy_calls = [node for node in eager(source_loops[0].body, loop_body=True) if isinstance(node.func, ast.Attribute)
                  and isinstance(node.func.value, ast.Name) and node.func.value.id == 'shutil' and node.func.attr == 'copyfile']
    require(len(copy_calls) == 1 and ast.dump(copy_calls[0]) == ast.dump(ast.parse('shutil.copyfile(io_path(regular(source)), io_path(path))', mode='eval').body),
            'portable actual source-map copy differs')
    zip_exact = [node for node in calls('zip_inventory') if isinstance(node.func, ast.Name) and node.func.id == 'exact']
    require(len(zip_exact) == 1 and ast.dump(zip_exact[0]) == ast.dump(ast.parse('exact(actual, expected)', mode='eval').body),
            'actual ZIP must compare the complete observed and expected member maps')
    main = definitions.get('main')
    require(main is not None, 'package action entrypoint missing')
    dispatch = []
    for node in calls('main'):
        if not (isinstance(node.func, ast.Subscript) and isinstance(node.func.value, ast.Dict)
                and ast.dump(node.func.slice) == ast.dump(ast.parse('args.action', mode='eval').body)
                and len(node.args) == 1 and ast.dump(node.args[0]) == ast.dump(ast.parse('ROOT', mode='eval').body)
                and not node.keywords):
            continue
        literal = node.func.value
        if (all(isinstance(key, ast.Constant) and isinstance(key.value, str) for key in literal.keys)
                and all(isinstance(value, ast.Name) for value in literal.values)):
            mapping = {key.value: value.id for key, value in zip(literal.keys, literal.values)}
            if len(mapping) == len(literal.keys):  # Reject duplicate action keys, not just their last value.
                dispatch.append(mapping)
    require(dispatch == [{'prepare': 'prepare', 'assemble': 'assemble', 'contract': 'contract'}],
            'actual eager package action dispatch differs')
    def step(name):
        pattern = r'^      - name: ' + re.escape(name) + r'\n(.*?)(?=^      - (?:name:|uses:)|\Z)'
        found = re.findall(pattern, workflow_text, re.MULTILINE | re.DOTALL)
        require(len(found) == 1, 'unique actual Windows workflow step required: ' + name)
        return found[0]
    sequence = [('Freeze Windows package inputs and isolate outputs', 'prepare'),
                ('Verify final NSIS payload and build exact Windows portable zip', 'assemble')]
    positions = []
    for name, action in sequence:
        block = step(name)
        executable = '\n'.join(line for line in block.splitlines() if line.strip() and not line.lstrip().startswith('#'))
        require(re.findall(r'^        if: (.+)$', executable, re.MULTILINE) == ["runner.os == 'Windows'"],
                'ordinary Windows package step must be mandatory: ' + name)
        require('continue-on-error' not in executable and '||' not in executable, 'Windows package failure suppression')
        command = 'python scripts/windows-package-integrity.py ' + action
        lines = [line.strip() for line in executable.splitlines()]
        require(lines.count(command) == 1, 'actual Windows package command missing: ' + command)
        at = lines.index(command)
        require(at + 1 < len(lines) and re.fullmatch(r"if \(\$LASTEXITCODE -ne 0\) \{ throw '[^']+' \}", lines[at + 1]),
                'Windows package command must propagate its exit code')
        positions.append(workflow_text.index('      - name: ' + name + '\n'))
    build_at = workflow_text.index('      - name: Build installers\n')
    assets_at = workflow_text.index('run: node scripts/verify-packaging.mjs assets --label windows --dir dist-win')
    require(positions[0] < build_at < positions[1] < assets_at, 'freeze/build/actual-carrier/assets order differs')
    return {'schema': 'polaris-windows-package-contract-v1', 'legalFiles': list(PUBLIC_LEGAL_FILES),
            'ordinaryWindowsCommands': [action for _, action in sequence],
            'actualZipComparison': 'exact(actual, expected)', 'requiredCallEdges': {name: sorted(edges) for name, edges in required_edges.items()}}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'assemble', 'contract'])
    args = parser.parse_args()
    result = {'prepare': prepare, 'assemble': assemble, 'contract': contract}[args.action](ROOT)
    if args.action == 'contract': print(json.dumps(result, sort_keys=True))


if __name__ == '__main__': main()
