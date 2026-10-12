"""Shared same-filesystem reservation; child-held flock survives parent loss."""
import contextlib
import fcntl
import json
import os
from pathlib import Path
import stat

from fork_input import canonical, require, sha


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
    if os.environ.get('GITHUB_ACTIONS') == 'true':
        require(os.statvfs(work).f_bavail * os.statvfs(work).f_frsize >= value['budget']['buildPeakReserveBytes'],
                'hosted build peak reserve unavailable')
        yield
        return
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
