#!/usr/bin/env python3
"""Run the frozen Linux carriers, not a recompiled App or browser mock.

`inspect` only extracts bytes. `run` is an exact-source GitHub Linux VM gate.
Only fixture setup uses config_save; every user action uses native WebDriver.
No root, FUSE, system installation, helper, TUN or sandbox workarounds.
"""
import argparse
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
import struct
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

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



def stable_runtime_capture(path, home, identity, destination):
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
    return proof


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
        self.forced = []
        self.network = {}
        self.network_errors = set()
        self.network_truncated = False
        self.network_next = 0.0
        self.thread = threading.Thread(target=self._monitor, daemon=False)
        self.thread.start()

    def _monitor(self):
        while not self.stop.wait(0.05):
            try:
                observed = self.scan()
                if time.monotonic() >= self.network_next:
                    self.observe_sockets(observed)
                    self.network_next = time.monotonic() + 0.5
            except Exception:
                self.unknown.append('process-inventory-unreadable')

    def scan(self):
        observed = {int(p.name): process_identity(int(p.name)) for p in Path('/proc').iterdir() if p.name.isdigit()}
        observed = {pid: item for pid, item in observed.items() if item}
        with self.lock:
            changed = True
            while changed:
                changed = False
                for pid, item in observed.items():
                    if same_process(item, self.owned.get(pid)):
                        require(item['uid'] == os.getuid() and self.owned[pid]['uid'] == os.getuid(),
                                'owned birth changed UID unexpectedly')
                        continue
                    parent = observed.get(item['ppid'])
                    if same_process(parent, self.owned.get(item['ppid'])):
                        require(item['uid'] == os.getuid(), 'owned descendant elevated unexpectedly')
                        self.owned[pid] = item
                        self.roles[pid] = self.roles.get(item['ppid'], 'unknown')
                        changed = True
                    elif item['ppid'] == os.getpid() and pid not in self.children:
                        # An adopted descendant was not observed before reparenting.
                        # It is ours to reap, but unknown lineage must fail the gate.
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
            item = process_identity(child.pid)
            require(item is not None, 'spawned process vanished before identity capture')
            self.children[child.pid] = child
            self.owned[child.pid] = item
            self.roles[child.pid] = role
            return child

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
        for child in self.children.values():
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.unknown.append('child-wait-timeout')
        while True:
            try:
                if os.waitpid(-1, os.WNOHANG)[0] == 0:
                    break
            except ChildProcessError:
                break
        remaining = [{'pid': p['pid'], 'startTime': p['startTime']} for p in self.living()]
        self.stop.set()
        self.thread.join(2)
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
        require(time.monotonic() < until, 'bounded observation timed out')
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
      // Callback replies do not identify commands. Capture only the original
      // RuntimeApplyResult shape, then correlate a click cursor with real status.
      if (states.has(data?.status) && typeof data.ok === 'boolean' &&
          typeof data.pendingApply === 'boolean' && typeof data.restartRequired === 'boolean' &&
          Array.isArray(data.reasons)) {
        observation.rows.push({sequence: ++observation.cursor,
          source:'original-native-callback', success:raw?.success === true,
          status:data.status, transactionId:data.transactionId,
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


class NativeRejected(ValueError):
    """Actual native failure envelope, not timeout or unknown outcome."""


class WebDriver:
    def __init__(self, port):
        self.url = f'http://127.0.0.1:{port}'
        self.session = None

    def request(self, method, path, data=None, timeout=25):
        raw = None if data is None else json.dumps(data).encode()
        request = urllib.request.Request(self.url + path, data=raw, method=method,
                                         headers={'Content-Type': 'application/json'})
        try:
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open(request, timeout=timeout) as response:
                body = json.load(response)
        except urllib.error.HTTPError as error:
            # Never echo remote response strings: they may contain fixture secrets.
            raise ValueError(f'WebDriver HTTP {error.code}') from None
        value = body.get('value')
        require(not isinstance(value, dict) or 'error' not in value, 'native WebDriver command failed')
        return value

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
        value = self.request('POST', self.path('/execute/async'), {'script': r'''
const [command, args, done] = arguments;
if (!window.__TAURI_INTERNALS__?.invoke) {done({error:'not-native'});return;}
window.__TAURI_INTERNALS__.invoke(command,args).then(raw => {
 if(raw?.success !== true) done({error:'native-command-rejected',rejected:raw?.success === false});
 else done({ok:true,data:raw.data ?? null});
},()=>done({error:'native-command-failed'}));
''', 'args': [command, args or {}]})
        if value and value.get('rejected') is True:
            raise NativeRejected('actual native fixture/read rejection')
        require(value and value.get('ok') is True, 'native IPC outcome unknown')
        return value.get('data')

    def element(self, selector, text=None):
        # Returning an element then using /click produces a real WebDriver input,
        # unlike executing element.click() or dispatching synthetic JS events.
        return self.execute(r'''
const matches = [...document.querySelectorAll(arguments[0])].filter(e =>
 e.getClientRects().length && !e.disabled && (arguments[1] === null ||
 e.textContent.trim() === arguments[1] || e.getAttribute('aria-label') === arguments[1]));
if(matches.length !== 1) return null;
return matches[0];
''', selector, text)

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


def save_fixture(driver, desired):
    before = driver.invoke('config_get')
    value = driver.invoke('config_save', {'config': desired, 'deferRestart': True,
                                          'baseVersion': config_version(before)})
    require(value and value.get('status') == 'saved', 'fixture config save conflicted')
    return value['config']


def carrier_path(directory, item):
    paths = list(directory.rglob(item['name']))
    require(len(paths) == 1, 'exact carrier inventory required')
    path = regular(paths[0], directory)
    require(path.stat().st_size == item['bytes'] and sha(path) == item['sha256'], 'frozen carrier differs')
    return path


class CarrierRun:
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
        self.mixed, self.control = free_port(), free_port()
        while self.control == self.mixed:
            self.control = free_port()
        self.core = None
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
        bus = self.supervisor.spawn(['dbus-daemon', '--session', '--nofork', '--print-address=1'],
                                   self.env, 'dbus', stdout=subprocess.PIPE)
        require(select.select([bus.stdout], [], [], 15)[0], 'private DBus unavailable')
        address = bus.stdout.readline(4096).decode().strip()
        require(address.startswith('unix:') and len(address) < 4096, 'private DBus address differs')
        self.env['DBUS_SESSION_BUS_ADDRESS'] = address
        driver = shutil.which('tauri-driver')
        native = shutil.which('WebKitWebDriver')
        require(driver and native, 'native automation tooling missing')
        version_process = self.supervisor.spawn([driver, '--version'], self.env, 'tool-version', stdout=subprocess.PIPE)
        version = version_process.communicate(timeout=10)[0].decode().strip()
        require(version_process.returncode == 0, 'automation version query failed')
        require(version == 'tauri-driver 2.0.5', 'exact reviewed tauri-driver required')
        self.result['tools'] = {'automation': version, 'nativeDriver': byte_identity(Path(native).resolve()),
                                'xvfb': 'private display', 'dbus': 'private session'}
        self.driver_port, native_port = free_port(), free_port()
        while native_port == self.driver_port:
            native_port = free_port()
        self.supervisor.spawn([driver, '--port', str(self.driver_port), '--native-port', str(native_port),
                               '--native-driver', native], self.env, 'driver')
        self.driver = WebDriver(self.driver_port)
        def available():
            try:
                return self.driver.request('GET', '/status')
            except (OSError, ValueError):
                return None
        wait_for(available, lambda row: bool(row and row.get('ready') is True), timeout=30)

    def launch(self):
        self.driver.start(self.entry)
        wait_for(lambda: self.driver.execute("return document.querySelector('#s-home') !== null && '__TAURI_INTERNALS__' in window;"), timeout=60)
        app = self.supervisor.executable(self.payload / self.members['app']['path'])
        env_raw = (Path('/proc') / str(app['pid']) / 'environ').read_bytes().split(b'\0')
        names = {line.split(b'=', 1)[0].decode(errors='replace') for line in env_raw if line}
        require(not names.intersection(FORBIDDEN)
                and not any('TOKEN' in name or name.startswith(('GITHUB_', 'ACTIONS_', 'RUNNER_', 'AWS_')) for name in names),
                'product inherited automation escape or runner credentials')
        self.app = app
        return self.driver.invoke('config_get')

    def snapshot(self, label):
        return self.driver.screenshot(self.evidence_dir / f'{self.item["kind"]}-{label}.png')

    def config(self):
        return self.driver.invoke('config_get')

    def status(self):
        return self.driver.invoke('proxy_get_status')

    def connect(self):
        require(not self.status().get('running'), 'connect requires stopped core')
        self.driver.click('#connect-btn')
        status = wait_for(self.status, lambda s: s.get('running') is True and not s.get('starting'), timeout=75)
        require(status.get('startedViaHelper') is not True, 'privileged helper prohibited')
        core = self.supervisor.executable(self.payload / self.members['core']['path'])
        require(status.get('pid') == core['pid'], 'App status is not owned packaged core')
        self.core = core
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
        content = json.loads(self.runtime_path.read_text())
        require(all(item.get('type') != 'tun' for item in content.get('inbounds', [])), 'TUN not permitted')
        api = [item for item in content.get('services', []) if item.get('type') == 'api']
        require(len(api) == 1 and api[0].get('listen') == '127.0.0.1'
                and api[0].get('listen_port') == self.control and api[0].get('secret') == self.secret,
                'actual App-owned API differs')
        maps = (Path('/proc') / str(core['pid']) / 'maps').read_text()
        expected = self.payload / self.members['cronet']['path']
        mapping = cronet_mapping(maps, expected, self.members['cronet']['sha256'])
        self.result['execution'] = {'uid': os.getuid(), 'defaultSandbox': True, 'originalEntry': True,
            'automation': 'tauri-driver 2.0.5',
            'app': {key: self.app[key] for key in ('sha256', 'pid', 'startTime')},
            'core': {key: core[key] for key in ('sha256', 'pid', 'startTime')},
            'cronet': {**mapping, 'path': self.members['cronet']['path']}}
        self.result['configIdentity'] = self.runtime_identity()
        self.result['cases']['default_dashboard']['evidence'].setdefault(
            'actualGeneratedRuntime', self.result['configIdentity'])
        return status

    def disconnect(self):
        before = self.core
        require(self.status().get('running'), 'disconnect requires running core')
        self.driver.click('#connect-btn')
        wait_for(self.status, lambda s: s.get('running') is False and not s.get('starting'), timeout=45)
        wait_for(lambda: process_identity(before['pid']),
                 lambda p: not same_process(p, before) or p['state'] == 'Z', timeout=15)

    def pick(self, label):
        if not self.driver.element('#node-menu .nm-name', 'Loop ' + label):
            self.driver.click('#node-trigger')
        self.driver.click('#node-menu .nm-name', 'Loop ' + label)
        wait_for(self.config, lambda c: c.get('selectedServerId') == 'node-' + label.lower()
                 and not c.get('selectionIntent'), timeout=30)
        if self.status().get('running'):
            wait_for(self.status, lambda s: s.get('actualServerId') == 'node-' + label.lower(), timeout=45)

    def socket(self, label):
        stream = self.fixtures.socks_connect(('127.0.0.1', self.mixed), self.network.domain, 80)
        self.sockets.append(stream)
        proof = self.fixtures.exchange(stream, label, os.urandom(18).hex())
        return stream, proof

    def exchange(self, stream, label):
        return self.fixtures.exchange(stream, label, os.urandom(18).hex())

    def runtime_identity(self):
        path = regular(self.runtime_path, self.home)
        return stable_runtime_capture(path, self.home, self.core, self.workspace / 'private-runtime-originals')

    def rpc(self, action, request=None):
        require(same_process(self.core, process_identity(self.core['pid'])), 'owned core identity changed')
        data = {'action': action, 'host': '127.0.0.1', 'port': self.control, 'secret': self.secret,
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
        return result['result']

    def apply(self, expected='hotApplied', retry=None):
        observations = self.driver.observations()
        cursor = max((row['sequence'] for row in observations), default=0)
        label = 'home.pendingRetryPersistenceBtn' if retry else 'home.pendingApplyBtn'
        self.driver.click('.pending-bar .pb-acts button', self.text(label))
        def changed():
            rows = self.driver.observations()
            matches = [row for row in rows if row.get('sequence', 0) > cursor
                       and row.get('source') == 'original-native-callback']
            if retry:
                matches = [row for row in matches if row.get('transactionId') == retry]
            return matches[0] if matches else None
        result = wait_for(changed, timeout=65)
        require(result.get('success') is True, 'GUI Apply outcome unknown; no automatic resubmission')
        transaction = result.get('transactionId')
        if result.get('status') == 'unconfirmed':
            require(transaction, 'unknown Apply without transaction is terminal failure')
            result = wait_for(lambda: self.driver.invoke('proxy_runtime_apply_status', {'transactionId': transaction}),
                              lambda row: row.get('status') != 'unconfirmed', timeout=60)
        require(result.get('status') == expected, 'GUI Apply did not reach required outcome')
        if retry:
            require(transaction == retry, 'GUI retried another transaction')
        return result

    def native_status(self, transaction):
        capability = self.rpc('capabilities')
        return self.rpc('status', {'transactionId': transaction,
                                  'runtimeIncarnation': capability['active']['incarnation']})

    def same_core(self, baseline):
        require(same_process(self.core, process_identity(self.core['pid']))
                and self.status().get('pid') == baseline['pid']
                and self.core['startTime'] == baseline['startTime'], 'HOT unexpectedly replaced core')

    def selection_cases(self):
        self.stage = 'gui-selection'
        self.pick('A')
        self.connect()
        first, first_proof = self.socket('A')
        self.pick('B')
        old_after = self.exchange(first, 'A')
        second, second_proof = self.socket('B')
        self.record('gui_selection', selected='node-b', before=first_proof,
                    newConnection=second_proof, screenshot=self.snapshot('selected-b'))
        # Toggle the real preference control. This setting is a user action, not a
        # fixture bypass; native config readback proves which policy was exercised.
        self.driver.click('.side .nav-item', self.text('sidebar.settings'))
        self.driver.click('.set-nav', self.text('settings.nav.network'))
        switch = self.driver.execute(r'''
const rows=[...document.querySelectorAll('.set-row')].filter(e=>e.textContent.includes(arguments[0]));
const buttons=rows.flatMap(e=>[...e.querySelectorAll('button[role=switch]')]);
return buttons.length===1?buttons[0]:null;
''', self.text('settings.network.interruptOnSwitch'))
        require(switch and ELEMENT in switch, 'real interrupt preference switch unavailable')
        self.driver.request('POST', self.driver.path('/element/' + switch[ELEMENT] + '/click'), {})
        # Settings can stage the preference. Use its real Save control if present.
        save = self.driver.element('.pending-bar .pb-acts button', self.text('home.pendingSaveBtn'))
        if save:
            self.driver.click('.pending-bar .pb-acts button', self.text('home.pendingSaveBtn'))
        wait_for(self.config, lambda c: c.get('interruptConnectionsOnSwitch') is True)
        self.driver.click('.side .nav-item', self.text('settings.nav.back'))
        self.pick('A')
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
                    interruptPreferenceObserved=True, oldBClosed=True)
        self.pick('B')
        saved = self.config()['selectedServerId']
        old_core = dict(self.core)
        self.disconnect()
        require(self.config()['selectedServerId'] == saved, 'OFF changed selection')
        self.connect()
        require(self.config()['selectedServerId'] == saved and not same_process(old_core, self.core), 'ON did not restore selection')
        _, proof = self.socket('B')
        self.record('gui_on_off', selected=saved, newProcess=True, restoredConnection=proof)

    def auto_case(self):
        self.stage = 'gui-auto'
        self.pick('A')
        self.driver.click('#node-trigger')
        self.driver.click('[data-auto-select-scope="global"] .auto-choice-pick')
        wait_for(self.config, lambda c: c.get('selectionIntent') is not None)
        wait_for(self.status, lambda s: s.get('actualServerId') == 'node-a', timeout=45)
        old, before = self.socket('A')
        initial = dict(self.core)
        self.network.a.set_admission(False)
        deadline = time.monotonic() + 370
        proofs = []
        switched = False
        while time.monotonic() < deadline:
            proofs.append(self.exchange(old, 'A'))
            self.same_core(initial)
            if self.status().get('actualServerId') == 'node-b':
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
        while structural['mixedPort'] in (self.mixed, self.control):
            structural['mixedPort'] = free_port()
        save_fixture(self.driver, structural)
        rejected = self.apply('restartRequired')
        self.driver.click('dialog button', self.text('home.restartApplyKeepConnection'))
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
            patch = self.network.config_patch(self.mixed, self.control, self.secret)
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
            save_fixture(self.driver, desired)
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
            self.result['failure'] = {'stage': self.stage, 'type': type(error).__name__}
            if self.driver and self.driver.session:
                try:
                    self.result['failure']['screenshot'] = self.snapshot('failed')
                except Exception:
                    self.result['failure']['screenshotState'] = 'unavailable'
        finally:
            for stream in self.sockets:
                stream.close()
            fixture_cleanup = True
            try:
                self.network.close()
            except Exception:
                fixture_cleanup = False
            self.result['network'] = self.network.snapshot()
            if self.result['network']['healthFailures'] or any(self.result['network'][key]['failures'] for key in ('a', 'b', 'dnsA', 'dnsB')):
                fixture_cleanup = False
            cleanup = self.supervisor.close(self.normal_quit)
            if not fixture_cleanup:
                cleanup['state'] = 'failed'
                cleanup['unknown'].append('fixture-cleanup')
            self.result['ownedNetworkObservation'] = self.supervisor.network_evidence()
            self.result['cleanup'] = cleanup
            if cleanup['state'] == 'normal':
                self.record('cleanup', **cleanup)
            self.result['result'] = 'success' if set(self.result['cases']) == set(CASES) and 'failure' not in self.result else 'failed'
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
            with tempfile.TemporaryDirectory(prefix='polaris-final-package-') as directory:
                case = CarrierRun(item, package, Path(directory), output.parent, rpc_client, fixtures)
                receipt = case.run()
                result['carriers'].append(receipt)
                require(sha(package) == item['sha256'], 'carrier changed during execution')
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
