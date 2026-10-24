#!/usr/bin/env python3
"""Narrow TUN VM support. No import-time action, auth changes or product launcher.

The ordinary-UID GUI consumer owns normal product install/stop/uninstall. This
module owns only its fresh VM ledger, fixture topology and FAIL-only watchdog.
Watchdog is frozen root-owned code, uses stdlib only, and never imports GUI code.
"""
from __future__ import annotations
import argparse
import hashlib
import ipaddress
import json
import os
import pwd
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import time

SCHEMA = 'polaris-linux-tun-support-v1'
KERNEL_COMMIT = 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a'
BASE = Path('/run/polaris-tun-acceptance')
FIXTURE_BASE = Path('/run/polaris-tun-fixtures')
SERVICE = 'polaris-helper.service'
HELPER = Path('/usr/local/lib/polaris/helper')
UNIT = Path('/etc/systemd/system') / SERVICE
IP = '/usr/sbin/ip'
SYSTEMCTL = '/usr/bin/systemctl'
ENV = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C', 'LC_ALL': 'C'}
CASE_IDS = ('default_graph', 'tun_data_plane', 'graph_apply', 'daily_switch',
            'auto_preserves', 'manual_preserve', 'manual_disconnect',
            'uncovered_target_restart', 'synthetic_route', 'normal_cleanup')


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def token(value):
    require(isinstance(value, str) and re.fullmatch(r'[0-9a-f]{12}', value), '12-hex run token required')
    return value


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':')) + '\n').encode()


def root_path(path, directory=False):
    path = Path(path)
    require(path.is_absolute(), 'absolute root path required')
    for parent in [*reversed(path.parents), path]:
        info = parent.lstat()
        require(not stat.S_ISLNK(info.st_mode) and info.st_uid == 0 and not info.st_mode & 0o022,
                'root-owned nonredirected nonwritable path required')
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode), 'root path type differs')
    if not directory:
        require(info.st_nlink == 1, 'root file must have one link')
    return path


def file_identity(path):
    path = root_path(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        first = os.fstat(fd)
        require(first.st_size <= 128 * 1024 * 1024, 'bounded root file required')
        h = hashlib.sha256()
        while block := os.read(fd, 1024 * 1024):
            h.update(block)
        last = os.fstat(fd)
        require((first.st_dev, first.st_ino, first.st_size, first.st_mtime_ns) ==
                (last.st_dev, last.st_ino, last.st_size, last.st_mtime_ns), 'file changed while read')
        return {'dev': first.st_dev, 'inode': first.st_ino, 'sha256': h.hexdigest()}
    finally:
        os.close(fd)


def process_identity(pid):
    """Birth and namespace identity; absence is distinct from unreadable evidence."""
    require(type(pid) is int and pid > 1, 'bounded non-init PID required')
    p = Path('/proc') / str(pid)
    try:
        raw = (p / 'stat').read_text()
        fields = raw[raw.rindex(')') + 2:].split()
        status = dict(line.split(':', 1) for line in (p / 'status').read_text().splitlines() if ':' in line)
        image = (p / 'exe').stat()
        namespaces = {name: (p / 'ns' / name).stat().st_ino for name in ('net', 'mnt', 'pid', 'cgroup')}
        result = {'pid': pid, 'startTime': fields[19], 'state': fields[0], 'ppid': int(fields[1]),
                  'uid': int(status['Uid'].split()[0]), 'exeDev': image.st_dev, 'exeInode': image.st_ino,
                  'exe': os.readlink(p / 'exe'), 'namespaces': namespaces,
                  'cgroup': (p / 'cgroup').read_text().strip(),
                  'capEff': int(status['CapEff'].strip(), 16), 'capAmb': int(status['CapAmb'].strip(), 16)}
        again = (p / 'stat').read_text()
        require(again[again.rindex(')') + 2:].split()[19] == result['startTime'], 'PID changed during observation')
        return result
    except FileNotFoundError:
        return None


def same_process(a, b):
    keys = ('pid', 'startTime', 'uid', 'exeDev', 'exeInode', 'namespaces', 'cgroup')
    def complete(v):
        return (isinstance(v, dict) and all(k in v for k in keys) and type(v['pid']) is int and v['pid'] > 1
                and isinstance(v['startTime'], str) and v['startTime'].isdigit() and int(v['startTime']) > 0
                and all(type(v[k]) is int and v[k] >= 0 for k in ('uid', 'exeDev', 'exeInode'))
                and isinstance(v['cgroup'], str) and bool(v['cgroup']) and isinstance(v['namespaces'], dict)
                and set(v['namespaces']) == {'net', 'mnt', 'pid', 'cgroup'}
                and all(type(n) is int and n > 0 for n in v['namespaces'].values()))
    return bool(complete(a) and complete(b) and all(a[k] == b[k] for k in keys))


class Commands:
    """Fixed argv supplied by this module, never a shell or GUI command string."""
    def run(self, argv, timeout=15):
        require(isinstance(argv, list) and argv and argv[0] in (IP, SYSTEMCTL, '/usr/bin/systemd-run',
                '/usr/bin/resolvectl', '/usr/sbin/nft'), 'unsupported system command')
        result = subprocess.run(argv, env=ENV, stdin=subprocess.DEVNULL, capture_output=True,
                                check=True, timeout=timeout)
        require(len(result.stdout) <= 8 * 1024 * 1024, 'bounded observation required')
        return result.stdout.decode()

    def json(self, argv):
        return json.loads(self.run(argv))

    def proc(self, pid):
        return process_identity(pid)

    def processes_in(self, inode):
        found = []
        for item in Path('/proc').iterdir():
            if not item.name.isdigit() or int(item.name) <= 1:
                continue
            try:
                current = self.proc(int(item.name))
                if current and current['namespaces']['net'] == inode:
                    found.append(current)
            except PermissionError:
                raise ValueError('namespace process census unreadable') from None
        return found

    def signal_owned(self, identity, sig):
        require(hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal'), 'pidfd required')
        fd = os.pidfd_open(identity['pid'], 0)
        try:
            require(same_process(identity, self.proc(identity['pid'])), 'PID birth/namespace changed before signal')
            signal.pidfd_send_signal(fd, sig)
            return bool(select.select([fd], [], [], 5)[0])
        finally:
            os.close(fd)

    def cgroup_closed(self):
        path = Path('/sys/fs/cgroup/system.slice') / SERVICE
        root_path(path.parent, directory=True)
        if not path.exists():
            return True
        root_path(path, directory=True)
        events = dict(line.split() for line in (path / 'cgroup.events').read_text().splitlines())
        return events.get('populated') == '0'

    def namespace_inode(self, name):
        try:
            return (Path('/run/netns') / name).stat().st_ino
        except FileNotFoundError:
            return None


def topology(run_id):
    """One backend namespace/veth; A/B are distinct bound source addresses.

The client->nonloopback origin crosses the real product TUN. A/B->origin is
local to this controlled backend; this is NOT independent physical egress.
"""
    run_id = token(run_id); p = 'pt' + run_id[:8]
    return {'runId': run_id, 'namespaces': {r: p + 'n' for r in ('A', 'B', 'O')},
            'hostLinks': {r: p + 'v0' for r in ('A', 'B')},
            'endpoint': {'A': '10.231.240.2', 'B': '10.231.240.3'},
            'host': {'A': '10.231.240.1', 'B': '10.231.240.1'},
            'exit': {'A': '10.231.240.2', 'B': '10.231.240.3'},
            'origin': '198.18.240.10', 'special': '198.18.240.20',
            'cidrs': ['10.231.240.0/29', '198.18.240.0/24'],
            'ports': {'proxy': 1080, 'dns': 5353, 'echo': 48080, 'health': 48081, 'special': 48082}}


def network_snapshot(ops, progress=None):
    def observe(name, call):
        if progress: progress(name)
        return call()
    value = {'links': observe('links', lambda: ops.json([IP, '-j', 'link', 'show'])),
             'addresses': observe('addresses', lambda: ops.json([IP, '-j', 'address', 'show'])),
             'route4': observe('route4', lambda: ops.json([IP, '-j', '-N', '-4', 'route', 'show', 'table', 'all'])),
             'route6': observe('route6', lambda: ops.json([IP, '-j', '-N', '-6', 'route', 'show', 'table', 'all'])),
             'rule4': observe('rule4', lambda: ops.json([IP, '-j', '-4', 'rule', 'show'])),
             'rule6': observe('rule6', lambda: ops.json([IP, '-j', '-6', 'rule', 'show'])),
             'resolved': observe('resolved', lambda: ops.run(['/usr/bin/resolvectl', 'status', '--no-pager'])),
             'firewall': observe('firewall', lambda: ops.json(['/usr/sbin/nft', '-j', 'list', 'ruleset']))}
    if progress: progress('resolv-conf')
    resolver = Path('/etc/resolv.conf')
    value['resolvConf'] = {'symlink': os.readlink(resolver) if resolver.is_symlink() else None,
                           'sha256': digest(resolver.read_bytes())}
    if progress: progress('network-forwarding')
    value['forwarding'] = [int(Path(p).read_text()) for p in
        ('/proc/sys/net/ipv4/ip_forward', '/proc/sys/net/ipv6/conf/all/forwarding')]
    return value


def tun_table_routes(snapshot):
    """Select the owned table from successful all-table dumps of both families.

    A fresh host need not have FIB table 2022 at all. Never turn a failed
    observation into an empty table or coerce floats/bools into table IDs.
    """
    selected = []
    for family in ('route4', 'route6'):
        rows = snapshot.get(family)
        require(type(rows) is list, 'complete all-table route observation required')
        for row in rows:
            require(type(row) is dict, 'route observation must be an object')
            table = row.get('table')
            require(table is None or type(table) in (int, str), 'route table identity differs')
            require(type(table) is not str or (table.isascii() and table.isdecimal()
                    and str(int(table)) == table), 'numeric table observation required')
            if (type(table) is int and table == 2022) or (type(table) is str and table == '2022'):
                selected.append(row)
    return selected


VM_STAGES = ('manager', 'pid1', 'namespaces', 'cgroup-mount', 'pidfd', 'privileges',
    'tun-device', 'forwarding', 'tools', 'product-paths', 'environment', 'network', 'route-tables', 'gate')
VM_CHECKS = ('githubHosted', 'dedicatedVM', 'ordinaryGUI', 'rootManager', 'pid1Systemd',
    'sameSystemNamespaces', 'pidfd', 'noNewPrivsZero', 'tunDevice', 'forwardingBaselineValid',
    'forwardingStable', 'toolsRootOwned', 'freshProductPaths', 'defaultEnvironment', 'networkSnapshotComplete', 'materialsDownloaded')
VM_BLOCKS = frozenset(('githubHosted', 'dedicatedVM', 'pid1Systemd', 'rootManager', 'tunDevice', 'pidfd',
    'sameSystemNamespaces', 'toolsRootOwned', 'ordinaryGUI', 'VMPrivilegesOrForwardingBaseline',
    'freshProductAndDefaultSandbox', 'fixtureCIDRCollision', 'productTunRuleOrTableAlreadyOwned',
    'localEvidenceAndMaterials', 'c6CompleteCarrierAdmission', 'completeCarrierHashes',
    'realPkexecAuthenticationPendingConfirmation'))
VM_PROBES = ('pid1-hierarchy', 'pid1-unit', 'pid1-membership', 'pid1-stability', 'none', 'ip', 'systemctl', 'systemd-run', 'resolvectl', 'nft', 'python', 'setpriv', 'tcpdump', 'tshark',
    'links', 'addresses', 'route4', 'route6', 'rule4', 'rule6', 'resolved', 'firewall', 'resolv-conf', 'network-forwarding')
VM_ERRORS = ('none', 'OSError', 'ValueError', 'SubprocessError', 'TypeError', 'KeyError', 'other')


def valid_forwarding(value):
    return type(value) is list and len(value) == 2 and all(type(v) is int and v in (0, 1) for v in value)


def vm_observation():
    return {'schema': 'polaris-vm-preflight-observation-v2',
        'stages': {name: {'state': 'not-reached', 'error': 'none'} for name in VM_STAGES},
        'checks': {name: None for name in VM_CHECKS}, 'managerControlGroup': 'unknown',
        'gate': 'not-reached', 'blocked': [], 'failedProbe': 'none', 'forwarding': None}


def vm_error(error):
    for kind in (OSError, ValueError, subprocess.SubprocessError, TypeError, KeyError):
        if isinstance(error, kind): return kind.__name__
    return 'other'


def validate_vm_observation(value):
    require(type(value) is dict and set(value) == {'schema', 'stages', 'checks', 'managerControlGroup', 'gate', 'blocked', 'failedProbe', 'forwarding'},
            'fixed VM observation required')
    require(value['schema'] == 'polaris-vm-preflight-observation-v2' and
        type(value['stages']) is dict and set(value['stages']) == set(VM_STAGES) and
        type(value['checks']) is dict and set(value['checks']) == set(VM_CHECKS), 'VM schema differs')
    for row in value['stages'].values():
        require(type(row) is dict and set(row) == {'state', 'error'} and
            row['state'] in ('observed', 'failed', 'not-reached') and row['error'] in VM_ERRORS and
            ((row['state'] == 'failed') == (row['error'] != 'none')), 'VM stage differs')
    require(value['forwarding'] is None or valid_forwarding(value['forwarding']), 'bounded forwarding baseline required')
    require(value['checks']['forwardingBaselineValid'] is not True or valid_forwarding(value['forwarding']), 'valid forwarding claim requires observation')
    require(value['failedProbe'] in VM_PROBES, 'fixed VM probe required')
    require(all(v is None or type(v) is bool for v in value['checks'].values()) and
        value['managerControlGroup'] in ('unknown', 'empty-root', 'root-slash', 'nonroot', 'missing') and
        value['gate'] in ('not-reached', 'blocked', 'ready', 'failed') and
        type(value['blocked']) is list and all(type(v) is str and v in VM_BLOCKS for v in value['blocked']) and
        value['blocked'] == sorted(set(value['blocked'])), 'VM bounded observations differ')
    require((value['gate'] == 'blocked') == bool(value['blocked']) and
        (value['gate'] not in ('ready', 'blocked') or value['stages']['gate']['state'] == 'observed') and
        (value['gate'] != 'failed' or value['stages']['gate']['state'] == 'failed'), 'VM gate state differs')
    if value['gate'] == 'ready' or value['blocked'] == ['realPkexecAuthenticationPendingConfirmation']:
        require(all(v is True for v in value['checks'].values()) and
            all(row['state'] == 'observed' for row in value['stages'].values()) and
            value['failedProbe'] == 'none' and value['managerControlGroup'] in ('empty-root', 'root-slash'),
            'incomplete VM observation cannot claim ready or authentication-only pending')
    return value


def manager_root(ops, observation):
    # Keep the property name: missing/unknown property stdout is NOT an empty root.
    raw = ops.run([SYSTEMCTL, 'show', '--property=ControlGroup'])
    rows = raw.splitlines()
    if len(rows) != 1 or not rows[0].startswith('ControlGroup='):
        observation['managerControlGroup'] = 'missing'
        raise ValueError('explicit unique manager property required')
    value = rows[0][len('ControlGroup='):]
    observation['managerControlGroup'] = 'empty-root' if value == '' else 'root-slash' if value == '/' else 'nonroot'
    return os.geteuid() == 0 and value in ('', '/')


def pid1_root_cgroup(ops, progress=lambda _: None):
    # Unit no-record is not manager root. Independently anchor to PID1's kernel
    # membership; the caller must still require root mount and system namespaces.
    progress('pid1-hierarchy')
    rows = Path('/proc/1/cgroup').read_text().splitlines()
    require(len(rows) == 1 and rows[0] in ('0::/', '0::/init.scope'), 'PID1 hierarchy root unconfirmed')
    directory = Path('/sys/fs/cgroup')
    if rows[0] == '0::/init.scope':
        progress('pid1-unit')
        props = {}
        for key in ('LoadState', 'ActiveState', 'ControlGroup'):
            value = ops.run([SYSTEMCTL, '--system', 'show', '--all', '--property=' + key, 'init.scope'])
            lines = value.splitlines()
            require(len(lines) == 1 and lines[0].startswith(key + '='), 'PID1 explicit unit property required')
            props[key] = lines[0][len(key) + 1:]
        require(props['LoadState'] == 'loaded' and props['ActiveState'] == 'active' and
                props['ControlGroup'] in ('', '/init.scope'), 'PID1 scope identity unconfirmed')
        directory /= 'init.scope'
    progress('pid1-membership')
    require(directory.is_dir() and not directory.is_symlink() and
            not (directory / 'cgroup.procs').is_symlink(), 'PID1 directory unconfirmed')
    for _ in range(2):
        members = (directory / 'cgroup.procs').read_text().splitlines()
        require(all(re.fullmatch(r'[1-9][0-9]*', item) for item in members) and '1' in members,
                'PID1 kernel membership unconfirmed')
    progress('pid1-stability')
    require(Path('/proc/1/cgroup').read_text().splitlines() == rows, 'PID1 membership changed')
    return True


def collect_preflight(gui_uid, vm_context, materials_downloaded=False, ops=None):
    """Read-only target-VM observations; no sudo, device creation or auth probe."""
    ops = ops or Commands()
    facts = {'guiUid': gui_uid, 'githubHosted': vm_context.get('githubHosted') is True,
             'dedicatedVM': vm_context.get('dedicatedVM') is True,
             'materialsDownloaded': materials_downloaded is True, 'observationErrors': []}
    observation = vm_observation()
    facts['vmPreflight'] = observation
    stage = None; probe = 'none'
    def probing(name):
        nonlocal probe
        require(name in VM_PROBES, 'fixed VM probe required')
        probe = name
    def reached(name):
        nonlocal stage, probe
        probe = 'none'
        if stage is not None: observation['stages'][stage] = {'state': 'observed', 'error': 'none'}
        stage = name
    try:
        reached('manager')
        facts['rootManager'] = manager_root(ops, observation)
        reached('pid1')
        facts['pid1Systemd'] = (Path('/proc/1/comm').read_text().strip() == 'systemd' and
                               Path('/proc/1').stat().st_uid == 0 and pid1_root_cgroup(ops, probing))
        reached('namespaces')
        ns = {name: (Path('/proc/1/ns') / name).stat().st_ino for name in ('net', 'mnt', 'pid', 'cgroup')}
        facts['systemNamespaces'] = ns
        facts['sameSystemNamespaces'] = all((Path('/proc/self/ns') / n).stat().st_ino == i for n, i in ns.items())
        reached('cgroup-mount')
        mounts = Path('/proc/self/mountinfo').read_text().splitlines()
        cg = [r.split() for r in mounts if ' - cgroup2 ' in r]
        require(len(cg) == 1 and cg[0][3:5] == ['/', '/sys/fs/cgroup'] and
                not any(r.split()[4].startswith('/sys/fs/cgroup/') for r in mounts), 'unmasked root cgroup2 required')
        reached('pidfd')
        facts['pidfd'] = hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal')
        reached('privileges')
        status = dict(r.split(':', 1) for r in Path('/proc/self/status').read_text().splitlines() if ':' in r)
        facts['noNewPrivs'] = int(status['NoNewPrivs'].strip())
        reached('tun-device')
        info = Path('/dev/net/tun').stat()
        facts['tunDevice'] = stat.S_ISCHR(info.st_mode) and (os.major(info.st_rdev), os.minor(info.st_rdev)) == (10, 200)
        reached('forwarding')
        facts['forwarding'] = [int(Path(path).read_text()) for path in
            ('/proc/sys/net/ipv4/ip_forward', '/proc/sys/net/ipv6/conf/all/forwarding')]
        reached('tools')
        tools_ok = True
        for name, path in (('ip', IP), ('systemctl', SYSTEMCTL), ('systemd-run', '/usr/bin/systemd-run'),
                ('resolvectl', '/usr/bin/resolvectl'), ('nft', '/usr/sbin/nft'), ('python', '/usr/bin/python3'),
                ('setpriv', '/usr/bin/setpriv'), ('tcpdump', '/usr/bin/tcpdump'), ('tshark', '/usr/bin/tshark')):
            probing(name)
            if not root_path(Path(path).resolve()).is_file():
                tools_ok = False; observation['failedProbe'] = name
                break
        facts['toolsRootOwned'] = tools_ok
        reached('product-paths')
        facts['existingProductPaths'] = [path for path in ('/usr/local/lib/polaris', str(UNIT), '/run/polaris', '/var/lib/polaris')
                                         if Path(path).exists() or Path(path).is_symlink()]
        reached('environment')
        forbidden = ('WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS', 'WEBKIT_FORCE_SANDBOX',
                     'POLARIS_SINGBOX_PATH', 'LD_PRELOAD', 'LD_LIBRARY_PATH')
        facts['unsafeEnvironment'] = [key for key in forbidden if key in os.environ]
        reached('network')
        facts['network'] = network_snapshot(ops, progress=probing)
        facts['forwardingStable'] = valid_forwarding(facts['forwarding']) and facts['network'].get('forwarding') == facts['forwarding'] and valid_forwarding(facts['network'].get('forwarding'))
        facts['routes'] = facts['network']['route4'] + facts['network']['route6']
        reached('route-tables')
        facts['tunTableRoutes'] = tun_table_routes(facts['network'])
        facts['networkSnapshotComplete'] = True
        observation['stages'][stage] = {'state': 'observed', 'error': 'none'}
    except Exception as error:
        observation['failedProbe'] = probe
        observation['stages'][stage] = {'state': 'failed', 'error': vm_error(error)}
        facts['observationErrors'].append(type(error).__name__)
        facts['networkSnapshotComplete'] = False
    for name in VM_CHECKS:
        if name in facts and type(facts[name]) is bool: observation['checks'][name] = facts[name]
    observation['checks']['ordinaryGUI'] = type(gui_uid) is int and gui_uid > 0
    for check, field, wanted in (('noNewPrivsZero', 'noNewPrivs', 0),
            ('freshProductPaths', 'existingProductPaths', []), ('defaultEnvironment', 'unsafeEnvironment', [])):
        if field in facts: observation['checks'][check] = facts[field] == wanted
    if 'forwarding' in facts:
        observation['checks']['forwardingBaselineValid'] = valid_forwarding(facts['forwarding'])
        if valid_forwarding(facts['forwarding']): observation['forwarding'] = list(facts['forwarding'])
    validate_vm_observation(observation)
    return facts


def executable_identity(pid):
    before = process_identity(pid)
    require(before is not None, 'live executable required')
    fd = os.open('/proc/' + str(pid) + '/exe', os.O_RDONLY | os.O_CLOEXEC)
    try:
        info = os.fstat(fd); h = hashlib.sha256()
        require(info.st_size <= 128 * 1024 * 1024, 'bounded executable required')
        while block := os.read(fd, 1024 * 1024): h.update(block)
        require(same_process(before, process_identity(pid)), 'executable process birth changed')
        return {'dev': info.st_dev, 'inode': info.st_ino, 'sha256': h.hexdigest()}
    finally: os.close(fd)


TRANSACTION_IDENTITY_PHASES = ('identity-before', 'executable-open', 'executable-initial',
    'inputs-before', 'executable-read', 'inputs-after', 'identity-after', 'snapshot-consistency')


TRANSACTION_PROBES = ('unknown', 'initial-identity-read', 'initial-live-state', 'initial-directory-absent', 'exe-open',
    'exe-shape', 'exe-match', 'status-read', 'uid-field', 'cmdline-read', 'cmdline-empty',
    'cmdline-shape', 'cmdline-decode', 'exe-read', 'exe-size', 'final-identity-read',
    'final-live-state', 'final-directory-absent', 'consistency-birth', 'consistency-uid', 'consistency-image',
    'consistency-topology', 'consistency-namespace', 'consistency-argv', 'consistency-file',
    'inventory-topology', 'parent-identity', 'consistency-identity', 'prior-identity-conflict', 'terminal-state')


class TransactionIdentityError(ValueError):
    def __init__(self, phase, error, probe='unknown', identities=(), uids=()):
        super().__init__('transaction candidate observation unconfirmed')
        self.phase = phase; self.probe = probe
        # Internal evidence only; never serialize status, argv, or these objects.
        self.identities = tuple(identities); self.uids = tuple(uids)
        self.error = ('enoent' if isinstance(error, OSError) and error.errno == 2 else
                      'eacces' if isinstance(error, OSError) and error.errno == 13 else
                      'eperm' if isinstance(error, OSError) and error.errno == 1 else
                      'esrch' if isinstance(error, OSError) and error.errno == 3 else
                      'other-os' if isinstance(error, OSError) else
                      'decode' if isinstance(error, UnicodeError) else 'contract')


def transaction_identity(pid):
    """Bounded held-executable observation, with fixed non-secret failure probes."""
    phase = 'identity-before'; probe = 'initial-identity-read'; identities = []; uid_reads = []
    try:
        before = process_identity(pid)
        if before is not None: identities.append(before)
        probe = 'initial-live-state'
        if before is None:
            try: (Path('/proc') / str(pid)).lstat()
            except FileNotFoundError:
                probe = 'initial-directory-absent'
        require(before is not None and before['state'] != 'Z', 'live transaction candidate required')
        phase = 'executable-open'; probe = 'exe-open'
        root = Path('/proc') / str(pid)
        fd = os.open(root / 'exe', os.O_RDONLY | os.O_CLOEXEC)
        try:
            phase = 'executable-initial'; probe = 'exe-shape'
            info = os.fstat(fd)
            require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= 128 * 1024 * 1024, 'transaction executable shape changed')
            probe = 'exe-match'
            require((info.st_dev, info.st_ino) == (before['exeDev'], before['exeInode']), 'transaction executable changed')
            def inputs():
                nonlocal probe
                probe = 'status-read'; status = (root / 'status').read_text()
                probe = 'uid-field'
                uids = tuple(int(v) for v in next(x for x in status.splitlines() if x.startswith('Uid:')).split()[1:])
                require(len(uids) == 4, 'transaction UID field unavailable')
                uid_reads.append(uids)
                probe = 'cmdline-read'; raw = (root / 'cmdline').read_bytes()
                probe = 'cmdline-empty'; require(bool(raw), 'transaction inputs unavailable')
                probe = 'cmdline-shape'
                require(len(raw) <= 1024 * 1024 and raw.endswith(b'\0'), 'transaction inputs malformed')
                probe = 'cmdline-decode'
                return uids, [x.decode('utf-8', 'strict') for x in raw[:-1].split(b'\0')]
            phase = 'inputs-before'
            uids, argv = inputs(); digest = hashlib.sha256()
            phase = 'executable-read'; probe = 'exe-read'
            remaining = info.st_size
            while remaining:
                block = os.read(fd, min(1024 * 1024, remaining))
                probe = 'exe-size'; require(block, 'transaction executable shortened')
                remaining -= len(block); digest.update(block); probe = 'exe-read'
            probe = 'exe-size'; require(not os.read(fd, 1), 'transaction executable grew')
            phase = 'inputs-after'; second_inputs = inputs()
            phase = 'identity-after'; probe = 'final-identity-read'
            after = process_identity(pid)
            if after is not None: identities.append(after)
            probe = 'final-live-state'
            if after is None:
                try: root.lstat()
                except FileNotFoundError:
                    probe = 'final-directory-absent'
            require(after is not None and after['state'] != 'Z', 'final candidate unavailable')
            last = os.fstat(fd); phase = 'snapshot-consistency'
            probe = 'consistency-birth'; require((before['pid'],before['startTime']) == (after['pid'],after['startTime']), 'candidate birth changed')
            probe = 'consistency-uid'; require(before['uid'] == after['uid'] == uids[0], 'candidate UID changed')
            probe = 'consistency-image'; require((before['exeDev'],before['exeInode'],before['exe']) == (after['exeDev'],after['exeInode'],after['exe']), 'candidate image changed')
            probe = 'consistency-topology'; require(before['ppid'] == after['ppid'], 'candidate parent changed')
            probe = 'consistency-namespace'; require(before['namespaces'] == after['namespaces'] and before['cgroup'] == after['cgroup'], 'candidate namespace changed')
            probe = 'consistency-argv'; require(second_inputs == (uids,argv), 'candidate inputs changed')
            probe = 'consistency-identity'; require(same_process(before, after), 'complete identity required')
            probe = 'consistency-file'
            require((info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns) ==
                    (last.st_dev, last.st_ino, last.st_size, last.st_mtime_ns, last.st_ctime_ns), 'transaction executable changed')
            return {'identity': before, 'uids': uids, 'argv': argv,
                    'executable': {'dev': info.st_dev, 'inode': info.st_ino, 'sha256': digest.hexdigest()}}
        finally:
            os.close(fd)
    except (OSError, ValueError, StopIteration, KeyError, IndexError) as error:
        raise TransactionIdentityError(phase, error, probe, identities, uid_reads) from None


TERMINAL_PROBES = ('terminal-live', 'terminal-birth', 'terminal-parent', 'terminal-format',
                   'terminal-transition', 'terminal-reappeared')

class TerminalIdentityError(ValueError):
    def __init__(self, probe, message):
        super().__init__(message)
        self.probe = probe


def terminal_identity(old):
    """Only a caller-held strong birth may use this sampled terminal proof.

    A missing exe/status file is never process absence. This grants no signal,
    executable identity, descendant closure, or transaction completion.
    """
    require(type(old) is dict and type(old.get('pid')) is int and old['pid'] > 1 and
            type(old.get('startTime')) is str and old['startTime'].isdigit(), 'held birth required')
    directory = Path('/proc') / str(old['pid'])
    observed = []
    for _ in range(2):
        try:
            directory.lstat()
        except FileNotFoundError:
            observed.append(('absent', None)); continue
        raw = (directory / 'stat').read_text()
        try:
            fields = raw[raw.rindex(')') + 2:].split()
            if len(fields) < 20 or not fields[19].isdigit() or not fields[1].isdigit():
                raise ValueError('malformed stat')
        except (ValueError, IndexError):
            raise TerminalIdentityError('terminal-format', 'terminal stat malformed') from None
        if fields[19] != old['startTime']:
            raise TerminalIdentityError('terminal-birth', 'held process not same-birth terminal')
        if int(fields[1]) != old['ppid']:
            raise TerminalIdentityError('terminal-parent', 'held process not same-birth terminal')
        if fields[0] != 'Z':
            raise TerminalIdentityError('terminal-live', 'held process not same-birth terminal')
        observed.append(('zombie', (fields[19], int(fields[1]))))
    if observed[0][0] == 'zombie' and observed[1][0] == 'absent':
        # A held same-birth/parent zombie can only disappear after reaping.
        # Confirm whole-directory absence again; never infer it from an exe or
        # stat read failure, and never accept the reverse or a live observation.
        try: directory.lstat()
        except FileNotFoundError: pass
        else: raise TerminalIdentityError('terminal-reappeared', 'terminal directory reappeared')
        state = 'absent'
    else:
        if observed[0] != observed[1]:
            raise TerminalIdentityError('terminal-transition', 'terminal observation changed')
        state = observed[0][0]
    return {'pid': old['pid'], 'startTime': old['startTime'], 'state': state,
            'scope': 'sampled-terminal-not-execution-identity'}


def unverified_terminal_identity(inventory):
    """Sample terminal birth only. Does NOT establish executable/UID or signal authority."""
    proof = terminal_identity(inventory)
    return {**proof, 'scope': 'sampled-terminal-unverified-execution'}


def service_properties(ops):
    return dict(line.split('=', 1) for line in ops.run([SYSTEMCTL, 'show', SERVICE,
        '--property=MainPID,ActiveState,FragmentPath,DropInPaths,User,ControlGroup,KillMode,SendSIGKILL,FinalKillSignal,Delegate']).splitlines() if '=' in line)


def validate_service(props):
    require(props.get('FragmentPath') == str(UNIT) and not props.get('DropInPaths') and
            props.get('User') in ('', 'root', '0') and props.get('KillMode') == 'control-group' and
            props.get('SendSIGKILL') == 'yes' and props.get('FinalKillSignal') in ('9', 'SIGKILL') and
            props.get('Delegate') == 'no' and props.get('ControlGroup') in ('', '/system.slice/' + SERVICE),
            'original production service required')


def service_closed(ops):
    props = service_properties(ops)
    require(props.get('MainPID') == '0' and props.get('ActiveState') in ('inactive', 'failed') and
            ops.cgroup_closed(), 'helper service descendants not proved closed')
    return True


def stable_network(value, context=()):
    # Preserve raw snapshots. Compare all structure except DHCP lifetime timers
    # and nft counter packets/bytes (only within firewall.*.counter objects).
    if isinstance(value, list): return [stable_network(v, context) for v in value]
    if isinstance(value, dict):
        ignored = {'valid_life_time', 'preferred_life_time'}
        if context and context[0] == 'firewall' and context[-1] == 'counter':
            ignored |= {'packets', 'bytes'}
        return {k: stable_network(v, (*context, k)) for k, v in value.items() if k not in ignored}
    return value


def preflight(facts, carrier=None, authentication_confirmed=False):
    """Pure gate. Carrier admission is supplied by the original verified consumer.

This is not a second transport/formal verifier and cannot mint release receipts.
"""
    blocked = []
    for field, wanted in [('githubHosted', True), ('dedicatedVM', True), ('pid1Systemd', True),
                          ('rootManager', True), ('tunDevice', True), ('pidfd', True),
                          ('sameSystemNamespaces', True), ('toolsRootOwned', True)]:
        if facts.get(field) is not wanted:
            blocked.append(field)
    if type(facts.get('guiUid')) is not int or facts['guiUid'] <= 0:
        blocked.append('ordinaryGUI')
    if facts.get('noNewPrivs') != 0 or not valid_forwarding(facts.get('forwarding')) or facts.get('forwardingStable') is not True:
        blocked.append('VMPrivilegesOrForwardingBaseline')
    if facts.get('existingProductPaths') or facts.get('unsafeEnvironment'):
        blocked.append('freshProductAndDefaultSandbox')
    reserved = [ipaddress.ip_network(n) for n in topology('000000000000')['cidrs']]
    for route in facts.get('routes', []):
        destination = route.get('dst', 'default')
        if destination in ('default', '0.0.0.0/0', '::/0'):
            continue
        network = ipaddress.ip_network(destination, strict=False)
        if any(network.version == item.version and network.overlaps(item) for item in reserved):
            blocked.append('fixtureCIDRCollision')
    rules = facts.get('network', {}).get('rule4', []) + facts.get('network', {}).get('rule6', [])
    if (facts.get('tunTableRoutes') or any(9000 <= int(r.get('priority', -1)) <= 9010 for r in rules)
            or any(str(r.get('table')) == '2022' for r in facts.get('routes', []))
            or any(r.get('ifname') == 'polaris-tun0' for r in facts.get('network', {}).get('links', []))):
        blocked.append('productTunRuleOrTableAlreadyOwned')
    if not facts.get('networkSnapshotComplete') or not facts.get('materialsDownloaded'):
        blocked.append('localEvidenceAndMaterials')
    if not carrier or carrier.get('kernelCommit') != KERNEL_COMMIT or carrier.get('originalAdmission') != 'verified':
        blocked.append('c6CompleteCarrierAdmission')
    elif not all(re.fullmatch(r'[0-9a-f]{64}', str(carrier.get(k, ''))) for k in ('carrierSha256', 'helperSha256', 'coreSha256')):
        blocked.append('completeCarrierHashes')
    if not authentication_confirmed:
        blocked.append('realPkexecAuthenticationPendingConfirmation')
    return {'schema': SCHEMA, 'status': 'BLOCKED' if blocked else 'READY_FOR_LOCAL_PREPARATION',
            'blocked': sorted(set(blocked)), 'publicationEligible': False,
            'scope': 'support preflight only; not GUI/TUN PASS',
            'controlConnectionMayBeInterrupted': True}


class Ledger:
    def __init__(self, run_id, ops=None):
        self.run_id = token(run_id)
        self.directory = BASE / self.run_id
        self.path = self.directory / 'ledger.json'
        self.ops = ops or Commands()
        self.children = {}  # native handles owned only by the live root controller

    def load(self):
        require(os.geteuid() == 0, 'root ledger only')
        path = root_path(self.path)
        require(path.stat().st_mode & 0o077 == 0 and path.stat().st_size < 4 * 1024 * 1024, 'private bounded ledger required')
        value = json.loads(path.read_bytes())
        require(value.get('schema') == SCHEMA and value.get('runId') == self.run_id, 'ledger binding differs')
        return value

    def save(self, value):
        require(os.geteuid() == 0 and value.get('runId') == self.run_id, 'root ledger binding required')
        root_path(self.directory, directory=True)
        temp = self.directory / ('ledger-' + os.urandom(8).hex() + '.tmp')
        fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(fd, 'wb') as stream:
                stream.write(json_bytes(value)); stream.flush(); os.fsync(stream.fileno())
            os.replace(temp, self.path)
            fd = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY)
            try: os.fsync(fd)
            finally: os.close(fd)
        finally:
            if temp.exists(): temp.unlink()

    def mutate(self, change):
        # The root controller is the sole writer before deadline. Watchdog runs
        # after deadline; controller MUST stop writes before its reserved margin.
        import fcntl
        lock_path = self.directory / 'ledger.lock'
        root_path(lock_path)
        with lock_path.open('rb') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            value = self.load()
            change(value)
            self.save(value)
            return value


def prepare_root(run_id, gui_uid, baseline, helper_sha256, core_sha256, seconds=420, ops=None):
    """Explicit privileged integration call; NEVER called by module import/tests.

No password/account operation exists here. Normal integration must obtain the
separate authentication confirmation before calling product helper_install.
"""
    require(os.geteuid() == 0 and type(gui_uid) is int and gui_uid > 0, 'root preparation, ordinary GUI required')
    require(type(seconds) is int and 60 <= seconds <= 600, 'bounded watchdog deadline required')
    require(all(re.fullmatch(r'[0-9a-f]{64}', v) for v in (helper_sha256, core_sha256)), 'frozen helper/core hashes required')
    ledger = Ledger(run_id, ops)
    if not BASE.exists(): BASE.mkdir(mode=0o700)
    root_path(BASE, directory=True)
    require(BASE.stat().st_mode & 0o077 == 0, 'root-private base required')
    ledger.directory.mkdir(mode=0o700)  # no reuse of another invocation
    (ledger.directory / 'ledger.lock').touch(mode=0o600, exist_ok=False)
    source = Path(__file__).read_bytes()
    frozen = ledger.directory / 'tun.py'
    with frozen.open('xb') as stream: stream.write(source)
    frozen.chmod(0o600)
    value = {'schema': SCHEMA, 'runId': run_id, 'guiUid': gui_uid, 'phase': 'prepared',
             'result': 'incomplete', 'forcedCleanup': False, 'unknown': [], 'processes': [],
             'namespaces': [], 'links': [], 'routes': [], 'intents': [], 'helper': None,
             'baseline': baseline, 'helperSha256': helper_sha256, 'coreSha256': core_sha256,
             'watchdogScriptSha256': digest(source), 'deadlineMonotonic': time.monotonic() + seconds,
             'controlConnectionMayBeInterrupted': True, 'publicationEligible': False}
    ledger.save(value)
    timer = 'polaris-tun-watchdog-' + run_id
    ledger.ops.run(['/usr/bin/systemd-run', '--unit=' + timer, '--on-active=' + str(seconds) + 's',
                    '--timer-property=AccuracySec=1s', '--property=Type=oneshot',
                    '--property=TimeoutStartSec=90s', '/usr/bin/env', '-i', 'PATH=' + ENV['PATH'],
                    'LANG=C', '/usr/bin/python3', '-I', str(frozen), 'watchdog', '--run-id', run_id])
    active = ledger.ops.run([SYSTEMCTL, 'show', timer + '.timer', '--property=ActiveState', '--value']).strip()
    require(active == 'active', 'independent watchdog timer not active')
    ledger.mutate(lambda v: v.update(watchdog={'timer': timer + '.timer', 'active': True}, phase='guarded'))
    return ledger


def _register_product(ledger, pid, role):
    require(role in ('helper', 'core'), 'only service-bound product roles')
    identity = ledger.ops.proc(pid)
    require(identity and identity['state'] != 'Z', 'live process required')
    actual = executable_identity(pid)
    def add(value):
        require(value['phase'] in ('guarded', 'network-ready', 'running'), 'registration phase closed')
        require(identity['cgroup'] == '0::/system.slice/' + SERVICE, 'product service cgroup differs')
        require(identity['uid'] == (0 if role == 'helper' else value['guiUid']), 'product UID differs')
        require(identity['namespaces'] == value['baseline']['systemNamespaces'], 'product escaped VM system namespaces')
        require(actual['sha256'] == value[role + 'Sha256'], 'actual product executable differs from carrier')
        if role == 'core':
            # Production AmbientCapsSpawner: NET_BIND_SERVICE(10), NET_ADMIN(12), NET_RAW(13).
            require(identity.get('capAmb') == 0x3400 and identity.get('capEff') == 0x3400, 'actual core capability set differs')
            require(value.get('helper') and identity['ppid'] == value['helper']['identity']['pid'] and
                    same_process(value['helper']['identity'], ledger.ops.proc(identity['ppid'])), 'core lacks owned helper parent')
        require(not any(same_process(identity, p['identity']) for p in value['processes']), 'duplicate process registration')
        value['processes'].append({'role': role, 'identity': identity, 'executable': actual})
    ledger.mutate(add)
    return identity


def register_helper(ledger):
    value = ledger.load()
    actual = file_identity(HELPER)
    require(actual['sha256'] == value['helperSha256'], 'installed helper differs from frozen carrier')
    unit = file_identity(UNIT)
    props = service_properties(ledger.ops); validate_service(props)
    pid = int(props['MainPID'])
    require(executable_identity(pid) == actual, 'MainPID is not the exact installed helper inode/hash')
    identity = _register_product(ledger, pid, 'helper')
    ledger.mutate(lambda v: v.update(helper={'identity': identity, 'binary': actual, 'unit': unit}))
    return identity


def register_core(ledger, pid):
    """PID from actual App/helper observation; all root service/hash checks apply."""
    return _register_product(ledger, pid, 'core')


def register_spawned_fixture(ledger, child, expected_argv, namespace_role):
    """Only the root controller's actual Popen child; never a GUI supplied PID."""
    require(isinstance(child, subprocess.Popen) and namespace_role in ('A', 'B', 'O'), 'actual owned fixture child required')
    require(child.args == expected_argv, 'spawn argv differs')
    value = ledger.load(); plan = topology(ledger.run_id)
    ns = next(row for row in value['namespaces'] if row['name'] == plan['namespaces'][namespace_role])
    identity = ledger.ops.proc(child.pid)
    require(identity and identity['ppid'] == os.getpid() and identity['uid'] == value['guiUid'] and
            identity['namespaces']['net'] == ns['inode'], 'owned fixture birth/UID/namespace differs')
    require(identity['exe'] == str(Path('/usr/bin/python3').resolve()), 'fixture must be ordinary Python')
    # This API does not spawn. Caller must freeze the reviewed fixture code and
    # use ip netns exec + setpriv; registration verifies the actual final argv.
    command = (Path('/proc') / str(child.pid) / 'cmdline').read_bytes().split(b'\0')
    require(len(expected_argv) >= 8, 'fixture launch argv incomplete')
    script = Path(expected_argv[-7])
    expected_tail = [b'-I', str(script).encode(), b'--run-id', ledger.run_id.encode(),
                     b'--role', namespace_role.encode(), b'--namespace-inode', str(ns['inode']).encode()]
    require(command[1:-1] == expected_tail, 'fixture final command differs')
    require(script == FIXTURE_BASE / ledger.run_id / 'tun_fixtures.py' and
            file_identity(script)['sha256'] == value.get('fixtureCode', {}).get('tun_fixtures.py'), 'frozen fixture source differs')
    ledger.mutate(lambda v: v['processes'].append({'role': 'fixture', 'identity': identity}))
    return identity


def freeze_fixture_code(ledger, source_dir, hashes):
    """Freeze only reviewed three-file fixture code; no secret/GUI data copied."""
    require(set(hashes) == {'tun.py', 'tun_fixtures.py', 'fixtures.py'}, 'exact fixture code inventory required')
    if not FIXTURE_BASE.exists(): FIXTURE_BASE.mkdir(mode=0o755)
    root_path(FIXTURE_BASE, directory=True)
    directory = FIXTURE_BASE / ledger.run_id
    directory.mkdir(mode=0o755)
    for name, expected in hashes.items():
        require(re.fullmatch(r'[0-9a-f]{64}', expected), 'fixture hash required')
        source = Path(source_dir) / name
        fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            require(os.fstat(fd).st_size < 1024 * 1024, 'fixture source bound')
            with os.fdopen(fd, 'rb') as stream: content = stream.read()
        except BaseException:
            # fdopen owns fd after it succeeds.
            raise
        require(digest(content) == expected, 'reviewed fixture source hash differs')
        target = directory / name
        with target.open('xb') as stream: stream.write(content)
        target.chmod(0o644)
    ledger.mutate(lambda v: v.update(fixtureCode=hashes.copy()))
    return directory


def _spawn_owned(ledger, argv, **pipes):
    child = subprocess.Popen(argv, env=ENV, **pipes)
    fd = None
    # Direct-child pidfd acquisition cannot be delegated to a GUI-supplied PID.
    try:
        fd = os.pidfd_open(child.pid, 0)
        identity = ledger.ops.proc(child.pid)
        require(identity and identity['ppid'] == os.getpid(), 'new native child birth missing')
        ledger.children[child.pid] = child
        ledger.mutate(lambda v: v.setdefault('spawning', []).append({'pid': child.pid,
            'startTime': identity['startTime'], 'controllerPid': os.getpid(), 'state': 'pending'}))
        return child, fd
    except BaseException:
        try:
            child.kill(); child.wait(timeout=5)  # direct Popen handle, never arbitrary PID
        finally:
            if fd is not None: os.close(fd)
            # If even this write fails, the outer controller must keep the exception terminal.
            ledger.mutate(lambda v: v.update(result='failed', forcedCleanup=True))
        raise


def _abort_spawn(ledger, child, fd):
    try:
        # pidfd pinned at direct spawn, so no exec transition/PID reuse race.
        signal.pidfd_send_signal(fd, signal.SIGKILL)
    except ProcessLookupError:
        pass
    finally:
        try: child.wait(timeout=5)
        finally:
            ledger.mutate(lambda v: (v.update(result='failed', forcedCleanup=True),
                [r.update(state='aborted') for r in v.get('spawning', []) if r['pid'] == child.pid]))


def _commit_spawn(ledger, child):
    ledger.mutate(lambda v: [r.update(state='registered') for r in v.get('spawning', []) if r['pid'] == child.pid])


def spawn_fixture(ledger, role):
    require(role in ('A', 'B', 'O'), 'bounded backend role')
    value = ledger.load(); plan = topology(ledger.run_id)
    require(value['phase'] == 'network-ready' and value.get('fixtureCode'), 'prepared network/code required')
    ns = next(n for n in value['namespaces'] if n['name'] == plan['namespaces'][role])
    user = pwd.getpwuid(value['guiUid'])
    directory = FIXTURE_BASE / ledger.run_id
    for name, expected in value['fixtureCode'].items():
        require(file_identity(directory / name)['sha256'] == expected, 'frozen fixture code changed')
    argv = [IP, 'netns', 'exec', ns['name'], '/usr/bin/setpriv', '--reuid=' + str(user.pw_uid),
            '--regid=' + str(user.pw_gid), '--clear-groups', '--bounding-set=-all', '--inh-caps=-all',
            '--ambient-caps=-all', '--no-new-privs', '/usr/bin/python3', '-I', str(directory / 'tun_fixtures.py'),
            '--run-id', ledger.run_id, '--role', role, '--namespace-inode', str(ns['inode'])]
    child, fd = _spawn_owned(ledger, argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        require(select.select([child.stdout], [], [], 10)[0], 'fixture readiness timed out')
        row = json.loads(child.stdout.readline(4097))
        require(row == {'ready': True, 'role': role}, 'fixture readiness differs')
        register_spawned_fixture(ledger, child, argv, role)
        _commit_spawn(ledger, child)
        return child
    except BaseException:
        _abort_spawn(ledger, child, fd); raise
    finally: os.close(fd)


def register_tun_interface(ledger):
    value = ledger.load()
    require(any(p['role'] == 'core' and same_process(p['identity'], ledger.ops.proc(p['identity']['pid']))
                for p in value['processes']), 'live registered core required')
    rows = ledger.ops.json([IP, '-j', '-details', 'link', 'show', 'dev', 'polaris-tun0'])
    require(len(rows) == 1 and rows[0].get('linkinfo', {}).get('info_kind') == 'tun', 'actual TUN link required')
    link = rows[0]
    rules = {family: ledger.ops.json([IP, '-j', '-' + family, 'rule', 'show']) for family in ('4', '6')}
    owned = {family: [r for r in rows if 9000 <= int(r.get('priority', -1)) <= 9010]
             for family, rows in rules.items()}
    require(owned['4'], 'actual product TUN policy rules missing')
    ledger.mutate(lambda v: v.update(tunInterface={'name': 'polaris-tun0', 'ifindex': link['ifindex'],
        'address': link.get('address')}, tunRules=owned))
    return ledger.load()['tunInterface']


def recover_tun(ledger, value):
    """Emergency-only exact owned policy cleanup after all product processes close."""
    require(ledger.ops.cgroup_closed(), 'product cgroup must be closed before network recovery')
    expected = value.get('tunInterface')
    links = ledger.ops.json([IP, '-j', 'link', 'show'])
    live = [r for r in links if r.get('ifname') == 'polaris-tun0']
    if live:
        require(expected and len(live) == 1 and all(live[0].get(k) == expected[k] for k in ('ifindex', 'address')),
                'unregistered or changed TUN interface')
        ledger.ops.run(['/usr/bin/resolvectl', 'revert', 'polaris-tun0'])
        ledger.ops.run([IP, 'link', 'del', 'dev', 'polaris-tun0'])
    for family in ('4', '6'):
        recorded = value.get('tunRules', {}).get(family, [])
        current = ledger.ops.json([IP, '-j', '-' + family, 'rule', 'show'])
        current = [r for r in current if 9000 <= int(r.get('priority', -1)) <= 9010]
        # A subset can already have been removed by normal core shutdown.
        available = list(recorded)
        for row in current:
            require(row in available, 'unknown policy rule in product range')
            available.remove(row)
        for priority in sorted({r['priority'] for r in current}, reverse=True):
            expected_group = [r for r in current if r['priority'] == priority]
            while expected_group:
                live_group = [r for r in ledger.ops.json([IP, '-j', '-' + family, 'rule', 'show']) if r.get('priority') == priority]
                require(sorted(map(json_bytes, live_group)) == sorted(map(json_bytes, expected_group)), 'policy rule changed during recovery')
                ledger.ops.run([IP, '-' + family, 'rule', 'del', 'priority', str(priority)])
                remaining = [r for r in ledger.ops.json([IP, '-j', '-' + family, 'rule', 'show']) if r.get('priority') == priority]
                require(len(remaining) == len(expected_group) - 1 and all(r in expected_group for r in remaining), 'policy delete outcome unknown')
                expected_group = remaining
    routes = {'route' + family: ledger.ops.json([IP, '-j', '-N', '-' + family,
              'route', 'show', 'table', 'all']) for family in ('4', '6')}
    require(not tun_table_routes(routes), 'remaining product route table requires VM disposal')


def start_capture(ledger):
    value = ledger.load()
    require(value['phase'] == 'network-ready' and value.get('helper') and value.get('tunInterface'), 'owned product/TUN required before capture')
    capture = ledger.directory / 'tun.pcap'
    require(not capture.exists(), 'fresh capture required')
    tool = Path('/usr/bin/tcpdump').resolve(); root_path(tool)
    argv = [str(tool), '-i', 'polaris-tun0', '-nn', '-s', '2048', '-c', '4096', '-U', '-Z', 'root', '-w', str(capture),
            'ip and (host 198.18.240.10 or (host 8.8.8.8 and port 53))']
    child, fd = _spawn_owned(ledger, argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        until = time.monotonic() + 5; identity = None
        while time.monotonic() < until and child.poll() is None:
            current = ledger.ops.proc(child.pid)
            if current and current['exe'] == str(tool) and capture.exists() and capture.stat().st_size >= 24:
                # tcpdump has opened the device and emitted a pcap header before clients begin.
                with capture.open('rb') as stream: magic = stream.read(4)
                require(magic in (b'\xd4\xc3\xb2\xa1', b'\xa1\xb2\xc3\xd4', b'\x4d\x3c\xb2\xa1', b'\xa1\xb2\x3c\x4d'), 'capture header differs')
                identity = current; break
            time.sleep(0.02)
        require(identity and identity['ppid'] == os.getpid() and identity['uid'] == 0 and
                identity['namespaces'] == value['baseline']['systemNamespaces'], 'owned capture birth differs')
        require(executable_identity(child.pid) == file_identity(tool), 'actual capture executable differs')
        ledger.mutate(lambda v: v['processes'].append({'role': 'capture', 'identity': identity, 'argv': argv}))
        _commit_spawn(ledger, child)
        return child
    except BaseException:
        _abort_spawn(ledger, child, fd); raise
    finally: os.close(fd)


def stop_capture(ledger):
    value = ledger.load()
    rows = [p for p in value['processes'] if p['role'] == 'capture']
    require(len(rows) == 1, 'one owned capture required')
    identity = rows[0]['identity']
    require(same_process(identity, ledger.ops.proc(identity['pid'])), 'capture exited unexpectedly')
    require(ledger.ops.signal_owned(identity, signal.SIGINT), 'capture did not close normally')
    child = ledger.children.get(identity['pid'])
    require(child is not None and child.wait(timeout=5) == 0, 'capture native reap/exit missing')
    proof = file_identity(ledger.directory / 'tun.pcap')
    ledger.mutate(lambda v: v.update(capture=proof))
    return proof


def confirm_product_uninstalled(ledger):
    """Called after original GUI helper_uninstall, never substitutes for it."""
    value = ledger.load()
    require(all(not p.exists() and not p.is_symlink() for p in
                (UNIT, Path('/usr/local/lib/polaris'), Path('/run/polaris'))), 'product uninstall incomplete')
    require(ledger.ops.cgroup_closed(), 'product cgroup not closed')
    require(not any(ledger.ops.proc(p['identity']['pid']) for p in value['processes'] if p['role'] in ('helper', 'core')),
            'registered product not reaped')
    ledger.mutate(lambda v: v.update(productUninstalled=True))


def cancel_watchdog(ledger):
    value = ledger.load()
    require(value['phase'] == 'closed' and not value['forcedCleanup'] and not value['unknown'] and
            value.get('networkRestored') is True and value.get('productUninstalled') is True, 'normal closure required')
    name = 'polaris-tun-watchdog-' + ledger.run_id
    ledger.ops.run([SYSTEMCTL, 'stop', name + '.timer'])
    require(ledger.ops.run([SYSTEMCTL, 'show', name + '.timer', '--property=ActiveState', '--value']).strip() == 'inactive',
            'watchdog cancellation not confirmed')
    # A timer already fired is failure, even if its service is currently idle.
    require(time.monotonic() < value['deadlineMonotonic'] and
            ledger.ops.run([SYSTEMCTL, 'show', name + '.service', '--property=ActiveState', '--value']).strip() in ('inactive', 'dead'),
            'watchdog execution raced normal finish')
    ledger.mutate(lambda v: v.update(watchdogCancelled=True))


def provision_network(ledger):
    """Only fixed fixture objects; journal intent BEFORE every mutating command."""
    t = topology(ledger.run_id)
    require(ledger.load()['phase'] == 'guarded', 'watchdog must precede network setup')
    def action(argv):
        require(time.monotonic() + 30 < ledger.load()['deadlineMonotonic'], 'watchdog margin exhausted')
        row = {'argv': argv, 'state': 'pending'}
        ledger.mutate(lambda v: v['intents'].append(row.copy()))
        ledger.ops.run(argv)
        ledger.mutate(lambda v: v['intents'][-1].update(state='done'))
    name = t['namespaces']['A']
    require(ledger.ops.namespace_inode(name) is None, 'backend namespace already exists')
    action([IP, 'netns', 'add', name])
    inode = ledger.ops.namespace_inode(name)
    require(type(inode) is int and inode > 0, 'created namespace inode missing')
    ledger.mutate(lambda v: v['namespaces'].append({'name': name, 'inode': inode}))
    action([IP, '-n', name, 'link', 'set', 'lo', 'up'])
    host = t['hostLinks']['A']; peer = host + 'p'
    action([IP, 'link', 'add', host, 'type', 'veth', 'peer', 'name', peer])
    link = ledger.ops.json([IP, '-j', 'link', 'show', 'dev', host])[0]
    ledger.mutate(lambda v: v['links'].append({'name': host, 'ifindex': link['ifindex'], 'address': link['address']}))
    action([IP, 'link', 'set', peer, 'netns', name])
    action([IP, '-n', name, 'link', 'set', peer, 'name', 'wan0'])
    action([IP, 'address', 'add', t['host']['A'] + '/29', 'dev', host])
    action([IP, 'link', 'set', host, 'up'])
    for address in t['endpoint'].values():
        action([IP, '-n', name, 'address', 'add', address + '/29', 'dev', 'wan0'])
    action([IP, '-n', name, 'link', 'set', 'wan0', 'up'])
    action([IP, '-n', name, 'address', 'add', t['origin'] + '/32', 'dev', 'lo'])
    action([IP, '-n', name, 'address', 'add', t['special'] + '/32', 'dev', 'lo'])
    route = [t['special'] + '/32', 'via', t['endpoint']['A'], 'dev', t['hostLinks']['A'], 'proto', 'static']
    action([IP, '-4', 'route', 'add', *route])
    ledger.mutate(lambda v: (v['routes'].append({'argv': route}), v.update(phase='network-ready')))
    return t


def _safe_remove_network(ledger, value):
    ops, errors = ledger.ops, []
    for ns in value['namespaces']:
        current = ops.namespace_inode(ns['name'])
        if current is None:
            continue
        if current != ns['inode'] or ops.processes_in(ns['inode']):
            errors.append('namespace-identity-or-live-members:' + ns['name'])
    # Do not remove routing under a still-live/unowned fixture or product.
    if errors:
        return errors
    for row in reversed(value['routes']):
        try:
            actual = ops.json([IP, '-j', '-4', 'route', 'show', row['argv'][0]])
            expected = row['argv']
            if actual:
                require(len(actual) == 1 and actual[0].get('dst') in (expected[0], expected[0].split('/')[0])
                        and actual[0].get('gateway') == expected[2] and actual[0].get('dev') == expected[4]
                        and actual[0].get('protocol') == 'static', 'synthetic route identity changed')
                ops.run([IP, '-4', 'route', 'del', *expected])
        except Exception:
            errors.append('route-removal-unknown')
    for row in reversed(value['links']):
        try:
            links = ops.json([IP, '-j', 'link', 'show'])
            found = [r for r in links if r.get('ifname') == row['name']]
            if found:
                require(len(found) == 1 and all(found[0].get(k) == row[k] for k in ('ifindex', 'address')),
                        'veth identity changed')
                ops.run([IP, 'link', 'del', 'dev', row['name']])
        except Exception:
            errors.append('link-removal-unknown:' + row['name'])
    for row in reversed(value['namespaces']):
        try:
            current = ops.namespace_inode(row['name'])
            if current is not None:
                require(current == row['inode'] and not ops.processes_in(current), 'namespace no longer closed')
                ops.run([IP, 'netns', 'del', row['name']])
        except Exception:
            errors.append('namespace-removal-unknown:' + row['name'])
    return errors


def cleanup(ledger, forced=False):
    """Normal network removal requires product uninstall evidence; watchdog FAILs."""
    prior = ledger.load()
    historical_failed = prior.get('result') == 'failed' or bool(prior.get('unknown'))
    forced = forced or prior.get('forcedCleanup') is True
    value = ledger.mutate(lambda v: v.update(phase='cleaning', result='failed' if forced or historical_failed else v['result'],
                                            forcedCleanup=v['forcedCleanup'] or forced))
    errors = list(value['unknown'])
    if historical_failed and not errors: errors.append('earlier-failure-is-permanent')
    if forced:
        helper = value.get('helper')
        if helper:
            try:
                validate_service(service_properties(ledger.ops))
                require(file_identity(HELPER) == helper['binary'] and file_identity(UNIT) == helper['unit'],
                        'helper installation changed')
                current = ledger.ops.proc(helper['identity']['pid'])
                if current:
                    require(same_process(helper['identity'], current), 'helper birth changed')
                    ledger.ops.run([SYSTEMCTL, 'stop', SERVICE], timeout=30)
                ledger.ops.run([SYSTEMCTL, 'disable', SERVICE])
                service_closed(ledger.ops)
            except Exception:
                errors.append('helper-emergency-stop-unknown')
        elif UNIT.exists() or HELPER.exists():
            errors.append('unregistered-product-installation')
        for row in reversed(value['processes']):
            old = row['identity']
            try:
                current = ledger.ops.proc(old['pid'])
                if not current or current.get('state') == 'Z':
                    continue
                require(same_process(old, current), 'registered process identity changed')
                gone = ledger.ops.signal_owned(old, signal.SIGTERM)
                if not gone:
                    require(ledger.ops.signal_owned(old, signal.SIGKILL), 'registered process not reaped')
            except Exception:
                errors.append('owned-process-cleanup-unknown')
    else:
        for child in ledger.children.values():
            require(child.poll() is not None, 'native fixture/capture child still running')
            require(child.wait(timeout=5) == 0, 'native fixture/capture child exit failed')
        require(value.get('productUninstalled') is True, 'real product uninstall required before normal cleanup')
    for row in value['processes']:
        current = ledger.ops.proc(row['identity']['pid'])
        if current and current.get('state') != 'Z':
            errors.append('registered-process-still-live')
    if any(row['state'] == 'pending' for row in value.get('spawning', [])):
        errors.append('native-child-spawn-outcome-unknown')
    if any(row['state'] != 'done' for row in value['intents']):
        errors.append('partial-network-command-unknown')
    if not errors and forced:
        try: recover_tun(ledger, value)
        except Exception: errors.append('product-network-recovery-unproved')
    if not errors:
        errors.extend(_safe_remove_network(ledger, value))
    if not errors:
        try:
            restored = network_snapshot(ledger.ops)
            require(stable_network(restored) == stable_network(value['baseline']['network']), 'VM network baseline differs')
            ledger.mutate(lambda v: v.update(networkRestored=True, finalNetwork=restored))
        except Exception:
            errors.append('network-restoration-unproved')
    ledger.mutate(lambda v: (v.update(unknown=sorted(set(v['unknown'] + errors))), v.update(
        phase='closed' if not errors else 'cleanup-incomplete', result='failed' if forced or errors else v['result'],
        residualInstallation='VM destruction required after emergency stop' if forced else 'product-uninstalled')))
    return {'state': 'forced' if forced else ('normal' if not errors else 'failed'),
            'forced': forced, 'unknown': errors, 'publicationEligible': False}


def _birth_pair(evidence):
    require(same_process(evidence.get('beforeCore'), evidence.get('afterCore')), 'same actual core birth required')


def _stream(row, expected):
    require(row.get('exit') == expected and type(row.get('bytes')) is int and row['bytes'] > 0 and
            re.fullmatch(r'[0-9a-f]{64}', str(row.get('nonceSha256', ''))), 'actual nonce stream proof required')


def validate_dns_rows(rows, backend_role=None):
    require(isinstance(rows, list) and len(rows) == 2 and {r.get('transport') for r in rows} == {'udp', 'tcp'},
            'UDP/TCP DNS required')
    for row in rows:
        require(row.get('destination') == '8.8.8.8' and row.get('tunObserved') is True and
                row.get('analysisExecution') == 'real-tool' and row.get('ttl') == 0 and
                row.get('querySha256') == row.get('captureQuerySha256') and
                row.get('name') == row.get('backendName') and bool(row.get('name')) and
                row.get('type') == row.get('backendType') == 1 and
                row.get('answer') == topology('000000000000')['origin'] and
                all(re.fullmatch(r'[0-9a-f]{64}', str(row.get(k, ''))) for k in
                    ('querySha256', 'backendQuerySha256', 'captureSha256')),
                'actual DNS capture/backend/TTL0 mismatch')
        if backend_role is not None:
            require(row.get('backendRole') == backend_role, 'actual DNS backend did not switch')


def validate_case(name, e):
    require(isinstance(e, dict), 'structured TUN evidence required')
    if name == 'default_graph':
        require(e.get('dashboardEnabled') is True and e.get('defaultSandbox') is True and
                type(e.get('guiUid')) is int and e['guiUid'] > 0 and e.get('startedViaHelper') is True and
                e.get('kernelCommit') == KERNEL_COMMIT and e.get('originalEntry') is True and
                e.get('stableDoubleRead') is True and re.fullmatch(r'[0-9a-f]{64}', str(e.get('actualGraphSha256', ''))),
                'original default graph/helper evidence missing')
    elif name == 'tun_data_plane':
        require(set(e) == {'A', 'B', 'dns'}, 'both actual TUN egresses and DNS required')
        for label in ('A', 'B'):
            require(validate_data_plane(e[label])['exit'] == label, 'TUN egress differs')
        validate_dns_rows(e['dns'])
    elif name in ('graph_apply', 'daily_switch', 'auto_preserves', 'manual_preserve', 'manual_disconnect'):
        _birth_pair(e); _stream(e['newConnection'], 'B')
        if name == 'graph_apply':
            before, after = e['beforeActive'], e['afterActive']
            require(e.get('appStatus') == 'hotApplied' and e.get('operationDurable') is True and
                    before.get('incarnation') == after.get('incarnation') and bool(before.get('incarnation')) and
                    type(before.get('appliedRevision')) is int and after.get('appliedRevision') == before['appliedRevision'] + 1,
                    'real durable graph Apply revision required')
            validate_dns_rows(e.get('dnsBefore'), 'A'); validate_dns_rows(e.get('dnsAfter'), 'B')
            require(not {row['name'] for row in e['dnsBefore']}.intersection(row['name'] for row in e['dnsAfter']),
                    'fresh post-Apply DNS nonce queries required')
        if name in ('graph_apply', 'auto_preserves', 'manual_preserve'):
            _stream(e['oldBefore'], 'A'); _stream(e['oldAfter'], 'A')
        if name == 'auto_preserves':
            require(e.get('autoSelected') == 'node-b' and e.get('backendAAlive') is True, 'actual Auto switch, live old backend required')
        if name == 'manual_preserve': require(e.get('interruptConnectionsOnSwitch') is False, 'preserve preference required')
        if name == 'manual_disconnect':
            require(e.get('interruptConnectionsOnSwitch') is True and e.get('oldConnectionClosed') is True and
                    e.get('backendAAlive') is True, 'preference-controlled disconnect required')
    elif name == 'uncovered_target_restart':
        _birth_pair(e)
        require(e.get('appStatus') == 'restartRequired' and e.get('reasonPath') == 'outbounds.physical_route_binding' and
                e.get('reason') == 'structuralChange' and e.get('restartDeclined') is True and
                e.get('beforeActive') == e.get('afterActive') and bool(e.get('beforeActive')) and
                e.get('oldConnectionPreserved') is True, 'uncovered route must preserve old active graph')
    elif name == 'synthetic_route':
        require(e.get('destination') == topology('000000000000')['special'] and
                e.get('before') == e.get('during') == e.get('after') and bool(e.get('before')) and
                e.get('actualReply') == 'synthetic-route-only' and e.get('claim') == 'synthetic-specific-route-only',
                'bounded synthetic route proof required')
    elif name == 'normal_cleanup':
        require(e.get('state') == 'normal' and e.get('forced') is False and not e.get('unknown') and
                all(e.get(k) is True for k in ('watchdogCancelled', 'networkRestored', 'productUninstalled', 'privilegedProcessesReaped', 'authenticationWindowClosed')),
                'complete normal cleanup proof required')
    else: raise ValueError('unknown TUN case')


def report_result(cases, cleanup_evidence, source_scope, blockers=()):
    """TUN report is deliberately not accepted by the non-TUN release schema."""
    require(source_scope in ('source-diagnostic', 'same-final-carrier'), 'explicit source scope required')
    missing = sorted(set(CASE_IDS) - set(cases)); invalid = []
    for name, case in cases.items():
        try:
            require(case.get('status') == 'passed', 'case did not pass')
            validate_case(name, case.get('evidence'))
        except (ValueError, KeyError, TypeError): invalid.append(name)
    try: validate_case('normal_cleanup', cleanup_evidence)
    except (ValueError, KeyError, TypeError): invalid.append('cleanup')
    passed = not blockers and not missing and not invalid
    actual_failure = (cleanup_evidence.get('forced') is True or cleanup_evidence.get('state') in ('failed', 'forced')
                      or bool(cleanup_evidence.get('unknown')) or any(c.get('status') == 'failed' for c in cases.values()))
    outcome = 'failed' if actual_failure else ('BLOCKED' if blockers else ('success' if passed else 'failed'))
    return {'schema': 'polaris-linux-tun-acceptance-v1', 'result': outcome,
            'scope': source_scope, 'publicationEligible': False, 'cases': cases, 'missingCases': missing,
            'invalidCases': sorted(set(invalid)), 'blocked': list(blockers), 'cleanup': cleanup_evidence}


def validate_data_plane(row):
    require(row.get('source') == 'owned-tun-capture-and-origin' and row.get('analysisExecution') == 'real-tool' and
            re.fullmatch(r'[0-9a-f]{64}', str(row.get('captureSha256', ''))), 'actual capture plus origin proof required')
    destination = ipaddress.ip_address(row['destination'])
    require(destination.version == 4 and not destination.is_loopback and not destination.is_unspecified
            and str(destination) == topology('000000000000')['origin'],
            'nonloopback IPv4 destination required')
    require(row.get('interface') == 'polaris-tun0' and row.get('proxyEnvironment') == 'absent'
            and row.get('clientMode') == 'direct-tcp' and row.get('packets', 0) > 0, 'real TUN ingress required')
    require(row.get('captureNonceSha256') == row.get('originNonceSha256') and
            re.fullmatch(r'[0-9a-f]{64}', str(row.get('captureNonceSha256', ''))), 'same captured/origin nonce required')
    t = topology('000000000000')
    require(row.get('exit') in ('A', 'B') and row.get('originPeer') == t['exit'][row['exit']], 'actual A/B egress source differs')
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ('plan', 'watchdog'):
        p = commands.add_parser(name); p.add_argument('--run-id', required=True)
    args = parser.parse_args()
    if args.command == 'plan':
        print(json.dumps({'topology': topology(args.run_id), 'status': 'BLOCKED',
                          'blocked': ['consumer-integration-and-execution-admission-required'],
                          'mutatingCommandsExecuted': False}, indent=2))
    else:
        ledger = Ledger(args.run_id)
        value = ledger.load()
        require(file_identity(Path(__file__))['sha256'] == value['watchdogScriptSha256'], 'frozen watchdog differs')
        require(time.monotonic() >= value['deadlineMonotonic'], 'watchdog invoked before bound deadline')
        cleanup(ledger, forced=True)
        raise SystemExit(1)  # recovery must never turn timeout into PASS


if __name__ == '__main__':
    main()
