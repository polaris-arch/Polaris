"""Shared same-filesystem reservation; child-held flock survives parent loss."""
import contextlib
import fcntl
import json
import math
import os
from pathlib import Path
import stat
import subprocess
import sys

from fork_input import canonical, require, sha


def _capacity_text(path):
    """Small kernel observations only; never read build environment or secrets."""
    try:
        with Path(path).open() as stream:
            value = stream.read(16385)
        return value if len(value) <= 16384 else None
    except (OSError, UnicodeError):
        return None


def _cgroup_v2_directories():
    root = Path('/sys/fs/cgroup')
    text = _capacity_text('/proc/self/cgroup') or ''
    for row in text.splitlines():
        if row.startswith('0::'):
            parts = Path(row[3:]).parts
            if '..' in parts:
                return []
            leaf = root.joinpath(*[part for part in parts if part != '/'])
            # Include parent constraints; a child may report "max" under a
            # limited parent. This is observation, not a new scheduler/lease.
            return [leaf, *[parent for parent in leaf.parents if parent == root or root in parent.parents]]
    return [root]


def native_capacity():
    """Observe this runner once, without changing locks, tools or global env.

    Two GiB per concurrent compiler and one GiB headroom only guide concurrency;
    they are unmeasured estimates, never a condition for refusing compilation.
    Low or unknown memory reduces concurrency to one. Darwin's selected vm_stat
    pages are a reclaimable subset, not a complete available-memory measurement.
    Other processes may change after sampling, so this is a cap, not a promise.
    """
    facts = {'schema': 'polaris-native-runner-capacity-v1',
             'scope': 'current runner only; no cross-runner admission',
             'memoryPerJobBytes': 2 * 1024 ** 3, 'memoryHeadroomBytes': 1024 ** 3,
             'logicalCPUs': os.cpu_count(), 'affinityCPUs': None,
             'quotaCPUs': None, 'load1': None, 'availableMemoryBytes': None,
             'memorySource': None, 'cgroupVersion': None, 'quotaObserved': False,
             'cgroupMemoryObserved': False, 'osMemoryObservationBytes': None,
             'osMemoryObservationKind': None, 'cgroupMemoryConstraints': []}
    try:
        facts['affinityCPUs'] = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        pass
    try:
        load = os.getloadavg()[0]
        if math.isfinite(load) and load >= 0:
            facts['load1'] = load
    except (AttributeError, OSError):
        pass
    if sys.platform.startswith('linux'):
        text = _capacity_text('/proc/meminfo') or ''
        for row in text.splitlines():
            words = row.split()
            if len(words) == 3 and words[0] == 'MemAvailable:' and words[1].isdigit() and words[2] == 'kB':
                facts.update(availableMemoryBytes=int(words[1]) * 1024,
                             osMemoryObservationBytes=int(words[1]) * 1024,
                             osMemoryObservationKind='kernel-MemAvailable-estimate',
                             memorySource='proc-meminfo-MemAvailable')
                break
        for path in _cgroup_v2_directories():
            cpu = (_capacity_text(path / 'cpu.max') or '').split()
            if len(cpu) == 2 and (cpu[0] == 'max' or cpu[0].isdigit()) and cpu[1].isdigit() and int(cpu[1]) > 0:
                if cpu[0] != 'max':
                    quota = int(cpu[0]) / int(cpu[1])
                    facts['quotaCPUs'] = min(facts['quotaCPUs'], quota) if facts['quotaCPUs'] is not None else quota
                facts['cgroupVersion'] = 2
                facts['quotaObserved'] = True
            limit = (_capacity_text(path / 'memory.max') or '').strip()
            current = (_capacity_text(path / 'memory.current') or '').strip()
            if (limit == 'max' or limit.isdigit()) and current.isdigit():
                facts['cgroupMemoryObserved'] = True
            if limit.isdigit() and current.isdigit():
                available = max(0, int(limit) - int(current))
                facts['cgroupMemoryConstraints'].append({'version': 2, 'path': str(path),
                    'limitBytes': int(limit), 'usageBytes': int(current), 'headroomBytes': available})
                if facts['availableMemoryBytes'] is None or available < facts['availableMemoryBytes']:
                    facts.update(availableMemoryBytes=available, memorySource='cgroup-v2-headroom')
                facts['cgroupVersion'] = 2
        # Legacy cgroup v1 observations use the process's actual controller path.
        for row in (_capacity_text('/proc/self/cgroup') or '').splitlines():
            fields = row.split(':', 2)
            if len(fields) != 3 or 'cpu' not in fields[1].split(',') or '..' in Path(fields[2]).parts:
                continue
            for root in (Path('/sys/fs/cgroup/cpu'), Path('/sys/fs/cgroup/cpu,cpuacct')):
                leaf = root / fields[2].lstrip('/')
                for path in [leaf, *[parent for parent in leaf.parents if parent == root or root in parent.parents]]:
                    quota = (_capacity_text(path / 'cpu.cfs_quota_us') or '').strip()
                    period = (_capacity_text(path / 'cpu.cfs_period_us') or '').strip()
                    if (quota == '-1' or quota.isdigit()) and period.isdigit() and int(period) > 0:
                        if quota != '-1':
                            count = int(quota) / int(period)
                            facts['quotaCPUs'] = min(facts['quotaCPUs'], count) if facts['quotaCPUs'] is not None else count
                        facts.update(cgroupVersion=1, quotaObserved=True)
        for row in (_capacity_text('/proc/self/cgroup') or '').splitlines():
            fields = row.split(':', 2)
            if len(fields) != 3 or 'memory' not in fields[1].split(',') or '..' in Path(fields[2]).parts:
                continue
            root = Path('/sys/fs/cgroup/memory')
            leaf = root / fields[2].lstrip('/')
            for path in [leaf, *[parent for parent in leaf.parents if parent == root or root in parent.parents]]:
                limit = (_capacity_text(path / 'memory.limit_in_bytes') or '').strip()
                current = (_capacity_text(path / 'memory.usage_in_bytes') or '').strip()
                if limit.isdigit() and current.isdigit():
                    facts['cgroupMemoryObserved'] = True
                    available = max(0, int(limit) - int(current))
                    facts['cgroupMemoryConstraints'].append({'version': 1, 'path': str(path),
                        'limitBytes': int(limit), 'usageBytes': int(current), 'headroomBytes': available})
                    if facts['availableMemoryBytes'] is None or available < facts['availableMemoryBytes']:
                        facts.update(availableMemoryBytes=available, memorySource='cgroup-v1-headroom')
                    facts['cgroupVersion'] = 1
    elif sys.platform == 'darwin':
        facts['quotaObserved'] = True  # No Linux cgroup controller on this host.
        try:
            result = subprocess.run(['/usr/bin/vm_stat'], check=True, capture_output=True,
                                    timeout=2, env={'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'})
            text = result.stdout.decode('ascii') if len(result.stdout) <= 16384 else ''
            first = text.splitlines()[0]
            size = int(first.split('page size of ', 1)[1].split(' bytes', 1)[0])
            pages = {}
            for row in text.splitlines()[1:]:
                name, separator, count = row.partition(':')
                if separator and count.strip().rstrip('.').isdigit():
                    pages[name] = int(count.strip().rstrip('.'))
            names = ('Pages free', 'Pages inactive', 'Pages speculative')
            if size > 0 and all(name in pages for name in names):
                facts.update(availableMemoryBytes=sum(pages[name] for name in names) * size,
                             osMemoryObservationBytes=sum(pages[name] for name in names) * size,
                             osMemoryObservationKind='reclaimable-page-subset-not-complete-available-memory',
                             memorySource='vm-stat-free-inactive-speculative')
        except (OSError, subprocess.SubprocessError, UnicodeError, ValueError, IndexError):
            pass
    result = capacity_limit(facts)
    if sys.platform.startswith('linux') and not facts['quotaObserved']:
        # Keep the real quota unknown; serialize without inventing a quota.
        result['jobCap'] = min(result['jobCap'], 1)
        result['unknown'].append('cpu-quota')
    if sys.platform.startswith('linux') and not facts['cgroupMemoryObserved']:
        # Host MemAvailable does not rule out an unreadable container limit.
        result['jobCap'] = min(result['jobCap'], 1)
        result['unknown'].append('container-memory')
    return result


def capacity_limit(facts):
    """Pure policy calculation so unknown/limited observations can be tested."""
    facts = dict(facts)
    cpus = [value for value in (facts['logicalCPUs'], facts['affinityCPUs'], facts['quotaCPUs'])
            if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value > 0]
    cpu = max(1, math.floor(min(cpus))) if cpus else 1
    load = facts['load1']
    load_cap = max(1, cpu - math.ceil(load)) if load is not None else 1
    memory = facts['availableMemoryBytes']
    estimate = max(0, (memory - facts['memoryHeadroomBytes']) // facts['memoryPerJobBytes']) if memory is not None else None
    memory_cap = max(1, estimate) if estimate is not None else 1
    facts.update(effectiveCPUs=cpu, loadJobCap=load_cap, memoryJobCap=memory_cap,
                 estimatedMemoryJobCap=estimate,
                 memoryEstimateRisk=('unknown-memory' if estimate is None else
                                     'below-unmeasured-single-job-estimate' if estimate == 0 else
                                     'within-unmeasured-concurrency-estimate'),
                 memoryPolicy='concurrency-guidance-only; no memory admission or reservation',
                 peakRSSBytes=None,
                 jobCap=min(cpu, load_cap, memory_cap),
                 unknown=[name for name, value in [('cpu', bool(cpus)), ('load', load is not None),
                                                    ('memory', memory is not None)] if not value])
    return facts


def native_environment(env, observation=None):
    """Return a copied child env and applied cap facts, never alter os.environ."""
    observation = native_capacity() if observation is None else observation
    cap = observation['jobCap']
    require(isinstance(cap, int) and not isinstance(cap, bool) and cap >= 1,
            'native compiler concurrency observation must be a positive integer')
    result, applied = dict(env), dict(observation)
    limits = {}
    for key in ('CARGO_BUILD_JOBS', 'GOMAXPROCS'):
        prior = result.get(key)
        if prior is not None:
            require(isinstance(prior, str) and prior.isascii() and prior.isdigit() and int(prior) > 0,
                    'explicit native compiler concurrency must be a positive integer')
        limits[key] = min(cap, int(prior)) if prior is not None else cap
        result[key] = str(limits[key])
    applied['appliedEnvironment'] = limits
    return result, applied


def group_alive(group):
    try:
        os.killpg(group, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True  # Unknown ownership never releases a reservation.


def register_group(group, *, finished=False):
    inherited_fd = os.environ.get('POLARIS_BUILD_RESERVATION_FD')
    if inherited_fd is None:
        return
    fd = int(inherited_fd)
    root = directory(os.environ.get('POLARIS_BUILD_BUDGET_ROOT', str(Path.home() / '.cache/polaris-build-budget')))
    gate = open_lock(root / '.admission.lock')
    try:
        fcntl.flock(gate, fcntl.LOCK_EX)
        row = json.loads(os.pread(fd, 8192, 0))
        groups = set(row.get('groups', []))
        if finished and not group_alive(group):
            groups.discard(group)
        else:
            groups.add(group)
        require(len(groups) <= 32, 'retained native process-group budget exceeded')
        row['groups'] = sorted(groups)
        raw = canonical(row)
        os.ftruncate(fd, 0)
        os.pwrite(fd, raw, 0)
        os.fsync(fd)
    finally:
        os.close(gate)


def directory(path):
    path = Path(path).absolute()
    for parent in (path, *path.parents):
        require(not parent.is_symlink(), 'budget directory redirected')
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.stat()
    require(info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700,
            'budget directory must be current-user private')
    return path


def open_lock(path):
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    info = os.fstat(fd)
    if not (stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and
            info.st_nlink == 1 and stat.S_IMODE(info.st_mode) == 0o600):
        os.close(fd)
        raise RuntimeError('budget lock identity differs')
    return fd


def inherited(fd, work, value, root):
    gate = open_lock(root / '.admission.lock')
    try:
        fcntl.flock(gate, fcntl.LOCK_SH)
        require(0 <= os.fstat(fd).st_size <= 8192, 'reservation record bounds')
        row = json.loads(os.pread(fd, 8192, 0))
        path = root / row['slot']
        require(path.name == row['slot'] and row['slot'].endswith('.reservation'), 'reservation path differs')
        info, actual = os.fstat(fd), path.lstat()
        require(stat.S_ISREG(actual.st_mode) and (info.st_dev, info.st_ino) == (actual.st_dev, actual.st_ino) and
                info.st_dev == work.stat().st_dev and info.st_uid == os.getuid() and info.st_nlink == 1 and
                stat.S_IMODE(info.st_mode) == 0o600 and not path.is_symlink() and
                row['filesystem'] == info.st_dev and Path(row['work']).stat().st_dev == info.st_dev and
                row['schema'] == 'polaris-mobile-filesystem-reservation-v1' and
                row['reserveBytes'] >= value['budget']['buildPeakReserveBytes'], 'inherited reservation identity differs')
        # On an inherited open-file description this retains the existing lock;
        # another description cannot steal a live reservation.
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    finally:
        os.close(gate)


@contextlib.contextmanager
def reservation(work, value, active_fds):
    work = directory(work)
    # Hosted and local builders use the same child-held ledger descriptor.
    # A free-space observation alone cannot admit/transport a reservation.
    # All local formal mobile entries share one ledger on this filesystem.
    # An explicit root supports an external build volume; all concurrent callers
    # on that volume must use the same root.
    root = directory(os.environ.get('POLARIS_BUILD_BUDGET_ROOT', str(Path.home() / '.cache/polaris-build-budget')))
    require(root.stat().st_dev == work.stat().st_dev, 'budget and work must share a filesystem')
    prior = os.environ.get('POLARIS_BUILD_RESERVATION_FD')
    if prior is not None:
        require(prior.isdigit(), 'reservation descriptor invalid')
        fd = int(prior)
        inherited(fd, work, value, root)
        added = fd not in active_fds
        if added:
            active_fds.append(fd)
        try:
            yield
        finally:
            if added:
                active_fds.remove(fd)
        return
    gate, fd = open_lock(root / '.admission.lock'), None
    try:
        fcntl.flock(gate, fcntl.LOCK_EX)
        slot = sha(str(work).encode()) + '.reservation'
        fd = open_lock(root / slot)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if os.fstat(fd).st_size:
            old = json.loads(os.pread(fd, 8192, 0))
            require(not any(group_alive(group) for group in old.get('groups', [])),
                    'same work slot has a retained live/unknown native session')
        outstanding = 0
        rows = list(root.glob('*.reservation'))
        require(len(rows) <= 128, 'reservation ledger slot budget exceeded')
        for path in rows:
            if path.name == slot:
                continue
            other = open_lock(path)
            try:
                try:
                    fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    if os.fstat(other).st_size:
                        old = json.loads(os.pread(other, 8192, 0))
                        if any(group_alive(group) for group in old.get('groups', [])):
                            outstanding += old['reserveBytes']
                except BlockingIOError:
                    require(os.fstat(other).st_size <= 8192, 'live reservation bounds')
                    outstanding += json.loads(os.pread(other, 8192, 0))['reserveBytes']
            finally:
                os.close(other)
        reserve = value['budget']['buildPeakReserveBytes']
        space = os.statvfs(work)
        require(space.f_bavail * space.f_frsize >= outstanding + reserve + 1024 ** 3,
                'shared filesystem peak reservations exhaust free space')
        row = {'schema': 'polaris-mobile-filesystem-reservation-v1', 'slot': slot,
               'work': str(work), 'filesystem': work.stat().st_dev, 'reserveBytes': reserve, 'groups': []}
        os.ftruncate(fd, 0)
        os.pwrite(fd, canonical(row), 0)
        os.fsync(fd)
        os.set_inheritable(fd, True)
        active_fds.append(fd)
        os.environ['POLARIS_BUILD_RESERVATION_FD'] = str(fd)
        fcntl.flock(gate, fcntl.LOCK_UN)
        yield
    finally:
        if fd is not None:
            if fd in active_fds:
                active_fds.remove(fd)
                os.environ.pop('POLARIS_BUILD_RESERVATION_FD', None)
            os.close(fd)
        os.close(gate)
