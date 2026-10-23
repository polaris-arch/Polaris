"""Pure/mocked tests: no privilege, sockets, product, namespaces or system changes."""
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import socket
import struct
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
            'guiUid': 1001, 'noNewPrivs': 0, 'forwarding': [0, 0],
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
        result = tun.preflight(good_facts(), carrier(), True)
        self.assertEqual(result['status'], 'READY_FOR_LOCAL_PREPARATION')
        self.assertFalse(result['publicationEligible'])
    def test_missing_carrier_and_auth_block(self):
        result = tun.preflight(good_facts())
        self.assertEqual(result['status'], 'BLOCKED')
        self.assertIn('c6CompleteCarrierAdmission', result['blocked'])
        self.assertIn('realPkexecAuthenticationPendingConfirmation', result['blocked'])
    def test_old_kernel_not_c6(self):
        c = carrier(); c['kernelCommit'] = '8' * 40
        self.assertIn('c6CompleteCarrierAdmission', tun.preflight(good_facts(), c, True)['blocked'])
    def test_root_gui_sandbox_and_sysctl_rejected(self):
        facts = good_facts(); facts.update(guiUid=0, forwarding=[1, 0], unsafeEnvironment=['LD_PRELOAD'])
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
        with self.assertRaises(ValueError): fixture.Server(PLAN, 'A', Path('/proc/1/ns/net').stat().st_ino)


class ReportTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
