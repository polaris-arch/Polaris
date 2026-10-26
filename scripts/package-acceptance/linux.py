#!/usr/bin/env python3
"""Run the frozen Linux carriers, not a recompiled App or browser mock.

`inspect` only extracts bytes. `run` is an exact-source GitHub Linux VM gate.
Only fixture setup uses config_save; every user action uses native WebDriver.
No root, FUSE, system installation, helper, TUN or sandbox workarounds.
"""
import argparse
import select
import base64
import copy
import ctypes
import hashlib
import importlib.util
import ipaddress
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import sys
import struct
import subprocess
import tempfile
import threading
import time
import http.client

SCHEMA = 'polaris-linux-package-acceptance-v1'
CASES = ('default_dashboard', 'gui_selection', 'gui_on_off',
         'gui_manual_disconnect_preference', 'gui_auto_preserves_existing',
         'gui_apply_tcp_dns', 'gui_durability_status_retry',
         'gui_rejected_preserves_old', 'rpc_contract', 'same_home_restart',
         'tray_quit', 'cleanup')
ROOT = Path(__file__).resolve().parents[2]
ELEMENT = 'element-6066-11e4-a52e-4f735466cecf'
FORBIDDEN = ('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS',
             'WEBKIT_FORCE_SANDBOX', 'LIBGL_ALWAYS_SOFTWARE',
             'WEBKIT_DISABLE_COMPOSITING_MODE', 'WEBKIT_DISABLE_DMABUF_RENDERER', 'GDK_BACKEND',
             'POLARIS_SINGBOX_PATH', 'POLARIS_NO_KERNEL_RUN',
             'POLARIS_NATIVE_PAYLOAD_VALIDATION', 'POLARIS_NATIVE_DNS_CANDIDATE')


def require(ok, message):
    if not ok:
        raise ValueError(message)


def sha(path):
    h = hashlib.sha256()
    with open(path, 'rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def regular(path, within=None):
    path = Path(path).absolute()
    require(path.is_file() and not path.is_symlink(), 'regular file required')
    require(all(not parent.is_symlink() for parent in path.parents), 'linked ancestor')
    if within is not None:
        require(path.is_relative_to(Path(within).resolve()), 'path outside owned directory')
    return path



def read_saved_preference(home, xdg_config):
    """Read only the real private App config, never invoke config_get/migration."""
    home = Path(home).absolute()
    require(Path(xdg_config).absolute() == home / 'config', 'private config root differs')
    require(all(not parent.is_symlink() for parent in (home, *home.parents)), 'linked config root')
    opened = []
    def hold(fd, location):
        opened.append((fd, location, None))
        info = os.fstat(fd)
        opened[-1] = (fd, location, info)
        return info
    try:
        current = home
        fd = os.open(home, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        hold(fd, current)
        for name in ('config', 'com.polaris.app', 'polaris'):
            fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            current = current / name
            hold(fd, current)
        require(all(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() for _, _, info in opened),
                'private config directory owner differs')
        fd = os.open('config.json', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        path = current / 'config.json'; before = hold(fd, path)
        require(stat.S_ISREG(before.st_mode) and before.st_uid == os.getuid() and
                0 < before.st_size <= 4 * 1024 * 1024, 'bounded private config required')
        chunks = []; remaining = before.st_size + 1
        while remaining:
            chunk = os.read(fd, min(65536, remaining))
            if not chunk: break
            chunks.append(chunk); remaining -= len(chunk)
        raw = b''.join(chunks); after = os.fstat(fd)
        fields = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns', 'st_uid', 'st_mode')
        require(len(raw) == before.st_size and all(getattr(before,k) == getattr(after,k) for k in fields),
                'config changed during read')
        require(all(not parent.is_symlink() for parent in (home, *home.parents)), 'linked config root changed')
        for _, location, original in opened:
            now = location.lstat()
            require(not stat.S_ISLNK(now.st_mode) and (now.st_dev,now.st_ino,now.st_uid) ==
                    (original.st_dev,original.st_ino,original.st_uid), 'config path changed during read')
        value = json.loads(raw)
        require(type(value) is dict and type(value.get('interruptConnectionsOnSwitch')) is bool,
                'saved preference boolean unavailable')
        return {'source': 'private-app-config-file', 'value': value['interruptConnectionsOnSwitch'],
                'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
    finally:
        reading_failed = sys.exc_info()[0] is not None
        close_failed = False
        for fd, _, _ in reversed(opened):
            try: os.close(fd)
            except OSError: close_failed = True
        if close_failed and not reading_failed:
            raise ValueError('private config descriptor close unconfirmed')

def byte_identity(path, root=None):
    path = regular(path, root)
    return {'path': str(path.relative_to(root)) if root else path.name,
            'bytes': path.stat().st_size, 'sha256': sha(path)}


def squashfs_offset(path):
    # Same bounded superblock predicate as the original final-package proof.
    data = regular(path).read_bytes()
    offsets, start = [], 0
    while True:
        at = data.find(b'hsqs', start)
        if at < 0:
            break
        start = at + 4
        if at + 96 > len(data):
            continue
        inodes, block = struct.unpack_from('<I4xI', data, at + 4)
        major, minor = struct.unpack_from('<HH', data, at + 28)
        used = struct.unpack_from('<Q', data, at + 40)[0]
        if inodes and block in [1 << n for n in range(12, 21)] and (major, minor) == (4, 0) and 96 <= used <= len(data) - at:
            offsets.append(at)
    require(len(offsets) == 1, 'unique final SquashFS required')
    return offsets[0]


def extract(carrier, kind, destination):
    require(kind in ('deb', 'AppImage') and not destination.exists(), 'fresh carrier destination required')
    require(os.geteuid() != 0, 'nonroot extraction required')
    carrier = regular(carrier)
    if kind == 'deb':
        command = ['dpkg-deb', '--extract', str(carrier), str(destination)]
    else:
        command = ['unsquashfs', '-no-progress', '-offset', str(squashfs_offset(carrier)),
                   '-dest', str(destination), str(carrier)]
    subprocess.run(command, check=True, timeout=180, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    members = {}
    for role, name in (('core', 'sing-box'), ('cronet', 'libcronet.so')):
        found = list(destination.rglob(name))
        require(len(found) == 1, 'unique carrier payload member required')
        members[role] = byte_identity(found[0], destination)
    require(Path(members['core']['path']).parent == Path(members['cronet']['path']).parent,
            'split core/Cronet payload')
    app = destination / 'usr/bin/polaris'
    require(regular(app, destination).read_bytes()[:4] == b'\x7fELF', 'actual packaged App ELF required')
    members['app'] = byte_identity(app, destination)
    for role in ('app', 'core'):
        require(os.access(destination / members[role]['path'], os.X_OK), 'packaged executable mode missing')
    entry = app
    if kind == 'AppImage':
        entry = destination / 'AppRun'
        require(entry.exists() and entry.resolve().is_relative_to(destination.resolve())
                and os.access(entry, os.X_OK), 'original AppRun missing or outside AppDir')
        # Preserve original launcher and hooks; do not invoke usr/bin/polaris instead.
        require(any(destination.glob('apprun-hooks/*'))
                or any(destination.glob('usr/lib/*/apprun-hooks/*')),
                'original AppRun hooks missing')
    return members, entry


def inspect_carrier(path, kind):
    path = regular(path)
    before = byte_identity(path)
    with tempfile.TemporaryDirectory(prefix='polaris-package-inspect-') as temporary:
        members, _ = extract(path, kind, Path(temporary) / 'payload')
    require(byte_identity(path) == before, 'carrier changed during inspection')
    return {'kind': kind, 'name': path.name, 'bytes': before['bytes'],
            'sha256': before['sha256'], 'members': members}



def stable_runtime_capture(path, home, identity, destination, *, include_content=False):
    """Keep exact original bytes privately; never put the API secret in artifacts."""
    path = regular(path, home)
    started = time.time_ns()
    require(same_process(identity, process_identity(identity['pid'])), 'runtime reader lost owned core')
    before = path.stat()
    require(0 < before.st_size <= 8 * 1024 ** 2, 'runtime JSON bound')
    first = path.read_bytes()
    middle = path.stat()
    second = path.read_bytes()
    after = path.stat()
    signature = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns, item.st_ctime_ns)
    require(signature(before) == signature(middle) == signature(after) and first == second
            and same_process(identity, process_identity(identity['pid'])), 'unstable actual runtime read')
    content = json.loads(first)
    apis = [item for item in content.get('services', []) if item.get('type') == 'api']
    require(len(apis) == 1 and apis[0].get('dashboard', {}).get('enabled') is True,
            'generated runtime lost default dashboard')
    require(apis[0]['dashboard'].get('http_client', {}).get('detour') == content.get('route', {}).get('final'),
            'actual dashboard HTTP client differs from original final route')
    proof = {'sha256': hashlib.sha256(first).hexdigest(), 'bytes': len(first),
             'pathSha256': hashlib.sha256(str(path).encode()).hexdigest(),
             'pid': identity['pid'], 'startTime': identity['startTime'],
             'readStartedUnixNs': str(started), 'readFinishedUnixNs': str(time.time_ns()),
             'stableDoubleRead': True, 'sameInodeAndMetadata': True,
             'dashboardEnabled': True, 'dashboardDetourMatchesFinal': True,
             'originalBytesRetention': 'private ephemeral runner only; not uploaded or cross-VM archived'}
    destination.mkdir(mode=0o700, exist_ok=True)
    archive = destination / (str(started) + '-' + proof['sha256'] + '.json')
    fd = os.open(archive, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as stream:
        json.dump({'evidence': proof, 'originalBytesBase64': base64.b64encode(first).decode()}, stream)
    return (proof, content) if include_content else proof


def bind_api_endpoint(graph, status, secret, core):
    """Pure contract binding. Returned proof contains no configuration/secret."""
    require(isinstance(graph, dict) and isinstance(status, dict), 'API binding inputs unavailable')
    require(isinstance(graph.get('services'), list), 'API service inventory invalid')
    apis = [item for item in graph['services'] if isinstance(item, dict) and item.get('type') == 'api']
    require(len(apis) == 1 and apis[0].get('listen') == '127.0.0.1', 'unique loopback API required')
    port = apis[0].get('listen_port')
    require(type(port) is int and 0 < port < 65536, 'actual API port invalid')
    require(type(status.get('pid')) is int and status['pid'] == core['pid']
            and status.get('running') is True and status.get('starting', False) is False, 'API status/core differs')
    require(type(status.get('clashApiPort')) is int and status['clashApiPort'] == port,
            'actual API/status port differs')
    require(isinstance(secret, str) and len(secret) >= 16 and apis[0].get('secret') == secret,
            'actual API secret differs')
    return {'host': '127.0.0.1', 'port': port, 'pid': core['pid'], 'startTime': core['startTime']}


def safe_connect_status(status):
    if not isinstance(status, dict):
        return {}
    safe = {key: status[key] for key in ('running', 'starting') if type(status.get(key)) is bool}
    safe.update({key: status[key] for key in ('pid', 'mainGeneration', 'mixedPort', 'clashApiPort')
                 if type(status.get(key)) is int and 0 <= status[key] < 2 ** 64})
    return safe


CONNECT_PHASES = ('apply-ready', 'stopped-check', 'gui-click', 'status-ready', 'core-birth',
                  'runtime-path', 'runtime-api', 'owned-api-probe', 'cronet-map', 'receipt')


def clean_exit_marker(path, started):
    path = regular(path)
    info = path.stat()
    require(info.st_uid == os.getuid() and info.st_size == 0 and started - 1_000_000_000 <= info.st_mtime_ns <= time.time_ns() + 1_000_000_000,
            'fresh original clean-exit commit marker missing')
    return {'name': path.name, 'bytes': 0, 'mtimeUnixNs': str(info.st_mtime_ns),
            'sha256': sha(path), 'createdAfterShortcut': True, 'timestampResolutionAllowanceNs': 1_000_000_000, 'writer': 'original desktop exit commit'}


def config_version(value):
    # Exact configBaseVersion: sorted JSON keys, FNV-1a over UTF-16 units.
    text = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
    units = text.encode('utf-16-le')
    result = 0x811c9dc5
    for offset in range(0, len(units), 2):
        result = ((result ^ int.from_bytes(units[offset:offset + 2], 'little')) * 0x01000193) & 0xffffffff
    return f'{result:08x}'


def private_env(home):
    home = Path(home).absolute()
    require(os.geteuid() != 0 and not any(name in os.environ for name in FORBIDDEN),
            'nonroot default sandbox environment required')
    # No inherited user HOME, tokens, proxies, loader overrides or runner secrets.
    env = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'HOME': str(home),
           'LANG': 'C.UTF-8', 'LC_ALL': 'C.UTF-8',
           'TAURI_WEBVIEW_AUTOMATION': 'true'}
    for key, name in (('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                      ('XDG_CACHE_HOME', 'cache'), ('XDG_STATE_HOME', 'state'),
                      ('XDG_RUNTIME_DIR', 'runtime')):
        directory = home / name
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        env[key] = str(directory)
    return env


def process_identity(pid):
    try:
        path = Path('/proc') / str(pid)
        text = (path / 'stat').read_text()
        rest = text[text.rindex(')') + 2:].split()
        return {'pid': int(pid), 'startTime': rest[19], 'ppid': int(rest[1]),
                'state': rest[0], 'uid': path.stat().st_uid}
    except (FileNotFoundError, ProcessLookupError):
        return None


def validate_app_identity(value, *, captured=False):
    """Validate only this ordinary App evidence contract, not scan/signal authority."""
    valid = (type(value) is dict and type(value.get('pid')) is int and value['pid'] > 1
        and type(value.get('startTime')) is str and value['startTime'].isdigit() and int(value['startTime']) > 0
        and type(value.get('ppid')) is int and value['ppid'] >= 0
        and type(value.get('uid')) is int and value['uid'] >= 0
        and type(value.get('state')) is str and len(value['state']) == 1)
    gui_require(valid, 'app-identity-schema-invalid')
    if captured:
        gui_require(all(type(value.get(k)) is int and value[k] >= 0 for k in ('exeDev', 'exeInode'))
                    and value['exeInode'] > 0, 'app-identity-schema-invalid')
    return value


def observe_app_executable(app):
    """Fresh executable metadata, fenced by the original ordinary App birth."""
    validate_app_identity(app)
    try:
        before = process_identity(app['pid'])
        gui_require(before is not None, 'app-identity-changed')
        validate_app_identity(before)
        gui_require(same_process(app, before) and before['uid'] == app['uid'] == os.getuid()
                    and before['state'] != 'Z', 'app-identity-changed')
        path = Path('/proc') / str(app['pid']) / 'exe'
        info = path.stat()
        after = process_identity(app['pid'])
        gui_require(after is not None, 'app-identity-changed')
        validate_app_identity(after)
        gui_require(same_process(before, after) and after['uid'] == app['uid']
                    and after['state'] != 'Z', 'app-identity-changed')
        current = path.stat()
        final = process_identity(app['pid'])
        gui_require(final is not None, 'app-identity-changed')
        validate_app_identity(final)
        fields = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns', 'st_mode')
        gui_require(stat.S_ISREG(info.st_mode) and same_process(before, after) and same_process(after, final)
                    and after['uid'] == final['uid'] == app['uid'] and after['state'] != 'Z' and final['state'] != 'Z'
                    and all(getattr(info,k) == getattr(current,k) for k in fields), 'app-identity-changed')
        return current
    except OSError:
        raise GuiFailure('app-executable-unreadable') from None


def app_execution_identity(app):
    """Bind already-owned App birth to its admitted bytes and opened executable."""
    validate_app_identity(app)
    gui_require(type(app.get('sha256')) is str and re.fullmatch(r'[0-9a-f]{64}', app['sha256']) is not None
                and type(app.get('exe')) is str and Path(app['exe']).is_absolute(), 'app-identity-schema-invalid')
    try:
        before = observe_app_executable(app)
        path = Path('/proc') / str(app['pid']) / 'exe'
        fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            gui_require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= 128 * 1024 * 1024
                        and (info.st_dev,info.st_ino) == (before.st_dev,before.st_ino), 'app-identity-changed')
            digest = hashlib.sha256(); remaining = info.st_size
            while remaining:
                chunk = stream.read(min(1024 * 1024, remaining))
                gui_require(bool(chunk), 'app-identity-changed')
                digest.update(chunk); remaining -= len(chunk)
            gui_require(not stream.read(1), 'app-identity-changed')
            final = os.fstat(stream.fileno())
        after = observe_app_executable(app)
        fields = ('st_dev','st_ino','st_size','st_mtime_ns','st_ctime_ns')
        gui_require(all(getattr(info,k) == getattr(final,k) == getattr(after,k) for k in fields)
                    and digest.hexdigest() == app['sha256'], 'app-identity-changed')
        result = {**app, 'exeDev': info.st_dev, 'exeInode': info.st_ino}
        if 'exeDev' in app or 'exeInode' in app:
            validate_app_identity(app, captured=True)
            gui_require(all(result[k] == app[k] for k in ('exeDev','exeInode')), 'app-identity-changed')
        return result
    except OSError:
        raise GuiFailure('app-executable-unreadable') from None


DIAGNOSTIC_EXE_LIMIT = 64 * 1024 * 1024
DIAGNOSTIC_ANCESTORS = 8


def diagnostic_error(error):
    # Do not export exception text, argv, environment, or arbitrary paths.
    return {'state': 'unknown', 'errno': error.errno if isinstance(error, OSError) else None}


def executable_diagnostic(item, budget):
    """Optional executable facts, not identity or signal authorization."""
    path = Path('/proc') / str(item['pid']) / 'exe'
    result = {'state': 'unknown'}
    deadline = time.monotonic() + 1
    diagnostic_stage = 'birth-check'
    try:
        if not same_process(item, process_identity(item['pid'])):
            return {'state': 'unstable'}
        diagnostic_stage = 'executable-link'
        link = os.readlink(path)
        require(0 < len(link) <= 4096 and link.startswith('/') and
                all(ord(char) >= 32 and ord(char) != 127 for char in link), 'executable link bound')
        result['link'] = link
        diagnostic_stage = 'executable-open'
        with path.open('rb') as stream:
            before = os.fstat(stream.fileno())
            require(stat.S_ISREG(before.st_mode), 'executable must be regular')
            result.update(device=before.st_dev, inode=before.st_ino, bytes=before.st_size,
                          mode=stat.S_IMODE(before.st_mode), ownerUid=before.st_uid, ownerGid=before.st_gid)
            diagnostic_stage = 'executable-byte-budget'
            require(0 < before.st_size <= min(DIAGNOSTIC_EXE_LIMIT, budget[0]), 'executable diagnostic byte budget')
            digest = hashlib.sha256()
            total = 0
            while total < before.st_size:
                diagnostic_stage = 'executable-time-budget'
                require(time.monotonic() <= deadline, 'executable diagnostic deadline')
                block = stream.read(min(65536, before.st_size - total))
                require(block, 'executable diagnostic short read')
                total += len(block)
                budget[0] -= len(block)
                digest.update(block)
                require(time.monotonic() <= deadline, 'executable diagnostic deadline')
            diagnostic_stage = 'executable-stability'
            require(not stream.read(1), 'executable diagnostic grew')
            after = os.fstat(stream.fileno())
            signature = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
            require(signature(before) == signature(after), 'executable diagnostic changed')
            # An exec can replace the executable without changing process birth.
            current = path.stat()
            require(signature(current) == signature(before) and os.readlink(path) == link,
                    'executable diagnostic exec transition')
        if not same_process(item, process_identity(item['pid'])):
            return {'state': 'unstable'}
        result.update(state='observed-not-authorization', sha256=digest.hexdigest())
    except (OSError, ValueError) as error:
        # Do not retain partial path/metadata across failed birth/exec checks.
        result = diagnostic_error(error)
        result['failureStage'] = diagnostic_stage
    return result


def process_diagnostic(item, *, executable_budget=None):
    """Bounded public kernel facts only; never grants signal authority."""
    result = {key: item.get(key) for key in ('pid', 'startTime', 'ppid', 'uid')}
    result['observation'] = 'unknown'
    budget = executable_budget if executable_budget is not None else [DIAGNOSTIC_EXE_LIMIT]
    base = Path('/proc') / str(item['pid'])
    try:
        before = process_identity(item['pid'])
        if not same_process(item, before):
            return result
        with (base / 'status').open() as stream:
            status = stream.read(16385)
        require(len(status) <= 16384, 'status diagnostic bound')
        fields = dict(line.split(':', 1) for line in status.splitlines() if ':' in line)
        name = fields.get('Name', '').strip()
        result['nameHint'] = name if re.fullmatch(r'[A-Za-z0-9_.+() -]{1,64}', name) else 'unknown'
        result['nameHintScope'] = 'mutable-kernel-name-not-executable-identity'
        for name, count, pattern in (('Uid', 4, r'[0-9]+'), ('Gid', 4, r'[0-9]+'),
                                      ('CapEff', 1, r'[0-9a-fA-F]{1,16}'),
                                      ('CapPrm', 1, r'[0-9a-fA-F]{1,16}'),
                                      ('CapAmb', 1, r'[0-9a-fA-F]{1,16}'),
                                      ('NoNewPrivs', 1, r'[01]'), ('Seccomp', 1, r'[012]')):
            values = fields.get(name, '').split()
            result[name] = values if len(values) == count and all(re.fullmatch(pattern, v) for v in values) else 'unknown'
        for namespace in ('user', 'mnt', 'pid'):
            try:
                value = os.readlink(base / 'ns' / namespace)
                result[namespace + 'Namespace'] = value if re.fullmatch(namespace + r':\[[0-9]+\]', value) else 'unknown'
            except OSError as error:
                result[namespace + 'Namespace'] = 'unknown'
                result[namespace + 'NamespaceError'] = diagnostic_error(error)
        try:
            with (base / 'uid_map').open() as stream:
                mapping = stream.read(1025)
            rows = [line.split() for line in mapping.splitlines()]
            result['uidMap'] = rows if len(mapping) <= 1024 and 0 < len(rows) <= 16 and all(len(row) == 3 and all(re.fullmatch(r'[0-9]+', v) for v in row) for row in rows) else 'unknown'
        except OSError as error:
            result['uidMap'] = 'unknown'
            result['uidMapError'] = diagnostic_error(error)
        result['executable'] = executable_diagnostic(item, budget)
        if not same_process(item, process_identity(item['pid'])):
            return {**{key: item.get(key) for key in ('pid', 'startTime', 'ppid', 'uid')}, 'observation': 'unstable'}
        result['observation'] = 'same-birth-snapshot-not-authorization'
    except (OSError, ValueError) as error:
        result['observation'] = 'unknown'
        result['error'] = diagnostic_error(error)
    return result


def cleanup_workspace(directory, *, processes_stopped=False):
    """Never recurse into an active portal mount or repair its permissions."""
    directory = Path(directory).absolute()
    if not processes_stopped:
        return {'state': 'failed', 'reason': 'workspace-process-exit-unconfirmed'}
    try:
        for line in Path('/proc/self/mountinfo').read_text().splitlines():
            fields = line.split()
            require(len(fields) >= 10 and '-' in fields, 'mount inventory malformed')
            mount = Path(re.sub(r'\\([0-7]{3})', lambda match: chr(int(match[1], 8)), fields[4]))
            if mount == directory or mount.is_relative_to(directory):
                return {'state': 'failed', 'reason': 'workspace-mount-remains'}
        # No onerror chmod retry, umount, privilege change, or link traversal.
        shutil.rmtree(directory)
        return {'state': 'normal'}
    except (OSError, ValueError) as error:
        return {'state': 'failed', 'reason': 'workspace-cleanup-unconfirmed', 'type': type(error).__name__}


def same_process(left, right):
    return bool(left and right and left['pid'] == right['pid'] and left['startTime'] == right['startTime'])


def parse_proc_socket(line, ipv6=False):
    fields = line.split()
    if len(fields) < 10 or ':' not in fields[0]:
        return None
    try:
        address, port = fields[2].split(':')
        raw = bytes.fromhex(address)
        if ipv6:
            require(len(raw) == 16, 'IPv6 socket address size')
            raw = b''.join(raw[index:index + 4][::-1] for index in range(0, 16, 4))
        else:
            require(len(raw) == 4, 'IPv4 socket address size')
            raw = raw[::-1]
        remote = ipaddress.ip_address(raw)
        return {'inode': fields[9], 'remoteIp': str(remote), 'remotePort': int(port, 16),
                'state': fields[3], 'loopback': remote.is_loopback,
                'unspecified': remote.is_unspecified}
    except (ValueError, IndexError):
        return None


def cronet_mapping(maps, expected, expected_sha):
    paths = set()
    for line in maps.splitlines():
        fields = line.split(None, 5)
        if len(fields) != 6 or not fields[5].startswith('/'):
            continue
        name = re.sub(r'\\([0-7]{3})', lambda match: chr(int(match[1], 8)), fields[5])
        if Path(name.removesuffix(' (deleted)')).name != 'libcronet.so':
            continue
        require(not name.endswith(' (deleted)'), 'deleted Cronet mapping')
        paths.add(name)
    expected = regular(expected)
    require(sha(expected) == expected_sha, 'packaged Cronet changed')
    require(not paths or paths == {str(expected)}, 'foreign or duplicate Cronet mapping')
    return {'sha256': expected_sha,
            'mapState': 'mapped-exact-packaged-library' if paths else 'not-mapped-in-socks-scenario',
            'mappedPath': str(expected) if paths else None,
            'scope': 'SOCKS GUI is not Cronet request acceptance; original final receipt proves native load'}


class Supervisor:
    """Per-process subreaper; signals require remembered PID, starttime and lineage."""
    def __init__(self):
        require(os.geteuid() != 0, 'nonroot supervisor required')
        libc = ctypes.CDLL(None, use_errno=True)
        require(libc.prctl(36, 1, 0, 0, 0) == 0, 'subreaper unavailable')
        self.self = process_identity(os.getpid())
        self.owned, self.children, self.roles = {}, {}, {}
        self.lock = threading.RLock()
        self.stop = threading.Event()
        self.unknown = []
        self.diagnostics = []
        self.diagnostics_truncated = False
        self.diagnostic_executable_budget = [DIAGNOSTIC_EXE_LIMIT]
        self.forced = []
        self.network = {}
        self.network_errors = set()
        self.network_truncated = False
        self.network_next = 0.0
        self.thread = threading.Thread(target=self._monitor, daemon=True)
        self.thread.start()

    def _monitor(self):
        while not self.stop.wait(0.05):
            try:
                observed = self.scan()
                if time.monotonic() >= self.network_next:
                    self.observe_sockets(observed)
                    self.network_next = time.monotonic() + 0.5
            except Exception:
                if 'process-inventory-unreadable' not in self.unknown:
                    self.unknown.append('process-inventory-unreadable')

    def record_diagnostic(self, code, item, parent=None, *, stage=None):
        try:
            self._record_diagnostic(code, item, parent, stage=stage)
        except Exception:
            # Diagnostic failure never substitutes for the original rejection.
            if 'identity-diagnostic-unavailable' not in self.unknown:
                self.unknown.append('identity-diagnostic-unavailable')

    def _record_diagnostic(self, code, item, parent=None, *, stage=None):
        """Shared by plain/TUN rejection paths; observation never grants ownership."""
        with self.lock:
            # Labels are fixed call-site classifications, not raw errors or argv.
            label = lambda value: value if isinstance(value, str) and re.fullmatch(r'[a-z][a-z0-9-]{0,63}', value) else 'unknown'
            key = (code, item['pid'], item['startTime'])
            if any((row['code'], row['process']['pid'], row['process']['startTime']) == key for row in self.diagnostics):
                return
            if len(self.diagnostics) >= 64:
                self.diagnostics_truncated = True
                return
            budget = self.diagnostic_executable_budget
            def facts(identity):
                remembered = same_process(identity, self.owned.get(identity['pid']))
                row = process_diagnostic(identity, executable_budget=budget)
                confirmed = row.get('observation') == 'same-birth-snapshot-not-authorization'
                row['role'] = label(self.roles.get(identity['pid'])) if remembered and confirmed else 'unknown'
                row['directSpawnRole'] = row['role'] if remembered and identity['pid'] in self.children else 'unknown'
                return row
            row = {'code': label(code), 'stage': label(stage or 'process-inventory'),
                   'process': facts(item), 'ancestors': [], 'ancestryState': 'unknown'}
            if parent is None:
                parent = self.owned.get(item.get('ppid'))
            if parent is not None:
                row['parent'] = {key: parent.get(key) for key in ('pid', 'startTime', 'uid', 'ppid')}
                row['parent'].update(observation='inventory-birth-not-revalidated', role='unknown', directSpawnRole='unknown')
            seen = {(item['pid'], item['startTime'])}
            for _ in range(DIAGNOSTIC_ANCESTORS):
                if parent is None:
                    break
                birth = (parent['pid'], parent['startTime'])
                if birth in seen:
                    row['ancestryState'] = 'cycle'
                    break
                seen.add(birth)
                try:
                    current = process_identity(parent['pid'])
                except (OSError, ValueError):
                    current = None
                if not same_process(parent, current) or not same_process(parent, self.owned.get(parent['pid'])):
                    row['ancestryState'] = 'unconfirmed-birth'
                    break
                fact = facts(current)
                row['ancestors'].append(fact)
                if fact.get('observation') != 'same-birth-snapshot-not-authorization':
                    row['ancestryState'] = 'unconfirmed-snapshot'
                    break
                if len(row['ancestors']) == 1:
                    row['parent'] = fact
                    row['parentRole'] = fact['role']
                if parent['pid'] in self.children:
                    row['ancestryState'] = 'direct-spawn-reached'
                    break
                parent = self.owned.get(current.get('ppid'))
            else:
                row['ancestryState'] = 'depth-limit'
            row.setdefault('parentRole', 'unknown')
            row['executableByteBudgetRemaining'] = budget[0]
            self.diagnostics.append(row)

    def scan(self):
        observed = {int(p.name): process_identity(int(p.name)) for p in Path('/proc').iterdir() if p.name.isdigit()}
        observed = {pid: item for pid, item in observed.items() if item}
        with self.lock:
            changed = True
            while changed:
                changed = False
                for pid, item in observed.items():
                    if same_process(item, self.owned.get(pid)):
                        if item['uid'] != os.getuid() or self.owned[pid]['uid'] != os.getuid():
                            self.record_diagnostic('owned-birth-uid-mismatch', item)
                        require(item['uid'] == os.getuid() and self.owned[pid]['uid'] == os.getuid(),
                                'owned birth changed UID unexpectedly')
                        continue
                    parent = observed.get(item['ppid'])
                    if same_process(parent, self.owned.get(item['ppid'])):
                        if item['uid'] != os.getuid():
                            self.record_diagnostic('owned-descendant-uid-mismatch', item, parent)
                        require(item['uid'] == os.getuid(), 'owned descendant elevated unexpectedly')
                        self.owned[pid] = item
                        self.roles[pid] = self.roles.get(item['ppid'], 'unknown')
                        changed = True
                    elif item['ppid'] == os.getpid() and pid not in self.children:
                        # An adopted descendant was not observed before reparenting.
                        # It is ours to reap, but unknown lineage must fail the gate.
                        self.record_diagnostic('unobserved-adopted-descendant', item)
                        self.unknown.append('unobserved-adopted-descendant')
                        self.owned[pid] = item
                        self.roles[pid] = 'unknown'
            # Correct children first seen before their App ancestor was identified.
            for _ in range(len(self.owned) + 1):
                changed = False
                for pid, item in observed.items():
                    if same_process(item, self.owned.get(pid)) and self.roles.get(item['ppid']) == 'product' and self.roles.get(pid) != 'product':
                        self.roles[pid] = 'product'
                        changed = True
                if not changed:
                    break
            return observed

    def observe_sockets(self, observed):
        for pid, identity in list(self.owned.items()):
            if not same_process(identity, observed.get(pid)):
                continue
            base = Path('/proc') / str(pid)
            try:
                inodes = set()
                for entry in (base / 'fd').iterdir():
                    try:
                        link = os.readlink(entry)
                    except FileNotFoundError:
                        continue
                    if link.startswith('socket:[') and link.endswith(']'):
                        inodes.add(link[8:-1])
                for protocol in ('tcp', 'tcp6', 'udp', 'udp6'):
                    for line in (base / 'net' / protocol).read_text().splitlines()[1:]:
                        item = parse_proc_socket(line, protocol.endswith('6'))
                        if not item or item['inode'] not in inodes or item['unspecified'] or item['remotePort'] == 0:
                            continue
                        key = (pid, identity['startTime'], protocol, item['remoteIp'], item['remotePort'], item['state'])
                        if key not in self.network:
                            if len(self.network) >= 4096:
                                self.network_truncated = True
                                continue
                            self.network[key] = {'pid': pid, 'startTime': identity['startTime'],
                                'protocol': protocol, 'remoteIp': item['remoteIp'], 'remotePort': item['remotePort'],
                                'state': item['state'], 'loopback': item['loopback'], 'observations': 0}
                        self.network[key]['observations'] += 1
            except (FileNotFoundError, ProcessLookupError):
                continue
            except (PermissionError, OSError):
                self.network_errors.add('owned-socket-read-unavailable')

    def network_evidence(self):
        return {'observation': 'owned socket snapshots, not packet capture; short-lived traffic can be missed',
                'endpoints': list(self.network.values()), 'truncated': self.network_truncated,
                'readErrors': sorted(self.network_errors),
                'observedNonloopback': sum(not item['loopback'] for item in self.network.values()),
                'zeroEgressClaim': False}

    def mark_product(self, item):
        with self.lock:
            self.roles[item['pid']] = 'product'
            # Retain an AppRun shell ancestor, but not the native driver itself.
            parent = self.owned.get(item['ppid'])
            while parent and parent['pid'] not in self.children:
                try:
                    name = (Path('/proc') / str(parent['pid']) / 'exe').resolve().name
                except OSError:
                    break
                if name == 'WebKitWebDriver':
                    break
                self.roles[parent['pid']] = 'product'
                parent = self.owned.get(parent['ppid'])
        self.scan()

    def spawn(self, command, env, role, *, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, pass_fds=()):
        with self.lock:
            child = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL,
                                     stdout=stdout, stderr=stderr,
                                     start_new_session=True, pass_fds=pass_fds)
            # Keep the Popen handle before fallible /proc capture. Handle
            # custody alone grants neither lineage nor signal authority.
            self.children[child.pid] = child
            try:
                item = process_identity(child.pid)
                require(item is not None, 'spawned process vanished before identity capture')
                self.owned[child.pid] = item
                self.roles[child.pid] = role
                return child
            except BaseException:
                self.unknown.append('spawn-identity-unconfirmed')
                try:
                    if child.poll() is not None:
                        child.wait(timeout=0)
                except (OSError, subprocess.TimeoutExpired):
                    self.unknown.append('spawn-reap-unconfirmed')
                finally:
                    for stream in (child.stdout, child.stderr):
                        if stream is not None:
                            try:
                                stream.close()
                            except Exception:
                                self.unknown.append('spawn-pipe-close-unconfirmed')
                raise

    def query(self, command, env, role='tool-probe', *, timeout=15, limit=16384):
        """Registered direct child; bounded bytes/time; no output in exceptions."""
        require(0 < timeout <= 15 and 0 < limit <= 16384, 'tool probe bounds differ')
        child = self.spawn(command, env, role, stdout=subprocess.PIPE)
        deadline, output = time.monotonic() + timeout, bytearray()
        try:
            while True:
                remaining = deadline - time.monotonic()
                require(remaining > 0, 'tool probe timed out')
                require(select.select([child.stdout], [], [], remaining)[0], 'tool probe timed out')
                block = os.read(child.stdout.fileno(), min(4096, limit + 1 - len(output)))
                if not block:
                    break
                output.extend(block)
                require(len(output) <= limit, 'tool probe output bound')
            require(child.wait(timeout=max(0, deadline - time.monotonic())) == 0, 'tool probe failed')
            return output.decode('utf-8', errors='strict')
        except BaseException:
            # Only our registered same-birth/same-UID child is signalable. A
            # failed identity check remains unknown, never a name exemption.
            if child.poll() is None:
                self.forced.append({'pid': child.pid, 'reason': 'tool-probe-aborted'})
                try:
                    self.signal(self.owned[child.pid], signal.SIGKILL)
                except (OSError, ValueError, KeyError):
                    self.unknown.append('tool-probe-signal-unconfirmed')
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.unknown.append('tool-probe-wait-timeout')
            raise
        finally:
            child.stdout.close()

    def living(self, role=None):
        observed = self.scan()
        with self.lock:
            return [item for pid, item in observed.items() if same_process(item, self.owned.get(pid))
                    and item['state'] != 'Z' and (role is None or self.roles.get(pid) == role)]

    def executable(self, expected):
        expected = regular(expected)
        matches = []
        for item in self.living():
            try:
                exe = Path('/proc') / str(item['pid']) / 'exe'
                if exe.resolve() == expected:
                    require(sha(exe) == sha(expected), 'executing bytes drifted')
                    matches.append({**item, 'sha256': sha(expected), 'exe': str(expected)})
            except (FileNotFoundError, ProcessLookupError):
                continue
        require(len(matches) == 1, 'exactly one owned executing payload required')
        self.mark_product(matches[0])
        return matches[0]

    def signal(self, item, sig):
        current = process_identity(item['pid'])
        require(item.get('uid') == os.getuid() and (current or {}).get('uid') == os.getuid() and
                same_process(item, self.owned.get(item['pid'])) and same_process(item, current),
                'signal identity/lineage/UID changed')
        # pidfd pins the kernel task; a PID cannot be recycled between identity
        # verification and delivery. Same birth cannot authorize a UID transition.
        require(hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal'), 'pidfd signaling required')
        fd = os.pidfd_open(item['pid'], 0)
        try:
            current = process_identity(item['pid'])
            require(item.get('uid') == os.getuid() and (current or {}).get('uid') == os.getuid() and
                    same_process(item, self.owned.get(item['pid'])) and same_process(item, current),
                    'pidfd identity/lineage/UID changed')
            signal.pidfd_send_signal(fd, sig, None, 0)
        finally:
            os.close(fd)

    def close(self, normal):
        cleanup = {'state': 'failed', 'forced': self.forced, 'remaining': [],
                   'remainingAssessment': 'unknown', 'unknown': []}
        try:
            cleanup = self._close_owned(normal)
            cleanup['remainingAssessment'] = 'observed'
        except Exception as error:
            self.unknown.append('cleanup-inventory-unconfirmed')
            cleanup['failure'] = {'stage': 'supervisor-close', 'type': type(error).__name__}
            # An inventory refusal must not prevent the exact direct session
            # bus from receiving TERM. This does not authorize any descendant,
            # especially a privileged helper, or change the failed assessment.
            for pid, child in list(self.children.items()):
                if self.roles.get(pid) != 'dbus' or child.poll() is not None:
                    continue
                try:
                    self.signal(self.owned[pid], signal.SIGTERM)
                    child.wait(timeout=5)
                except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
                    self.unknown.append('private-session-close-unconfirmed')
        finally:
            self.stop.set()
            self.thread.join(2)
            if self.thread.is_alive():
                self.unknown.append('monitor-stop-unconfirmed')
                cleanup['remainingAssessment'] = 'unknown'
            cleanup['unknown'] = sorted(set(self.unknown))
            cleanup['identityDiagnostics'] = getattr(self, 'diagnostics', [])
            cleanup['identityDiagnosticsTruncated'] = getattr(self, 'diagnostics_truncated', False)
            if cleanup['unknown']:
                cleanup['state'] = 'failed'
        return cleanup

    def _close_owned(self, normal):
        # Product must already have exited through Ctrl+Q. Tools may receive TERM.
        product = self.living('product')
        if product:
            normal = False
            self.forced.extend({'pid': p['pid'], 'startTime': p['startTime'], 'reason': 'product-not-exited'} for p in product)
        for item in reversed(self.living()):
            try:
                self.signal(item, signal.SIGTERM)
            except ProcessLookupError:
                pass
            except (ValueError, OSError):
                self.unknown.append('pidfd-signal-identity-unconfirmed')
        until = time.monotonic() + 8
        while self.living() and time.monotonic() < until:
            time.sleep(0.1)
        for item in reversed(self.living()):
            self.forced.append({'pid': item['pid'], 'startTime': item['startTime'], 'reason': 'forced-kill'})
            try:
                self.signal(item, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except (ValueError, OSError):
                self.unknown.append('pidfd-signal-identity-unconfirmed')
        wait_deadline = time.monotonic() + 5
        for child in self.children.values():
            try:
                child.wait(timeout=max(0, wait_deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                self.unknown.append('child-wait-timeout')
        while True:
            try:
                if os.waitpid(-1, os.WNOHANG)[0] == 0:
                    break
            except ChildProcessError:
                break
        remaining = [{'pid': p['pid'], 'startTime': p['startTime']} for p in self.living()]
        return {'state': 'normal' if normal and not self.forced and not remaining and not self.unknown else 'failed',
                'forced': self.forced, 'remaining': remaining, 'unknown': sorted(set(self.unknown))}


def free_port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def wait_for(read, condition=bool, timeout=30, interval=0.2):
    until = time.monotonic() + timeout
    while True:
        value = read()
        if condition(value):
            return value
        gui_require(time.monotonic() < until, 'observation-timeout')
        time.sleep(interval)


OBSERVER = r'''
const internals = window.__TAURI_INTERNALS__;
const originalInvoke = internals?.invoke;
const invokeDescriptor = Object.getOwnPropertyDescriptor(internals ?? {}, 'invoke');
const mapDescriptor = Object.getOwnPropertyDescriptor(internals ?? {}, 'callbacks');
const callbacks = mapDescriptor?.value;
if (typeof originalInvoke !== 'function' || invokeDescriptor?.writable !== false ||
    invokeDescriptor?.configurable !== false || mapDescriptor?.writable !== false || mapDescriptor?.configurable !== false ||
    !(callbacks instanceof Map) || !Object.isExtensible(callbacks) ||
    Object.hasOwn(callbacks, 'get') || window.__packageObservation) throw Error('original callback observation unavailable');
const observation = {rows: [], cursor: 0, failed: false};
const get = Map.prototype.get;
const states = new Set(['hotApplied','hotDurabilityPending','unconfirmed','restartRequired','rejected','conflict','skipped','applied']);
const observe = function(id) {
  const callback = Reflect.apply(get, callbacks, [id]);
  if (typeof callback !== 'function') return callback;
  return function(...args) {
    try {
      const raw = args[0], data = raw?.data;
      const event = raw?.payload;
      if (raw?.event === 'event:proxyLifecycle' &&
          ['ready','stopped','failed'].includes(event?.phase) &&
          ['start','stop','restart','unknown'].includes(event?.kind) &&
          Number.isSafeInteger(event?.mainGeneration) && event.mainGeneration > 0 &&
          typeof event?.terminal === 'boolean') {
        observation.rows.push({sequence:++observation.cursor,source:'original-lifecycle-event',
          phase:event.phase,kind:event.kind,mainGeneration:event.mainGeneration,terminal:event.terminal,
          applyId:Number.isSafeInteger(event.applyId)&&event.applyId>0?event.applyId:null});
        if(observation.rows.length>512) observation.rows.shift();
      }
      // NativeAutoReceipt is shape-only: status polling lacks intentApplied.
      // Mode actions add applyOutcome; selecting a default/retest does not.
      // Absence is a serialization fact, never a fabricated pending/applied result.
      if (data && typeof data==='object' && Object.hasOwn(data,'intentApplied')) {
        const status=data.status, hasOutcome=status && typeof status==='object' && Object.hasOwn(status,'applyOutcome');
        const valid=typeof raw?.success==='boolean' && typeof data.intentApplied==='boolean' &&
          status && typeof status==='object' && !Array.isArray(status) &&
          typeof status.featureEnabled==='boolean' && Number.isSafeInteger(status.generation) && status.generation>=0 &&
          Array.isArray(status.nativeAutoGroups) && Array.isArray(status.nativeMeasurements) &&
          (status.actualServerId===null || typeof status.actualServerId==='string') &&
          (!hasOutcome || ['saved','applied','restarting','pending','deferred'].includes(status.applyOutcome));
        observation.rows.push(valid ? {sequence:++observation.cursor,
          source:hasOutcome?'original-auto-mode-callback':'original-auto-intent-compatible-callback',
          success:raw.success,intentApplied:data.intentApplied,featureEnabled:status.featureEnabled,
          generation:status.generation,...(hasOutcome?{applyOutcome:status.applyOutcome}:{})}
          : {sequence:++observation.cursor,source:'original-auto-receipt-invalid'});
        if(observation.rows.length>512) observation.rows.shift();
      }
      // SaveOutcome remains shape-only. The UI click window and independent
      // private-file read prove this preference without replaying a write/read IPC.
      if (data?.status === 'saved' || data?.status === 'conflict') {
        observation.rows.push({sequence: ++observation.cursor,
          source:'original-config-save-callback', success:raw?.success === true,
          status:data.status, versionValid:typeof data.version==='string' && /^[0-9a-f]{8}$/.test(data.version),
          preference:typeof data.config?.interruptConnectionsOnSwitch==='boolean'?data.config.interruptConnectionsOnSwitch:null});
        if (observation.rows.length > 512) observation.rows.shift();
      }
      // Callback replies do not identify commands. Capture only the original
      // RuntimeApplyResult shape, then correlate a click cursor with real status.
      if (states.has(data?.status) && typeof data.ok === 'boolean' &&
          typeof data.pendingApply === 'boolean' && typeof data.restartRequired === 'boolean' &&
          Array.isArray(data.reasons)) {
        observation.rows.push({sequence: ++observation.cursor,
          source:'original-native-callback', success:raw?.success === true,
          status:data.status, ok:data.ok, transactionId:data.transactionId,
          ...(Number.isSafeInteger(data.applyId)&&data.applyId>0?{applyId:data.applyId}:{}),
          runtimeGeneration:data.runtimeGeneration, selectionIntentGeneration:data.selectionIntentGeneration,
          persistenceMode:data.persistenceMode, restartRequired:data.restartRequired});
        if (observation.rows.length > 512) observation.rows.shift();
      }
    } catch (_) { observation.failed = true; }
    return Reflect.apply(callback, this, args);
  };
};
Object.defineProperty(callbacks, 'get', {value: observe, configurable: true});
Object.defineProperty(window, '__packageObservation', {value: observation});
if (internals.invoke !== originalInvoke || callbacks.get !== observe) throw Error('callback observation not installed');
return true;
'''


WEBDRIVER_ERRORS = frozenset(('element click intercepted', 'element not interactable', 'stale element reference',
    'no such element', 'no such window', 'invalid session id', 'javascript error', 'script timeout',
    'timeout', 'unexpected alert open', 'unknown error'))


class GuiFailure(ValueError):
    """Fixed non-secret failure classification, never a remote message."""
    def __init__(self, code, http_status=None, protocol=None):
        super().__init__('bounded GUI contract failed')
        self.code = code
        self.http_status = http_status
        self.protocol = protocol if type(protocol) is str and protocol in WEBDRIVER_ERRORS else 'unknown'


def gui_failure(error):
    result = {'code': error.code if isinstance(error, GuiFailure) else 'native-command-rejected' if isinstance(error, NativeRejected) else 'unclassified-contract-failure'}
    if isinstance(error, GuiFailure) and type(error.http_status) is int:
        result['httpStatus'] = error.http_status
    if isinstance(error, GuiFailure) and error.code.startswith('webdriver-'):
        result['protocol'] = error.protocol
    if isinstance(error, NativeRejected):
        result['nativeDiagnostic'] = error.diagnostic
    return result


def gui_require(ok, code):
    if not ok:
        raise GuiFailure(code)


class NativeRejected(ValueError):
    """Actual native failure envelope, not timeout or unknown outcome."""
    def __init__(self, message, diagnostic='unknown'):
        super().__init__(message)
        self.diagnostic = diagnostic if diagnostic in ('saved-state-unavailable', 'proxy-status-unavailable') + STATUS_BUSY_DIAGNOSTICS + STATUS_HARD_DIAGNOSTICS else 'unknown'



STATUS_BUSY_DIAGNOSTICS = ('status-writer-busy', 'status-cache-busy')
STATUS_HARD_DIAGNOSTICS = ('status-writer-poisoned', 'status-cache-poisoned', 'status-cache-cold')
NATIVE_BRIDGE_SCHEMA = 'polaris-native-read-bridge-v1'
NATIVE_BRIDGE_JS = r'''
const [command, args, done] = arguments;
const finish=(state,data=null,diagnostic='none')=>done({schema:'polaris-native-read-bridge-v1',state,data,diagnostic});
if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function') {finish('unavailable');return;}
try {
 const pending=window.__TAURI_INTERNALS__.invoke(command,args);
 if (!pending || typeof pending.then !== 'function') {finish('invalid-envelope');return;}
 pending.then(raw => {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw) || typeof raw.success !== 'boolean') finish('invalid-envelope');
  else if (raw.success === false) {
   let diagnostic='unknown';
   if(command==='proxy_get_status') {
    const codes={PROXY_STATUS_WRITER_BUSY:'status-writer-busy',PROXY_STATUS_CACHE_BUSY:'status-cache-busy',
     PROXY_STATUS_WRITER_POISONED:'status-writer-poisoned',PROXY_STATUS_CACHE_POISONED:'status-cache-poisoned',
     PROXY_STATUS_CACHE_COLD:'status-cache-cold',PROXY_STATUS_UNAVAILABLE:'proxy-status-unavailable'};
    if(typeof raw.code==='string' && Object.hasOwn(codes,raw.code) && raw.data==null) diagnostic=codes[raw.code];
    else if(raw.error==='config validation failed: saved commit unavailable') diagnostic='saved-state-unavailable';
    else if(raw.error==='proxy status unavailable') diagnostic='proxy-status-unavailable';
   }
   finish('envelope-rejected',null,diagnostic);
  }
  else finish('returned',raw.data ?? null);
 },()=>finish('promise-rejected'));
} catch (_) {finish('invoke-threw');}
'''


def native_bridge_state(value, command=None):
    if (type(value) is not dict or set(value) != {'schema', 'state', 'data', 'diagnostic'} or
            value.get('schema') != NATIVE_BRIDGE_SCHEMA or
            value.get('state') not in ('returned', 'unavailable', 'invalid-envelope',
                                      'envelope-rejected', 'promise-rejected', 'invoke-threw') or
            (value.get('state') != 'returned' and value.get('data') is not None) or
            value.get('diagnostic') not in (('unknown', 'saved-state-unavailable', 'proxy-status-unavailable') + STATUS_BUSY_DIAGNOSTICS + STATUS_HARD_DIAGNOSTICS
                if value.get('state') == 'envelope-rejected' else ('none',))):
        return 'invalid-bridge'
    if value['diagnostic'] in ('saved-state-unavailable', 'proxy-status-unavailable') + STATUS_BUSY_DIAGNOSTICS + STATUS_HARD_DIAGNOSTICS and command != 'proxy_get_status':
        return 'invalid-bridge'
    return value['state']


def preference_restart_terminal(rows, apply_id, previous_generation):
    gui_require(type(previous_generation) is int and previous_generation>0, 'preference-restart-generation-invalid')
    floor=previous_generation
    for row in rows:
        gui_require(set(row)=={'sequence','source','phase','kind','mainGeneration','terminal','applyId'} and
                    type(row['mainGeneration']) is int and 0<row['mainGeneration']<=2**53-1 and
                    type(row['sequence']) is int and row['sequence']>0 and
                    type(row['terminal']) is bool and row['phase'] in ('ready','stopped','failed') and
                    row['kind'] in ('start','stop','restart','unknown') and
                    (row['applyId'] is None or type(row['applyId']) is int and 0<row['applyId']<=2**53-1),
                    'preference-lifecycle-schema-invalid')
        floor=max(floor,row['mainGeneration'])
        if row['kind']=='stop' and row['terminal'] and row['phase']=='stopped' and row['mainGeneration']>previous_generation:
            raise GuiFailure('preference-restart-cancelled')
    owned=[r for r in rows if r['applyId']==apply_id and r['kind']=='restart' and r['terminal'] and
           r['mainGeneration']==floor and r['mainGeneration']>previous_generation]
    gui_require(not any(r['phase']=='failed' for r in owned),'preference-restart-failed')
    return next((r for r in reversed(owned) if r['phase']=='ready'),None)


class WebDriver:
    def __init__(self, port):
        require(type(port) is int and 0 < port < 65536, 'native driver port required')
        self.port = port
        self.session = None

    def request(self, method, path, data=None, timeout=25):
        # Fixed route/command vocabulary only: never retain scripts, arguments,
        # session IDs, response bodies, remote messages or transport URLs.
        routes = ('/timeouts', '/execute/sync', '/execute/async', '/screenshot', '/actions', '/element')
        route = path if path in ('/status', '/session') else next(
            (suffix for suffix in routes if self.session and path == '/session/' + self.session + suffix), 'other')
        if route == 'other' and self.session and path.startswith(self.path('/element/')) and path.endswith('/click'):
            route = '/element/:element/click'
        command = getattr(self, '_observed_command', None)
        if command not in ('config_get', 'config_save', 'proxy_get_status', 'proxy_runtime_apply_status', 'auto_select_status'):
            command = 'none'
        row = {'targetRole': 'tauri-driver', 'method': method if method in ('GET', 'POST', 'DELETE') else 'other',
               'route': route, 'command': command, 'state': 'sent', 'callback': 'not-observed'}
        observation = getattr(self, 'observation', None)
        if observation is None:
            observation = self.observation = {'schema': 'polaris-webdriver-observation-v1', 'requests': [], 'truncated': False}
        if len(observation['requests']) < 128:
            observation['requests'].append(row)
        else:
            observation['truncated'] = True
        started = time.monotonic()
        self._http_status = None
        try:
            value = self._request(method, path, data, timeout)
            row['state'] = 'decoded'
            if route == '/execute/async' and command != 'none':
                row['callback'] = 'returned'
                row['nativeBridge'] = native_bridge_state(value, command)
                if row['nativeBridge'] == 'envelope-rejected':
                    row['nativeDiagnostic'] = value['diagnostic']
                if row['nativeBridge'] != 'returned':
                    observation.setdefault('firstNativeFailure', row)
            return value
        except Exception as error:
            row['state'] = 'failed'
            allowed = ('RemoteDisconnected', 'ConnectionResetError', 'ConnectionRefusedError',
                       'BrokenPipeError', 'TimeoutError', 'URLError', 'HTTPError', 'GuiFailure',
                       'JSONDecodeError', 'IncompleteRead', 'BadStatusLine')
            row['error'] = type(error).__name__ if type(error).__name__ in allowed else 'other'
            if isinstance(error, GuiFailure):
                row.update(gui_failure(error))
            observation.setdefault('firstFailure', row)
            raise
        finally:
            if type(self._http_status) is int and 100 <= self._http_status <= 599:
                row['httpStatus'] = self._http_status
            row['elapsedMs'] = max(0, int((time.monotonic() - started) * 1000))

    def _request(self, method, path, data=None, timeout=25):
        require(type(timeout) in (int, float) and 0 < timeout < float('inf'), 'finite request timeout required')
        raw = None if data is None else json.dumps(data).encode()
        deadline = time.monotonic() + timeout
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=timeout)
        response = None
        wire = None
        def budget(touch_wire=True):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('native driver request budget exhausted')
            connection.timeout = remaining
            if touch_wire and wire is not None:
                wire.settimeout(remaining)
        try:
            budget()
            # Fresh direct connection, no proxy, pooling, redirect or replay.
            # Do not propagate urllib's hop-by-hop Connection:close upstream.
            connection.request(method, path, body=raw, headers={'Content-Type': 'application/json'})
            wire = connection.sock
            budget()
            response = connection.getresponse()
            self._http_status = response.status
            budget(False)
            chunks = []
            error_remaining = None if 200 <= response.status < 300 else 16385
            while not response.isclosed() and error_remaining != 0:
                budget()
                chunk = response.read1(65536 if error_remaining is None else min(65536, error_remaining))
                if not chunk:
                    break
                chunks.append(chunk)
                if error_remaining is not None:
                    error_remaining -= len(chunk)
            budget(False)
            if error_remaining is None and response.length not in (None, 0):
                raise http.client.IncompleteRead(b'', response.length)
            body = b''.join(chunks)
            if error_remaining is not None:
                protocol = None
                try:
                    if len(body) <= 16384:
                        protocol = json.loads(body).get('value', {}).get('error')
                except (ValueError, AttributeError, TypeError):
                    pass
                raise GuiFailure('webdriver-http-error', response.status, protocol)
            value = json.loads(body).get('value')
            budget(False)
            if isinstance(value, dict) and 'error' in value:
                raise GuiFailure('webdriver-protocol-error', protocol=value.get('error'))
            return value
        finally:
            # getresponse may detach a closing socket from HTTPConnection.
            # Close both holders even when parsing/protocol/budget checks fail.
            try:
                if response is not None:
                    response.close()
            finally:
                connection.close()
        # Socket timeouts bound individual blocking operations. Header parsing
        # inside getresponse is stdlib-owned; elapsed checks reject late returns,
        # not a claim of a preemptive hard wall-clock bound for slow-drip headers.

    def start(self, entry):
        value = self.request('POST', '/session', {'capabilities': {'alwaysMatch': {
            'tauri:options': {'application': str(entry)}}}}, timeout=90)
        require(isinstance(value, dict) and value.get('sessionId'), 'native session missing')
        self.session = value['sessionId']
        self.request('POST', self.path('/timeouts'), {'script': 20000, 'pageLoad': 60000, 'implicit': 0})
        wait_for(lambda: self.execute("return typeof window.__TAURI_INTERNALS__?.invoke === 'function';"), timeout=45)
        self.execute(OBSERVER)
        return value.get('capabilities', {})

    def path(self, suffix):
        require(self.session, 'native session required')
        return '/session/' + self.session + suffix

    def execute(self, script, *args):
        return self.request('POST', self.path('/execute/sync'), {'script': script, 'args': list(args)})

    def invoke(self, command, args=None):
        # This is read-only observation or explicit private fixture setup, never a
        # replacement for connect/selection/Auto/Apply/retry/quit GUI controls.
        require(command in ('config_get', 'config_save', 'proxy_get_status',
                            'proxy_runtime_apply_status', 'auto_select_status'), 'IPC not allowed to bypass GUI')
        self._observed_command = command
        try:
            value = self.request('POST', self.path('/execute/async'),
                                 {'script': NATIVE_BRIDGE_JS, 'args': [command, args or {}]})
        finally:
            self._observed_command = None
        bridge = native_bridge_state(value, command)
        if bridge == 'envelope-rejected':
            raise NativeRejected('actual native fixture/read rejection', value['diagnostic'])
        gui_require(bridge == 'returned', 'native-bridge-' + bridge)
        return value['data']

    def element(self, selector, text=None):
        # Returning an element then using /click produces a real WebDriver input,
        # unlike executing element.click() or dispatching synthetic JS events.
        return self.execute(r'''
const matches = [...document.querySelectorAll(arguments[0])].filter(e =>
 e.getClientRects().length && getComputedStyle(e).visibility === 'visible' && !e.disabled && e.getAttribute('aria-disabled') !== 'true' && (arguments[1] === null ||
 e.textContent.trim() === arguments[1] || e.getAttribute('aria-label') === arguments[1]));
if(matches.length !== 1) return null;
return matches[0];
''', selector, text)

    def contract_control(self, kind, label):
        gui_require(kind in ('fixture-group', 'fixture-node', 'preference-switch'), 'control-kind-invalid')
        value = self.execute(r''' 
const visible=e=>e.getClientRects().length && getComputedStyle(e).visibility==='visible';
let matches;
if(arguments[0]==='fixture-group') {
 matches=[...document.querySelectorAll('#node-menu button.ns-grp')].filter(e=>
 [...e.children].some(c=>c.tagName==='SPAN' && !c.classList.contains('ns-c') && c.textContent.trim()===arguments[1]));
} else if(arguments[0]==='fixture-node') {
 matches=[...document.querySelectorAll('#node-menu button[role="option"]')].filter(e=>
 [...e.querySelectorAll('.nm-name')].some(c=>c.textContent.trim()===arguments[1]));
} else {
 const rows=[...document.querySelectorAll('.set-row')].filter(e=>
 [...e.querySelectorAll('.sr-title > b')].some(c=>c.textContent.trim()===arguments[1]));
 matches=rows.flatMap(e=>[...e.querySelectorAll('[role="switch"]')]);
}
const result={count:matches.length};
if(matches.length===1) {
 const e=matches[0]; result.visible=!!visible(e); result.disabled=!!e.disabled || e.getAttribute('aria-disabled')==='true';
 result.expanded=e.getAttribute('aria-expanded'); result.checked=e.getAttribute('aria-checked'); result.element=e;
}
return result;
''', kind, label)
        if isinstance(value, dict) and value.get('count') == 0:
            raise GuiFailure('control-missing')
        gui_require(isinstance(value, dict) and value.get('count') == 1, 'control-not-unique')
        gui_require(value.get('visible') is True and value.get('disabled') is False, 'control-not-interactable')
        gui_require(isinstance(value.get('element'), dict) and ELEMENT in value['element'], 'control-identity-invalid')
        return value

    def wait_control(self, kind, label):
        def read():
            try:
                return self.contract_control(kind, label)
            except GuiFailure as error:
                if error.code == 'control-missing':
                    return None
                raise
        return wait_for(read, timeout=10)

    def click_control(self, value):
        gui_require(isinstance(value, dict) and isinstance(value.get('element'), dict)
                    and ELEMENT in value['element'], 'control-identity-invalid')
        self.request('POST', self.path('/element/' + value['element'][ELEMENT] + '/click'), {})

    def click(self, selector, text=None):
        element = wait_for(lambda: self.element(selector, text))
        require(ELEMENT in element, 'unique native element required')
        self.request('POST', self.path('/element/' + element[ELEMENT] + '/click'), {})

    def keys(self, *values):
        self.request('POST', self.path('/actions'), {'actions': [{'type': 'key', 'id': 'keyboard',
            'actions': [{'type': kind, 'value': value} for value in values
                        for kind in ('keyDown', 'keyUp')]}]})

    def quit_shortcut(self):
        actions = [{'type': 'keyDown', 'value': '\ue009'}, {'type': 'keyDown', 'value': 'q'},
                   {'type': 'keyUp', 'value': 'q'}, {'type': 'keyUp', 'value': '\ue009'}]
        try:
            self.request('POST', self.path('/actions'), {'actions': [{'type': 'key', 'id': 'keyboard', 'actions': actions}]})
        except (ValueError, OSError):
            # Disconnect is only success if the separately identified App/core
            # really exit. This exception never itself establishes normal quit.
            pass

    def screenshot(self, destination):
        value = self.request('GET', self.path('/screenshot'))
        require(isinstance(value, str), 'native screenshot missing')
        data = base64.b64decode(value, validate=True)
        require(data.startswith(b'\x89PNG\r\n\x1a\n') and len(data) > 1024, 'visible native PNG required')
        destination.write_bytes(data)
        return {'name': destination.name, 'sha256': sha(destination), 'bytes': len(data)}

    def observations(self):
        return self.execute("if(window.__packageObservation?.failed) throw Error('observation failed'); return window.__packageObservation.rows;")

    def close(self):
        if self.session:
            self.request('DELETE', self.path(''), timeout=15)
            self.session = None


def load_fixtures():
    spec = importlib.util.spec_from_file_location('package_fixtures', Path(__file__).with_name('fixtures.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verified_tools(supervisor):
    # The harness reads runner custody; none of this context enters private_env.
    spec = importlib.util.spec_from_file_location('package_tooling', Path(__file__).with_name('tooling.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.verify(run=lambda argv: supervisor.query(
        argv, {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'}))


PORTAL_NOT_TESTED = ['portal', 'filechooser', 'document-export', 'screenshare']


def private_session_config(workspace):
    """A real, non-activating session bus; never reads host session policy."""
    directory = Path(workspace) / 'session-bus'
    directory.mkdir(mode=0o700)
    endpoint = directory / 'bus'
    path = str(endpoint.absolute())
    require(re.fullmatch(r'/[A-Za-z0-9_./-]+', path) and len(os.fsencode(path)) <= 100,
            'private DBus socket path unsupported')
    xml = f'''<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:path={path}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <deny user="*"/>
    <allow user="{os.getuid()}"/>
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
'''
    config = directory / 'session.conf'
    with config.open('x') as stream:
        os.chmod(config, 0o600)
        stream.write(xml)
    return config, endpoint


def private_bus_address(stream):
    deadline, output = time.monotonic() + 15, bytearray()
    while len(output) < 4096:
        remaining = deadline - time.monotonic()
        require(remaining > 0 and select.select([stream], [], [], remaining)[0], 'private DBus unavailable')
        block = os.read(stream.fileno(), 1)
        require(block, 'private DBus closed before address')
        if block == b'\n':
            return output.decode('ascii', errors='strict')
        output.extend(block)
    raise ValueError('private DBus address bound')


def verify_private_session(supervisor, env, address, endpoint):
    require(re.fullmatch(re.escape('unix:path=' + str(endpoint.absolute())) + r',guid=[0-9a-f]{32}', address),
            'private DBus address differs')
    info = endpoint.lstat()
    require(stat.S_ISSOCK(info.st_mode) and info.st_uid == os.getuid(), 'private DBus socket differs')
    def call(method):
        return supervisor.query(['/usr/bin/gdbus', 'call', '--address', address,
            '--dest', 'org.freedesktop.DBus', '--object-path', '/org/freedesktop/DBus',
            '--method', 'org.freedesktop.DBus.' + method], env, 'dbus-probe')
    names_reply = call('ListActivatableNames').strip()
    # GVariant annotates an empty string array; either empty or only the bus
    # itself is acceptable. No external activatable name is admitted.
    require(names_reply in ("(['org.freedesktop.DBus'],)", "(@as [],)", "([],)"),
            'private DBus activation differs')
    names = ['org.freedesktop.DBus'] if 'org.freedesktop.DBus' in names_reply else []
    require(re.fullmatch(r"\('[0-9a-f]{32}',\)", call('GetId').strip()),
            'private DBus identity unavailable')
    return {'state': 'observed', 'auth': 'EXTERNAL', 'peerUid': os.getuid(), 'activation': 'none',
            'activatableNames': names, 'systemBus': 'unchanged',
            'notTested': list(PORTAL_NOT_TESTED)}


def save_fixture(driver, desired):
    before = driver.invoke('config_get')
    value = driver.invoke('config_save', {'config': desired, 'deferRestart': True,
                                          'baseVersion': config_version(before)})
    require(type(value) is dict and value.get('status') == 'saved' and type(value.get('config')) is dict,
            'fixture config save conflicted')
    saved=value['config']
    # Actual full-save producer mirrors these backend-owned fields. These
    # fixtures do not delete live Auto references; such a deletion needs its
    # dedicated user selection path, not an invented successful whole save.
    for key in ('recentServerIds','builtinGeoMeta','meshRoutePolicy','meshRouteState',
                'selectionIntent','autoSelectEnabled','autoSelectPins'):
        gui_require((key in saved)==(key in before) and saved.get(key)==before.get(key),
                    'fixture-authoritative-field-differs')
    for key in ('language','autoConnect','interruptConnectionsOnSwitch','logLevel','mixedPort',
                'controlPort','clashApiSecret','selectedServerId'):
        if key in desired:
            gui_require(type(saved.get(key)) is type(desired[key]) and saved.get(key)==desired[key],
                        'fixture-writable-field-differs')
    if 'servers' in desired:
        wanted=desired['servers'];actual=saved.get('servers')
        gui_require(type(wanted) is list and type(actual) is list and len(actual)==len(wanted),
                    'fixture-servers-differ')
        for node in wanted:
            rows=[row for row in actual if row.get('id')==node.get('id')]
            gui_require(len(rows)==1 and all(type(rows[0].get(k)) is type(node.get(k)) and rows[0].get(k)==node.get(k) for k in
                        ('id','name','protocol','address','port','subscriptionId')), 'fixture-servers-differ')
    return saved


def carrier_path(directory, item):
    paths = list(directory.rglob(item['name']))
    require(len(paths) == 1, 'exact carrier inventory required')
    path = regular(paths[0], directory)
    require(path.stat().st_size == item['bytes'] and sha(path) == item['sha256'], 'frozen carrier differs')
    return path


class CarrierRun:
    fixture_group_label = 'Package loopback'
    def __init__(self, item, package, workspace, evidence_dir, rpc_client, fixtures):
        self.item, self.package = item, package
        self.workspace, self.evidence_dir = workspace, evidence_dir
        self.rpc_client, self.fixtures = rpc_client, fixtures
        self.home = workspace / 'home'
        self.home.mkdir(mode=0o700)
        self.payload = workspace / 'payload'
        members, self.entry = extract(package, item['kind'], self.payload)
        require(members == item['members'], 'frozen payload identity changed')
        self.members = members
        self.env = private_env(self.home)
        self.driver = None
        self.result = {'kind': item['kind'], 'sha256': item['sha256'], 'cases': {},
                       'interface': 'native-webdriver',
                       'candidateSetup': 'native-config-save-fixture',
                       'scope': 'unpacked nonroot non-TUN final package; not installation or FUSE',
                       'networkBoundary': {'fixtureDataPlane': 'loopback',
                           'backgroundReadOnlyInternet': 'allowed; not claimed zero-egress'}}
        self.stage = 'launch'
        self.normal_quit = False
        self.sockets = []
        self.secret = os.urandom(32).hex()
        self.mixed, self.fixture_control_port = free_port(), free_port()
        while self.fixture_control_port == self.mixed:
            self.fixture_control_port = free_port()
        self.core = None
        self.api_endpoint = None
        self.runtime_path = None
        self.translations = json.loads((ROOT / 'ui/src/i18n/locales/en-US.json').read_text())
        self.network = self.create_network()
        try:
            self.supervisor = self.create_supervisor()
        except BaseException:
            self.network.close()
            raise

    def create_network(self):
        return self.fixtures.Network()

    def create_supervisor(self):
        return Supervisor()

    def text(self, path):
        value = self.translations
        for key in path.split('.'):
            value = value[key]
        return value

    def record(self, key, **evidence):
        require(key in CASES and evidence, 'mandatory evidence required')
        self.result['cases'][key] = {'status': 'passed', 'evidence': evidence}

    def start_tools(self):
        self.result['sessionBoundary'] = {'state': 'unconfirmed', 'notTested': list(PORTAL_NOT_TESTED)}
        tools = verified_tools(self.supervisor)
        import select
        read_fd, write_fd = os.pipe()
        try:
            self.supervisor.spawn(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1440x1000x24',
                                   '-nolisten', 'tcp'], self.env, 'xvfb', pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = -1
            require(select.select([read_fd], [], [], 15)[0], 'Xvfb display unavailable')
            display = os.read(read_fd, 32).decode().strip()
            require(display.isdigit(), 'Xvfb display differs')
            self.env['DISPLAY'] = ':' + display
        finally:
            os.close(read_fd)
            if write_fd >= 0:
                os.close(write_fd)
        config, endpoint = private_session_config(self.workspace)
        bus = self.supervisor.spawn(['/usr/bin/dbus-daemon', '--config-file=' + str(config), '--nofork', '--print-address=1'],
                                   self.env, 'dbus', stdout=subprocess.PIPE)
        try:
            address = private_bus_address(bus.stdout)
        finally:
            bus.stdout.close()
        boundary = verify_private_session(self.supervisor, self.env, address, endpoint)
        boundary['configSha256'] = sha(config)
        self.result['sessionBoundary'] = boundary
        self.env['DBUS_SESSION_BUS_ADDRESS'] = address
        driver = tools['automation']['binary']['path']
        native = tools['nativeDriver']['binary']['path']
        self.result['tools'] = {'automation': 'tauri-driver ' + tools['automation']['version'],
                                'identity': tools, 'nativeDriver': tools['nativeDriver']['binary'],
                                'xvfb': 'private display', 'dbus': 'private session'}
        self.driver_port, native_port = free_port(), free_port()
        while native_port == self.driver_port:
            native_port = free_port()
        self.driver_binaries={'automation':copy.deepcopy(tools['automation']['binary']),
                              'native':copy.deepcopy(tools['nativeDriver']['binary'])}
        self.driver_child=self.supervisor.spawn([driver, '--port', str(self.driver_port), '--native-port', str(native_port),
                               '--native-driver', native], self.env, 'driver')
        self.driver = WebDriver(self.driver_port)
        startup_deadline=time.monotonic()+30
        def available():
            try:
                row=self.driver.request('GET', '/status')
            except (OSError, ValueError):
                return None
            if row and row.get('ready') is True and not self.capture_driver_services():return None
            gui_require(time.monotonic()<startup_deadline,'driver-startup-timeout')
            return row
        wait_for(available, lambda row: bool(row and row.get('ready') is True), timeout=max(0,startup_deadline-time.monotonic()))
        gui_require(time.monotonic()<startup_deadline,'driver-startup-timeout')

    def launch(self):
        self.check_driver_services()
        self.driver.start(self.entry)
        self.check_driver_services()
        gui_require(type(self.driver.session) is str and bool(self.driver.session),'driver-session-unconfirmed')
        self.driver_service_session=self.driver.session
        self.result['driverServiceBoundary']['state']='session-bound'
        wait_for(lambda: self.driver.execute("return document.querySelector('#s-home') !== null && '__TAURI_INTERNALS__' in window;"), timeout=60)
        app = self.supervisor.executable(self.payload / self.members['app']['path'])
        env_raw = (Path('/proc') / str(app['pid']) / 'environ').read_bytes().split(b'\0')
        names = {line.split(b'=', 1)[0].decode(errors='replace') for line in env_raw if line}
        require(not names.intersection(FORBIDDEN)
                and not any('TOKEN' in name or name.startswith(('GITHUB_', 'ACTIONS_', 'RUNNER_', 'AWS_')) for name in names),
                'product inherited automation escape or runner credentials')
        self.app = app_execution_identity(app)
        return self.driver.invoke('config_get')

    def capture_driver_failure(self):
        # Diagnostic only. No scan/adoption/signal, and no authority is inferred
        # from an unconfirmed live PID. Keep the original failure even if this
        # snapshot itself cannot be collected.
        try:
            if isinstance(getattr(self, 'driver', None), WebDriver):
                self.result['webdriverObservation'] = copy.deepcopy(getattr(self.driver, 'observation', {}))
        except Exception:
            self.result['webdriverObservation'] = {'state': 'unknown'}
        evidence = {'state': 'observed', 'processes': [], 'truncated': False}
        try:
            supervisor = self.supervisor
            with supervisor.lock:
                known = list(supervisor.owned.values())
                for item in known[:64]:
                    role = supervisor.roles.get(item['pid'], 'unknown')
                    if role not in ('driver', 'product', 'xvfb', 'dbus', 'tool-probe'):
                        role = 'unknown'
                    row = {'pid': item['pid'], 'startTime': item['startTime'], 'role': role, 'identity': 'unknown'}
                    try:
                        current = process_identity(item['pid'])
                        row['identity'] = 'same-birth-uid' if same_process(item, current) else 'not-observed' if current is None else 'changed'
                        child = supervisor.children.get(item['pid'])
                        if child is not None:
                            code = child.poll()
                            row['childState'] = 'running' if code is None else 'exited'
                            if type(code) is int:
                                row['exitCode'] = code
                    except Exception:
                        pass
                    evidence['processes'].append(row)
                evidence['truncated'] = len(known) > 64
        except Exception:
            evidence['state'] = 'unknown'
        self.result['failureProcessObservation'] = evidence

    def snapshot(self, label):
        return self.driver.screenshot(self.evidence_dir / f'{self.item["kind"]}-{label}.png')

    def config(self):
        return self.driver.invoke('config_get')

    def status(self):
        return self.driver.invoke('proxy_get_status')

    def connect_phase(self, phase, status=None):
        require(phase in CONNECT_PHASES, 'fixed connect phase required')
        if phase in ('stopped-check', 'apply-ready'):
            self.api_endpoint = None
            self.result.pop('apiEndpoint', None)
            self._readiness_fence = None
            self._ready_status_context = None
            self._connect_started = time.monotonic()
            self.result['connectObservation'] = {'phases': []}
        record = {'phase': phase, 'elapsedMs': max(0, int((time.monotonic() - self._connect_started) * 1000))}
        if status is not None:
            record['status'] = safe_connect_status(status)
        self.result['connectObservation']['phase'] = phase
        self.result['connectObservation']['phases'].append(record)

    def capture_driver_services(self):
        """Pin the one native Child spawned by fixed tauri-driver before /session."""
        gui_require(not hasattr(self,'driver_services'),'driver-services-already-bound')
        observed=self.supervisor.scan()
        main=self.driver_child
        with self.supervisor.lock:
            item=copy.deepcopy(self.supervisor.owned.get(main.pid))
        gui_require(item is not None and main.poll() is None and
                    same_process(item,observed.get(main.pid)), 'driver-automation-unconfirmed')
        automation=self.capture_driver_service(item,self.driver_binaries['automation'],os.getpid())
        native_path=Path(self.driver_binaries['native']['path']).resolve(strict=True)
        candidates=[]
        for pid,current in observed.items():
            if current['ppid']!=automation['pid'] or current['state']=='Z':continue
            try:
                path=(Path('/proc')/str(pid)/'exe').resolve(strict=True)
            except (FileNotFoundError,ProcessLookupError):continue
            except OSError:raise GuiFailure('driver-native-unreadable') from None
            if path==native_path:
                with self.supervisor.lock:
                    owned=copy.deepcopy(self.supervisor.owned.get(pid))
                gui_require(same_process(owned,current),'driver-native-lineage-unconfirmed')
                candidates.append(self.capture_driver_service(owned,self.driver_binaries['native'],automation['pid']))
        gui_require(len(candidates)<=1,'driver-native-ambiguous')
        if not candidates:return False
        anchors={'automation':automation,'native':candidates[0]}
        self.check_driver_services(anchors)
        self.driver_services=anchors
        self.result['driverServiceBoundary']={'schema':'polaris-driver-service-boundary-v1',
            'state':'pre-session-bound','services':{role:{'pid':v['pid'],'startTime':v['startTime'],
                'sha256':v['sha256']} for role,v in anchors.items()},
            'scope':'explicit-service-anchors-not-descendant-liveness'}
        return True

    def capture_driver_service(self,item,binary,parent_pid):
        validate_app_identity(item)
        gui_require(item['uid']==os.getuid() and item['ppid']==parent_pid and item['state']!='Z',
                    'driver-service-identity-changed')
        path=Path(binary['path']).resolve(strict=True)
        gui_require((Path('/proc')/str(item['pid'])/'exe').resolve(strict=True)==path,
                    'driver-service-executable-changed')
        value=app_execution_identity({**item,'exe':str(path),'sha256':binary['sha256']})
        info=observe_app_executable(value)
        current=process_identity(item['pid'])
        gui_require(same_process(item,current) and current['uid']==os.getuid() and
                    current['ppid']==parent_pid and current['state']!='Z' and info.st_size==binary['bytes'],
                    'driver-service-identity-changed')
        for name in ('st_size','st_mtime_ns','st_ctime_ns','st_mode'):value[name]=getattr(info,name)
        return value

    def check_driver_services(self,anchors=None):
        anchors=getattr(self,'driver_services',None) if anchors is None else anchors
        gui_require(type(anchors) is dict and set(anchors)=={'automation','native'},'driver-services-unbound')
        gui_require(self.driver_child.pid==anchors['automation']['pid'] and self.driver_child.poll() is None,
                    'status-readiness-driver-changed')
        for role,anchor in anchors.items():
            try:
                with self.supervisor.lock:
                    owned=copy.deepcopy(self.supervisor.owned.get(anchor['pid']))
                gui_require(same_process(anchor,owned) and owned['uid']==anchor['uid']==os.getuid(),
                            'status-readiness-driver-changed')
                info=observe_app_executable(anchor)
                current=process_identity(anchor['pid'])
                parent=os.getpid() if role=='automation' else anchors['automation']['pid']
                gui_require(same_process(anchor,current) and current['uid']==anchor['uid'] and current['state']!='Z' and
                            current['ppid']==parent and (info.st_dev,info.st_ino)==(anchor['exeDev'],anchor['exeInode']) and
                            all(getattr(info,k)==anchor[k] for k in ('st_size','st_mtime_ns','st_ctime_ns','st_mode')),
                            'status-readiness-driver-changed')
            except (OSError,ValueError,KeyError,TypeError):
                self.result.setdefault('driverServiceFailure',{'role':role,'state':'identity-unconfirmed'})
                raise GuiFailure('status-readiness-driver-changed') from None

    def readiness_observation_fence(self, session, owner=None):
        gui_require(type(session) is str and bool(session) and self.driver.session == session and
                    session==getattr(self,'driver_service_session',None), 'status-readiness-session-changed')
        current = observe_app_executable(self.app)
        gui_require((current.st_dev,current.st_ino)==(self.app['exeDev'],self.app['exeInode']),
                    'status-readiness-app-changed')
        self.check_driver_services()

    def wait_ready_status(self, timeout):
        gui_require(type(timeout) in (int,float) and 0<timeout<float('inf'), 'status-readiness-timeout-invalid')
        self._readiness_deadline=time.monotonic()+timeout
        try:
            status=wait_for(self.connect_status,lambda s:s.get('running') is True and not s.get('starting'),timeout=timeout)
            gui_require(time.monotonic()<self._readiness_deadline,'observation-timeout')
            return status
        finally:
            self._readiness_deadline=None

    def readiness_budget(self):
        deadline=getattr(self,'_readiness_deadline',None)
        if deadline is not None:
            gui_require(time.monotonic()<deadline,'observation-timeout')

    def new_status_observation(self, context, deadline):
        # These are existing observation loops, not general status/IPC retry.
        gui_require(context in ('off','pick','activation','auto','ready-binding'), 'status-observation-context-invalid')
        gui_require(type(deadline) in (int,float) and time.monotonic()<deadline<float('inf'),
                    'observation-timeout')
        record={'context':context,'state':'waiting','samples':0}
        rows=self.result.setdefault('statusWaitObservations',[])
        if len(rows)<64: rows.append(record)
        else: self.result['statusWaitObservationsTruncated']=True
        return {'deadline':deadline,'record':record,'session':getattr(self.driver,'session',None),
                'fence':None,'busySeen':False}

    def status_observation_sample(self, context, read=None):
        def budget():
            deadline=context['deadline']
            if deadline is not None:
                gui_require(time.monotonic()<deadline,'observation-timeout')
        record=context['record']
        try:
            budget()
            record['samples']=record.get('samples',0)+1
            try:
                status=self.status() if read is None else read()
            except NativeRejected as error:
                if error.diagnostic not in STATUS_BUSY_DIAGNOSTICS: raise
                budget()
                self.readiness_observation_fence(context['session'],context)
                budget()
                transient=record.setdefault('savedStateObservation',{'state':'waiting','busyCount':0,
                    'firstDiagnostic':error.diagnostic,'lastDiagnostic':error.diagnostic})
                transient['busyCount']+=1
                transient.update(lastDiagnostic=error.diagnostic,state='waiting')
                context['busySeen']=True
                record['state']='waiting'
                return None  # no invented running/starting/owned-stop values
            budget()
            if context['busySeen']:
                self.readiness_observation_fence(context['session'],context)
                record['savedStateObservation']['state']='observed'
            budget()
            if read is None: record['lastStatus']=safe_connect_status(status)
            record['state']='waiting' if status is None else 'observed'
            return status
        except Exception:
            record['state']='failed'
            raise

    def wait_status_proof(self, observation, read):
        # Only explicit, read-only stage proofs use this entry. A busy failure
        # discards the entire partial capture; never wrap a click/commit/register.
        try:
            value=wait_for(lambda:self.status_observation_sample(observation,read),
                           lambda value:value is not None,
                           timeout=observation['deadline']-time.monotonic())
            gui_require(time.monotonic()<observation['deadline'],'observation-timeout')
            return value
        except Exception:
            observation['record']['state']='failed'
            raise

    def wait_status_observation(self, context, condition, deadline, phase=None):
        observation=self.new_status_observation(context,deadline)
        def read():
            status=self.status_observation_sample(observation)
            if status is not None and phase is not None: self.gui_phase(phase,status)
            return status
        try:
            result=wait_for(read,lambda value:value is not None and condition(value),
                            timeout=deadline-time.monotonic())
            gui_require(time.monotonic()<deadline,'observation-timeout')
            observation['record']['state']='complete'
            return result
        except Exception:
            observation['record']['state']='failed'
            raise

    def connect_status(self):
        context=getattr(self,'_ready_status_context',None)
        if context is None:
            context={'record':self.result['connectObservation'],'session':getattr(getattr(self,'driver',None),'session',None),
                     'fence':None,'busySeen':False}
            self._ready_status_context=context
        context['deadline']=getattr(self,'_readiness_deadline',None)
        status=self.status_observation_sample(context)
        return {} if status is None else status

    def observed_api_endpoint(self, expected_status=None):
        # Parse exactly the stable bytes captured privately, never a subsequent
        # unbound read. Status and the parsed graph are bracketed by captures.
        proof, graph = stable_runtime_capture(self.runtime_path, self.home, self.core,
            self.workspace / 'private-runtime-originals', include_content=True)
        status = self.status()
        if expected_status is not None:
            gui_require(type(status.get('mainGeneration')) is int and
                        status['mainGeneration']==expected_status.get('mainGeneration') and
                        status.get('pid')==expected_status.get('pid'), 'status-binding-generation-changed')
        after = self.runtime_identity()
        require(proof['sha256'] == after['sha256'] and proof['pid'] == after['pid']
                and proof['startTime'] == after['startTime'], 'API runtime changed during binding')
        return bind_api_endpoint(graph, status, self.secret, self.core), proof, graph

    def ready_capabilities(self, expected):
        def current():
            value=self.status()
            gui_require(type(value.get('mainGeneration')) is int and
                        value['mainGeneration']==expected.get('mainGeneration') and
                        value.get('pid')==expected.get('pid') and value.get('running') is True and
                        not value.get('starting'),'status-binding-generation-changed')
        current()
        value=self.rpc('capabilities')
        current()
        return value

    def revalidate_api_endpoint(self):
        require(self.api_endpoint is not None, 'API endpoint not bound')
        endpoint, proof, _ = self.observed_api_endpoint()
        require(endpoint == self.api_endpoint, 'bound API endpoint/core changed')
        return proof

    def connect(self):
        self.stage = 'gui-connect'
        self.connect_phase('stopped-check')
        require(not self.status().get('running'), 'connect requires stopped core')
        self.connect_phase('gui-click')
        self.driver.click('#connect-btn')
        return self.observe_ready_core()

    def observe_ready_core(self, timeout=75, register=True):
        # Observe the original ready core; never click or replay a start here.
        self.connect_phase('status-ready')
        deadline=time.monotonic()+timeout
        status = self.wait_ready_status(timeout)
        binding_wait=self.new_status_observation('ready-binding',deadline)
        self.connect_phase('core-birth', status)
        require(status.get('startedViaHelper') is not True, 'privileged helper prohibited')
        core = self.supervisor.executable(self.payload / self.members['core']['path'])
        require(status.get('pid') == core['pid'], 'App status is not owned packaged core')
        self.core = core
        self.connect_phase('runtime-path')
        arguments = (Path('/proc') / str(core['pid']) / 'cmdline').read_bytes().split(b'\0')
        candidates = []
        for index, arg in enumerate(arguments[:-1]):
            if arg in (b'-c', b'--config'):
                candidates.append(Path(os.fsdecode(arguments[index + 1])))
        require(len(candidates) == 1, 'one real core configuration argument required')
        path = candidates[0]
        if not path.is_absolute():
            path = (Path('/proc') / str(core['pid']) / 'cwd').resolve() / path
        self.runtime_path = regular(path, self.home)
        require(self.runtime_path.name == 'singbox-runtime.json', 'unexpected runtime config source')
        self.connect_phase('runtime-api')
        endpoint, runtime, content = self.wait_status_proof(binding_wait,lambda:self.observed_api_endpoint(expected_status=status))
        require(all(item.get('type') != 'tun' for item in content.get('inbounds', [])), 'TUN not permitted')
        self.api_endpoint = endpoint
        self.connect_phase('owned-api-probe')
        self.wait_status_proof(binding_wait,lambda:self.ready_capabilities(status))  # Read-only owned LISTEN proof.
        self.connect_phase('cronet-map')
        maps = (Path('/proc') / str(core['pid']) / 'maps').read_text()
        expected = self.payload / self.members['cronet']['path']
        mapping = cronet_mapping(maps, expected, self.members['cronet']['sha256'])
        self.connect_phase('receipt')
        self.result['apiEndpoint'] = {**endpoint, 'ownedListenerProbe': True}
        self.result['execution'] = {'uid': os.getuid(), 'defaultSandbox': True, 'originalEntry': True,
            'automation': 'tauri-driver 2.0.5',
            'app': {key: self.app[key] for key in ('sha256', 'pid', 'startTime')},
            'core': {key: core[key] for key in ('sha256', 'pid', 'startTime')},
            'cronet': {**mapping, 'path': self.members['cronet']['path']}}
        self.result['configIdentity'] = self.runtime_identity()
        self.result['cases']['default_dashboard']['evidence'].setdefault(
            'actualGeneratedRuntime', self.result['configIdentity'])
        self.gui_phase('connect-backend-observed', status)
        return status

    def disconnect(self):
        self.gui_phase('disconnect-begin')
        self.api_endpoint = None
        before = self.core
        require(self.status().get('running'), 'disconnect requires running core')
        self.driver.click('#connect-btn')
        self.wait_status_observation('off', lambda s:s.get('running') is False and not s.get('starting'),
                                     time.monotonic()+45, phase='disconnect-status')
        wait_for(lambda: process_identity(before['pid']),
                 lambda p: not same_process(p, before) or p['state'] == 'Z', timeout=15)

    def gui_phase(self, phase, state=None):
        observation = self.result.setdefault('guiObservation', {'steps': [], 'truncated': False})
        now = time.monotonic()
        if not hasattr(self, '_gui_started'):
            self._gui_started = now
        row = {'phase': phase, 'elapsedMs': max(0, int((now - self._gui_started) * 1000))}
        if state is not None:
            row['state'] = {key: state[key] for key in ('running', 'starting') if type(state.get(key)) is bool}
            if 'selectionIntent' in state:
                row['state']['selectionIntentPresent'] = state['selectionIntent'] is not None
            for key in ('mainGeneration', 'pid'):
                if type(state.get(key)) is int and state[key] >= 0:
                    row['state'][key] = state[key]
            for key in ('selectedServerId', 'actualServerId'):
                if key in state:
                    row['state'][key] = state[key] if state[key] in ('node-a', 'node-b', None) else 'other'
        observation['phase'] = phase
        if 'state' in row:
            observation['lastState'] = row['state']
        if observation['steps'] and all(observation['steps'][-1].get(key) == row.get(key) for key in ('phase', 'state')):
            return
        if len(observation['steps']) < 128:
            observation['steps'].append(row)
        else:
            observation['truncated'] = True

    def gui_read(self, phase, read):
        value = read()
        self.gui_phase(phase, value)
        return value

    def pick(self, label):
        gui_require(label in ('A', 'B'), 'fixture-node-invalid')
        self.gui_phase('pick-fixture-identity')
        config = self.config()
        nodes = [n for n in config.get('servers', []) if n.get('id') == 'node-' + label.lower()]
        gui_require(len(nodes) == 1 and nodes[0].get('name') == 'Loop ' + label
                    and sum(n.get('name') == 'Loop ' + label for n in config.get('servers', [])) == 1, 'fixture-node-identity-differs')
        group_id = nodes[0].get('subscriptionId')
        if group_id:
            groups = [g for g in config.get('subscriptions', []) if g.get('id') == group_id]
            gui_require(group_id == 'package-fixture' and len(groups) == 1 and
                        groups[0].get('name') == self.fixture_group_label, 'fixture-group-identity-differs')
            group_label = groups[0]['name']
        else:
            group_label = self.text('nodes.tab.manual')
        self.gui_phase('pick-menu-open')
        if not self.driver.element('#node-menu .nm-name', 'Loop ' + label):
            expanded = self.driver.execute("return document.querySelector('#node-trigger')?.getAttribute('aria-expanded');")
            gui_require(expanded in ('true', 'false'), 'node-menu-state-invalid')
            if expanded == 'false':
                self.driver.click('#node-trigger')
            self.gui_phase('pick-group-resolve')
            group = self.driver.wait_control('fixture-group', group_label)
            gui_require(group.get('expanded') in ('true', 'false'), 'fixture-group-state-invalid')
            if group['expanded'] == 'false':
                self.gui_phase('pick-group-expand')
                self.driver.click_control(group)
                wait_for(lambda: self.driver.contract_control('fixture-group', group_label),
                         lambda row: row.get('expanded') == 'true', timeout=10)
        self.gui_phase('pick-node-click')
        target = self.driver.wait_control('fixture-node', 'Loop ' + label)
        self.driver.click_control(target)
        self.gui_phase('pick-config-readback')
        observed = wait_for(lambda: self.gui_read('pick-config-readback', self.config), lambda c: c.get('selectedServerId') == 'node-' + label.lower()
                 and not c.get('selectionIntent'), timeout=30)
        self.gui_phase('pick-status-readback', observed)
        status_deadline=time.monotonic()+45
        status_context=self.new_status_observation('pick',status_deadline)
        baseline=self.wait_status_proof(status_context,self.status)
        gui_require(type(baseline.get('running')) is bool,'status-observation-schema-invalid')
        if baseline['running']:
            def selected():
                value=self.status()
                self.gui_phase('pick-status-readback',value)
                return value if value.get('actualServerId') == 'node-' + label.lower() else None
            status=self.wait_status_proof(status_context,selected)
            self.gui_phase('pick-complete', status)
        else:
            self.gui_phase('pick-complete', observed)

    def preference_disk(self):
        # Existing observed App birth only; no process scan/adoption or signal.
        validate_app_identity(self.app, captured=True)
        app = dict(self.app)
        def same_app():
            current = observe_app_executable(app)
            gui_require((current.st_dev, current.st_ino) == (app['exeDev'], app['exeInode']),
                        'preference-app-identity-changed')
        same_app()
        value = read_saved_preference(self.home, self.env['XDG_CONFIG_HOME'])
        same_app()
        return value

    def preference_runtime(self, wanted):
        endpoint, proof, graph = self.observed_api_endpoint()
        gui_require(endpoint == self.api_endpoint, 'preference-runtime-endpoint-changed')
        selectors = [o for o in graph.get('outbounds', []) if o.get('tag') == 'proxy-selector']
        gui_require(len(selectors) == 1 and selectors[0].get('type') == 'selector', 'preference-runtime-selector-invalid')
        # Fixed core SelectorOutboundOptions uses bool/omitempty: canonical
        # exact-file HOT persistence omits false. Null/non-bool is not absence.
        value = selectors[0].get('interrupt_exist_connections', False)
        gui_require(type(value) is bool and value is wanted, 'preference-runtime-value-differs')
        return {'wanted': wanted, 'selectorInterrupt': value, 'runtime': proof,
                'core': {k: self.core[k] for k in ('pid','startTime')}}

    def pending_apply_control(self):
        return self.driver.execute(r"""
const visible=e=>e.getClientRects().length&&getComputedStyle(e).visibility!=='hidden';
const bars=[...document.querySelectorAll('.pending-bar')].filter(visible);
const buttons=bars.flatMap(b=>[...b.querySelectorAll('.pb-acts button')]).filter(e=>visible(e)&&e.textContent.trim()===arguments[0]);
return {bars:bars.length,buttons:buttons.length,disabled:buttons.some(e=>e.disabled||e.getAttribute('aria-disabled')==='true')};
""", self.text('home.pendingApplyBtn'))

    def retire_applied_core(self, previous):
        current = process_identity(previous['pid'])
        gui_require(not same_process(previous, current) or current['state'] == 'Z', 'preference-old-core-still-running')

    def activate_saved_preference(self, wanted):
        return self.activate_saved_graph(wanted, self.preference_disk, self.preference_runtime, 'preferenceActivations')

    def activation_runtime_ready(self, wanted, phase):
        # Read-only preparation predicate, not the final business proof. An absent
        # first Auto graph or the previous boolean policy is a legitimate old R.
        # Malformed schemas, ownership failures and transport errors still fail.
        if phase == 'preference':
            endpoint, _, graph = self.observed_api_endpoint()
            gui_require(endpoint == self.api_endpoint, 'preference-runtime-endpoint-changed')
            outbounds = graph.get('outbounds')
            gui_require(type(outbounds) is list and all(type(o) is dict for o in outbounds),
                        'preference-runtime-selector-invalid')
            rows = [o for o in outbounds if o.get('tag') == 'proxy-selector']
            gui_require(len(rows) == 1 and rows[0].get('type') == 'selector', 'preference-runtime-selector-invalid')
            value = rows[0].get('interrupt_exist_connections', False)
            gui_require(type(value) is bool, 'preference-runtime-value-invalid')
            return value is wanted
        observation = {'schema':'polaris-auto-runtime-observation-v1','phase':'begin','state':'checking','checks':{}}
        self.result['autoRuntimePreparation'] = observation
        try:
            self._auto_runtime(wanted, observation, preparing=True)
            return True
        except GuiFailure as error:
            if error.code != 'auto-runtime-target-not-ready':
                observation['state'] = 'failed'
                raise
            observation['state'] = 'waiting'
            return False

    def activate_saved_graph(self, wanted, saved_read, runtime_read, receipt_key, phase_prefix='preference'):
        gui_require(phase_prefix in ('preference','auto'), 'activation-context-invalid')
        # Saved D, accepted R, and the subsequent selection are separate proofs.
        self.gui_phase(phase_prefix+'-activation-begin')
        deadline = time.monotonic() + 75
        def remaining():
            left = deadline-time.monotonic()
            gui_require(left > 0, 'observation-timeout')
            return left
        status_wait=self.new_status_observation('activation',deadline)
        def baseline():
            disk=saved_read()
            gui_require(disk['value'] is wanted,'preference-saved-value-differs')
            previous=dict(self.core);status=self.status()
            self.same_core(previous,status=status)
            return disk,previous,status
        disk,previous,before_status=self.wait_status_proof(status_wait,baseline)
        transition = {'schema':'polaris-activation-observation-v1','context':phase_prefix,
                      'state':'waiting','samples':0,'targetReady':False,'applyVisible':False}
        self.result['activationObservation'] = transition
        self.gui_phase(phase_prefix+'-activation-wait')
        def observe_control():
            control = self.pending_apply_control()
            gui_require(type(control) is dict and set(control) == {'bars','buttons','disabled'} and
                        type(control['bars']) is int and type(control['buttons']) is int and
                        type(control['disabled']) is bool and control['bars'] in (0,1) and control['buttons'] in (0,1) and
                        control['buttons'] <= control['bars'], 'preference-apply-control-invalid')
            return control
        def prepared():
            remaining()
            status = self.status()
            self.same_core(previous, status=status)
            gui_require(status.get('running') is True and status.get('pid') == before_status.get('pid') and
                        status.get('mainGeneration') == before_status.get('mainGeneration'), 'activation-generation-changed')
            gui_require(saved_read() == disk, 'preference-saved-file-changed')
            control = observe_control()
            transition['samples'] += 1
            transition['applyVisible'] = bool(control['buttons'])
            if control['buttons'] and not control['disabled']:
                transition['state'] = 'apply-ready'
                return control
            # A temporarily empty/disabled bar is not proof of no pending work.
            if control['bars']:
                return None
            ready = self.activation_runtime_ready(wanted, phase_prefix)
            transition['targetReady'] = ready
            if not ready:
                return None
            # Re-read the actual control after the owned R capture; a newly
            # rendered pending action wins over the no-action path.
            control = observe_control()
            if control['bars']:
                return None
            current = self.status()
            self.same_core(previous, status=current)
            gui_require(current.get('running') is True and current.get('pid') == before_status.get('pid') and
                        current.get('mainGeneration') == before_status.get('mainGeneration'), 'activation-generation-changed')
            gui_require(saved_read() == disk, 'preference-saved-file-changed')
            transition['state'] = 'target-already-accepted'
            return control
        try:
            control = self.wait_status_proof(status_wait,prepared)
        except Exception:
            transition['state'] = 'failed'
            raise
        proof = {'wanted':wanted,'saved':disk,'apply':'not-needed','restartConfirmed':False}
        if control['buttons']:
            gui_require(not control['disabled'], 'preference-apply-disabled')
            self.gui_phase(phase_prefix+'-apply-click')
            transition['state'] = 'apply-requested'
            result = self.apply(('hotApplied','restartRequired'), timeout=min(65,remaining()))
            proof['apply'] = result['status']
            if result['status'] == 'restartRequired':
                cursor = max((r['sequence'] for r in self.driver.observations()), default=0)
                self.gui_phase(phase_prefix+'-restart-confirm')
                self.driver.click('dialog[open][aria-labelledby="restart-apply-title"] button', self.text('home.restartApplyConfirm'))
                def callback():
                    rows=[r for r in self.driver.observations() if r.get('sequence',0)>cursor and r.get('source')=='original-native-callback']
                    gui_require(len(rows)<=1,'preference-apply-callback-ambiguous')
                    return rows[0] if rows else None
                scheduled=wait_for(callback,timeout=remaining())
                gui_require(scheduled.get('success') is True and scheduled.get('ok') is True and scheduled.get('status')=='applied' and
                            type(scheduled.get('applyId')) is int and scheduled['applyId']>0,'preference-restart-not-scheduled')
                apply_id=scheduled['applyId']
                def terminal():
                    rows=[r for r in self.driver.observations() if r.get('sequence',0)>cursor and r.get('source')=='original-lifecycle-event']
                    return preference_restart_terminal(rows,apply_id,before_status.get('mainGeneration'))
                ready=wait_for(terminal,timeout=remaining())
                self.retire_applied_core(previous)
                self.connect_phase('apply-ready')
                status=self.observe_ready_core(timeout=remaining())
                gui_require(status.get('mainGeneration')==ready['mainGeneration'] and not same_process(previous,self.core),
                            'preference-restart-generation-differs')
                proof.update(restartConfirmed=True,applyId=apply_id,lifecycle=ready)
                self.gui_phase(phase_prefix+'-restart-ready',status)
            else:
                gui_require(result.get('ok') is True, 'preference-hot-rejected')
                self.wait_status_proof(status_wait,lambda:(self.same_core(previous),True)[1])
                self.connect_phase('apply-ready')
                self.observe_ready_core(timeout=remaining(),register=False)
                self.wait_status_proof(status_wait,lambda:(self.same_core(previous),True)[1])
                transaction=result.get('transactionId')
                gui_require(type(transaction) is str and bool(transaction),'preference-hot-transaction-missing')
                def accepted_hot():
                    self.same_core(previous)
                    lookup=self.native_status(transaction)
                    gui_require(type(lookup.get('lookupState')) is int and lookup['lookupState']==1 and type(lookup.get('operation')) is dict, 'preference-hot-lookup-unconfirmed')
                    operation=lookup['operation']
                    gui_require(operation.get('durable') is True and operation.get('persistenceMode')=='exact_configuration_file'
                                and operation.get('active')==self.rpc('capabilities')['active'], 'preference-hot-durability-unconfirmed')
                    return {'transactionId':transaction,'durable':True,
                            'persistenceMode':'exact_configuration_file','active':operation['active']}
                proof['hotReceipt']=self.wait_status_proof(status_wait,accepted_hot)
        else:
            gui_require(control['bars']==0, 'preference-pending-without-apply')
            self.wait_status_proof(status_wait,lambda:(self.same_core(previous),True)[1])
        self.gui_phase(phase_prefix+'-runtime-proof')
        runtime_deadline=min(deadline,time.monotonic()+35) if phase_prefix=='auto' else deadline
        def accepted_runtime():
            gui_require(saved_read()==disk,'preference-saved-file-changed')
            value=runtime_read(wanted,deadline=runtime_deadline) if phase_prefix=='auto' else runtime_read(wanted)
            gui_require(saved_read()==disk,'preference-saved-file-changed')
            return value
        proof['acceptedRuntime']=self.wait_status_proof(status_wait,accepted_runtime)
        wait_for(self.pending_apply_control, lambda value: value=={'bars':0,'buttons':0,'disabled':False}, timeout=remaining())
        remaining()
        transition['state'] = 'accepted-runtime'
        self.result.setdefault(receipt_key,[]).append(proof)
        return proof

    def preference_pending_save(self):
        count = self.driver.execute(r'''
const visible=e=>e.getClientRects().length && getComputedStyle(e).visibility!=='hidden';
return [...document.querySelectorAll('.pending-bar .pb-acts button')].filter(e=>visible(e)&&e.textContent.trim()===arguments[0]).length;
''', self.text('home.pendingSaveBtn'))
        gui_require(type(count) is int and count in (0, 1), 'preference-save-not-unique')
        return count == 1

    def preference(self, wanted):
        gui_require(type(wanted) is bool, 'preference-value-invalid')
        self.gui_phase('preference-navigation')
        self.driver.click('.side .nav-item', self.text('sidebar.settings'))
        self.driver.click('.set-nav', self.text('settings.nav.network'))
        self.gui_phase('preference-control')
        control = self.driver.wait_control('preference-switch', self.text('settings.network.interruptOnSwitch'))
        gui_require(control.get('checked') in ('true', 'false'), 'preference-state-differs')
        gui_require(not self.preference_pending_save(), 'preference-preexisting-dirty')
        before = self.preference_disk()
        checked = control['checked'] == 'true'
        gui_require(before['value'] == checked, 'preference-ui-disk-differs')
        proof = {'wanted': wanted, 'before': before, 'clicked': False, 'commandIdentityClaimed': False}
        proofs = self.result.setdefault('preferenceObservations', [])
        gui_require(len(proofs) < 128, 'preference-observation-bound')
        proofs.append(proof)
        # Preserve the original 30s confirmation budget across all observations;
        # do not grant a fresh budget for each callback/disk/UI check.
        deadline = time.monotonic() + 30
        def confirm(read, condition=bool):
            remaining = deadline - time.monotonic()
            gui_require(remaining > 0, 'observation-timeout')
            value = wait_for(read, condition, timeout=remaining)
            gui_require(time.monotonic() <= deadline, 'observation-timeout')
            return value
        if checked != wanted:
            self.gui_phase('preference-click')
            self.driver.click_control(control)
            proof['clicked'] = True
            self.gui_phase('preference-save-ready')
            confirm(self.preference_pending_save, lambda visible: visible is True)
            cursor = max((row['sequence'] for row in self.driver.observations()), default=0)
            self.gui_phase('preference-save')
            self.driver.click('.pending-bar .pb-acts button', self.text('home.pendingSaveBtn'))
            def saved():
                rows = [r for r in self.driver.observations() if r.get('sequence', 0) > cursor
                        and r.get('source') == 'original-config-save-callback']
                gui_require(len(rows) <= 1, 'preference-save-ambiguous')
                if not rows: return None
                row = rows[0]
                gui_require(set(row) == {'sequence','source','success','status','versionValid','preference'} and
                            type(row['sequence']) is int and row['success'] is True and row['status'] == 'saved' and
                            row['versionValid'] is True and type(row['preference']) is bool and row['preference'] == wanted,
                            'preference-save-unconfirmed')
                return {'association': 'shape-only-click-window', 'status': 'saved',
                        'versionValid': True, 'preference': row['preference']}
            self.gui_phase('preference-save-callback')
            proof['saveCallback'] = confirm(saved)
        self.gui_phase('preference-durable-readback')
        after = self.preference_disk()
        gui_require(after['value'] == wanted, 'preference-disk-value-differs')
        proof['after'] = after
        self.gui_phase('preference-clean-readback')
        confirm(self.preference_pending_save, lambda visible: visible is False)
        observed = self.driver.wait_control('preference-switch', self.text('settings.network.interruptOnSwitch'))
        gui_require(observed.get('checked') == ('true' if wanted else 'false'), 'preference-state-differs')
        gui_require(time.monotonic() <= deadline, 'observation-timeout')
        proof['clean'] = True
        self.gui_phase('preference-return')
        self.driver.click('.side .nav-item', self.text('settings.nav.back'))

    def socket(self, label):
        self.gui_phase('stream-connect-begin')
        stream = self.fixtures.socks_connect(('127.0.0.1', self.mixed), self.network.domain, 80)
        self.sockets.append(stream)
        proof = self.exchange(stream, label)
        return stream, proof

    def exchange(self, stream, label):
        gui_require(label in ('A', 'B'), 'fixture-node-invalid')
        self.gui_phase('stream-exchange')
        nonce = os.urandom(18).hex()
        row = {'backend': label, 'bytes': len(nonce), 'nonceSha256': hashlib.sha256(nonce.encode()).hexdigest(),
               'state': 'pending'}
        observations = self.result.setdefault('streamObservation', {'exchanges': [], 'truncated': False})
        if len(observations['exchanges']) < 128:
            observations['exchanges'].append(row)
        else:
            observations['truncated'] = True
        try:
            proof = self.fixtures.exchange(stream, label, nonce)
        except Exception as error:
            row['state'] = 'failed'
            row['failure'] = ('echo-mismatch' if type(error) is ValueError else
                              'read-eof' if isinstance(error, EOFError) else
                              'io-timeout' if isinstance(error, TimeoutError) else 'io-unconfirmed')
            raise
        row.update(state='validated', exactEchoMatched=True)
        self.gui_phase('stream-validated')
        return proof

    def runtime_identity(self):
        path = regular(self.runtime_path, self.home)
        return stable_runtime_capture(path, self.home, self.core, self.workspace / 'private-runtime-originals')

    def rpc(self, action, request=None):
        require(same_process(self.core, process_identity(self.core['pid'])), 'owned core identity changed')
        self.revalidate_api_endpoint()
        data = {'action': action, 'host': self.api_endpoint['host'], 'port': self.api_endpoint['port'], 'secret': self.secret,
                'owned': {'pid': self.core['pid'], 'startTimeTicks': int(self.core['startTime']),
                          'exe': self.core['exe'], 'runtimeConfig': str(self.runtime_path)},
                'request': request or {}}
        # The secret/config enter only stdin; no argv, environment or artifact.
        with self.supervisor.lock:
            child = subprocess.Popen([str(self.rpc_client)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, env=self.env, start_new_session=True)
            item = process_identity(child.pid)
            require(item, 'RPC client process identity unavailable')
            self.supervisor.children[child.pid] = child
            self.supervisor.owned[child.pid] = item
            self.supervisor.roles[child.pid] = 'probe'
        try:
            output, _ = child.communicate(json.dumps(data).encode(), timeout=90)
        except subprocess.TimeoutExpired:
            self.supervisor.forced.append({'pid': child.pid, 'startTime': item['startTime'], 'reason': 'rpc-timeout'})
            self.supervisor.signal(item, signal.SIGKILL)
            child.wait(timeout=5)
            raise ValueError('bounded RPC probe timed out') from None
        require(len(output) <= 1024 * 1024, 'RPC evidence bound')
        result = json.loads(output)
        require(child.returncode == 0 and result.get('ok') is True
                and result.get('schema') == 'polaris-package-rpc-probe-v1'
                and result.get('action') == action, 'RPC probe failed')
        self.revalidate_api_endpoint()
        return result['result']

    def apply(self, expected='hotApplied', retry=None, timeout=65):
        self.gui_phase('apply-begin')
        observations = self.driver.observations()
        cursor = max((row['sequence'] for row in observations), default=0)
        label = 'home.pendingRetryPersistenceBtn' if retry else 'home.pendingApplyBtn'
        self.gui_phase('apply-click')
        self.driver.click('.pending-bar .pb-acts button', self.text(label))
        def changed():
            rows = self.driver.observations()
            matches = [row for row in rows if row.get('sequence', 0) > cursor
                       and row.get('source') == 'original-native-callback']
            if retry:
                matches = [row for row in matches if row.get('transactionId') == retry]
            return matches[0] if matches else None
        self.gui_phase('apply-original-callback')
        result = wait_for(changed, timeout=timeout)
        require(result.get('success') is True, 'GUI Apply outcome unknown; no automatic resubmission')
        transaction = result.get('transactionId')
        if result.get('status') == 'unconfirmed':
            require(transaction, 'unknown Apply without transaction is terminal failure')
            result = wait_for(lambda: self.driver.invoke('proxy_runtime_apply_status', {'transactionId': transaction}),
                              lambda row: row.get('status') != 'unconfirmed', timeout=60)
        require(result.get('status') in ((expected,) if type(expected) is str else expected), 'GUI Apply did not reach required outcome')
        if retry:
            require(transaction == retry, 'GUI retried another transaction')
        return result

    def native_status(self, transaction):
        capability = self.rpc('capabilities')
        return self.rpc('status', {'transactionId': transaction,
                                  'runtimeIncarnation': capability['active']['incarnation']})

    def same_core(self, baseline, status=None):
        require(same_process(self.core, process_identity(self.core['pid']))
                and (self.status() if status is None else status).get('pid') == baseline['pid']
                and self.core['startTime'] == baseline['startTime'], 'HOT unexpectedly replaced core')

    def selection_cases(self):
        self.stage = 'gui-selection'
        self.pick('A')
        self.connect()
        self.stage = 'selection-initial-traffic'
        first, first_proof = self.socket('A')
        self.stage = 'selection-switch-b'
        self.pick('B')
        self.stage = 'selection-preserved-traffic'
        old_after = self.exchange(first, 'A')
        self.stage = 'selection-new-traffic'
        second, second_proof = self.socket('B')
        self.record('gui_selection', selected='node-b', before=first_proof,
                    newConnection=second_proof, screenshot=self.snapshot('selected-b'))
        self.stage = 'selection-interrupt-preference'
        self.preference(True)
        self.activate_saved_preference(True)
        # Apply/restart closure is not node-switch evidence. Establish a fresh
        # B stream only after the accepted preference runtime is proved.
        self.stage = 'selection-interrupt-baseline'
        self.pick('B')
        second, interrupt_before = self.socket('B')
        interrupt_core = dict(self.core)
        self.stage = 'selection-interrupt-switch'
        self.pick('A')
        self.same_core(interrupt_core)
        self.stage = 'selection-old-stream-close'
        def closed():
            try:
                second.settimeout(0.3)
                second.sendall(b'closed-probe')
                return second.recv(1) == b''
            except (BrokenPipeError, ConnectionResetError):
                return True
            except socket.timeout:
                return False
        wait_for(closed, timeout=12)
        self.record('gui_manual_disconnect_preference', gracefulOldConnection=old_after,
                    interruptPreferenceObserved=True, oldBClosed=True,
                    interruptionBaseline=interrupt_before, interruptionCore=interrupt_core,
                    preferenceActivation=self.result['preferenceActivations'][-1])
        self.stage = 'selection-restore-b'
        self.pick('B')
        saved = self.config()['selectedServerId']
        old_core = dict(self.core)
        self.stage = 'selection-off'
        self.disconnect()
        require(self.config()['selectedServerId'] == saved, 'OFF changed selection')
        self.connect()
        self.stage = 'selection-on-readback'
        require(self.config()['selectedServerId'] == saved and not same_process(old_core, self.core), 'ON did not restore selection')
        self.stage = 'selection-restored-traffic'
        _, proof = self.socket('B')
        self.record('gui_on_off', selected=saved, newProcess=True, restoredConnection=proof)

    def auto_saved(self):
        config = self.config()
        gui_require(config.get('autoSelectEnabled') is True and
                    config.get('selectionIntent') == {'mode':'auto','scope':'global'}, 'auto-saved-intent-invalid')
        servers = config.get('servers')
        gui_require(type(servers) is list and len(servers)==2 and
                    {s.get('id') for s in servers}=={'node-a','node-b'}, 'auto-members-invalid')
        # Private comparison only; never return full config or subscription credentials.
        return {'value':True,'intent':'global','membersSha256':hashlib.sha256(
            json.dumps(servers,sort_keys=True,separators=(',',':')).encode()).hexdigest()}

    def auto_runtime(self, wanted, deadline=None):
        observation={'schema':'polaris-auto-runtime-observation-v1','phase':'begin','state':'checking','checks':{}}
        self.result['autoRuntimeObservation']=observation
        try:
            return self._auto_runtime(wanted,observation,deadline=deadline)
        except Exception:
            observation['state']='failed'
            raise

    def _auto_runtime(self, wanted, observation, preparing=False, deadline=None):
        def check(phase, ok, code, facts=None):
            observation.update(phase=phase,state='observed' if ok else 'failed')
            observation['checks'][phase]={'passed':bool(ok),**(facts or {})}
            gui_require(ok,code)
        check('requested-mode',wanted is True,'auto-runtime-value-invalid')
        observation.update(phase='runtime-capture',state='checking')
        endpoint, proof, graph = self.observed_api_endpoint()
        check('endpoint',endpoint==self.api_endpoint,'auto-runtime-endpoint-changed')
        outbounds=graph.get('outbounds')
        check('outbound-schema',type(outbounds) is list and all(type(o) is dict for o in outbounds), 'auto-runtime-outbound-schema-invalid')
        def one(tag,kind,role):
            rows=[o for o in outbounds if o.get('tag')==tag]
            if preparing and not rows and role in ('auto-wrapper','native-urltest'):
                check(role,False,'auto-runtime-target-not-ready',{'count':0})
            check(role,len(rows)==1 and rows[0].get('type')==kind,'auto-runtime-'+role+'-invalid',{'count':len(rows)})
            return rows[0]
        selector=one('proxy-selector','selector','main-selector')
        wrapper=one('polaris-auto-global','selector','auto-wrapper')
        native=one('polaris-auto-global-test','urltest','native-urltest')
        for role,group in (('main-selector',selector),('auto-wrapper',wrapper),('native-urltest',native)):
            members=group.get('outbounds')
            check(role+'-members-schema',type(members) is list and all(type(t) is str and 0<len(t)<=512 for t in members)
                  and len(members)==len(set(members)), 'auto-runtime-'+role+'-members-schema-invalid')
        check('references',selector.get('default')=='polaris-auto-global' and
              'polaris-auto-global' in selector['outbounds'] and
              'polaris-auto-global-test' in wrapper['outbounds'] and
              wrapper.get('default')=='polaris-auto-global-test',
              'auto-runtime-target-not-ready' if preparing else 'auto-runtime-reference-invalid')
        for role,group in (('main-selector',selector),('auto-wrapper',wrapper),('native-urltest',native)):
            value=group.get('interrupt_exist_connections',False)
            check(role+'-interrupt',type(value) is bool and value is False,'auto-runtime-interrupt-invalid',
                  {'encoding':'omitted-false' if 'interrupt_exist_connections' not in group else 'explicit-boolean' if type(value) is bool else 'invalid'})
        observation.update(phase='saved-config-read',state='checking')
        config=self.config(); servers=config.get('servers');tags=[]
        check('fixture-members',type(servers) is list and len(servers)==2 and all(type(n) is dict for n in servers)
              and {n.get('id') for n in servers}=={'node-a','node-b'},'auto-runtime-fixture-members-invalid')
        for node in servers:
            matches=[o for o in outbounds if o.get('type')=='socks' and
                     o.get('server')==node.get('address') and type(o.get('server_port')) is int and
                     type(node.get('port')) is int and o['server_port']==node['port']]
            check('physical-'+node['id'][-1],len(matches)==1 and type(matches[0].get('tag')) is str
                  and 0<len(matches[0]['tag'])<=512,'auto-runtime-member-invalid',{'count':len(matches)})
            tags.append(matches[0]['tag'])
        digest=lambda values:hashlib.sha256(json.dumps(sorted(values),separators=(',',':')).encode()).hexdigest()
        members=native['outbounds']
        check('member-set',len(set(tags))==2 and set(members)==set(tags),'auto-runtime-member-set-differs',
              {'physicalCount':len(tags),'nativeCount':len(members),'physicalSha256':digest(tags),'nativeSha256':digest(members)})
        expected_wrapper=set(tags)|{'polaris-auto-global-test'}
        check('wrapper-member-set',set(wrapper['outbounds'])==expected_wrapper,'auto-runtime-wrapper-member-set-differs',
              {'expectedCount':len(expected_wrapper),'actualCount':len(wrapper['outbounds']),
               'expectedSha256':digest(expected_wrapper),'actualSha256':digest(wrapper['outbounds'])})
        interval=native.get('interval')
        # The fixed Rust producer emits 5m. The fixed Go badoption.Duration
        # MarshalJSON uses time.Duration.String and emits 5m0s. Both are exactly
        # 300 seconds; no tolerance, arbitrary parser, missing/default, or new cadence.
        duration_kind='rust-5m' if type(interval) is str and interval=='5m' else 'go-5m0s' if type(interval) is str and interval=='5m0s' else 'invalid'
        check('interval',duration_kind!='invalid','auto-runtime-interval-differs',
              {'encoding':duration_kind,'seconds':300 if duration_kind!='invalid' else None})
        url=native.get('url')
        check('health-url',type(url) is str and url==self.network.health_url,'auto-runtime-health-url-differs',
              {'matchesFixture':type(url) is str and url==self.network.health_url})
        if preparing:
            observation.update(phase='graph-ready',state='observed')
            return {'runtime':proof}
        observation.update(phase='status-read',state='checking')
        status=self.status()
        observation.update(phase='native-confirmation',state='checking')
        def observed():
            value=self.driver.invoke('auto_select_status')
            check('native-status-schema',type(value) is dict and type(value.get('nativeAutoGroups')) is list
                  and all(type(g) is dict for g in value['nativeAutoGroups']),'auto-status-schema-invalid')
            groups=value['nativeAutoGroups'];matches=[g for g in groups if g.get('target')=={'scope':'global'}]
            confirmed=(value.get('featureEnabled') is True and type(value.get('generation')) is int and
                value['generation']==status.get('mainGeneration') and value.get('actualServerId')=='node-a' and
                len(matches)==1 and matches[0].get('confirmed') is True and matches[0].get('state')=='auto'
                and matches[0].get('actualServerId')=='node-a')
            observation.update(phase='native-confirmation',state='observed' if confirmed else 'waiting')
            observation['checks']['native-confirmation']={'passed':confirmed,'globalGroupCount':len(matches),
                'generationMatches':type(value.get('generation')) is int and value['generation']==status.get('mainGeneration')}
            return value if confirmed else None
        try:
            left=35 if deadline is None else deadline-time.monotonic()
            gui_require(left>0,'observation-timeout')
            native_status=wait_for(observed,timeout=left)
            if deadline is not None: gui_require(time.monotonic()<deadline,'observation-timeout')
        except Exception:
            observation['state']='failed'
            raise
        observation.update(phase='generation-read',state='checking')
        current=self.status()
        check('generation',current.get('pid')==self.core['pid'] and current.get('mainGeneration')==status.get('mainGeneration')
              and current.get('actualServerId')=='node-a','auto-status-generation-changed')
        observation.update(phase='runtime-recapture',state='checking')
        after=self.runtime_identity()
        check('stable-runtime',after['sha256']==proof['sha256'] and after['pid']==proof['pid'] and
              after['startTime']==proof['startTime'],'auto-runtime-changed')
        observation.update(phase='complete',state='observed')
        return {'runtime':proof,'globalConfirmed':True,'actualServerId':'node-a',
                'mainGeneration':native_status['generation'],'selectorInterrupt':False,'intervalMinutes':5}

    def prepare_auto(self):
        gui_require(self.pending_apply_control()=={'bars':0,'buttons':0,'disabled':False},'auto-prior-pending-conflict')
        self.gui_phase('auto-feature-navigation')
        self.driver.click('.side .nav-item',self.text('sidebar.settings'))
        self.driver.click('.set-nav',self.text('settings.nav.network'))
        selector='[data-auto-select-scope="global"].auto-choice-switch button[role="switch"]'
        def feature():
            return self.driver.execute("""
const rows=[...document.querySelectorAll(arguments[0])].filter(e=>e.getClientRects().length&&getComputedStyle(e).visibility==='visible');
if(rows.length!==1)return {count:rows.length}; const e=rows[0];
return {count:1,checked:e.getAttribute('aria-checked'),disabled:!!e.disabled||e.getAttribute('aria-disabled')==='true',busy:e.closest('[data-auto-select-controls]').getAttribute('aria-busy')};
""",selector)
        value=feature()
        gui_require(type(value) is dict and value.get('count')==1 and value.get('checked') in ('true','false')
                    and value.get('disabled') is False and value.get('busy')=='false','auto-feature-control-invalid')
        config=self.config()
        gui_require(type(config.get('autoSelectEnabled')) is bool and
                    config['autoSelectEnabled']==(value['checked']=='true'), 'auto-feature-state-conflict')
        preparation={'featureBefore':config['autoSelectEnabled'],'featureClicked':False,'modeAssociation':'shape-only-click-window'}
        self.result['autoPreparation']=preparation
        before=self.status()
        gui_require(before.get('running') is True and not before.get('starting'),'auto-feature-core-not-ready')
        if value['checked']=='false':
            cursor=max((r['sequence'] for r in self.driver.observations()),default=0)
            self.gui_phase('auto-feature-enable')
            self.driver.click(selector)
            preparation['featureClicked']=True
            wait_for(feature,lambda v:v=={'count':1,'checked':'true','disabled':False,'busy':'false'})
            wait_for(self.config,lambda c:c.get('autoSelectEnabled') is True)
            def mode_receipt():
                rows=[r for r in self.driver.observations() if r.get('sequence',0)>cursor and r.get('source') in ('original-auto-mode-callback','original-auto-intent-compatible-callback','original-auto-receipt-invalid')]
                gui_require(len(rows)<=1,'auto-feature-callback-ambiguous')
                return rows[0] if rows else None
            receipt=wait_for(mode_receipt)
            gui_require(receipt.get('source')=='original-auto-mode-callback' and
                        receipt.get('success') is True and receipt.get('intentApplied') is True and
                        receipt.get('featureEnabled') is True and receipt.get('applyOutcome')=='applied' and
                        receipt.get('generation')==before.get('mainGeneration'),'auto-feature-mode-unconfirmed')
            preparation['modeReceipt']={k:receipt[k] for k in ('success','intentApplied','featureEnabled','generation','applyOutcome')}
        preparation['featureAfter']=True
        gui_require(not self.preference_pending_save(),'auto-feature-dirty-conflict')
        native=self.driver.invoke('auto_select_status')
        after=self.status()
        gui_require(native.get('featureEnabled') is True and native.get('generation')==before.get('mainGeneration')
                    and after.get('mainGeneration')==before.get('mainGeneration') and after.get('pid')==before.get('pid'),
                    'auto-feature-generation-changed')
        self.driver.click('.side .nav-item',self.text('settings.nav.back'))
        gui_require(self.pending_apply_control()=={'bars':0,'buttons':0,'disabled':False},'auto-feature-pending-conflict')
        self.pick('A')
        expanded=self.driver.execute("return document.querySelector('#node-trigger')?.getAttribute('aria-expanded');")
        gui_require(expanded in ('true','false'),'auto-menu-state-invalid')
        if expanded=='false':self.driver.click('#node-trigger')
        # Fence the existing user action, not a new session or mutation retry.
        action_session=self.driver.session
        action_fence={}
        self.readiness_observation_fence(action_session,action_fence)
        action_core=dict(self.core)
        action_status=self.status()
        self.same_core(action_core,status=action_status)
        gui_require(action_status.get('mainGeneration')==before.get('mainGeneration'), 'auto-intent-generation-changed')
        cursor=max((r['sequence'] for r in self.driver.observations()),default=0)
        self.gui_phase('auto-enable-click')
        self.driver.click('#node-menu [data-auto-select-scope="global"] .auto-choice-pick')
        self.gui_phase('auto-intent-callback')
        def intent_receipt():
            rows=[r for r in self.driver.observations() if r.get('sequence',0)>cursor and r.get('source') in ('original-auto-mode-callback','original-auto-intent-compatible-callback','original-auto-receipt-invalid')]
            gui_require(len(rows)<=1,'auto-intent-callback-ambiguous')
            return rows[0] if rows else None
        receipt=wait_for(intent_receipt)
        preparation['intentCallbackClass']=receipt.get('source') if receipt.get('source') in (
            'original-auto-mode-callback','original-auto-intent-compatible-callback','original-auto-receipt-invalid') else 'unknown'
        gui_require(receipt.get('source')=='original-auto-intent-compatible-callback' and 'applyOutcome' not in receipt and
                    receipt.get('success') is True and receipt.get('intentApplied') is True and
                    receipt.get('featureEnabled') is True and type(receipt.get('generation')) is int and
                    receipt['generation']==before.get('mainGeneration'), 'auto-intent-unconfirmed')
        preparation['intentAssociation']='shape-only-click-window'
        preparation['intentReceipt']={k:receipt[k] for k in ('success','intentApplied','featureEnabled','generation')}
        self.gui_phase('auto-intent-readback')
        intent_wait=self.new_status_observation('auto',time.monotonic()+30)
        def intent_readback():
            self.readiness_observation_fence(action_session,action_fence)
            config=self.config()
            control=self.driver.execute("""
const rows=[...document.querySelectorAll('#node-menu [data-auto-select-scope="global"] .auto-choice-pick')].filter(e=>e.getClientRects().length&&getComputedStyle(e).visibility==='visible');
if(rows.length!==1)return {count:rows.length};const e=rows[0];
return {count:1,pressed:e.getAttribute('aria-pressed'),disabled:!!e.disabled||e.getAttribute('aria-disabled')==='true',busy:e.closest('[data-auto-select-controls]').getAttribute('aria-busy')};
""")
            current=self.status()
            self.same_core(action_core,status=current)
            gui_require(current.get('mainGeneration')==action_status.get('mainGeneration'), 'auto-intent-generation-changed')
            saved=self.config()
            gui_require(saved==config,'auto-intent-saved-state-changed')
            self.readiness_observation_fence(action_session,action_fence)
            if (config.get('autoSelectEnabled') is True and
                    config.get('selectionIntent')=={'mode':'auto','scope':'global'} and
                    control=={'count':1,'pressed':'true','disabled':False,'busy':'false'}):
                gui_require(intent_receipt()==receipt,'auto-intent-callback-changed')
                return True
            return None
        self.wait_status_proof(intent_wait,intent_readback)
        preparation['intentReadback']={'savedGlobal':True,'selectedControl':True,'settled':True}
        self.gui_phase('auto-graph-activation')
        proof=self.activate_saved_graph(True,self.auto_saved,self.auto_runtime,'autoActivations',phase_prefix='auto')
        self.gui_phase('auto-baseline-ready')
        return proof

    def auto_case(self):
        self.gui_phase('auto-begin')
        self.stage = 'gui-auto'
        self.prepare_auto()
        old, before = self.socket('A')
        initial = dict(self.core)
        self.network.a.set_admission(False)
        deadline = time.monotonic() + 370
        proofs = []
        status_wait=self.new_status_observation('auto',deadline)
        switched = False
        while time.monotonic() < deadline:
            proofs.append(self.exchange(old, 'A'))
            require(same_process(self.core,process_identity(self.core['pid'])) and
                    self.core['startTime']==initial['startTime'], 'HOT unexpectedly replaced core')
            status=self.status_observation_sample(status_wait)
            if status is not None: self.same_core(initial,status=status)
            if status is not None and status.get('actualServerId') == 'node-b':
                gui_require(time.monotonic()<deadline,'observation-timeout')
                switched = True
                break
            time.sleep(3)
        require(switched, 'native Auto failed to switch within its real five-minute interval')
        _, fresh = self.socket('B')
        after = self.exchange(old, 'A')
        require(self.config().get('selectionIntent') is not None, 'Auto became manual')
        self.record('gui_auto_preserves_existing', before=before, oldAfter=after,
                    newConnection=fresh, keepaliveExchanges=len(proofs),
                    nativeIntervalMinutes=5, manuallyRequestedSwitch=False,
                    screenshot=self.snapshot('auto-b'))
        self.network.a.set_admission(True)
        self.pick('A')

    def graph_cases(self, patch):
        self.gui_phase('graph-begin')
        self.stage = 'gui-apply-tcp-dns'
        self.disconnect()
        desired = self.config()
        desired.update(patch)
        desired['language'] = 'en-US'
        desired['autoSelectEnabled'] = False
        desired['interruptConnectionsOnSwitch'] = False
        desired = save_fixture(self.driver, desired)
        self.pick('A')
        self.connect()
        baseline = dict(self.core)
        capabilities = self.rpc('capabilities')
        old, before = self.socket('A')
        snapshot_a = self.network.snapshot()
        desired_b = self.network.changed_graph(self.config())
        desired_b = save_fixture(self.driver, desired_b)
        applied = self.apply()
        require(applied.get('transactionId'), 'real GUI transaction missing')
        status = self.native_status(applied['transactionId'])
        operation = status['operation']
        require(status['lookupState'] == 1 and operation['durable'] is True
                and operation['persistenceMode'] == 'exact_configuration_file'
                and operation['active']['appliedRevision'] == capabilities['active']['appliedRevision'] + 1,
                'real GUI Apply identity/durability differs')
        self.same_core(baseline)
        old_after = self.exchange(old, 'A')
        fresh, new_after = self.socket('B')
        observed = self.network.snapshot()
        require(len(observed['dnsB']['queries']) > len(snapshot_a['dnsB']['queries'])
                and observed['dnsB']['queries'][-1]['answer'] == '127.0.0.3'
                and observed['dnsB']['queries'][-1]['ttl'] == 0,
                'fresh DNS B TTL0 observation missing')
        require(any(row['kind'] == 'echo' and row['host'] == '127.0.0.3' for row in observed['b']['targets']),
                'actual proxy destination did not use new DNS answer')
        self.record('gui_apply_tcp_dns', appResult=applied, operation=operation,
                    oldBefore=before, oldAfter=old_after, newAfter=new_after,
                    acceptedConfig=self.runtime_identity(), answerTtl=0,
                    screenshot=self.snapshot('hot-applied'))

        self.stage = 'gui-durability-status-retry'
        original = self.runtime_path.read_bytes()
        disturbed = original + b'\n'
        self.runtime_path.write_bytes(disturbed)
        pending_desired = self.config()
        pending_desired['logLevel'] = 'warn' if pending_desired.get('logLevel') != 'warn' else 'info'
        save_fixture(self.driver, pending_desired)
        pending = self.apply('hotDurabilityPending')
        transaction = pending.get('transactionId')
        require(transaction, 'durability transaction missing')
        published = self.native_status(transaction)['operation']
        require(published['durable'] is False and published['state'] == 4, 'durability pending not observed')
        for _ in range(3):
            polled = self.driver.invoke('proxy_runtime_apply_status', {'transactionId': transaction})
            require(polled.get('status') == 'hotDurabilityPending'
                    and polled.get('transactionId') == transaction
                    and self.runtime_path.read_bytes() == disturbed,
                    'status changed transaction or wrote persistence')
        require(self.runtime_path.read_bytes() == disturbed, 'private file changed outside fixture custody')
        self.runtime_path.write_bytes(original)
        recovered = self.apply(retry=transaction)
        terminal = self.native_status(transaction)['operation']
        require(terminal['durable'] is True and terminal['active'] == published['active'],
                'persistence retry republished graph')
        accepted = self.runtime_path.read_bytes()
        repeated = self.rpc('retry', {'transactionId': transaction, 'active': terminal['active'],
                                      'controlRevision': terminal['controlRevision'],
                                      'persistenceMode': terminal['persistenceMode']})
        require(repeated['active'] == terminal['active'] and repeated['durable'] is True
                and self.runtime_path.read_bytes() == accepted, 'terminal persistence retry not idempotent')
        self.same_core(baseline)
        self.record('gui_durability_status_retry', pending=pending, recovered=recovered,
                    operation=terminal, readOnlyStatusPolls=3,
                    originalSha256=hashlib.sha256(original).hexdigest(),
                    disturbedSha256=hashlib.sha256(disturbed).hexdigest(),
                    acceptedConfig=self.runtime_identity(), terminalRetrySource='authenticated-sideclient',
                    terminalRetry=repeated)

        self.stage = 'gui-rejected-preserves-old'
        source = self.runtime_path.read_bytes()
        before_cap = self.rpc('capabilities')
        wanted = self.config()
        structural = copy.deepcopy(wanted)
        structural['mixedPort'] = free_port()
        while structural['mixedPort'] in (self.mixed, self.fixture_control_port, self.api_endpoint['port']):
            structural['mixedPort'] = free_port()
        save_fixture(self.driver, structural)
        rejected = self.apply('restartRequired')
        self.gui_phase('restart-dialog-dismiss')
        self.driver.click('dialog[open][aria-labelledby="restart-apply-title"] button', self.text('home.restartApplyKeepConnection'))
        require(self.runtime_path.read_bytes() == source, 'declined restart changed runtime source')
        after_cap = self.rpc('capabilities')
        require(before_cap['active'] == after_cap['active'] and before_cap['controlRevision'] == after_cap['controlRevision'],
                'declined structural change mutated graph')
        self.same_core(baseline)
        old_rejected = self.exchange(fresh, 'B')
        invalid = copy.deepcopy(wanted)
        missing = 'pacc_missing0'
        require(not (Path('/sys/class/net') / missing).exists(), 'rejection fixture interface unexpectedly exists')
        invalid['networkInterfaces'] = {**(invalid.get('networkInterfaces') or {}), 'proxy': missing}
        # A valid desired-state string can be saved, but the real dispatcher
        # checks actual required NIC availability and returns rejected. Store
        # refusal or unknown API result is a failed precondition, never a pass.
        save_fixture(self.driver, invalid)
        invalid_result = self.apply('rejected')
        require(not invalid_result.get('transactionId'), 'invalid interface candidate acquired transaction')
        require(self.runtime_path.read_bytes() == source and self.rpc('capabilities')['active'] == before_cap['active'],
                'rejected candidate changed accepted runtime')
        save_fixture(self.driver, wanted)
        self.record('gui_rejected_preserves_old', structuralResult=rejected,
                    explicitRestartDeclined=True, rejectedAppResult=invalid_result,
                    rejectionFixture='nonexistent required proxy interface; no system interface change',
                    sourceUnchanged=True, oldConnection=old_rejected,
                    acceptedIdentity=before_cap['active'])
        return wanted

    def quit_product(self):
        self.gui_phase('quit-begin')
        self.api_endpoint = None
        self.stage = 'tray-quit'
        marker = self.runtime_path.parent / 'clean-exit.marker'
        require(not marker.exists() and not marker.is_symlink(), 'stale clean-exit marker before GUI quit')
        identities = [self.app, self.core]
        require(all(same_process(item, process_identity(item['pid'])) for item in identities), 'product exited before GUI quit')
        requested = time.time_ns()
        self.driver.quit_shortcut()
        wait_for(lambda: [process_identity(item['pid']) for item in identities],
                 lambda rows: all(not same_process(row, old) or row['state'] == 'Z'
                                  for row, old in zip(rows, identities)), timeout=45)
        wait_for(lambda: self.supervisor.living('product'), lambda rows: not rows, timeout=15)
        # Session deletion is tool cleanup only, after all product descendants exit.
        try:
            self.driver.close()
        except (ValueError, OSError):
            self.driver.session = None
        marker_proof = clean_exit_marker(marker, requested)
        self.normal_quit = True
        return {'identities': [{'pid': item['pid'], 'startTime': item['startTime']} for item in identities],
                'cleanExitMarker': marker_proof}

    def restart_case(self, wanted):
        self.gui_phase('restart-begin')
        self.stage = 'same-home-restart'
        previous_app, previous_core = dict(self.app), dict(self.core)
        accepted = self.runtime_identity()
        self.quit_product()
        self.normal_quit = False
        current = self.launch()
        require(not (self.runtime_path.parent / 'clean-exit.marker').exists(), 'restart did not consume clean marker')
        require(current.get('selectedServerId') == wanted.get('selectedServerId')
                and current.get('dnsRules') == wanted.get('dnsRules')
                and current.get('trafficRules') == wanted.get('trafficRules')
                and current.get('logLevel') == wanted.get('logLevel'), 'same HOME desired state was not restored')
        self.connect()
        require(not same_process(previous_app, self.app) and not same_process(previous_core, self.core),
                'restart did not launch new packaged processes')
        _, proof = self.socket('B')
        self.record('same_home_restart', sameHome=True, acceptedBefore=accepted,
                    restoredConfig=self.runtime_identity(), selected=current.get('selectedServerId'),
                    restoredDataPlane=proof, screenshot=self.snapshot('restarted'))

    def run(self):
        try:
            self.start_tools()
            initial = self.launch()
            require(initial.get('singboxDashboard') is True, 'empty HOME dashboard default is not true')
            require(initial.get('hardwareAcceleration') is not False, 'default graphics escape active')
            self.record('default_dashboard', emptyHome=True, observed=True,
                        defaultGraphics=True, screenshot=self.snapshot('default'))
            patch = self.network.config_patch(self.mixed, self.fixture_control_port, self.secret)
            selection_patch = copy.deepcopy(patch)
            for key in ('trafficRules', 'policyRules', 'customRules', 'routeRuleOrder'):
                selection_patch[key] = []
            selection_patch.update(language='en-US', autoConnect=False, autoSelectEnabled=True,
                                   periodicSpeedTestEnabled=False)
            # Legal minimum native URLTest interval: no product timing override.
            selection_patch['subscriptions'] = [{'id': 'package-fixture', 'name': 'Package loopback',
                'url': self.network.health_url, 'autoUpdate': False, 'createdAt': '2026-01-01T00:00:00Z',
                'periodicSpeedTest': False, 'speedTestIntervalMinutes': 5}]
            for node in selection_patch['servers']:
                node['subscriptionId'] = 'package-fixture'
            desired = copy.deepcopy(initial)
            desired.update(selection_patch)
            saved_fixture=save_fixture(self.driver, desired)
            gui_require(type(saved_fixture.get('autoSelectEnabled')) is bool,'fixture-auto-mode-schema-invalid')
            self.result['fixtureAutoSetup']={'requested':True,'saved':saved_fixture['autoSelectEnabled'],
                'authority':'backend-owned-not-config-save','sameAsRequested':saved_fixture['autoSelectEnabled'] is True}
            wait_for(lambda: self.driver.execute("return document.documentElement.lang"), lambda lang: lang.startswith('en'), timeout=15)
            self.selection_cases()
            self.auto_case()
            wanted = self.graph_cases(patch)
            # Test App-owned durable state restoration before the explicitly
            # separate sideclient suite changes only runtime log state.
            self.restart_case(wanted)
            self.stage = 'rpc-contract'
            rpc = self.rpc('suite', {'configContent': self.runtime_path.read_text()})
            require(len(rpc.get('methods', [])) == 6 and rpc.get('unauthenticatedPerMethod') == 2
                    and rpc.get('sameTransactionReplay') is True and rpc.get('cliRegenerationAckRejected') is True,
                    'six real authenticated RPC coverage missing')
            self.record('rpc_contract', source='independent authenticated sideclient, not GUI', **rpc)
            exited = self.quit_product()
            self.record('tray_quit', entry='native WebDriver Ctrl+Q -> original tray_quit',
                        normalProductExit=True, **exited)
        except Exception as error:
            self.result['failure'] = {'stage': self.stage, 'type': type(error).__name__, **gui_failure(error)}
            self.capture_driver_failure()
            if self.driver and self.driver.session:
                try:
                    self.result['failure']['screenshot'] = self.snapshot('failed')
                except Exception:
                    self.result['failure']['screenshotState'] = 'unavailable'
        finally:
            failures = []
            try:
                for stream in self.sockets:
                    try:
                        stream.close()
                    except Exception as error:
                        failures.append({'stage': 'fixture-stream-close', 'type': type(error).__name__})
                try:
                    self.network.close()
                except Exception as error:
                    failures.append({'stage': 'fixture-close', 'type': type(error).__name__})
                try:
                    self.result['network'] = self.network.snapshot()
                    if self.result['network']['healthFailures'] or any(self.result['network'][key]['failures'] for key in ('a', 'b', 'dnsA', 'dnsB')):
                        failures.append({'stage': 'fixture-health', 'type': 'Unconfirmed'})
                except Exception as error:
                    failures.append({'stage': 'fixture-snapshot', 'type': type(error).__name__})
            finally:
                cleanup = self.supervisor.close(self.normal_quit)
                self.result['cleanup'] = cleanup
            if failures:
                cleanup['state'] = 'failed'
                cleanup['unknown'].append('fixture-cleanup')
                cleanup['failures'] = failures
            try:
                self.result['ownedNetworkObservation'] = self.supervisor.network_evidence()
            except Exception as error:
                cleanup['state'] = 'failed'
                cleanup['unknown'].append('network-evidence-unavailable')
                cleanup.setdefault('failures', []).append({'stage': 'network-evidence', 'type': type(error).__name__})
            if cleanup['state'] == 'normal':
                self.record('cleanup', **cleanup)
            self.result['result'] = 'success' if set(self.result['cases']) == set(CASES) and 'failure' not in self.result and cleanup['state'] == 'normal' else 'failed'
        return self.result


class AcceptanceCancelled(Exception):
    pass


def cancellation(_signum, _frame):
    raise AcceptanceCancelled('acceptance cancelled')


def run_all(index, packages, rpc_client, output):
    require(os.geteuid() != 0 and os.environ.get('GITHUB_ACTIONS') == 'true', 'dedicated nonroot GitHub VM required')
    require(not any(key in os.environ for key in FORBIDDEN), 'unsafe inherited acceptance environment')
    require(not output.exists(), 'fresh acceptance output required')
    # Re-use the genuine committed formal admission without altering its env.
    subprocess.run(['node', '--input-type=module', '-e',
                    "import {formalGateContext} from './scripts/desktop-core/native-dns-gates.mjs'; formalGateContext(process.cwd());"],
                   cwd=ROOT, check=True, timeout=30, stdout=subprocess.DEVNULL)
    transport = json.loads(regular(index).read_text())
    carriers = transport['freeze']['carriers']
    require(len(carriers) == 2 and {item['kind'] for item in carriers} == {'deb', 'AppImage'}, 'both frozen carriers required')
    require(regular(rpc_client).is_file(), 'test-side RPC client required')
    output.parent.mkdir(parents=True, exist_ok=True)
    fixtures = load_fixtures()
    result = {'schema': SCHEMA, 'result': 'failed', 'carriers': [],
              'transportSha256': sha(index), 'cleanup': {'state': 'failed', 'forced': [], 'remaining': []}}
    previous_signals = {sig: signal.signal(sig, cancellation) for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        for item in carriers:
            package = carrier_path(packages, item)
            directory = Path(tempfile.mkdtemp(prefix='polaris-final-package-'))
            receipt = {'kind': item['kind'], 'result': 'failed', 'cases': {},
                       'cleanup': {'state': 'failed', 'forced': [], 'remaining': []}}
            result['carriers'].append(receipt)
            case = None
            try:
                case = CarrierRun(item, package, directory, output.parent, rpc_client, fixtures)
                receipt.update(case.run())
                require(sha(package) == item['sha256'], 'carrier changed during execution')
            except Exception as error:
                if case is not None:
                    receipt.update(case.result)
                receipt.setdefault('failure', {'stage': 'carrier-run', 'type': type(error).__name__})
                receipt['result'] = 'failed'
            finally:
                receipt['workspaceCleanup'] = cleanup_workspace(directory,
                    processes_stopped=receipt['cleanup'].get('remainingAssessment') == 'observed'
                    and not receipt['cleanup']['remaining']
                    and not receipt['cleanup'].get('unknown'))
                if receipt['workspaceCleanup']['state'] != 'normal':
                    receipt['cleanup']['state'] = 'failed'
                    receipt['cleanup'].setdefault('unknown', []).append('workspace-cleanup-unconfirmed')
                    receipt['result'] = 'failed'
            require(receipt['result'] == 'success', 'mandatory native carrier scenario failed')
        cleanups = [item['cleanup'] for item in result['carriers']]
        result['cleanup'] = {'state': 'normal', 'forced': [p for c in cleanups for p in c['forced']],
                             'remaining': [p for c in cleanups for p in c['remaining']]}
        require(not result['cleanup']['forced'] and not result['cleanup']['remaining'], 'cleanup not normal')
        result['result'] = 'success'
    finally:
        for sig, handler in previous_signals.items():
            signal.signal(sig, handler)
        output.write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    inspect_parser = sub.add_parser('inspect')
    inspect_parser.add_argument('--deb', type=Path, required=True)
    inspect_parser.add_argument('--appimage', type=Path, required=True)
    run_parser = sub.add_parser('run')
    for name in ('transport', 'packages', 'rpc-client', 'output'):
        run_parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'inspect':
        print(json.dumps({'carriers': [inspect_carrier(args.deb, 'deb'), inspect_carrier(args.appimage, 'AppImage')]}))
    else:
        run_all(args.transport.absolute(), args.packages.absolute(), args.rpc_client.absolute(), args.output.absolute())


if __name__ == '__main__':
    main()
