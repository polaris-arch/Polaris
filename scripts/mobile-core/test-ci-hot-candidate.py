#!/usr/bin/env python3
"""Pure Python fixtures/source contracts only: no tool, network or Git execution."""
import ast
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest.mock import patch
from xml.etree import ElementTree as ET
import zipfile

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location('hot_candidate', Path(__file__).with_name('ci-hot-candidate.py'))
hot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hot)
NORMAL = "(inputs.hot_consumer_candidate || '') == ''"
ACTIVE = "(inputs.hot_consumer_candidate || '') != ''"
ADDED = {'Admit exact Android consumer candidate', 'Verify candidate admission and routing contracts',
         'Verify preinstalled candidate SDK and licenses', 'Build frozen Android arm64 diagnostic consumer',
         'Retain Android candidate evidence only'}
CANDIDATE_STEPS = {'actions/checkout@v7', 'Admit exact Android consumer candidate',
    'Verify candidate admission and routing contracts', 'actions/setup-node@v7', 'Setup pnpm (pinned)',
    'Install UI dependencies', 'Install global Tauri CLI (pinned)', 'Install Rust stable + android target',
    'Install protoc (pinned URL + sha256)', 'Read pinned libbox Go version', 'actions/setup-go@v6',
    'Pin libbox JDK 17', 'Verify preinstalled candidate SDK and licenses',
    'Build frozen Android arm64 diagnostic consumer', 'Retain Android candidate evidence only'}
# Baseline executable step blocks from public App 6923dbb6, before this change.
# Removing only the exact empty-input gates/added steps must recover these bytes.
DEFAULT_DIGESTS = {'release_check': '389b87f5a18c383ff96deb197678d598cd82c2c3acde7608506138eba2c9d659', 'release-apk': 'be7be28aad17304fd2e7cb5e2464431bca7fde18c232d0f7f05bea5140e8b4b5'}


def job(source, name):
    match = re.search(r'^  ' + re.escape(name) + r':\n(.*?)(?=^  [a-zA-Z][a-zA-Z0-9_-]*:|\Z)', source, re.M | re.S)
    if not match:
        raise AssertionError('job missing: ' + name)
    return match[1]


def steps(source, name):
    block = job(source, name)
    chunks = re.findall(r'^      - (?:name|uses): .+\n(?:(?!^      - ).*\n)*', block, re.M)
    result = {}
    for chunk in chunks:
        label = chunk.splitlines()[0].split(': ', 1)[1]
        if label in result:
            raise AssertionError('duplicate step: ' + label)
        result[label] = chunk
    if not result:
        raise AssertionError('empty step inventory')
    return result


def condition(block):
    found = re.findall(r'^        if: (.+)$', block, re.M)
    if len(found) > 1:
        raise AssertionError('duplicate step if')
    return found[0] if found else 'success()'


def evaluate(expression, selected='', publish=False, success=True, outcome='success'):
    replacements = {'inputs.hot_consumer_candidate': selected, 'inputs.publish_release': publish,
                    'steps.hot_candidate.outcome': outcome, 'steps.libbox_verified.outcome': 'skipped' if selected else outcome,
                    'steps.libbox_input.outcome': 'skipped' if selected else outcome,
                    'steps.libbox.outputs.cache-hit': 'false'}
    for needle, value in replacements.items():
        expression = expression.replace(needle, repr(value))
    expression = expression.replace('success()', repr(success)).replace('always()', 'True')
    expression = re.sub(r'\btrue\b', 'True', expression).replace('&&', 'and').replace('||', 'or')
    tree = ast.parse(expression, mode='eval')
    allowed = (ast.Expression, ast.BoolOp, ast.And, ast.Or, ast.Compare, ast.Eq, ast.NotEq, ast.Constant)
    if any(not isinstance(node, allowed) for node in ast.walk(tree)):
        raise AssertionError('condition outside closed route evaluator')
    return bool(eval(compile(tree, '<workflow route>', 'eval'), {'__builtins__': {}}, {}))


def projected_defaults(source, name):
    result = []
    for label, block in steps(source, name).items():
        if label in ADDED:
            continue
        block = block.replace('        if: ' + NORMAL + '\n', '')
        block = block.replace(' && ' + NORMAL, '')
        if label == 'Read pinned libbox Go version':
            block = block.replace("        env:\n          HOT_CONSUMER_CANDIDATE: ${{ inputs.hot_consumer_candidate || '' }}\n", '')
            block = block.replace('          if [ -n "$HOT_CONSUMER_CANDIDATE" ]; then\n            echo \'version=1.27.2\' >> "$GITHUB_OUTPUT"\n            exit 0\n          fi\n', '')
        lines = [line for line in block.splitlines() if line.strip() and not line.lstrip().startswith('#')]
        result.append('\n'.join(lines))
    return hashlib.sha256('\n\n'.join(result).encode()).hexdigest()


class PureFixtures(unittest.TestCase):
    def setUp(self):
        # Any accidental external execution in these fixtures is a test failure.
        self.guards = [patch('subprocess.Popen', side_effect=AssertionError('fixture may not execute tools')),
                       patch.object(hot, 'urlopen', side_effect=AssertionError('fixture may not use network'))]
        for guard in self.guards:
            guard.start(); self.addCleanup(guard.stop)
        self.sha = 'a' * 40
        self.inputs = {'hot_consumer_candidate': self.sha, 'preserve_run': True}
        self.env = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': hot.APP_REPOSITORY,
                    'GITHUB_REPOSITORY_ID': hot.APP_REPOSITORY_ID, 'GITHUB_EVENT_NAME': 'workflow_dispatch',
                    'GITHUB_REF': hot.REF, 'GITHUB_WORKFLOW_REF': hot.WORKFLOW_REF,
                    'GITHUB_SHA': self.sha, 'GITHUB_WORKFLOW_SHA': self.sha, 'GITHUB_JOB': 'release_check'}
        # The official Octokit workflow_dispatch example uses refs/heads/master.
        # Substitute only this candidate's pinned branch/repository and inputs.
        self.event = {'ref': hot.REF, 'repository': {'full_name': hot.APP_REPOSITORY, 'id': int(hot.APP_REPOSITORY_ID)},
                      'inputs': {'hot_consumer_candidate': self.sha, 'preserve_run': 'true'}}

    def test_exact_direct_candidate(self):
        self.assertEqual(hot.validate_request(self.env, self.inputs, self.event), self.sha)

    def test_dispatch_ref_requires_exact_full_branch(self):
        for value in [hot.REF.removeprefix('refs/heads/'), 'refs/heads/main',
                      hot.REF.replace('refs/heads/', 'refs/tags/'), hot.REF + '/',
                      hot.REF + '\n', ' ' + hot.REF, self.sha, '', None, False, []]:
            with self.subTest(value=value), self.assertRaisesRegex(RuntimeError, '^direct dispatch event ref differs from pinned full branch ref$'):
                hot.validate_request(self.env, self.inputs, dict(self.event, ref=value))

    def test_dispatch_identity_errors_are_field_specific(self):
        for payload in [None, [], 'workflow_dispatch']:
            with self.subTest(payload=payload), self.assertRaisesRegex(RuntimeError, '^direct dispatch event object required$'):
                hot.validate_request(self.env, self.inputs, payload)
        for repository in [None, [], 'polaris-arch/Polaris']:
            with self.subTest(repository=repository), self.assertRaisesRegex(RuntimeError, '^direct dispatch event repository object required$'):
                hot.validate_request(self.env, self.inputs, dict(self.event, repository=repository))
        for field, values in [('full_name', [None, 'other/Polaris', 'polaris-arch/Other', 'polaris-arch/polaris']),
                              ('id', [None, 0, int(hot.APP_REPOSITORY_ID) + 1, False])]:
            for value in values:
                repository = dict(self.event['repository'], **{field: value})
                with self.subTest(field=field, value=value), self.assertRaisesRegex(RuntimeError, '^direct dispatch event repository ' + field + ' differs$'):
                    hot.validate_request(self.env, self.inputs, dict(self.event, repository=repository))

    def test_full_ref_does_not_relax_exact_event_inputs(self):
        for inputs in [None, {}, dict(self.event['inputs'], hot_consumer_candidate='b' * 40),
                       dict(self.event['inputs'], preserve_run=True),
                       dict(self.event['inputs'], preserve_run='false'),
                       dict(self.event['inputs'], publish_release='false')]:
            with self.subTest(inputs=inputs), self.assertRaisesRegex(RuntimeError, '^inherited or conflicting dispatch inputs refused$'):
                hot.validate_request(self.env, self.inputs, dict(self.event, inputs=inputs))

    def test_wrong_context_never_admits(self):
        for key in self.env:
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                hot.validate_request(dict(self.env, **{key: 'wrong'}), self.inputs, self.event)

    def test_partial_uppercase_whitespace_and_empty_sha(self):
        for value in ['', 'a' * 39, 'a' * 41, 'A' * 40, ' ' + self.sha, self.sha + '\n', True, None]:
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                hot.validate_request(self.env, dict(self.inputs, hot_consumer_candidate=value), self.event)

    def test_call_other_ref_other_repo_and_publish(self):
        for extra in [{'GITHUB_EVENT_NAME': 'workflow_call'}, {'GITHUB_WORKFLOW_REF': hot.APP_REPOSITORY + '/.github/workflows/package.yml@' + hot.REF},
                      {'GITHUB_REF': 'refs/heads/main'}, {'POLARIS_PUBLISH_RELEASE': '1'}, {'POLARIS_LIGHT_BUILD': '1'}, {'GITHUB_JOB': 'release-apk'}]:
            with self.subTest(extra=extra), self.assertRaises(RuntimeError):
                hot.validate_request(dict(self.env, **extra), self.inputs, self.event)

    def test_release_and_formal_inputs_rejected_even_false(self):
        for key in ['publish_release', 'release_tag', 'light_build', 'runtime_config_source_only', 'kernel_sha', 'signed']:
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                hot.validate_request(self.env, dict(self.inputs, **{key: False}), self.event)

    def test_unpreserved_inherited_payload_refused(self):
        for payload in [dict(self.event, inputs={}), dict(self.event, ref='main'), dict(self.event, repository={'id': 0}),
                        dict(self.event, inputs=dict(self.event['inputs'], publish_release='false'))]:
            with self.subTest(payload=payload), self.assertRaises(RuntimeError):
                hot.validate_request(self.env, self.inputs, payload)
        for value in [False, 'true', None]:
            with self.assertRaises(RuntimeError):
                hot.validate_request(self.env, dict(self.inputs, preserve_run=value), self.event)

    def test_environment_mutual_exclusion(self):
        for key in sorted(hot.OVERRIDES | {'CARGO_PROFILE_DEBUG_OPT_LEVEL', 'CGO_LDFLAGS', 'PKG_CONFIG_PATH', 'POLARIS_RUNTIME_MODE', 'TAURI_ANDROID_PROJECT_PATH', 'ORG_GRADLE_PROJECT_skipRust'}):
            with self.subTest(key=key), self.assertRaises((RuntimeError, KeyError, FileNotFoundError)):
                hot.reject_overrides({key: 'injected'})

    def test_only_exact_installed_runner_ndk_allowed(self):
        with tempfile.TemporaryDirectory() as tmp:
            sdk = Path(tmp); ndk = sdk / 'ndk/27.3.13750724'; ndk.mkdir(parents=True)
            (ndk / 'source.properties').write_text('Pkg.Revision = 27.3.13750724\n')
            env = {'ANDROID_HOME': str(sdk), 'ANDROID_NDK_HOME': str(ndk), 'ANDROID_NDK_ROOT': str(ndk)}
            hot.reject_overrides(env)
            (ndk / 'source.properties').write_text('Pkg.Revision = 28.0.0\n')
            with self.assertRaises(RuntimeError): hot.reject_overrides(env)

    def test_license_hash_keeps_original_whitespace(self):
        text = '\n  Exact package license.\n'
        xml = ET.fromstring('<repository><license id="android-sdk-license">' + text + '</license><localPackage><uses-license ref="android-sdk-license"/></localPackage></repository>')
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); (home / 'licenses').mkdir()
            file = home / 'licenses/android-sdk-license'
            exact = hashlib.sha1(text.encode()).hexdigest()
            file.write_text(exact + '\n')
            self.assertEqual(hot.accepted_licenses(xml, xml, home), {'android-sdk-license': exact})
            file.write_text(hashlib.sha1(text.strip().encode()).hexdigest() + '\n')
            with self.assertRaises(RuntimeError): hot.accepted_licenses(xml, xml, home)
            file.unlink()
            with self.assertRaises(RuntimeError): hot.accepted_licenses(xml, xml, home)

    def test_license_requires_specific_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); (home / 'licenses').mkdir(); (home / 'licenses/other').write_text('anything\n')
            for xml in ['<repository/>', '<repository><uses-license ref="missing"/></repository>']:
                with self.assertRaises(RuntimeError): hot.accepted_licenses(ET.fromstring(xml), ET.fromstring(xml), home)

    def test_duplicate_json_input_is_rejected(self):
        with self.assertRaises(RuntimeError): hot.decode(b'{"hot_consumer_candidate":"a","hot_consumer_candidate":"b"}')

    def test_clean_rejects_dirty_hidden_and_unknown_generated(self):
        def fake(*args):
            if 'status' in args: return ''
            if '-v' in args: return 'H go.mod'
            return 'build/arm64/libbox/main.go' if '--others' in args else ''
        with patch.object(hot, 'git', side_effect=fake):
            with self.assertRaises(RuntimeError): hot.clean(Path('/fixture'))
            hot.clean(Path('/fixture'), generated=True)
        for output in ['third_party/injected.go', '../escape', 'build-out/file']:
            with patch.object(hot, 'git', side_effect=lambda *a: '' if 'status' in a else ('H x' if '-v' in a else output)):
                with self.assertRaises(RuntimeError): hot.clean(Path('/fixture'), generated=True)
        with patch.object(hot, 'git', side_effect=lambda *a: '' if 'status' in a else 'h hidden.go'):
            with self.assertRaises(RuntimeError): hot.clean(Path('/fixture'))

    def test_tracked_mirror_rejects_bad_hash_and_mode(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root / 'main.go'; path.write_bytes(b'package main\n')
            raw = path.read_bytes(); blob = hashlib.sha1(b'blob ' + str(len(raw)).encode() + b'\0' + raw).hexdigest()
            row = '100644 blob ' + blob + '\tmain.go'
            with patch.object(hot, 'git', return_value=row):
                self.assertEqual(hot.tracked_mirror(root)['leaves'], 1)
                path.write_bytes(b'package other\n')
                with self.assertRaises(RuntimeError): hot.tracked_mirror(root)
                path.write_bytes(raw); path.chmod(0o755)
                with self.assertRaises(RuntimeError): hot.tracked_mirror(root)

    def native_layout(self):
        # Actual a567 arm64 carrier's main/path and Cronet record, from static
        # report 17146ed4... / SO 91183f3e...; no generated carrier executes.
        name = 'github.com/sagernet/sing-box'
        remote = 'github.com/sagernet/cronet-go/lib/android_arm64'
        version = 'v0.0.0-20260929213014-a1cafd93eb1f'
        checksum = 'h1:mmItB2/i9mrPuiqIrZm0UjcQ+m1RRhC+ayj+scOAvaw='
        facts = {'goVersion': '1.27.2', 'path': name + '/build/arm64/libbox',
                 'settings': {'GOOS': 'android', 'GOARCH': 'arm64', 'CGO_ENABLED': '1', '-buildmode': 'c-shared', '-trimpath': 'true', '-tags': ','.join(hot.TAGS)},
                 'modules': {name: {'kind': 'mod', 'version': '(devel)'}, remote: {'kind': 'dep', 'version': version, 'sum': checksum}}}
        return facts, [{'Path': name, 'Main': True, 'Dir': '/fixture'}, {'Path': remote, 'Version': version, 'Sum': checksum}]

    def test_real_main_native_layout_and_remote_checksum(self):
        facts, graph = self.native_layout(); hot.native_modules(facts, graph, Path('/fixture'))
        for mutate in [lambda f: f.update(path='command-line-arguments'),
                       lambda f: f['settings'].update(GOARCH='arm'),
                       lambda f: f['modules']['github.com/sagernet/sing-box'].update(replacement='/other'),
                       lambda f: f['modules']['github.com/sagernet/cronet-go/lib/android_arm64'].update(sum='h1:wrong')]:
            changed = copy.deepcopy(facts); mutate(changed)
            with self.assertRaises(RuntimeError): hot.native_modules(changed, graph, Path('/fixture'))

    def test_apk_requires_exact_consumed_aar_and_rust_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); rust = root / 'rust.so'; rust.write_bytes(b'rust')
            component = {'members': {'jni/arm64-v8a/libbox.so': {'bytes': 3, 'sha256': hot.sha(b'box')}}}
            apk = root / 'candidate.apk'
            def make(box=b'box', abi='arm64-v8a', dex=True):
                with zipfile.ZipFile(apk, 'w') as z:
                    z.writestr('lib/' + abi + '/libbox.so', box)
                    z.writestr('lib/' + abi + '/libpolaris_lib.so', b'rust')
                    if dex: z.writestr('classes.dex', b'dex')
            make(); self.assertIn('artifact', hot.inspect_apk(apk, component, rust))
            for kwargs in [{'box': b'old'}, {'abi': 'armeabi-v7a'}, {'dex': False}]:
                make(**kwargs)
                with self.assertRaises((RuntimeError, KeyError)): hot.inspect_apk(apk, component, rust)


class RoutingContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = (hot.ROOT / '.github/workflows/android.yml').read_text()
        cls.source = Path(hot.__file__).read_text()

    def test_default_full_step_commands_conditions_and_paths_unchanged(self):
        for name, digest in DEFAULT_DIGESTS.items():
            self.assertEqual(projected_defaults(self.workflow, name), digest, name)

    def test_candidate_exact_reachable_steps_and_default_inequivalence(self):
        inventory = steps(self.workflow, 'release_check')
        reached = {name for name, block in inventory.items() if evaluate(condition(block), 'a' * 40)}
        self.assertEqual(reached, CANDIDATE_STEPS)
        self.assertTrue({'Build unsigned release-profile validation APK', 'Verify S5 Android network observation and admission unit tests'} <=
                        {name for name, block in inventory.items() if evaluate(condition(block))})
        for name in inventory:
            if name not in CANDIDATE_STEPS:
                self.assertFalse(evaluate(condition(inventory[name]), 'a' * 40), name)

    def test_illegal_selection_reaches_red_admission_not_skip_green(self):
        expression = re.search(r'^    if: (.+)$', job(self.workflow, 'release_check'), re.M)[1]
        for selected in ['bad', ' ', 'A' * 40, 'a' * 40]:
            for publish in [False, True]:
                self.assertTrue(evaluate(expression, selected, publish))
                self.assertTrue(evaluate(condition(steps(self.workflow, 'release_check')['Admit exact Android consumer candidate']), selected))
        self.assertFalse(evaluate(expression, '', True))

    def test_admission_and_nonzero_fixture_suite_precede_any_setup(self):
        inventory = list(steps(self.workflow, 'release_check'))
        self.assertEqual(inventory[:4], ['actions/checkout@v7', 'Admit exact Android consumer candidate', 'Verify candidate admission and routing contracts', 'actions/setup-node@v7'])
        block = steps(self.workflow, 'release_check')['Verify candidate admission and routing contracts']
        self.assertIn('run: python3 scripts/mobile-core/test-ci-hot-candidate.py', block)
        self.assertNotIn('continue-on-error', block)
        self.assertNotIn('|| true', block)
        self.assertGreater(unittest.defaultTestLoader.loadTestsFromTestCase(PureFixtures).countTestCases(), 10)

    def test_only_dispatch_declares_candidate_no_event_payload_activation(self):
        call = self.workflow.split('  workflow_call:\n', 1)[1].split('  workflow_dispatch:\n', 1)[0]
        dispatch = self.workflow.split('  workflow_dispatch:\n', 1)[1].split('\npermissions:', 1)[0]
        self.assertNotIn('hot_consumer_candidate:', call)
        self.assertEqual(re.findall(r'^      ([a-z_]+):$', dispatch, re.M), ['preserve_run', 'hot_consumer_candidate'])
        self.assertNotIn('github.event.inputs.hot_consumer_candidate', self.workflow)
        self.assertNotIn('steps.hot_candidate.outputs.mode', self.workflow)

    def test_signed_job_cannot_build_candidate_or_increase_permissions(self):
        signed = steps(self.workflow, 'release-apk')
        self.assertNotIn('Build frozen Android arm64 diagnostic consumer', signed)
        self.assertEqual(condition(signed['Admit exact Android consumer candidate']), ACTIVE)
        self.assertTrue(list(signed).index('Admit exact Android consumer candidate') < list(signed).index('Verify target tag and draft release'))
        self.assertIn('\npermissions:\n  contents: read\n', self.workflow)

    def test_evidence_never_contains_app_or_component_payload(self):
        block = steps(self.workflow, 'release_check')['Retain Android candidate evidence only']
        self.assertEqual(condition(block), "always() && " + ACTIVE + " && steps.hot_candidate.outcome == 'success'")
        paths = [line.strip() for line in block.splitlines() if line.startswith('            ')]
        self.assertEqual(paths, ['${{ steps.hot_candidate.outputs.evidence }}/*' + suffix for suffix in ('.json', '.log', '.log.stdout.raw', '.log.stderr.raw')])
        self.assertFalse(any(re.search(r'\.(?:aar|apk|aab|so|jks)', path) for path in paths))
        self.assertIn('if-no-files-found: error', block)

    def test_no_product_tests_release_install_or_formal_shortcuts(self):
        self.assertEqual(hot.BUILD, ['bash', 'scripts/build-android-apk.sh', '--debug', '--apk', '--split-per-abi', '--target', 'aarch64', '--ci', '--config', 'src-tauri/tauri.android.conf.json'])
        for forbidden in ['ci_android.py prepare', 'fork_input.load(', 'inspect_aar(', 'producer.execute(', 'materialize-android', 'gh release', 'adb ', 'sudo ', 'sdkmanager', 'gomobile init']:
            executable = '\n'.join(line for line in self.source.splitlines() if not line.lstrip().startswith(('#', '"""')))
            self.assertNotIn(forbidden, executable)
        self.assertIn("'runtimeTestsRun': 0", self.source)
        self.assertIn("'full native archive provenance and reproducibility'", self.source)
        self.assertIn("'org.gradle.daemon=false\\norg.gradle.workers.max=2\\nkotlin.compiler.execution.strategy=in-process", self.source)

    def test_pins_all_nine_source_vendor_roots_and_formal_untouched(self):
        self.assertEqual(sum(row[2] for row in hot.VENDORS.values()), 2835)
        self.assertEqual(len(hot.VENDORS), 9)
        self.assertEqual(hot.sha((hot.ROOT / 'scripts/mobile-core/source-contract.json').read_bytes()), hot.FORMAL_HASH)
        self.assertIn('trackedMirror', self.source)
        self.assertIn("git(source, 'ls-tree', '-r', 'HEAD')", self.source)
        self.assertIn("mode == '160000'", self.source)
        self.assertNotIn('artifactPins', self.source)

    def test_mutated_gate_and_default_command_are_detected(self):
        changed = self.workflow.replace('        if: ' + NORMAL + '\n', '        if: false\n', 1)
        with self.assertRaises(AssertionError):
            self.assertEqual(projected_defaults(changed, 'release_check'), DEFAULT_DIGESTS['release_check'])
        changed = self.workflow.replace('run: pnpm install --frozen-lockfile', 'run: pnpm install', 1)
        self.assertNotEqual(projected_defaults(changed, 'release_check'), DEFAULT_DIGESTS['release_check'])
        changed = self.workflow.replace('        if: ' + NORMAL + '\n', '', 1)
        reached = {name for name, block in steps(changed, 'release_check').items() if evaluate(condition(block), 'a' * 40)}
        self.assertNotEqual(reached, CANDIDATE_STEPS)


if __name__ == '__main__':
    suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
    if suite.countTestCases() < 28:
        raise SystemExit('candidate fixture suite is empty/incomplete')
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    raise SystemExit(0 if result.wasSuccessful() and not result.skipped else 1)
