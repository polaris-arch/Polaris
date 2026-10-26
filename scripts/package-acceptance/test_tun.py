"""Pure/mocked tests: no privilege, sockets, product, namespaces or system changes."""
import copy
from contextlib import ExitStack
from types import SimpleNamespace
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch, Mock

HERE = Path(__file__).resolve().parent
BASE_FIXTURE = Path(os.environ.get('POLARIS_BASE_FIXTURES', str(HERE / 'fixtures.py')))
TEMP = tempfile.TemporaryDirectory(prefix='polaris-tun-pure-tests-')
for name in ('tun.py', 'tun_fixtures.py'):
    shutil.copyfile(HERE / name, Path(TEMP.name) / name)
shutil.copyfile(BASE_FIXTURE, Path(TEMP.name) / 'fixtures.py')

def load(name):
    spec = importlib.util.spec_from_file_location('test_' + name, Path(TEMP.name) / (name + '.py'))
    result = importlib.util.module_from_spec(spec); spec.loader.exec_module(result); return result

tun = load('tun'); fixture = load('tun_fixtures')
TOKEN = '0123456789ab'; PLAN = tun.topology(TOKEN); H = 'a' * 64


def identity(pid=10, uid=1001, net=5):
    return {'pid': pid, 'startTime': '1234', 'state': 'S', 'ppid': 9, 'uid': uid,
            'exeDev': 1, 'exeInode': pid + 100, 'exe': '/usr/bin/python3.12', 'capEff': 0x3400, 'capAmb': 0x3400,
            'namespaces': {'net': net, 'mnt': 6, 'pid': 7, 'cgroup': 8}, 'cgroup': '0::/test.slice'}


def good_facts():
    return {'githubHosted': True, 'dedicatedVM': True, 'pid1Systemd': True,
            'rootManager': True, 'tunDevice': True, 'pidfd': True,
            'sameSystemNamespaces': True, 'toolsRootOwned': True,
            'guiUid': 1001, 'noNewPrivs': 0, 'forwarding': [0, 0], 'forwardingStable': True,
            'existingProductPaths': [], 'unsafeEnvironment': [], 'routes': [],
            'networkSnapshotComplete': True, 'materialsDownloaded': True}


def carrier():
    return {'kernelCommit': tun.KERNEL_COMMIT, 'originalAdmission': 'verified',
            'carrierSha256': H, 'helperSha256': H, 'coreSha256': H}


class FakeOps:
    def __init__(self):
        self.commands = []; self.pids = {}; self.namespaces = {}; self.links = []; self.routes = []
        self.rules = {'4': [], '6': []}; self.members = {}; self.closed = True
        self.props = {'MainPID': '0', 'ActiveState': 'inactive', 'FragmentPath': str(tun.UNIT),
                      'DropInPaths': '', 'User': '', 'ControlGroup': '', 'KillMode': 'control-group',
                      'SendSIGKILL': 'yes', 'FinalKillSignal': '9', 'Delegate': 'no'}
    def proc(self, pid): return copy.deepcopy(self.pids.get(pid))
    def cgroup_closed(self): return self.closed
    def namespace_inode(self, name): return self.namespaces.get(name)
    def processes_in(self, inode): return self.members.get(inode, [])
    def signal_owned(self, old, sig):
        self.commands.append(['signal', old['pid'], int(sig)])
        if not tun.same_process(old, self.pids.get(old['pid'])): raise ValueError('birth mismatch')
        self.pids.pop(old['pid'], None); return True
    def run(self, args, timeout=15):
        self.commands.append(args)
        if args[0] == tun.SYSTEMCTL:
            if args[1] == 'show':
                if args[-1] == '--value': return 'inactive\n'
                return '\n'.join(k + '=' + v for k, v in self.props.items())
            return ''
        if args[:3] == [tun.IP, 'netns', 'del']:
            self.namespaces.pop(args[3], None)
        elif args[:4] == [tun.IP, 'link', 'del', 'dev']:
            self.links = [v for v in self.links if v['ifname'] != args[4]]
        elif args[:4] == [tun.IP, '-4', 'route', 'del']:
            self.routes = []
        elif len(args) > 4 and args[2:4] == ['rule', 'del']:
            family = args[1][1:]; priority = int(args[-1])
            for i, row in enumerate(self.rules[family]):
                if row['priority'] == priority: self.rules[family].pop(i); break
        return ''
    def json(self, args):
        self.commands.append(args)
        if 'rule' in args: return copy.deepcopy(self.rules[args[2][1:]])
        if 'route' in args: return copy.deepcopy(self.routes)
        if 'link' in args: return copy.deepcopy(self.links)
        return []


class FakeLedger:
    def __init__(self):
        self.run_id = TOKEN; self.ops = FakeOps(); self.children = {}; self.directory = Path('/not-used')
        self.value = {'runId': TOKEN, 'phase': 'network-ready', 'result': 'incomplete', 'guiUid': 1001,
                      'forcedCleanup': False, 'unknown': [], 'processes': [], 'namespaces': [],
                      'links': [], 'routes': [], 'intents': [], 'helper': None, 'productUninstalled': True,
                      'baseline': {'network': {}, 'systemNamespaces': identity()['namespaces']},
                      'deadlineMonotonic': 1000000000000, 'helperSha256': H, 'coreSha256': H}
    def load(self): return copy.deepcopy(self.value)
    def mutate(self, fn): fn(self.value); return self.load()


class MemorySocket:
    def __init__(self, chunks): self.chunks = list(chunks); self.sent = b''; self.closed = False
    def recv(self, count):
        if not self.chunks: return b''
        item = self.chunks.pop(0)
        if isinstance(item, Exception): raise item
        if len(item) > count: self.chunks.insert(0, item[count:]); item = item[:count]
        return item
    def sendall(self, data): self.sent += data
    def settimeout(self, value): pass
    def close(self): self.closed = True




class AllTableObservationTests(unittest.TestCase):
    def test_fresh_absent_table_uses_successful_empty_family_observations(self):
        self.assertEqual(tun.tun_table_routes({'route4': [], 'route6': []}), [])

    def test_exact_numeric_and_string_ids_in_both_families(self):
        a = {'table': 2022, 'dst': '198.18.240.0/24'}
        b = {'table': '2022', 'dst': 'fd00::/64'}
        snapshot = {'route4': [a, {'table': '254'}, {}],
                    'route6': [b, {'table': 254}, {'table': '20220'}]}
        self.assertEqual(tun.tun_table_routes(snapshot), [a, b])
        facts = good_facts(); facts['tunTableRoutes'] = tun.tun_table_routes(snapshot)
        self.assertIn('productTunRuleOrTableAlreadyOwned',
                      tun.preflight(facts, carrier(), True)['blocked'])

    def test_incomplete_or_malformed_observations_are_not_empty_success(self):
        for snapshot in ({'route4': []}, {'route4': [], 'route6': None},
                         {'route4': [None], 'route6': []},
                         {'route4': [{'table': True}], 'route6': []},
                         {'route4': [], 'route6': [{'table': 2022.0}]},
                         {'route4': [{'table': 'custom-alias'}], 'route6': []},
                         {'route4': [{'table': '02022'}], 'route6': []}):
            with self.subTest(snapshot=snapshot), self.assertRaises(ValueError):
                tun.tun_table_routes(snapshot)

    def test_real_snapshot_requests_numeric_all_tables_for_both_families(self):
        ops = Mock(); ops.json.return_value = []; ops.run.return_value = 'resolved'
        with patch.object(tun.Path, 'is_symlink', return_value=False), \
             patch.object(tun.Path, 'read_bytes', return_value=b'fixture resolver'), \
             patch.object(tun.Path, 'read_text', return_value='0'):
            snapshot = tun.network_snapshot(ops)
        self.assertEqual(tun.tun_table_routes(snapshot), [])
        commands = [call.args[0] for call in ops.json.call_args_list]
        for family in ('4', '6'):
            self.assertIn([tun.IP, '-j', '-N', '-' + family, 'route', 'show', 'table', 'all'], commands)
        self.assertFalse(any(command[-1] == '2022' for command in commands))

    def test_failed_all_table_command_is_not_suppressed(self):
        ops = Mock(); ops.json.side_effect = OSError('unavailable observation')
        with self.assertRaises(OSError): tun.network_snapshot(ops)
        self.assertNotIn("'show', 'table', '2022'", (HERE / 'tun.py').read_text())


class PreflightTests(unittest.TestCase):
    def test_complete_is_preparation_not_pass(self):
        self.assertEqual(tun.KERNEL_COMMIT, '332935c310e78e15319d434f33a9103329200931')
        result = tun.preflight(good_facts(), carrier(), True)
        self.assertEqual(result['status'], 'READY_FOR_LOCAL_PREPARATION')
        self.assertFalse(result['publicationEligible'])
    def test_missing_carrier_and_auth_block(self):
        result = tun.preflight(good_facts())
        self.assertEqual(result['status'], 'BLOCKED')
        self.assertIn('reviewedFormalCarrierAdmission', result['blocked'])
        self.assertIn('realPkexecAuthenticationPendingConfirmation', result['blocked'])
    def test_historical_kernel_cannot_admit_current_carrier(self):
        for commit in ('0cfa24573ff34c26608151068334329268cfdf14',
                       'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a',
                       '814da362838e96e54f904836a92c2b0d8c3ae29b', '8' * 40):
            with self.subTest(commit=commit):
                c = carrier(); c['kernelCommit'] = commit; before = copy.deepcopy(c)
                self.assertIn('reviewedFormalCarrierAdmission', tun.preflight(good_facts(), c, True)['blocked'])
                self.assertEqual(c, before)  # Rejection never relabels historical evidence.
    def test_root_gui_sandbox_and_sysctl_rejected(self):
        facts = good_facts(); facts.update(guiUid=0, forwarding=[2, 0], unsafeEnvironment=['LD_PRELOAD'])
        result = tun.preflight(facts, carrier(), True)
        self.assertIn('ordinaryGUI', result['blocked']); self.assertIn('freshProductAndDefaultSandbox', result['blocked'])
    def test_existing_reserved_rules_table_and_tun_block(self):
        for addition in ({'network': {'rule4': [{'priority': 9003}]}}, {'routes': [{'dst': 'default', 'table': 2022}]},
                         {'tunTableRoutes': [{'dst': 'default'}]}, {'network': {'links': [{'ifname': 'polaris-tun0'}]}}):
            f = good_facts(); f.update(addition)
            self.assertIn('productTunRuleOrTableAlreadyOwned', tun.preflight(f, carrier(), True)['blocked'])
    def test_cidr_collision_but_not_default(self):
        f = good_facts(); f['routes'] = [{'dst': 'default'}, {'dst': '198.18.240.9/32'}]
        self.assertIn('fixtureCIDRCollision', tun.preflight(f, carrier(), True)['blocked'])
    def test_no_shell_or_escape_names(self):
        for value in ('../helper', 'a' * 13, 'a;id', ''):
            with self.assertRaises(ValueError): tun.topology(value)
        self.assertTrue(all(len(n) <= 15 for n in PLAN['hostLinks'].values()))
        self.assertEqual(len(set(PLAN['namespaces'].values())), 1)
        self.assertEqual(len(set(PLAN['hostLinks'].values())), 1)
    def test_no_auth_mutation_in_module(self):
        text = (HERE / 'tun.py').read_text()
        self.assertNotIn("'/usr/sbin/chpasswd'", text); self.assertNotIn("'/usr/bin/passwd'", text)
        self.assertNotIn("'/usr/bin/sudo'", text)


class IdentityAndCleanupTests(unittest.TestCase):
    def test_complete_birth_required(self):
        self.assertFalse(tun.same_process({'pid': 7}, {'pid': 7}))
        self.assertTrue(tun.same_process(identity(), identity()))
        changed = identity(); changed['startTime'] = '1235'
        self.assertFalse(tun.same_process(identity(), changed))
        changed = identity(); changed['namespaces']['net'] += 1
        self.assertFalse(tun.same_process(identity(), changed))
    def test_pidfd_rechecks_before_signalling(self):
        ops = tun.Commands()
        with patch.object(tun.os, 'pidfd_open', return_value=99), patch.object(tun.os, 'close'), \
             patch.object(tun.signal, 'pidfd_send_signal') as signal_mock, patch.object(ops, 'proc', return_value=identity(pid=11)):
            with self.assertRaises(ValueError): ops.signal_owned(identity(), tun.signal.SIGKILL)
            signal_mock.assert_not_called()
    def test_namespace_unknown_member_prevents_all_network_delete(self):
        ledger = FakeLedger(); ns = PLAN['namespaces']['A']
        ledger.value['namespaces'] = [{'name': ns, 'inode': 70}]
        ledger.ops.namespaces[ns] = 70; ledger.ops.members[70] = [identity(pid=90)]
        errors = tun._safe_remove_network(ledger, ledger.value)
        self.assertTrue(errors); self.assertFalse(any('del' in c for c in ledger.ops.commands))
    def test_namespace_reuse_not_removed(self):
        ledger = FakeLedger(); ns = PLAN['namespaces']['A']
        ledger.value['namespaces'] = [{'name': ns, 'inode': 70}]; ledger.ops.namespaces[ns] = 71
        self.assertTrue(tun._safe_remove_network(ledger, ledger.value))
        self.assertFalse(any('del' in c for c in ledger.ops.commands))
    def test_veth_identity_reuse_not_removed(self):
        ledger = FakeLedger(); name = PLAN['hostLinks']['A']
        ledger.value['links'] = [{'name': name, 'ifindex': 10, 'address': 'aa'}]
        ledger.ops.links = [{'ifname': name, 'ifindex': 11, 'address': 'bb'}]
        self.assertIn('link-removal-unknown:' + name, tun._safe_remove_network(ledger, ledger.value))
        self.assertFalse(any('del' in c for c in ledger.ops.commands))
    def test_fixed_provision_is_one_backend_and_no_host_default_or_forwarding(self):
        ledger = FakeLedger(); ledger.value['phase'] = 'guarded'
        class ProvisionOps(FakeOps):
            def run(self, args, timeout=15):
                if args[:3] == [tun.IP, 'netns', 'add']: self.namespaces[args[3]] = 70
                if args[:3] == [tun.IP, 'link', 'add']:
                    self.links = [{'ifname': args[3], 'ifindex': 10, 'address': 'aa'}]
                return super().run(args, timeout)
        ledger.ops = ProvisionOps(); tun.provision_network(ledger)
        commands = ledger.ops.commands
        self.assertEqual(sum(c[:3] == [tun.IP, 'netns', 'add'] for c in commands), 1)
        self.assertEqual(sum(c[:3] == [tun.IP, 'link', 'add'] for c in commands), 1)
        self.assertFalse(any('default' in c or 'sysctl' in c or 'nft' in c for c in commands))
        self.assertEqual(ledger.value['phase'], 'network-ready')
        self.assertTrue(all(r['state'] == 'done' for r in ledger.value['intents']))
    def test_pending_mutation_never_normal(self):
        ledger = FakeLedger(); ledger.value['intents'] = [{'state': 'pending'}]
        with patch.object(tun, 'network_snapshot', return_value={}): result = tun.cleanup(ledger)
        self.assertEqual(result['state'], 'failed'); self.assertIn('partial-network-command-unknown', result['unknown'])
    def test_live_product_blocks_network_cleanup(self):
        ledger = FakeLedger(); old = identity(); ledger.value['processes'] = [{'role': 'core', 'identity': old}]
        ledger.ops.pids[old['pid']] = old
        result = tun.cleanup(ledger)
        self.assertEqual(result['state'], 'failed'); self.assertFalse(any('del' in c for c in ledger.ops.commands))
    def test_forced_cleanup_never_normal_even_if_restored(self):
        ledger = FakeLedger()
        with patch.object(tun, 'network_snapshot', return_value={}): result = tun.cleanup(ledger, forced=True)
        self.assertEqual(result['state'], 'forced'); self.assertEqual(ledger.value['result'], 'failed')
        self.assertTrue(ledger.value['forcedCleanup'])
    def test_forced_then_normal_cannot_launder_failure(self):
        ledger = FakeLedger()
        with patch.object(tun, 'network_snapshot', return_value={}):
            tun.cleanup(ledger, forced=True)
            again = tun.cleanup(ledger, forced=False)
        self.assertEqual(again['state'], 'forced'); self.assertTrue(again['forced'])
        self.assertEqual(ledger.value['result'], 'failed')
    def test_earlier_unknown_is_permanent(self):
        ledger = FakeLedger(); ledger.value['unknown'] = ['old-unknown']
        with patch.object(tun, 'network_snapshot', return_value={}): result = tun.cleanup(ledger)
        self.assertEqual(result['state'], 'failed'); self.assertIn('old-unknown', result['unknown'])
    def test_firewall_counter_runtime_values_only_are_normalized(self):
        before = {'firewall': {'nftables': [{'rule': {'expr': [{'counter': {'name': 'owned', 'packets': 1, 'bytes': 20}},
                    {'accept': None}], 'handle': 8}}]}, 'packets': 3}
        after = copy.deepcopy(before); counter = after['firewall']['nftables'][0]['rule']['expr'][0]['counter']
        counter.update(packets=2, bytes=40)
        self.assertEqual(tun.stable_network(before), tun.stable_network(after))
        counter['name'] = 'other'
        self.assertNotEqual(tun.stable_network(before), tun.stable_network(after))
        after = copy.deepcopy(before); after['firewall']['nftables'][0]['rule']['handle'] = 9
        self.assertNotEqual(tun.stable_network(before), tun.stable_network(after))
        after = copy.deepcopy(before); after['packets'] = 4
        self.assertNotEqual(tun.stable_network(before), tun.stable_network(after))
    def test_snapshot_mismatch_fails(self):
        ledger = FakeLedger()
        with patch.object(tun, 'network_snapshot', return_value={'forwarding': [1, 0]}): result = tun.cleanup(ledger)
        self.assertIn('network-restoration-unproved', result['unknown'])
    def test_service_cgroup_empty_required(self):
        ops = FakeOps(); ops.closed = False
        with self.assertRaises(ValueError): tun.service_closed(ops)
    def test_dropin_rejected(self):
        ops = FakeOps(); ops.props['DropInPaths'] = '/etc/systemd/system/polaris-helper.service.d/test.conf'
        with self.assertRaises(ValueError): tun.validate_service(ops.props)
    def test_recover_only_recorded_rules(self):
        ledger = FakeLedger(); ledger.ops.rules['4'] = [{'priority': 9001, 'table': 2022}]
        with self.assertRaises(ValueError): tun.recover_tun(ledger, ledger.value)
        self.assertFalse(any('del' in c for c in ledger.ops.commands))
    def test_recorded_rules_can_be_removed_exactly(self):
        ledger = FakeLedger(); row = {'priority': 9001, 'table': 2022}
        ledger.ops.rules['4'] = [row]; ledger.value['tunRules'] = {'4': [row], '6': []}
        tun.recover_tun(ledger, ledger.value)
        self.assertFalse(ledger.ops.rules['4'])
    def test_uninstall_requires_entire_product_and_runtime_dirs_absent(self):
        for leftover in ('/usr/local/lib/polaris', '/run/polaris'):
            ledger = FakeLedger(); ledger.value['productUninstalled'] = False
            with patch.object(tun.Path, 'exists', lambda p: str(p) == leftover), patch.object(tun.Path, 'is_symlink', return_value=False):
                with self.assertRaises(ValueError): tun.confirm_product_uninstalled(ledger)
            self.assertFalse(ledger.value['productUninstalled'])
    def test_normal_requires_actual_uninstall_marker(self):
        ledger = FakeLedger(); ledger.value['productUninstalled'] = False
        with self.assertRaises(ValueError): tun.cleanup(ledger)
    def test_cancel_after_forced_forbidden(self):
        ledger = FakeLedger(); ledger.value.update(phase='closed', forcedCleanup=True, networkRestored=True)
        with self.assertRaises(ValueError): tun.cancel_watchdog(ledger)
        self.assertFalse(ledger.ops.commands)
    def test_early_spawn_failure_is_permanent_and_reaps_direct_child(self):
        ledger = FakeLedger(); child = Mock(pid=44)
        with patch.object(tun.subprocess, 'Popen', return_value=child), patch.object(tun.os, 'pidfd_open', side_effect=OSError('test')):
            with self.assertRaises(OSError): tun._spawn_owned(ledger, ['/bounded/mock'])
        child.kill.assert_called_once(); child.wait.assert_called_once_with(timeout=5)
        self.assertTrue(ledger.value['forcedCleanup']); self.assertEqual(ledger.value['result'], 'failed')
    def test_abort_wait_failure_still_marks_permanent_fail(self):
        ledger = FakeLedger(); child = Mock(pid=44); child.wait.side_effect = TimeoutError('test')
        with patch.object(tun.signal, 'pidfd_send_signal'):
            with self.assertRaises(TimeoutError): tun._abort_spawn(ledger, child, 99)
        self.assertTrue(ledger.value['forcedCleanup']); self.assertEqual(ledger.value['result'], 'failed')
    def test_normal_cleanup_can_cancel_timer_only_before_deadline(self):
        ledger = FakeLedger()
        with patch.object(tun, 'network_snapshot', return_value={}): result = tun.cleanup(ledger)
        self.assertEqual(result['state'], 'normal'); tun.cancel_watchdog(ledger)
        self.assertTrue(ledger.value['watchdogCancelled'])
        ledger.value['deadlineMonotonic'] = 0
        with self.assertRaises(ValueError): tun.cancel_watchdog(ledger)
    def test_arbitrary_pid_registration_absent(self):
        self.assertFalse(hasattr(tun, 'register_process'))
        with self.assertRaises(ValueError): tun.register_spawned_fixture(FakeLedger(), Mock(pid=1), [], 'A')


class FixtureAndCaptureTests(unittest.TestCase):
    def test_config_reuses_base_without_dashboard_override(self):
        adapter = fixture.ConfigAdapter(TOKEN)
        p = adapter.config_patch(10000, 10001, 'x' * 32)
        self.assertEqual(p['proxyModeType'], 'tun'); self.assertNotIn('singboxDashboard', p)
        self.assertEqual(p['trafficRules'][0]['type'], 'ipCidr')
        self.assertEqual(p['dnsRules'][0]['type'], 'domainSuffix')
        self.assertEqual([n['address'] for n in p['servers']], list(PLAN['endpoint'].values()))
        self.assertEqual(p['dnsServers'][-1]['endpoint']['host'], PLAN['endpoint']['B'])
        changed = adapter.changed_graph(p)
        self.assertEqual(changed['trafficRules'][0]['targetServerId'], 'node-b')
    def test_proxy_cannot_relay_arbitrary_destination(self):
        for host, port in [('1.1.1.1', 443), ('127.0.0.1', 80), (PLAN['origin'], 22)]:
            with self.assertRaises(ValueError): fixture.allowed_target(PLAN, host, port)
    def test_framed_nonce_survives_segmentation_and_idle(self):
        body = b'nonceabc'; frame = struct.pack('!H', len(body)) + body
        sock = MemorySocket([socket.timeout(), frame[:1], frame[1:4], frame[4:]])
        self.assertEqual(fixture.recv_nonce_frame(sock, threading.Event()), body)
        echo = b''.join(b'A' + bytes([b]) for b in body)
        client = MemorySocket([echo[:3], echo[3:]])
        self.assertEqual(fixture.exchange(client, 'A', body)['nonceSha256'], hashlib.sha256(body).hexdigest())
        self.assertEqual(client.sent, frame)
    def test_socks_request_parses_only_controlled_protocol(self):
        sock = MemorySocket([b'\x05\x01\x00\x05\x01\x00\x01' + socket.inet_aton(PLAN['origin']) + struct.pack('!H', 48080)])
        self.assertEqual(fixture.parse_socks_request(sock), (PLAN['origin'], 48080))
    def test_nonce_suffix_dns_is_controlled(self):
        labels = ('abcdef.' + fixture.base.DOMAIN).split('.')
        q = b'\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00' + b''.join(bytes([len(l)]) + l.encode() for l in labels) + b'\0\0\1\0\1'
        answer, row = fixture.dns_response(PLAN, q, 'udp')
        self.assertEqual(row['answer'], PLAN['origin']); self.assertEqual(row['ttl'], 0)
        self.assertIn(socket.inet_aton(PLAN['origin']), answer)
    def test_mature_follow_output_decodes_only_client_frames(self):
        frame = b'\0\3abc'
        text = ('===\nFollow: tcp,raw\nFilter: tcp.stream eq 0\nNode 0: 172.19.0.1:4000\n'
                f'Node 1: {PLAN["origin"]}:48080\n' + frame[:2].hex() + '\n' + frame[2:].hex() + '\n\t4142\n===\n')
        self.assertEqual(fixture.followed_client_bytes(text, PLAN['origin'], 48080), frame)
        self.assertEqual(fixture.framed_payloads(frame), [b'abc'])
        with self.assertRaises(ValueError): fixture.framed_payloads(frame[:-1])
        with self.assertRaises(ValueError): fixture.followed_client_bytes(text, '127.0.0.1', 48080)
    def test_offline_tshark_adapter_is_bounded_and_simulations_labelled(self):
        path = Path(TEMP.name) / 'mock-capture.pcap'; path.write_bytes(b'0' * 24)
        def execute(args):
            if '-z' in args:
                return ('===\nFollow: tcp,raw\nFilter: tcp.stream eq 0\nNode 0: 172.19.0.1:4000\n'
                        f'Node 1: {PLAN["origin"]}:48080\n0003616263\n===\n')
            select = args[args.index('-Y') + 1]
            if 'tcp.flags.syn' in select: return '1\n'
            if PLAN['origin'] in select: return '0\n0\n'
            return ''
        proof = fixture.analyze_capture(path, PLAN, execute)
        self.assertEqual(proof['analysisExecution'], 'simulated')
        self.assertEqual(proof['nonces'][0]['nonceSha256'], hashlib.sha256(b'abc').hexdigest())
        with self.assertRaises(ValueError): fixture.analyze_capture(path, PLAN, lambda _: '1\n')
    def test_fixture_refuses_current_root_network_namespace(self):
        # Exercise the real constructor gate without observing the host's PID 1
        # namespace or opening listeners. Permission errors remain fail-closed.
        self_path, root_path = '/proc/self/ns/net', '/proc/1/ns/net'
        cases = [
            ('root-namespace', 1001, 'A', 41, 41, 41, None, ValueError, [self_path, root_path]),
            ('wrong-registered-namespace', 1001, 'A', 41, 42, 43, None, ValueError, [self_path]),
            ('root-uid', 0, 'A', 41, 41, 43, None, ValueError, []),
            ('unknown-role', 1001, 'X', 41, 41, 43, None, ValueError, []),
            ('self-unreadable', 1001, 'A', 41, 41, 43, self_path, PermissionError, [self_path]),
            ('root-unreadable', 1001, 'A', 41, 41, 43, root_path, PermissionError, [self_path, root_path]),
            *[('registered-' + role, 1001, role, 41, 41, 43, None, None, [self_path, root_path])
              for role in ('A', 'B', 'O')],
        ]
        for name, uid, role, current, expected, root, denied, error, reads in cases:
            with self.subTest(case=name):
                observed = []
                denied_error = PermissionError(13, 'controlled namespace observation denied')
                def namespace_stat(path, *args, **kwargs):
                    path = str(path)
                    self.assertIn(path, (self_path, root_path))
                    observed.append(path)
                    if path == denied: raise denied_error
                    return SimpleNamespace(st_ino={self_path: current, root_path: root}[path])
                with patch.object(fixture.os, 'geteuid', return_value=uid), \
                     patch.object(fixture.Path, 'stat', autospec=True, side_effect=namespace_stat), \
                     patch.object(fixture.Server, 'listen', autospec=True) as listen, \
                     patch.object(fixture.Server, 'listen_udp', autospec=True) as listen_udp, \
                     patch.object(fixture.socket, 'socket', side_effect=AssertionError('real socket forbidden')) as socket_mock:
                    if error:
                        message = ('ordinary fixture role required' if not reads
                                   else 'registered backend namespace required')
                        if error is PermissionError:
                            with self.assertRaises(PermissionError) as caught:
                                fixture.Server(PLAN, role, expected)
                            self.assertIs(caught.exception, denied_error)
                        else:
                            with self.assertRaisesRegex(ValueError, '^' + message + '$'):
                                fixture.Server(PLAN, role, expected)
                        listen.assert_not_called(); listen_udp.assert_not_called()
                    else:
                        server = fixture.Server(PLAN, role, expected)
                        tcp = ([(PLAN['origin'], PLAN['ports'][kind], kind) for kind in ('echo', 'health')]
                               if role == 'O' else [(PLAN['endpoint'][role], PLAN['ports']['proxy'], 'proxy'),
                                                  (PLAN['endpoint'][role], PLAN['ports']['dns'], 'dns-tcp')])
                        if role == 'A': tcp.append((PLAN['special'], PLAN['ports']['special'], 'special'))
                        self.assertEqual([call.args[1:] for call in listen.call_args_list], tcp)
                        self.assertEqual([call.args[1:] for call in listen_udp.call_args_list],
                                         [] if role == 'O' else [(PLAN['endpoint'][role], PLAN['ports']['dns'])])
                        self.assertIs(server.plan, PLAN); self.assertEqual(server.role, role)
                    self.assertEqual(observed, reads)
                    socket_mock.assert_not_called()


class ReportTests(unittest.TestCase):
    def test_default_graph_requires_current_kernel_without_relabelling_history(self):
        evidence = {'dashboardEnabled': True, 'defaultSandbox': True, 'guiUid': 1001,
                    'startedViaHelper': True, 'kernelCommit': '332935c310e78e15319d434f33a9103329200931',
                    'originalEntry': True, 'stableDoubleRead': True, 'actualGraphSha256': H}
        tun.validate_case('default_graph', evidence)
        for commit in ('0cfa24573ff34c26608151068334329268cfdf14',
                       'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a',
                       '814da362838e96e54f904836a92c2b0d8c3ae29b'):
            with self.subTest(commit=commit):
                historical = dict(evidence, kernelCommit=commit); before = copy.deepcopy(historical)
                with self.assertRaises(ValueError): tun.validate_case('default_graph', historical)
                self.assertEqual(historical, before)

    def test_boolean_case_fabrication_does_not_pass(self):
        cases = {name: {'status': 'passed', 'evidence': {'ok': True}} for name in tun.CASE_IDS}
        result = tun.report_result(cases, {'state': 'normal', 'forced': False}, 'same-final-carrier')
        self.assertEqual(result['result'], 'failed'); self.assertTrue(result['invalidCases'])
    def test_missing_integration_is_blocked_not_skipped_pass(self):
        result = tun.report_result({}, {}, 'source-diagnostic', ['GUI-root-transaction-wiring'])
        self.assertEqual(result['result'], 'BLOCKED'); self.assertFalse(result['publicationEligible'])
    def test_forced_fail_overrides_later_blocker(self):
        result = tun.report_result({}, {'state': 'forced', 'forced': True}, 'source-diagnostic', ['missing-carrier'])
        self.assertEqual(result['result'], 'failed')
    def test_dns_backend_may_rewrite_transaction_id_not_question(self):
        row = {'source': 'owned-tun-capture-and-origin', 'destination': PLAN['origin'], 'interface': 'polaris-tun0',
               'proxyEnvironment': 'absent', 'clientMode': 'direct-tcp', 'packets': 2,
               'captureNonceSha256': H, 'originNonceSha256': H, 'exit': 'A', 'originPeer': PLAN['exit']['A'],
               'analysisExecution': 'real-tool', 'captureSha256': H}
        e = {'A': row, 'B': dict(row, exit='B', originPeer=PLAN['exit']['B']), 'dns': []}
        for transport in ('udp', 'tcp'):
            e['dns'].append({'transport': transport, 'destination': '8.8.8.8', 'tunObserved': True, 'ttl': 0,
                'analysisExecution': 'real-tool', 'querySha256': H, 'captureQuerySha256': H,
                'backendQuerySha256': 'b' * 64, 'captureSha256': H, 'name': 'ab.' + fixture.base.DOMAIN,
                'backendName': 'ab.' + fixture.base.DOMAIN, 'type': 1, 'backendType': 1, 'answer': PLAN['origin']})
        tun.validate_case('tun_data_plane', e)
        e['dns'][0]['backendName'] = 'unrelated.invalid'
        with self.assertRaises(ValueError): tun.validate_case('tun_data_plane', e)
    def test_loopback_or_proxy_evidence_rejected(self):
        row = {'source': 'owned-tun-capture-and-origin', 'destination': PLAN['origin'], 'interface': 'polaris-tun0',
               'proxyEnvironment': 'absent', 'clientMode': 'direct-tcp', 'packets': 2,
               'captureNonceSha256': H, 'originNonceSha256': H, 'exit': 'A', 'originPeer': PLAN['exit']['A'],
               'analysisExecution': 'real-tool', 'captureSha256': H}
        tun.validate_data_plane(row)
        for change in ({'destination': '127.0.0.2'}, {'analysisExecution': 'simulated'}, {'clientMode': 'socks'}, {'originPeer': '127.0.0.1'}, {'captureNonceSha256': 'b' * 64}):
            with self.assertRaises(ValueError): tun.validate_data_plane(dict(row, **change))
    def test_incomplete_core_identity_rejected(self):
        with self.assertRaises(ValueError): tun._birth_pair({'beforeCore': {'pid': 4}, 'afterCore': {'pid': 4}})
    def test_cleanup_requires_cancel_and_reap(self):
        evidence = {'state': 'normal', 'forced': False, 'unknown': [], 'networkRestored': True, 'productUninstalled': True}
        with self.assertRaises(ValueError): tun.validate_case('normal_cleanup', evidence)
    def test_graph_apply_requires_actual_fresh_dns_backend_transition(self):
        def rows(role):
            return [dict(transport=t, destination='8.8.8.8', tunObserved=True, analysisExecution='real-tool',
                         ttl=0, querySha256=H, captureQuerySha256=H, name=role+t+'.fixture.invalid',
                         backendName=role+t+'.fixture.invalid', type=1, backendType=1,
                         answer=PLAN['origin'], backendQuerySha256=H, captureSha256=H, backendRole=role)
                    for t in ('udp', 'tcp')]
        before, after = rows('A'), rows('B')
        tun.validate_dns_rows(before, 'A'); tun.validate_dns_rows(after, 'B')
        for change in (lambda r: r[0].update(backendRole='A'), lambda r: r[0].update(ttl=30),
                       lambda r: r[0].update(analysisExecution='simulated'), lambda r: r.pop()):
            bad = copy.deepcopy(after); change(bad)
            with self.assertRaises(ValueError): tun.validate_dns_rows(bad, 'B')

    def test_selector_not_required_to_increment_graph_revision(self):
        e = {'beforeCore': identity(), 'afterCore': identity(), 'newConnection': {'exit': 'B', 'bytes': 1, 'nonceSha256': H}}
        tun.validate_case('daily_switch', e)
        with self.assertRaises((ValueError, KeyError)): tun.validate_case('graph_apply', e)


class VMObservationTests(unittest.TestCase):
    def collect(self, fault=None):
        hierarchy_reads = [0]
        def hit(stage):
            if stage == fault: raise ValueError('PRIVATE raw host data')
        def text(path, *args, **kwargs):
            value = str(path)
            if value == '/proc/1/comm': hit('pid1'); return 'systemd\n'
            if value == '/proc/1/cgroup':
                hierarchy_reads[0] += 1
                hit('pid1-hierarchy' if hierarchy_reads[0] == 1 else 'pid1-stability')
                return '0::/init.scope\n'
            if value == '/sys/fs/cgroup/init.scope/cgroup.procs': hit('pid1-membership'); return '1\n'
            if value == '/proc/self/mountinfo':
                hit('cgroup-mount'); return '1 0 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n'
            if value == '/proc/self/status': hit('privileges'); return 'NoNewPrivs: 0\n'
            hit('forwarding'); return '0'
        def stat_(path, *args, **kwargs):
            if str(path) == '/proc/1': return SimpleNamespace(st_uid=0)
            if '/ns/' in str(path):
                if fault=='namespace-permission': raise PermissionError('PRIVATE')
                hit('namespaces'); return SimpleNamespace(st_ino=1)
            hit('tun-device'); return SimpleNamespace(st_mode=0o020600, st_rdev=os.makedev(10,200))
        def paths(path, *args, **kwargs): hit('product-paths'); return False
        tool_names=iter(('ip','systemctl','systemd-run','resolvectl','nft','python','setpriv','tcpdump','tshark'))
        def root(path, *args, **kwargs):
            hit('tools'); hit('tool:'+next(tool_names)); return Mock(is_file=lambda: True)
        def network(*args, **kwargs):
            hit('network')
            if fault and fault.startswith('network:'):
                kwargs['progress'](fault.split(':')[1]); raise ValueError('PRIVATE')
            return {'route4': [], 'route6': [], 'forwarding': [0,0]}
        def tables(*args): hit('route-tables'); return []
        ops = Mock()
        def manager(*args):
            if args[0][-1] == 'init.scope':
                hit('pid1-unit')
                key=args[0][-2].split('=',1)[1]
                return key+'='+{'ControlGroup':'','LoadState':'loaded','ActiveState':'active'}[key]+'\n'
            hit('manager'); return 'ControlGroup=\n'
        ops.run.side_effect = manager
        with ExitStack() as stack:
            for target, kwargs in ((('read_text'), {'autospec':True,'side_effect':text}),
                    ('stat', {'autospec':True,'side_effect':stat_}),
                    ('exists', {'autospec':True,'side_effect':paths}), ('is_dir', {'return_value':True}), ('is_symlink', {'return_value':False})):
                stack.enter_context(patch.object(tun.Path,target,**kwargs))
            stack.enter_context(patch.object(tun,'root_path',side_effect=root))
            stack.enter_context(patch.object(tun,'network_snapshot',side_effect=network))
            stack.enter_context(patch.object(tun,'tun_table_routes',side_effect=tables))
            stack.enter_context(patch.object(tun.os,'geteuid',return_value=0))
            stack.enter_context(patch.dict(tun.os.environ,{},clear=True))
            if fault=='pidfd': stack.enter_context(patch.object(tun,'hasattr',side_effect=ValueError('PRIVATE'),create=True))
            if fault=='environment':
                bad=Mock();bad.__contains__=Mock(side_effect=ValueError('PRIVATE'))
                stack.enter_context(patch.object(tun.os,'environ',bad))
            return tun.collect_preflight(1001,{'githubHosted':True,'dedicatedVM':True},True,ops)

    def test_collector_success_and_pending_gate(self):
        facts=self.collect(); obs=tun.validate_vm_observation(facts['vmPreflight'])
        self.assertEqual(obs['managerControlGroup'],'empty-root')
        self.assertTrue(all(row['state']=='observed' for k,row in obs['stages'].items() if k!='gate'))
        self.assertEqual(tun.preflight(facts,carrier())['blocked'],['realPkexecAuthenticationPendingConfirmation'])

    def test_each_io_stage_failure_is_bounded_and_remaining_not_reached(self):
        for stage in ('manager','pid1','namespaces','cgroup-mount','privileges','tun-device',
                      'forwarding','tools','product-paths','network','route-tables','pidfd','environment'):
            with self.subTest(stage=stage):
                facts=self.collect(stage); obs=tun.validate_vm_observation(facts['vmPreflight'])
                self.assertEqual(obs['stages'][stage],{'state':'failed','error':'ValueError'})
                remaining=tun.VM_STAGES[tun.VM_STAGES.index(stage)+1:]
                self.assertTrue(all(obs['stages'][name]['state']=='not-reached' for name in remaining))
                self.assertFalse(facts['networkSnapshotComplete'])
                self.assertIn('localEvidenceAndMaterials',tun.preflight(facts,carrier())['blocked'])
                self.assertNotIn('PRIVATE',json.dumps(obs))

    def test_manager_property_presence_and_root_variants(self):
        for raw,expected in (('ControlGroup=\n',True),('ControlGroup=/\n',True),('ControlGroup=/user.slice\n',False)):
            ops=Mock();ops.run.return_value=raw;obs=tun.vm_observation()
            with patch.object(tun.os,'geteuid',return_value=0): self.assertEqual(tun.manager_root(ops,obs),expected)
            self.assertNotIn('--value',ops.run.call_args.args[0])
        for raw in ('','\n','Unknown=','ControlGroup=\nControlGroup=/','ControlGroup=\nOther=x'):
            with self.subTest(raw=raw),self.assertRaises(ValueError):
                tun.manager_root(Mock(run=Mock(return_value=raw)),tun.vm_observation())
        with patch.object(tun.os,'geteuid',return_value=1001):
            self.assertFalse(tun.manager_root(Mock(run=Mock(return_value='ControlGroup=')),tun.vm_observation()))

    def test_observation_rejects_injected_unbounded_or_false_success_fields(self):
        for change in (lambda o:o.update(raw='secret'),lambda o:o['checks'].update(rootManager='yes'),
                       lambda o:o['stages']['manager'].update(error='PRIVATE'),
                       lambda o:o.update(blocked=['PRIVATE']),lambda o:o.update(gate='ready')):
            obs=tun.vm_observation();change(obs)
            with self.assertRaises(ValueError):tun.validate_vm_observation(obs)

    def test_each_tool_and_network_failure_has_exact_safe_label(self):
        tools=('ip','systemctl','systemd-run','resolvectl','nft','python','setpriv','tcpdump','tshark')
        network=('links','addresses','route4','route6','rule4','rule6','resolved','firewall','resolv-conf','network-forwarding')
        for prefix,names in (('tool:',tools),('network:',network)):
            for name in names:
                with self.subTest(probe=name):
                    facts=self.collect(prefix+name);obs=facts['vmPreflight']
                    tun.validate_vm_observation(obs)
                    self.assertEqual(obs['failedProbe'],name)
                    self.assertEqual(obs['stages']['tools' if prefix=='tool:' else 'network']['state'],'failed')
                    if prefix=='tool:':self.assertIsNone(obs['checks']['toolsRootOwned'])
                    self.assertNotIn('PRIVATE',json.dumps(obs))

    def test_real_network_snapshot_reports_each_fixed_probe_before_call(self):
        ops=Mock();ops.json.return_value=[];ops.run.return_value='PRIVATE raw resolver'
        probes=[]
        with patch.object(tun.Path,'is_symlink',return_value=False),patch.object(tun.Path,'read_bytes',return_value=b'raw'), \
             patch.object(tun.Path,'read_text',return_value='0'):
            tun.network_snapshot(ops,progress=probes.append)
        self.assertEqual(probes,['links','addresses','route4','route6','rule4','rule6','resolved','firewall','resolv-conf','network-forwarding'])

    def test_unknown_checks_cannot_claim_authentication_only_pending(self):
        obs=tun.vm_observation();obs['gate']='blocked'
        obs['blocked']=['realPkexecAuthenticationPendingConfirmation']
        obs['stages']['gate']={'state':'observed','error':'none'}
        with self.assertRaises(ValueError):tun.validate_vm_observation(obs)
        obs['stages']={name:{'state':'observed','error':'none'} for name in tun.VM_STAGES}
        obs['checks']=dict.fromkeys(tun.VM_CHECKS,True);obs['managerControlGroup']='empty-root';obs['forwarding']=[0,0]
        tun.validate_vm_observation(obs)
        obs['checks']['networkSnapshotComplete']=None
        with self.assertRaises(ValueError):tun.validate_vm_observation(obs)

    def test_supported_baselines_are_not_normalized_and_invalid_or_drift_is_blocked(self):
        for initial in ([0,0],[1,0],[0,1],[1,1]):
            facts=good_facts();facts['forwarding']=list(initial)
            self.assertEqual(tun.preflight(facts,carrier())['blocked'],['realPkexecAuthenticationPendingConfirmation'])
            self.assertEqual(facts['forwarding'],initial)
        for invalid in (None,[],[0],[0,0,0],[True,0],[1.0,0],[2,0],['1',0]):
            facts=good_facts();facts['forwarding']=invalid
            self.assertIn('VMPrivilegesOrForwardingBaseline',tun.preflight(facts,carrier())['blocked'])
        facts=good_facts();facts['forwardingStable']=False
        self.assertIn('VMPrivilegesOrForwardingBaseline',tun.preflight(facts,carrier())['blocked'])

    def test_exception_classes_are_fixed(self):
        for e,wanted in ((FileNotFoundError('PRIVATE'),'OSError'),(subprocess.TimeoutExpired('secret',1),'SubprocessError'),
                         (KeyError('secret'),'KeyError'),(RuntimeError('secret'),'other')):
            self.assertEqual(tun.vm_error(e),wanted)



class TransactionIdentityTests(unittest.TestCase):
    def observe(self, *, change=None, second_argv=None, second_uid=None, grow=False):
        with tempfile.TemporaryFile() as f:
            f.write(b'pure-test-executable');f.flush();f.seek(0);st=os.fstat(f.fileno())
            row={'pid':321,'startTime':'123','state':'S','ppid':123,'uid':1001,'exeDev':st.st_dev,
                 'exeInode':st.st_ino,'exe':'/usr/bin/pkexec','namespaces':dict(net=1,mnt=2,pid=3,cgroup=4),'cgroup':'0::/test'}
            after={**row,**(change or {})}
            status='Uid:\t1001\t0\t0\t0\n';argv=b'/usr/bin/pkexec\0/bin/bash\0/private.sh\0'
            original_read=os.read
            def read(fd,n):
                value=original_read(fd,n)
                if grow and not value:return b'x'
                return value
            with patch.object(tun,'process_identity',side_effect=[row,after]), \
                 patch.object(tun.os,'open',return_value=os.dup(f.fileno())), \
                 patch.object(tun.os,'read',side_effect=read), \
                 patch.object(Path,'read_text',side_effect=[status,second_uid or status]), \
                 patch.object(Path,'read_bytes',side_effect=[argv,second_argv or argv]):
                return tun.transaction_identity(321)
    def test_held_fd_and_actual_five_identity_fields(self):
        value=self.observe();self.assertEqual(value['uids'],(1001,0,0,0))
        self.assertEqual(value['executable']['sha256'],hashlib.sha256(b'pure-test-executable').hexdigest())
        self.assertEqual(value['identity']['exeInode'],value['executable']['inode'])
    def test_birth_exe_uid_parent_and_namespace_changes_rejected(self):
        for changed in ({'startTime':'124'},{'exeInode':999},{'uid':0},{'ppid':999},{'namespaces':dict(net=9,mnt=2,pid=3,cgroup=4)}):
            with self.subTest(changed=changed),self.assertRaises(ValueError):self.observe(change=changed)
    def test_second_inputs_must_match_before_final_identity(self):
        with self.assertRaises(ValueError):self.observe(second_argv=b'/other\0')
        with self.assertRaises(ValueError):self.observe(second_uid='Uid:\t0\t0\t0\t0\n')
    def test_growing_file_is_bounded_and_rejected(self):
        with self.assertRaises(ValueError):self.observe(grow=True)


class TransactionRealProcTests(unittest.TestCase):
    """Owned ordinary test children only: no pkexec, helper, account or product."""
    def child(self, action):
        import sys
        code="import os,sys; print('ready',flush=True); sys.stdin.buffer.read(1); "+action
        process=subprocess.Popen([sys.executable,'-I','-c',code],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
        self.addCleanup(self.close_child,process)
        self.assertEqual(process.stdout.readline(),b'ready\n')
        return process
    @staticmethod
    def close_child(process):
        if process.poll() is None:process.terminate()
        process.wait(timeout=3)
        if process.stdin:process.stdin.close()
        if process.stdout:process.stdout.close()
    def test_real_same_birth_exec_changes_strong_identity_without_pid_reuse(self):
        import time
        process=self.child("os.execv('/bin/sleep',['/bin/sleep','20'])")
        before=tun.transaction_identity(process.pid)
        process.stdin.write(b'x');process.stdin.flush()
        deadline=time.monotonic()+3
        while True:
            try:after=tun.transaction_identity(process.pid)
            except tun.TransactionIdentityError:
                if time.monotonic()>=deadline:raise
                continue
            if after['identity']['exeInode']!=before['identity']['exeInode']:break
            self.assertLess(time.monotonic(),deadline);time.sleep(.001)
        self.assertEqual(before['identity']['startTime'],after['identity']['startTime'])
        self.assertEqual(before['identity']['pid'],after['identity']['pid'])
        self.assertFalse(tun.same_process(before['identity'],after['identity']))
        self.assertEqual(after['argv'],['/bin/sleep','20'])
        self.assertEqual(after['uids'],(os.getuid(),)*4)
    def test_real_exit_after_inventory_remains_unconfirmed_not_admitted(self):
        process=self.child('sys.exit(0)')
        before=tun.transaction_identity(process.pid)
        process.stdin.write(b'x');process.stdin.flush();process.wait(timeout=3)
        self.assertGreater(before['identity']['pid'],1)
        with self.assertRaises(tun.TransactionIdentityError) as caught:tun.transaction_identity(process.pid)
        self.assertEqual(caught.exception.phase,'identity-before')
        self.assertEqual(str(caught.exception),'transaction candidate observation unconfirmed')
    def test_real_zombie_before_reap_remains_unconfirmed(self):
        import time
        process=self.child('sys.exit(0)')
        before=tun.transaction_identity(process.pid)
        process.stdin.write(b'x');process.stdin.flush()
        deadline=time.monotonic()+3
        while True:
            raw=Path('/proc',str(process.pid),'stat').read_text()
            fields=raw[raw.rindex(')')+2:].split()
            if fields[0]=='Z':break
            self.assertLess(time.monotonic(),deadline);time.sleep(.001)
        self.assertEqual(fields[19],before['identity']['startTime'])
        with self.assertRaises(tun.TransactionIdentityError):tun.transaction_identity(process.pid)
        process.wait(timeout=3)


class TransactionDiagnosticTests(unittest.TestCase):
    observe = TransactionIdentityTests.observe
    def test_fixed_stage_matrix_never_includes_original_exception(self):
        for error,expected in ((PermissionError(13,'secret-path'),'eacces'),(FileNotFoundError(2,'secret-path'),'enoent'),
                               (ProcessLookupError(3,'secret-path'),'esrch'),(ValueError('secret-path'),'contract')):
            with patch.object(tun,'process_identity',side_effect=error):
                with self.assertRaises(tun.TransactionIdentityError) as caught:tun.transaction_identity(123)
            self.assertEqual(caught.exception.phase,'identity-before');self.assertEqual(caught.exception.error,expected)
            self.assertNotIn('secret-path',str(caught.exception))
    def test_input_change_and_exec_change_have_consistency_stage(self):
        for options in ({'change':{'exeInode':999}},{'second_argv':b'/other\0'}):
            with self.assertRaises(tun.TransactionIdentityError) as caught:self.observe(**options)
            self.assertEqual(caught.exception.phase,'snapshot-consistency')
    def test_executable_growth_has_read_stage(self):
        with self.assertRaises(tun.TransactionIdentityError) as caught:self.observe(grow=True)
        self.assertEqual(caught.exception.phase,'executable-read')


class OwnedTerminalTests(unittest.TestCase):
    child = TransactionRealProcTests.child
    close_child = staticmethod(TransactionRealProcTests.close_child)
    def test_owned_exit_and_zombie_use_whole_directory_or_stable_stat(self):
        import time
        process=self.child('sys.exit(0)');old=tun.transaction_identity(process.pid)['identity']
        process.stdin.write(b'x');process.stdin.flush()
        end=time.monotonic()+3
        while Path('/proc',str(process.pid),'stat').read_text().split(') ',1)[1][0]!='Z':
            self.assertLess(time.monotonic(),end);time.sleep(.001)
        self.assertEqual(tun.terminal_identity(old)['state'],'zombie')
        process.wait(timeout=3)
        self.assertEqual(tun.terminal_identity(old)['state'],'absent')
    def test_reuse_permission_partial_file_disappearance_and_reparent_rejected(self):
        old=identity();fields=['Z',str(old['ppid'])]+['0']*17+[old['startTime']]
        raw=f"{old['pid']} (fixture) "+' '.join(fields)
        for fault in ('permission','partial','reuse','reparent','changed'):
            bad=list(fields)
            if fault=='reuse':bad[19]='9999'
            if fault=='reparent':bad[1]='9999'
            text=f"{old['pid']} (fixture) "+' '.join(bad)
            with patch.object(Path,'lstat',side_effect=PermissionError(13,'hidden') if fault=='permission' else [Mock(),Mock()]), \
                 patch.object(Path,'read_text',side_effect=FileNotFoundError(2,'stat') if fault=='partial' else [raw,text] if fault=='changed' else [text,text]):
                if fault=='changed':
                    # First directory absent then present is not stable absence.
                    with patch.object(Path,'lstat',side_effect=[FileNotFoundError(2,'directory'),Mock()]):
                        with self.assertRaises(ValueError):tun.terminal_identity(old)
                else:
                    with self.assertRaises((ValueError,OSError)):tun.terminal_identity(old)
    def test_original_readonly_sample_is_unit_counterexample_not_root_authority(self):
        sample=json.loads((Path(__file__).parent/'fixtures/n16-readonly-host-cgroup.json').read_text())
        self.assertEqual(sample['context']['runId'],'40074321479')
        host=sample['hostFacts']['observations']
        self.assertEqual(host['initScope']['value'],{'active':'active','controlGroup':'explicit-empty','load':'loaded'})
        # The original exact /init.scope equality necessarily rejects explicit empty.
        self.assertNotEqual(host['initScope']['value']['controlGroup'],'init-scope')
        for key in ('pidNamespace','mntNamespace','cgroupNamespace'):
            self.assertEqual(host[key],{'error':'permission','state':'unknown','value':None})
        # Supplemented root fixture passes; ordinary permission sample alone does not.
        strong=VMObservationTests().collect()['vmPreflight']
        self.assertEqual(strong['checks']['pid1Systemd'],True)
        self.assertEqual(strong['checks']['sameSystemNamespaces'],True)
        unknown_facts=VMObservationTests().collect('namespace-permission')
        unknown_gate=tun.preflight(unknown_facts,carrier())
        unknown=unknown_facts['vmPreflight']
        self.assertEqual(unknown['stages']['namespaces'],{'state':'failed','error':'OSError'})
        self.assertEqual(unknown['checks']['sameSystemNamespaces'],None)
        self.assertIn('sameSystemNamespaces',unknown_gate['blocked'])
        for probe in ('pid1-hierarchy','pid1-unit','pid1-membership','pid1-stability'):
            value=VMObservationTests().collect(probe)['vmPreflight']
            self.assertEqual(value['failedProbe'],probe)
            self.assertEqual(tun.validate_vm_observation(value),value)

    def test_pid1_root_cgroup_exact_semantics(self):
        cases = [('0::/','', '1\n',True), ('0::/','','2\n',False), ('0::/init.scope','','1\n2\n',True),
            ('0::/init.scope','/init.scope','1\n',True), ('0::/nested/init.scope','','1\n',False),
            ('0::/init.scope','/','1\n',False), ('0::/init.scope','/other','1\n',False),
            ('0::/init.scope','','2\n',False), ('0::/init.scope','','PRIVATE\n',False),
            ('0::/init.scope\n0::/','','1\n',False)]
        for raw,group,members,expected in cases:
            def read(path):return members if str(path).endswith('cgroup.procs') else raw+'\n'
            def query(argv):
                key=argv[-2].split('=',1)[1]
                return key+'='+{'ControlGroup':group,'LoadState':'loaded','ActiveState':'active'}[key]+'\n'
            with self.subTest(raw=raw,group=group,members=members), \
                 patch.object(Path,'read_text',autospec=True,side_effect=read), \
                 patch.object(Path,'is_dir',return_value=True),patch.object(Path,'is_symlink',return_value=False):
                ops=Mock(run=Mock(side_effect=query))
                if expected:self.assertTrue(tun.pid1_root_cgroup(ops))
                else:
                    with self.assertRaises(ValueError):tun.pid1_root_cgroup(ops)

    def test_pid1_unit_missing_duplicate_errors_and_kernel_changes_reject(self):
        for fault in ('missing','duplicate','inactive','not-found','query','permission','second-members','changed','symlink'):
            calls={'group':0,'members':0}
            def read(path):
                if str(path).endswith('cgroup.procs'):
                    calls['members']+=1
                    if fault=='permission':raise PermissionError('PRIVATE')
                    return '2\n' if fault=='second-members' and calls['members']==2 else '1\n'
                calls['group']+=1
                return '0::/\n' if fault=='changed' and calls['group']==2 else '0::/init.scope\n'
            def query(argv):
                key=argv[-2].split('=',1)[1]
                if fault=='query':raise RuntimeError('PRIVATE')
                if fault=='missing':return ''
                value={'ControlGroup':'','LoadState':'not-found' if fault=='not-found' else 'loaded',
                       'ActiveState':'inactive' if fault=='inactive' else 'active'}[key]
                return (key+'='+value+'\n')*(2 if fault=='duplicate' else 1)
            with self.subTest(fault=fault),patch.object(Path,'read_text',autospec=True,side_effect=read), \
                 patch.object(Path,'is_dir',return_value=True),patch.object(Path,'is_symlink',return_value=fault=='symlink'):
                with self.assertRaises((ValueError,OSError,RuntimeError)):
                    tun.pid1_root_cgroup(Mock(run=Mock(side_effect=query)))

class OrdinaryExitCaptureProbeMatrix(unittest.TestCase):
    def test_real_exit_at_each_capture_boundary_has_specific_exit_probe(self):
        import sys
        for point, expected in (
            ('identity-before',('identity-before','initial-live-state')),
            ('executable-open',('executable-open','exe-open')),
            ('executable-initial',('inputs-before','cmdline-empty')),
            ('inputs-before',('inputs-before','cmdline-empty')),
            ('executable-read',('inputs-after','cmdline-empty')),
            ('inputs-after',('inputs-after','cmdline-empty')),
            ('identity-after',('identity-after','final-live-state'))):
            with self.subTest(point=point):
                child=subprocess.Popen([sys.executable,'-c',"import os;os.write(1,b'R');os.read(0,1)"],
                    stdin=subprocess.PIPE,stdout=subprocess.PIPE)
                self.assertEqual(child.stdout.read(1),b'R')
                prior=tun.transaction_identity(child.pid)
                state={'exit':False,'fd':None,'fstat':0,'inputs':0}
                def release():
                    if state['exit']:return
                    state['exit']=True;child.stdin.write(b'X');child.stdin.flush()
                    os.waitid(os.P_PID,child.pid,os.WEXITED|os.WNOWAIT)
                real_identity=tun.process_identity;real_open=os.open;real_fstat=os.fstat;real_read=os.read;real_bytes=Path.read_bytes
                def identify(pid):
                    if point=='identity-before':release()
                    value=real_identity(pid)
                    if point=='executable-open':release()
                    return value
                def opening(path,flags,*args,**kwargs):
                    fd=real_open(path,flags,*args,**kwargs)
                    if str(path)==f'/proc/{child.pid}/exe':
                        state['fd']=fd
                        if point=='executable-initial':release()
                    return fd
                def fstat(fd):
                    value=real_fstat(fd)
                    if fd==state['fd']:
                        state['fstat']+=1
                        if point=='inputs-before' and state['fstat']==1:release()
                    return value
                def read(fd,count):
                    if fd==state['fd'] and (point=='executable-read' or point=='inputs-after' and count==1):release()
                    return real_read(fd,count)
                def read_bytes(path):
                    value=real_bytes(path)
                    if str(path)==f'/proc/{child.pid}/cmdline':
                        state['inputs']+=1
                        if point=='identity-after' and state['inputs']==2:release()
                    return value
                try:
                    with patch.object(tun,'process_identity',side_effect=identify),patch.object(os,'open',side_effect=opening),                          patch.object(os,'fstat',side_effect=fstat),patch.object(os,'read',side_effect=read),                          patch.object(Path,'read_bytes',autospec=True,side_effect=read_bytes):
                        with self.assertRaises(tun.TransactionIdentityError) as caught:tun.transaction_identity(child.pid)
                    self.assertEqual((caught.exception.phase,caught.exception.probe),expected)
                    self.assertEqual(tun.terminal_identity(prior['identity'])['state'],'zombie')
                    child.wait(timeout=2)
                    self.assertEqual(tun.terminal_identity(prior['identity'])['state'],'absent')
                    unverified=tun.unverified_terminal_identity(prior['identity'])
                    self.assertEqual(unverified['scope'],'sampled-terminal-unverified-execution')
                    self.assertNotIn('uid',unverified);self.assertNotIn('exe',unverified)
                finally:
                    if child.poll() is None:release();child.wait(timeout=2)
                    child.stdin.close();child.stdout.close()
    def test_whole_missing_is_not_partial_file_or_permission_absence(self):
        with patch.object(tun,'process_identity',return_value=None),patch.object(Path,'lstat',side_effect=FileNotFoundError(2,'private')):
            with self.assertRaises(tun.TransactionIdentityError) as caught:tun.transaction_identity(99999)
        self.assertEqual(caught.exception.probe,'initial-directory-absent')
        with patch.object(tun,'process_identity',return_value=None),patch.object(Path,'lstat',return_value=Mock()):
            with self.assertRaises(tun.TransactionIdentityError) as caught:tun.transaction_identity(99999)
        self.assertEqual(caught.exception.probe,'initial-live-state')
        with patch.object(tun,'process_identity',return_value=None),patch.object(Path,'lstat',side_effect=PermissionError(13,'private')):
            with self.assertRaises(tun.TransactionIdentityError) as caught:tun.transaction_identity(99999)
        self.assertEqual(caught.exception.error,'eacces')
        self.assertNotIn('private',str(caught.exception))


class MonotonicTerminalContracts(unittest.TestCase):
    def test_exact_zombie_then_two_whole_directory_absences(self):
        old=identity();raw=f"{old['pid']} (fixture) "+' '.join(['Z',str(old['ppid'])]+['0']*17+[old['startTime']])
        with patch.object(Path,'lstat',side_effect=[Mock(),FileNotFoundError(2,'directory'),FileNotFoundError(2,'directory')]) as dirs, \
             patch.object(Path,'read_text',return_value=raw):
            proof=tun.terminal_identity(old)
        self.assertEqual(proof['state'],'absent');self.assertEqual(dirs.call_count,3)
        self.assertNotIn('reaped',proof)

    def test_transition_negative_matrix_has_fixed_probe_or_errno(self):
        old=identity();fields=['Z',str(old['ppid'])]+['0']*17+[old['startTime']]
        raw=lambda v:f"{old['pid']} (fixture) "+' '.join(v)
        cases=[('live',['S']+fields[1:],'terminal-live'),
               ('reuse',fields[:19]+['9999'],'terminal-birth'),
               ('reparent',[fields[0],'9999']+fields[2:],'terminal-parent'),
               ('malformed',[],'terminal-format')]
        for _,v,probe in cases:
            with patch.object(Path,'lstat',return_value=Mock()),patch.object(Path,'read_text',return_value=raw(v)):
                with self.assertRaises(tun.TerminalIdentityError) as error:tun.terminal_identity(old)
            self.assertEqual(error.exception.probe,probe)
        for last in (Mock(),PermissionError(13,'private')):
            with patch.object(Path,'lstat',side_effect=[Mock(),FileNotFoundError(2,'directory'),last]), \
                 patch.object(Path,'read_text',return_value=raw(fields)):
                with self.assertRaises((tun.TerminalIdentityError,PermissionError)):tun.terminal_identity(old)
        with patch.object(Path,'lstat',side_effect=[FileNotFoundError(2,'directory'),Mock()]), \
             patch.object(Path,'read_text',return_value=raw(fields)):
            with self.assertRaises(tun.TerminalIdentityError) as error:tun.terminal_identity(old)
        self.assertEqual(error.exception.probe,'terminal-transition')
        with patch.object(Path,'lstat',return_value=Mock()),patch.object(Path,'read_text',side_effect=FileNotFoundError(2,'stat')):
            with self.assertRaises(FileNotFoundError):tun.terminal_identity(old)


class StructuredProcObservationTests(unittest.TestCase):
    def contexts(self, probe=None, error_type=FileNotFoundError, reuse=False):
        count=[0]
        def failure(current):
            if current==probe:raise error_type(2 if error_type is FileNotFoundError else 13,'PRIVATE')
        def text(path,*args,**kwargs):
            name=str(path)
            if name.endswith('/stat'):
                count[0]+=1;current='stat-before' if count[0]%2 else 'stat-after';failure(current)
                birth='999' if reuse and current=='stat-after' else '88'
                return '123 (fixture) '+' '.join(['S','20']+['0']*17+[birth])
            if name.endswith('/status'):
                failure('status');return 'Uid: 1000 1000 1000 1000\nCapEff: 0\nCapAmb: 0\n'
            failure('cgroup');return '0::/fixture'
        def stat(path,*args,**kwargs):
            name=str(path)
            if name.endswith('/exe'):failure('exe-stat');return SimpleNamespace(st_dev=1,st_ino=77)
            current='namespace-'+path.name;failure(current);return SimpleNamespace(st_ino=2)
        def link(path,*args,**kwargs):failure('exe-link');return '/usr/bin/fixed-test'
        return (patch.object(Path,'read_text',text),patch.object(Path,'stat',stat),patch.object(tun.os,'readlink',link))
    def test_every_missing_field_is_named_and_legacy_none_is_preserved(self):
        for probe in tun.PROCESS_PROBES:
            with ExitStack() as stack:
                for context in self.contexts(probe):stack.enter_context(context)
                value=tun.observe_process_identity(123)
            self.assertIsNone(value['identity']);self.assertEqual(value['missing'],{'probe':probe,'error':'enoent'})
            with ExitStack() as stack:
                for context in self.contexts(probe):stack.enter_context(context)
                self.assertIsNone(tun.process_identity(123))
            self.assertNotIn('PRIVATE',json.dumps(value))
    def test_permission_never_becomes_missing_and_reuse_is_contract_error(self):
        for probe in tun.PROCESS_PROBES:
            with ExitStack() as stack:
                for context in self.contexts(probe,PermissionError):stack.enter_context(context)
                with self.assertRaises(PermissionError) as caught:tun.observe_process_identity(123)
            self.assertEqual(caught.exception.process_probe,probe)
        with ExitStack() as stack:
            for context in self.contexts(reuse=True):stack.enter_context(context)
            with self.assertRaises(ValueError) as caught:tun.observe_process_identity(123)
        self.assertEqual(caught.exception.process_probe,'stat-after')
    def test_complete_result_and_legacy_identity_have_identical_fields(self):
        with ExitStack() as stack:
            for context in self.contexts():stack.enter_context(context)
            observation=tun.observe_process_identity(123);identity=tun.process_identity(123)
        self.assertEqual(identity,observation['identity']);self.assertIsNone(observation['missing'])
        self.assertTrue(tun.same_process(identity,identity))

class LedgerStageDiagnosticTests(unittest.TestCase):
    def ledger(self,directory):
        value=tun.Ledger(TOKEN);value.directory=Path(directory);value.path=value.directory/'ledger.json'
        (value.directory/'ledger.lock').write_bytes(b'');return value
    def test_private_ordinary_io_load_save_mutate_and_fixed_failures(self):
        # Root ownership checks are explicitly mocked; only this private user
        # temp directory is touched. This is not a privileged ledger proof.
        with tempfile.TemporaryDirectory() as directory,patch.object(tun.os,'geteuid',return_value=0), \
             patch.object(tun,'root_path',side_effect=lambda path,**kwargs:Path(path)):
            ledger=self.ledger(directory);value={'schema':tun.SCHEMA,'runId':TOKEN,'count':0}
            ledger.save(value);self.assertEqual(ledger.load(),value)
            self.assertEqual(ledger.mutate(lambda row:row.update(count=1))['count'],1)
            def reject(row):raise ValueError('PRIVATE CHANGE')
            with self.assertRaises(ValueError) as caught:ledger.mutate(reject)
            self.assertEqual((caught.exception.ledger_operation,caught.exception.ledger_probe),('mutate','change'))
            ledger.path.write_text('{invalid')
            with self.assertRaises(ValueError) as caught:ledger.load()
            self.assertEqual((caught.exception.ledger_operation,caught.exception.ledger_probe),('load','decode'))
    def test_save_encode_fsync_replace_and_cleanup_keep_first_original_error(self):
        for fault in ('encode','file-fsync','replace','directory-fsync','replace-and-cleanup'):
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as directory, \
                 patch.object(tun.os,'geteuid',return_value=0),patch.object(tun,'root_path',side_effect=lambda path,**kwargs:Path(path)):
                ledger=self.ledger(directory);value={'schema':tun.SCHEMA,'runId':TOKEN};count=[0]
                real_fsync=os.fsync
                def fsync(fd):
                    count[0]+=1
                    if (fault=='file-fsync' and count[0]==1) or (fault=='directory-fsync' and count[0]==2):raise OSError(5,'PRIVATE FSYNC')
                    return real_fsync(fd)
                with ExitStack() as stack:
                    stack.enter_context(patch.object(tun.os,'fsync',side_effect=fsync))
                    if fault=='encode':stack.enter_context(patch.object(tun,'json_bytes',side_effect=ValueError('PRIVATE ENCODE')))
                    if fault.startswith('replace'):stack.enter_context(patch.object(tun.os,'replace',side_effect=OSError(5,'PRIVATE REPLACE')))
                    if fault=='replace-and-cleanup':stack.enter_context(patch.object(Path,'unlink',side_effect=PermissionError(13,'PRIVATE CLEANUP')))
                    with self.assertRaises((ValueError,OSError)) as caught:ledger.save(value)
                self.assertEqual(caught.exception.ledger_operation,'save')
                self.assertEqual(caught.exception.ledger_probe,'replace' if fault.startswith('replace') else fault)
                if fault=='replace-and-cleanup':self.assertTrue(caught.exception.ledger_cleanup_failed)

if __name__ == '__main__': unittest.main()

