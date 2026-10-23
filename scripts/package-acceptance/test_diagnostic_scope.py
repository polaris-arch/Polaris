"""Local-only diagnostic writer contracts; no product/system/network operations."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import types
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('diagnostic_scope', Path(__file__).with_name('diagnostic_scope.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def environment():
    return {'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '39947115011',
            'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_JOB': 'linux_package_acceptance'}


class DiagnosticScopeTests(unittest.TestCase):
    def test_all_lanes(self):
        for lane, kind in (('plain', 'both'), ('tun', 'deb'), ('tun', 'AppImage')):
            with self.subTest(lane=lane, kind=kind):
                value = m.build_scope(environment(), lane, kind)
                self.assertEqual(value['schema'], m.SCHEMA)
                self.assertFalse(value['acceptance'])
                self.assertFalse(value['publicationEligible'])
                self.assertEqual(value['cleanup'], 'not-assessed')

    def test_preparation_failure_runtime_skipped(self):
        env = environment() | {'DIAGNOSTIC_TEST_TOOLS_OUTCOME': 'failure',
                               'DIAGNOSTIC_RUNTIME_OUTCOME': 'skipped'}
        value = m.build_scope(env, 'plain', 'both')
        self.assertEqual(value['runtime'], 'not-started')
        self.assertEqual(value['stepOutcomes']['test_tools'], 'failure')
        self.assertNotIn('result', value)

    def test_runtime_not_inferred_from_missing_or_failed_preparation(self):
        for outcome in ('', 'success', 'failure', 'cancelled'):
            value = m.build_scope(environment() | {'DIAGNOSTIC_RUNTIME_OUTCOME': outcome}, 'tun', 'deb')
            self.assertEqual(value['runtime'], 'attempted-or-unknown')
            self.assertEqual(value['cleanup'], 'not-assessed')
            self.assertFalse(value['acceptance'])

    def test_missing_steps_are_not_passes(self):
        value = m.build_scope(environment(), 'plain', 'both')
        self.assertEqual(set(value['stepOutcomes'].values()), {'not-recorded'})
        self.assertEqual(tuple(value['stepOutcomes']), m.STEPS)

    def test_reject_unrecognized_outcome(self):
        for bad in ('PASS', 'success\nsecret', {}, None, 'not-recorded'):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                m.build_scope(environment() | {'DIAGNOSTIC_RUNTIME_OUTCOME': bad}, 'plain', 'both')

    def test_no_unlisted_environment_export(self):
        env = environment() | {'GH_TOKEN': 'secret-value', 'GITHUB_EVENT_PATH': '/private/event',
                               'HOME': '/private/home', 'DIAGNOSTIC_EXTRA_OUTCOME': 'secret-value'}
        data = json.dumps(m.build_scope(env, 'plain', 'both'))
        for forbidden in ('secret-value', '/private', 'GH_TOKEN', 'GITHUB_EVENT_PATH', 'EXTRA'):
            self.assertNotIn(forbidden, data)

    def test_bad_identity_rejected_without_value_echo(self):
        for key in environment():
            for bad in ('secret\nvalue', '', None):
                with self.subTest(key=key, bad=bad), self.assertRaises(ValueError) as caught:
                    m.build_scope(environment() | {key: bad}, 'plain', 'both')
                self.assertNotIn('secret', str(caught.exception))

    def test_lane_kind_cross_product_rejected(self):
        for lane, kind in (('plain', 'deb'), ('tun', 'both'), ('unknown', 'both'), ('tun', '../deb')):
            with self.subTest(lane=lane, kind=kind), self.assertRaises(ValueError):
                m.build_scope(environment(), lane, kind)

    def test_file_does_not_create_runtime_receipt(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = m.build_scope(environment(), 'plain', 'both')
            output = m.write_scope(temp, scope)
            self.assertEqual(output.relative_to(temp).as_posix(), 'linux-package-evidence/scope.json')
            self.assertEqual(json.loads(output.read_text()), scope)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            self.assertEqual([p.name for p in output.parent.iterdir()], ['scope.json'])
            self.assertLess(output.stat().st_size, 8192)

    def test_existing_scope_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = m.build_scope(environment(), 'tun', 'AppImage')
            output = m.write_scope(temp, scope)
            before = output.read_bytes()
            with self.assertRaises(FileExistsError):
                m.write_scope(temp, scope)
            self.assertEqual(output.read_bytes(), before)

    def test_directory_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); (root / 'outside').mkdir()
            (root / 'linux-tun-evidence').symlink_to(root / 'outside', target_is_directory=True)
            with self.assertRaises(ValueError):
                m.write_scope(root, m.build_scope(environment(), 'tun', 'deb'))
            self.assertEqual(list((root / 'outside').iterdir()), [])

    def test_output_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); directory = root / 'linux-package-evidence'; directory.mkdir()
            (directory / 'scope.json').symlink_to(root / 'must-not-exist')
            with self.assertRaises(FileExistsError):
                m.write_scope(root, m.build_scope(environment(), 'plain', 'both'))
            self.assertFalse((root / 'must-not-exist').exists())


def preflight_fixture():
    env = environment() | {'GITHUB_JOB':'setup', 'GITHUB_EVENT_NAME':'workflow_dispatch',
        'GITHUB_REPOSITORY':'polaris-arch/Polaris', 'GITHUB_REPOSITORY_ID':'1347028220',
        'GITHUB_REF':'refs/heads/hot/app-runtime-consumer-20261020', 'RUNNER_OS':'Linux', 'RUNNER_ENVIRONMENT':'github-hosted',
        'GITHUB_WORKFLOW_REF':'polaris-arch/Polaris/.github/workflows/package.yml@refs/heads/hot/app-runtime-consumer-20261020',
        'PREFLIGHT_WORKTREE_CLEAN':'true', 'PREFLIGHT_CHECKOUT_SHA':'a'*40}
    event = {'ref':'refs/heads/hot/app-runtime-consumer-20261020', 'repository':{'full_name':'polaris-arch/Polaris','id':1347028220},
             'inputs':{'linux_auth_preflight_only':'true','platform':'linux','linux_acceptance_candidate':'a'*40}}
    return env,event


class AuthPreflightTests(unittest.TestCase):
    def user(self): return types.SimpleNamespace(pw_uid=1001,pw_gid=42,pw_name='runner',pw_gecos='PRIVATE',pw_passwd='PRIVATE')

    def build(self, rows):
        env,event=preflight_fixture();user=self.user()
        with patch.object(m.os,'getuid',return_value=1001), patch.object(m.pwd,'getpwuid',return_value=user), \
             patch.object(m.pwd,'getpwall',return_value=[user]), patch('grp.getgrall',return_value=rows), \
             patch('subprocess.run',side_effect=AssertionError('no command')), patch('subprocess.Popen',side_effect=AssertionError('no child')):
            return m.build_auth_preflight(env,event)

    def test_readonly_shared_original_gate_no_authority(self):
        for name, expected in (('sudo',True),('admin',True),('adm',False),('docker',False)):
            value=self.build([types.SimpleNamespace(gr_name=name,gr_gid=42,gr_mem=[])])
            self.assertEqual(value['groupObservation']['groupRequirementSatisfied'],expected)
            self.assertFalse(value['acceptance']);self.assertFalse(value['publicationEligible'])
            self.assertEqual(value['cleanup'],'not-assessed')
            self.assertNotIn('PRIVATE',json.dumps(value))

    def test_binding_and_all_conflicts_fail_before_account_lookup(self):
        env,event=preflight_fixture()
        invalid=[('linux_auth_preflight_only','false'),('platform','all'),('linux_acceptance_candidate','b'*40),
                 ('linux_tun_auth_mode','temporary-existing-runner-password')]
        invalid += [(key,'true') for key in ('linux_package_acceptance','linux_tun_acceptance','native_validation','light_build','skip_quality_gates')]
        invalid += [(key,'PRIVATE') for key in ('linux_tun_credential_authorization','windows_system_candidate','platforms','core_candidate','core_bundle_artifact','core_wire_input_artifact','core_wire_input_artifact_id','core_wire_input_artifact_digest')]
        with patch.object(m.pwd,'getpwuid') as lookup:
            for key,value in invalid:
                with self.subTest(key=key),self.assertRaises(ValueError):
                    m.build_auth_preflight(env,event | {'inputs':event['inputs'] | {key:value}})
            lookup.assert_not_called()
        for key in ('GITHUB_EVENT_NAME','GITHUB_REPOSITORY','GITHUB_REPOSITORY_ID','GITHUB_REF','GITHUB_WORKFLOW_REF','GITHUB_SHA','GITHUB_JOB','RUNNER_OS','RUNNER_ENVIRONMENT','PREFLIGHT_CHECKOUT_SHA','PREFLIGHT_WORKTREE_CLEAN','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT'):
            with self.subTest(key=key),self.assertRaises(ValueError):m.preflight_context(env | {key:'PRIVATE'},event)

    def test_positive_attempt_not_credential_attempt_one(self):
        env,event=preflight_fixture()
        self.assertEqual(m.preflight_context(env | {'GITHUB_RUN_ATTEMPT':'2'},event)['attempt'],'2')
        for attempt in ('0','-1','1.0',''):
            with self.assertRaises(ValueError):m.preflight_context(env | {'GITHUB_RUN_ATTEMPT':attempt},event)

    def test_original_event_repo_ref_binding(self):
        env,event=preflight_fixture()
        for change in ({'ref':'refs/heads/other'},{'repository':{'full_name':'other','id':1347028220}}):
            with self.assertRaises(ValueError):m.preflight_context(env,event | change)

    def test_cli_errors_never_echo_raw_exception(self):
        with patch('sys.argv',['diagnostic_scope.py','--auth-preflight-only']), \
             patch.object(m,'run_auth_preflight',side_effect=OSError('PRIVATE NSS')):
            with self.assertRaises(SystemExit) as caught: m.main()
        self.assertEqual(caught.exception.code,'AUTH_PREFLIGHT_INVALID')

    def test_nonboolean_selection_and_gate_flags_rejected(self):
        env,event=preflight_fixture()
        for key,bad in (('linux_auth_preflight_only',1),('linux_package_acceptance',0),
                        ('linux_package_acceptance',None),('linux_tun_acceptance','FALSE')):
            with self.subTest(key=key,bad=bad),self.assertRaises(ValueError):
                m.preflight_context(env,event | {'inputs':event['inputs'] | {key:bad}})

    def test_success_diagnostic_has_no_runtime_or_credential_receipt(self):
        env,event=preflight_fixture();user=self.user()
        with tempfile.TemporaryDirectory() as temp, patch.object(m.os,'getuid',return_value=1001), \
             patch.object(m.pwd,'getpwuid',return_value=user),patch.object(m.pwd,'getpwall',return_value=[user]), \
             patch('grp.getgrall',return_value=[types.SimpleNamespace(gr_name='sudo',gr_gid=42,gr_mem=[])]):
            path=Path(temp)/'event.json';path.write_text(json.dumps(event))
            self.assertEqual(m.run_auth_preflight(env | {'GITHUB_EVENT_PATH':str(path)},temp),0)
            value=json.loads((Path(temp)/m.AUTH_OUTPUT).read_text())
            self.assertNotIn('authorization',value); self.assertNotIn('passwordStatus',value)
            self.assertEqual(value['runtime'],'not-started');self.assertEqual(value['cleanup'],'not-assessed')

    def test_failed_group_gate_json_persisted_before_nonzero(self):
        env,event=preflight_fixture(); user=self.user()
        for error in (None,OSError('PRIVATE')):
            with tempfile.TemporaryDirectory() as temp, patch.object(m.os,'getuid',return_value=1001), \
                 patch.object(m.pwd,'getpwuid',return_value=user),patch.object(m.pwd,'getpwall',return_value=[user]), \
                 patch('grp.getgrall',return_value=[],side_effect=error):
                path=Path(temp)/'event.json';path.write_text(json.dumps(event))
                self.assertEqual(m.run_auth_preflight(env | {'GITHUB_EVENT_PATH':str(path)},temp),1)
                data=(Path(temp)/m.AUTH_OUTPUT).read_text(); value=json.loads(data)
                self.assertNotIn('PRIVATE',data);self.assertFalse(value['acceptance'])
                self.assertEqual(value['groupObservation']['lookupState'],'failed' if error else 'observed')


if __name__ == '__main__':
    unittest.main()
