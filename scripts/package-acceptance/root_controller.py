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
BOOTSTRAP_STAGES = ('envelope', 'sources', 'staging', 'probe', 'exec',
                    'frozen-load', 'subreaper', 'controller-init')
CONTROLLER_STAGES = ('idle', 'bind-gui', 'authentication-readiness', 'vm-preflight',
                     'tool-identities', 'root-ledger', 'controller-ledger',
                     'auth-watchdog-start', 'auth-watchdog-verify', 'prepared', 'dispatch')
BOOTSTRAP = r'''import os,sys,json,base64,hashlib,re,stat
from pathlib import Path
stage='envelope'; run_id=None; d=None; dfd=None; directory_identity=None; created={}
def cleanup_staging():
 if d is None: return 'not-created'
 if dfd is None: return 'unconfirmed'
 ok=True
 for name,identity in created.items():
  try:
   s=os.stat(name,dir_fd=dfd,follow_symlinks=False)
   assert stat.S_ISREG(s.st_mode) and s.st_uid==0 and s.st_nlink==1 and (s.st_dev,s.st_ino)==identity
   os.unlink(name,dir_fd=dfd)
  except Exception: ok=False
 try:
  s=d.lstat()
  assert stat.S_ISDIR(s.st_mode) and s.st_uid==0 and (s.st_dev,s.st_ino)==directory_identity
  if ok: d.rmdir()
 except Exception: ok=False
 return 'removed' if ok else 'unconfirmed'
try:
 assert os.geteuid()==0 and sys.flags.isolated
 raw=sys.stdin.buffer.readline(2*1024*1024+1)
 assert raw.endswith(b'\n') and len(raw)<=2*1024*1024
 v=json.loads(raw); assert set(v)=={'launch','files'}
 l=v['launch']; r=l['runId']; assert re.fullmatch('[0-9a-f]{12}',r); run_id=r
 stage='sources'
 names={'root_controller.py','tun.py','tun_fixtures.py','fixtures.py'}
 assert set(v['files'])==set(l['codeHashes'])==names
 contents={}
 for n in names:
  b=base64.b64decode(v['files'][n],validate=True)
  assert 0<len(b)<=1024*1024 and hashlib.sha256(b).hexdigest()==l['codeHashes'][n]
  contents[n]=b
 stage='staging'
 base=Path('/run/polaris-tun-controller')
 if not base.exists(): base.mkdir(mode=0o700)
 for p in [base,*base.parents]:
  s=p.lstat(); assert stat.S_ISDIR(s.st_mode) and s.st_uid==0 and not s.st_mode&0o022
 assert not base.stat().st_mode&0o077
 candidate=base/r; candidate.mkdir(mode=0o700)
 d=candidate
 dfd=os.open(d,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
 s=os.fstat(dfd); directory_identity=(s.st_dev,s.st_ino)
 contents['launch.json']=json.dumps(l,sort_keys=True,separators=(',',':')).encode()
 for n,b in contents.items():
  fd=os.open(n,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=dfd)
  with os.fdopen(fd,'wb') as f:
   s=os.fstat(f.fileno()); created[n]=(s.st_dev,s.st_ino)
   f.write(b); f.flush(); os.fsync(f.fileno())
 stage='probe'
 p=Path(l['rpcProbePath']); fd=os.open(p,os.O_RDONLY|os.O_NOFOLLOW)
 with os.fdopen(fd,'rb') as f:
  s=os.fstat(f.fileno()); assert stat.S_ISREG(s.st_mode) and s.st_uid==l['guiUid'] and s.st_nlink==1 and 0<s.st_size<=128*1024*1024
  b=f.read(); assert hashlib.sha256(b).hexdigest()==l['rpcProbeSha256']
 fd=os.open('rpc-probe',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o500,dir_fd=dfd)
 with os.fdopen(fd,'wb') as f:
  s=os.fstat(f.fileno()); created['rpc-probe']=(s.st_dev,s.st_ino)
  f.write(b); f.flush(); os.fsync(f.fileno())
 stage='exec'
 os.execve('/usr/bin/python3',['/usr/bin/python3','-I',str(d/'root_controller.py'),'serve','--run-id',r],{'PATH':'/usr/sbin:/usr/bin:/sbin:/bin','LANG':'C','LC_ALL':'C'})
except BaseException:
 cleanup=cleanup_staging()
 print(json.dumps({'schema':'polaris-tun-controller-v1','ready':False,'runId':run_id,
  'startupFailure':{'stage':stage,'code':'BOOTSTRAP_FAILED','stagingCleanup':cleanup}}),flush=True)
 raise SystemExit(70 if cleanup in ('not-created','removed') else 71)
finally:
 if dfd is not None: os.close(dfd)
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
    require(value['authenticationMode'] in ('existing-session', 'pkttyagent-observe', *CREDENTIAL_MODES), 'unknown authentication mode')
    if value['authenticationMode'] in CREDENTIAL_MODES: validate_permission(value, recovery=recovery)
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
AUTH_GROUP_MODE = 'temporary-existing-runner-password-and-sudo-group'
AUTH_GROUP_PHRASE = 'AUTHORIZE TEMPORARY RUNNER PASSWORD AND SUDO GROUP MEMBERSHIP WITH RELOCK AND REMOVAL FOR THIS RUN'
CREDENTIAL_MODES = (AUTH_MODE, AUTH_GROUP_MODE)
GROUP_ACTIVITY_SECONDS = 20
GROUP_RECOVERY_RESERVE = 70
GROUP_RECOVERY_SERVICE_SECONDS = 70
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


AUTH_GUARDS = ('permission-schema', 'permission-mode', 'permission-binding', 'permission-actions',
    'permission-window', 'permission-time', 'account-unique', 'account-login',
    'account-groups-enumerate', 'account-groups-records', 'account-groups-required',
    'tool-passwd', 'tool-chpasswd', 'tool-pkttyagent', 'tool-setsid', 'tool-setpriv',
    'status-command', 'status-format', 'status-existing-password', 'complete')


def auth_observation():
    return {'schema': 'polaris-auth-readiness-observation-v1', 'guard': 'permission-schema',
            'code': 'AUTH_PERMISSION_SCHEMA', 'passwordStatus': 'unknown', 'completed': False, 'groups': None,
            'scope': 'read-only-check-observation-not-account-restoration'}


def auth_mark(observation, guard):
    if observation is not None:
        require(guard in AUTH_GUARDS, 'fixed authentication guard required')
        observation.update(guard=guard, code='AUTH_' + guard.upper().replace('-', '_'))


def validate_auth_observation(value):
    exact(value, ('schema', 'guard', 'code', 'passwordStatus', 'completed', 'scope', 'groups'))
    if value['groups'] is not None: validate_group_observation(value['groups'])
    require(value['schema'] == 'polaris-auth-readiness-observation-v1' and value['guard'] in AUTH_GUARDS and
            value['code'] == 'AUTH_' + value['guard'].upper().replace('-', '_') and
            value['passwordStatus'] in ('L', 'NP', 'P', 'unknown') and type(value['completed']) is bool and
            value['completed'] == (value['guard'] == 'complete') and
            value['scope'] == 'read-only-check-observation-not-account-restoration', 'authentication observation differs')
    require(not value['completed'] or value['passwordStatus'] in ('L', 'NP'), 'usable existing password is never admitted')
    return value


GROUP_NAMES = ('sudo', 'admin')
GROUP_ERRORS = ('none', 'OSError', 'KeyError', 'TypeError', 'ValueError', 'other')


def observe_existing_admin_groups(user):
    """Observe the original primary/explicit-membership gate; no authorization."""
    value = {'schema': 'polaris-existing-admin-groups-v1', 'guard': 'account-groups-enumerate',
             'lookupState': 'unreached', 'errorType': 'none', 'groupRequirementSatisfied': None,
             'groups': {name: {'exists': None, 'primaryMatch': None, 'explicitSupplementaryMatch': None}
                        for name in GROUP_NAMES}}
    try:
        import grp
        rows = grp.getgrall()
        value['guard'] = 'account-groups-records'
        require(type(rows) in (list, tuple), 'group enumeration shape differs')
        groups = {name: {'exists': False, 'primaryMatch': False, 'explicitSupplementaryMatch': False}
                  for name in GROUP_NAMES}
        for group in rows:
            require(type(group.gr_name) is str and type(group.gr_gid) is int and
                    type(group.gr_mem) in (list, tuple) and all(type(name) is str for name in group.gr_mem),
                    'group record shape differs')
            if group.gr_name in groups:
                record = groups[group.gr_name]
                record['exists'] = True
                record['primaryMatch'] |= group.gr_gid == user.pw_gid
                record['explicitSupplementaryMatch'] |= user.pw_name in group.gr_mem
        value.update(lookupState='observed', guard='account-groups-required', groups=groups,
                     groupRequirementSatisfied=any(row['primaryMatch'] or row['explicitSupplementaryMatch']
                                                   for row in groups.values()))
    except Exception as error:
        value.update(lookupState='failed', errorType=next((name for kind, name in
            ((OSError, 'OSError'), (KeyError, 'KeyError'), (TypeError, 'TypeError'), (ValueError, 'ValueError'))
            if isinstance(error, kind)), 'other'))
    return value


def validate_group_observation(value):
    exact(value, ('schema', 'guard', 'lookupState', 'errorType', 'groupRequirementSatisfied', 'groups'))
    require(value['schema'] == 'polaris-existing-admin-groups-v1' and
            value['guard'] in ('account-groups-enumerate', 'account-groups-records', 'account-groups-required') and
            value['lookupState'] in ('unreached', 'observed', 'failed') and value['errorType'] in GROUP_ERRORS,
            'fixed group observation required')
    exact(value['groups'], GROUP_NAMES)
    for row in value['groups'].values():
        exact(row, ('exists', 'primaryMatch', 'explicitSupplementaryMatch'))
        require(all(v is None or type(v) is bool for v in row.values()), 'finite group flags required')
    if value['lookupState'] == 'observed':
        require(value['guard'] == 'account-groups-required' and value['errorType'] == 'none' and
                type(value['groupRequirementSatisfied']) is bool and
                all(type(v) is bool for row in value['groups'].values() for v in row.values()) and
                all(row['exists'] or not (row['primaryMatch'] or row['explicitSupplementaryMatch'])
                    for row in value['groups'].values()) and
                value['groupRequirementSatisfied'] == any(row['primaryMatch'] or row['explicitSupplementaryMatch']
                                                         for row in value['groups'].values()),
                'group result differs from observation')
    else:
        require(value['guard'] != 'account-groups-required' and value['groupRequirementSatisfied'] is None and
                all(v is None for row in value['groups'].values() for v in row.values()) and
                ((value['lookupState'] == 'failed') == (value['errorType'] != 'none')),
                'incomplete group observation cannot pass')
    return value


AUTH_OPERATION_STAGES = ('idle', 'transaction-guard', 'agent-spawn', 'agent-notify', 'agent-identity', 'agent-record',
    'lineage-start', 'lease-account', 'lease-budget', 'group-intent', 'lease-timer-start',
    'lease-timer-verify', 'group-add', 'setting-intent', 'password-writer', 'password-readback',
    'dialogue', 'dialogue-identity', 'dialogue-password', 'dialogue-complete', 'transaction-observer',
    'transaction-finish', 'controller-input')
TRANSACTION_BOUNDARIES = ('gui', 'inventory', 'candidate', 'pkexec-argv', 'pkexec-uid', 'script',
    'pkexec-executable', 'shell-prior', 'shell-argv', 'shell-executable', 'namespace', 'ordinary-uid',
    'descendant-uid', 'ledger', 'credential-fence')
AUTH_FAILURE_CODES = ('operation-failed', 'abort-requested', 'input-ended', 'recovery-failed') + tuple(
    'transaction-' + key + '-unconfirmed' for key in TRANSACTION_BOUNDARIES)
AUTH_RECOVERY_STEPS = ('agent', 'credential', 'group', 'lineage', 'timer')


AUTH_PUMP_STAGES = ('not-started', 'waiting', 'bytes-observed', 'action-checked', 'identity-checked',
    'echo-wait', 'pkexec-fence', 'credential-write', 'completion-observed', 'stopped')
AUTH_PUMP_CODES = ('pty-read-failed', 'pty-eof', 'dialogue-limit', 'dialogue-encoding',
    'action-mismatch', 'identity-not-unique', 'identity-mismatch', 'identity-write-failed',
    'echo-unconfirmed', 'pkexec-fence-unconfirmed', 'credential-write-failed', 'native-authentication-failed',
    'native-authentication-canceled', 'completion-unconfirmed', 'activity-deadline', 'unexpected-error')
AUTH_PUMP_FLAGS = ('ptyBytes', 'expectedAction', 'expectedIdentity', 'echoDisabled',
                   'credentialWriteCompleted', 'authenticationComplete')

def auth_execution_observation():
    return {'pump': {'stage': 'not-started', 'firstFailure': None,
                    'observed': {key: False for key in AUTH_PUMP_FLAGS}, 'stoppedBy': 'none'},
            'agent': {'notifyClosed': False, 'subjectBound': False, 'registrationLive': False,
                      'latestLive': None, 'exitCode': None, 'preCleanupObserved': False,
                      'preCleanupLive': None, 'preCleanupExitCode': None}}

def validate_auth_execution(value):
    exact(value, ('pump', 'agent'))
    pump = value['pump']; exact(pump, ('stage', 'firstFailure', 'observed', 'stoppedBy'))
    require(pump['stage'] in AUTH_PUMP_STAGES and pump['stoppedBy'] in
            ('none', 'controller-cleanup', 'worker-failure', 'deadline'), 'fixed pump observation required')
    exact(pump['observed'], AUTH_PUMP_FLAGS)
    require(all(type(v) is bool for v in pump['observed'].values()), 'fixed pump flags required')
    flags = pump['observed']
    require(not flags['authenticationComplete'] or flags['credentialWriteCompleted'], 'completion without input')
    require(not flags['credentialWriteCompleted'] or (flags['echoDisabled'] and flags['expectedIdentity'] and flags['expectedAction']), 'input without guarded dialogue')
    require(not any(flags[k] for k in AUTH_PUMP_FLAGS[1:]) or flags['ptyBytes'], 'dialogue without PTY observation')
    if pump['firstFailure'] is not None:
        exact(pump['firstFailure'], ('stage', 'code'))
        require(pump['firstFailure']['stage'] in AUTH_PUMP_STAGES and
                pump['firstFailure']['code'] in AUTH_PUMP_CODES, 'fixed pump failure required')
    agent = value['agent']; exact(agent, ('notifyClosed', 'subjectBound', 'registrationLive', 'latestLive', 'exitCode',
                                       'preCleanupObserved', 'preCleanupLive', 'preCleanupExitCode'))
    require(all(type(agent[k]) is bool for k in ('notifyClosed', 'subjectBound', 'registrationLive')) and
            (agent['latestLive'] is None or type(agent['latestLive']) is bool) and
            (agent['exitCode'] is None or type(agent['exitCode']) is int and -128 <= agent['exitCode'] <= 255), 'fixed agent observation required')
    require(type(agent['preCleanupObserved']) is bool and
            (agent['preCleanupLive'] is None or type(agent['preCleanupLive']) is bool) and
            (agent['preCleanupExitCode'] is None or type(agent['preCleanupExitCode']) is int and -128 <= agent['preCleanupExitCode'] <= 255), 'fixed pre-cleanup agent observation required')
    require(agent['preCleanupObserved'] or (agent['preCleanupLive'] is None and agent['preCleanupExitCode'] is None), 'unobserved pre-cleanup state')
    require(agent['preCleanupExitCode'] is None or agent['preCleanupLive'] is False, 'pre-cleanup exit/live mismatch')
    require(not agent['registrationLive'] or (agent['notifyClosed'] and agent['subjectBound']), 'agent registration incomplete')
    require(agent['exitCode'] is None or agent['latestLive'] is False, 'agent exit/live mismatch')
    return value


def auth_runtime_observation(mode):
    return {'schema': 'polaris-auth-runtime-observation-v2', 'mode': mode, 'kind': None,
            'lastReached': 'idle', 'firstFailure': None,
            'recovery': {key: 'not-reached' for key in AUTH_RECOVERY_STEPS},
            'credentialRecovery': None, 'groupRecovery': None, 'persistence': 'not-attempted',
            'execution': auth_execution_observation()}


def validate_auth_runtime_observation(value):
    exact(value, ('schema', 'mode', 'kind', 'lastReached', 'firstFailure', 'recovery',
                  'credentialRecovery', 'groupRecovery', 'persistence', 'execution'))
    require(value['schema'] == 'polaris-auth-runtime-observation-v2' and
            value['mode'] in ('existing-session', 'pkttyagent-observe', *CREDENTIAL_MODES) and
            value['kind'] in (None, 'install', 'uninstall') and
            value['lastReached'] in AUTH_OPERATION_STAGES and
            value['persistence'] in ('not-attempted', 'recorded', 'unknown'), 'fixed auth observation required')
    validate_auth_execution(value['execution'])
    if value['mode'] not in CREDENTIAL_MODES or value['kind'] is None:
        require(value['execution']['pump'] == auth_execution_observation()['pump'], 'credential dialogue outside authority')
    failure = value['firstFailure']
    if failure is not None:
        exact(failure, ('stage', 'code'))
        require(failure['stage'] in AUTH_OPERATION_STAGES and failure['code'] in AUTH_FAILURE_CODES,
                'fixed first authentication failure required')
    exact(value['recovery'], AUTH_RECOVERY_STEPS)
    require(all(state in ('not-reached', 'attempted', 'completed', 'unknown')
                for state in value['recovery'].values()), 'fixed recovery state required')
    if value['mode'] not in CREDENTIAL_MODES:
        require(value['recovery']['credential'] == 'not-reached', 'credential recovery outside authority')
    if value['kind'] is None:
        require(all(v == 'not-reached' for v in value['recovery'].values()), 'recovery without transaction')
    proof = value['credentialRecovery']
    if proof is not None:
        exact(proof, ('account', 'guiUid', 'finalPasswordStatus', 'emptyPasswordRestored',
                      'plaintextPersisted', 'temporaryPasswordHash'))
        require(value['mode'] in CREDENTIAL_MODES and value['kind'] is not None and
                type(proof['account']) is str and re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', proof['account']) and
                type(proof['guiUid']) is int and proof['guiUid'] > 0 and proof['finalPasswordStatus'] == 'L' and
                proof['emptyPasswordRestored'] is False and proof['plaintextPersisted'] is False and
                proof['temporaryPasswordHash'] == 'locked until disposable VM destruction', 'fixed credential recovery proof required')
    require((proof is not None) == (value['recovery']['credential'] == 'completed'), 'credential proof/state differs')
    group = value['groupRecovery']
    if group is not None:
        exact(group, ('schema', 'account', 'guiUid', 'group', 'groupGid', 'originalMembership',
                      'finalMembership', 'removedOnlyAddedMembership', 'writerStopped', 'maxLeaseSeconds'))
        require(value['mode'] == AUTH_GROUP_MODE and value['kind'] is not None and
                group['schema'] == 'polaris-sudo-membership-recovery-v1' and group['account'] == 'runner' and
                type(group['guiUid']) is int and group['guiUid'] == 1001 and group['group'] == 'sudo' and
                type(group['groupGid']) is int and group['groupGid'] > 0 and group['originalMembership'] == 'absent' and
                group['finalMembership'] == 'absent' and group['removedOnlyAddedMembership'] is True and
                group['writerStopped'] is True and type(group['maxLeaseSeconds']) is int and group['maxLeaseSeconds'] == 90,
                'fixed group recovery proof required')
    require((group is not None) == (value['recovery']['group'] == 'completed'), 'group proof/state differs')
    if value['mode'] != AUTH_GROUP_MODE:
        require(value['recovery']['group'] == 'not-reached' and group is None, 'group recovery outside authority')
    elif proof is not None:
        require(proof['account'] == 'runner' and proof['guiUid'] == 1001, 'joint credential subject differs')
    return value


ABORT_UNKNOWN_CODES = frozenset(('root-preparation-incomplete', 'root-ledger-failure-unconfirmed', 'authentication-lease-recovery-unknown',
    'authentication-recovery-record-unknown',
    'root-ledger-recovery-unconfirmed', 'authentication-subtree-cleanup-unknown', 'authentication-cleanup-unknown',
    'privilege-transaction-cleanup-unknown', 'product-cleanup-unknown', 'owned-secret-controller-stop-unproved',
    'credential-setting-intent-unknown', 'setting-writer-not-yet-recorded', 'credential-writer-stop-unproved',
    'authentication-lineage-stop-unproved', 'credential-relock-unproved', 'product-ledger-recovery-incomplete',
    'group-membership-recovery-unproved', 'group-intent-unproved', 'group-writer-stop-unproved',
    'joint-lease-deadline-exceeded', 'joint-closed-unproved',
    'transaction-birth-or-exec-changed', 'transaction-required-force-kill', 'authentication-agent-spawn-unknown',
    'authentication-agent-required-force-kill', 'authentication-agent-exited-early', 'earlier-failure-is-permanent',
    'helper-emergency-stop-unknown', 'unregistered-product-installation', 'owned-process-cleanup-unknown',
    'registered-process-still-live', 'native-child-spawn-outcome-unknown', 'partial-network-command-unknown',
    'product-network-recovery-unproved', 'network-restoration-unproved', 'route-removal-unknown'))


def validate_abort_receipt(value, vm_validator=None):
    """Parse failure evidence only; an empty unknown list grants no authority."""
    fields = ('schema', 'state', 'forced', 'prepared', 'actualReadOnlyAuth', 'unknown')
    require(type(value) is dict, 'typed abort receipt required')
    exact(value, fields + (('productCleanup',) if 'productCleanup' in value else ()) +
          (('vmPreflight',) if 'vmPreflight' in value else ()) +
          (('authenticationRuntime',) if 'authenticationRuntime' in value else ()))
    if 'authenticationRuntime' in value: validate_auth_runtime_observation(value['authenticationRuntime'])
    if 'vmPreflight' in value:
        require(callable(vm_validator), 'VM observation validator required')
        vm_validator(value['vmPreflight'])
    require(value['schema'] == 'polaris-root-abort-v1' and value['state'] == 'failed' and value['forced'] is True and
            (value['prepared'] is None or type(value['prepared']) is bool), 'abort failure scope differs')
    if value['actualReadOnlyAuth'] is not None: validate_auth_observation(value['actualReadOnlyAuth'])
    def codes(rows):
        require(type(rows) is list and len(rows) <= 128, 'bounded abort unknown codes required')
        for code in rows:
            require(type(code) is str and len(code) <= 96 and (code in ABORT_UNKNOWN_CODES or
                re.fullmatch(r'(namespace-identity-or-live-members|namespace-removal-unknown):pt[0-9a-f]{8}n', code) or
                re.fullmatch(r'link-removal-unknown:pt[0-9a-f]{8}v0', code)), 'fixed abort unknown code required')
    codes(value['unknown'])
    require(('productCleanup' in value) == (value['prepared'] is True), 'abort preparation/outcome differs')
    if value['prepared'] is None:
        require('root-preparation-incomplete' in value['unknown'], 'partial preparation cannot claim clean closure')
    if 'productCleanup' in value:
        product = value['productCleanup']
        require(type(product) is dict, 'typed product cleanup required')
        if set(product) == {'state'}:
            require(product['state'] == 'failed' and 'product-cleanup-unknown' in value['unknown'], 'missing cleanup evidence')
        else:
            exact(product, ('state', 'forced', 'unknown', 'publicationEligible'))
            require(product['state'] in ('forced', 'failed') and product['forced'] is True and
                    product['publicationEligible'] is False, 'abort cannot claim normal cleanup')
            codes(product['unknown'])
    return value


def validate_permission(launch, now=None, recovery=False, observation=None):
    """Frozen real-event admission is authority, NEVER pkexec authentication proof."""
    auth_mark(observation, 'permission-schema')
    p = launch.get('authorization')
    grouped = launch['authenticationMode'] == AUTH_GROUP_MODE
    exact(p, ('schema', 'phrase', 'authMode', 'execution', 'runToken', 'guiUid', 'account',
              'transactions', 'notBeforeUnix', 'expiresUnix', 'maxLeaseSeconds') + (('groupLease',) if grouped else ()))
    auth_mark(observation, 'permission-mode')
    require(p['schema'] == ('polaris-tun-credential-group-lease-v1' if grouped else 'polaris-tun-credential-lease-v1') and
            p['phrase'] == (AUTH_GROUP_PHRASE if grouped else AUTH_PHRASE) and
            p['authMode'] == launch['authenticationMode'] and p['authMode'] in CREDENTIAL_MODES,
            'specific credential authorization absent')
    if grouped:
        require(p['account'] == 'runner' and p['guiUid'] == 1001 and
                p['groupLease'] == {'group': 'sudo', 'account': 'runner', 'guiUid': 1001,
                    'maxLeaseSeconds': 90, 'originalMembership': 'absent', 'restore': 'remove-only-added-membership'},
                'specific bounded existing-group authority absent')
    auth_mark(observation, 'permission-binding')
    require(p['execution'] == launch['execution'] and launch['execution']['githubRunAttempt'] == '1' and p['runToken'] == launch['runId'] and
            p['guiUid'] == launch['guiUid'] and type(p['guiUid']) is int, 'credential execution/UID binding differs')
    auth_mark(observation, 'permission-actions')
    require(re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', p['account']) and p['transactions'] == ['install', 'uninstall'] and
            p['maxLeaseSeconds'] == 90, 'fixed existing-account actions required')
    auth_mark(observation, 'permission-window')
    require(type(p['notBeforeUnix']) is int and type(p['expiresUnix']) is int and
            0 < p['expiresUnix'] - p['notBeforeUnix'] <= 900, 'bounded credential authority required')
    if not recovery:
        auth_mark(observation, 'permission-time')
        now = int(time.time()) if now is None else now
        require(p['notBeforeUnix'] <= now and now + p['maxLeaseSeconds'] <= p['expiresUnix'],
                'credential permission not yet valid or expired')
    return p


def account_status(name, observation=None):
    auth_mark(observation, 'status-command')
    result = subprocess.run(['/usr/bin/passwd', '-S', name], env=ENV, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=True)
    auth_mark(observation, 'status-format')
    require(len(result.stdout) <= 4096, 'bounded account status required')
    words = result.stdout.decode('ascii', 'strict').split()
    require(len(words) == 7 and words[0] == name and words[1] in ('L', 'NP', 'P'), 'account status ambiguous')
    return words[1]


def credential_account(launch, tun, recovery=False, observation=None, *, group_pending=False):
    p = validate_permission(launch, recovery=recovery, observation=observation)
    require(not group_pending or (not recovery and launch['authenticationMode'] == AUTH_GROUP_MODE), 'group preparation authority absent')
    auth_mark(observation, 'account-unique')
    users = [u for u in pwd.getpwall() if u.pw_uid == p['guiUid']]
    require(len(users) == 1 and users[0].pw_name == p['account'] and pwd.getpwnam(p['account']).pw_uid == p['guiUid'],
            'unique existing approved account required')
    user = users[0]
    if not recovery:
        auth_mark(observation, 'account-login')
        require(user.pw_shell in ('/bin/bash', '/usr/bin/bash') and user.pw_dir.startswith('/home/'), 'existing runner login account required')
    if not recovery:
        groups = validate_group_observation(observe_existing_admin_groups(user))
        if observation is not None: observation['groups'] = groups
        auth_mark(observation, groups['guard'])
        if group_pending:
            group_binding(launch, expected_member=False)
        else:
            require(groups['lookupState'] == 'observed' and groups['groupRequirementSatisfied'] is True,
                    'existing administrator group required; no group changes allowed')
    # Recovery already deliberately ignores membership changes. Do not make
    # exact-account re-lock depend on an unused NSS group enumeration.
    for path in ('/usr/bin/passwd', '/usr/sbin/chpasswd', '/usr/bin/pkttyagent', '/usr/bin/setsid', '/usr/bin/setpriv'):
        auth_mark(observation, 'tool-' + Path(path).name)
        tun.file_identity(Path(path).resolve())
    status = account_status(user.pw_name, observation=observation)
    if observation is not None: observation['passwordStatus'] = status
    auth_mark(observation, 'status-existing-password')
    if not recovery: require(status in ('L', 'NP'), 'existing usable password must never be overwritten')
    if observation is not None:
        auth_mark(observation, 'complete'); observation['completed'] = True
    return user, status


def group_binding(launch, *, expected_member=None, original=None):
    """Public NSS identity only; never reads shadow/gshadow or changes groups."""
    import grp
    validate_permission(launch, recovery=True)
    require(launch['authenticationMode'] == AUTH_GROUP_MODE, 'group lease not authorized')
    user = pwd.getpwnam('runner'); rows = [u for u in pwd.getpwall() if u.pw_uid == launch['guiUid'] or u.pw_name == 'runner']
    group = grp.getgrnam('sudo')
    require(len(rows) == 1 and user.pw_uid == launch['guiUid'] == 1001 and rows[0].pw_name == 'runner' and
            group.gr_name == 'sudo' and type(group.gr_gid) is int and group.gr_gid > 0 and
            user.pw_gid != group.gr_gid and type(group.gr_mem) in (list, tuple) and
            all(type(n) is str for n in group.gr_mem) and len(group.gr_mem) == len(set(group.gr_mem)),
            'exact existing account/group identity required')
    value = {'account': 'runner', 'guiUid': user.pw_uid, 'primaryGid': user.pw_gid,
             'group': 'sudo', 'groupGid': group.gr_gid}
    if original is not None: require(value == original, 'account/group identity changed')
    member = 'runner' in group.gr_mem
    if expected_member is not None: require(member is expected_member, 'group membership differs')
    return value, member


def group_intent(directory, kind, launch, binding, deadline):
    require(kind in ('install', 'uninstall') and time.monotonic() < deadline <= time.monotonic() + 90,
            'bounded joint lease deadline required')
    value = {'schema': 'polaris-sudo-membership-intent-v1', 'runId': launch['runId'], 'kind': kind,
             'binding': binding, 'originalMembership': 'absent', 'deadlineMonotonic': deadline,
             'permissionSha256': sha(json.dumps(launch['authorization'], sort_keys=True).encode())}
    independent_record(directory, 'group-intent-' + kind + '.json', value)
    return value


def read_group_intent(directory, kind, launch, tun):
    path = directory / ('group-intent-' + kind + '.json')
    if not path.exists(): return None
    tun.root_path(path); require(path.stat().st_size <= 16384, 'group intent bound')
    value = json.loads(path.read_bytes())
    exact(value, ('schema', 'runId', 'kind', 'binding', 'originalMembership', 'deadlineMonotonic', 'permissionSha256'))
    require(value['schema'] == 'polaris-sudo-membership-intent-v1' and value['runId'] == launch['runId'] and
            value['kind'] == kind and value['originalMembership'] == 'absent' and
            type(value['deadlineMonotonic']) in (int, float) and 0 < value['deadlineMonotonic'] < float('inf') and
            value['permissionSha256'] == sha(json.dumps(launch['authorization'], sort_keys=True).encode()), 'group intent authority differs')
    group_binding(launch, original=value['binding'])
    return value


def group_writer_path(directory, kind, operation):
    require(kind in ('install', 'uninstall') and operation in ('add', 'remove'), 'fixed group operation required')
    return directory / ('group-writer-' + kind + '-' + operation + '.json')


def stop_group_writer(launch, tun, directory, kind, operation, ops, *, immediate=False):
    path = group_writer_path(directory, kind, operation)
    if not path.exists(): return False
    tun.root_path(path); require(path.stat().st_size <= 16384, 'group writer record bound')
    value = json.loads(path.read_bytes())
    exact(value, ('runId', 'kind', 'operation', 'identity', 'python', 'gpasswd'))
    require(value['runId'] == launch['runId'] and value['kind'] == kind and value['operation'] == operation,
            'group writer transaction differs')
    old = value['identity']; now = ops.proc(old['pid'])
    if now and now['state'] != 'Z':
        require(birth(now) == birth(old) and now['uid'] == old['uid'] == 0 and
                now['namespaces'] == old['namespaces'] and now['cgroup'] == old['cgroup'] and
                tun.executable_identity(now['pid']) in (value['python'], value['gpasswd']) and
                value['python'] == tun.file_identity(Path('/usr/bin/python3').resolve()) and
                value['gpasswd'] == tun.file_identity(Path('/usr/bin/gpasswd').resolve()),
                'fixed group writer birth/exec changed')
        stop_recorded(ops, tun, now, **({'immediate': True} if immediate else {}))  # unchanged exact identity gate
    else: stop_recorded(ops, tun, old, **({'immediate': True} if immediate else {}))
    return True


def run_group_writer(launch, tun, directory, kind, operation, ops):
    intent = read_group_intent(directory, kind, launch, tun)
    require(intent is not None, 'durable group intent required')
    child = subprocess.Popen(['/usr/bin/python3', '-I', str(directory / 'root_controller.py'), 'group-write',
        '--run-id', launch['runId'], '--transaction', kind, '--group-operation', operation], env=ENV,
        stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        row = ops.proc(child.pid)
        python = tun.file_identity(Path('/usr/bin/python3').resolve())
        gpasswd = tun.file_identity(Path('/usr/bin/gpasswd').resolve())
        require(row and row['uid'] == 0 and row['ppid'] == os.getpid() and tun.executable_identity(child.pid) == python,
                'gated fixed group writer birth required')
        independent_record(directory, group_writer_path(directory, kind, operation).name,
            {'runId': launch['runId'], 'kind': kind, 'operation': operation, 'identity': row, 'python': python, 'gpasswd': gpasswd})
        child.communicate(input=b'GO\n', timeout=10)
        require(child.returncode == 0, 'fixed group writer failed')
    except BaseException:
        if child.stdin: child.stdin.close()  # before GO, EOF cannot mutate
        if child.poll() is None:
            # A missing record never permits mutation (no GO was sent).
            # Retain unknown rather than signal an unrecorded process.
            stop_group_writer(launch, tun, directory, kind, operation, ops)
        child.wait(timeout=5)
        raise
    group_binding(launch, original=intent['binding'], expected_member=operation == 'add')
    return True


def group_writer_main(launch, tun, directory, kind, operation):
    intent = read_group_intent(directory, kind, launch, tun)
    require(intent is not None, 'group intent absent')
    require(read_line(sys.stdin.buffer, timeout=10, maximum=3) == b'GO\n', 'group writer gate closed')
    path = group_writer_path(directory, kind, operation); tun.root_path(path)
    value = json.loads(path.read_bytes()); current = tun.process_identity(os.getpid())
    require(value['runId'] == launch['runId'] and value['kind'] == kind and value['operation'] == operation and
            tun.same_process(value['identity'], current) and
            tun.executable_identity(os.getpid()) == value['python'] and
            tun.file_identity(Path('/usr/bin/gpasswd').resolve()) == value['gpasswd'], 'group writer gate identity differs')
    group_binding(launch, original=intent['binding'], expected_member=operation == 'remove')
    if operation == 'add':
        validate_permission(launch)
        require(time.monotonic() < intent['deadlineMonotonic'] - GROUP_RECOVERY_RESERVE, 'joint lease deadline exhausted')
    # No subprocess/grandchild: the recorded gated birth becomes only this exact executable.
    os.execve('/usr/bin/gpasswd', ['/usr/bin/gpasswd', '-a' if operation == 'add' else '-d', 'runner', 'sudo'], ENV)


def recover_group_membership(launch, tun, directory, kind, ops):
    intent = read_group_intent(directory, kind, launch, tun)
    require(intent is not None, 'group recovery intent absent')
    require(stop_group_writer(launch, tun, directory, kind, 'add', ops), 'group writer outcome unrecorded')
    previous_remove = stop_group_writer(launch, tun, directory, kind, 'remove', ops)
    _, member = group_binding(launch, original=intent['binding'])
    require(not (member and previous_remove), 'previous removal failed; no unrecorded retry')
    if member:
        run_group_writer(launch, tun, directory, kind, 'remove', ops)
        require(stop_group_writer(launch, tun, directory, kind, 'remove', ops), 'removal writer outcome unrecorded')
    group_binding(launch, original=intent['binding'], expected_member=False)
    return {'schema': 'polaris-sudo-membership-recovery-v1', 'account': 'runner', 'guiUid': launch['guiUid'],
            'group': 'sudo', 'groupGid': intent['binding']['groupGid'], 'originalMembership': 'absent',
            'finalMembership': 'absent', 'removedOnlyAddedMembership': True, 'writerStopped': True, 'maxLeaseSeconds': 90}


def write_group_closed(directory, kind, launch, binding, deadline, credential, group):
    now = time.monotonic()
    require(now <= deadline and credential and credential.get('finalPasswordStatus') == 'L' and
            group and group.get('finalMembership') == 'absent', 'joint lease deadline or restoration unproved')
    value = {'schema': 'polaris-joint-lease-closed-v1', 'runId': launch['runId'], 'kind': kind,
             'permissionSha256': sha(json.dumps(launch['authorization'], sort_keys=True).encode()),
             'binding': binding, 'deadlineMonotonic': deadline, 'closedMonotonic': now,
             'credentialRecovery': credential, 'groupRecovery': group}
    independent_record(directory, 'group-closed-' + kind + '.json', value)
    require(time.monotonic() <= deadline, 'joint closure persistence exceeded deadline')
    return value


def read_group_closed(directory, kind, launch, tun):
    path = directory / ('group-closed-' + kind + '.json')
    if not path.exists(): return None
    tun.root_path(path); require(path.stat().st_size <= 16384, 'joint closed receipt bound')
    value = json.loads(path.read_bytes())
    exact(value, ('schema', 'runId', 'kind', 'permissionSha256', 'binding', 'deadlineMonotonic',
                  'closedMonotonic', 'credentialRecovery', 'groupRecovery'))
    intent = read_group_intent(directory, kind, launch, tun)
    require(intent is not None and value['schema'] == 'polaris-joint-lease-closed-v1' and
            value['runId'] == launch['runId'] and value['kind'] == kind and value['binding'] == intent['binding'] and
            value['permissionSha256'] == intent['permissionSha256'] and value['deadlineMonotonic'] == intent['deadlineMonotonic'] and
            type(value['closedMonotonic']) in (int, float) and 0 < value['closedMonotonic'] <= value['deadlineMonotonic'] and
            value['credentialRecovery'] == {'account': 'runner', 'guiUid': 1001, 'finalPasswordStatus': 'L',
                'emptyPasswordRestored': False, 'plaintextPersisted': False,
                'temporaryPasswordHash': 'locked until disposable VM destruction'} and
            value['groupRecovery'] == {'schema': 'polaris-sudo-membership-recovery-v1', 'account': 'runner', 'guiUid': 1001,
                'group': 'sudo', 'groupGid': intent['binding']['groupGid'], 'originalMembership': 'absent',
                'finalMembership': 'absent', 'removedOnlyAddedMembership': True, 'writerStopped': True, 'maxLeaseSeconds': 90},
            'joint closed receipt identity/phase differs')
    # Fresh account/group identity and old writer/agent process absence, not a
    # mutable ledger flag. A later active lease may currently own the membership;
    # its final recovery (or the all-closed path) must separately prove absence.
    group_binding(launch, original=intent['binding'])
    for name in ('writer-' + kind + '.json', 'agent-' + kind + '.json',
                 'group-writer-' + kind + '-add.json', 'group-writer-' + kind + '-remove.json'):
        row_path = directory / name; tun.root_path(row_path)
        require(row_path.stat().st_size <= 16384, 'closed writer proof bound')
        row = json.loads(row_path.read_bytes()); old = row['identity'] if name.startswith('group-') else row
        require(proc_absent(old['pid']), 'closed transaction process still exists or reused')
    return value


def relock_account(launch, tun):
    """Independent recovery does not need a readable mutable product ledger."""
    if launch['authenticationMode'] == AUTH_GROUP_MODE:
        p = validate_permission(launch, recovery=True)
        rows = [u for u in pwd.getpwall() if u.pw_uid == p['guiUid'] or u.pw_name == p['account']]
        require(len(rows) == 1 and rows[0].pw_uid == p['guiUid'] and rows[0].pw_name == p['account'] and
                pwd.getpwnam(p['account']).pw_uid == p['guiUid'], 'unique approved recovery account required')
        user = rows[0]; tun.file_identity(Path('/usr/bin/passwd').resolve())
    else: user, _ = credential_account(launch, tun, recovery=True)
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


def verify_timer(ops, tun, unit, argv, seconds, started, *, recovery_seconds=None):
    props = dict(line.split('=', 1) for line in ops.run([tun.SYSTEMCTL, 'show', unit + '.service',
        '--property=ExecStart,User,Group,DynamicUser,FragmentPath,DropInPaths,ActiveState' +
        (',TimeoutStartUSec' if recovery_seconds is not None else '')]).splitlines() if '=' in line)
    require(props.get('User') in ('', 'root', '0') and props.get('Group') in ('', 'root', '0') and
            props.get('DynamicUser') == 'no' and not props.get('DropInPaths') and props.get('ActiveState') == 'inactive' and
            props.get('FragmentPath') == '/run/systemd/transient/' + unit + '.service', 'watchdog root transient service differs')
    require('path=' + argv[0] + ' ;' in props.get('ExecStart', '') and
            'argv[]=' + ' '.join(argv) + ' ;' in props.get('ExecStart', ''), 'watchdog fixed command differs')
    if recovery_seconds is not None:
        require(duration_seconds(props.get('TimeoutStartUSec', '')) == recovery_seconds, 'watchdog recovery service budget differs')
    timer = dict(line.split('=', 1) for line in ops.run([tun.SYSTEMCTL, 'show', unit + '.timer',
        '--property=ActiveState,Unit,TimersMonotonic']).splitlines() if '=' in line)
    require(timer.get('ActiveState') == 'active' and timer.get('Unit') == unit + '.service', 'watchdog target/activation differs')
    triggers = re.findall(r'OnActiveUSec=([^;]+)\s*;\s*next_elapse=([^;}]+)', timer.get('TimersMonotonic', ''))
    require(len(triggers) == 1 and duration_seconds(triggers[0][0]) == seconds, 'watchdog interval differs')
    target = duration_seconds(triggers[0][1])
    require(started + seconds - 2 <= target <= time.monotonic() + seconds + 2, 'watchdog actual deadline differs')
    return {'unit': unit, 'deadlineMonotonic': target, 'rootFixedCommandVerified': True}


def independent_record(directory, name, value):
    require(re.fullmatch(r'(controller-birth|agent-(install|uninstall)|writer-(install|uninstall)|setting-intent-(install|uninstall)|group-intent-(install|uninstall)|group-closed-(install|uninstall)|group-writer-(install|uninstall)-(add|remove))\.json', name), 'fixed recovery identity filename required')
    fd = os.open(directory / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(value, f, sort_keys=True); f.flush(); os.fsync(f.fileno())
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def watchdog_recover(launch, tun, directory, transaction=None):
    """Stop recorded writers/secret processes BEFORE final lock; no intent, no lock."""
    errors = []; recovery = None; ops = tun.Commands()
    grouped = launch['authenticationMode'] == AUTH_GROUP_MODE
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
        stop_recorded(ops, tun, controller, **({'immediate': True} if grouped else {})); producer_stopped = True
    except Exception: errors.append('owned-secret-controller-stop-unproved')
    if grouped:
        active = []
        for kind in kinds:
            try:
                if read_group_closed(directory, kind, launch, tun) is None:
                    if read_group_intent(directory, kind, launch, tun) is not None: active.append(kind)
            except Exception:
                errors.append('joint-closed-unproved'); active.append(kind)
        if len(active) > 1: errors.append('joint-closed-unproved')
        kinds = active
        if not kinds:
            try:
                group_binding(launch, expected_member=False)
                require(account_status('runner') == 'L', 'closed account no longer locked')
            except Exception: errors.append('joint-closed-unproved')
    intents = []
    if launch['authenticationMode'] in CREDENTIAL_MODES:
        for kind in kinds:
            try:
                if has_setting_intent(directory, kind, launch, tun): intents.append(kind)
            except Exception: errors.append('credential-setting-intent-unknown')
    # Even if producer termination is unknown, attempt owned writer stop and
    # lock but never claim restoration; this timeout is permanently failed.
    for kind in kinds:
        try:
            writer = read_identity('writer-' + kind + '.json')
            if writer: stop_recorded(ops, tun, writer, **({'immediate': True} if grouped else {}))
            elif kind in intents: errors.append('setting-writer-not-yet-recorded')
        except Exception: errors.append('credential-writer-stop-unproved')
    def close_lineage():
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
    if not grouped: close_lineage()
    group_intents = []; group_recoveries = []
    if launch['authenticationMode'] == AUTH_GROUP_MODE:
        for kind in kinds:
            try: stop_group_writer(launch, tun, directory, kind, 'add', ops, immediate=True)
            except Exception: errors.append('group-writer-stop-unproved')
            try:
                if read_group_intent(directory, kind, launch, tun) is not None: group_intents.append(kind)
            except Exception: errors.append('group-intent-unproved')
    if grouped:
        for kind in kinds:
            try:
                agent = read_identity('agent-' + kind + '.json')
                if agent: stop_recorded(ops, tun, agent, immediate=True)
            except Exception: errors.append('authentication-lineage-stop-unproved')
    # Unknown setting result still requires lock; a final L is not a stability
    # claim if any writer/lineage stop above is unproved. Such a run always FAILs.
    if intents:
        try: recovery = relock_account(launch, tun)
        except Exception: errors.append('credential-relock-unproved')
    for kind in group_intents:
        try: group_recoveries.append(recover_group_membership(launch, tun, directory, kind, ops))
        except Exception: errors.append('group-membership-recovery-unproved')
    if grouped:
        for kind in group_intents:
            try:
                intent = read_group_intent(directory, kind, launch, tun)
                require(time.monotonic() <= intent['deadlineMonotonic'], 'joint deadline exceeded')
            except Exception: errors.append('joint-lease-deadline-exceeded')
        checkpoint = directory / ('emergency-' + (transaction or 'auth') + '-account.json')
        fd = os.open(checkpoint, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'w') as f:
            json.dump({'result': 'failed', 'restorationProven': False, 'credentialRecovery': recovery,
                       'groupRecoveries': group_recoveries, 'errors': sorted(set(errors))}, f)
            f.flush(); os.fsync(f.fileno())
        close_lineage()  # full auth/root cleanup still required, never inferred from group removal
    try: ledger = tun.Ledger(launch['runId'])
    except Exception:
        if launch['authenticationMode'] != AUTH_GROUP_MODE: raise
        ledger = None; errors.append('product-ledger-recovery-incomplete')
    try:
        require(ledger is not None, 'product ledger unavailable')
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
                   'restorationProven': False, 'errors': sorted(set(errors)),
                   **({'groupRecoveries': group_recoveries} if launch['authenticationMode'] == AUTH_GROUP_MODE else {})}, f)
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


def stop_recorded(ops, tun, old, *, immediate=False):
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
    if immediate:
        require(ops.signal_owned(old, signal.SIGKILL), 'registered process failed emergency fence')
        return True
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
        self.parent_stop_requested = threading.Event(); self.parent_stop_ack = threading.Event()
        agent = c.ledger.load()['authentication']['agent']['identity']; self.rows[birth(agent)] = agent
        self.parent_identity = agent
        self.thread = threading.Thread(target=self.loop, name='owned-auth-lineage', daemon=True)

    def start(self):
        try: self.scan(); self.thread.start()
        except Exception:
            self.error = 'initial-observation-incomplete'
            raise

    def loop(self):
        while not self.stop.wait(0.005):
            final_requested = self.parent_stop_requested.is_set()
            try:
                if final_requested: self.require_live_parent()
                self.scan()
                if final_requested: self.require_live_parent()
            except Exception as e:
                self.error = self.error or type(e).__name__
                if self.parent_stop_requested.is_set(): self.parent_stop_ack.set()
                return
            if final_requested:
                self.parent_stop_ack.set(); return

    def require_live_parent(self):
        current = self.c.ops.proc(self.parent_identity['pid'])
        require(current and current['state'] != 'Z' and
                self.c.tun.same_process(self.parent_identity, current),
                'pre-stop registered authentication parent unavailable')

    def scan(self):
        values = {}
        for entry in Path('/proc').iterdir():
            if entry.name.isdigit() and int(entry.name) > 1:
                p = proc_stat(int(entry.name))
                if p and p['state'] != 'Z': values[p['pid']] = p
        with self.lock:
            for _ in range(8):
                changed = False
                ordered = sorted(values.values(), key=lambda row: 0 if birth(row) in self.rows else 1)
                for light in ordered:
                    key = birth(light); parent = values.get(light['ppid'])
                    if key not in self.rows and not (parent and birth(parent) in self.rows): continue
                    p = self.c.ops.proc(light['pid'])
                    require(p and birth(p) == key and p['ppid'] == light['ppid'], 'auth candidate topology changed')
                    if key not in self.rows:
                        require(self.c.tun.same_process(self.rows[birth(parent)], self.c.ops.proc(parent['pid'])), 'auth parent changed')
                    if key == birth(self.parent_identity):
                        require(self.c.tun.same_process(self.parent_identity, p), 'registered authentication parent changed')
                    require(p['uid'] in (0, self.c.launch['guiUid']) and p['namespaces'] == self.c.gui['namespaces'] and
                            p['cgroup'] == self.rows[next(iter(self.rows))]['cgroup'], 'auth helper escaped native lineage')
                    if self.rows.get(key) != p:
                        self.rows[key] = p; changed = True
                if not changed: break
            write_recovery_snapshot(self.c.directory, 'auth-lineage-' + self.kind + '.json',
                {'runId': self.c.launch['runId'], 'kind': self.kind, 'processes': list(self.rows.values()), 'observationError': self.error})

    def prepare_parent_stop(self, deadline=None):
        """Bounded ack from the existing observer; no procfs/fsync on this thread.

        A late snapshot only updates auth-lineage evidence, never final ledger,
        closed or recovery receipts. Timeout remains permanently unknown even
        if the observer later finishes. This is not a fork barrier.
        """
        try:
            require(self.thread.ident is not None and self.error is None, 'authentication observer unavailable')
            self.parent_stop_requested.set()
            remaining = 3 if deadline is None else max(0, min(3, deadline - time.monotonic()))
            require(self.parent_stop_ack.wait(timeout=remaining), 'pre-stop lineage snapshot timed out')
            require(self.error is None, 'pre-stop lineage snapshot failed')
        except Exception:
            self.error = self.error or 'pre-stop-observation-incomplete'
            raise
        finally:
            self.stop.set()

    def close(self, forced=False):
        self.stop.set()
        if self.thread.ident is not None: self.thread.join(timeout=3)
        require(not self.thread.is_alive(), 'authentication lineage observer stuck')
        try: self.scan()
        except Exception: self.error = 'closure-observation-incomplete'
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
        require(self.error is None, 'authentication lineage observation incomplete')
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
        self.grouped = controller.launch['authenticationMode'] == AUTH_GROUP_MODE
        self.group_intent_written = False; self.group_timer_verified = False; self.group_recovery = None
        self.group_deadline = None; self.credential_recovery = None

    def open(self):
        try: return self._open()
        except BaseException:
            self.c.auth_note('auth_failed')
            if self.grouped:
                try: self.close()
                except BaseException: pass  # original failure retained; fixed cleanup unknown is recorded
            raise

    def _open(self):
        c = self.c; c.auth_reached('lease-account', self.kind); p = validate_permission(c.launch)
        self.user, original = credential_account(c.launch, c.tun, **({'group_pending': True} if self.grouped else {}))
        require(c.agent and c.ledger.load()['authentication'].get('registered') is True, 'real agent must register first')
        c.auth_reached('lease-budget')
        window = min(p['maxLeaseSeconds'], int(c.ledger.load()['deadlineMonotonic'] - time.monotonic()) - 35)
        require(window >= (90 if self.grouped else 15), 'not enough guarded time for another credential lease')
        self.window = window
        timer_seconds = GROUP_ACTIVITY_SECONDS if self.grouped else window
        if self.grouped and self.kind == 'uninstall':
            require(read_group_closed(c.directory, 'install', c.launch, c.tun) is not None, 'install joint lease not durably closed')
        state = {'kind': self.kind, 'account': self.user.pw_name, 'guiUid': self.user.pw_uid,
                 'originalPasswordStatus': original, 'state': 'guard-pending',
                 'deadlineMonotonic': time.monotonic() + window, 'timer': self.timer + '.timer'}
        c.ledger.mutate(lambda v: v['authentication'].update(lease=state))
        started = time.monotonic()
        if self.grouped:
            require(getattr(c, 'group_binding', None) is not None, 'initial group identity not bound')
            binding, _ = group_binding(c.launch, expected_member=False, original=c.group_binding)
            self.group_deadline = started + window
            c.auth_reached('group-intent')
            group_intent(c.directory, self.kind, c.launch, binding, self.group_deadline)
            self.group_intent_written = True
        argv = ['/usr/bin/env', '-i', 'PATH=' + ENV['PATH'], 'LANG=C', '/usr/bin/python3', '-I',
                str(c.directory / 'root_controller.py'), 'watch-lease', '--run-id', c.launch['runId'], '--transaction', self.kind]
        timer_started = started
        if self.grouped:
            timer_started = time.monotonic()
            timer_seconds = int(self.group_deadline - GROUP_RECOVERY_RESERVE - timer_started) - 2
            require(timer_seconds >= 1, 'no time remains to arm joint recovery')
        c.auth_reached('lease-timer-start')
        c.ops.run(['/usr/bin/systemd-run', '--unit=' + self.timer, '--on-active=' + str(timer_seconds) + 's', '--timer-property=AccuracySec=1s',
            '--property=Type=oneshot', '--property=TimeoutStartSec=' + str(GROUP_RECOVERY_SERVICE_SECONDS if self.grouped else 20) + 's', '/usr/bin/env', '-i', 'PATH=' + ENV['PATH'], 'LANG=C',
            '/usr/bin/python3', '-I', str(c.directory / 'root_controller.py'), 'watch-lease', '--run-id', c.launch['runId'],
            '--transaction', self.kind])
        c.auth_reached('lease-timer-verify')
        timer_proof = verify_timer(c.ops, c.tun, self.timer, argv, timer_seconds, timer_started,
                     **({'recovery_seconds': GROUP_RECOVERY_SERVICE_SECONDS} if self.grouped else {}))
        if self.grouped:
            require(timer_proof['deadlineMonotonic'] <= self.group_deadline - GROUP_RECOVERY_RESERVE,
                    'actual recovery timer leaves insufficient joint-deadline reserve')
            self.group_timer_verified = True
            c.auth_reached('group-add')
            run_group_writer(c.launch, c.tun, c.directory, self.kind, 'add', c.ops)
            self.user, original = credential_account(c.launch, c.tun)
            require(time.monotonic() < self.group_deadline - GROUP_RECOVERY_RESERVE - 5, 'joint lease deadline exhausted')
        # The setting outcome may become unknown: persist recovery identity first.
        c.ledger.mutate(lambda v: v['authentication']['lease'].update(state='setting-pending', watchdogActive=True))
        import secrets
        self.secret = bytearray(secrets.token_urlsafe(48).encode('ascii'))
        try:
            if self.grouped: require(time.monotonic() < self.group_deadline - GROUP_RECOVERY_RESERVE - 5, 'joint lease deadline exhausted')
            original_now = account_status(self.user.pw_name)
            require(original_now in ('L', 'NP'), 'account changed before setting')
            c.auth_reached('setting-intent')
            setting_intent(c.directory, self.kind, c.launch, original_now)
            self.attempted = True  # only the durable fresh-status intent grants re-lock recovery
            c.auth_reached('password-writer')
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
            c.auth_reached('password-readback')
            require(account_status(self.user.pw_name) == 'P', 'temporary password installation not observed')
            c.ledger.mutate(lambda v: (v['authentication'].update(accountMutated=True),
                                      v['authentication']['lease'].update(state='active')))
            self.thread = threading.Thread(target=self.pump, name='owned-private-auth-pty', daemon=True)
            self.thread.start()
        except BaseException:
            c.auth_note('auth_failed')
            self.close(); raise
        return {'permissionBound': True, 'agentRegistered': True, 'authenticated': False,
                'maxLeaseSeconds': 90, 'passwordInArgvOrEnvironment': False}

    def pump(self):
        import termios
        data = bytearray(); deadline = self.group_deadline - GROUP_RECOVERY_RESERVE if self.grouped else time.monotonic() + self.window - 5; selected = False
        user = self.user
        display = user.pw_name if not user.pw_gecos or user.pw_gecos == user.pw_name else user.pw_gecos + ' (' + user.pw_name + ')'
        failure_code = 'unexpected-error'; cleanup_stop = False
        def observe(stage=None, flag=None, code=None, stopped=None):
            self.c.auth_note('auth_pump', stage, flag, code, stopped)
        def check(condition, code):
            nonlocal failure_code
            failure_code = code; require(condition, code)
        try:
            observe('waiting')
            while not self.stop.is_set() and time.monotonic() < deadline:
                failure_code = 'pty-read-failed'
                if not select.select([self.c.pty], [], [], 0.05)[0]: continue
                part = os.read(self.c.pty, 1024); check(part, 'pty-eof')
                observe('bytes-observed', 'ptyBytes')
                data += part; check(len(data) <= 16384, 'dialogue-limit')
                failure_code = 'dialogue-encoding'
                text = re.sub(rb'\x1b\[[0-9;?]*[a-zA-Z]', b'', bytes(data)).decode('utf-8', 'strict').replace('\r', '')
                check('AUTHENTICATION FAILED' not in text, 'native-authentication-failed')
                check('AUTHENTICATION CANCELED' not in text, 'native-authentication-canceled')
                check('AUTHENTICATING FOR org.freedesktop.policykit.exec' in text or not ('Password:' in text), 'action-mismatch')
                if 'AUTHENTICATING FOR org.freedesktop.policykit.exec' in text: observe('action-checked', 'expectedAction')
                observe('identity-checked')
                if 'Choose identity to authenticate as' in text and not selected:
                    matches = re.findall(r'^ ([0-9]+)\.  ' + re.escape(display) + r'$', text, re.M)
                    check(len(matches) == 1, 'identity-not-unique')
                    failure_code = 'identity-write-failed'
                    os.write(self.c.pty, matches[0].encode() + b'\n'); selected = True
                if self.grouped:
                    offered = re.findall(r'^Authenticating as: (.+)$', text, re.M)
                    check(all(name == display for name in offered), 'identity-mismatch')
                identity_ok = selected or ('Authenticating as: ' + display + '\n') in text
                if identity_ok: observe(flag='expectedIdentity')
                if 'Password:' in text and not self.sent:
                    check(identity_ok, 'identity-mismatch')
                    observe('echo-wait')
                    failure_code = 'echo-unconfirmed'
                    # pkttyagent prints the prompt just before disabling echo; wait
                    # for the actual terminal state, never sleep-and-assume it.
                    echo_deadline = time.monotonic() + 2
                    while termios.tcgetattr(self.c.pty)[3] & termios.ECHO:
                        check(time.monotonic() < echo_deadline, 'echo-unconfirmed')
                        time.sleep(0.005)
                    if self.grouped:
                        observe('pkexec-fence')
                        failure_code = 'pkexec-fence-unconfirmed'
                        check(self.c.observer is not None, failure_code)
                        self.c.observer.credential_fence(deadline, self.stop)
                        check(not self.stop.is_set() and time.monotonic() < deadline, failure_code)
                        check(not termios.tcgetattr(self.c.pty)[3] & termios.ECHO, 'echo-unconfirmed')
                    observe('credential-write', 'echoDisabled')
                    failure_code = 'credential-write-failed'
                    if self.grouped:
                        check(not self.stop.is_set() and time.monotonic() < deadline, 'pkexec-fence-unconfirmed')
                    written = os.write(self.c.pty, bytes(self.secret) + b'\n')
                    check(written == len(self.secret) + 1, 'credential-write-failed')
                    self.sent = True; observe(flag='credentialWriteCompleted')
                if '==== AUTHENTICATION COMPLETE ====' in text:
                    check(self.sent and identity_ok, 'completion-unconfirmed')
                    observe('completion-observed', 'authenticationComplete')
                    self.authenticated = True; return
            if self.stop.is_set():
                cleanup_stop = True; observe('stopped', stopped='controller-cleanup')
            else: observe(stopped='deadline')
            failure_code = 'activity-deadline'
            raise ValueError('authentication request did not complete within lease')
        except BaseException as error:
            if not cleanup_stop: observe(code=failure_code, stopped='worker-failure' if failure_code != 'activity-deadline' else 'deadline')
            self.c.auth_note('auth_failed')
            self.error = type(error).__name__
            try: self.close()
            except BaseException: self.error = 'CredentialRelockFailed'
        finally:
            for i in range(len(data)): data[i] = 0

    def close(self):
        self.c.auth_note('auth_agent', 'before-cleanup')
        if self.grouped: return self.close_group()
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
                self.c.auth_note('auth_recovery_step', 'credential', 'attempted')
                try:
                    self.credential_recovery = relock_account(self.c.launch, self.c.tun)
                    proof = self.credential_recovery
                    self.c.auth_note('auth_recovery_step', 'credential', 'completed', proof)
                except Exception:
                    self.c.auth_note('auth_recovery_step', 'credential', 'unknown'); raise
                # Recovery happens before this potentially failing ledger write.
                self.c.ledger.mutate(lambda v: (v['authentication'].update(accountMutated=False),
                    v['authentication']['lease'].update(state='closed', recovery=proof)))
            require(worker_stopped, 'private credential worker did not stop')
            require('authentication-recovery-record-unknown' not in self.c.recovery_unknown, 'recovery persistence unknown')
            self.c.auth_note('auth_recovery_step', 'timer', 'attempted')
            require('authentication-recovery-record-unknown' not in self.c.recovery_unknown, 'recovery persistence unknown')
            self.c.ops.run([self.c.tun.SYSTEMCTL, 'stop', self.timer + '.timer'])
            for unit in (self.timer + '.timer', self.timer + '.service'):
                require(self.c.ops.run([self.c.tun.SYSTEMCTL, 'show', unit, '--property=ActiveState', '--value']).strip() == 'inactive',
                        'credential watchdog fired or did not close')
            self.c.auth_note('auth_recovery_step', 'timer', 'completed')
            self.closed = True
        finally:
            for i in range(len(self.secret)): self.secret[i] = 0
            self.secret.clear()
            if self.writer_fd is not None: os.close(self.writer_fd); self.writer_fd = None
            self.close_lock.release()

    def close_group(self):
        self.c.auth_note('auth_agent', 'before-cleanup')
        self.stop.set()
        errors = []; proof = self.credential_recovery
        if self.thread and self.thread is not threading.current_thread():
            self.thread.join(timeout=3)
            if self.thread.is_alive(): errors.append('credential-writer-stop-unproved')
        # No closure/observer/ledger lock is held during this bounded wait.
        # Failure never skips the independent existing recovery actions below.
        try:
            if not self.closed and self.c.auth_observer:
                self.c.auth_observer.prepare_parent_stop(self.group_deadline)
        except Exception: errors.append('authentication-lineage-stop-unproved')
        require(self.close_lock.acquire(timeout=15), 'credential closure concurrency unknown')
        try:
            if self.closed: return
            try:
                if self.writer and self.writer.poll() is None:
                    require(self.writer_fd is not None, 'credential writer pidfd missing')
                    signal.pidfd_send_signal(self.writer_fd, signal.SIGKILL); self.writer.wait(timeout=5)
            except Exception: errors.append('credential-writer-stop-unproved')
            try:
                self.c.fence_registered_agent()
            except Exception: errors.append('authentication-lineage-stop-unproved')
            # These independent restorations must both be attempted, even if either fails.
            if self.attempted:
                self.c.auth_note('auth_recovery_step', 'credential', 'attempted')
                try:
                    self.credential_recovery = relock_account(self.c.launch, self.c.tun)
                    proof = self.credential_recovery
                    self.c.auth_note('auth_recovery_step', 'credential', 'completed', proof)
                except Exception:
                    errors.append('credential-relock-unproved')
                    self.c.auth_note('auth_recovery_step', 'credential', 'unknown')
                else:
                    try: self.c.ledger.mutate(lambda v: v['authentication'].update(accountMutated=False))
                    except Exception: errors.append('authentication-recovery-record-unknown')
            if self.group_intent_written:
                self.c.auth_note('auth_recovery_step', 'group', 'attempted')
                try:
                    self.group_recovery = recover_group_membership(self.c.launch, self.c.tun, self.c.directory, self.kind, self.c.ops)
                    self.c.auth_note('auth_recovery_step', 'group', 'completed', self.group_recovery)
                except Exception:
                    errors.append('group-membership-recovery-unproved')
                    self.c.auth_note('auth_recovery_step', 'group', 'unknown')
            if self.group_deadline is not None and time.monotonic() > self.group_deadline:
                errors.append('joint-lease-deadline-exceeded')
            try:
                if self.c.auth_observer:
                    lineage = self.c.auth_observer.close()
                    self.c.group_lineage_closed = lineage; self.c.auth_observer = None
                    self.c.retain_auth_lineage(lineage)
            except Exception: errors.append('authentication-lineage-stop-unproved')
            if 'authentication-recovery-record-unknown' in self.c.recovery_unknown:
                errors.append('authentication-recovery-record-unknown')
            if errors:
                self.c.recovery_unknown.extend(errors)
                raise ValueError('joint credential/group recovery unproved')
            if self.group_intent_written:
                if proof is not None:
                    intent = read_group_intent(self.c.directory, self.kind, self.c.launch, self.c.tun)
                    write_group_closed(self.c.directory, self.kind, self.c.launch, intent['binding'],
                                       self.group_deadline, proof, self.group_recovery)
                self.c.ledger.mutate(lambda v: (v['authentication'].update(accountMutated=False),
                    v['authentication']['lease'].update(state='closed', recovery=proof, groupRecovery=self.group_recovery)))
            if self.group_timer_verified:
                self.c.auth_note('auth_recovery_step', 'timer', 'attempted')
                require('authentication-recovery-record-unknown' not in self.c.recovery_unknown, 'recovery persistence unknown')
                self.c.ops.run([self.c.tun.SYSTEMCTL, 'stop', self.timer + '.timer'])
                for unit in (self.timer + '.timer', self.timer + '.service'):
                    require(self.c.ops.run([self.c.tun.SYSTEMCTL, 'show', unit, '--property=ActiveState', '--value']).strip() == 'inactive',
                            'joint lease watchdog not inactive')
                self.c.auth_note('auth_recovery_step', 'timer', 'completed')
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
        self.error_lock = threading.Lock()
        self.thread = threading.Thread(target=self.loop, name='owned-pkexec-observer', daemon=True)

    def start(self):
        self.thread.start()

    def fail_boundary(self, boundary):
        # Fixed non-secret cause survives snapshot's secondary exception and recovery.
        with self.error_lock:
            first = self.error is None
            if first: self.error = boundary
        if first: self.c.auth_note('auth_failed', 'transaction-' + boundary + '-unconfirmed')

    def loop(self):
        while not self.stop.wait(0.01):
            try: self.scan()
            except Exception: return  # scan retains the first fixed boundary

    def candidate(self, pid, light):
        for attempt in range(3):
            try:
                value = self.c.tun.transaction_identity(pid)
                p = value['identity']
                require(birth(p) == birth(light) and p['ppid'] == light['ppid'], 'candidate topology changed')
                return value
            except (OSError, ValueError):
                if attempt == 2: raise
                time.sleep(0.001)

    def scan(self):
        c = self.c; tun = c.tun; boundary = 'gui'
        try:
            require(tun.same_process(c.gui, c.ops.proc(c.gui['pid'])), 'GUI birth changed during privilege transaction')
            boundary = 'inventory'; values = {}
            # No full exe/namespace/cmdline reads for unrelated system processes.
            for item in Path('/proc').iterdir():
                if item.name.isdigit() and int(item.name) > 1:
                    p = proc_stat(int(item.name))
                    if p and p['state'] != 'Z': values[p['pid']] = p
            def depth(row):
                seen = set(); count = 0
                while row['ppid'] in values and row['ppid'] not in seen and count < 32:
                    seen.add(row['ppid']); row = values[row['ppid']]; count += 1
                return count
            with self.lock:
                # Parent exec-state updates precede child ownership checks even
                # when procfs enumeration happens to return children first.
                for light in sorted(values.values(), key=depth):
                    parent = values.get(light['ppid'])
                    direct = parent and birth(parent) == birth(c.gui)
                    child = parent and birth(parent) in self.rows
                    if not direct and not child: continue
                    boundary = 'candidate'; observation = self.candidate(light['pid'], light)
                    p = observation['identity']; key = birth(p)
                    if direct:
                        require(tun.same_process(c.gui, c.ops.proc(c.gui['pid'])), 'GUI parent changed')
                    else:
                        require(tun.same_process(self.rows[birth(parent)], c.ops.proc(parent['pid'])), 'owned parent changed')
                    argv = observation['argv']; uids = observation['uids']; executable = observation['executable']
                    if direct:
                        if p['exe'] == str(Path('/usr/bin/pkexec').resolve()):
                            boundary = 'pkexec-argv'
                            require(argv[:2] == ['/usr/bin/pkexec', '/bin/bash'] and len(argv) == 3, 'unexpected GUI pkexec argv')
                            boundary = 'pkexec-uid'
                            initial = uids == (c.launch['guiUid'], 0, 0, 0)
                            require(initial or (uids == (0, 0, 0, 0) and key in self.pkexec), 'pkexec UID transition differs')
                            boundary = 'script'; proof = c.verify_script(argv[2], self.kind)
                            if key in self.pkexec:
                                require(argv[2] == self.pkexec[key]['script'] and proof == self.pkexec[key]['proof'], 'original script changed')
                            boundary = 'pkexec-executable'; require(executable == c.tools['pkexec'], 'pkexec object changed')
                            boundary = 'namespace'; require(p['namespaces'] == c.gui['namespaces'] and p['cgroup'] == c.gui['cgroup'], 'pkexec namespace/cgroup differs')
                            pending = {'identity': p, 'script': argv[2], 'proof': proof}
                        elif p['exe'] == str(Path('/bin/bash').resolve()) and uids == (0, 0, 0, 0):
                            boundary = 'shell-prior'; require(key in self.pkexec, 'root bash without original pkexec birth')
                            boundary = 'shell-argv'; require(argv == ['/bin/bash', self.pkexec[key]['script']], 'root bash argv differs')
                            boundary = 'shell-executable'; require(executable == c.tools['bash'], 'root bash object changed')
                            boundary = 'script'; proof = c.verify_script(argv[1], self.kind)
                            require(proof == self.pkexec[key]['proof'], 'original script changed')
                            boundary = 'namespace'; require(p['namespaces'] == c.gui['namespaces'] and p['cgroup'] == c.gui['cgroup'], 'root shell escaped GUI')
                            pending = None
                        else:
                            boundary = 'ordinary-uid'; require(uids[0] == c.launch['guiUid'] and uids[1] == c.launch['guiUid'], 'unobserved privileged GUI child')
                            continue
                    else:
                        boundary = 'descendant-uid'; require(uids == (0, 0, 0, 0), 'privileged descendant UID differs')
                        boundary = 'namespace'; require(p['namespaces'] == c.gui['namespaces'] and p['cgroup'] == c.gui['cgroup'], 'privileged descendant escaped GUI')
                        pending = None
                    boundary = 'ledger'
                    def record(v):
                        records = v.setdefault('transactionProcesses', [])
                        old = next((r for r in records if birth(r['identity']) == key), None)
                        if old: old['identity'] = p
                        else: records.append({'role': 'privilege-transaction', 'identity': p})
                    c.ledger.mutate(record)
                    self.rows[key] = p
                    if direct and p['exe'] == str(Path('/usr/bin/pkexec').resolve()): self.pkexec[key] = pending
                    elif direct: self.shells[key] = proof
        except Exception:
            self.fail_boundary(boundary)
            raise

    def credential_fence(self, deadline, stopped):
        # No lock is held while waiting, reading procfs or persisting observations.
        def acquire():
            while not stopped.is_set():
                remaining = deadline - time.monotonic()
                require(remaining > 0, 'credential observer lock deadline')
                if self.lock.acquire(timeout=min(remaining, 0.005)): return
            raise ValueError('credential observer canceled')
        try:
            while not stopped.is_set() and time.monotonic() < deadline:
                acquire()
                try:
                    require(self.error is None, 'transaction observer failed')
                    require(len(self.pkexec) <= 1 and not self.shells, 'ambiguous credential transaction')
                    prior = json.loads(json.dumps(next(iter(self.pkexec.values())))) if self.pkexec else None
                finally: self.lock.release()
                if prior is None:
                    stopped.wait(0.005); continue
                p = prior['identity']; observed = self.candidate(p['pid'], p)
                now = observed['identity']
                require(self.c.tun.same_process(p, now) and now['ppid'] == self.c.gui['pid'] and
                        self.c.tun.same_process(self.c.gui, self.c.ops.proc(self.c.gui['pid'])) and
                        observed['uids'] == (self.c.launch['guiUid'], 0, 0, 0) and
                        observed['executable'] == self.c.tools['pkexec'] and
                        observed['argv'] == ['/usr/bin/pkexec', '/bin/bash', prior['script']] and
                        self.c.verify_script(prior['script'], self.kind) == prior['proof'], 'credential pkexec proof changed')
                acquire()
                try:
                    require(self.error is None and len(self.pkexec) == 1 and not self.shells, 'credential observer changed')
                finally: self.lock.release()
                require(not stopped.is_set() and time.monotonic() < deadline, 'credential deadline reached')
                return
            raise ValueError('credential fence unavailable')
        except Exception:
            self.fail_boundary('credential-fence')
            raise

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
        self.stage = 'idle'; self.preparation_pending = False; self.recovery_unknown = []
        self.readonly_auth = None
        self.vm_preflight = None
        self.auth_runtime = auth_runtime_observation(self.launch['authenticationMode'])
        self.auth_runtime_lock = threading.RLock()

    def auth_snapshot(self):
        with self.auth_runtime_lock:
            return json.loads(json.dumps(validate_auth_runtime_observation(self.auth_runtime)))

    def auth_checkpoint(self):
        # A partial successful action remains an observed fact if later cleanup fails.
        # Persistence failure is independently permanent; never turn it into clean closure.
        with self.auth_runtime_lock:
            if self.ledger is None: return
            try:
                if self.auth_runtime['persistence'] != 'unknown': self.auth_runtime['persistence'] = 'recorded'
                snapshot = self.auth_snapshot()
                self.ledger.mutate(lambda v: v['authentication'].update(runtimeObservation=snapshot))
            except Exception:
                self.auth_runtime['persistence'] = 'unknown'
                self.recovery_unknown.append('authentication-recovery-record-unknown')

    def auth_reached(self, stage, kind=None):
        require(stage in AUTH_OPERATION_STAGES and kind in (None, 'install', 'uninstall'), 'fixed auth stage required')
        with self.auth_runtime_lock:
            if kind is not None and self.auth_runtime['kind'] != kind:
                # Keep the first overall failure permanent; a new successful transaction
                # starts a new per-transaction recovery view only on the normal path.
                require(self.auth_runtime['firstFailure'] is None and not self.recovery_unknown, 'failed authentication cannot restart')
                self.auth_runtime = auth_runtime_observation(self.launch['authenticationMode'])
                self.auth_runtime['kind'] = kind
            self.auth_runtime['lastReached'] = stage
        self.auth_checkpoint()

    def auth_failed(self, code='operation-failed'):
        require(code in AUTH_FAILURE_CODES, 'fixed auth failure code required')
        with self.auth_runtime_lock:
            if self.auth_runtime['firstFailure'] is None:
                self.auth_runtime['firstFailure'] = {'stage': self.auth_runtime['lastReached'], 'code': code}
        self.auth_checkpoint()

    def auth_recovery_step(self, step, state, proof=None):
        require(step in AUTH_RECOVERY_STEPS and state in ('attempted', 'completed', 'unknown'), 'fixed recovery step required')
        with self.auth_runtime_lock:
            # Re-entering idempotent cleanup does not erase a completed action/proof.
            if self.auth_runtime['recovery'][step] == 'completed' and state == 'attempted': return
            updated = json.loads(json.dumps(self.auth_runtime))
            updated['recovery'][step] = state
            if step in ('credential', 'group'):
                updated[step + 'Recovery'] = proof if state == 'completed' else None
            validate_auth_runtime_observation(updated)
            self.auth_runtime = updated
        self.auth_checkpoint()

    def auth_pump(self, stage=None, flag=None, code=None, stopped=None):
        with self.auth_runtime_lock:
            value = self.auth_runtime['execution']['pump']
            if stage is not None:
                require(stage in AUTH_PUMP_STAGES, 'fixed pump stage required'); value['stage'] = stage
            if flag is not None:
                require(flag in AUTH_PUMP_FLAGS, 'fixed pump flag required'); value['observed'][flag] = True
            if code is not None and value['firstFailure'] is None:
                require(code in AUTH_PUMP_CODES, 'fixed pump code required')
                value['firstFailure'] = {'stage': value['stage'], 'code': code}
            if stopped is not None: value['stoppedBy'] = stopped
        self.auth_checkpoint()

    def auth_agent(self, event):
        with self.auth_runtime_lock:
            value = self.auth_runtime['execution']['agent']
            require(event in ('notify', 'bound', 'poll', 'before-cleanup'), 'fixed agent event required')
            if event == 'notify': value['notifyClosed'] = True
            if event == 'bound':
                value['subjectBound'] = True; value['registrationLive'] = True
            if self.agent is not None:
                code = self.agent.poll()
                require(code is None or type(code) is int and -128 <= code <= 255, 'fixed agent exit required')
                value['latestLive'] = code is None; value['exitCode'] = code
                if event == 'before-cleanup' and not value['preCleanupObserved']:
                    value['preCleanupObserved'] = True
                    value['preCleanupLive'] = code is None; value['preCleanupExitCode'] = code
        self.auth_checkpoint()

    def auth_note(self, method, *args):
        # Diagnostic persistence is never a prerequisite for emergency recovery.
        try: return getattr(self, method)(*args)
        except Exception:
            self.recovery_unknown.append('authentication-recovery-record-unknown')

    def fail(self, reason='operation-failed'):
        self.auth_note('auth_failed', reason)
        self.failed = True
        if self.lease:
            try: self.lease.close()
            except Exception:
                self.recovery_unknown.append('authentication-lease-recovery-unknown')  # watchdog remains armed
        if self.ledger:
            try: self.ledger.mutate(lambda v: v.update(result='failed'))
            except Exception: self.recovery_unknown.append('root-ledger-failure-unconfirmed')

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
        # The real shared Unix producer emits this one header. The product
        # explicitly invokes /bin/bash; this is not generic shebang authorization.
        require(data.startswith(b'#!/bin/sh\nset -eu\n'), 'original Linux script header required')
        return proof

    def bind_gui(self, pid):
        self.stage = 'bind-gui'
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
            light = proc_stat(int(entry.name))
            if not light or light['state'] == 'Z': continue
            try: uids = uid_tuple(light['pid'])
            except FileNotFoundError:
                require(proc_absent(light['pid']), 'binding candidate unavailable'); continue
            again = proc_stat(light['pid'])
            if not again:
                require(proc_absent(light['pid']), 'binding candidate unavailable'); continue
            require(birth(again) == birth(light), 'binding candidate changed')
            if uids[0] != gui['uid']: continue
            candidate = self.ops.proc(light['pid'])
            if not candidate:
                require(proc_absent(light['pid']), 'binding candidate unavailable'); continue
            require(birth(candidate) == birth(light), 'binding candidate changed')
            if candidate['state'] == 'Z' or candidate['uid'] != gui['uid']: continue
            if (candidate['exeDev'], candidate['exeInode']) == (gui['exeDev'], gui['exeInode']): matches.append(birth(candidate))
        require(matches == [birth(gui)], 'actual packaged App is not unique in the dedicated VM')
        require(self.tun.same_process(gui, self.ops.proc(pid)), 'GUI changed while binding')
        self.gui = gui
        return {'identity': gui, 'ancestorBirths': chain, 'executable': executable}

    def authentication_readiness(self):
        mode = self.launch['authenticationMode']
        if mode in CREDENTIAL_MODES:
            self.readonly_auth = auth_observation()
            grouped = mode == AUTH_GROUP_MODE
            if grouped: self.group_binding, _ = group_binding(self.launch, expected_member=False)
            user, status = credential_account(self.launch, self.tun, observation=self.readonly_auth, group_pending=grouped)
            return {'kind': 'exact-dispatch-credential-authority', 'account': user.pw_name,
                    'originalPasswordStatus': status, 'authenticated': False,
                    **({'groupMembership': 'pending-guarded-lease'} if grouped else {})}
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
        self.stage = 'authentication-readiness'
        readiness = self.authentication_readiness()
        self.stage = 'vm-preflight'
        self.vm_preflight = self.tun.vm_observation()
        facts = self.tun.collect_preflight(self.launch['guiUid'], self.launch['vmContext'], True, self.ops)
        self.vm_preflight = self.tun.validate_vm_observation(facts['vmPreflight'])
        try:
            check = self.tun.preflight(facts, self.launch, authentication_confirmed=False)
            self.vm_preflight['blocked'] = check['blocked']
            self.vm_preflight['gate'] = 'blocked' if check['blocked'] else 'ready'
            self.vm_preflight['stages']['gate'] = {'state': 'observed', 'error': 'none'}
            self.tun.validate_vm_observation(self.vm_preflight)
        except Exception as error:
            self.vm_preflight['blocked'] = []
            self.vm_preflight['gate'] = 'failed'
            self.vm_preflight['stages']['gate'] = {'state': 'failed', 'error': self.tun.vm_error(error)}
            raise
        require(check['blocked'] == ['realPkexecAuthenticationPendingConfirmation'], 'VM/material preflight blocked')
        # Registration/guard preparation is NOT authentication. Network remains closed
        # until the real original product installation authorization is observed.
        self.stage = 'tool-identities'
        self.tools = {name: self.tun.file_identity(Path(path).resolve()) for name, path in
                      (('pkexec', '/usr/bin/pkexec'), ('bash', '/bin/bash'), ('pkttyagent', '/usr/bin/pkttyagent'))}
        self.stage = 'root-ledger'; self.preparation_pending = True
        self.ledger = self.tun.prepare_root(self.launch['runId'], self.launch['guiUid'],
            {'systemNamespaces': facts['systemNamespaces'], 'network': facts['network']},
            self.launch['helperSha256'], self.launch['coreSha256'], seconds=600, ops=self.ops)
        self.preparation_pending = False; self.stage = 'controller-ledger'
        self.ledger.mutate(lambda v: v.update(controllerGui=self.gui, controllerIdentity=self.tun.process_identity(os.getpid()), authentication={'mode': self.launch['authenticationMode'],
            'authorized': False, 'closed': False, 'accountMutated': False, 'agent': None}, transactionProcesses=[]))
        independent_record(self.directory, 'controller-birth.json', self.tun.process_identity(os.getpid()))
        unit = 'polaris-tun-auth-' + self.launch['runId']
        started = time.monotonic()
        self.stage = 'auth-watchdog-start'
        self.ops.run(['/usr/bin/systemd-run', '--unit=' + unit, '--on-active=580s', '--timer-property=AccuracySec=1s',
            '--property=Type=oneshot', '--property=TimeoutStartSec=' + str(GROUP_RECOVERY_SERVICE_SECONDS if self.launch['authenticationMode'] == AUTH_GROUP_MODE else 15) + 's', '/usr/bin/env', '-i', 'PATH=' + ENV['PATH'],
            'LANG=C', '/usr/bin/python3', '-I', str(self.directory / 'root_controller.py'),
            'watch-auth', '--run-id', self.launch['runId']])
        self.stage = 'auth-watchdog-verify'
        verify_timer(self.ops, self.tun, unit, ['/usr/bin/env', '-i', 'PATH=' + ENV['PATH'], 'LANG=C',
                     '/usr/bin/python3', '-I', str(self.directory / 'root_controller.py'), 'watch-auth',
                     '--run-id', self.launch['runId']], 580, started,
            **({'recovery_seconds': GROUP_RECOVERY_SERVICE_SECONDS} if self.launch['authenticationMode'] == AUTH_GROUP_MODE else {}))
        require(time.monotonic() + 580 < self.ledger.load()['deadlineMonotonic'] - 5, 'auth watchdog preparation consumed reserved margin')
        self.ledger.mutate(lambda v: v.update(authenticationWatchdogActive=True, authenticationTimer=unit + '.timer'))
        self.stage = 'prepared'
        return {'guarded': True, 'authentication': 'pending-real-product-authorization', 'preflight': check, 'authenticationReadiness': readiness,
                'controllerDeadlineMarginSeconds': 30}

    def agent_register(self):
        self.auth_reached('agent-spawn')
        self.guard()
        require(self.launch['authenticationMode'] in ('pkttyagent-observe', *CREDENTIAL_MODES) and self.agent is None,
                'owned observer agent not requested or already started')
        import pty
        master, slave = pty.openpty(); read_fd, notify_fd = os.pipe()
        user = pwd.getpwuid(self.launch['guiUid'])
        os.fchown(slave, user.pw_uid, user.pw_gid); os.fchmod(slave, 0o600)
        argv = ['/usr/bin/setsid', '--ctty', '/usr/bin/setpriv', '--reuid=' + str(user.pw_uid),
                '--regid=' + str(user.pw_gid), '--clear-groups', '--bounding-set=-all', '--inh-caps=-all', '--ambient-caps=-all',
                '/usr/bin/pkttyagent', '--process', str(self.gui['pid']) + ',' + self.gui['startTime'],
                '--notify-fd', str(notify_fd)]
        # The approved joint credential mode uses this exact GUI agent, not a
        # session-selected fallback. Normal process registration still rejects
        # an existing same-subject agent; never unregister or replace one.
        # Password-only and passive observation retain their prior preference.
        if self.launch['authenticationMode'] != AUTH_GROUP_MODE:
            argv.append('--fallback')
        self.ledger.mutate(lambda v: v['authentication'].update(agentSpawnPending=True, lineageClosed=None, agentStopObserved=None))
        try:
            child = subprocess.Popen(argv, env=ENV, stdin=slave, stdout=slave, stderr=slave, pass_fds=(notify_fd,))
            self.agent = child; self.pty = master
            os.close(slave); slave = -1; os.close(notify_fd); notify_fd = -1
            self.auth_reached('agent-notify')
            require(select.select([read_fd], [], [], 10)[0] and os.read(read_fd, 1) == b'', 'agent registration not notified')
            self.auth_note('auth_agent', 'notify')
            self.auth_reached('agent-identity')
            row = self.ops.proc(child.pid)
            require(child.poll() is None and row and row['ppid'] == os.getpid() and row['uid'] == user.pw_uid and
                    row['namespaces'] == self.gui['namespaces'] and
                    self.tun.executable_identity(child.pid) == self.tools['pkttyagent'], 'owned real pkttyagent not running')
            self.auth_note('auth_agent', 'bound')
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
        self.auth_reached('transaction-guard', kind)
        value = self.guard()
        require(kind in ('install', 'uninstall') and self.observer is None, 'single fixed transaction required')
        require(not value['authentication']['closed'], 'authentication window closed')
        require((kind == 'install' and not value.get('helper')) or
                (kind == 'uninstall' and value.get('helper') and value.get('capture')), 'transaction order differs')
        if self.launch['authenticationMode'] in CREDENTIAL_MODES:
            self.agent_register()
            self.auth_reached('agent-record')
            independent_record(self.directory, 'agent-' + kind + '.json',
                               self.ledger.load()['authentication']['agent']['identity'])
            self.auth_reached('lineage-start')
            self.auth_observer = AuthObserver(self, kind); self.auth_observer.start()
            self.lease = CredentialLease(self, kind); self.lease.open()
        self.auth_reached('transaction-observer')
        self.observer = TransactionObserver(self, kind); self.observer.start()
        return {'observing': kind}

    def transaction_end(self):
        self.auth_reached('transaction-finish')
        require(self.observer is not None, 'no active transaction')
        result = self.observer.finish(); self.observer = None
        if self.lease:
            self.lease.close()
            require(self.lease.authenticated and self.lease.error is None, 'real credential authentication not proved')
            result['credentialAuthenticationComplete'] = True
            closed_auth = self.stop_agent(close_window=False)
            result['credentialRecovery'] = self.ledger.load()['authentication']['lease']['recovery']
            if self.launch['authenticationMode'] == AUTH_GROUP_MODE:
                require(self.lease.group_recovery is not None, 'joint membership recovery absent')
                self.group_recovery = self.lease.group_recovery
                result['jointLease'] = {'credentialRecovery': result['credentialRecovery'], 'groupRecovery': self.group_recovery}
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
                light = proc_stat(int(item.name))
                if not light or light['state'] == 'Z' or light['ppid'] != value['helper']['identity']['pid']: continue
                row = self.ops.proc(light['pid'])
                require(row and birth(row) == birth(light) and row['ppid'] == light['ppid'], 'core candidate changed')
                rows.append(row)
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

    def fence_registered_agent(self):
        agent = self.ledger.load()['authentication'].get('agent')
        if not agent: return
        needed_force = stop_recorded(self.ops, self.tun, agent['identity'])
        require(self.agent is not None and self.agent.pid == agent['identity']['pid'], 'registered agent handle missing')
        status = self.agent.wait(timeout=5)
        require(proc_absent(agent['identity']['pid']), 'registered agent not reaped')
        require(not needed_force and status == -signal.SIGTERM, 'agent required emergency fence')
        self.ledger.mutate(lambda v: v['authentication'].update(agentStopObserved=agent['identity']))
        self.auth_note('auth_recovery_step', 'agent', 'completed')

    def retain_auth_lineage(self, lineage):
        exact(lineage, ('ownedBirths', 'unknownAdoptedChildren', 'forced'))
        require(lineage['unknownAdoptedChildren'] == [] and type(lineage['forced']) is bool,
                'authentication lineage closure incomplete')
        if self.agent:
            require(self.agent.wait(timeout=5) == -signal.SIGTERM, 'owned agent shutdown differs')
        for pid, start in lineage['ownedBirths']:
            require(proc_absent(pid), 'authentication descendant not actually reaped or PID reused')
        self.ledger.mutate(lambda v: v['authentication'].update(lineageClosed=lineage))
        self.auth_note('auth_recovery_step', 'agent', 'completed')
        self.auth_note('auth_recovery_step', 'lineage', 'completed')
        return lineage

    def stop_agent(self, close_window=True, forced=False):
        self.auth_note('auth_agent', 'before-cleanup')
        lineage = self.auth_observer.close(forced=forced) if self.auth_observer else getattr(self, 'group_lineage_closed', None)
        if lineage:
            self.group_lineage_closed = lineage
            self.retain_auth_lineage(lineage)
        self.auth_observer = None
        result = close_auth(self.ledger, self.tun, forced=forced, close_window=close_window)
        if self.agent:
            require(self.agent.wait(timeout=5) == -signal.SIGTERM, 'owned agent shutdown differs')
        if self.pty is not None: os.close(self.pty); self.pty = None
        self.auth_note('auth_agent', 'poll')
        self.agent = None
        if lineage:
            result['authLineage'] = lineage
            result['authDescendantsReaped'] = True
        return result

    def auth_close(self):
        require(self.observer is None, 'privilege transaction not closed')
        if self.lease: self.lease.close()
        result = self.stop_agent()
        if self.launch['authenticationMode'] == AUTH_GROUP_MODE:
            require(getattr(self, 'group_recovery', None) is not None, 'final membership recovery absent')
            intent = read_group_intent(self.directory, 'uninstall', self.launch, self.tun)
            require(intent is not None, 'final group intent absent')
            group_binding(self.launch, original=intent['binding'], expected_member=False)
            result['groupRecovery'] = self.group_recovery
        return result

    def finish(self):
        value = self.guard()
        require(not self.failed and not self.recovery_unknown and value['phase'] == 'closed' and value['authentication']['closed'] and
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
        self.stage = 'dispatch'
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
            self.auth_note('auth_agent', 'poll')
            result['authenticationRuntime'] = self.auth_snapshot()
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
        self.fail('abort-requested')
        if self.observer:
            try:
                self.observer.stop.set()
                if self.observer.thread.ident is not None: self.observer.thread.join(timeout=3)
                require(not self.observer.thread.is_alive(), 'transaction observer remains live')
                self.observer = None
            except Exception: self.recovery_unknown.append('privilege-transaction-cleanup-unknown')
        if not self.ledger:
            return {'schema': 'polaris-root-abort-v1', 'state': 'failed', 'forced': True,
                'prepared': None if self.preparation_pending else False, 'actualReadOnlyAuth': self.readonly_auth,
                'authenticationRuntime': self.auth_snapshot(),
                **({'vmPreflight': self.tun.validate_vm_observation(self.vm_preflight)} if self.vm_preflight is not None else {}),
                'unknown': sorted(set(self.recovery_unknown +
                (['root-preparation-incomplete'] if self.preparation_pending else [])))}
        # Auth recovery is independent and attempted even if product cleanup raises.
        errors = list(self.recovery_unknown)
        try: self.stop_agent(forced=True)
        except Exception:
            errors.append('authentication-cleanup-unknown')
            # Incomplete lineage must not skip the independent exact registered-agent fence.
            try: self.fence_registered_agent()
            except Exception: errors.append('authentication-subtree-cleanup-unknown')
            try: close_auth(self.ledger, self.tun, forced=True)
            except Exception: errors.append('authentication-cleanup-unknown')
            try:
                if self.pty is not None: os.close(self.pty); self.pty = None
            except Exception: errors.append('authentication-cleanup-unknown')
        self.auth_note('auth_agent', 'poll')
        try: close_transactions(self.ledger, self.tun)
        except Exception: errors.append('privilege-transaction-cleanup-unknown')
        try: result = self.tun.cleanup(self.ledger, forced=True)
        except Exception: result = {'state': 'failed'}; errors.append('product-cleanup-unknown')
        try:
            self.ledger.mutate(lambda v: v.update(unknown=sorted(set(v['unknown'] + errors)), result='failed'))
        except Exception: errors.append('root-ledger-recovery-unconfirmed')
        return {'schema': 'polaris-root-abort-v1', 'state': 'failed', 'forced': True, 'prepared': True,
                'actualReadOnlyAuth': self.readonly_auth, 'unknown': sorted(set(errors + self.recovery_unknown)), 'productCleanup': result,
                'authenticationRuntime': self.auth_snapshot(),
                **({'vmPreflight': self.tun.validate_vm_observation(self.vm_preflight)} if self.vm_preflight is not None else {})}


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
        elif not auth.get('closed') and not auth.get('lineageClosed') and auth.get('agentStopObserved') != old:
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
    parser.add_argument('command', choices=('serve', 'watch-auth', 'watch-lease', 'group-write'))
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--transaction', choices=('install', 'uninstall'))
    parser.add_argument('--group-operation', choices=('add', 'remove'))
    args = parser.parse_args()
    if args.command == 'serve':
        startup_stage = 'frozen-load'
        try:
            launch, tun, directory = load_frozen(args.run_id)
            startup_stage = 'subreaper'
            import ctypes
            require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0, 'process-local child subreaper required')
            startup_stage = 'controller-init'
            controller = Controller(launch, tun, directory)
        except BaseException:
            print(json.dumps({'schema': SCHEMA, 'ready': False,
                'runId': args.run_id if re.fullmatch(r'[0-9a-f]{12}', args.run_id) else None,
                'startupFailure': {'stage': startup_stage, 'code': 'FROZEN_STARTUP_FAILED',
                                   'stagingCleanup': 'retained'}}), flush=True)
            raise SystemExit(72) from None
    else:
        launch, tun, directory = load_frozen(args.run_id, recovery=True)
    if args.command == 'group-write':
        group_writer_main(launch, tun, directory, args.transaction, args.group_operation)
        raise SystemExit(1)
    require(args.group_operation is None, 'unexpected group operation')
    if args.command in ('watch-lease', 'watch-auth'):
        require((args.command == 'watch-lease') == bool(args.transaction), 'watchdog transaction binding differs')
        watchdog_recover(launch, tun, directory, args.transaction)
        raise SystemExit(1)  # timeout/recovery always fails acceptance
    print(json.dumps({'schema': SCHEMA, 'ready': True, 'runId': args.run_id,
                      'credentialMutationImplemented': True, 'credentialAuthorityPresent': launch['authorization'] is not None}), flush=True)
    try:
        while not controller.finished:
            # Input timeout is bounded by the reserved watchdog margin as well.
            try: raw = read_line(sys.stdin.buffer, timeout=570)
            except BaseException:
                controller.auth_note('auth_failed', 'input-ended')
                raise
            try:
                response = controller.dispatch(json.loads(raw))
            except Exception as error:
                controller.fail()
                not_ready = isinstance(error, AuthenticationNotReady) and controller.ledger is None
                response = {'seq': controller.seq, 'ok': False, 'terminal': True,
                            'error': {'code': 'AUTHENTICATION_NOT_READY' if not_ready else 'CONTROLLER_FAILED',
                                      'kind': type(error).__name__,
                                      'stage': controller.stage if controller.stage in CONTROLLER_STAGES else 'dispatch',
                                      'message': 'fixed operation failed; no acceptance or authentication success'}}
                response['error']['authenticationRuntime'] = controller.auth_snapshot()
                if controller.vm_preflight is not None:
                    response['error']['vmPreflight'] = controller.tun.validate_vm_observation(controller.vm_preflight)
                if controller.readonly_auth is not None:
                    response['error']['actualReadOnlyAuth'] = validate_auth_observation(controller.readonly_auth)
                if not_ready:
                    response['error'].update(mutated=False, mutationScope='product-network-account')
            print(json.dumps(response, separators=(',', ':')), flush=True)
    finally:
        if not controller.finished: controller.abort()


if __name__ == '__main__':
    main()
