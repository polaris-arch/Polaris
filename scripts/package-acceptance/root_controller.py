#!/usr/bin/env python3
"""Fixed, local TUN QA controller. Import/tests never perform privileged actions.

The workflow first admits the exact source and carrier. BOOTSTRAP is a fixed
stdlib program, not a sudo execution of a runner-writable Python filename.
Credential actions are default-closed behind exact dispatch authority; no arbitrary
exec/path/kill/secret RPC exists. Source implementation does not authorize execution.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
import re
import select
import signal
import stat
import subprocess
import sys
import threading
import time

SCHEMA = 'polaris-tun-controller-v1'
BASE = Path('/run/polaris-tun-controller')
FILES = ('root_controller.py', 'tun.py', 'tun_fixtures.py', 'fixtures.py')
ENV = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C', 'LC_ALL': 'C'}
MAX_REQUEST = 65536
MAX_CAPTURE = 16 * 1024 * 1024
# This literal is the entire root bootstrap program. Parent source/transport
# admission must bind its bytes and codeHashes; the stdin manifest is not itself
# a source/carrier verifier. No user-selected destination or command is accepted.
BOOTSTRAP = r'''import os,sys,json,base64,hashlib,re,stat
from pathlib import Path
assert os.geteuid()==0 and sys.flags.isolated
raw=sys.stdin.buffer.readline(2*1024*1024+1)
assert raw.endswith(b'\n') and len(raw)<=2*1024*1024
v=json.loads(raw); assert set(v)=={'launch','files'}
l=v['launch']; r=l['runId']; assert re.fullmatch('[0-9a-f]{12}',r)
names={'root_controller.py','tun.py','tun_fixtures.py','fixtures.py'}
assert set(v['files'])==set(l['codeHashes'])==names
contents={}
for n in names:
 b=base64.b64decode(v['files'][n],validate=True)
 assert 0<len(b)<=1024*1024 and hashlib.sha256(b).hexdigest()==l['codeHashes'][n]
 contents[n]=b
base=Path('/run/polaris-tun-controller')
if not base.exists(): base.mkdir(mode=0o700)
for p in [base,*base.parents]:
 s=p.lstat(); assert stat.S_ISDIR(s.st_mode) and s.st_uid==0 and not s.st_mode&0o022
assert not base.stat().st_mode&0o077
d=base/r; d.mkdir(mode=0o700)
contents['launch.json']=json.dumps(l,sort_keys=True,separators=(',',':')).encode()
for n,b in contents.items():
 fd=os.open(d/n,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(fd,'wb') as f: f.write(b); f.flush(); os.fsync(f.fileno())
p=Path(l['rpcProbePath']); fd=os.open(p,os.O_RDONLY|os.O_NOFOLLOW)
with os.fdopen(fd,'rb') as f:
 s=os.fstat(f.fileno()); assert stat.S_ISREG(s.st_mode) and s.st_uid==l['guiUid'] and s.st_nlink==1 and 0<s.st_size<=128*1024*1024
 b=f.read(); assert hashlib.sha256(b).hexdigest()==l['rpcProbeSha256']
fd=os.open(d/'rpc-probe',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o500)
with os.fdopen(fd,'wb') as f: f.write(b); f.flush(); os.fsync(f.fileno())
os.execve('/usr/bin/python3',['/usr/bin/python3','-I',str(d/'root_controller.py'),'serve','--run-id',r],{'PATH':'/usr/sbin:/usr/bin:/sbin:/bin','LANG':'C','LC_ALL':'C'})
'''


class AuthenticationNotReady(ValueError):
    pass


def require(value, message):
    if not value:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def exact(value, keys):
    require(type(value) is dict and set(value) == set(keys), 'unexpected fields')


def validate_launch(value, recovery=False):
    exact(value, ('schema', 'runId', 'guiUid', 'driver', 'guiHome', 'helperScriptDir',
                  'runtimePath', 'appSha256', 'helperSha256', 'coreSha256',
                  'carrierSha256', 'kernelCommit', 'originalAdmission', 'vmContext',
                  'materialsDownloaded', 'codeHashes', 'authenticationMode', 'execution', 'authorization',
                  'rpcProbePath', 'rpcProbeSha256'))
    require(value['schema'] == SCHEMA and re.fullmatch(r'[0-9a-f]{12}', value['runId']), 'launch binding differs')
    require(type(value['guiUid']) is int and value['guiUid'] > 0, 'ordinary GUI required')
    exact(value['driver'], ('pid', 'startTime'))
    require(type(value['driver']['pid']) is int and value['driver']['pid'] > 1 and
            re.fullmatch(r'[0-9]+', value['driver']['startTime']), 'driver birth required')
    home = Path(value['guiHome'])
    require(home.is_absolute() and '..' not in home.parts and home != Path('/'), 'private HOME required')
    for field in ('helperScriptDir', 'runtimePath'):
        path = Path(value[field])
        require(path.is_absolute() and '..' not in path.parts and path.is_relative_to(home) and path != home,
                'fixed private runtime path required')
    exact(value['vmContext'], ('githubHosted', 'dedicatedVM'))
    require(all(v is True for v in value['vmContext'].values()) and value['materialsDownloaded'] is True,
            'exclusive prefetched GitHub VM required')
    exact(value['codeHashes'], FILES)
    for h in [value[k] for k in ('appSha256', 'helperSha256', 'coreSha256', 'carrierSha256', 'rpcProbeSha256')] + list(value['codeHashes'].values()):
        require(type(h) is str and re.fullmatch(r'[0-9a-f]{64}', h), 'frozen hash required')
    require(value['kernelCommit'] == 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a' and
            value['originalAdmission'] == 'verified', 'complete c6 carrier admission required')
    validate_execution(value['execution'])
    require(Path(value['rpcProbePath']).is_absolute(), 'fixed admitted test probe required')
    require(value['authenticationMode'] in ('existing-session', 'pkttyagent-observe', AUTH_MODE), 'unknown authentication mode')
    if value['authenticationMode'] == AUTH_MODE: validate_permission(value, recovery=recovery)
    else: require(value['authorization'] is None, 'unexpected credential permission')
    return value


def bootstrap_envelope(launch, source_dir):
    """Ordinary side only; call after original transport/source admission."""
    validate_launch(launch)
    files = {}
    for name in FILES:
        data = (Path(source_dir) / name).read_bytes()
        require(sha(data) == launch['codeHashes'][name], 'source admission changed before bootstrap')
        files[name] = base64.b64encode(data).decode()
    return json.dumps({'launch': launch, 'files': files}, separators=(',', ':')).encode() + b'\n'


def load_frozen(run_id, recovery=False):
    require(os.geteuid() == 0 and sys.flags.isolated, 'isolated root entry required')
    require(re.fullmatch(r'[0-9a-f]{12}', run_id), 'run token required')
    directory = BASE / run_id
    require(Path(__file__).absolute() == directory / 'root_controller.py', 'frozen entry required')
    for path in (directory, *directory.parents):
        info = path.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == 0 and not info.st_mode & 0o022,
                'root-owned directory required')
    def read(name):
        fd = os.open(directory / name, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, 'rb') as f:
            s = os.fstat(f.fileno())
            require(stat.S_ISREG(s.st_mode) and s.st_uid == 0 and s.st_nlink == 1 and
                    not s.st_mode & 0o077 and s.st_size < 1024 * 1024, 'private frozen file required')
            return f.read()
    launch = validate_launch(json.loads(read('launch.json')), recovery=recovery)
    require(launch['runId'] == run_id, 'frozen run differs')
    for name in FILES:
        require(sha(read(name)) == launch['codeHashes'][name], 'frozen code changed')
    spec = importlib.util.spec_from_file_location('polaris_root_tun', directory / 'tun.py')
    tun = importlib.util.module_from_spec(spec); spec.loader.exec_module(tun)
    return launch, tun, directory


def read_line(stream, timeout=10, maximum=MAX_REQUEST):
    """Unbuffered FD reads avoid buffered readline/select disagreement and stalls."""
    end = time.monotonic() + timeout; data = bytearray()
    while len(data) <= maximum:
        remaining = end - time.monotonic()
        require(remaining > 0 and select.select([stream], [], [], remaining)[0], 'bounded pipe response timed out')
        part = os.read(stream.fileno(), 1)
        require(part, 'pipe closed before complete response')
        data += part
        if part == b'\n':
            return bytes(data)
    raise ValueError('pipe response exceeded bound')


def command_line(pid):
    data = Path('/proc', str(pid), 'cmdline').read_bytes()
    require(0 < len(data) <= MAX_REQUEST and data.endswith(b'\0'), 'bounded live command required')
    return [p.decode('utf-8', 'strict') for p in data[:-1].split(b'\0')]


def uid_tuple(pid):
    text = Path('/proc', str(pid), 'status').read_text()
    return tuple(int(v) for v in next(l for l in text.splitlines() if l.startswith('Uid:')).split()[1:])


def birth(value):
    return value['pid'], value['startTime']


def private_bytes(path, uid, home, maximum=8 * 1024 * 1024):
    path, home = Path(path), Path(home)
    require(path.is_relative_to(home), 'private file outside HOME')
    for parent in (path.parent, *path.parent.parents):
        info = parent.lstat()
        require(stat.S_ISDIR(info.st_mode), 'symlink/private ancestor differs')
        if parent == home:
            require(info.st_uid == uid and not info.st_mode & 0o077, 'HOME is not private')
            break
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, 'rb') as f:
        before = os.fstat(f.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_uid == uid and before.st_nlink == 1 and
                not before.st_mode & 0o077 and 0 < before.st_size <= maximum, 'private regular bounded file required')
        data = f.read(); f.seek(0); again = f.read(); after = os.fstat(f.fileno())
        sig = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
        require(data == again and sig(before) == sig(after) == sig(path.lstat()), 'private file unstable or replaced')
    return data, {'sha256': sha(data), 'bytes': len(data), 'dev': before.st_dev, 'inode': before.st_ino,
                  'stableDoubleRead': True}


AUTH_PHRASE = 'AUTHORIZE TEMPORARY RUNNER PASSWORD AND RELOCK FOR THIS RUN'
AUTH_MODE = 'temporary-existing-runner-password'
EXECUTION_KEYS = ('repository', 'sourceSha', 'sourceTree', 'githubRunId', 'githubRunAttempt',
                  'actor', 'workflowRef', 'eventSha256')


def validate_execution(value):
    exact(value, EXECUTION_KEYS)
    require(re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', value['repository']) and
            re.fullmatch(r'[A-Za-z0-9-]+', value['actor']), 'execution actor/repository required')
    require(all(re.fullmatch(r'[0-9a-f]{40}', value[k]) for k in ('sourceSha', 'sourceTree')) and
            re.fullmatch(r'[0-9a-f]{64}', value['eventSha256']), 'actual event/source hashes required')
    require(all(type(value[k]) is str and re.fullmatch(r'[1-9][0-9]*', value[k]) for k in
                ('githubRunId', 'githubRunAttempt')) and value['workflowRef'].startswith(value['repository'] + '/.github/workflows/'),
            'actual workflow/run attempt required')


def validate_permission(launch, now=None, recovery=False):
    """Frozen real-event admission is authority, NEVER pkexec authentication proof."""
    p = launch.get('authorization')
    exact(p, ('schema', 'phrase', 'authMode', 'execution', 'runToken', 'guiUid', 'account',
              'transactions', 'notBeforeUnix', 'expiresUnix', 'maxLeaseSeconds'))
    require(p['schema'] == 'polaris-tun-credential-lease-v1' and p['phrase'] == AUTH_PHRASE and
            p['authMode'] == AUTH_MODE and launch['authenticationMode'] == AUTH_MODE, 'specific credential authorization absent')
    require(p['execution'] == launch['execution'] and launch['execution']['githubRunAttempt'] == '1' and p['runToken'] == launch['runId'] and
            p['guiUid'] == launch['guiUid'] and type(p['guiUid']) is int, 'credential execution/UID binding differs')
    require(re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', p['account']) and p['transactions'] == ['install', 'uninstall'] and
            p['maxLeaseSeconds'] == 90, 'fixed existing-account actions required')
    require(type(p['notBeforeUnix']) is int and type(p['expiresUnix']) is int and
            0 < p['expiresUnix'] - p['notBeforeUnix'] <= 900, 'bounded credential authority required')
    if not recovery:
        now = int(time.time()) if now is None else now
        require(p['notBeforeUnix'] <= now and now + p['maxLeaseSeconds'] <= p['expiresUnix'],
                'credential permission not yet valid or expired')
    return p


def account_status(name):
    result = subprocess.run(['/usr/bin/passwd', '-S', name], env=ENV, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=True)
    words = result.stdout.decode('ascii', 'strict').split()
    require(len(words) == 7 and words[0] == name and words[1] in ('L', 'NP', 'P'), 'account status ambiguous')
    return words[1]


def credential_account(launch, tun, recovery=False):
    import grp
    p = validate_permission(launch, recovery=recovery)
    users = [u for u in pwd.getpwall() if u.pw_uid == p['guiUid']]
    require(len(users) == 1 and users[0].pw_name == p['account'] and pwd.getpwnam(p['account']).pw_uid == p['guiUid'],
            'unique existing approved account required')
    user = users[0]
    if not recovery:
        require(user.pw_shell in ('/bin/bash', '/usr/bin/bash') and user.pw_dir.startswith('/home/'), 'existing runner login account required')
    groups = [g.gr_name for g in grp.getgrall() if user.pw_name in g.gr_mem or g.gr_gid == user.pw_gid]
    if not recovery:
        require('sudo' in groups or 'admin' in groups, 'existing administrator group required; no group changes allowed')
    for path in ('/usr/bin/passwd', '/usr/sbin/chpasswd', '/usr/bin/pkttyagent', '/usr/bin/setsid', '/usr/bin/setpriv'):
        tun.file_identity(Path(path).resolve())
    status = account_status(user.pw_name)
    if not recovery: require(status in ('L', 'NP'), 'existing usable password must never be overwritten')
    return user, status


def relock_account(launch, tun):
    """Independent recovery does not need a readable mutable product ledger."""
    user, _ = credential_account(launch, tun, recovery=True)
    subprocess.run(['/usr/bin/passwd', '-l', user.pw_name], env=ENV, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, check=True)
    require(account_status(user.pw_name) == 'L', 'temporary account re-lock not proved')
    return {'account': user.pw_name, 'guiUid': user.pw_uid, 'finalPasswordStatus': 'L',
            'emptyPasswordRestored': False, 'plaintextPersisted': False, 'temporaryPasswordHash': 'locked until disposable VM destruction'}


def duration_seconds(text):
    text = text.strip(); pieces = re.findall(r'([0-9]+(?:\.[0-9]+)?)\s*(us|ms|s|min|h|d)', text)
    residue = re.sub(r'([0-9]+(?:\.[0-9]+)?)\s*(us|ms|s|min|h|d)', '', text).strip()
    require(pieces and not residue, 'unknown systemd monotonic duration representation')
    scale = {'us': 0.000001, 'ms': 0.001, 's': 1, 'min': 60, 'h': 3600, 'd': 86400}
    return sum(float(n) * scale[u] for n, u in pieces)


def verify_timer(ops, tun, unit, argv, seconds, started):
    props = dict(line.split('=', 1) for line in ops.run([tun.SYSTEMCTL, 'show', unit + '.service',
        '--property=ExecStart,User,Group,DynamicUser,FragmentPath,DropInPaths,ActiveState']).splitlines() if '=' in line)
    require(props.get('User') in ('', 'root', '0') and props.get('Group') in ('', 'root', '0') and
            props.get('DynamicUser') == 'no' and not props.get('DropInPaths') and props.get('ActiveState') == 'inactive' and
            props.get('FragmentPath') == '/run/systemd/transient/' + unit + '.service', 'watchdog root transient service differs')
    require('path=' + argv[0] + ' ;' in props.get('ExecStart', '') and
            'argv[]=' + ' '.join(argv) + ' ;' in props.get('ExecStart', ''), 'watchdog fixed command differs')
    timer = dict(line.split('=', 1) for line in ops.run([tun.SYSTEMCTL, 'show', unit + '.timer',
        '--property=ActiveState,Unit,TimersMonotonic']).splitlines() if '=' in line)
    require(timer.get('ActiveState') == 'active' and timer.get('Unit') == unit + '.service', 'watchdog target/activation differs')
    triggers = re.findall(r'OnActiveUSec=([^;]+)\s*;\s*next_elapse=([^;}]+)', timer.get('TimersMonotonic', ''))
    require(len(triggers) == 1 and duration_seconds(triggers[0][0]) == seconds, 'watchdog interval differs')
    target = duration_seconds(triggers[0][1])
    require(started + seconds - 2 <= target <= time.monotonic() + seconds + 2, 'watchdog actual deadline differs')
    return {'unit': unit, 'deadlineMonotonic': target, 'rootFixedCommandVerified': True}


def independent_record(directory, name, value):
    require(re.fullmatch(r'(controller-birth|agent-(install|uninstall)|writer-(install|uninstall)|setting-intent-(install|uninstall))\.json', name), 'fixed recovery identity filename required')
    fd = os.open(directory / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(value, f, sort_keys=True); f.flush(); os.fsync(f.fileno())
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def watchdog_recover(launch, tun, directory, transaction=None):
    """Stop recorded writers/secret processes BEFORE final lock; no intent, no lock."""
    errors = []; recovery = None; ops = tun.Commands()
    kinds = [transaction] if transaction else ['install', 'uninstall']
    def read_identity(name):
        path = directory / name
        if not path.exists(): return None
        tun.root_path(path); require(path.stat().st_size <= 16384, 'bounded recovery birth required')
        return json.loads(path.read_bytes())
    # Stop the sole producer FIRST. This closes the intent->spawn->record->stdin
    # race: after exit no new writer can be spawned or receive a secret. A writer
    # not yet recorded has received no input and observes EOF when this parent dies.
    producer_stopped = False
    try:
        controller = read_identity('controller-birth.json')
        require(controller is not None, 'independent controller birth missing')
        stop_recorded(ops, tun, controller); producer_stopped = True
    except Exception: errors.append('owned-secret-controller-stop-unproved')
    intents = []
    if launch['authenticationMode'] == AUTH_MODE:
        for kind in kinds:
            try:
                if has_setting_intent(directory, kind, launch, tun): intents.append(kind)
            except Exception: errors.append('credential-setting-intent-unknown')
    # Even if producer termination is unknown, attempt owned writer stop and
    # lock but never claim restoration; this timeout is permanently failed.
    for kind in kinds:
        try:
            writer = read_identity('writer-' + kind + '.json')
            if writer: stop_recorded(ops, tun, writer)
            elif kind in intents: errors.append('setting-writer-not-yet-recorded')
        except Exception: errors.append('credential-writer-stop-unproved')
    known = set()
    for kind in kinds:
        try:
            path = directory / ('auth-lineage-' + kind + '.json')
            if path.exists():
                tun.root_path(path); require(path.stat().st_size <= 1024 * 1024, 'auth lineage bound')
                rows = json.loads(path.read_bytes())
                require(rows['runId'] == launch['runId'] and rows['kind'] == kind and not rows.get('observationError'),
                        'authentication lineage snapshot incomplete')
                for old in reversed(rows['processes']):
                    known.add(birth(old)); stop_recorded(ops, tun, old)
            agent = read_identity('agent-' + kind + '.json')
            if agent and birth(agent) not in known: known.add(birth(agent)); stop_recorded(ops, tun, agent)
        except Exception: errors.append('authentication-lineage-stop-unproved')
    # Unknown setting result still requires lock; a final L is not a stability
    # claim if any writer/lineage stop above is unproved. Such a run always FAILs.
    if intents:
        try: recovery = relock_account(launch, tun)
        except Exception: errors.append('credential-relock-unproved')
    ledger = tun.Ledger(launch['runId'])
    try:
        ledger.mutate(lambda v: (v.update(result='failed', forcedCleanup=True),
            v['authentication'].update(accountMutated=bool(intents) and recovery is None),
            v.update(unknown=sorted(set(v['unknown'] + errors)))))
        try: close_auth(ledger, tun, forced=True)
        finally:
            try: close_transactions(ledger, tun)
            finally: tun.cleanup(ledger, forced=True)
    except Exception: errors.append('product-ledger-recovery-incomplete')
    out = directory / ('emergency-' + (transaction or 'auth') + '.json')
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump({'result': 'failed', 'settingIntents': intents, 'credentialRecovery': recovery,
                   'restorationProven': False, 'errors': sorted(set(errors))}, f)
        f.flush(); os.fsync(f.fileno())
    return {'result': 'failed', 'errors': sorted(set(errors))}


def write_recovery_snapshot(directory, name, value):
    require(re.fullmatch(r'auth-lineage-(install|uninstall)\.json', name), 'fixed auth lineage snapshot required')
    temp = directory / (name + '.tmp-' + os.urandom(6).hex())
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f: json.dump(value, f, sort_keys=True); f.flush(); os.fsync(f.fileno())
    os.replace(temp, directory / name)
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def setting_intent(directory, kind, launch, original):
    require(kind in ('install', 'uninstall') and original in ('L', 'NP'), 'fresh locked account intent required')
    value = {'runId': launch['runId'], 'guiUid': launch['guiUid'], 'account': launch['authorization']['account'],
             'kind': kind, 'originalPasswordStatus': original, 'permissionSha256': sha(json.dumps(launch['authorization'], sort_keys=True).encode()),
             'settingAttemptMayHaveOccurred': True}
    independent_record(directory, 'setting-intent-' + kind + '.json', value)
    return value


def has_setting_intent(directory, kind, launch, tun):
    path = directory / ('setting-intent-' + kind + '.json')
    if not path.exists(): return False
    tun.root_path(path); require(path.stat().st_size < 16384, 'setting intent bound')
    value = json.loads(path.read_bytes())
    require(value == {'runId': launch['runId'], 'guiUid': launch['guiUid'], 'account': launch['authorization']['account'],
        'kind': kind, 'originalPasswordStatus': value.get('originalPasswordStatus'),
        'permissionSha256': sha(json.dumps(launch['authorization'], sort_keys=True).encode()), 'settingAttemptMayHaveOccurred': True}
        and value['originalPasswordStatus'] in ('L', 'NP'), 'setting intent authority differs')
    return True


def proc_absent(pid):
    """Absence only: unreadable /proc or an extant zombie/reused PID is not gone."""
    require(type(pid) is int and pid > 1, 'exact non-init PID required')
    try: Path('/proc', str(pid)).lstat()
    except FileNotFoundError: return True
    return False


def proc_stat(pid):
    """Read birth/state/parent even after zombie exe has disappeared."""
    require(type(pid) is int and pid > 1, 'exact non-init PID required')
    try: raw = Path('/proc', str(pid), 'stat').read_text()
    except FileNotFoundError:
        require(proc_absent(pid), 'process exists but stat is unavailable')
        return None
    end = raw.rfind(')')
    require(end > 0 and raw[:raw.index(' ')].isdigit() and int(raw[:raw.index(' ')]) == pid, 'process stat identity malformed')
    fields = raw[end + 2:].split()
    require(len(fields) >= 20 and fields[1].isdigit() and fields[19].isdigit(), 'process stat fields malformed')
    return {'pid': pid, 'startTime': fields[19], 'state': fields[0], 'ppid': int(fields[1])}


def stop_recorded(ops, tun, old):
    now = ops.proc(old['pid'])
    if not now:
        remaining = proc_stat(old['pid'])
        if remaining is None: return False
        require(birth(remaining) == birth(old) and remaining['state'] == 'Z', 'registered process exists without verifiable executable')
        return False
    if now['state'] == 'Z':
        require(birth(old) == birth(now), 'registered zombie birth changed')
        return False
    require(tun.same_process(old, now), 'registered secret-process birth/exec changed')
    if not ops.signal_owned(old, signal.SIGTERM):
        require(ops.signal_owned(old, signal.SIGKILL), 'registered secret-process failed to stop')
        return True
    return False


class AuthObserver:
    """Own native agent subtree; unknown adopted/reparented children remain FAIL.

The controller is a process-local Linux subreaper, not a namespace/PAM change.
This makes an unobserved orphan detectable at closure rather than treating agent
exit as proof all authentication helpers are gone. No unknown PID is killed.
"""
    def __init__(self, c, kind):
        self.c = c; self.kind = kind; self.rows = {}; self.error = None
        self.stop = threading.Event(); self.lock = threading.Lock()
        agent = c.ledger.load()['authentication']['agent']['identity']; self.rows[birth(agent)] = agent
        self.thread = threading.Thread(target=self.loop, name='owned-auth-lineage', daemon=True)

    def start(self):
        self.scan(); self.thread.start()

    def loop(self):
        while not self.stop.wait(0.005):
            try: self.scan()
            except Exception as e: self.error = type(e).__name__; return

    def scan(self):
        values = {}
        for entry in Path('/proc').iterdir():
            if entry.name.isdigit() and int(entry.name) > 1:
                p = self.c.ops.proc(int(entry.name))
                if p and p['state'] != 'Z': values[p['pid']] = p
        with self.lock:
            for _ in range(8):
                changed = False
                for p in values.values():
                    key = birth(p); parent = values.get(p['ppid'])
                    if key not in self.rows and not (parent and birth(parent) in self.rows and
                            self.c.tun.same_process(self.rows[birth(parent)], parent)):
                        continue
                    require(p['uid'] in (0, self.c.launch['guiUid']) and p['namespaces'] == self.c.gui['namespaces'] and
                            p['cgroup'] == self.rows[next(iter(self.rows))]['cgroup'], 'auth helper escaped native lineage')
                    if self.rows.get(key) != p:
                        self.rows[key] = p; changed = True
                if not changed: break
            write_recovery_snapshot(self.c.directory, 'auth-lineage-' + self.kind + '.json',
                {'runId': self.c.launch['runId'], 'kind': self.kind, 'processes': list(self.rows.values()), 'observationError': self.error})

    def close(self, forced=False):
        self.stop.set(); self.thread.join(timeout=3)
        require(not self.thread.is_alive(), 'authentication lineage observer stuck')
        self.scan()
        require(self.error is None, 'authentication lineage observation incomplete')
        # Signal only root-observed, exact births; children before their parent.
        for old in reversed(list(self.rows.values())):
            needed_force = stop_recorded(self.c.ops, self.c.tun, old)
            require(forced or not needed_force, 'authentication helper required force kill')
        allowed = set(self.c.ledger.children)
        if self.c.agent: allowed.add(self.c.agent.pid)
        if self.c.lease and self.c.lease.writer: allowed.add(self.c.lease.writer.pid)
        unknown = []
        for entry in Path('/proc').iterdir():
            if not entry.name.isdigit() or int(entry.name) <= 1: continue
            p = proc_stat(int(entry.name))
            if not p: continue
            key = birth(p)
            if p['pid'] in {old['pid'] for old in self.rows.values()}:
                require(key in self.rows, 'auth child birth changed at reap')
                require(p['state'] == 'Z', 'authentication helper remains live')
                if p['ppid'] == os.getpid() and p['pid'] not in allowed:
                    result = os.waitpid(p['pid'], os.WNOHANG)
                    require(result[0] == p['pid'], 'adopted authentication helper was not reaped')
            elif p['ppid'] == os.getpid() and p['pid'] not in allowed:
                unknown.append({'pid': p['pid'], 'startTime': p['startTime']})
        require(not unknown, 'unobserved adopted child: authentication closure unknown')
        return {'ownedBirths': [list(k) for k in self.rows], 'unknownAdoptedChildren': [], 'forced': forced}


class CredentialLease:
    """Default closed; complete guarded implementation, never exercised by tests.

Only a frozen exact real-event authority enables this path. Independent timers
hold account identity/code, NEVER the random password. A timer exists and is
observably active before a password-setting attempt. Each transaction re-locks.
"""
    def __init__(self, controller, kind):
        self.c = controller; self.kind = kind; self.secret = bytearray(); self.attempted = False
        self.closed = False; self.error = None; self.sent = False; self.authenticated = False; self.close_lock = threading.Lock()
        self.stop = threading.Event(); self.thread = None; self.writer = None; self.writer_fd = None
        self.timer = 'polaris-tun-lease-' + controller.launch['runId'] + '-' + kind

    def open(self):
        c = self.c; p = validate_permission(c.launch)
        self.user, original = credential_account(c.launch, c.tun)
        require(c.agent and c.ledger.load()['authentication'].get('registered') is True, 'real agent must register first')
        window = min(p['maxLeaseSeconds'], int(c.ledger.load()['deadlineMonotonic'] - time.monotonic()) - 35)
        require(window >= 15, 'not enough guarded time for another credential lease')
        self.window = window
        state = {'kind': self.kind, 'account': self.user.pw_name, 'guiUid': self.user.pw_uid,
                 'originalPasswordStatus': original, 'state': 'guard-pending',
                 'deadlineMonotonic': time.monotonic() + window, 'timer': self.timer + '.timer'}
        c.ledger.mutate(lambda v: v['authentication'].update(lease=state))
        started = time.monotonic()
        argv = ['/usr/bin/env', '-i', 'PATH=' + ENV['PATH'], 'LANG=C', '/usr/bin/python3', '-I',
                str(c.directory / 'root_controller.py'), 'watch-lease', '--run-id', c.launch['runId'], '--transaction', self.kind]
        c.ops.run(['/usr/bin/systemd-run', '--unit=' + self.timer, '--on-active=' + str(window) + 's', '--timer-property=AccuracySec=1s',
            '--property=Type=oneshot', '--property=TimeoutStartSec=20s', '/usr/bin/env', '-i', 'PATH=' + ENV['PATH'], 'LANG=C',
            '/usr/bin/python3', '-I', str(c.directory / 'root_controller.py'), 'watch-lease', '--run-id', c.launch['runId'],
            '--transaction', self.kind])
        verify_timer(c.ops, c.tun, self.timer, argv, window, started)
        # The setting outcome may become unknown: persist recovery identity first.
        c.ledger.mutate(lambda v: v['authentication']['lease'].update(state='setting-pending', watchdogActive=True))
        import secrets
        self.secret = bytearray(secrets.token_urlsafe(48).encode('ascii'))
        try:
            original_now = account_status(self.user.pw_name)
            require(original_now in ('L', 'NP'), 'account changed before setting')
            setting_intent(c.directory, self.kind, c.launch, original_now)
            self.attempted = True  # only the durable fresh-status intent grants re-lock recovery
            self.writer = subprocess.Popen(['/usr/sbin/chpasswd', '--crypt-method', 'SHA512', '--sha-rounds', '100000'], env=ENV, stdin=subprocess.PIPE,
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.writer_fd = os.pidfd_open(self.writer.pid, 0)
            writer = c.ops.proc(self.writer.pid)
            require(writer and writer['uid'] == 0 and writer['ppid'] == os.getpid() and
                    c.tun.executable_identity(self.writer.pid) == c.tun.file_identity(Path('/usr/sbin/chpasswd').resolve()),
                    'fixed credential writer birth/executable differs')
            independent_record(c.directory, 'writer-' + self.kind + '.json', writer)
            # No secret reaches chpasswd before its exact birth is durably recoverable.
            self.writer.communicate(input=self.user.pw_name.encode() + b':' + bytes(self.secret) + b'\n', timeout=10)
            require(self.writer.returncode == 0, 'credential writer failed')
            require(account_status(self.user.pw_name) == 'P', 'temporary password installation not observed')
            c.ledger.mutate(lambda v: (v['authentication'].update(accountMutated=True),
                                      v['authentication']['lease'].update(state='active')))
            self.thread = threading.Thread(target=self.pump, name='owned-private-auth-pty', daemon=True)
            self.thread.start()
        except BaseException:
            self.close(); raise
        return {'permissionBound': True, 'agentRegistered': True, 'authenticated': False,
                'maxLeaseSeconds': 90, 'passwordInArgvOrEnvironment': False}

    def pump(self):
        import termios
        data = bytearray(); deadline = time.monotonic() + self.window - 5; selected = False
        user = self.user
        display = user.pw_name if not user.pw_gecos or user.pw_gecos == user.pw_name else user.pw_gecos + ' (' + user.pw_name + ')'
        try:
            while not self.stop.is_set() and time.monotonic() < deadline:
                if not select.select([self.c.pty], [], [], 0.05)[0]: continue
                part = os.read(self.c.pty, 1024); require(part, 'owned authentication PTY closed')
                data += part; require(len(data) <= 16384, 'authentication dialogue bound')
                text = re.sub(rb'\x1b\[[0-9;?]*[a-zA-Z]', b'', bytes(data)).decode('utf-8', 'strict').replace('\r', '')
                require('AUTHENTICATION FAILED' not in text and 'AUTHENTICATION CANCELED' not in text, 'actual authentication rejected')
                require('AUTHENTICATING FOR org.freedesktop.policykit.exec' in text or not ('Password:' in text), 'unexpected polkit action')
                if 'Choose identity to authenticate as' in text and not selected:
                    matches = re.findall(r'^ ([0-9]+)\.  ' + re.escape(display) + r'$', text, re.M)
                    require(len(matches) == 1, 'approved existing identity not uniquely offered')
                    os.write(self.c.pty, matches[0].encode() + b'\n'); selected = True
                identity_ok = selected or ('Authenticating as: ' + display + '\n') in text
                if 'Password:' in text and not self.sent:
                    require(identity_ok, 'password requested for another identity')
                    # pkttyagent prints the prompt just before disabling echo; wait
                    # for the actual terminal state, never sleep-and-assume it.
                    echo_deadline = time.monotonic() + 2
                    while termios.tcgetattr(self.c.pty)[3] & termios.ECHO:
                        require(time.monotonic() < echo_deadline, 'authentication terminal echo remains enabled')
                        time.sleep(0.005)
                    os.write(self.c.pty, bytes(self.secret) + b'\n'); self.sent = True
                if '==== AUTHENTICATION COMPLETE ====' in text:
                    require(self.sent and identity_ok, 'authentication completion lacks owned credential exchange')
                    self.authenticated = True; return
            raise ValueError('authentication request did not complete within lease')
        except BaseException as error:
            self.error = type(error).__name__
            try: self.close()
            except BaseException: self.error = 'CredentialRelockFailed'
        finally:
            for i in range(len(data)): data[i] = 0

    def close(self):
        self.stop.set()
        worker_stopped = True
        if self.thread and self.thread is not threading.current_thread():
            self.thread.join(timeout=3); worker_stopped = not self.thread.is_alive()
        require(self.close_lock.acquire(timeout=15), 'credential closure concurrency unknown')
        try:
            if self.closed: return
            if self.writer and self.writer.poll() is None:
                require(self.writer_fd is not None, 'credential writer pidfd missing')
                signal.pidfd_send_signal(self.writer_fd, signal.SIGKILL)
                self.writer.wait(timeout=5)
            if self.attempted:
                proof = relock_account(self.c.launch, self.c.tun)
                # Recovery happens before this potentially failing ledger write.
                self.c.ledger.mutate(lambda v: (v['authentication'].update(accountMutated=False),
                    v['authentication']['lease'].update(state='closed', recovery=proof)))
            self.c.ops.run([self.c.tun.SYSTEMCTL, 'stop', self.timer + '.timer'])
            for unit in (self.timer + '.timer', self.timer + '.service'):
                require(self.c.ops.run([self.c.tun.SYSTEMCTL, 'show', unit, '--property=ActiveState', '--value']).strip() == 'inactive',
                        'credential watchdog fired or did not close')
            require(worker_stopped, 'private credential worker did not stop')
            self.closed = True
        finally:
            for i in range(len(self.secret)): self.secret[i] = 0
            self.secret.clear()
            if self.writer_fd is not None: os.close(self.writer_fd); self.writer_fd = None
            self.close_lock.release()


class TransactionObserver:
    """Conservative evidence; a missed exec transition never becomes inferred PASS."""
    def __init__(self, controller, kind):
        self.c = controller; self.kind = kind; self.rows = {}; self.pkexec = {}; self.shells = {}
        self.error = None; self.stop = threading.Event(); self.lock = threading.Lock()
        self.thread = threading.Thread(target=self.loop, name='owned-pkexec-observer', daemon=True)

    def start(self):
        self.thread.start()

    def loop(self):
        while not self.stop.wait(0.01):
            try:
                self.scan()
            except Exception as error:
                self.error = type(error).__name__; return

    def scan(self):
        c = self.c; tun = c.tun
        require(tun.same_process(c.gui, c.ops.proc(c.gui['pid'])), 'GUI birth changed during privilege transaction')
        values = {}
        for item in Path('/proc').iterdir():
            if item.name.isdigit() and int(item.name) > 1:
                try:
                    p = c.ops.proc(int(item.name))
                except FileNotFoundError:
                    continue
                if p and p['state'] != 'Z': values[p['pid']] = p
        with self.lock:
            for p in values.values():
                key = birth(p); parent = values.get(p['ppid'])
                direct = parent and tun.same_process(parent, c.gui)
                child = parent and birth(parent) in self.rows and tun.same_process(self.rows[birth(parent)], parent)
                if not direct and not child:
                    continue
                if direct:
                    argv = command_line(p['pid']); uids = uid_tuple(p['pid'])
                    if p['exe'] == str(Path('/usr/bin/pkexec').resolve()):
                        require(argv[:2] == ['/usr/bin/pkexec', '/bin/bash'] and len(argv) == 3,
                                'unexpected GUI pkexec argv')
                        require(uids[0] == c.launch['guiUid'] and uids[1] == 0, 'pkexec UID transition differs')
                        c.verify_script(argv[2], self.kind)
                        require(tun.executable_identity(p['pid']) == c.tools['pkexec'], 'pkexec object changed')
                        self.pkexec[key] = {'identity': p, 'script': argv[2]}
                    elif p['exe'] == str(Path('/bin/bash').resolve()) and uids == (0, 0, 0, 0):
                        require(key in self.pkexec, 'root bash without observed original pkexec birth')
                        require(argv == ['/bin/bash', self.pkexec[key]['script']], 'root bash argv differs')
                        require(tun.executable_identity(p['pid']) == c.tools['bash'], 'root bash object changed')
                        proof = c.verify_script(argv[1], self.kind)
                        require(p['namespaces'] == c.gui['namespaces'] and p['cgroup'] == c.gui['cgroup'],
                                'privilege transaction escaped GUI namespace/cgroup')
                        self.shells[key] = proof
                    else:
                        # Normal ordinary WebKit children remain the GUI supervisor's domain.
                        require(uids[0] == c.launch['guiUid'] and uids[1] == c.launch['guiUid'],
                                'unobserved privileged GUI child')
                        continue
                else:
                    require(uid_tuple(p['pid']) == (0, 0, 0, 0) and p['namespaces'] == c.gui['namespaces'] and
                            p['cgroup'] == c.gui['cgroup'], 'privileged descendant ownership differs')
                self.rows[key] = p
                def record(v):
                    records = v.setdefault('transactionProcesses', [])
                    old = next((r for r in records if birth(r['identity']) == key), None)
                    if old: old['identity'] = p
                    else: records.append({'role': 'privilege-transaction', 'identity': p})
                c.ledger.mutate(record)

    def snapshot(self):
        require(self.error is None, 'privilege transaction observation failed')
        with self.lock:
            return {'kind': self.kind, 'subject': {k: self.c.gui[k] for k in ('pid', 'startTime', 'uid')},
                    'authenticationMode': self.c.launch['authenticationMode'], 'pkexecBirths': [list(k) for k in self.pkexec],
                    'rootShellBirths': [list(k) for k in self.shells],
                    'owned': list(self.rows.values()), 'authorizationObserved': bool(self.shells),
                    'credentialPromptClaimed': False}

    def finish(self):
        self.stop.set(); self.thread.join(timeout=3)
        require(not self.thread.is_alive(), 'transaction observer did not stop')
        result = self.snapshot()
        require(len(self.pkexec) == len(self.shells) == 1, 'real pkexec authorization unavailable or not fully observed')
        for row in self.rows.values():
            now = self.c.ops.proc(row['pid'])
            require(not now or (birth(now) == birth(row) and now['state'] == 'Z'), 'privilege transaction still live or reused')
        result['closed'] = True
        return result


class Controller:
    def __init__(self, launch, tun, directory):
        self.launch = validate_launch(launch); self.tun = tun; self.directory = directory
        self.ops = tun.Commands(); self.seq = 0; self.failed = False; self.finished = False
        self.gui = None; self.ledger = None; self.observer = None; self.fixtures = {}; self.capture_offset = 0
        self.agent = None; self.pty = None; self.tools = {}; self.lease = None; self.auth_observer = None

    def fail(self):
        self.failed = True
        if self.lease:
            try: self.lease.close()
            except Exception: pass  # independent fixed lease watchdog remains armed
        if self.ledger:
            self.ledger.mutate(lambda v: v.update(result='failed'))

    def guard(self):
        require(self.ledger is not None, 'guard not prepared')
        value = self.ledger.load()
        require(time.monotonic() < value['deadlineMonotonic'] - 30, 'reserved watchdog recovery window entered')
        require(value.get('authenticationWatchdogActive') is True, 'independent authentication watchdog missing')
        return value

    def verify_script(self, path, kind):
        path = Path(path)
        require(path.parent == Path(self.launch['helperScriptDir']) and
                re.fullmatch(r'[0-9a-f]{24}-polaris-helper-' + kind + r'\.sh', path.name), 'original product script path differs')
        data, proof = private_bytes(path, self.launch['guiUid'], self.launch['guiHome'], 1024 * 1024)
        require(data.startswith(b'#!/bin/bash'), 'original shell script required')
        return proof

    def bind_gui(self, pid):
        require(self.gui is None and type(pid) is int and pid > 1, 'single actual GUI birth required')
        gui = self.ops.proc(pid); driver = self.ops.proc(self.launch['driver']['pid'])
        require(driver and birth(driver) == (self.launch['driver']['pid'], self.launch['driver']['startTime']) and
                driver['uid'] == self.launch['guiUid'], 'driver birth differs')
        executable = self.tun.executable_identity(pid)
        require(gui and gui['uid'] == self.launch['guiUid'] and gui['capEff'] == gui['capAmb'] == 0 and
                gui['namespaces'] == driver['namespaces'] and
                executable['sha256'] == self.launch['appSha256'] and
                (executable['dev'], executable['inode']) == (gui['exeDev'], gui['exeInode']), 'actual admitted App differs')
        p = gui; chain = []
        for _ in range(32):
            chain.append(birth(p))
            if self.tun.same_process(p, driver): break
            p = self.ops.proc(p['ppid']); require(p, 'GUI ancestor disappeared')
            require(p['uid'] == self.launch['guiUid'], 'GUI ancestor not ordinary UID')
        else: raise ValueError('GUI is not driver-owned')
        matches = []
        for entry in Path('/proc').iterdir():
            if not entry.name.isdigit() or int(entry.name) <= 1: continue
            candidate = self.ops.proc(int(entry.name))
            if not candidate or candidate['state'] == 'Z' or candidate['uid'] != gui['uid']: continue
            if (candidate['exeDev'], candidate['exeInode']) == (gui['exeDev'], gui['exeInode']): matches.append(birth(candidate))
        require(matches == [birth(gui)], 'actual packaged App is not unique in the dedicated VM')
        require(self.tun.same_process(gui, self.ops.proc(pid)), 'GUI changed while binding')
        self.gui = gui
        return {'identity': gui, 'ancestorBirths': chain, 'executable': executable}

    def authentication_readiness(self):
        mode = self.launch['authenticationMode']
        if mode == AUTH_MODE:
            user, status = credential_account(self.launch, self.tun)
            return {'kind': 'exact-dispatch-credential-authority', 'account': user.pw_name,
                    'originalPasswordStatus': status, 'authenticated': False}
        require(mode == 'existing-session', 'observer-only agent cannot authorize mutation')
        tool = Path('/usr/bin/pkcheck'); self.tun.file_identity(tool.resolve())
        subject = str(self.gui['pid']) + ',' + self.gui['startTime'] + ',' + str(self.launch['guiUid'])
        result = subprocess.run([str(tool), '--action-id', 'org.freedesktop.policykit.exec', '--process', subject,
                                 '--detail', 'program', '/bin/bash', '--detail', 'user', 'root'],
                                env=ENV, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=10)
        if result.returncode != 0:
            raise AuthenticationNotReady('actual subject lacks noninteractive authorization')
        return {'kind': 'pkcheck-noninteractive-subject-authorized', 'authenticated': False,
                'agentPresenceOrCredentialPromptClaimed': False}

    def prepare(self):
        require(self.gui and self.ledger is None, 'single preparation after GUI binding required')
        readiness = self.authentication_readiness()
        facts = self.tun.collect_preflight(self.launch['guiUid'], self.launch['vmContext'], True, self.ops)
        check = self.tun.preflight(facts, self.launch, authentication_confirmed=False)
        require(check['blocked'] == ['realPkexecAuthenticationPendingConfirmation'], 'VM/material preflight blocked')
        # Registration/guard preparation is NOT authentication. Network remains closed
        # until the real original product installation authorization is observed.
        self.tools = {name: self.tun.file_identity(Path(path).resolve()) for name, path in
                      (('pkexec', '/usr/bin/pkexec'), ('bash', '/bin/bash'), ('pkttyagent', '/usr/bin/pkttyagent'))}
        self.ledger = self.tun.prepare_root(self.launch['runId'], self.launch['guiUid'],
            {'systemNamespaces': facts['systemNamespaces'], 'network': facts['network']},
            self.launch['helperSha256'], self.launch['coreSha256'], seconds=600, ops=self.ops)
        self.ledger.mutate(lambda v: v.update(controllerGui=self.gui, controllerIdentity=self.tun.process_identity(os.getpid()), authentication={'mode': self.launch['authenticationMode'],
            'authorized': False, 'closed': False, 'accountMutated': False, 'agent': None}, transactionProcesses=[]))
        independent_record(self.directory, 'controller-birth.json', self.tun.process_identity(os.getpid()))
        unit = 'polaris-tun-auth-' + self.launch['runId']
        started = time.monotonic()
        self.ops.run(['/usr/bin/systemd-run', '--unit=' + unit, '--on-active=580s', '--timer-property=AccuracySec=1s',
            '--property=Type=oneshot', '--property=TimeoutStartSec=15s', '/usr/bin/env', '-i', 'PATH=' + ENV['PATH'],
            'LANG=C', '/usr/bin/python3', '-I', str(self.directory / 'root_controller.py'),
            'watch-auth', '--run-id', self.launch['runId']])
        verify_timer(self.ops, self.tun, unit, ['/usr/bin/env', '-i', 'PATH=' + ENV['PATH'], 'LANG=C',
                     '/usr/bin/python3', '-I', str(self.directory / 'root_controller.py'), 'watch-auth',
                     '--run-id', self.launch['runId']], 580, started)
        require(time.monotonic() + 580 < self.ledger.load()['deadlineMonotonic'] - 5, 'auth watchdog preparation consumed reserved margin')
        self.ledger.mutate(lambda v: v.update(authenticationWatchdogActive=True, authenticationTimer=unit + '.timer'))
        return {'guarded': True, 'authentication': 'pending-real-product-authorization', 'preflight': check, 'authenticationReadiness': readiness,
                'controllerDeadlineMarginSeconds': 30}

    def agent_register(self):
        self.guard()
        require(self.launch['authenticationMode'] in ('pkttyagent-observe', AUTH_MODE) and self.agent is None,
                'owned observer agent not requested or already started')
        import pty
        master, slave = pty.openpty(); read_fd, notify_fd = os.pipe()
        user = pwd.getpwuid(self.launch['guiUid'])
        os.fchown(slave, user.pw_uid, user.pw_gid); os.fchmod(slave, 0o600)
        argv = ['/usr/bin/setsid', '--ctty', '/usr/bin/setpriv', '--reuid=' + str(user.pw_uid),
                '--regid=' + str(user.pw_gid), '--clear-groups', '--bounding-set=-all', '--inh-caps=-all', '--ambient-caps=-all',
                '/usr/bin/pkttyagent', '--process', str(self.gui['pid']) + ',' + self.gui['startTime'],
                '--notify-fd', str(notify_fd), '--fallback']
        self.ledger.mutate(lambda v: v['authentication'].update(agentSpawnPending=True, lineageClosed=None))
        try:
            child = subprocess.Popen(argv, env=ENV, stdin=slave, stdout=slave, stderr=slave, pass_fds=(notify_fd,))
            self.agent = child; self.pty = master
            os.close(slave); slave = -1; os.close(notify_fd); notify_fd = -1
            require(select.select([read_fd], [], [], 10)[0] and os.read(read_fd, 1) == b'', 'agent registration not notified')
            row = self.ops.proc(child.pid)
            require(child.poll() is None and row and row['ppid'] == os.getpid() and row['uid'] == user.pw_uid and
                    row['namespaces'] == self.gui['namespaces'] and
                    self.tun.executable_identity(child.pid) == self.tools['pkttyagent'], 'owned real pkttyagent not running')
            self.ledger.mutate(lambda v: v['authentication'].update(agent={'identity': row}, agentSpawnPending=False,
                registered=True, subject={'pid': self.gui['pid'], 'startTime': self.gui['startTime']}))
            return {'registered': True, 'authenticated': False, 'credentialInputImplemented': False,
                    'status': 'REGISTERED_NOT_AUTHENTICATED'}
        except Exception:
            # A broken ledger must not skip native child reaping. This direct
            # child has received no credential; no arbitrary PID is accepted.
            try:
                if self.agent:
                    self.agent.kill(); self.agent.wait(timeout=5)
            finally:
                self.fail()
            raise
        finally:
            for fd in (read_fd, notify_fd, slave):
                if fd >= 0: os.close(fd)

    def transaction_begin(self, kind):
        value = self.guard()
        require(kind in ('install', 'uninstall') and self.observer is None, 'single fixed transaction required')
        require(not value['authentication']['closed'], 'authentication window closed')
        require((kind == 'install' and not value.get('helper')) or
                (kind == 'uninstall' and value.get('helper') and value.get('capture')), 'transaction order differs')
        if self.launch['authenticationMode'] == AUTH_MODE:
            self.agent_register()
            independent_record(self.directory, 'agent-' + kind + '.json',
                               self.ledger.load()['authentication']['agent']['identity'])
            self.auth_observer = AuthObserver(self, kind); self.auth_observer.start()
            self.lease = CredentialLease(self, kind); self.lease.open()
        self.observer = TransactionObserver(self, kind); self.observer.start()
        return {'observing': kind}

    def transaction_end(self):
        require(self.observer is not None, 'no active transaction')
        result = self.observer.finish(); self.observer = None
        if self.lease:
            self.lease.close()
            require(self.lease.authenticated and self.lease.error is None, 'real credential authentication not proved')
            result['credentialAuthenticationComplete'] = True
            closed_auth = self.stop_agent(close_window=False)
            result['credentialRecovery'] = self.ledger.load()['authentication']['lease']['recovery']
            result['authDescendantsReaped'] = closed_auth['authDescendantsReaped']
            self.lease = None
        self.ledger.mutate(lambda v: (v.setdefault('transactions', []).append(result),
                                     v['authentication'].update(authorized=True)))
        return result

    def network_start(self):
        value = self.guard()
        require(value.get('helper') and value['authentication']['authorized'] and not self.fixtures,
                'actual authorized product helper required before fixture network')
        hashes = {n: self.launch['codeHashes'][n] for n in ('tun.py', 'tun_fixtures.py', 'fixtures.py')}
        self.tun.freeze_fixture_code(self.ledger, self.directory, hashes)
        self.tun.provision_network(self.ledger)
        for role in ('O', 'A', 'B'):
            self.fixtures[role] = self.tun.spawn_fixture(self.ledger, role)
        return {'topology': self.tun.topology(self.launch['runId']), 'rolesReady': ['O', 'A', 'B']}

    def fixture(self, role, operation, seconds=None):
        self.guard()
        require(role in self.fixtures and operation in ('snapshot', 'latency', 'stop'), 'fixed fixture command required')
        command = {'action': operation}
        if operation == 'latency':
            require(role in ('A', 'B') and type(seconds) in (int, float) and 0 <= seconds <= 2, 'bounded proxy latency required')
            command['seconds'] = seconds
        child = self.fixtures[role]
        require(child.poll() is None, 'fixture exited early')
        child.stdin.write(json.dumps(command).encode() + b'\n'); child.stdin.flush()
        result = json.loads(read_line(child.stdout, maximum=1024 * 1024))
        if operation == 'stop':
            require(result.get('closed') is True and child.wait(timeout=5) == 0, 'fixture did not close normally')
        return result

    def register_core(self):
        value = self.guard(); require(value.get('helper'), 'owned helper required')
        rows = []
        for item in Path('/proc').iterdir():
            if item.name.isdigit() and int(item.name) > 1:
                row = self.ops.proc(int(item.name))
                if row and row['ppid'] == value['helper']['identity']['pid'] and row['state'] != 'Z': rows.append(row)
        require(len(rows) == 1, 'single actual helper core child required')
        return {'identity': self.tun.register_core(self.ledger, rows[0]['pid'])}

    def core_attest(self):
        value = self.guard(); rows = [r for r in value['processes'] if r['role'] == 'core']
        live = [r for r in rows if self.tun.same_process(r['identity'], self.ops.proc(r['identity']['pid']))]
        require(len(live) == 1, 'one owned current core required')
        row = live[0]; pid = row['identity']['pid']; argv = command_line(pid)
        require(argv[-3:] == ['run', '-c', self.launch['runtimePath']], 'actual core runtime path differs')
        require(self.tun.executable_identity(pid) == row['executable'], 'owned core executable changed')
        raw, proof = private_bytes(self.launch['runtimePath'], self.launch['guiUid'], self.launch['guiHome'])
        config = json.loads(raw)
        apis = [s for s in config.get('services', []) if s.get('type') == 'api']
        require(len(apis) == 1, 'single actual API service required')
        require(self.tun.same_process(row['identity'], self.ops.proc(pid)), 'core changed during runtime attestation')
        # No secret, original JSON, FD content or arbitrary path is returned.
        return {'identity': row['identity'], 'executable': {**row['executable'], 'path': row['identity']['exe']}, 'runtime': proof,
                'readUnixNs': str(time.time_ns()), 'rpcContractClaimed': False,
                'apiEnabled': True, 'dashboardEnabled': apis[0].get('dashboard', {}).get('enabled') is True}

    def core_stopped(self):
        value = self.guard(); rows = [r for r in value['processes'] if r['role'] == 'core']
        require(rows, 'no registered core')
        for row in rows:
            now = self.ops.proc(row['identity']['pid'])
            require(not now or (self.tun.same_process(row['identity'], now) and now['state'] == 'Z'),
                    'owned core still live or PID reused')
        return {'stopped': True, 'identity': rows[-1]['identity']}

    def network_observe(self):
        self.guard()
        return {'network': self.tun.network_snapshot(self.ops), 'specialRoute':
                self.ops.json([self.tun.IP, '-j', 'route', 'get', self.tun.topology(self.launch['runId'])['special']])}

    def rpc(self, method, transaction_id=None, incarnation=None):
        require(method in ('capabilities', 'status'), 'only two read-only native RPC methods')
        before = self.core_attest(); identity = before['identity']
        raw, proof = private_bytes(self.launch['runtimePath'], self.launch['guiUid'], self.launch['guiHome'])
        config = json.loads(raw); apis = [s for s in config.get('services', []) if s.get('type') == 'api']
        require(len(apis) == 1 and apis[0].get('listen') == '127.0.0.1', 'actual loopback API required')
        api = apis[0]; require(type(api.get('listen_port')) is int and 0 < api['listen_port'] < 65536,
                              'actual API port required')
        secret = api.get('secret')
        require(type(secret) is str and 16 <= len(secret) <= 512 and all(0x21 <= ord(v) <= 0x7e for v in secret),
                'actual private API secret required')
        tuns = [i for i in config.get('inbounds', []) if i.get('type') == 'tun']
        require(len(tuns) == 1 and tuns[0].get('interface_name') == 'polaris-tun0', 'actual production TUN required')
        request = {}
        if method == 'status':
            require(type(transaction_id) is str and 0 < len(transaction_id) <= 256 and
                    type(incarnation) is str and 0 < len(incarnation) <= 256, 'bounded actual transaction identity required')
            request = {'transactionId': transaction_id, 'runtimeIncarnation': incarnation}
        payload = {'action': 'tun-' + method, 'host': '127.0.0.1', 'port': api['listen_port'], 'secret': secret,
                   'owned': {'pid': identity['pid'], 'startTimeTicks': int(identity['startTime']),
                             'exe': identity['exe'], 'runtimeConfig': self.launch['runtimePath'],
                             'guiUid': self.launch['guiUid'], 'exeDev': identity['exeDev'], 'exeInode': identity['exeInode']},
                   'request': request}
        probe = self.directory / 'rpc-probe'
        require(self.tun.file_identity(probe)['sha256'] == self.launch['rpcProbeSha256'], 'frozen test probe changed')
        result = subprocess.run([str(probe)], input=json.dumps(payload).encode(), env=ENV,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=25)
        require(result.returncode == 0 and 0 < len(result.stdout) <= 1024 * 1024, 'read-only native probe failed')
        response = json.loads(result.stdout)
        require(response.get('schema') == 'polaris-package-rpc-probe-v1' and response.get('action') == 'tun-' + method
                and response.get('ok') is True, 'native read-only probe response differs')
        after = self.core_attest()
        require(self.tun.same_process(identity, after['identity']) and proof['sha256'] == after['runtime']['sha256'],
                'core/runtime changed during read-only RPC')
        # Probe output uses fixed redacted schemas; additionally reject literal secret leakage.
        require(secret.encode() not in result.stdout, 'probe output unexpectedly contains API secret')
        return response

    def normal_cleanup(self):
        require(self.ledger.load()['authentication']['closed'], 'authentication must close before network cleanup')
        close_transactions(self.ledger, self.tun, signal_live=False)
        result = self.tun.cleanup(self.ledger, forced=False); value = self.ledger.load()
        identities = [r['identity'] for r in value['processes']] + [r['identity'] for r in value.get('transactionProcesses', [])]
        reaped = all(proc_absent(i['pid']) for i in identities)
        require(reaped, 'registered privileged processes remain live or reused')
        result.update(networkRestored=value.get('networkRestored') is True,
                      productUninstalled=value.get('productUninstalled') is True,
                      privilegedProcessesReaped=reaped, authWindowClosed=value['authentication']['closed'])
        return result

    def capture_read(self, offset):
        value = self.guard(); proof = value.get('capture')
        require(proof and type(offset) is int and offset == self.capture_offset, 'capture not stopped or offset replay')
        path = self.ledger.directory / 'tun.pcap'
        require(self.tun.file_identity(path) == proof, 'captured bytes changed')
        size = path.stat().st_size
        require(24 <= size <= MAX_CAPTURE and 0 <= offset <= size, 'capture byte bound')
        with path.open('rb') as stream: stream.seek(offset); data = stream.read(32768)
        self.capture_offset += len(data)
        return {'offset': offset, 'nextOffset': self.capture_offset, 'eof': self.capture_offset == size,
                'bytes': size, 'sha256': proof['sha256'], 'base64': base64.b64encode(data).decode()}

    def stop_agent(self, close_window=True):
        lineage = self.auth_observer.close() if self.auth_observer else None
        if lineage:
            self.ledger.mutate(lambda v: v['authentication'].update(lineageClosed=lineage))
        self.auth_observer = None
        result = close_auth(self.ledger, self.tun, forced=False, close_window=close_window)
        if self.agent:
            require(self.agent.wait(timeout=5) == -signal.SIGTERM, 'owned agent shutdown differs')
        if self.pty is not None: os.close(self.pty); self.pty = None
        self.agent = None
        if lineage:
            result['authLineage'] = lineage
            for pid, start in lineage['ownedBirths']:
                require(proc_absent(pid), 'authentication descendant not actually reaped or PID reused')
            result['authDescendantsReaped'] = True
        return result

    def auth_close(self):
        require(self.observer is None, 'privilege transaction not closed')
        if self.lease: self.lease.close()
        return self.stop_agent()

    def finish(self):
        value = self.guard()
        require(not self.failed and value['phase'] == 'closed' and value['authentication']['closed'] and
                not value.get('forcedCleanup') and not value.get('unknown'), 'normal proven closure required')
        require(value.get('controllerGui') and proc_absent(value['controllerGui']['pid']), 'original GUI has not exited or PID reused')
        self.tun.cancel_watchdog(self.ledger)
        timer = value['authenticationTimer']
        self.ops.run([self.tun.SYSTEMCTL, 'stop', timer])
        for unit in (timer, timer.replace('.timer', '.service')):
            require(self.ops.run([self.tun.SYSTEMCTL, 'show', unit, '--property=ActiveState', '--value']).strip() == 'inactive',
                    'authentication watchdog fired or failed to cancel')
        self.finished = True
        return {'state': 'normal', 'authenticationClosed': True, 'watchdogsCancelled': True, 'watchdogCancelled': True, 'authWindowClosed': True,
                'accountMutated': False, 'publicationEligible': False}

    def dispatch(self, message):
        require(type(message) is dict and type(message.get('seq')) is int and message['seq'] == self.seq + 1,
                'request sequence/replay differs')
        seq = message['seq']; self.seq = seq; action = message.get('action')
        fields = {
            'bind_gui': ('pid',), 'prepare': (), 'agent_register': (), 'transaction_begin': ('kind',),
            'transaction_scan': (), 'transaction_end': (), 'register_helper': (), 'network_start': (),
            'fixture': ('role', 'operation'), 'register_core': (), 'core_attest': (), 'core_stopped': (), 'network_observe': (), 'rpc': ('method',), 'tun_register': (),
            'capture_start': (), 'capture_stop': (), 'capture_read': ('offset',), 'product_uninstalled': (),
            'auth_close': (), 'cleanup': (), 'finish': (), 'abort': ()}
        require(action in fields, 'unknown fixed action')
        expected = ('seq', 'action') + fields[action]
        if action == 'fixture' and message.get('operation') == 'latency': expected += ('seconds',)
        if action == 'rpc' and message.get('method') == 'status': expected += ('transactionId', 'runtimeIncarnation')
        exact(message, expected)
        require(not self.finished, 'controller already closed')
        require(not self.failed or action in ('abort', 'auth_close'), 'earlier failure is terminal')
        if action not in ('bind_gui', 'prepare', 'abort'): self.guard()
        if self.observer and action not in ('transaction_scan', 'transaction_end', 'abort'):
            raise ValueError('privilege transaction still in progress')
        if action == 'bind_gui': result = self.bind_gui(message['pid'])
        elif action == 'prepare': result = self.prepare()
        elif action == 'agent_register': result = self.agent_register()
        elif action == 'transaction_begin': result = self.transaction_begin(message['kind'])
        elif action == 'transaction_scan':
            require(self.observer, 'no transaction'); result = self.observer.snapshot()
        elif action == 'transaction_end': result = self.transaction_end()
        elif action == 'register_helper':
            value = self.guard()
            require(value['authentication']['authorized'] and value.get('transactions', [])[-1]['kind'] == 'install',
                    'real install authorization missing')
            result = {'identity': self.tun.register_helper(self.ledger), 'executable': self.ledger.load()['helper']['binary']}
        elif action == 'network_start': result = self.network_start()
        elif action == 'fixture': result = self.fixture(message['role'], message['operation'], message.get('seconds'))
        elif action == 'register_core': result = self.register_core()
        elif action == 'core_attest': result = self.core_attest()
        elif action == 'core_stopped': result = self.core_stopped()
        elif action == 'network_observe': result = self.network_observe()
        elif action == 'rpc': result = self.rpc(message['method'], message.get('transactionId'), message.get('runtimeIncarnation'))
        elif action == 'tun_register': self.tun.register_tun_interface(self.ledger); result = {'registered': True}
        elif action == 'capture_start': self.tun.start_capture(self.ledger); result = {'started': True}
        elif action == 'capture_stop': result = self.tun.stop_capture(self.ledger)
        elif action == 'capture_read': result = self.capture_read(message['offset'])
        elif action == 'product_uninstalled':
            require(self.ledger.load().get('transactions', [])[-1]['kind'] == 'uninstall', 'real uninstall transaction missing')
            self.tun.confirm_product_uninstalled(self.ledger); result = {'productUninstalled': True}
        elif action == 'auth_close': result = self.auth_close()
        elif action == 'cleanup': result = self.normal_cleanup()
        elif action == 'finish': result = self.finish()
        else: result = self.abort()
        return {'seq': seq, 'ok': True, 'result': result}

    def abort(self):
        self.fail()
        if self.observer:
            self.observer.stop.set(); self.observer.thread.join(timeout=3); self.observer = None
        if not self.ledger: return {'state': 'failed', 'prepared': False}
        # Auth recovery is independent and attempted even if product cleanup raises.
        errors = []
        if self.auth_observer:
            try: self.auth_observer.close(forced=True)
            except Exception: errors.append('authentication-subtree-cleanup-unknown')
            self.auth_observer = None
        try: close_auth(self.ledger, self.tun, forced=True)
        except Exception: errors.append('authentication-cleanup-unknown')
        try: close_transactions(self.ledger, self.tun)
        except Exception: errors.append('privilege-transaction-cleanup-unknown')
        try: result = self.tun.cleanup(self.ledger, forced=True)
        except Exception: result = {'state': 'failed'}; errors.append('product-cleanup-unknown')
        self.ledger.mutate(lambda v: v.update(unknown=sorted(set(v['unknown'] + errors)), result='failed'))
        return {'state': 'failed', 'forced': True, 'unknown': errors, 'productCleanup': result}


def close_transactions(ledger, tun, signal_live=True):
    errors = []
    for row in reversed(ledger.load().get('transactionProcesses', [])):
        old = row['identity']; now = ledger.ops.proc(old['pid'])
        if not now or now['state'] == 'Z': continue
        if not tun.same_process(old, now): errors.append('transaction-birth-or-exec-changed'); continue
        require(signal_live, 'privilege transaction child still live at normal close')
        if not ledger.ops.signal_owned(old, signal.SIGTERM):
            ledger.ops.signal_owned(old, signal.SIGKILL)
            errors.append('transaction-required-force-kill')
    if errors:
        ledger.mutate(lambda v: v.update(unknown=sorted(set(v['unknown'] + errors)), result='failed'))
    require(not errors, 'transaction cleanup unknown')


def close_auth(ledger, tun, forced, close_window=True):
    value = ledger.load(); auth = value.get('authentication', {})
    require(auth.get('accountMutated') is False, 'unsupported credential mutation requires VM disposal')
    errors = []
    if auth.get('agentSpawnPending'): errors.append('authentication-agent-spawn-unknown')
    if auth.get('agent'):
        old = auth['agent']['identity']; now = ledger.ops.proc(old['pid'])
        if now and now['state'] != 'Z':
            require(tun.same_process(old, now), 'authentication agent birth changed')
            if not ledger.ops.signal_owned(old, signal.SIGTERM):
                ledger.ops.signal_owned(old, signal.SIGKILL); errors.append('authentication-agent-required-force-kill')
        elif not auth.get('closed') and not auth.get('lineageClosed'):
            errors.append('authentication-agent-exited-early')
    def save(v):
        v['authentication'].update(closed=(not errors and close_window), accountMutated=False)
        if not errors and v['authentication'].get('agent'):
            v['authentication'].setdefault('agentHistory', []).append(v['authentication']['agent'])
            v['authentication'].update(agent=None, registered=False)
        if forced or errors: v.update(result='failed', forcedCleanup=v['forcedCleanup'] or forced)
        v['unknown'] = sorted(set(v['unknown'] + errors))
    ledger.mutate(save)
    require(not errors, 'authentication cleanup unknown')
    return {'closed': True, 'accountMutated': False, 'ownedAgentStopped': bool(auth.get('agent')),
            'existingSessionAgentModified': False, 'forced': forced}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('serve', 'watch-auth', 'watch-lease'))
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--transaction', choices=('install', 'uninstall'))
    args = parser.parse_args()
    launch, tun, directory = load_frozen(args.run_id, recovery=args.command != 'serve')
    if args.command in ('watch-lease', 'watch-auth'):
        require((args.command == 'watch-lease') == bool(args.transaction), 'watchdog transaction binding differs')
        watchdog_recover(launch, tun, directory, args.transaction)
        raise SystemExit(1)  # timeout/recovery always fails acceptance
    import ctypes
    require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0, 'process-local child subreaper required')
    controller = Controller(launch, tun, directory)
    print(json.dumps({'schema': SCHEMA, 'ready': True, 'runId': args.run_id,
                      'credentialMutationImplemented': True, 'credentialAuthorityPresent': launch['authorization'] is not None}), flush=True)
    try:
        while not controller.finished:
            # Input timeout is bounded by the reserved watchdog margin as well.
            raw = read_line(sys.stdin.buffer, timeout=570)
            try:
                response = controller.dispatch(json.loads(raw))
            except Exception as error:
                controller.fail()
                not_ready = isinstance(error, AuthenticationNotReady) and controller.ledger is None
                response = {'seq': controller.seq, 'ok': False, 'terminal': True,
                            'error': {'code': 'AUTHENTICATION_NOT_READY' if not_ready else 'CONTROLLER_FAILED',
                                      'kind': type(error).__name__,
                                      'message': 'fixed operation failed; no acceptance or authentication success'}}
                if not_ready:
                    response['error'].update(mutated=False, mutationScope='product-network-account')
            print(json.dumps(response, separators=(',', ':')), flush=True)
    finally:
        if not controller.finished: controller.abort()


if __name__ == '__main__':
    main()
