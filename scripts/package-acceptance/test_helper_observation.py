"""Actual Node execution of the pure callback projector (no browser/product)."""
import copy
import json
import os
import re
from pathlib import Path
import subprocess
import unittest
import helper_observation as h


def project(raw):
    script = 'const p=' + h.PROJECTOR_JS + ';process.stdout.write(JSON.stringify(p(JSON.parse(process.argv[1]))));'
    return json.loads(subprocess.check_output(['node', '-e', script, json.dumps(raw)], text=True))


def action(success=False, code='failed', **status):
    return {'success': True, 'data': {'success': success, 'errorCode': code,
        'status': {'supported': True, 'installed': False, 'ready': False, **status}}}


class HelperProjectionTests(unittest.TestCase):
    def test_whitelist_matches_actual_rust_serialized_error_enum(self):
        root = Path(os.environ.get('POLARIS_ACCEPTANCE_TEST_SOURCE_ROOT', Path(__file__).resolve().parents[2]))
        source = (root / 'src-tauri/src/runtime/helper.rs').read_text()
        body = source.split('pub enum HelperActionErrorCode {', 1)[1].split('}', 1)[0]
        names = re.findall(r'^\s*([A-Z][A-Za-z]+),\s*$', body, re.M)
        self.assertEqual(tuple(name[0].lower() + name[1:] for name in names), h.ERROR_CODES)

    def test_all_fifteen_actual_codes_remain_distinct(self):
        self.assertEqual(len(h.ERROR_CODES), 15)
        self.assertEqual(len(set(h.ERROR_CODES)), 15)
        for code in h.ERROR_CODES:
            with self.subTest(code=code):
                p = h.validate_projection(project(action(code=code)))
                self.assertEqual(p['errorCode'], code)
                self.assertFalse(p['actionSuccess'])
                self.assertTrue(p['envelopeSuccess'])
                self.assertEqual(p['association'], 'shape-only')

    def test_success_and_half_install_are_not_conflated(self):
        for success, installed, ready in [(True, True, True), (False, True, False), (False, False, False)]:
            p = h.validate_projection(project(action(success, None, installed=installed, ready=ready)))
            self.assertEqual((p['actionSuccess'], p['status']['installed'], p['status']['ready']),
                             (success, installed, ready))
        # Uninstall success legitimately reports absent/notready, unlike install.
        p = h.validate_projection(project(action(True, None)))
        self.assertTrue(p['actionSuccess'])
        self.assertFalse(p['status']['installed'])

    def test_status_refresh_preserves_repair_and_core_state(self):
        s = {'supported': True, 'installed': True, 'ready': False, 'needsRepair': True,
             'upgradeable': False, 'backgroundDisabled': False, 'pathMismatch': False,
             'selectedCore': {'state': 'invalid', 'id': 'SECRET'},
             'version': 3, 'expectedProtocolVersion': 4,
             'helperBuildId': 'PRIVATE-A', 'expectedBuildId': 'PRIVATE-B',
             'installedSingboxPath': '/secret/path'}
        p = h.validate_projection(project({'success': True, 'data': s}))
        self.assertEqual(p['kind'], 'helperStatus')
        self.assertEqual(p['errorCode'], 'not-applicable')
        self.assertTrue(p['status']['needsRepair'])
        self.assertEqual(p['selectedCoreState'], 'invalid')
        self.assertFalse(p['buildIdentityMatches'])
        self.assertNotIn('SECRET', json.dumps(p))
        self.assertNotIn('PRIVATE', json.dumps(p))
        self.assertNotIn('/secret', json.dumps(p))

    def test_outer_rejection_has_no_fabricated_helper_or_permission_identity(self):
        p = h.validate_projection(project({'success': False, 'data': None, 'error': 'password=SECRET', 'code': 'arbitrary'}))
        self.assertEqual(p['kind'], 'outerFailure')
        self.assertEqual(p['association'], 'uncorrelated')
        self.assertIsNone(p['actionSuccess'])
        self.assertNotIn('SECRET', json.dumps(p))
        # A raw rejection has no command evidence; never identify it as helper.
        self.assertIsNone(project('SECRET native rejection'))
        self.assertIsNone(project({'message': 'SECRET'}))

    def test_missing_null_unknown_codes_and_bad_booleans_remain_unknown(self):
        raw = action()
        del raw['data']['errorCode']
        self.assertEqual(project(raw)['errorCode'], 'missing')
        for code, expected in [(None, 'none'), ('secret-diagnostic', 'unknown'), ({'secret': 1}, 'unknown')]:
            self.assertEqual(project(action(code=code))['errorCode'], expected)
        raw['data']['status']['ready'] = 'true'
        p = h.validate_projection(project(raw))
        self.assertEqual(p['statusShape'], 'malformed')
        self.assertIsNone(p['status']['ready'])

    def test_no_arbitrary_fields_or_invalid_numbers_survive(self):
        p = project(action(code='cancelled', version=-1, expectedProtocolVersion=2**40,
                           selectedCore={'state': 'secret'}, helperBuildId='a', expectedBuildId='a'))
        h.validate_projection(p)
        self.assertIsNone(p['protocolVersion'])
        self.assertIsNone(p['expectedProtocolVersion'])
        self.assertEqual(p['selectedCoreState'], 'unknown')
        self.assertTrue(p['buildIdentityMatches'])
        for key, value in [('raw', 'secret'), ('errorCode', 'secret'), ('association', 'command-proven'), ('protocolVersion', True)]:
            bad = {**p, key: value}
            with self.subTest(key=key), self.assertRaises(ValueError):
                h.validate_projection(bad)
        bad = copy.deepcopy(p)
        bad['status']['password'] = 'secret'
        with self.assertRaises(ValueError): h.validate_projection(bad)
        detached = h.validate_projection(p)
        detached['status']['ready'] = True
        self.assertFalse(p['status']['ready'])

    def test_unrelated_success_and_shape_cannot_be_assigned_an_action(self):
        self.assertIsNone(project({'success': True, 'data': {'status': 'saved', 'config': {'secret': 1}}}))
        p = project(action(True, None, installed=True, ready=True))
        self.assertNotIn('command', p)
        self.assertNotIn('permission', p)
        self.assertEqual(p['association'], 'shape-only')

    def test_projection_never_mutates_native_callback_value(self):
        script = 'const p=' + h.PROJECTOR_JS + ';const raw=' + json.dumps(action()) + ';const before=JSON.stringify(raw);p(raw);if(JSON.stringify(raw)!==before)process.exit(1);'
        subprocess.run(['node', '-e', script], check=True)


if __name__ == '__main__':
    unittest.main()

class ExecutionDiagnosticProjectionTests(unittest.TestCase):
    def raw(self):
        raw=action(code='authorizationIncomplete')
        raw['data']['executionDiagnostic']={'schema':h.EXECUTION_SCHEMA,'exitCode':127,
            'marker':'noAuthenticationAgent','scope':h.EXECUTION_SCOPE}
        return raw
    def test_every_marker_survives_without_claiming_emitter(self):
        for marker in h.EXECUTION_MARKERS:
            raw=self.raw();raw['data']['executionDiagnostic']['marker']=marker
            value=h.validate_projection(project(raw))
            self.assertEqual(value['executionDiagnosticState'],'valid')
            self.assertEqual(value['executionDiagnostic']['marker'],marker)
            self.assertEqual(value['executionDiagnostic']['scope'],'child-stderr-marker-not-verified-emitter')
            detached=h.validate_projection(value);detached['executionDiagnostic']['marker']='unknown'
            self.assertEqual(value['executionDiagnostic']['marker'],marker)
    def test_malformed_and_secret_input_never_survive(self):
        for mutate in (lambda e:e.update(raw='PRIVATE'),lambda e:e.update(marker='PRIVATE'),
                       lambda e:e.update(exitCode=True),lambda e:e.update(exitCode=126),
                       lambda e:e.update(scope='verified-authority'),lambda e:e.update(schema='wrong')):
            raw=self.raw();mutate(raw['data']['executionDiagnostic'])
            value=h.validate_projection(project(raw))
            self.assertEqual(value['executionDiagnosticState'],'malformed');self.assertIsNone(value['executionDiagnostic'])
            self.assertNotIn('PRIVATE',json.dumps(value))
        raw=self.raw();raw['data']['success']=True
        self.assertEqual(project(raw)['executionDiagnosticState'],'malformed')
    def test_legacy_absence_and_strict_injected_observation(self):
        value=project(action());self.assertEqual(value['executionDiagnosticState'],'absent')
        value['executionDiagnostic']={'raw':'PRIVATE'}
        with self.assertRaises(ValueError):h.validate_projection(value)
    def test_markers_match_rust_enum_and_existing_failure_boundaries(self):
        root=Path(os.environ.get('POLARIS_ACCEPTANCE_TEST_SOURCE_ROOT',Path(__file__).resolve().parents[2]))
        source=(root/'src-tauri/src/runtime/helper.rs').read_text()
        body=source.split('pub enum HelperExecutionMarker {',1)[1].split('}',1)[0]
        names=re.findall(r'^\s*([A-Z][A-Za-z]+),\s*$',body,re.M)
        self.assertEqual(tuple(n[0].lower()+n[1:] for n in names),h.EXECUTION_MARKERS)
        self.assertEqual(source.count('result.execution_diagnostic ='),2)
        command=(root/'src-tauri/src/commands/helper.rs').read_text()
        self.assertEqual(command.count('execution_diagnostic: None'),2)

class InstallerStageProjectionTests(unittest.TestCase):
    def raw(self):
        value=action(code='installerFailed')
        value['data']['executionDiagnostic']={'schema':h.EXECUTION_SCHEMA,'scope':h.EXECUTION_SCOPE,
            'exitCode':1,'marker':'unknown','installerFailure':{'schema':'polaris-helper-installer-failure-v1',
            'stage':'manager-property','recovery':'recovery-unconfirmed'}}
        return value
    def test_all_stages_fixed_and_detached(self):
        for stage in h.INSTALLER_STAGES:
            raw=self.raw();raw['data']['executionDiagnostic']['installerFailure']['stage']=stage
            value=h.validate_projection(project(raw));copy_=h.validate_projection(value)
            value['executionDiagnostic']['installerFailure']['stage']='SECRET'
            self.assertEqual(copy_['executionDiagnostic']['installerFailure']['stage'],stage)
    def test_malformed_unknown_and_success_do_not_leak(self):
        for change in (lambda e:e.update(exitCode=0),lambda e:e.update(installerFailure=None),
                       lambda e:e['installerFailure'].update(stage='SECRET'),lambda e:e['installerFailure'].update(secret='SECRET')):
            raw=self.raw();change(raw['data']['executionDiagnostic'])
            value=h.validate_projection(project(raw))
            self.assertEqual(value['executionDiagnosticState'],'malformed')
            self.assertNotIn('SECRET',json.dumps(value))
        value=h.validate_projection(project({'success':False,'data':None,'error':'SECRET'}))
        self.assertEqual(value['kind'],'outerFailure');self.assertNotIn('SECRET',json.dumps(value))
