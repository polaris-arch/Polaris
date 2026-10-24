#!/usr/bin/env python3
"""Separate, fail-closed final-carrier TUN GUI consumer.

No import-time privilege/network/product action. Ordinary GUI stays sandboxed;
root operations use only frozen root_controller typed IPC. Mock tests are never
runtime evidence. deb means extraction, not package installation/uninstallation.
"""
from __future__ import annotations
import argparse
import base64
import copy
import hashlib
import json
import os
import re
from pathlib import Path
import select
import signal
import socket
import stat
import subprocess
import tempfile
import threading
import time

import linux as base
import tun
import tun_fixtures as fixture
import helper_observation

require = base.require
HERE = Path(__file__).resolve().parent


AUTH_GROUP_MODE = 'temporary-existing-runner-password-and-sudo-group'


def observe_logind_session(supervisor, env, app):
    """Existing ordinary read-only tool; unique destination cannot auto-activate login1.

    All errors are unknown, never inferred session absence or authentication.
    Supervisor-owned forced/unknown cleanup records are deliberately not cleared.
    """
    result = {'schema': 'polaris-logind-session-observation-v1', 'state': 'unknown',
              'subject': {key: app[key] for key in ('pid', 'startTime', 'uid')},
              'birthStable': False, 'scope': 'direct-logind-session-only-not-polkit-display-fallback-or-authorization'}
    try:
        before = base.process_identity(app['pid'])
        require(base.same_process(app, before), 'session subject birth changed')
        prefix = ['/usr/bin/gdbus', 'call', '--system']
        owner = supervisor.query(prefix + ['--dest', 'org.freedesktop.DBus', '--object-path',
            '/org/freedesktop/DBus', '--method', 'org.freedesktop.DBus.GetNameOwner',
            'org.freedesktop.login1'], env, 'logind-observation', timeout=2, limit=1024).strip()
        match = re.fullmatch(r"\('(:[0-9]+\.[0-9]+)',\)", owner)
        require(match is not None, 'fixed unique service owner required')
        reply = supervisor.query(prefix + ['--dest', match[1], '--object-path', '/org/freedesktop/login1',
            '--method', 'org.freedesktop.login1.Manager.GetSessionByPID', str(app['pid'])],
            env, 'logind-observation', timeout=2, limit=1024).strip()
        require(re.fullmatch(r"\(objectpath '/org/freedesktop/login1/session/[A-Za-z0-9_]+',\)", reply) is not None,
                'fixed session response required')
        after = base.process_identity(app['pid'])
        require(base.same_process(app, after) and base.same_process(before, after), 'session subject birth changed')
        result['birthStable'] = True; result['state'] = 'present'
    except Exception:
        pass
    return result


def verify_group_recovery(value, uid, account, expected_gid=None):
    keys = {'schema', 'account', 'guiUid', 'group', 'groupGid', 'originalMembership',
            'finalMembership', 'removedOnlyAddedMembership', 'writerStopped', 'maxLeaseSeconds'}
    require(isinstance(value, dict) and set(value) == keys
            and value['schema'] == 'polaris-sudo-membership-recovery-v1'
            and account == 'runner' and type(uid) is int and uid == 1001
            and value['account'] == account and type(value['guiUid']) is int and value['guiUid'] == uid
            and value['group'] == 'sudo' and type(value['groupGid']) is int and value['groupGid'] > 0
            and value['originalMembership'] == value['finalMembership'] == 'absent'
            and value['removedOnlyAddedMembership'] is True and value['writerStopped'] is True
            and type(value['maxLeaseSeconds']) is int and value['maxLeaseSeconds'] == 90,
            'bounded membership recovery unconfirmed')
    require(expected_gid is None or value['groupGid'] == expected_gid, 'bound sudo group identity changed')
    return value


def verify_joint_recovery(proof, subject, expected_gid=None):
    joint = proof.get('jointLease', {})
    credential = proof.get('credentialRecovery')
    require(isinstance(credential, dict) and joint.get('credentialRecovery') == credential
            and proof.get('credentialAuthenticationComplete') is True
            and proof.get('authDescendantsReaped') is True
            and credential.get('guiUid') == subject['guiUid'] and credential.get('account') == subject['account']
            and credential.get('finalPasswordStatus') == 'L' and credential.get('plaintextPersisted') is False
            and credential.get('emptyPasswordRestored') is False
            and credential.get('temporaryPasswordHash') == 'locked until disposable VM destruction',
            'joint credential recovery unconfirmed')
    return verify_group_recovery(joint.get('groupRecovery'), subject['guiUid'], subject['account'], expected_gid)


def verify_group_preparation(value):
    readiness = value.get('authenticationReadiness', {})
    require(value.get('guarded') is True and value.get('authentication') == 'pending-real-product-authorization'
            and readiness.get('authenticated') is False
            and readiness.get('groupMembership') == 'pending-guarded-lease',
            'membership must remain pending before guarded lease')
    return 'prepared-pending-group-lease'


def private_root_probe(source, directory, expected_hash):
    """Detach Cargo's hardlinked example; never relax the root single-link guard."""
    source, directory = Path(source), Path(directory)
    uid = os.getuid()
    def identity(value):
        return tuple(getattr(value, key) for key in
                     ('st_dev', 'st_ino', 'st_mode', 'st_uid', 'st_gid', 'st_nlink',
                      'st_size', 'st_mtime_ns', 'st_ctime_ns'))
    parent = directory.lstat()
    require(stat.S_ISDIR(parent.st_mode) and parent.st_uid == uid and
            parent.st_mode & 0o077 == 0, 'private probe directory required')
    dfd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        require(identity(os.fstat(dfd)) == identity(parent), 'private probe directory changed')
        before = source.lstat()
        require(stat.S_ISREG(before.st_mode) and before.st_uid == uid and
                0 < before.st_size <= 128 * 1024 * 1024, 'private probe source metadata invalid')
        fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as incoming:
            require(identity(os.fstat(fd)) == identity(before), 'private probe source changed')
            # Bounded even if another same-user writer grows the source.
            data = incoming.read(128 * 1024 * 1024 + 1)
            require(len(data) == before.st_size and
                    hashlib.sha256(data).hexdigest() == expected_hash and
                    identity(os.fstat(fd)) == identity(before) and
                    identity(source.lstat()) == identity(before), 'private probe source changed')
        name = 'root-rpc-probe'
        out = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=dfd)
        with os.fdopen(out, 'w+b') as target:
            created = os.fstat(out)
            require(stat.S_ISREG(created.st_mode) and created.st_uid == uid and created.st_nlink == 1 and
                    (created.st_dev, created.st_ino) != (before.st_dev, before.st_ino),
                    'private probe copy identity invalid')
            target.write(data); target.flush(); os.fchmod(out, 0o500); os.fsync(out)
            copied = os.fstat(out)
            target.seek(0)
            require(hashlib.sha256(target.read(128 * 1024 * 1024 + 1)).hexdigest() == expected_hash and
                    copied.st_size == len(data) and copied.st_uid == uid and copied.st_nlink == 1 and
                    identity(os.fstat(out)) == identity(copied) and
                    identity(os.stat(name, dir_fd=dfd, follow_symlinks=False)) == identity(copied),
                    'private probe copy changed')
        final_parent = directory.lstat()
        require(stat.S_ISDIR(final_parent.st_mode) and final_parent.st_uid == uid and
                final_parent.st_mode & 0o077 == 0 and
                (final_parent.st_dev, final_parent.st_ino) == (parent.st_dev, parent.st_ino) and
                identity(source.lstat()) == identity(before) and
                identity((directory / name).lstat()) == identity(copied), 'private probe path changed')
        return directory / name
    finally:
        # On error retain any partial/changed target for the existing guarded
        # workspace cleanup; never unlink an unconfirmed same-name replacement.
        os.close(dfd)


def inspect_helper(payload, members):
    payload = Path(payload).resolve()
    # The package also carries the source/update configuration with the same
    # basename. Installation identity belongs only beside the frozen core.
    core = base.regular(payload / members['core']['path'], payload)
    cronet = base.regular(payload / members['cronet']['path'], payload)
    # CarrierRun already verified these bytes against the frozen members.
    require(core.parent == cronet.parent, 'split frozen core/Cronet payload')
    helper = base.regular(core.parent / 'polaris-helper', payload)
    require(len([p for p in payload.rglob('polaris-helper') if p.is_file()]) == 1,
            'one packaged helper required')
    require(helper.read_bytes()[:4] == b'\x7fELF' and os.access(helper, os.X_OK), 'packaged helper must be executable ELF')
    manifest = base.regular(core.parent / 'core-manifest.json', payload)
    require(0 < manifest.stat().st_size <= 65536, 'bounded adjacent selected-core manifest required')
    return {'helper': base.byte_identity(helper, payload), 'manifest': base.byte_identity(manifest, payload),
            'embeddedManifest': json.loads(manifest.read_bytes()),
            'core': members['core'], 'cronet': members['cronet']}


def birth(value):
    """Public, stable identity, independent of restricted proc dumpability."""
    require(isinstance(value, dict) and type(value.get('pid')) is int and value['pid'] > 0,
            'actual process birth required')
    start = value.get('startTime', value.get('startTimeTicks'))
    require(str(start).isdigit() and int(start) > 0, 'actual process start time required')
    return {'pid': value['pid'], 'startTime': str(start)}


def same_birth(a, b):
    try:
        return birth(a) == birth(b)
    except ValueError:
        return False


def app_execution_identity(app):
    # Same ordinary producer contract as plain; not tun.process_identity's
    # namespace/capability-rich root authorization identity.
    return base.app_execution_identity(app)


def verify_gui_binding(app, proof):
    root = proof.get('identity', {}); executable = proof.get('executable', {})
    require(all(type(app.get(key)) is int and app[key] > 0 for key in ('exeDev', 'exeInode')),
            'observed App inode identity required')
    require(same_birth(app, root) and root.get('uid') == os.getuid() and
            root.get('exeDev') == app.get('exeDev') and root.get('exeInode') == app.get('exeInode') and
            executable.get('dev') == app.get('exeDev') and executable.get('inode') == app.get('exeInode') and
            executable.get('sha256') == app['sha256'], 'root authorization subject is not exact observed App ELF')
    return proof


def join_capture(analysis, snapshots, streams, dns, plan):
    """Join real pcap frames, ordinary client nonces and origin observations."""
    require(analysis.get('analysisExecution') == 'real-tool', 'simulated capture cannot establish TUN evidence')
    require(set(snapshots) == {'A', 'B', 'O'}, 'all controlled backend observations required')
    for role, snapshot in snapshots.items():
        require(snapshot.get('role') == role and not snapshot.get('errors'), 'backend failed or changed role')
    captured = {r['nonceSha256']: r for r in analysis['nonces']}
    require(len(captured) == len(analysis['nonces']), 'duplicate captured nonce identity')
    origins = snapshots['O']['rows']
    joined = {}
    for label, stream in streams.items():
        nonce = stream['nonceSha256']
        matches = [r for r in origins if r.get('kind') == 'echo' and r.get('nonceSha256') == nonce]
        require(nonce in captured and len(matches) == 1, 'client/capture/origin nonce not uniquely joined')
        origin = matches[0]
        require(stream['bytes'] == captured[nonce]['bytes'] == origin['bytes'] and
                stream['exit'] == origin['exit'] and origin['originPeer'] == plan['exit'][stream['exit']],
                'origin egress/nonce byte identity differs')
        row = {'source': 'owned-tun-capture-and-origin', 'analysisExecution': 'real-tool',
               'captureSha256': analysis['captureSha256'], 'destination': plan['origin'],
               'interface': 'polaris-tun0', 'proxyEnvironment': 'absent', 'clientMode': 'direct-tcp',
               'packets': analysis['packets'], 'captureNonceSha256': nonce, 'originNonceSha256': nonce,
               'exit': stream['exit'], 'originPeer': origin['originPeer']}
        tun.validate_data_plane(row)
        joined[label] = row
    joined_dns = []
    for query in dns:
        cap = [r for r in analysis['dnsQueries'] if r.get('querySha256') == query['querySha256']
               and r.get('transport') == query['transport'] and r.get('name') == query['name']]
        back = [{**r, 'backendRole': role} for role in ('A', 'B') for r in snapshots[role]['rows']
                if r.get('name') == query['name'] and r.get('type') == query['type']]
        require(cap and back and all(r.get('answer') == plan['origin'] and r.get('ttl') == 0
                and r['backendRole'] == query.get('expectedBackend', r['backendRole']) for r in back),
                'actual TUN DNS capture/backend/TTL0 join missing')
        joined_dns.append({**{k: v for k, v in query.items() if k != 'expectedBackend'}, 'tunObserved': True, 'analysisExecution': 'real-tool',
                           'captureSha256': analysis['captureSha256'],
                           'captureQuerySha256': cap[0]['querySha256'],
                           'backendName': back[0]['name'], 'backendType': back[0]['type'],
                           'backendQuerySha256': back[0]['querySha256'], 'backendRole': back[0]['backendRole'], 'ttl': back[0]['ttl']})
    return joined, joined_dns


class TunSupervisor(base.Supervisor):
    """Ordinary ownership is unchanged. Privileged births are quarantined only.

No privileged birth is admitted to ``owned``, and no name/path inference grants
signal authority. A bounded GUI window is accepted only after an independent
root ledger strongly confirms mandatory births. Separately typed terminal-only
auxiliary evidence never verifies execution or grants signal authority.
"""
    def __init__(self):
        self.window = None
        self.quarantine = {}
        self.confirmed = set()
        self.auxiliary_confirmed = {}
        self.controller_birth = None
        self.controller_descendants = {}
        self.gui_birth = None
        super().__init__()

    def begin_window(self, kind, gui):
        require(kind in ('install', 'uninstall') and self.window is None, 'bounded GUI auth window required')
        self.gui_birth = birth(gui)
        self.window = {'kind': kind, 'started': time.monotonic(), 'births': set(),
                       'subject':{k:gui[k] for k in ('pid','startTime','uid')},'binding':None}

    def bind_window(self, binding, launch):
        import root_controller
        require(self.window is not None and self.window['binding'] is None,'one root binding per GUI window')
        root_controller.validate_transaction_binding(binding,launch=launch,subject=self.window['subject'],kind=self.window['kind'])
        self.window['binding']=copy.deepcopy(binding)

    def end_window(self, proof):
        import root_controller
        require(self.window is not None and self.window['binding'] is not None,'bound GUI authentication window required')
        window=self.window
        require(time.monotonic()-window['started']<90,'GUI transaction window expired')
        require(type(proof) is dict and proof.get('closed') is True and proof.get('transactionBinding')==window['binding'] and
                proof.get('kind')==window['kind'] and proof.get('subject')==window['subject'], 'root GUI window proof differs')
        binding=root_controller.validate_transaction_binding(proof['transactionBinding'],subject=window['subject'],kind=window['kind'])
        owned=proof['owned'];aux=proof['auxiliaryTerminals']
        root_controller.validate_auxiliary_terminals(aux,binding,owned,proof['pkexecBirths'],proof['rootShellBirths'])
        require(len(proof['pkexecBirths'])==len(proof['rootShellBirths'])==1 and proof.get('authorizationObserved') is True,
                'strong mandatory authorization proof required')
        require(proof['auxiliaryIdentityCoverage']==('incomplete' if aux else 'complete'),'auxiliary coverage differs')
        observed={(birth(p)['pid'],birth(p)['startTime']) for p in owned}
        require(len(observed)==len(owned) and observed,'strong root births required')
        auxiliary={}
        for row in aux:
            item=row['inventory'];key=(item['pid'],item['startTime'])
            if key in self.quarantine:
                require(key in window['births'] and self.quarantine[key]['ppid']==item['ppid'],
                        'auxiliary observation outside window or different parent')
            fresh=tun.unverified_terminal_identity(item)
            require(fresh['state'] in ('absent','zombie') and
                    (row['terminal']['state']!='absent' or fresh['state']=='absent'), 'auxiliary terminal contradicted')
            auxiliary[key]=copy.deepcopy(row)
        require(window['births'] <= observed | set(auxiliary),'root GUI lineage observation incomplete')
        # Keep the two classes disjoint. In particular no auxiliary ever enters
        # owned, confirmed(strong), or a signal path; prior unknowns are untouched.
        require(not (observed & set(auxiliary)),'auxiliary cannot replace strong ownership')
        with self.lock:
            require(self.window is window and window['births'] <= observed | set(auxiliary),
                    'GUI transaction window changed or new birth unconfirmed')
            for key,row in auxiliary.items():
                if key in self.quarantine:
                    require(key in window['births'] and same_birth(self.quarantine[key],row['inventory']) and
                            self.quarantine[key]['ppid']==row['inventory']['ppid'],
                            'auxiliary raced ordinary inventory boundary')
            self.confirmed.update(observed)
            self.auxiliary_confirmed.update(auxiliary)
            self.window=None

    def scan(self):
        observed = {int(p.name): base.process_identity(int(p.name)) for p in Path('/proc').iterdir() if p.name.isdigit()}
        observed = {pid: item for pid, item in observed.items() if item}
        with self.lock:
            changed = True
            while changed:
                changed = False
                for pid, item in observed.items():
                    key = (pid, item['startTime'])
                    parent = observed.get(item['ppid'])
                    controller_parent = self.controller_descendants.get(item['ppid'])
                    if same_birth(item, self.controller_birth) or same_birth(parent, controller_parent):
                        if not same_birth(item, self.controller_descendants.get(pid)):
                            self.controller_descendants[pid] = item; changed = True
                        continue
                    if key in self.quarantine:
                        continue
                    foreign = item['uid'] != os.getuid()
                    owned_parent = base.same_process(parent, self.owned.get(item['ppid']))
                    root_parent = bool(parent and (parent['pid'], parent['startTime']) in self.quarantine)
                    if foreign and (base.same_process(item, self.owned.get(pid)) or owned_parent or root_parent):
                        within = self.window and time.monotonic() - self.window['started'] < 90
                        gui_ancestor = same_birth(parent, self.gui_birth) or root_parent
                        if not (within and gui_ancestor):
                            self.record_diagnostic('foreign-descendant-outside-exact-gui-transaction', item, parent, stage='tun-scan')
                            self.unknown.append('foreign-descendant-outside-exact-GUI-transaction')
                        self.owned.pop(pid, None); self.roles.pop(pid, None)
                        self.record_diagnostic('tun-privileged-birth-quarantined', item, parent, stage='tun-scan')
                        self.quarantine[key] = item
                        if self.window: self.window['births'].add(key)
                        changed = True
                        continue
                    if base.same_process(item, self.owned.get(pid)):
                        continue
                    if owned_parent:
                        require(not foreign, 'ordinary ownership cannot admit elevated descendant')
                        self.owned[pid] = item
                        self.roles[pid] = self.roles.get(item['ppid'], 'unknown'); changed = True
                    elif item['ppid'] == os.getpid() and pid not in self.children:
                        self.record_diagnostic('unobserved-adopted-descendant', item, parent, stage='tun-scan')
                        self.unknown.append('unobserved-adopted-descendant')
                        # Unknown foreign births are never eligible for signal.
                        if not foreign:
                            self.owned[pid] = item; self.roles[pid] = 'unknown'
            for _ in range(len(self.owned) + 1):
                changed = False
                for pid, item in observed.items():
                    if base.same_process(item, self.owned.get(pid)) and self.roles.get(item['ppid']) == 'product' and self.roles.get(pid) != 'product':
                        self.roles[pid] = 'product'; changed = True
                if not changed: break
        return observed

    def signal(self, item, sig):
        require(item.get('uid') == os.getuid() and
                (base.process_identity(item['pid']) or {}).get('uid') == os.getuid(),
                'TUN ordinary supervisor cannot signal foreign UID')
        require((item['pid'], item['startTime']) not in self.quarantine and
                not same_birth(item, self.controller_descendants.get(item['pid'])),
                'privileged observer must not gain signal authority')
        return super().signal(item, sig)

    def close(self, normal):
        try:
            if self.window or any(key not in self.confirmed and key not in self.auxiliary_confirmed for key in self.quarantine):
                self.unknown.append('unconfirmed-privileged-GUI-lineage')
            for item in self.quarantine.values():
                current = base.process_identity(item['pid'])
                if base.same_process(item, current) and current['state'] != 'Z':
                    self.record_diagnostic('privileged-gui-descendant-not-reaped', current, stage='tun-close')
                    self.unknown.append('privileged-GUI-descendant-not-reaped')
            # The fixed root controller must finish before ordinary tool cleanup.
            for item in self.controller_descendants.values():
                current = base.process_identity(item['pid'])
                if base.same_process(item, current) and current['state'] != 'Z':
                    self.record_diagnostic('root-controller-descendant-not-reaped', current, stage='tun-close')
                    self.unknown.append('root-controller-descendant-not-reaped')
        except Exception:
            self.unknown.append('privileged-cleanup-observation-unconfirmed')
        cleanup=super().close(normal)
        # Root's sampled zombie is not reaping. After ordinary descendants/tools
        # close, every terminal-only auxiliary must be whole-PID absent twice.
        for row in self.auxiliary_confirmed.values():
            try:
                fresh=tun.unverified_terminal_identity(row['inventory'])
                require(fresh['state']=='absent','auxiliary not reaped')
            except Exception:
                self.unknown.append('auxiliary-terminal-not-reaped')
        cleanup['auxiliaryIdentityCoverage']='incomplete' if self.auxiliary_confirmed else 'complete'
        cleanup['unknown']=sorted(set(self.unknown))
        if cleanup['unknown']: cleanup['state']='failed'
        return cleanup


TUN_OBSERVER = r'''
const state={rows:[],cursor:0,failed:false,started:performance.now(),firstHelperFailure:null,firstOuterFailure:null,firstUnclassified:null,context:null};
const project=__HELPER_PROJECTOR__;
const callbacks=window.__TAURI_INTERNALS__?.callbacks, prior=callbacks?.get;
if(!(callbacks instanceof Map)||typeof prior!=='function'||window.__tunObservation) throw Error('TUN callback observer unavailable');
Object.defineProperty(callbacks,'get',{configurable:true,value:function(id){
 const callback=Reflect.apply(prior,this,[id]); if(typeof callback!=='function')return callback;
 return function(...args){try{const raw=args[0],d=raw?.data;
  const projected=project(raw);
  if(projected){
   const row={sequence:++state.cursor,kind:'helperObservation',observedElapsedMs:Math.max(0,Math.floor(performance.now()-state.started)),projection:projected};
   state.rows.push(row);
   if(projected.kind==='helperAction'&&projected.actionSuccess===false&&!state.firstHelperFailure)state.firstHelperFailure=row;
   if(projected.kind==='outerFailure'&&!state.firstOuterFailure)state.firstOuterFailure=row;
  }else if(state.context!==null){
   const type=raw===null?'null':Array.isArray(raw)?'array':typeof raw;
   const row={sequence:++state.cursor,kind:'unclassifiedCallback',observedElapsedMs:Math.max(0,Math.floor(performance.now()-state.started)),
    valueType:['null','array','object','string','number','boolean','undefined'].includes(type)?type:'other'};
   state.rows.push(row);if(!state.firstUnclassified)state.firstUnclassified=row;
  }
  if(typeof d?.status==='string'&&typeof d?.pendingApply==='boolean'&&Array.isArray(d.reasons))
   state.rows.push({sequence:++state.cursor,kind:'apply',status:d.status,
    reasons:d.reasons.map(r=>({code:r.code,path:r.path}))});
  if(state.rows.length>256)state.rows.shift();
 }catch(_){state.failed=true;}return Reflect.apply(callback,this,args);};
}});Object.defineProperty(window,'__tunObservation',{value:state});return true;
'''.replace('__HELPER_PROJECTOR__', helper_observation.PROJECTOR_JS)


def direct_driver(supervisor):
    candidates = [p for pid, p in supervisor.owned.items()
                  if pid in supervisor.children and supervisor.roles.get(pid) == 'driver']
    require(len(candidates) == 1 and base.same_process(candidates[0], base.process_identity(candidates[0]['pid'])),
            'one direct registered tauri-driver birth required')
    return candidates[0]


class Controller:
    """Bounded JSON-line IPC to the independently frozen, narrow root broker."""
    def __init__(self):
        # Assignment to the owning run precedes every possible child launch.
        self.seq, self.closed = 0, False
        self.child = None
        self.read_buffer = bytearray()
        self.lock = threading.Lock()
        self.lifecycle = {'schema': 'polaris-tun-controller-lifecycle-v1',
                          'stage': 'allocated', 'spawned': False, 'ready': False,
                          'claim': 'observed-boundaries-not-privileged-cleanup-proof'}

    def start(self, launch, sources, supervisor, env):
        try:
            return self._start(launch, sources, supervisor, env)
        except BaseException:
            self.lifecycle['failureCode'] = 'CONTROLLER_START_FAILED'
            raise

    def _start(self, launch, sources, supervisor, env):
        import root_controller
        require(self.child is None and not self.closed, 'single controller start required')
        self.lifecycle['stage'] = 'envelope'
        envelope = {'launch': launch, 'files': {name: base64.b64encode(data).decode() for name, data in sources.items()}}
        require(len((json.dumps(envelope, separators=(',', ':')) + '\n').encode()) <= 2 * 1024 * 1024,
                'bootstrap envelope bound')
        self.lifecycle['stage'] = 'spawn'
        with supervisor.lock:
            self.child = subprocess.Popen(['sudo', '-n', '/usr/bin/env', '-i',
                'PATH=/usr/sbin:/usr/bin:/sbin:/bin', '/usr/bin/python3', '-I', '-c', root_controller.BOOTSTRAP],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                env=env, start_new_session=True)
            self.lifecycle.update(spawned=True, stage='birth')
            identity = base.process_identity(self.child.pid)
            require(identity, 'root controller birth unavailable')
            self.lifecycle['childBirth'] = birth(identity)
            supervisor.controller_birth = identity
            supervisor.controller_descendants[identity['pid']] = identity
        self.lifecycle['stage'] = 'write-envelope'
        self._write(envelope)
        self.lifecycle['stage'] = 'read-ready'
        ready = self._read(30)
        if (isinstance(ready, dict) and ready.get('schema') == root_controller.SCHEMA and
                ready.get('runId') in (None, launch['runId']) and ready.get('ready') is False):
            failure = ready.get('startupFailure', {})
            if (failure.get('stage') in root_controller.BOOTSTRAP_STAGES and
                    failure.get('code') in ('BOOTSTRAP_FAILED', 'FROZEN_STARTUP_FAILED') and
                    failure.get('stagingCleanup') in ('not-created', 'removed', 'unconfirmed', 'retained')):
                self.lifecycle['rootStartupFailure'] = {key: failure[key] for key in ('stage', 'code', 'stagingCleanup')}
        require(isinstance(ready, dict) and ready.get('schema') == root_controller.SCHEMA and
                ready.get('ready') is True and ready.get('runId') == launch['runId'],
                'root controller preparation blocked or failed')
        self.lifecycle.update(stage='ready', ready=True)

    def recover(self):
        """Only typed broker abort, own stdin closure and direct-child wait.

        Ordinary ownership never gains signal authority over root processes.
        Child exit is not evidence of privileged resource restoration.
        """
        unknown = []
        if self.lifecycle['ready'] and not self.closed:
            try:
                import root_controller
                recovery = root_controller.validate_abort_receipt(self.call('abort'), vm_validator=tun.validate_vm_observation)
                self.lifecycle['abortReceipt'] = copy.deepcopy(recovery)
                unknown.extend(recovery.get('unknown', []))
                self.lifecycle['abortAcknowledged'] = True
            except Exception:
                unknown.append('root-abort-unconfirmed')
        if self.child is not None:
            try:
                self.child.stdin.close()
                self.lifecycle['inputClosed'] = True
            except Exception:
                unknown.append('controller-input-close-unconfirmed')
            try:
                status = self.child.wait(timeout=20)
                self.lifecycle['childExit'] = status
                self.closed = True
            except Exception:
                unknown.append('controller-exit-unconfirmed')
        else:
            self.closed = True
        self.lifecycle['recoveryUnknown'] = sorted(set(unknown))
        return {'state': 'failed', 'forced': True,
                'unknown': sorted(set(unknown + ['exception-required-recovery']))}

    def _write(self, value):
        raw = (json.dumps(value, separators=(',', ':')) + '\n').encode()
        require(len(raw) < 4 * 1024 * 1024, 'controller request bound')
        fd = self.child.stdin.fileno(); deadline = time.monotonic() + 15
        os.set_blocking(fd, False)
        try:
            at = 0
            while at < len(raw):
                remaining = deadline - time.monotonic()
                require(remaining > 0 and select.select([], [fd], [], remaining)[1], 'controller request timeout')
                try: written = os.write(fd, raw[at:at + 65536])
                except BlockingIOError: continue
                require(written > 0, 'controller pipe closed')
                at += written
        finally:
            os.set_blocking(fd, True)

    def _read(self, timeout):
        deadline = time.monotonic() + timeout
        while b'\n' not in self.read_buffer:
            remaining = deadline - time.monotonic()
            require(remaining > 0 and select.select([self.child.stdout], [], [], remaining)[0],
                    'controller response timeout')
            chunk = os.read(self.child.stdout.fileno(), 65536)
            require(chunk, 'controller closed without response')
            self.read_buffer.extend(chunk)
            require(len(self.read_buffer) <= 4 * 1024 * 1024, 'controller bounded response required')
        line, _, remainder = self.read_buffer.partition(b'\n')
        self.read_buffer = bytearray(remainder)
        return json.loads(line)

    def call(self, action, **args):
        with self.lock:
            require(not self.closed, 'controller already closed')
            self.seq += 1
            self.lifecycle['lastAction'] = action
            boundary = 'request-write'
            try:
                self._write({'seq': self.seq, 'action': action, **args})
                boundary = 'response-read'
                value = self._read(90 if action in ('cleanup', 'finish') else 45)
            except Exception:
                self.lifecycle.setdefault('rootBoundaryFailure', {'stage': boundary, 'action': action if action in
                    ('transaction_begin', 'transaction_scan', 'transaction_end', 'auth_close', 'abort') else 'other',
                    'code': 'ROOT_IPC_UNCONFIRMED'})
                raise
            if not (isinstance(value, dict) and type(value.get('seq')) is int and value['seq'] == self.seq
                    and type(value.get('ok')) is bool):
                self.lifecycle.setdefault('rootBoundaryFailure', {'stage': 'response-envelope', 'action': action if action in
                    ('transaction_begin', 'transaction_scan', 'transaction_end', 'auth_close', 'abort') else 'other',
                    'code': 'ROOT_IPC_UNCONFIRMED'})
                raise ValueError('controller response sequence/schema unconfirmed')
            if value.get('ok') is False:
                error = value.get('error', {})
                import root_controller
                if error.get('stage') in root_controller.CONTROLLER_STAGES and error.get('code') in ('CONTROLLER_FAILED', 'AUTHENTICATION_NOT_READY'):
                    self.lifecycle.setdefault('rootOperationFailure', {'stage': error['stage'], 'code': error['code']})
                    if 'authenticationRuntime' in error:
                        self.lifecycle['rootOperationFailure']['authenticationRuntime'] = copy.deepcopy(
                            root_controller.validate_auth_runtime_observation(error['authenticationRuntime']))
                    if 'vmPreflight' in error:
                        self.lifecycle['rootOperationFailure']['vmPreflight'] = copy.deepcopy(tun.validate_vm_observation(error['vmPreflight']))
                    if 'actualReadOnlyAuth' in error:
                        observed = root_controller.validate_auth_observation(error['actualReadOnlyAuth'])
                        self.lifecycle['rootOperationFailure']['actualReadOnlyAuth'] = copy.deepcopy(observed)
            require(isinstance(value, dict) and value.get('seq') == self.seq and value.get('ok') is True, 'root controller action failed')
            return value['result']

    def finish(self):
        value = self.call('finish')
        self.child.stdin.close()
        self.lifecycle['inputClosed'] = True
        status = self.child.wait(timeout=15)
        self.lifecycle['childExit'] = status
        self.closed = True
        require(status == 0, 'root controller did not exit normally')
        return value


CONNECTIVITY_PROBE = r'''
import hashlib, http.client, json, socket, ssl, time
host = 'api.github.com'
started = time.time_ns()
addresses = sorted({r[4][0] for r in socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)})
if not addresses or len(addresses) > 64: raise RuntimeError('DNS proof unavailable')
context = ssl.create_default_context()
if context.verify_mode != ssl.CERT_REQUIRED or not context.check_hostname: raise RuntimeError('TLS verification unavailable')
connection = http.client.HTTPSConnection(host, 443, timeout=5, context=context)
try:
    connection.request('GET', '/meta', headers={'User-Agent': 'Polaris-package-TUN-acceptance', 'Connection': 'close'})
    response = connection.getresponse()
    if response.status != 200: raise RuntimeError('control HTTPS unavailable')
    prefix = response.read(8192)
    print(json.dumps({'host': host, 'dnsResolved': True, 'httpsStatus': response.status,
        'tlsVerified': True, 'addresses': addresses, 'boundedResponseBytes': len(prefix),
        'boundedResponseSha256': hashlib.sha256(prefix).hexdigest(),
        'readStartedUnixNs': str(started), 'readFinishedUnixNs': str(time.time_ns()),
        'scope': 'fresh DNS and verified HTTPS control connectivity; not zero DNS side effects'}))
finally:
    connection.close()
'''


class GuiConfigAdapter(fixture.ConfigAdapter):
    def close(self):
        # Adapter owns no sockets/processes. Root controller, not this no-op,
        # supplies all actual fixture cleanup evidence.
        return None


class TunCarrierRun(base.CarrierRun):
    fixture_group_label = 'Package isolated TUN'
    def __init__(self, item, package, workspace, evidence_dir, rpc_client, admission, bound, launch_options):
        self.run_id = os.urandom(6).hex()
        self.controller = None
        self.admission, self.bound, self.launch_options = admission, bound, launch_options
        self.pending_streams, self.dns_before, self.dns_after = {}, [], []
        self.network_observations = []
        self.stopped = False
        self.root_cleanup = None
        super().__init__(item, package, workspace, evidence_dir, rpc_client, fixture)
        self.result.update(scope='same-final-carrier', publicationEligible=False,
            networkBoundary={'fixtureDataPlane': 'isolated nonloopback TUN; one backend netns',
                             'backgroundReadOnlyInternet': 'allowed and observed; not zero-egress'})

    def create_network(self):
        return GuiConfigAdapter(self.run_id, control=self.fixture_control)

    def create_supervisor(self):
        return TunSupervisor()

    def fixture_control(self, role, request):
        return self.controller.call('fixture', role=role, operation=request['action'],
                                    **{k: v for k, v in request.items() if k != 'action'})

    def record(self, key, **evidence):
        require(key in tun.CASE_IDS and evidence, 'mandatory TUN evidence required')
        tun.validate_case(key, evidence)
        self.result['cases'][key] = {'status': 'passed', 'evidence': evidence}

    def launch(self):
        initial = super().launch()
        self.app = app_execution_identity(self.app)
        self.driver.execute(TUN_OBSERVER)
        return initial

    def tun_observations(self):
        return self.driver.execute("const s=window.__tunObservation;if(s.failed) throw Error('TUN observer failed');const rows=s.rows.slice();for(const first of [s.firstHelperFailure,s.firstOuterFailure,s.firstUnclassified])if(first&&!rows.some(r=>r.sequence===first.sequence))rows.unshift(first);return rows;")

    def helper_observations(self, kind, cursor, rows):
        # Click context is not native command identity; never attribute an outer
        # failure to helper authorization solely because it arrived in this window.
        require(kind in ('install', 'uninstall') and type(rows) is list and len(rows) <= 259,
                'bounded helper observation required')
        saved = []; actions = []; sequences = set()
        for row in rows:
            require(type(row) is dict, 'fixed callback row required')
            if row.get('kind') == 'unclassifiedCallback':
                require(set(row) == {'sequence','kind','observedElapsedMs','valueType'} and
                        type(row['sequence']) is int and row['sequence'] > 0 and
                        type(row['observedElapsedMs']) is int and 0 <= row['observedElapsedMs'] <= 86400000 and
                        row['valueType'] in ('null','array','object','string','number','boolean','undefined','other'),
                        'fixed uncorrelated callback observation required')
                if row['sequence'] <= cursor: continue
                require(row['sequence'] not in sequences, 'duplicate callback observation')
                sequences.add(row['sequence'])
                saved.append({'sequence':row['sequence'],'callbackObservedElapsedMs':row['observedElapsedMs'],
                    'kind':'unclassifiedCallback','association':'uncorrelated','valueType':row['valueType']})
                continue
            if row.get('kind') != 'helperObservation': continue
            require(set(row) == {'sequence', 'kind', 'observedElapsedMs', 'projection'} and
                    type(row['sequence']) is int and row['sequence'] > 0 and
                    type(row['observedElapsedMs']) is int and 0 <= row['observedElapsedMs'] <= 86400000,
                    'fixed helper callback observation required')
            if row['sequence'] <= cursor: continue
            require(row['sequence'] not in sequences, 'duplicate callback observation')
            sequences.add(row['sequence'])
            projection = helper_observation.validate_projection(row['projection'])
            observed = {'sequence': row['sequence'], 'callbackObservedElapsedMs': row['observedElapsedMs'],
                        'projection': projection}
            saved.append(observed)
            if projection['kind'] == 'helperAction':
                actions.append(observed)
                if projection['actionSuccess'] is False:
                    self.result.setdefault('firstBusinessFailure', {'source': 'native-helper-action-shape',
                        'requestedActionContext': kind, **copy.deepcopy(observed)})
        self.result['helperObservation'] = {'schema': 'polaris-helper-observation-v1',
            'requestedActionContext': kind, 'commandIdentityClaimed': False,
            'completeHistoryClaimed': False, 'rows': saved}
        require(len(actions) <= 1, 'ambiguous helper action callbacks')
        return actions[0] if actions else None

    def record_transaction_observation(self, value, kind):
        # This is the already-returned read-only snapshot, never a replacement
        # for transaction_end or evidence admitting ordinary root signals.
        keys = {'kind', 'subject', 'authenticationMode', 'pkexecBirths', 'rootShellBirths',
                'owned', 'authorizationObserved', 'credentialPromptClaimed', 'authenticationRuntime',
                'transactionBinding','auxiliaryIdentityCoverage','auxiliaryTerminals'}
        require(type(value) is dict and set(value) == keys and value['kind'] == kind and
                value['authenticationMode'] == self.launch_options['authenticationMode'],
                'fixed transaction observation required')
        import root_controller
        runtime = root_controller.validate_auth_runtime_observation(value['authenticationRuntime'])
        require(runtime['mode'] == value['authenticationMode'] and runtime['kind'] == kind,
                'authentication observation transaction differs')
        subject = value['subject']
        require(type(subject) is dict and set(subject) == {'pid', 'startTime', 'uid'} and
                type(subject['pid']) is int and subject['pid'] == self.app['pid'] and
                type(subject['startTime']) is str and subject['startTime'] == self.app['startTime'] and
                type(subject['uid']) is int and subject['uid'] == self.app['uid'],
                'transaction observation subject differs')
        def births(rows):
            require(type(rows) is list and len(rows) <= 256, 'bounded transaction births required')
            result = []
            for row in rows:
                require(type(row) is list and len(row) == 2 and type(row[0]) is int and row[0] > 1 and
                        type(row[1]) is str and row[1].isascii() and row[1].isdigit() and 0 < len(row[1]) <= 24,
                        'fixed transaction birth required')
                require(row not in result, 'duplicate transaction birth'); result.append(list(row))
            return result
        pkexec = births(value['pkexecBirths']); shells = births(value['rootShellBirths'])
        require(type(value['owned']) is list and len(value['owned']) <= 256, 'bounded owned observation required')
        owned = []
        for row in value['owned']:
            require(type(row) is dict and type(row.get('uid')) is int and row['uid'] in (0, subject['uid']),
                    'transaction observed UID differs')
            owned.extend(births([[row.get('pid'), row.get('startTime')]]))
        owned = births(owned)
        require(all(row in owned for row in pkexec + shells) and all(row in pkexec for row in shells) and
                type(value['authorizationObserved']) is bool and value['authorizationObserved'] == bool(shells) and
                value['credentialPromptClaimed'] is False, 'transaction observation claims differ')
        binding=root_controller.validate_transaction_binding(value['transactionBinding'],launch=self.transaction_launch,subject=self.app,kind=kind)
        require(binding==self.transaction_binding,'transaction scan differs from begin')
        aux=root_controller.validate_auxiliary_terminals(value['auxiliaryTerminals'],binding,value['owned'],
                                                         value['pkexecBirths'],value['rootShellBirths'])
        coverage='incomplete' if aux else 'complete'
        require(value['auxiliaryIdentityCoverage']==coverage,'transaction auxiliary coverage differs')
        if coverage=='incomplete' or self.result.get('auxiliaryIdentityCoverage')!='incomplete':
            self.result['auxiliaryIdentityCoverage']=coverage
        self.result['rootTransactionObservation'] = {'schema': 'polaris-root-transaction-observation-v1',
            'kind': kind, 'subject': dict(subject), 'pkexecBirths': pkexec, 'rootShellBirths': shells,
            'ownedBirths': owned, 'authorizationObserved': value['authorizationObserved'],
            'credentialPromptClaimed': False, 'scope': 'root-observed-births-not-transaction-completion-or-cleanup',
            'receivedElapsedMs': max(0, int((time.monotonic() - self._gui_started) * 1000)),
            'authenticationRuntime': copy.deepcopy(runtime), 'transactionBinding':copy.deepcopy(binding),
            'auxiliaryIdentityCoverage':coverage,'auxiliaryTerminals':copy.deepcopy(aux)}

    def helper_action(self, kind):
        self.gui_phase('helper-begin')
        require(kind in ('install', 'uninstall'), 'only genuine helper action')
        self.gui_phase('helper-navigation')
        self.driver.click('.side .nav-item', self.text('sidebar.settings'))
        self.driver.click('.set-nav', self.text('settings.nav.helper'))
        cursor = max((r['sequence'] for r in self.tun_observations()), default=0)
        self.gui_phase('helper-session-observation')
        self.result['logindSessionObservation'] = observe_logind_session(self.supervisor, getattr(self, 'env', {}), self.app)
        self.gui_phase('helper-window')
        self.supervisor.begin_window(kind, self.app)
        self.gui_phase('helper-transaction-begin')
        begin=self.controller.call('transaction_begin', kind=kind)
        import root_controller
        require(type(begin) is dict and set(begin)=={'observing','transactionBinding'} and begin['observing']==kind,
                'fixed transaction begin required')
        self.transaction_binding=copy.deepcopy(root_controller.validate_transaction_binding(begin['transactionBinding'],
            launch=self.transaction_launch,subject=self.app,kind=kind))
        self.supervisor.bind_window(self.transaction_binding,self.transaction_launch)
        self.driver.execute("window.__tunObservation.context=arguments[0];return true;", kind)
        self.gui_phase('helper-click')
        if kind == 'install':
            self.driver.click('.helper-state[data-s="none"] button', self.text('helper.installAction'))
        else:
            self.driver.click('.helper-state[data-s="installed"] button', self.text('helper.uninstall'))
            self.driver.click('.helper-state button.confirming', self.text('helper.uninstallConfirmAgain'))
        def result():
            self.gui_phase('helper-transaction-scan')
            scan_error = None
            try:
                observed = self.controller.call('transaction_scan')
                self.record_transaction_observation(observed, kind)
            except Exception as error:
                scan_error = error
                self.result.setdefault('helperObservationFailure', {'stage':'root-transaction-scan','code':'ROOT_TRANSACTION_OBSERVATION_FAILED'})
            # Both existing read-only observations are independent. A root scan
            # error must not erase an already-delivered native business failure.
            try:
                self.gui_phase('helper-callback-read')
                reply = self.helper_observations(kind, cursor, self.tun_observations())
            except Exception:
                if scan_error is not None:
                    self.gui_phase('helper-transaction-scan-failed')
                    raise scan_error
                raise
            if scan_error is not None:
                self.gui_phase('helper-transaction-scan-failed')
                raise scan_error
            return reply
        self.gui_phase('helper-callback')
        reply = base.wait_for(result, timeout=80)
        self.gui_phase('helper-callback-observed')
        projection = reply['projection']
        callback = {'envelopeSuccess': projection['envelopeSuccess'], 'success': projection['actionSuccess'],
                    'installed': projection['status']['installed'], 'ready': projection['status']['ready']}
        self.result['helperCallback'] = {**callback, 'errorCode': projection['errorCode'],
            'association': projection['association'], 'requestedActionContext': kind,
            'callbackObservedElapsedMs': reply['callbackObservedElapsedMs'], 'projection': copy.deepcopy(projection)}
        require(projection['statusShape'] == 'valid' and projection['executionDiagnosticState'] == 'absent' and
                projection['errorCode'] in ('none', 'missing') and
                projection['status']['supported'] is True and callback['envelopeSuccess'] is True and callback['success'] is True and
                callback['installed'] == (kind == 'install') and callback['ready'] == (kind == 'install'),
                'actual helper GUI action failed')
        self.gui_phase('helper-transaction-end')
        proof = self.controller.call('transaction_end')
        if self.launch_options.get('authenticationMode') == AUTH_GROUP_MODE:
            subject = self.launch_options['authorization']
            recovered = verify_joint_recovery(proof, subject, getattr(self, 'membership_gid', None))
            self.membership_gid = recovered['groupGid']
        require(proof.get('transactionBinding')==self.transaction_binding,'transaction end differs from begin')
        root_controller.validate_auxiliary_terminals(proof['auxiliaryTerminals'],self.transaction_binding,
            proof['owned'],proof['pkexecBirths'],proof['rootShellBirths'])
        coverage='incomplete' if proof['auxiliaryTerminals'] else 'complete'
        require(proof['auxiliaryIdentityCoverage']==coverage,'transaction end coverage differs')
        self.supervisor.end_window(proof)
        if coverage=='incomplete' or self.result.get('auxiliaryIdentityCoverage')!='incomplete':
            self.result['auxiliaryIdentityCoverage']=coverage
        self.driver.execute("window.__tunObservation.context=null;return true;")
        self.driver.click('.side .nav-item', self.text('settings.nav.back'))
        return {'callback': self.result['helperCallback'], 'authorization': proof}

    def connect(self):
        self.gui_phase('connect-begin')
        """TUN-only override: never weakens base CarrierRun.connect."""
        self.connect_phase('stopped-check')
        require(not self.status().get('running'), 'TUN connect requires stopped core')
        self.connect_phase('gui-click')
        self.driver.click('#connect-btn')
        return self.observe_ready_core()

    def retire_applied_core(self, previous):
        proof = self.controller.call('core_stopped')
        require(proof.get('stopped') is True and same_birth(proof.get('identity'), previous), 'old TUN core exit unconfirmed')
        self.stopped = True

    def observe_ready_core(self, timeout=75, register=True):
        self.connect_phase('status-ready')
        deadline=time.monotonic()+timeout
        status = self.wait_ready_status(timeout)
        binding_wait=self.new_status_observation('ready-binding',deadline)
        self.connect_phase('core-birth', status)
        require(status.get('startedViaHelper') is True, 'real production helper path required')
        if register:
            self.controller.call('register_core')
        attested = self.controller.call('core_attest')
        self.core = {**attested['identity'], 'sha256': attested['executable']['sha256']}
        require(status.get('pid') == self.core['pid'] and self.core['sha256'] == self.members['core']['sha256'],
                'App/core frozen helper identity differs')
        self.connect_phase('runtime-path')
        self.runtime_path = base.regular(self.runtime_path, self.home)
        require(self.runtime_path.name == 'singbox-runtime.json', 'actual private runtime path required')
        self.connect_phase('runtime-api')
        def binding():
            fresh=self.controller.call('core_attest')  # Existing read-only ledger/proc/runtime attestation.
            require(same_birth(self.core,fresh['identity']) and fresh['executable']['sha256']==self.core['sha256'],
                    'ready core changed during observation')
            endpoint,runtime,graph=self.observed_api_endpoint(expected_status=status)
            require(runtime['sha256']==fresh['runtime']['sha256'],'root/core actual runtime differs')
            return endpoint,runtime,graph,fresh
        endpoint,runtime,graph,attested=self.wait_status_proof(binding_wait,binding)
        inbounds = [i for i in graph.get('inbounds', []) if i.get('type') == 'tun']
        require(len(inbounds) == 1 and inbounds[0].get('interface_name') == 'polaris-tun0', 'original TUN interface graph required')
        self.api_endpoint = endpoint
        self.connect_phase('owned-api-probe')
        self.wait_status_proof(binding_wait,lambda:self.ready_capabilities(status))  # Read-only root owned LISTEN proof.
        if register:
            self.controller.call('tun_register')
        self.stopped = False
        self.connect_phase('receipt')
        self.result['apiEndpoint'] = {**endpoint, 'ownedListenerProbe': True}
        self.result['execution'] = {'uid': os.getuid(), 'defaultSandbox': True, 'originalEntry': True,
            'automation': 'tauri-driver 2.0.5', 'app': {k: self.app[k] for k in ('pid', 'startTime', 'sha256', 'exeDev', 'exeInode')},
            'core': self.core, 'startedViaHelper': True, 'rootAttestation': attested,
            'rpcScope': 'capabilities/status only; not six-RPC TUN acceptance'}
        self.result['configIdentity'] = runtime
        return status

    def same_core(self, baseline, status=None):
        actual = self.controller.call('core_attest')
        require(same_birth(baseline, actual['identity']) and (self.status() if status is None else status).get('pid') == baseline['pid'],
                'TUN hot operation replaced owned core')
        return actual

    def rpc(self, action, request=None):
        require(action in ('capabilities', 'status'), 'TUN sideclient only observes capabilities/status')
        args = {'method': action}
        if action == 'status':
            require(isinstance(request, dict) and set(request) == {'transactionId', 'runtimeIncarnation'},
                    'exact original transaction lookup required')
            args.update(request)
        else:
            require(request is None, 'capabilities has no request payload')
        self.revalidate_api_endpoint()
        envelope = self.controller.call('rpc', **args)
        require(envelope.get('schema') == 'polaris-package-rpc-probe-v1' and
                envelope.get('ok') is True and envelope.get('action') == 'tun-' + action,
                'root-attested restricted native RPC failed')
        self.revalidate_api_endpoint()
        return envelope['result']

    def socket(self, label):
        self.gui_phase('stream-connect-begin')
        stream = fixture.direct_connect(self.network.plan)
        self.sockets.append(stream)
        return stream, self.exchange(stream, label)

    def exchange(self, stream, label):
        self.fixtures = fixture
        proof = super().exchange(stream, label)
        self.pending_streams[proof['nonceSha256']] = proof
        return proof

    def boundary(self, label):
        row = self.controller.call('network_observe')
        self.network_observations.append({'case': label, 'observation': row})
        return row

    def preference(self, wanted):
        return super().preference(wanted)

    def graph_case(self):
        self.stage = 'graph_apply'; self.boundary(self.stage)
        initial = dict(self.core); before_active = self.rpc('capabilities')['active']
        old, before = self.socket('A')
        base.save_fixture(self.driver, self.network.changed_graph(self.config()))
        applied = self.apply()
        require(applied.get('transactionId'), 'real GUI Apply transaction required')
        status = self.native_status(applied['transactionId'])
        operation = status['operation']
        after = self.rpc('capabilities')['active']
        require(status.get('lookupState') == 1 and operation.get('durable') is True,
                'original GUI transaction not durably applied')
        self.same_core(initial)
        _, new = self.socket('B')
        self.dns_after = [{**fixture.nonce_dns_query(self.network.plan, os.urandom(16).hex(), mode),
                           'expectedBackend': 'B'} for mode in ('udp', 'tcp')]
        self.result['cases']['graph_apply'] = {'status': 'pending-capture', 'evidence': dict(
                    beforeCore=initial, afterCore=dict(self.core),
                    beforeActive=before_active, afterActive=after, appStatus=applied['status'],
                    operationDurable=operation['durable'], transactionId=applied['transactionId'],
                    oldBefore=before, oldAfter=self.exchange(old, 'A'), newConnection=new,
                    screenshot=self.snapshot('tun-applied-b'))}
        # Remove only the explicit fixture routing rules through genuine Apply;
        # subsequent ordinary user selection acts on the default selector.
        wanted = self.config()
        for key in ('trafficRules', 'policyRules', 'customRules', 'routeRuleOrder'): wanted[key] = []
        base.save_fixture(self.driver, wanted)
        self.apply(); self.same_core(initial)

    def daily_case(self):
        self.stage = 'daily_switch'; self.boundary(self.stage)
        self.pick('A'); _, first = self.socket('A'); initial = dict(self.core)
        self.pick('B'); _, fresh = self.socket('B'); self.same_core(initial)
        self.record('daily_switch', beforeCore=initial, afterCore=dict(self.core),
                    firstConnection=first, newConnection=fresh, selected='node-b')

    def auto_case(self):
        self.gui_phase('auto-begin')
        self.stage = 'auto_preserves'; self.boundary(self.stage)
        self.preference(False)
        self.activate_saved_preference(False)
        self.network.set_latency('A', 0); self.network.set_latency('B', 0.15)
        self.prepare_auto()
        old, before = self.socket('A'); initial = dict(self.core)
        # A remains alive and continues the old stream; only health latency changes.
        self.network.set_latency('A', 1.5); self.network.set_latency('B', 0)
        deadline = time.monotonic() + 340
        status_wait=self.new_status_observation('auto',deadline)
        switched=False
        while time.monotonic() < deadline:
            self.exchange(old, 'A')
            status=self.status_observation_sample(status_wait)
            if status is not None and status.get('actualServerId') == 'node-b':
                switched=True
                break
            time.sleep(3)
        require(switched and self.config().get('selectionIntent'),
                'real native Auto failed to choose B on unchanged timer')
        self.same_core(initial,status=status)
        base.gui_require(time.monotonic()<deadline,'observation-timeout')
        _, fresh = self.socket('B')
        live = self.fixture_control('A', {'action': 'snapshot'})
        require(not live.get('errors') and live.get('closed') is not True, 'old backend must remain alive')
        self.record('auto_preserves', beforeCore=initial, afterCore=dict(self.core),
            oldBefore=before, oldAfter=self.exchange(old, 'A'), newConnection=fresh,
            autoSelected='node-b', backendAAlive=True, nativeIntervalMinutes=5, screenshot=self.snapshot('tun-auto-b'))
        self.network.set_latency('A', 0); self.network.set_latency('B', 0.15)

    def manual_cases(self):
        for interrupt, name in ((False, 'manual_preserve'), (True, 'manual_disconnect')):
            self.stage = name; self.boundary(name)
            self.pick('A')  # Clear Auto intent before accepting a manual-policy graph.
            self.preference(interrupt); self.activate_saved_preference(interrupt)
            self.pick('A'); old, before = self.socket('A'); initial = dict(self.core)
            self.pick('B'); self.same_core(initial); _, fresh = self.socket('B')
            evidence = dict(beforeCore=initial, afterCore=dict(self.core), oldBefore=before,
                            newConnection=fresh, interruptConnectionsOnSwitch=interrupt)
            if not interrupt:
                evidence['oldAfter'] = self.exchange(old, 'A')
            else:
                def closed():
                    try:
                        old.settimeout(0.25)
                        result = old.recv(1)
                        require(not result, 'unexpected bytes after complete nonce response')
                        return True
                    except (BrokenPipeError, ConnectionResetError): return True
                    except socket.timeout: return False
                base.wait_for(closed, timeout=12)
                live = self.fixture_control('A', {'action': 'snapshot'})
                require(not live.get('errors') and live.get('closed') is not True, 'backend A died instead of preference disconnect')
                evidence.update(oldConnectionClosed=True, backendAAlive=True)
            self.record(name, **evidence)

    def uncovered_case(self):
        self.stage = 'uncovered_target_restart'; self.boundary(self.stage)
        self.preference(False); self.activate_saved_preference(False); self.pick('B')
        old, _ = self.socket('B'); initial = dict(self.core)
        active = self.rpc('capabilities')['active']; accepted = self.config()
        wanted = copy.deepcopy(accepted)
        # This candidate cannot be dialled: the route-binding admission must stop
        # the Apply before runtime publication and the user explicitly declines.
        next(n for n in wanted['servers'] if n['id'] == 'node-b')['address'] = '10.231.240.4'
        base.save_fixture(self.driver, wanted)
        cursor = max((r['sequence'] for r in self.tun_observations()), default=0)
        applied = self.apply('restartRequired')
        rows = [r for r in self.tun_observations() if r['sequence'] > cursor and r['kind'] == 'apply']
        require(rows and {'code': 'structuralChange', 'path': 'outbounds.physical_route_binding'} in rows[-1]['reasons'],
                'uncovered route must return original structural reason')
        self.driver.click('dialog[open][aria-labelledby="restart-apply-title"] button', self.text('home.restartApplyKeepConnection'))
        self.same_core(initial); after = self.rpc('capabilities')['active']
        preserved = self.exchange(old, 'B')
        self.record('uncovered_target_restart', beforeCore=initial, afterCore=dict(self.core),
            appStatus=applied['status'], reasonPath='outbounds.physical_route_binding', reason='structuralChange',
            restartDeclined=True, beforeActive=active, afterActive=after, oldConnectionPreserved=bool(preserved))
        base.save_fixture(self.driver, accepted)

    def disconnect(self):
        self.api_endpoint = None
        require(self.status().get('running'), 'TUN OFF requires running core')
        self.driver.click('#connect-btn')
        self.wait_status_observation('off', lambda s:s.get('running') is False and not s.get('starting'),
                                     time.monotonic()+45, phase='disconnect-status')
        proof = self.controller.call('core_stopped')
        require(proof.get('stopped') is True and same_birth(proof['identity'], self.core), 'OFF did not reap exact root-ledger core')
        self.stopped = True
        return proof

    def quit_product(self):
        self.gui_phase('quit-begin')
        self.api_endpoint = None
        require(self.stopped and not self.status().get('running'), 'quit requires verified TUN OFF')
        require(base.same_process(self.app, base.process_identity(self.app['pid'])), 'GUI exited before tray quit')
        marker = self.runtime_path.parent / 'clean-exit.marker'
        require(not marker.exists() and not marker.is_symlink(), 'stale clean exit marker')
        requested = time.time_ns(); self.driver.quit_shortcut()
        base.wait_for(lambda: base.process_identity(self.app['pid']),
            lambda p: not base.same_process(p, self.app) or p['state'] == 'Z', timeout=45)
        base.wait_for(lambda: self.supervisor.living('product'), lambda p: not p, timeout=15)
        proof = base.clean_exit_marker(marker, requested)
        try: self.driver.close()
        except (ValueError, OSError): self.driver.session = None
        self.normal_quit = True
        return {'app': birth(self.app), 'coreAlreadyStopped': birth(self.core), 'cleanExitMarker': proof}

    def start_controller(self):
        import root_controller
        self.result['rootPreparationProgress'] = {'stage': 'direct-driver', 'claim': 'entry-reached-only'}
        driver = direct_driver(self.supervisor)
        self.result['rootPreparationProgress']['stage'] = 'config-directory'
        directory = Path(self.env['XDG_CONFIG_HOME']) / 'com.polaris.app' / 'polaris'
        require(directory.is_dir() and not directory.is_symlink(), 'real App config directory unavailable')
        self.runtime_path = directory / 'singbox-runtime.json'
        self.result['rootPreparationProgress']['stage'] = 'frozen-sources'
        sources = {name: base.regular(HERE / name).read_bytes() for name in root_controller.FILES}
        self.result['rootPreparationProgress']['stage'] = 'private-probe'
        probe_hash = base.sha(self.rpc_client)
        probe = private_root_probe(self.rpc_client, self.home, probe_hash)
        launch = {'schema': root_controller.SCHEMA, 'runId': self.run_id, 'guiUid': os.getuid(),
            'driver': birth(driver), 'guiHome': str(self.home), 'helperScriptDir': str(directory),
            'runtimePath': str(self.runtime_path), 'appSha256': self.members['app']['sha256'],
            'helperSha256': self.bound['helperBinding']['helper']['sha256'],
            'coreSha256': self.members['core']['sha256'], 'carrierSha256': self.item['sha256'],
            'kernelCommit': tun.KERNEL_COMMIT, 'originalAdmission': 'verified',
            'vmContext': {'githubHosted': os.environ.get('GITHUB_ACTIONS') == 'true' and
                          os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted', 'dedicatedVM': True},
            'materialsDownloaded': True, 'codeHashes': {name: hashlib.sha256(data).hexdigest() for name, data in sources.items()},
            'rpcProbePath': str(probe), 'rpcProbeSha256': probe_hash,
            **self.launch_options}
        self.result['rootPreparationProgress']['stage'] = 'launch-contract'
        root_controller.validate_launch(launch)
        self.transaction_launch={'runId':launch['runId'],'execution':copy.deepcopy(launch['execution'])}
        self.controller = Controller()
        self.result['controllerLifecycle'] = self.controller.lifecycle
        self.result['rootPreparationProgress']['stage'] = 'controller-start'
        self.controller.start(launch, sources, self.supervisor, self.env)
        self.result['rootPreparationProgress']['stage'] = 'bind-gui'
        self.result['rootGuiBinding'] = verify_gui_binding(self.app, self.controller.call('bind_gui', pid=self.app['pid']))
        self.result['rootPreparationProgress']['stage'] = 'prepare'
        self.result['rootPreparation'] = self.controller.call('prepare')
        if self.launch_options.get('authenticationMode') == AUTH_GROUP_MODE:
            self.result['rootPreparationProgress']['stage'] = verify_group_preparation(self.result['rootPreparation'])
        else:
            self.result['rootPreparationProgress']['stage'] = 'prepared'
        # CredentialLease, if explicitly authorized, opens/closes inside each
        # controller transaction_begin/end. No password action is exposed here.

    def control_connectivity(self, label):
        require(label in ('before', 'after'), 'bounded control proof position')
        child = self.supervisor.spawn(['/usr/bin/python3', '-I', '-c', CONNECTIVITY_PROBE],
                                      self.env, 'control-connectivity', stdout=subprocess.PIPE)
        identity = self.supervisor.owned[child.pid]
        try:
            raw = child.communicate(timeout=15)[0]
        except subprocess.TimeoutExpired:
            self.supervisor.forced.append({**birth(identity), 'reason': 'control-connectivity-timeout'})
            self.supervisor.signal(identity, signal.SIGKILL); child.wait(timeout=5)
            raise ValueError('bounded fresh control connectivity timed out') from None
        require(child.returncode == 0 and 0 < len(raw) < 32768, 'fresh control connectivity failed')
        value = json.loads(raw)
        require(value.get('host') == 'api.github.com' and value.get('dnsResolved') is True and
                value.get('httpsStatus') == 200 and value.get('tlsVerified') is True,
                'fresh DNS/HTTPS proof unavailable')
        require(int(value['readFinishedUnixNs']) >= int(value['readStartedUnixNs']), 'control observation clock differs')
        return {**value, 'position': label, 'freshProcess': birth(identity)}

    def special_reply(self):
        with socket.create_connection((self.network.plan['special'], self.network.plan['ports']['special']), timeout=5) as stream:
            value = b''
            while not value.endswith(b'\n') and len(value) <= 64:
                part = stream.recv(64)
                require(part, 'synthetic route reply incomplete')
                value += part
        require(value == b'synthetic-route-only\n', 'synthetic route reply differs')
        return value.decode().strip()

    def capture_results(self):
        self.stage = 'capture-analysis'
        proof = self.controller.call('capture_stop')
        target = self.workspace / 'private-capture.pcap'
        fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        offset, expected = 0, None
        with os.fdopen(fd, 'wb') as stream:
            while True:
                row = self.controller.call('capture_read', offset=offset)
                identity = (row['sha256'], row['bytes'])
                require(expected in (None, identity) and row['offset'] == offset, 'root capture identity/offset drift')
                expected = identity
                raw = base64.b64decode(row['base64'], validate=True)
                require(row['nextOffset'] == offset + len(raw) and 0 < len(raw) <= 32768, 'bounded capture chunk differs')
                stream.write(raw); offset += len(raw)
                require(offset <= row['bytes'] <= 16 * 1024 * 1024, 'capture size bound')
                if row['eof']:
                    require(offset == row['bytes'], 'capture EOF differs'); break
        require(base.sha(target) == expected[0], 'transferred capture byte hash differs')
        analysis = fixture.analyze_capture(target, self.network.plan, supervisor=self.supervisor)
        require(analysis['captureSha256'] == expected[0], 'analysis no longer bound to root capture')
        snapshots = {role: self.fixture_control(role, {'action': 'snapshot'}) for role in ('A', 'B', 'O')}
        joined, dns = join_capture(analysis, snapshots, self.pending_streams, self.dns_before + self.dns_after, self.network.plan)
        # Every business-case nonce must join the actual packet/origin evidence,
        # not merely two convenient streams used for the summary case.
        require(len(joined) == len(self.pending_streams), 'some GUI-case stream not captured')
        self.result['dataPlaneEvidence'] = {'capture': {'sha256': expected[0], 'bytes': expected[1]},
            'analysis': analysis, 'joinedStreams': joined, 'backendSnapshots': snapshots,
            'captureStop': proof}
        selected = {}
        for label in ('A', 'B'):
            selected[label] = next(row for row in joined.values() if row['exit'] == label)
        before_names = {r['name'] for r in self.dns_before}
        dns_before = [r for r in dns if r['name'] in before_names]
        dns_after = [r for r in dns if r['name'] not in before_names]
        graph = self.result['cases']['graph_apply']['evidence']
        self.record('graph_apply', **graph, dnsBefore=dns_before, dnsAfter=dns_after)
        self.record('tun_data_plane', **selected, dns=dns_after)

    def run(self):
        root_finish = None
        cleanup = {'state': 'failed', 'forced': False, 'unknown': ['normal-closure-not-established']}
        try:
            self.stage = 'native-launch'
            self.start_tools(); initial = self.launch()
            require(initial.get('singboxDashboard') is True and initial.get('hardwareAcceleration') is not False,
                    'empty HOME original dashboard/graphics default differs')
            self.result['defaultObservation'] = {'emptyHome': True, 'dashboardEnabled': True,
                                                 'screenshot': self.snapshot('tun-default')}
            desired = copy.deepcopy(initial)
            desired.update(language='en-US', autoConnect=False)
            base.save_fixture(self.driver, desired)
            base.wait_for(lambda: self.driver.execute('return document.documentElement.lang'), lambda s: s.startswith('en'))
            self.stage = 'baseline-control-connectivity'
            self.result['controlConnectivity'] = {'before': self.control_connectivity('before')}
            self.stage = 'root-preparation'
            self.start_controller()
            self.stage = 'helper-install'
            self.result['helperInstall'] = self.helper_action('install')
            self.result['helperIdentity'] = self.controller.call('register_helper')
            self.stage = 'fixture-network'
            network = self.controller.call('network_start')
            require(network['topology'] == self.network.plan, 'root fixture topology differs')
            before = self.boundary('synthetic-before')
            self.special_reply()
            patch = self.network.config_patch(self.mixed, self.fixture_control_port, self.secret)
            patch.update(language='en-US', autoConnect=False, autoSelectEnabled=True,
                         periodicSpeedTestEnabled=False, interruptConnectionsOnSwitch=False)
            patch['subscriptions'] = [{'id': 'package-fixture', 'name': 'Package isolated TUN',
                'url': self.network.health_url, 'autoUpdate': False, 'createdAt': '2026-01-01T00:00:00Z',
                'periodicSpeedTest': False, 'speedTestIntervalMinutes': 5}]
            for node in patch['servers']: node['subscriptionId'] = 'package-fixture'
            desired = self.config(); desired.update(patch); saved_fixture=base.save_fixture(self.driver, desired)
            base.gui_require(type(saved_fixture.get('autoSelectEnabled')) is bool,'fixture-auto-mode-schema-invalid')
            self.result['fixtureAutoSetup']={'requested':True,'saved':saved_fixture['autoSelectEnabled'],
                'authority':'backend-owned-not-config-save','sameAsRequested':saved_fixture['autoSelectEnabled'] is True}
            self.stage = 'default-graph'
            self.pick('A'); status = self.connect()
            runtime = self.runtime_identity()
            self.record('default_graph', dashboardEnabled=True, defaultSandbox=True, guiUid=os.getuid(),
                startedViaHelper=status['startedViaHelper'], kernelCommit=tun.KERNEL_COMMIT,
                originalEntry=True, stableDoubleRead=runtime['stableDoubleRead'], actualGraphSha256=runtime['sha256'],
                screenshot=self.snapshot('tun-started'))
            self.controller.call('capture_start')
            during = self.boundary('synthetic-during'); reply = self.special_reply()
            self.socket('A')
            self.dns_before = [{**fixture.nonce_dns_query(self.network.plan, os.urandom(16).hex(), mode),
                                'expectedBackend': 'A'} for mode in ('udp', 'tcp')]
            self.graph_case(); self.daily_case(); self.auto_case(); self.manual_cases(); self.uncovered_case()
            for stream in self.sockets: stream.close()
            self.sockets.clear()
            self.capture_results()
            self.stage = 'gui-off'
            self.result['off'] = self.disconnect()
            after = self.boundary('synthetic-after'); self.special_reply()
            self.record('synthetic_route', destination=self.network.plan['special'],
                before=before['specialRoute'], during=during['specialRoute'], after=after['specialRoute'],
                actualReply=reply, claim='synthetic-specific-route-only')
            self.stage = 'helper-uninstall'
            self.result['helperUninstall'] = self.helper_action('uninstall')
            self.result['productUninstalled'] = self.controller.call('product_uninstalled')
            self.stage = 'tray-quit'
            self.result['trayQuit'] = self.quit_product()
            self.stage = 'fixture-normal-stop'
            for role in ('A', 'B', 'O'):
                closed = self.fixture_control(role, {'action': 'stop'})
                require(closed.get('closed') is True and not closed.get('errors'), 'fixture failed normal native shutdown')
            self.stage = 'authentication-close'
            self.result['authenticationClosed'] = self.controller.call('auth_close')
            if self.launch_options.get('authenticationMode') == AUTH_GROUP_MODE:
                subject = self.launch_options['authorization']
                require(getattr(self, 'membership_gid', None) is not None, 'no completed membership lease')
                verify_group_recovery(self.result['authenticationClosed'].get('groupRecovery'),
                                      subject['guiUid'], subject['account'], self.membership_gid)
            self.stage = 'root-normal-cleanup'
            cleanup = self.controller.call('cleanup')
            self.stage = 'root-finish'
            root_finish = self.controller.finish()
            # Evidence booleans originate only from controller postconditions.
            cleanup = {**cleanup, 'watchdogCancelled': root_finish['watchdogCancelled'],
                       'authenticationWindowClosed': root_finish['authWindowClosed']}
            self.root_cleanup = cleanup
            self.stage = 'restored-control-connectivity'
            self.result['controlConnectivity']['after'] = self.control_connectivity('after')
        except BaseException as error:
            self.result['failure'] = {'stage': self.stage, 'kind': type(error).__name__, 'code': 'TUN_RUNTIME_FAILED', 'guiFailure': base.gui_failure(error)}
            self.capture_driver_failure()
            if self.controller and not self.controller.closed:
                cleanup = self.controller.recover()
        finally:
            for stream in self.sockets:
                try: stream.close()
                except OSError: pass
            ordinary = self.supervisor.close(self.normal_quit and root_finish is not None)
            self.result['ordinaryCleanup'] = ordinary
            self.result['ownedNetworkObservation'] = self.supervisor.network_evidence()
            self.result['networkObservations'] = self.network_observations
            if ordinary['state'] != 'normal':
                cleanup = {**cleanup, 'state': 'failed',
                           'unknown': list(cleanup.get('unknown', [])) + ['ordinary-product-or-tool-cleanup-failed']}
            if cleanup.get('state') == 'normal' and 'failure' not in self.result:
                self.record('normal_cleanup', **cleanup)
            result = tun.report_result(self.result['cases'], cleanup, 'same-final-carrier')
            self.result.update(result)
        return self.result


def node_environment(environ):
    # Preserve ALL source/gate flags (including empty/0 legacy values) so the
    # original formal gate rejects them; never sanitize an invalid run to valid.
    prefixes = ('GITHUB_', 'RUNNER_', 'PACKAGE_ACCEPTANCE_', 'PACKAGE_TUN_',
                'POLARIS_', 'CORE_', 'ADMITTED_')
    env = {k: v for k, v in environ.items() if k.startswith(prefixes)
           and not any(word in k for word in ('TOKEN', 'SECRET', 'PASSWORD', 'CREDENTIAL'))}
    env.update(PATH=environ.get('PATH', '/usr/bin:/bin'), HOME=str(Path.home()))
    return env


def run_node(args, supervisor, stdin=None):
    # Context is inherited only by admission tools, never App/root environments.
    env = node_environment(os.environ)
    with supervisor.lock:
        child = subprocess.Popen(['node', str(HERE / 'tun_transport.mjs'), *map(str, args)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, cwd=base.ROOT)
        identity = base.process_identity(child.pid)
        require(identity, 'transport process birth unavailable')
        supervisor.children[child.pid] = child; supervisor.owned[child.pid] = identity
        supervisor.roles[child.pid] = 'admission'
    raw = None if stdin is None else json.dumps(stdin).encode()
    try: output = child.communicate(raw, timeout=45)[0]
    except subprocess.TimeoutExpired:
        supervisor.forced.append({**birth(identity), 'reason': 'transport-timeout'})
        supervisor.signal(identity, signal.SIGKILL); child.wait(timeout=5)
        raise ValueError('transport operation timeout') from None
    require(child.returncode == 0 and len(output) < 1024 * 1024, 'real transport admission failed')
    return json.loads(output) if output.strip() else None


ENTRY_FAILURE_CODES = {
    'construct': 'TUN_CONSTRUCTION_FAILED',
    'inspect-helper': 'TUN_INSPECTION_FAILED',
    'bind-helper': 'TUN_BIND_HELPER_FAILED',
    'authorize': 'TUN_AUTHORIZE_FAILED',
    'runtime': 'TUN_RUNTIME_BOUNDARY_FAILED',
    'post-runtime-carrier': 'TUN_CARRIER_RECHECK_FAILED',
    'binding-copy': 'TUN_BINDING_COPY_FAILED',
}


def run_all(args):
    import pwd
    admission_path = base.regular(args.admission)
    admission = json.loads(admission_path.read_bytes())
    require(admission.get('schema') == 'polaris-linux-tun-admission-v1' and
            admission.get('kernelCommit') == tun.KERNEL_COMMIT and
            admission.get('sourceScope') == 'same-final-carrier' and admission.get('publicationEligible') is False,
            'real final-carrier TUN admission required')
    require(base.sha(base.regular(args.transport)) == admission['transportSha256'], 'original transport changed')
    item = admission['carrier']; require(item['kind'] == args.kind, 'one fresh VM per carrier required')
    package = base.carrier_path(Path(args.packages), item)
    output = Path(args.output).absolute(); output.parent.mkdir(parents=True, exist_ok=True)
    require(not output.is_symlink(), 'redirected report prohibited')
    previous = {sig: signal.signal(sig, base.cancellation) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        workspace = Path(tempfile.mkdtemp(prefix='polaris-tun-', dir='/tmp'))
        result = None
        try:
            run = None
            entry_stage = 'construct'
            try:
                workspace.chmod(0o700)
                run = TunCarrierRun(item, package, workspace, output.parent, base.regular(args.rpc_client), admission, None, {})
                entry_stage = 'inspect-helper'
                inspection_path = workspace / 'inspection.json'
                inspection_path.write_text(json.dumps(inspect_helper(run.payload, run.members)))
                bound_path = workspace / 'bound.json'
                entry_stage = 'bind-helper'
                run_node(['bind-helper', admission_path, inspection_path, bound_path], run.supervisor)
                bound_bytes = bound_path.read_bytes()
                run.bound = json.loads(bound_bytes)
                require(run.bound['schema'] == 'polaris-linux-tun-bound-v1' and run.bound['admission'] == admission,
                        'helper binding changed admission')
                entry_stage = 'authorize'
                run.launch_options = run_node(['authorize', admission_path], run.supervisor,
                    {'runToken': run.run_id, 'guiUid': os.getuid(), 'account': pwd.getpwuid(os.getuid()).pw_name})
                entry_stage = 'runtime'
                result = run.run()
                entry_stage = 'post-runtime-carrier'
                require(base.sha(package) == item['sha256'], 'carrier bytes drifted during TUN run')
                entry_stage = 'binding-copy'
                binding_copy = output.parent / (args.kind + '-bound.json')
                require(not binding_copy.exists() and not binding_copy.is_symlink(), 'fresh public binding copy required')
                binding_copy.write_bytes(bound_bytes)
                result.update(binding=run.bound, bindingFile=binding_copy.name,
                    admissionSha256=base.sha(admission_path), bindingSha256=base.sha(binding_copy),
                    transportSha256=admission['transportSha256'], carrierKind=item['kind'], carrierSha256=item['sha256'],
                    context=admission['context'], scope='same-final-carrier', publicationEligible=False)
            except BaseException as error:
                # This boundary also covers runtime/post-runtime exceptions.
                # Reached-stage diagnostics do not establish absence of auth or cleanup.
                try:
                    ordinary = run.supervisor.close(False) if run is not None else {
                        'state': 'failed', 'forced': [], 'remaining': [], 'unknown': ['construction-unconfirmed']}
                except BaseException:
                    ordinary = {'state': 'failed', 'forced': [], 'remaining': [],
                                'unknown': ['ordinary-cleanup-unconfirmed']}
                # Preserve earlier runtime/root/ordinary evidence if a second
                # exception occurs at cleanup or the post-runtime binding boundary.
                prior = result if isinstance(result, dict) else getattr(run, 'result', None)
                result = dict(prior) if isinstance(prior, dict) else {}
                cleanup = dict(result.get('cleanup', {}))
                cleanup.update(state='failed', forced=cleanup.get('forced', False),
                    unknown=list(cleanup.get('unknown', [])) + ['admission-or-authorization-unconfirmed'])
                result.update(tun.report_result(result.get('cases', {}), cleanup, 'same-final-carrier',
                                                result.get('blocked', [])))
                entry_failure = {'schema': 'polaris-linux-tun-entry-failure-v1', 'kind': type(error).__name__, 'stage': entry_stage,
                                 'code': ENTRY_FAILURE_CODES[entry_stage]}
                result['entryFailure'] = entry_failure
                result.setdefault('failure', entry_failure)
                if 'ordinaryCleanup' in result:
                    result['boundaryOrdinaryCleanup'] = ordinary
                else:
                    result['ordinaryCleanup'] = ordinary
            result['entryProgress'] = {'schema': 'polaris-linux-tun-entry-progress-v1',
                'lastReached': entry_stage, 'claim': 'entry-reached-only-not-completion'}
            return result
        finally:
            ordinary_rows = [(result or {}).get('ordinaryCleanup', {})]
            if 'boundaryOrdinaryCleanup' in (result or {}):
                ordinary_rows.append(result['boundaryOrdinaryCleanup'])
            ordinary_stopped = all(ordinary.get('remainingAssessment') == 'observed' and
                                   ordinary.get('remaining') == [] and ordinary.get('unknown') == []
                                   for ordinary in ordinary_rows)
            # TunSupervisor's unknown list includes unreaped quarantine/controller
            # births. A controller also needs affirmative privileged reaping proof;
            # absent evidence keeps the workspace, never attempts mount cleanup.
            privileged_stopped = (run is not None and (run.controller is None or
                (run.controller.closed is True and
                 (result or {}).get('cleanup', {}).get('privilegedProcessesReaped') is True)))
            workspace_cleanup = base.cleanup_workspace(workspace,
                processes_stopped=ordinary_stopped and privileged_stopped)
            if result is not None:
                result['workspaceCleanup'] = workspace_cleanup
                if workspace_cleanup.get('state') != 'normal':
                    cleanup = dict(result['cleanup'])
                    cleanup.update(state='failed', unknown=list(cleanup.get('unknown', [])) +
                                   ['workspace-cleanup-unconfirmed'])
                    result.update(tun.report_result(result['cases'], cleanup, 'same-final-carrier',
                                                    result.get('blocked', [])))
                output.write_text(json.dumps(result, indent=2) + '\n')
    finally:
        for sig, old in previous.items(): signal.signal(sig, old)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    run = sub.add_parser('run')
    for name in ('admission', 'transport', 'packages', 'rpc-client', 'output'):
        run.add_argument('--' + name, required=True)
    run.add_argument('--kind', choices=('deb', 'AppImage'), required=True)
    args = parser.parse_args()
    result = run_all(args)
    raise SystemExit(0 if result['result'] == 'success' else 1)


if __name__ == '__main__':
    main()
