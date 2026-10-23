"""Pure/mock controller contracts; never execute root/product/account/network tools."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import types
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('test_root_controller_module', HERE / 'root_controller.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
H = 'a' * 64


def launch(mode='existing-session'):
    now = int(time.time())
    execution = {'repository': 'owner/Polaris', 'sourceSha': 'b' * 40, 'sourceTree': 'c' * 40,
                 'githubRunId': '12345', 'githubRunAttempt': '1', 'actor': 'owner',
                 'workflowRef': 'owner/Polaris/.github/workflows/package.yml@refs/heads/reviewed', 'eventSha256': H}
    value = {'schema': m.SCHEMA, 'runId': '0123456789ab', 'guiUid': 1001,
             'driver': {'pid': 20, 'startTime': '1234'}, 'guiHome': '/tmp/qa/home',
             'helperScriptDir': '/tmp/qa/home/config/polaris', 'runtimePath': '/tmp/qa/home/config/polaris/singbox-runtime.json',
             'appSha256': H, 'helperSha256': H, 'coreSha256': H, 'carrierSha256': H,
             'kernelCommit': 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a', 'originalAdmission': 'verified',
             'vmContext': {'githubHosted': True, 'dedicatedVM': True}, 'materialsDownloaded': True,
             'codeHashes': dict.fromkeys(m.FILES, H), 'authenticationMode': mode, 'execution': execution,
             'authorization': None, 'rpcProbePath': '/tmp/admitted-test-probe', 'rpcProbeSha256': H}
    if mode == m.AUTH_MODE:
        value['authorization'] = {'schema': 'polaris-tun-credential-lease-v1', 'phrase': m.AUTH_PHRASE,
            'authMode': m.AUTH_MODE, 'execution': copy.deepcopy(execution), 'runToken': value['runId'],
            'guiUid': value['guiUid'], 'account': 'runner', 'transactions': ['install', 'uninstall'],
            'notBeforeUnix': now - 1, 'expiresUnix': now + 899, 'maxLeaseSeconds': 90}
    return value


def identity(pid=50, uid=1001):
    return {'pid': pid, 'startTime': '2345', 'state': 'S', 'ppid': 20, 'uid': uid,
            'exeDev': 1, 'exeInode': 77, 'exe': '/usr/bin/test-app', 'capEff': 0, 'capAmb': 0,
            'namespaces': {'net': 1, 'mnt': 2, 'pid': 3, 'cgroup': 4}, 'cgroup': '0::/test.slice'}


class Ledger:
    def __init__(self):
        self.value = {'phase': 'guarded', 'result': 'incomplete', 'forcedCleanup': False, 'unknown': [],
                      'processes': [], 'transactionProcesses': [], 'deadlineMonotonic': time.monotonic() + 600,
                      'authenticationWatchdogActive': True,
                      'authentication': {'accountMutated': False, 'closed': False, 'agent': None}}
        self.children = {}; self.ops = Mock(); self.ops.proc.return_value = None
        self.directory = Path('/root/not-executed'); self.run_id = '0123456789ab'
    def load(self): return copy.deepcopy(self.value)
    def mutate(self, change): change(self.value); return self.load()


def controller(mode='existing-session'):
    tun = Mock(); tun.Commands.return_value = Mock()
    tun.same_process.side_effect = lambda a, b: bool(a and b and all(a.get(k) == b.get(k) for k in ('pid','startTime','uid','exeDev','exeInode','namespaces','cgroup')))
    c = m.Controller(launch(mode), tun, Path('/root/not-executed'))
    c.ledger = Ledger(); c.ops = c.ledger.ops
    c.gui = identity(); return c


class AbortLeaseRegressionTests(unittest.TestCase):
    def test_existing_abort_fail_path_closes_lease_before_auth_and_product(self):
        c = controller(m.AUTH_MODE); order = []
        c.ledger.value['authentication']['accountMutated'] = True
        c.lease = Mock()
        def close_lease():
            order.append('lease')
            c.ledger.value['authentication']['accountMutated'] = False
        c.lease.close.side_effect = close_lease
        def auth(*args, **kwargs):
            self.assertFalse(c.ledger.load()['authentication']['accountMutated'])
            order.append('auth')
        c.tun.cleanup.side_effect = lambda *a, **kw: order.append('product') or {'state': 'forced'}
        with patch.object(m, 'close_auth', side_effect=auth), patch.object(m, 'close_transactions'):
            result = c.abort()
        self.assertEqual(order, ['lease', 'auth', 'product'])
        self.assertEqual(result['state'], 'failed')
        self.assertTrue(result['forced']); self.assertTrue(c.failed)
        self.assertEqual(c.ledger.load()['result'], 'failed')

    def test_relock_failure_does_not_claim_recovery_or_cancel_watchdog(self):
        c = controller(m.AUTH_MODE)
        c.ledger.value['authentication']['accountMutated'] = True
        c.lease = Mock(); c.lease.close.side_effect = ValueError('relock unproved')
        c.tun.cleanup.return_value = {'state': 'failed'}
        with patch.object(m, 'close_transactions'):
            result = c.abort()
        c.lease.close.assert_called_once()
        self.assertEqual(result['state'], 'failed')
        self.assertIn('authentication-cleanup-unknown', result['unknown'])
        self.assertTrue(c.ledger.load()['authentication']['accountMutated'])
        self.assertTrue(c.ledger.load()['authenticationWatchdogActive'])
        c.ops.run.assert_not_called()
        c.tun.cleanup.assert_called_once_with(c.ledger, forced=True)


class LaunchTests(unittest.TestCase):
    def test_valid_closed_default(self): self.assertIs(m.validate_launch(v := launch()), v)
    def test_valid_permission_is_not_authentication(self):
        v = launch(m.AUTH_MODE); self.assertIs(m.validate_launch(v), v)
        self.assertNotIn('authenticated', v['authorization'])
    def test_unknown_fields_denied(self):
        v = launch(); v['command'] = '/bin/sh'
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_bad_code_inventory(self):
        for field in ('omit', 'extra'):
            v = launch()
            if field == 'omit': del v['codeHashes']['fixtures.py']
            else: v['codeHashes']['evil.py'] = H
            with self.assertRaises(ValueError): m.validate_launch(v)
    def test_root_gui_denied(self):
        for uid in (0, -1, True, '1001'):
            v = launch(); v['guiUid'] = uid
            with self.assertRaises(ValueError): m.validate_launch(v)
    def test_runtime_outside_home_denied(self):
        for p in ('/etc/shadow', '/tmp/qa/home/../other', 'relative'):
            v = launch(); v['runtimePath'] = p
            with self.assertRaises(ValueError): m.validate_launch(v)
    def test_wrong_kernel_denied(self):
        v = launch(); v['kernelCommit'] = '814' + 'a' * 37
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_boolean_authority_denied(self):
        v = launch(m.AUTH_MODE); v['authorization'] = True
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_permission_absent_denied(self):
        v = launch(m.AUTH_MODE); v['authorization'] = None
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_permission_exact_phrase(self):
        v = launch(m.AUTH_MODE); v['authorization']['phrase'] = 'allow TUN'
        with self.assertRaises(ValueError): m.validate_permission(v)
    def test_permission_source_run_actor_bindings(self):
        for field in m.EXECUTION_KEYS:
            v = launch(m.AUTH_MODE); v['authorization']['execution'][field] += 'x'
            with self.assertRaises(ValueError): m.validate_permission(v)
    def test_permission_uid_account_actions(self):
        for field, bad in (('guiUid', 0), ('runToken', 'f' * 12), ('account', 'root:evil'),
                           ('transactions', ['install']), ('maxLeaseSeconds', 999)):
            v = launch(m.AUTH_MODE); v['authorization'][field] = bad
            with self.assertRaises(ValueError): m.validate_permission(v)
    def test_permission_times(self):
        now = int(time.time())
        for begin, end in ((now + 10, now + 100), (now - 100, now - 1), (now, now + 10), (now, now + 901)):
            v = launch(m.AUTH_MODE); v['authorization'].update(notBeforeUnix=begin, expiresUnix=end)
            with self.assertRaises(ValueError): m.validate_permission(v)
    def test_recovery_does_not_expire_relock_authority(self):
        v = launch(m.AUTH_MODE); v['authorization'].update(notBeforeUnix=1, expiresUnix=500)
        with self.assertRaises(ValueError): m.validate_launch(v)
        self.assertIs(m.validate_launch(v, recovery=True), v)
    def test_bootstrap_constant_no_root_import_runner_paths(self):
        self.assertNotIn('import root_controller', m.BOOTSTRAP)
        self.assertIn('os.O_EXCL|os.O_NOFOLLOW', m.BOOTSTRAP)
        self.assertIn("os.execve('/usr/bin/python3'", m.BOOTSTRAP)
        self.assertIn("d/'root_controller.py'", m.BOOTSTRAP)
    def test_envelope_requires_exact_source_hash(self):
        with tempfile.TemporaryDirectory() as d:
            v = launch()
            for name in m.FILES: Path(d, name).write_text('test')
            with self.assertRaises(ValueError): m.bootstrap_envelope(v, d)
            v['codeHashes'] = dict.fromkeys(m.FILES, m.sha(b'test'))
            e = json.loads(m.bootstrap_envelope(v, d)); self.assertEqual(set(e['files']), set(m.FILES))


class ProtocolTests(unittest.TestCase):
    def test_fixed_action_no_exec_kill_path(self):
        for action in ('exec', 'kill', 'password', 'register_pid', 'read_file', 'apply', 'plan'):
            with self.assertRaises(ValueError): controller().dispatch({'seq': 1, 'action': action})
    def test_sequence_and_bool_rejected(self):
        for seq in (0, 2, True, '1'):
            with self.assertRaises(ValueError): controller().dispatch({'seq': seq, 'action': 'core_attest'})
    def test_extra_path_denied(self):
        with self.assertRaises(ValueError): controller().dispatch({'seq': 1, 'action': 'capture_read', 'offset': 0, 'path': '/etc/shadow'})
    def test_replay_denied(self):
        c = controller(); c.core_attest = Mock(return_value={})
        c.dispatch({'seq': 1, 'action': 'core_attest'})
        with self.assertRaises(ValueError): c.dispatch({'seq': 1, 'action': 'core_attest'})
    def test_failed_cannot_resume(self):
        c = controller(); c.failed = True
        with self.assertRaises(ValueError): c.dispatch({'seq': 1, 'action': 'core_attest'})
    def test_rpc_exact_fields(self):
        c = controller(); c.rpc = Mock(return_value={})
        c.dispatch({'seq': 1, 'action': 'rpc', 'method': 'status', 'transactionId': 't', 'runtimeIncarnation': 'i'})
        c.rpc.assert_called_once_with('status', 't', 'i')
        with self.assertRaises(ValueError): c.dispatch({'seq': 2, 'action': 'rpc', 'method': 'capabilities', 'secret': 'x'})
    def test_reserved_recovery_margin(self):
        c = controller(); c.ledger.value['deadlineMonotonic'] = time.monotonic() + 29
        with self.assertRaises(ValueError): c.guard()
    def test_missing_auth_watchdog(self):
        c = controller(); c.ledger.value['authenticationWatchdogActive'] = False
        with self.assertRaises(ValueError): c.guard()
    def test_existing_readiness_does_not_assume_agent(self):
        c = controller()
        with patch.object(m.subprocess, 'run', return_value=types.SimpleNamespace(returncode=1)) as run:
            with self.assertRaises(ValueError): c.authentication_readiness()
            self.assertNotIn('--allow-user-interaction', run.call_args.args[0])
        with patch.object(m.subprocess, 'run', return_value=types.SimpleNamespace(returncode=0)):
            self.assertFalse(c.authentication_readiness()['authenticated'])
    def test_no_network_before_actual_install_auth(self):
        c = controller(); c.ledger.value['authentication']['authorized'] = False
        with self.assertRaises(ValueError): c.network_start()
    def test_register_helper_not_from_bool_only(self):
        c = controller(); c.ledger.value['authentication']['authorized'] = True
        with self.assertRaises((ValueError, IndexError)): c.dispatch({'seq': 1, 'action': 'register_helper'})
    def test_root_core_registration_does_not_take_pid(self):
        with self.assertRaises(ValueError): controller().dispatch({'seq': 1, 'action': 'register_core', 'pid': 1})
    def test_core_stopped_reuse_rejected(self):
        c = controller(); c.ledger.value['processes'] = [{'role': 'core', 'identity': identity()}]
        c.ops.proc.return_value = {**identity(), 'startTime': 'changed'}
        with self.assertRaises(ValueError): c.core_stopped()
        c.ops.proc.return_value = None; self.assertTrue(c.core_stopped()['stopped'])
    def test_capture_not_finished_or_replayed_denied(self):
        c = controller()
        with self.assertRaises(ValueError): c.capture_read(0)
        c.ledger.value['capture'] = {'sha256': H}; c.capture_offset = 10
        with self.assertRaises(ValueError): c.capture_read(0)
    def test_rpc_other_methods_denied_before_process_access(self):
        for action in ('apply', 'retry', 'ack', 'plan', 'suite'):
            with self.assertRaises(ValueError): controller().rpc(action)
    def test_transaction_begun_cannot_run_network(self):
        c = controller(); c.observer = Mock()
        with self.assertRaises(ValueError): c.dispatch({'seq': 1, 'action': 'network_start'})


class TimerTests(unittest.TestCase):
    def test_machine_duration_parser(self):
        self.assertEqual(m.duration_seconds('1min 30s'), 90)
        self.assertAlmostEqual(m.duration_seconds('1h 3ms'), 3600.003)
        for value in ('infinity', '90', '90s evil', 'NaN s'):
            with self.assertRaises(ValueError): m.duration_seconds(value)
    def timer(self, **change):
        unit = 'polaris-tun-lease-0123456789ab-install'; argv = ['/usr/bin/env', '-i', '/usr/bin/python3', '-I', '/run/frozen/controller.py']
        service = {'ExecStart': '{ path=/usr/bin/env ; argv[]=' + ' '.join(argv) + ' ; }', 'User': '', 'Group': '',
                   'DynamicUser': 'no', 'FragmentPath': '/run/systemd/transient/' + unit + '.service', 'DropInPaths': '', 'ActiveState': 'inactive'}
        timer = {'ActiveState': 'active', 'Unit': unit + '.service', 'TimersMonotonic': '[ { OnActiveUSec=1min 30s ; next_elapse=3min 10s } ]'}
        service.update(change.get('service', {})); timer.update(change.get('timer', {}))
        ops = Mock(); ops.run.side_effect = ['\n'.join(k+'='+v for k,v in service.items()), '\n'.join(k+'='+v for k,v in timer.items())]
        with patch.object(m.time, 'monotonic', return_value=100):
            return m.verify_timer(ops, types.SimpleNamespace(SYSTEMCTL='/usr/bin/systemctl'), unit, argv, 90, 100)
    def test_actual_timer_shape(self): self.assertTrue(self.timer()['rootFixedCommandVerified'])
    def test_wrong_service_command_user_dropin_failed(self):
        for field, value in (('ExecStart', '/bin/sh'), ('User', 'runner'), ('DynamicUser', 'yes'),
                             ('DropInPaths', '/etc/override'), ('ActiveState', 'active'), ('FragmentPath', '/tmp/unit')):
            with self.assertRaises(ValueError): self.timer(service={field: value})
    def test_timer_skip_cancel_wrong_deadline_failed(self):
        for field, value in (('ActiveState', 'inactive'), ('Unit', 'other.service'),
                             ('TimersMonotonic', '[ { OnActiveUSec=90s ; next_elapse=999s } ]')):
            with self.assertRaises(ValueError): self.timer(timer={field: value})


class CredentialTests(unittest.TestCase):
    def account(self, status='L', duplicate=False, admin=True, recovery=False):
        user = types.SimpleNamespace(pw_uid=1001, pw_name='runner', pw_shell='/bin/bash', pw_dir='/home/runner', pw_gid=1001, pw_gecos='')
        group = types.SimpleNamespace(gr_name='sudo' if admin else 'users', gr_mem=['runner'], gr_gid=2)
        with patch.object(m.pwd, 'getpwall', return_value=[user, user] if duplicate else [user]), \
             patch.object(m.pwd, 'getpwnam', return_value=user), patch('grp.getgrall', return_value=[group]), \
             patch.object(m, 'account_status', return_value=status):
            return m.credential_account(launch(m.AUTH_MODE), Mock(), recovery=recovery)
    def test_original_locked_or_no_password_only(self):
        self.assertEqual(self.account('L')[1], 'L'); self.assertEqual(self.account('NP')[1], 'NP')
        with self.assertRaises(ValueError): self.account('P')
    def test_no_new_account_or_admin_membership(self):
        with self.assertRaises(ValueError): self.account(duplicate=True)
        with self.assertRaises(ValueError): self.account(admin=False)
    def test_recovery_still_locks_if_membership_changed(self): self.assertEqual(self.account('P', admin=False, recovery=True)[1], 'P')
    def test_relock_fixed_existing_account_stdin_no_secret(self):
        user = types.SimpleNamespace(pw_name='runner', pw_uid=1001)
        with patch.object(m, 'credential_account', return_value=(user, 'P')), patch.object(m, 'account_status', return_value='L'), \
             patch.object(m.subprocess, 'run') as run:
            self.assertEqual(m.relock_account(launch(m.AUTH_MODE), Mock())['finalPasswordStatus'], 'L')
            self.assertEqual(run.call_args.args[0], ['/usr/bin/passwd', '-l', 'runner'])
            self.assertNotIn('input', run.call_args.kwargs)
    def test_relock_requires_actual_l(self):
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'P')), \
             patch.object(m, 'account_status', return_value='P'), patch.object(m.subprocess, 'run'):
            with self.assertRaises(ValueError): m.relock_account(launch(m.AUTH_MODE), Mock())
    def test_no_setting_before_real_agent_registration(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install')
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'L')), \
             patch.object(m.subprocess, 'run') as run:
            with self.assertRaises(ValueError): lease.open()
            run.assert_not_called()
    def test_setting_unknown_always_attempts_relock(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install')
        c.agent = Mock(); c.ledger.value['authentication']['registered'] = True
        c.ops.run.return_value = 'inactive'
        writer = Mock(pid=89); writer.communicate.side_effect = subprocess.TimeoutExpired('chpasswd', 10); writer.poll.return_value = None
        c.ops.proc.return_value = {**identity(89, 0), 'ppid': os.getpid()}
        c.tun.executable_identity.return_value = c.tun.file_identity.return_value = {'sha256': H}
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'L')), \
             patch.object(m, 'verify_timer'), patch.object(m, 'account_status', return_value='L'), \
             patch.object(m, 'setting_intent'), patch.object(m, 'independent_record'), \
             patch.object(m.subprocess, 'Popen', return_value=writer), patch.object(m.os, 'pidfd_open', return_value=99), \
             patch.object(m.os, 'close'), patch.object(m.signal, 'pidfd_send_signal') as stop, \
             patch.object(m, 'relock_account', return_value={'finalPasswordStatus': 'L'}) as relock:
            with self.assertRaises(subprocess.TimeoutExpired): lease.open()
            stop.assert_called_once_with(99, signal.SIGKILL); relock.assert_called_once()
            self.assertTrue(lease.closed); self.assertFalse(lease.secret)
    def test_timer_failure_no_chpasswd(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install'); c.agent = Mock()
        c.ledger.value['authentication']['registered'] = True
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'L')), \
             patch.object(m, 'verify_timer', side_effect=ValueError('timer')), patch.object(m.subprocess, 'run') as run:
            with self.assertRaises(ValueError): lease.open()
            run.assert_not_called(); self.assertFalse(lease.attempted)
    def test_lease_close_locks_before_broken_ledger(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install'); lease.attempted = True; lease.secret = bytearray(b'private')
        c.ledger.mutate = Mock(side_effect=OSError('ledger unavailable'))
        with patch.object(m, 'relock_account', return_value={'finalPasswordStatus': 'L'}) as relock:
            with self.assertRaises(OSError): lease.close()
            relock.assert_called_once(); self.assertFalse(lease.secret)
    def test_auth_cleanup_never_kills_other_birth(self):
        c = controller(); c.ledger.value['authentication']['agent'] = {'identity': identity(90)}
        c.ops.proc.return_value = {**identity(90), 'startTime': 'reused'}
        with self.assertRaises(ValueError): m.close_auth(c.ledger, c.tun, False)
        c.ops.signal_owned.assert_not_called()
    def test_agent_sigkill_never_normal(self):
        c = controller(); old = identity(90); c.ledger.value['authentication']['agent'] = {'identity': old}
        c.ops.proc.return_value = old; c.ops.signal_owned.return_value = False
        with self.assertRaises(ValueError): m.close_auth(c.ledger, c.tun, False)
        self.assertEqual(c.ledger.value['result'], 'failed')
    def test_normal_close_no_signal_to_live_transaction(self):
        c = controller(); old = identity(90, 0); c.ledger.value['transactionProcesses'] = [{'identity': old}]
        c.ops.proc.return_value = old
        with self.assertRaises(ValueError): m.close_transactions(c.ledger, c.tun, signal_live=False)
        c.ops.signal_owned.assert_not_called()
    def test_observer_missing_pkexec_is_not_authorization(self):
        c = controller(); obs = m.TransactionObserver(c, 'install'); obs.thread = Mock(); obs.thread.is_alive.return_value = False
        obs.shells[(10, '1')] = {}; obs.rows = {}
        with self.assertRaises(ValueError): obs.finish()
    def test_observer_error_permanent(self):
        obs = m.TransactionObserver(controller(), 'install'); obs.error = 'missing'
        with self.assertRaises(ValueError): obs.snapshot()
    def test_watchdog_relock_precedes_mutable_ledger_load(self):
        c = controller(m.AUTH_MODE); calls = []
        c.tun.Ledger.side_effect = lambda _: calls.append('ledger') or (_ for _ in ()).throw(ValueError('bad ledger'))
        with tempfile.TemporaryDirectory() as d, patch.object(m, 'has_setting_intent', return_value=True), \
             patch.object(m, 'relock_account', side_effect=lambda *_: calls.append('relock') or {}):
            with self.assertRaises(ValueError): m.watchdog_recover(c.launch, c.tun, Path(d), 'install')
        self.assertEqual(calls[:2], ['relock', 'ledger'])


class AdditionalContracts(unittest.TestCase):
    def test_credential_rerun_attempt_closed_even_matching_objects(self):
        v = launch(m.AUTH_MODE); v['execution']['githubRunAttempt'] = '2'; v['authorization']['execution']['githubRunAttempt'] = '2'
        with self.assertRaises(ValueError): m.validate_permission(v)
    def test_core_scan_skips_pid_one(self):
        c = controller(); c.ledger.value['helper'] = {'identity': identity(70, 0)}
        child = identity(80); child['ppid'] = 70
        c.ops.proc.side_effect = lambda pid: child if pid == 80 else (_ for _ in ()).throw(AssertionError('PID1 must not be read'))
        with patch.object(m.Path, 'iterdir', return_value=[Path('/proc/1'), Path('/proc/self'), Path('/proc/80')]):
            c.register_core()
        c.tun.register_core.assert_called_once_with(c.ledger, 80)
    def test_transaction_scan_skips_pid_one(self):
        c = controller(); c.ops.proc.side_effect = lambda pid: c.gui if pid == c.gui['pid'] else (_ for _ in ()).throw(AssertionError('PID1'))
        obs = m.TransactionObserver(c, 'install')
        with patch.object(m.Path, 'iterdir', return_value=[Path('/proc/1'), Path('/proc/self')]): obs.scan()
        self.assertFalse(obs.snapshot()['authorizationObserved'])
    def test_private_read_rejects_symlink(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d); home.chmod(0o700); target = home/'target'; target.write_text('x'); target.chmod(0o600)
            alias = home/'alias'; alias.symlink_to(target)
            with self.assertRaises(OSError): m.private_bytes(alias, os.getuid(), home)
    def test_private_read_has_actual_bytes_hash_no_public_original(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d); home.chmod(0o700); target = home/'runtime'; target.write_bytes(b'{"secret":"fixture"}'); target.chmod(0o600)
            data, proof = m.private_bytes(target, os.getuid(), home)
            self.assertEqual(proof['sha256'], m.sha(data)); self.assertNotIn('secret', json.dumps(proof))
    def test_private_read_rejects_public_file(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d); home.chmod(0o700); target = home/'runtime'; target.write_text('fixture'); target.chmod(0o644)
            with self.assertRaises(ValueError): m.private_bytes(target, os.getuid(), home)
    def pump(self, chunks, *, echo=0, name='runner'):
        c = controller(m.AUTH_MODE); c.pty = 90
        lease = m.CredentialLease(c, 'install'); lease.window = 90
        lease.user = types.SimpleNamespace(pw_name=name, pw_gecos=''); lease.secret = bytearray(b'never-log-this-password')
        lease.close = Mock(); writes = []
        with patch.object(m.select, 'select', return_value=([90], [], [])), patch.object(m.os, 'read', side_effect=chunks), \
             patch.object(m.os, 'write', side_effect=lambda fd,b: writes.append(b) or len(b)), \
             patch('termios.tcgetattr', return_value=[0,0,0,echo,0,0,[]]):
            lease.pump()
        return lease, writes
    def test_real_prompt_account_and_complete_separate(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: ',
                                  b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertTrue(lease.authenticated); self.assertEqual(writes, [b'never-log-this-password\n']); self.assertIsNone(lease.error)
    def test_wrong_prompt_identity_never_gets_secret(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: root\nPassword: '])
        self.assertFalse(lease.authenticated); self.assertEqual(writes, []); lease.close.assert_called_once()
    def test_unrelated_action_never_gets_secret(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR other.action ====\nAuthenticating as: runner\nPassword: '])
        self.assertEqual(writes, []); self.assertIsNotNone(lease.error)
    def test_completion_without_password_is_not_authentication(self):
        lease, writes = self.pump([b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertFalse(lease.authenticated); self.assertEqual(writes, [])
    def test_multiple_identity_selects_only_exact_existing_account(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nMultiple identities can be used for authentication:\n 1.  root\n 2.  runner\nChoose identity to authenticate as (1-2): ',
                                  b'Password: ', b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertTrue(lease.authenticated); self.assertEqual(writes, [b'2\n', b'never-log-this-password\n'])
    def test_multiple_identity_ambiguity_denied(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\n 1.  runner\n 2.  runner\nChoose identity to authenticate as (1-2): '])
        self.assertEqual(writes, []); self.assertIsNotNone(lease.error)
    def test_authentication_denial_closes_lease(self):
        lease, writes = self.pump([b'==== AUTHENTICATION FAILED ====\n'])
        self.assertEqual(writes, []); lease.close.assert_called_once()
    def test_missing_notify_does_not_become_auth(self):
        c = controller(m.AUTH_MODE); c.ledger.value['authentication'].update(registered=True, authorized=False)
        self.assertFalse(c.ledger.load()['authentication']['authorized'])


class RecoveryAndObserverTests(unittest.TestCase):
    def observer_case(self, include_pkexec=True):
        c = controller(); c.tools = {'pkexec': {'sha256': H}, 'bash': {'sha256': H}}
        c.verify_script = Mock(return_value={'sha256': H}); c.tun.executable_identity.return_value = {'sha256': H}
        obs = m.TransactionObserver(c, 'install')
        process = {**identity(60), 'ppid': c.gui['pid'], 'exe': str(Path('/usr/bin/pkexec').resolve())}
        script = c.launch['helperScriptDir'] + '/' + 'd'*24 + '-polaris-helper-install.sh'
        rows = {c.gui['pid']: c.gui, 60: process}; c.ops.proc.side_effect = lambda pid: rows.get(pid)
        with patch.object(m.Path, 'iterdir', side_effect=lambda: [Path('/proc/1'), *[Path('/proc', str(k)) for k in rows]]), \
             patch.object(m, 'command_line', return_value=['/usr/bin/pkexec','/bin/bash',script]), \
             patch.object(m, 'uid_tuple', return_value=(1001,0,0,0)):
            if include_pkexec: obs.scan()
        rows[60] = {**process, 'uid': 0, 'exe': str(Path('/bin/bash').resolve())}
        with patch.object(m.Path, 'iterdir', side_effect=lambda: [Path('/proc/1'), *[Path('/proc', str(k)) for k in rows]]), \
             patch.object(m, 'command_line', return_value=['/bin/bash',script]), \
             patch.object(m, 'uid_tuple', return_value=(0,0,0,0)):
            obs.scan()
        return c, obs, rows
    def test_actual_observer_pkexec_same_birth_root_exec(self):
        c, obs, rows = self.observer_case()
        self.assertTrue(obs.snapshot()['authorizationObserved']); self.assertEqual(obs.snapshot()['pkexecBirths'], obs.snapshot()['rootShellBirths'])
        self.assertEqual(obs.snapshot()['subject'], {'pid': c.gui['pid'], 'startTime': c.gui['startTime'], 'uid': c.gui['uid']})
    def test_unobserved_root_shell_is_fail_not_inferred(self):
        with self.assertRaises(ValueError): self.observer_case(include_pkexec=False)
    def test_reparented_root_not_reconstructed_from_argv(self):
        c = controller(); obs = m.TransactionObserver(c, 'install')
        orphan = {**identity(60,0), 'ppid': 1}; rows = {c.gui['pid']:c.gui,60:orphan}
        c.ops.proc.side_effect = lambda pid: rows.get(pid)
        with patch.object(m.Path, 'iterdir', return_value=[Path('/proc/1'),Path('/proc/60')]): obs.scan()
        self.assertEqual(obs.snapshot()['owned'], [])
    def test_auth_subtree_tracks_actual_child_and_skips_pid_one(self):
        c = controller(); agent = identity(90); child = {**identity(91,0), 'ppid':90}
        c.ledger.value['authentication']['agent'] = {'identity':agent}; rows={90:agent,91:child}
        c.ops.proc.side_effect=lambda pid: rows.get(pid)
        observer=m.AuthObserver(c,'install')
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/1'),Path('/proc/90'),Path('/proc/91')]), \
             patch.object(m,'write_recovery_snapshot') as write:
            observer.scan()
            saved=write.call_args.args[2]; self.assertEqual(len(saved['processes']),2)
            self.assertEqual(saved['processes'][1]['uid'],0)
    def test_fresh_usable_password_rejection_never_locks(self):
        c=controller(m.AUTH_MODE); lease=m.CredentialLease(c,'install'); c.agent=Mock()
        c.ledger.value['authentication']['registered']=True; c.ops.run.return_value='inactive'
        with patch.object(m,'credential_account',return_value=(types.SimpleNamespace(pw_name='runner',pw_uid=1001),'L')), \
             patch.object(m,'verify_timer'),patch.object(m,'account_status',return_value='P'), \
             patch.object(m,'setting_intent') as intent,patch.object(m,'relock_account') as relock, \
             patch.object(m.subprocess,'Popen') as writer:
            with self.assertRaises(ValueError): lease.open()
            intent.assert_not_called(); relock.assert_not_called(); writer.assert_not_called(); self.assertFalse(lease.attempted)
    def test_watchdog_without_setting_intent_does_not_touch_account(self):
        c=controller(m.AUTH_MODE); c.tun.Ledger.side_effect=ValueError('ledger')
        with tempfile.TemporaryDirectory() as d,patch.object(m,'has_setting_intent',return_value=False), \
             patch.object(m,'relock_account') as relock:
            with self.assertRaises(ValueError): m.watchdog_recover(c.launch,c.tun,Path(d),'install')
            relock.assert_not_called()
    def test_watchdog_stops_writer_and_controller_before_final_relock(self):
        c=controller(m.AUTH_MODE); calls=[]; c.tun.Ledger.side_effect=ValueError('ledger')
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d); (directory/'writer-install.json').write_text(json.dumps(identity(80,0)))
            (directory/'controller-birth.json').write_text(json.dumps(identity(81,0)))
            with patch.object(m,'has_setting_intent',return_value=True),patch.object(m.Path,'iterdir',return_value=[]), \
                 patch.object(m,'stop_recorded',side_effect=lambda ops,tun,old:calls.append(('stop',old['pid']))), \
                 patch.object(m,'relock_account',side_effect=lambda *_:calls.append(('relock',1001)) or {}):
                with self.assertRaises(ValueError): m.watchdog_recover(c.launch,c.tun,directory,'install')
        self.assertEqual(calls,[('stop',81),('stop',80),('relock',1001)])
    def test_invalid_setting_intent_binding_denied(self):
        c=controller(m.AUTH_MODE)
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d); m.setting_intent(directory,'install',c.launch,'L')
            self.assertTrue(m.has_setting_intent(directory,'install',c.launch,c.tun))
            v=json.loads((directory/'setting-intent-install.json').read_text());v['account']='other'
            (directory/'setting-intent-install.json').write_text(json.dumps(v))
            with self.assertRaises(ValueError):m.has_setting_intent(directory,'install',c.launch,c.tun)
    def test_writer_cannot_receive_secret_before_birth_record(self):
        c=controller(m.AUTH_MODE); lease=m.CredentialLease(c,'install');c.agent=Mock();c.ops.run.return_value='inactive'
        c.ledger.value['authentication']['registered']=True
        writer=Mock(pid=89,returncode=0);writer.poll.return_value=0
        c.ops.proc.return_value={**identity(89,0),'ppid':os.getpid()};c.tun.executable_identity.return_value=c.tun.file_identity.return_value={'sha256':H}
        calls=[];writer.communicate.side_effect=lambda **kw:calls.append('secret')
        fake_thread=Mock()
        with patch.object(m,'credential_account',return_value=(types.SimpleNamespace(pw_name='runner',pw_uid=1001),'L')), \
             patch.object(m,'verify_timer'),patch.object(m,'account_status',side_effect=['L','P']), \
             patch.object(m,'setting_intent',side_effect=lambda *_:calls.append('intent')), \
             patch.object(m,'independent_record',side_effect=lambda *_:calls.append('writer-birth')), \
             patch.object(m.subprocess,'Popen',return_value=writer) as spawn,patch.object(m.os,'pidfd_open',return_value=99), \
             patch.object(m.threading,'Thread',return_value=fake_thread):
            lease.open()
        self.assertEqual(calls,['intent','writer-birth','secret']);self.assertEqual(spawn.call_args.args[0],['/usr/sbin/chpasswd','--crypt-method','SHA512','--sha-rounds','100000'])
        lease.secret.clear(); lease.writer_fd=None


class EntryAndConcurrencyTests(unittest.TestCase):
    def test_echo_still_enabled_never_receives_secret(self):
        c=controller(m.AUTH_MODE);c.pty=90;lease=m.CredentialLease(c,'install');lease.window=90
        lease.user=types.SimpleNamespace(pw_name='runner',pw_gecos='');lease.secret=bytearray(b'private');lease.close=Mock()
        chunk=b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: '
        import termios
        with patch.object(m.time,'monotonic',side_effect=[0,1,2,3,4]),patch.object(m.time,'sleep'), \
             patch.object(m.select,'select',return_value=([90],[],[])),patch.object(m.os,'read',return_value=chunk), \
             patch('termios.tcgetattr',return_value=[0,0,0,termios.ECHO,0,0,[]]),patch.object(m.os,'write') as write:
            lease.pump()
        write.assert_not_called();lease.close.assert_called_once();self.assertFalse(lease.authenticated)
    def test_close_concurrency_is_serial_and_no_duplicate_relock(self):
        c=controller(m.AUTH_MODE);lease=m.CredentialLease(c,'install');lease.attempted=True
        c.ledger.value['authentication']['lease']={};c.ops.run.return_value='inactive'
        with patch.object(m,'relock_account',return_value={'finalPasswordStatus':'L'}) as relock:
            lease.close();lease.close()
        relock.assert_called_once();self.assertTrue(lease.closed)
    def test_stuck_private_worker_still_relocks_before_failure(self):
        c=controller(m.AUTH_MODE);lease=m.CredentialLease(c,'install');lease.attempted=True;lease.secret=bytearray(b'private')
        lease.thread=Mock();lease.thread.is_alive.return_value=True;c.ops.run.return_value='inactive'
        c.ledger.value['authentication']['lease']={}
        with patch.object(m,'relock_account',return_value={'finalPasswordStatus':'L'}) as relock:
            with self.assertRaises(ValueError):lease.close()
        relock.assert_called_once();self.assertFalse(lease.secret);self.assertFalse(lease.closed)
    def main_fixture(self, dispatch_error=False, finish=False):
        c=Mock();c.finished=False;c.seq=1
        def dispatch(_):
            if dispatch_error:raise ValueError('PRIVATE SECRET MUST NOT BE PRINTED')
            if finish:c.finished=True
            return {'seq':1,'ok':True,'result':{}}
        c.dispatch.side_effect=dispatch
        lines=[b'{"seq":1,"action":"prepare"}\n',ValueError('EOF')]
        out=io.StringIO()
        import ctypes
        with patch.object(m.sys,'argv',['controller','serve','--run-id','0123456789ab']), \
             patch.object(m,'load_frozen',return_value=(launch(),Mock(),Path('/root/frozen'))), \
             patch.object(m,'Controller',return_value=c),patch.object(ctypes,'CDLL',return_value=Mock(prctl=Mock(return_value=0))), \
             patch.object(m,'read_line',side_effect=lines),patch.object(m.sys,'stdout',out):
            if finish:m.main()
            else:
                with self.assertRaises(ValueError):m.main()
        return c,out.getvalue()
    def test_main_eof_forces_abort(self):
        c,out=self.main_fixture();c.abort.assert_called_once();self.assertIn('ready',out)
    def test_main_error_redacts_details_and_aborts(self):
        c,out=self.main_fixture(dispatch_error=True);c.fail.assert_called_once();c.abort.assert_called_once()
        self.assertNotIn('PRIVATE SECRET',out);self.assertIn('CONTROLLER_FAILED',out)
    def test_main_normal_finish_does_not_force_recovery(self):
        c,_=self.main_fixture(finish=True);c.abort.assert_not_called()
    def test_watchdog_entry_always_nonzero(self):
        with patch.object(m.sys,'argv',['controller','watch-lease','--run-id','0123456789ab','--transaction','install']), \
             patch.object(m,'load_frozen',return_value=(launch(m.AUTH_MODE),Mock(),Path('/root/frozen'))) as load, \
             patch.object(m,'watchdog_recover') as recover:
            with self.assertRaises(SystemExit) as result:m.main()
            self.assertEqual(result.exception.code,1);self.assertTrue(load.call_args.kwargs['recovery']);recover.assert_called_once()


class WatchdogRaceTests(unittest.TestCase):
    def race(self, writer_registered):
        c=controller(m.AUTH_MODE);events=[];c.tun.Ledger.side_effect=ValueError('mutable ledger unavailable')
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d);(directory/'controller-birth.json').write_text(json.dumps(identity(81,0)))
            if writer_registered:(directory/'writer-install.json').write_text(json.dumps(identity(80,0)))
            def stopped(ops,tun,old):events.append(('stop',old['pid']))
            def intent(*_):
                self.assertEqual(events,[('stop',81)])
                events.append(('read-intent',True));return True
            with patch.object(m,'has_setting_intent',side_effect=intent),patch.object(m,'stop_recorded',side_effect=stopped), \
                 patch.object(m,'relock_account',side_effect=lambda *_:events.append(('relock',1001)) or {}):
                with self.assertRaises(ValueError):m.watchdog_recover(c.launch,c.tun,directory,'install')
        return events
    def test_timeout_after_intent_before_writer_registration_stops_producer_first(self):
        self.assertEqual(self.race(False),[('stop',81),('read-intent',True),('relock',1001)])
    def test_timeout_after_writer_registration_before_secret_stops_both_first(self):
        self.assertEqual(self.race(True),[('stop',81),('read-intent',True),('stop',80),('relock',1001)])
    def test_failing_agent_ledger_cannot_skip_native_reap(self):
        # Structural ordering supplements registration mocks without opening a real PTY.
        source=Path(m.__file__).read_text();block=source[source.index('    def agent_register('):source.index('    def transaction_begin(')]
        self.assertLess(block.index('self.agent.kill(); self.agent.wait'),block.index('self.fail()'))
        self.assertIn('finally:\n                self.fail()',block)


class GuiBindingProofTests(unittest.TestCase):
    def binding(self, mismatch=False, changed=False):
        c=controller();c.gui=None;gui=identity();driver={**identity(20),'startTime':'1234','exeInode':5}
        c.ops.proc.side_effect=lambda pid:copy.deepcopy(gui if pid==50 else driver)
        c.tun.executable_identity.return_value={'sha256':H,'dev':1,'inode':78 if mismatch else 77}
        if changed:c.tun.same_process.side_effect=[True,False]
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/1'),Path('/proc/50')]):return c.bind_gui(50)
    def test_binding_returns_observed_executable_same_inode(self):
        result=self.binding();self.assertEqual(result['executable'],{'sha256':H,'dev':1,'inode':77})
        self.assertEqual(result['identity']['exeInode'],result['executable']['inode'])
    def test_binding_wrong_inode_rejected(self):
        with self.assertRaises(ValueError):self.binding(mismatch=True)
    def test_binding_birth_changed_after_hash_rejected(self):
        with self.assertRaises(ValueError):self.binding(changed=True)


class ZombieClosureTests(unittest.TestCase):
    def test_stat_only_reads_zombie_birth_without_exe(self):
        fields=['Z',str(os.getpid())]+['0']*17+['2345']
        with patch.object(m.Path,'read_text',return_value='91 (PAM (helper)) '+' '.join(fields)):
            self.assertEqual(m.proc_stat(91),{'pid':91,'startTime':'2345','state':'Z','ppid':os.getpid()})
    def test_missing_stat_but_existing_pid_is_unknown(self):
        with patch.object(m.Path,'read_text',side_effect=FileNotFoundError),patch.object(m,'proc_absent',return_value=False):
            with self.assertRaises(ValueError):m.proc_stat(91)
    def test_absence_requires_missing_proc_not_unreadable(self):
        with patch.object(m.Path,'lstat',side_effect=PermissionError):
            with self.assertRaises(PermissionError):m.proc_absent(91)
        with patch.object(m.Path,'lstat',side_effect=FileNotFoundError):self.assertTrue(m.proc_absent(91))
    def auth(self):
        c=controller(m.AUTH_MODE);c.agent=Mock(pid=90)
        c.ledger.value['authentication']['agent']={'identity':identity(90)}
        obs=m.AuthObserver(c,'install');obs.thread=Mock();obs.thread.is_alive.return_value=False;obs.scan=Mock()
        obs.rows[m.birth(identity(91,0))]=identity(91,0)
        return c,obs
    def test_known_adopted_zombie_uses_actual_waitpid(self):
        c,obs=self.auth()
        rows={90:{**identity(90),'state':'Z','ppid':os.getpid()},91:{**identity(91,0),'state':'Z','ppid':os.getpid()}}
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/90'),Path('/proc/91')]), \
             patch.object(m,'stop_recorded',return_value=False),patch.object(m,'proc_stat',side_effect=rows.get), \
             patch.object(m.os,'waitpid',return_value=(91,0)) as wait:
            obs.close()
        wait.assert_called_once_with(91,os.WNOHANG)
    def test_unknown_adopted_zombie_cannot_be_ignored(self):
        c,obs=self.auth()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/92')]),patch.object(m,'stop_recorded',return_value=False), \
             patch.object(m,'proc_stat',return_value={**identity(92,0),'state':'Z','ppid':os.getpid()}):
            with self.assertRaises(ValueError):obs.close()
    def test_known_zombie_reuse_cannot_be_reaped(self):
        c,obs=self.auth()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/91')]),patch.object(m,'stop_recorded',return_value=False), \
             patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'Z','startTime':'999','ppid':os.getpid()}), \
             patch.object(m.os,'waitpid') as wait:
            with self.assertRaises(ValueError):obs.close()
        wait.assert_not_called()
    def test_waitpid_zero_not_reaped(self):
        c,obs=self.auth()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/91')]),patch.object(m,'stop_recorded',return_value=False), \
             patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'Z','ppid':os.getpid()}), \
             patch.object(m.os,'waitpid',return_value=(0,0)):
            with self.assertRaises(ValueError):obs.close()
    def test_full_identity_none_does_not_prove_secret_process_gone(self):
        c=controller();c.ops.proc.return_value=None
        with patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'S'}):
            with self.assertRaises(ValueError):m.stop_recorded(c.ops,c.tun,identity(91,0))
        with patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'Z'}):
            self.assertFalse(m.stop_recorded(c.ops,c.tun,identity(91,0)))
    def test_normal_cleanup_rejects_existing_pid_despite_full_identity_none(self):
        c=controller();c.ledger.value['authentication']['closed']=True
        c.ledger.value['processes']=[{'identity':identity(91,0)}];c.ops.proc.return_value=None
        with patch.object(m,'close_transactions'),patch.object(m,'proc_absent',return_value=False):
            with self.assertRaises(ValueError):c.normal_cleanup()
    def test_stop_agent_final_absence_is_independent_of_full_identity(self):
        c=controller();c.auth_observer=Mock();c.auth_observer.close.return_value={'ownedBirths':[[91,'2345']]}
        c.agent=Mock();c.agent.wait.return_value=-signal.SIGTERM;c.ops.proc.return_value=None
        with patch.object(m,'close_auth',return_value={}),patch.object(m,'proc_absent',return_value=False):
            with self.assertRaises(ValueError):c.stop_agent()


if __name__ == '__main__': unittest.main(verbosity=2)
