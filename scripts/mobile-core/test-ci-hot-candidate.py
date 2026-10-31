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
# Exact default executable steps after the reviewed runtime-protocol unit class addition.
# Candidate routing projection still binds every remaining command/condition/path.
DEFAULT_DIGESTS = {'release_check': '509ff38ac8d5eedcca84491b556e4c7af7539da9e8bed7636ab68a0ee790b139', 'release-apk': 'e507797072b2960a340a35bbbe87c8d7293f95c2db92c7634ceb7a817d6b2083'}


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


# PKG-01 intentionally replaces only the unconsumed normal-lane diagnostic.
# Restore its exact historical bytes for the existing full-lane digest, after
# separately requiring the new verified source-contract consumer and gate.
NORMAL_VERSION_COMMANDS = '''          v="$(python3 -c "import sys; sys.path.insert(0, 'scripts/mobile-core'); from fork_input import load; print(load()['version'])")"
          [ -n "$v" ] || { echo "::error::verified mobile source-contract version missing"; exit 1; }
          echo "已验证移动 source-contract 选中版本：v$v"
'''
LEGACY_VERSION_COMMANDS = '''          v="$(node -p "require('./src-tauri/core-manifest.json').bundledCoreVersion")"
          [ -n "$v" ] || { echo "::error::core-manifest.json 里读不到 bundledCoreVersion"; exit 1; }
          echo "内核版本真值：v$v"
'''


def restore_normal_version_diagnostic(block):
    executable = [line for line in block.splitlines() if line.strip() and not line.lstrip().startswith('#')]
    expected = ['      - name: Resolve bundled core version', '        if: ' + NORMAL,
                '        id: core', '        shell: bash', '        run: |', '          set -euo pipefail',
                *NORMAL_VERSION_COMMANDS.rstrip('\n').splitlines(), '          echo "version=$v" >> "$GITHUB_OUTPUT"']
    if executable != expected or block.count(NORMAL_VERSION_COMMANDS) != 1:
        raise AssertionError('normal diagnostic must load the validated mobile source-contract with its existing guard/output')
    return block.replace(NORMAL_VERSION_COMMANDS, LEGACY_VERSION_COMMANDS)


def projected_defaults(source, name):
    result = []
    for label, block in steps(source, name).items():
        if label in ADDED:
            continue
        if label == 'Resolve bundled core version':
            block = restore_normal_version_diagnostic(block)
        block = block.replace('        if: ' + NORMAL + '\n', '')
        block = block.replace(' && ' + NORMAL, '')
        if label == 'Read pinned libbox Go version':
            block = block.replace("        env:\n          HOT_CONSUMER_CANDIDATE: ${{ inputs.hot_consumer_candidate || '' }}\n", '')
            block = block.replace('          if [ -n "$HOT_CONSUMER_CANDIDATE" ]; then\n            echo \'version=1.27.2\' >> "$GITHUB_OUTPUT"\n            exit 0\n          fi\n', '')
        lines = [line for line in block.splitlines() if line.strip() and not line.lstrip().startswith('#')]
        result.append('\n'.join(lines))
    return hashlib.sha256('\n\n'.join(result).encode()).hexdigest()


class PureFixtures(unittest.TestCase):
    def test_explicit_source_identity_keeps_android_defaults(self):
        before = hot.source_identity()
        self.assertEqual(before['commit'], hot.KERNEL_COMMIT)
        self.assertEqual(before['vendors'], hot.VENDORS)
        desktop = copy.deepcopy(before)
        desktop['commit'] = 'a' * 40
        desktop['tree'] = 'b' * 40
        self.assertEqual(hot.source_identity(desktop), desktop)
        self.assertEqual(hot.source_identity(), before)

    def test_alpha124_source_cannot_satisfy_current_frozen_android_identity(self):
        old_commit = '0cfa24573ff34c26608151068334329268cfdf14'
        old_tree = '412b1640c777831b1e8811b4c596cfb6df27fdb0'
        for commit, tree in ((old_commit, old_tree), (old_commit, hot.KERNEL_TREE),
                             (hot.KERNEL_COMMIT, old_tree)):
            with self.subTest(commit=commit, tree=tree):
                def git(_source, *args):
                    return tree if args == ('rev-parse', 'HEAD^{tree}') else commit
                with patch.object(hot, 'git', side_effect=git), patch.object(hot, 'clean') as clean, \
                        self.assertRaisesRegex(RuntimeError, '^kernel commit/tree differs$'):
                    hot.source_facts(Path('inert-unopened-source'))
                clean.assert_not_called()
        self.assertEqual(hot.source_identity()['commit'], hot.KERNEL_COMMIT)

    def test_explicit_source_identity_rejects_wrong_repository_proto_and_vendor(self):
        value = copy.deepcopy(hot.source_identity())
        for key, changed in [('repository', 'sagernet/sing-box'), ('repositoryId', 1),
                             ('commit', 'main'), ('tree', None)]:
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                hot.source_identity(dict(value, **{key: changed}))
        for changed in [{}, {'go.mod': 'a' * 64}, dict(value['files'], **{'../escape': 'a' * 64}),
                        dict(value['files'], **{'daemon/runtime_config.proto': 'a' * 64})]:
            with self.assertRaises(RuntimeError):
                hot.source_identity(dict(value, files=changed))
        for changed in [{}, dict(value['vendors'], **{'foreign/module': ['other', 'a' * 40, 1]})]:
            with self.assertRaises(RuntimeError):
                hot.source_identity(dict(value, vendors=changed))

    def test_frozen_source_caller_uses_shared_verifier_and_retains_all_guards(self):
        from source_policy import candidate_vendors
        from test_source_policy import ten_vendor_fixture
        _, files, _, _ = ten_vendor_fixture()
        selected = copy.deepcopy(hot.source_identity())
        expected = {name: {'path': './third_party/' + row[0], 'tree': row[1], 'files': row[2]}
                    for name, row in hot.VENDORS.items()}
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp).resolve(); (source / 'go.mod').write_bytes(files['go.mod'])
            def git(_, *args):
                return hot.KERNEL_TREE if args == ('rev-parse', 'HEAD^{tree}') else hot.KERNEL_COMMIT
            def pin(path): return {'sha256': hot.SOURCE_HASHES[str(path.relative_to(source))]}
            with patch.object(hot, 'git', side_effect=git) as read_git, \
                    patch.object(hot, 'clean') as clean, patch.object(hot, 'file_pin', side_effect=pin) as read_pin, \
                    patch.object(hot, 'verify_vendor_sources', return_value=(expected, {}, {}, {})) as verify, \
                    patch.object(hot, 'tracked_mirror', return_value={'leaves': 1}) as mirror:
                result = hot.source_facts(source, generated=True)
                self.assertEqual(read_git.call_args_list, [unittest.mock.call(source, 'rev-parse', 'HEAD'),
                                                          unittest.mock.call(source, 'rev-parse', 'HEAD^{tree}')])
                clean.assert_called_once_with(source, generated=True)
                self.assertEqual(read_pin.call_count, len(hot.SOURCE_HASHES))
                verify.assert_called_once_with(source, candidate_vendors(hot.VENDORS, schema='polaris-reviewed-kernel-source-v3'), hot.git)
                mirror.assert_called_once_with(source)
                self.assertEqual(result['vendors'], expected)
                self.assertEqual(result['files'], hot.SOURCE_HASHES)
                self.assertEqual(result['trackedMirror'], {'leaves': 1})
        self.assertEqual(hot.source_identity(), selected)

    def test_candidate_graph_uses_shared_exact_ten_vendor_paths_and_no_local_h1(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp).resolve(); graph = []
            for name, (directory, _, _) in hot.VENDORS.items():
                path = source / 'third_party' / directory; path.mkdir(parents=True)
                graph.append({'Path': name, 'Version': 'v0.0.0-fixture',
                              'Replace': {'Path': './third_party/' + directory, 'Dir': str(path)}})
            self.assertIs(hot.graph_facts(graph, source), graph)
            for case in ('missing', 'extra', 'duplicate', 'sum', 'replacement-sum', 'path', 'directory', 'remote'):
                changed = copy.deepcopy(graph)
                if case == 'missing': changed.pop()
                elif case == 'extra': changed.append({'Path': 'example.invalid/extra', 'Replace': {'Path': './extra', 'Dir': str(source / 'extra')}})
                elif case == 'duplicate': changed.append(copy.deepcopy(changed[0]))
                elif case == 'sum': changed[0]['Sum'] = 'h1:invented'
                elif case == 'replacement-sum': changed[0]['Replace']['Sum'] = 'h1:invented'
                elif case == 'path': changed[0]['Replace']['Path'] = '../outside'
                elif case == 'directory': changed[0]['Replace']['Dir'] = str(source)
                else: changed[0]['Replace']['Version'] = 'v1.0.0'
                with self.subTest(case=case), self.assertRaises(RuntimeError): hot.graph_facts(changed, source)

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

    def test_final_kernel_identity_and_runtime_api_source_are_frozen(self):
        self.assertEqual(hot.KERNEL_COMMIT, 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e')
        self.assertEqual(hot.KERNEL_TREE, '4c0fdb31d59967f41c808ea6b63f87ca9661c2cf')
        self.assertEqual(hot.SOURCE_HASHES['experimental/libbox/command_runtime.go'],
                         '3041a85bbda753e961b2b9b19ce1881b5afe000e3bcd6352d786679118055dc9')
        self.assertEqual(hot.SOURCE_HASHES['daemon/runtime_config.proto'], hot.PROTO)

    def test_alpha124_formal_hash_cannot_authorize_current_candidate(self):
        with tempfile.TemporaryDirectory() as tmp:
            event_path = Path(tmp) / 'event.json'
            event_path.write_text(json.dumps(self.event))
            env = dict(self.env, GITHUB_EVENT_PATH=str(event_path),
                       HOT_CONSUMER_INPUTS=json.dumps(self.inputs))
            with patch.dict(hot.os.environ, env, clear=True), \
                    patch.object(hot, 'git', return_value=self.sha), \
                    patch.object(hot, 'clean'), patch.object(hot, 'slot') as slot, \
                    patch.object(hot, 'FORMAL_HASH',
                        '3740b0fe0739ae8f811b2a2a3665cfb0d0dc0d7882ffaec796956fed7714e5b6'), \
                    self.assertRaisesRegex(RuntimeError, '^formal pin changed$'):
                hot.admit()
            slot.assert_not_called()

    def test_alpha125_formal_hash_cannot_authorize_current_candidate(self):
        with tempfile.TemporaryDirectory() as tmp:
            event_path = Path(tmp) / 'event.json'
            event_path.write_text(json.dumps(self.event))
            env = dict(self.env, GITHUB_EVENT_PATH=str(event_path),
                       HOT_CONSUMER_INPUTS=json.dumps(self.inputs))
            with patch.dict(hot.os.environ, env, clear=True), \
                    patch.object(hot, 'git', return_value=self.sha), \
                    patch.object(hot, 'clean'), patch.object(hot, 'slot') as slot, \
                    patch.object(hot, 'FORMAL_HASH',
                        '2933510a43097440b336a4d146eca790a3d1a744a3f75e8acad8d8c5a386ce5a'), \
                    self.assertRaisesRegex(RuntimeError, '^formal pin changed$'):
                hot.admit()
            slot.assert_not_called()

    def unit_fixture(self, root):
        app, evidence = root / 'app', root / 'evidence'
        evidence.mkdir()
        for name in hot.UNIT_TEST_CLASSES:
            source = app / 'src/test/java/com/polaris2/app/vpn' / (name + '.kt')
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text('@Test fun firstCase() {}\n@Test(expected = Exception::class) fun secondCase() {}\n')
            report = app / 'build/test-results/testArm64DebugUnitTest' / ('TEST-com.polaris2.app.vpn.' + name + '.xml')
            report.parent.mkdir(parents=True, exist_ok=True)
            classname = 'com.polaris2.app.vpn.' + name
            report.write_text('<testsuite name="' + classname + '" tests="2" failures="0" errors="0" skipped="0">' +
                              ''.join('<testcase name="' + method + '" classname="' + classname + '"/>'
                                      for method in ('firstCase', 'secondCase')) + '</testsuite>')
        return app, evidence

    def test_real_unit_receipt_requires_complete_nonzero_method_results(self):
        with tempfile.TemporaryDirectory() as tmp:
            app, evidence = self.unit_fixture(Path(tmp))
            reports = hot.unit_test_reports(app, evidence)
            self.assertEqual(set(reports), set(hot.UNIT_TEST_CLASSES))
            receipt = hot.inspect_unit_results(app, evidence, self.sha, {'sha256': 'aar'}, {'sha256': 'rust'})
            self.assertEqual(receipt['variant'], 'arm64Debug')
            self.assertEqual(receipt['candidate'], self.sha)
            self.assertEqual(receipt['kernelCommit'], hot.KERNEL_COMMIT)
            self.assertEqual(sum(row['tests'] for row in receipt['classes']), 4)
            self.assertTrue(all(row['methods'] == ['firstCase', 'secondCase'] for row in receipt['classes']))
            for row in receipt['classes']:
                name = row['class'].rsplit('.', 1)[1]
                self.assertEqual(row['reportSha256'], reports[name]['sha256'])
            self.assertEqual(receipt['component'], {'sha256': 'aar'})
            self.assertEqual(receipt['rustApp'], {'sha256': 'rust'})

    def test_unit_receipt_rejects_zero_missing_duplicate_skipped_failed_and_foreign_methods(self):
        mutations = [lambda x: x.replace('tests="2"', 'tests="0"'),
                     lambda x: x.replace('tests="2"', 'tests="3"'),
                     lambda x: x.replace('name="secondCase"', 'name="firstCase"'),
                     lambda x: x.replace('name="secondCase"', 'name="foreignCase"'),
                     lambda x: x.replace('failures="0"', 'failures="1"'),
                     lambda x: x.replace('errors="0"', 'errors="1"'),
                     lambda x: x.replace('skipped="0"', 'skipped="1"'),
                     lambda x: x.replace('/>', '><skipped/></testcase>', 1),
                     lambda x: x.replace('/>', '><failure/></testcase>', 1),
                     lambda x: x.replace('/>', '><error/></testcase>', 1),
                     lambda x: x.replace('classname="com.polaris2', 'classname="foreign'),
                     lambda x: x.replace('name="com.polaris2', 'name="foreign'),
                     lambda x: x.replace('testsuite', 'not-testsuite')]
        for change in mutations:
            with self.subTest(change=change), tempfile.TemporaryDirectory() as tmp:
                app, evidence = self.unit_fixture(Path(tmp))
                report = next((app / 'build/test-results/testArm64DebugUnitTest').glob('*.xml'))
                report.write_text(change(report.read_text()))
                hot.unit_test_reports(app, evidence)
                with self.assertRaises(RuntimeError):
                    hot.inspect_unit_results(app, evidence, self.sha, {}, {})
        with tempfile.TemporaryDirectory() as tmp:
            app, evidence = self.unit_fixture(Path(tmp))
            report = next((app / 'build/test-results/testArm64DebugUnitTest').glob('*.xml')); report.unlink()
            self.assertEqual(len(hot.unit_test_reports(app, evidence)), 1)
            with self.assertRaises(RuntimeError): hot.inspect_unit_results(app, evidence, self.sha, {}, {})

    def test_unit_receipt_rejects_stale_retained_bytes_and_empty_source_inventory(self):
        with tempfile.TemporaryDirectory() as tmp:
            app, evidence = self.unit_fixture(Path(tmp)); hot.unit_test_reports(app, evidence)
            report = next((app / 'build/test-results/testArm64DebugUnitTest').glob('*.xml'))
            original = report.read_bytes(); report.write_bytes(original + b'\n')
            with self.assertRaisesRegex(RuntimeError, 'original/retained'):
                hot.inspect_unit_results(app, evidence, self.sha, {}, {})
            report.write_bytes(original)
            source = next((app / 'src/test/java/com/polaris2/app/vpn').glob('*.kt')); source.write_text('class Empty {}')
            with self.assertRaisesRegex(RuntimeError, 'empty/duplicate'):
                hot.inspect_unit_results(app, evidence, self.sha, {}, {})

    def test_unit_report_retention_rejects_symlink_and_replacement(self):
        with tempfile.TemporaryDirectory() as tmp:
            app, evidence = self.unit_fixture(Path(tmp))
            report = next((app / 'build/test-results/testArm64DebugUnitTest').glob('*.xml'))
            report = app / 'build/test-results/testArm64DebugUnitTest' / ('TEST-com.polaris2.app.vpn.' + hot.UNIT_TEST_CLASSES[0] + '.xml')
            raw = report.read_bytes(); target = Path(tmp) / 'redirected'; target.write_bytes(raw)
            report.unlink(); report.symlink_to(target)
            with self.assertRaises(RuntimeError): hot.unit_test_reports(app, evidence)
            report.unlink(); report.write_bytes(raw)
            hot.unit_test_reports(app, evidence)
            with self.assertRaises(FileExistsError): hot.unit_test_reports(app, evidence)

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

    def test_setup_go_local_is_allowed_only_after_preparation(self):
        for prepared in (False, True):
            for value in ['local', 'auto', 'path', 'go1.27.2', 'go1.28.0+auto', 'go1.27.2+path',
                          'LOCAL', ' local', 'local ', 'local\n', '\t']:
                with self.subTest(prepared=prepared, value=value):
                    if prepared and value == 'local':
                        hot.reject_overrides({'GOTOOLCHAIN': value}, prepared=prepared)
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'ambient candidate override refused: GOTOOLCHAIN'):
                            hot.reject_overrides({'GOTOOLCHAIN': value}, prepared=prepared)
        # An explicitly injected local value must still fail the actual admit path.
        with self.assertRaises(RuntimeError):
            hot.validate_request(dict(self.env, GOTOOLCHAIN='local'), self.inputs, self.event)

    def test_prepared_local_does_not_exempt_any_other_override(self):
        keys = hot.OVERRIDES | {'CARGO_PROFILE_DEBUG_OPT_LEVEL', 'CGO_LDFLAGS', 'PKG_CONFIG_PATH',
                               'POLARIS_RUNTIME_MODE', 'TAURI_ANDROID_PROJECT_PATH', 'ORG_GRADLE_PROJECT_skipRust'}
        for key in sorted(keys):
            with self.subTest(key=key), self.assertRaises((RuntimeError, KeyError, FileNotFoundError)):
                hot.reject_overrides(dict(GOTOOLCHAIN='local', **{key: 'injected'}) if key != 'GOTOOLCHAIN' else {key: 'injected'}, prepared=True)

    def test_full_observed_prepared_environment_and_ndk_guards(self):
        # Complete visible build-step env from run 39557631480, plus the already
        # audited hosted-runner SDK/NDK baseline. Only that path becomes temporary.
        observed = {'PYTHONDONTWRITEBYTECODE': '1', 'POLARIS_LIGHT_BUILD': '0', 'POLARIS_PUBLISH_RELEASE': '0',
                    'CARGO_INCREMENTAL': '0', 'CARGO_TERM_COLOR': 'always', 'PROTOC_EXPECT': 'libprotoc 35.1',
                    'GOTOOLCHAIN': 'local', 'JAVA_HOME': '/usr/lib/jvm/temurin-21-jdk-amd64',
                    'JAVA_HOME_17_X64': '/opt/hostedtoolcache/Java_Temurin-Hotspot_jdk/17.0.20-101/x64',
                    'MAVEN_ARGS': '-ntp', 'JDK17': '/opt/hostedtoolcache/Java_Temurin-Hotspot_jdk/17.0.20-101/x64'}
        with tempfile.TemporaryDirectory() as tmp:
            sdk = Path(tmp); ndk = sdk / 'ndk/27.3.13750724'; ndk.mkdir(parents=True)
            (ndk / 'source.properties').write_text('Pkg.Revision = 27.3.13750724\n')
            env = dict(self.env, **observed, ANDROID_HOME=str(sdk), ANDROID_SDK_ROOT=str(sdk),
                       ANDROID_NDK_HOME=str(ndk), ANDROID_NDK_ROOT=str(ndk), NDK_RUST=str(ndk))
            hot.reject_overrides(env, prepared=True)
            with self.assertRaises(RuntimeError): hot.reject_overrides(env)
            wrong = sdk / 'ndk/28.0.0'; wrong.mkdir(); (wrong / 'source.properties').write_text('Pkg.Revision = 28.0.0\n')
            for key in ['ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT', 'NDK_RUST']:
                with self.subTest(key=key), self.assertRaises(RuntimeError):
                    hot.reject_overrides(dict(env, **{key: str(wrong)}), prepared=True)
            (ndk / 'source.properties').write_text('Pkg.Revision = 28.0.0\n')
            with self.assertRaises(RuntimeError): hot.reject_overrides(env, prepared=True)

    def test_selected_rust_ndk_is_allowed_only_after_sdk_preparation(self):
        with tempfile.TemporaryDirectory() as tmp:
            sdk = Path(tmp); ndk = sdk / 'ndk/27.3.13750724'; ndk.mkdir(parents=True)
            (ndk / 'source.properties').write_text('Pkg.Revision = 27.3.13750724\n')
            env = {'ANDROID_HOME': str(sdk), 'NDK_RUST': str(ndk)}
            hot.reject_overrides(env, prepared=True)
            with self.assertRaisesRegex(RuntimeError, 'ambient candidate override refused: NDK_RUST'):
                hot.reject_overrides(env)
            for value in ['28.0.0', '27.3.13750724-beta1', '']:
                (ndk / 'source.properties').write_text('Pkg.Revision = ' + value + '\n')
                with self.subTest(value=value), self.assertRaises(RuntimeError): hot.reject_overrides(env, prepared=True)

    def test_only_exact_installed_runner_ndk_allowed(self):
        with tempfile.TemporaryDirectory() as tmp:
            sdk = Path(tmp); ndk = sdk / 'ndk/27.3.13750724'; ndk.mkdir(parents=True)
            (ndk / 'source.properties').write_text('Pkg.Revision = 27.3.13750724\n')
            env = {'ANDROID_HOME': str(sdk), 'ANDROID_NDK_HOME': str(ndk), 'ANDROID_NDK_ROOT': str(ndk)}
            hot.reject_overrides(env)
            (ndk / 'source.properties').write_text('Pkg.Revision = 28.0.0\n')
            with self.assertRaises(RuntimeError): hot.reject_overrides(env)

    def test_license_hash_uses_sdk_xml_normalization(self):
        text = '\n  Exact  package\nlicense.\n'
        xml = ET.fromstring('<repository><license id="android-sdk-license">' + text + '</license><localPackage><uses-license ref="android-sdk-license"/></localPackage></repository>')
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); (home / 'licenses').mkdir()
            file = home / 'licenses/android-sdk-license'
            exact = hashlib.sha1(b'Exact package license.').hexdigest()
            file.write_text(exact + '\n')
            self.assertEqual(hot.accepted_licenses(xml, xml, home), {'android-sdk-license': exact})
            for incorrect in [text, text.strip(), 'Different package license.']:
                file.write_text(hashlib.sha1(incorrect.encode()).hexdigest() + '\n')
                with self.assertRaises(RuntimeError): hot.accepted_licenses(xml, xml, home)
            file.unlink()
            with self.assertRaises(RuntimeError): hot.accepted_licenses(xml, xml, home)

    def test_sdk_normalization_matches_each_adapter_step(self):
        for raw, expected in [(' A \t  B\nC\n\n D ', 'A B C\n\nD'),
                              ('A\tB', 'A\tB'), ('A\t \tB', 'A\tB'),
                              ('A\rB', 'A\rB'), ('A\n \t\nB', 'A\n\nB'),
                              ('\x00\x1f A \x20', 'A'), ('', '')]:
            with self.subTest(raw=raw):
                self.assertEqual(hot.sdk_license_value(raw), expected)
        # XML parsing normalizes CRLF before the SDK's adapter is applied.
        xml = ET.fromstring('<license> A\r\nB\r\n\r\n C </license>')
        self.assertEqual(hot.sdk_license_value(''.join(xml.itertext())), 'A B\n\nC')

    def test_sdk_normalization_does_not_use_unicode_whitespace(self):
        for space in ['\u00a0', '\u0085', '\u2003']:
            with self.subTest(space=space):
                self.assertEqual(hot.sdk_license_value(' ' + space + 'Text' + space + ' '), space + 'Text' + space)
                self.assertEqual(hot.sdk_license_value('A' + space + ' \tB'), 'A' + space + ' B')

    def test_license_acceptance_requires_exact_sdk_lines(self):
        xml = ET.fromstring('<repository><license id="android-sdk-license">Exact license.</license><localPackage><uses-license ref="android-sdk-license"/></localPackage></repository>')
        digest = hashlib.sha1(b'Exact license.').hexdigest()
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); (home / 'licenses').mkdir(); file = home / 'licenses/android-sdk-license'
            for separator in ['\n', '\r\n', '\r']:
                file.write_bytes(('older-hash' + separator + digest + separator).encode())
                self.assertEqual(hot.accepted_licenses(xml, xml, home), {'android-sdk-license': digest})
            for separator in ['\x0b', '\x0c', '\u0085', '\u2028', '\u2029']:
                file.write_bytes(('older-hash' + separator + digest + '\n').encode())
                with self.subTest(separator=separator), self.assertRaises(RuntimeError):
                    hot.accepted_licenses(xml, xml, home)
            for value in [' ' + digest, digest + ' ', digest.upper()]:
                file.write_text(value + '\n')
                with self.subTest(value=value), self.assertRaises(RuntimeError):
                    hot.accepted_licenses(xml, xml, home)

    def test_license_requires_specific_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); (home / 'licenses').mkdir(); (home / 'licenses/other').write_text('anything\n')
            for xml in ['<repository/>', '<repository><uses-license ref="missing"/></repository>']:
                with self.assertRaises(RuntimeError): hot.accepted_licenses(ET.fromstring(xml), ET.fromstring(xml), home)

    def stable_ndk_properties(self):
        # Exact 107-byte member from the official stable r30 Linux archive.
        return b'Pkg.Desc = Android NDK\nPkg.Revision = 30.0.16248370\nPkg.BaseRevision = 30.0.16248370\nPkg.ReleaseName = r30\n'

    def test_stable_ndk_profile_and_actual_properties(self):
        from android_sdk import validate_ndk_properties
        self.assertEqual(hot.ANDROID['ndkRevision'], '30.0.16248370')
        self.assertEqual(hot.ANDROID['ndkSourceRevision'], '30.0.16248370')
        self.assertEqual(hot.ANDROID['ndkPackage'], 'android-ndk-r30')
        self.assertEqual(hot.ANDROID['licenseId'], 'android-sdk-license')
        self.assertEqual(hot.ndk_archive_contract(), {
            'url': 'https://dl.google.com/android/repository/android-ndk-r30-linux.zip',
            'bytes': 738633529, 'sha1': '5107f898313790e449e87eee2183d9a20602dee9',
            'sha256': '753611f410d002cfcd3f3dc2ef49aad532089d3180b436c060a90bf0fcb64df2',
            'archiveRoot': 'android-ndk-r30'})
        raw = self.stable_ndk_properties()
        self.assertEqual(hot.sha(raw), hot.ANDROID['sourcePropertiesSha256'])
        self.assertEqual(validate_ndk_properties(raw.decode(), hot.ANDROID)['Pkg.ReleaseName'], 'r30')
        for key, value in [('Pkg.Revision', '30.0.16138531-beta3'), ('Pkg.BaseRevision', '30.0.16138531'), ('Pkg.ReleaseName', 'r30-beta3')]:
            changed = re.sub(r'^' + re.escape(key) + r' = .+$', key + ' = ' + value, raw.decode(), flags=re.M)
            with self.subTest(key=key), self.assertRaises(RuntimeError): validate_ndk_properties(changed, hot.ANDROID)

    def stable_ndk_metadata(self):
        archive = hot.ndk_archive_contract()
        return ('<repository><license id="android-sdk-license">Exact license.</license>'
                '<remotePackage path="ndk;30.0.16248370"><revision><major>30</major><minor>0</minor><micro>16248370</micro></revision>'
                '<uses-license ref="android-sdk-license"/><channelRef ref="channel-0"/><archives><archive><complete>'
                '<size>' + str(archive['bytes']) + '</size><checksum type="sha1">' + archive['sha1'] + '</checksum>'
                '<url>android-ndk-r30-linux.zip</url></complete><host-os>linux</host-os></archive></archives></remotePackage></repository>').encode()

    def test_stable_ndk_metadata_rejects_preview_and_other_identity(self):
        raw = self.stable_ndk_metadata()
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); (home / 'licenses').mkdir(); accepted = home / 'licenses/android-sdk-license'
            accepted.write_text(hashlib.sha1(b'Exact license.').hexdigest() + '\n')
            facts = hot.ndk_metadata(raw, home)
            self.assertEqual(facts['profile'], hot.ANDROID)
            self.assertEqual(facts['archive'], hot.ndk_archive_contract())
            self.assertEqual(set(facts['licenses']), {'android-sdk-license'})
            changes = [(b'ndk;30.0.16248370', b'ndk;30.0.16138531'),
                       (b'<micro>16248370</micro>', b'<micro>16138531</micro>'),
                       (b'</revision>', b'<preview>3</preview></revision>'), (b'channel-0', b'channel-1'),
                       (b'<uses-license ref="android-sdk-license"/>', b'<uses-license ref="android-sdk-preview-license"/>'),
                       (b'android-ndk-r30-linux.zip', b'android-ndk-r30-beta3-linux.zip'),
                       (b'<host-os>linux', b'<host-os>macosx'), (b'type="sha1"', b'type="sha256"'),
                       (b'738633529', b'738633528'), (hot.ndk_archive_contract()['sha1'].encode(), b'0' * 40)]
            for old, new in changes:
                with self.subTest(old=old), self.assertRaises(RuntimeError): hot.ndk_metadata(raw.replace(old, new), home)
            accepted.write_text('0' * 40 + '\n')
            with self.assertRaises(RuntimeError): hot.ndk_metadata(raw, home)
            accepted.unlink()
            with self.assertRaises(RuntimeError): hot.ndk_metadata(raw, home)

    def ndk_zip_fixture(self, names=None):
        payload = io.BytesIO()
        with zipfile.ZipFile(payload, 'w') as bundle:
            for name in names or ['android-ndk-r30/source.properties']:
                bundle.writestr(name, self.stable_ndk_properties())
        raw = payload.getvalue()
        archive = dict(hot.ndk_archive_contract(), bytes=len(raw), sha1=hashlib.sha1(raw).hexdigest(), sha256=hot.sha(raw))
        return raw, archive

    def test_candidate_ndk_archive_requires_both_hashes_and_safe_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            download = Path(tmp) / 'archive.zip'; raw, archive = self.ndk_zip_fixture(); download.write_bytes(raw)
            with patch.object(hot, 'ndk_archive_contract', return_value=archive):
                self.assertEqual(hot.verify_ndk_archive(download)['sha256'], archive['sha256'])
            for key, value in [('bytes', len(raw) + 1), ('sha1', '0' * 40), ('sha256', '0' * 64)]:
                with self.subTest(key=key), patch.object(hot, 'ndk_archive_contract', return_value=dict(archive, **{key: value})), self.assertRaises(RuntimeError):
                    hot.verify_ndk_archive(download)
            for name in ['other/source.properties', '/android-ndk-r30/source.properties', 'android-ndk-r30/../escape', 'android-ndk-r30/dir\\escape']:
                raw, archive = self.ndk_zip_fixture([name]); download.write_bytes(raw)
                with self.subTest(name=name), patch.object(hot, 'ndk_archive_contract', return_value=archive), self.assertRaises(RuntimeError):
                    hot.verify_ndk_archive(download)

    def test_candidate_ndk_prepare_is_owned_and_receipts_actual_bytes(self):
        # Fake command adapter writes only data fixtures. Popen/network guards
        # remain active; no curl, unzip or native executable is actually invoked.
        raw, archive = self.ndk_zip_fixture(); calls = []
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); evidence = root / 'evidence'; evidence.mkdir()
            def fake_run(args, **kwargs):
                calls.append(args)
                if args[0] == 'curl':
                    self.assertEqual(args[-1], archive['url']); self.assertNotIn('-L', args)
                    self.assertEqual(args[1], '--disable')
                    Path(args[args.index('-o') + 1]).write_bytes(raw)
                elif args[0] == 'unzip':
                    with zipfile.ZipFile(args[2]) as bundle: bundle.extractall(args[4])
                else: raise AssertionError('unexpected command')
            with patch.object(hot, 'ndk_archive_contract', return_value=archive):
                ndk, facts = hot.prepare_candidate_ndk(root, evidence, fake_run)
                self.assertEqual((ndk / 'source.properties').read_bytes(), self.stable_ndk_properties())
                self.assertEqual(facts['profile'], hot.ANDROID)
                self.assertEqual(facts['archive']['sha256'], archive['sha256'])
                self.assertEqual(facts['sourceProperties']['sha256'], hot.ANDROID['sourcePropertiesSha256'])
                self.assertEqual(json.loads((evidence / 'candidate-ndk.json').read_text()), facts)
                self.assertFalse((root / 'candidate-ndk.zip').exists())
                with self.assertRaises(RuntimeError): hot.prepare_candidate_ndk(root, evidence, fake_run)
                self.assertEqual([args[0] for args in calls], ['curl', 'unzip'])

    def test_candidate_ndk_prepare_refuses_properties_drift(self):
        bad = self.stable_ndk_properties().replace(b'30.0.16248370', b'30.0.16138531')
        with patch.object(self, 'stable_ndk_properties', return_value=bad): raw, archive = self.ndk_zip_fixture()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); evidence = root / 'evidence'; evidence.mkdir()
            def fake_run(args, **kwargs):
                if args[0] == 'curl': Path(args[args.index('-o') + 1]).write_bytes(raw)
                elif args[0] == 'unzip':
                    with zipfile.ZipFile(args[2]) as bundle: bundle.extractall(args[4])
                else: raise AssertionError('unexpected command')
            with patch.object(hot, 'ndk_archive_contract', return_value=archive), self.assertRaisesRegex(RuntimeError, 'source.properties bytes differ'):
                hot.prepare_candidate_ndk(root, evidence, fake_run)
            self.assertFalse((evidence / 'candidate-ndk.json').exists())

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
            selected = steps(self.workflow, name)['Resolve bundled core version']
            self.assertEqual(condition(selected), NORMAL)
            self.assertNotIn('bundledCoreVersion', selected)
            self.assertIn(LEGACY_VERSION_COMMANDS, restore_normal_version_diagnostic(selected))
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
        self.assertEqual(paths, ['${{ steps.hot_candidate.outputs.evidence }}/*' + suffix for suffix in ('.json', '.xml', '.log', '.log.stdout.raw', '.log.stderr.raw')])
        self.assertFalse(any(re.search(r'\.(?:aar|apk|aab|so|jks)', path) for path in paths))
        self.assertIn('if-no-files-found: error', block)

    def test_no_native_runtime_release_install_or_formal_shortcuts(self):
        self.assertEqual(hot.BUILD, ['bash', 'scripts/build-android-apk.sh', '--debug', '--apk', '--split-per-abi', '--target', 'aarch64', '--ci', '--config', 'src-tauri/tauri.android.conf.json'])
        for forbidden in ['ci_android.py prepare', 'fork_input.load(', 'inspect_aar(', 'producer.execute(', 'materialize-android', 'gh release', 'adb ', 'sudo ', 'sdkmanager', 'gomobile init']:
            executable = '\n'.join(line for line in self.source.splitlines() if not line.lstrip().startswith(('#', '"""')))
            self.assertNotIn(forbidden, executable)
        self.assertIn("'runtimeTestsRun': 0", self.source)
        self.assertIn("'full native archive provenance and reproducibility'", self.source)
        self.assertIn("'org.gradle.daemon=false\\norg.gradle.workers.max=2\\nkotlin.compiler.execution.strategy=in-process", self.source)

    def test_candidate_runs_exact_debug_unit_suite_after_build_and_retains_same_inputs(self):
        self.assertEqual(hot.UNIT_TEST_CLASSES, ('RuntimeProtocolAdmissionTest', 'RuntimePersistenceSourceTest'))
        self.assertEqual(hot.UNIT_TEST_BUILD, ['bash', 'src-tauri/gen/android/gradlew', '--project-dir', 'src-tauri/gen/android',
            ':app:cleanTestArm64DebugUnitTest', ':app:testArm64DebugUnitTest',
            '--tests', 'com.polaris2.app.vpn.RuntimeProtocolAdmissionTest',
            '--tests', 'com.polaris2.app.vpn.RuntimePersistenceSourceTest',
            '-x', ':app:rustBuildArm64Debug', '--rerun-tasks', '--max-workers=2', '--no-daemon'])
        self.assertLess(self.source.index('run(BUILD,'), self.source.index('run(UNIT_TEST_BUILD,'))
        self.assertLess(self.source.index("result['apk'] = inspect_apk("), self.source.index('run(UNIT_TEST_BUILD,'))
        self.assertIn("finally:\n            result['unitTestReports'] = unit_test_reports(", self.source)
        self.assertIn("file_pin(stage) == component['artifact'] and file_pin(rust) == rust_pin", self.source)
        self.assertIn("write_json(evidence / 'unit-results.json', result['unitTests'])", self.source)
        component = self.source.split('def inspect_component(', 1)[1].split('def inspect_apk(', 1)[0]
        self.assertIn('java = inspect_hot_java(classes, tools, run)', component)
        self.assertIn('expected = hot_jni_contract(exported)', component)

    def test_normal_lane_selects_protocol_class_and_retains_full_method_evidence(self):
        selected = steps(self.workflow, 'release_check')['Verify S5 Android network observation and admission unit tests']
        self.assertIn('--tests com.polaris2.app.vpn.RuntimeProtocolAdmissionTest', selected)
        self.assertIn("'RuntimePersistenceSourceTest', 'RuntimeProtocolAdmissionTest']", selected)
        for guard in ['methods and len(methods) == len(set(methods))', 'len(cases) == len(methods)',
                      "sorted(c.get('name') for c in cases) == sorted(methods)", "('failures', 'errors', 'skipped')"]:
            self.assertIn(guard, selected)
        self.assertIn('build/test-results/testArm64ReleaseUnitTest/TEST-com.polaris2.app.vpn.RuntimeProtocolAdmissionTest.xml', self.workflow)

    def test_pins_all_ten_source_vendor_roots_and_formal_untouched(self):
        self.assertEqual(sum(row[2] for row in hot.VENDORS.values()), 2876)
        self.assertEqual(len(hot.VENDORS), 10)
        self.assertEqual(hot.sha((hot.ROOT / 'scripts/mobile-core/source-contract.json').read_bytes()), hot.FORMAL_HASH)
        self.assertIn('trackedMirror', self.source)
        self.assertIn("git(source, 'ls-tree', '-r', 'HEAD')", self.source)
        self.assertIn("mode == '160000'", self.source)
        self.assertNotIn('artifactPins', self.source)

    def test_mutated_gate_and_default_command_are_detected(self):
        for name in DEFAULT_DIGESTS:
            original = steps(self.workflow, name)['Resolve bundled core version']
            for changed in [original.replace("print(load()['version'])", "print('1.15.0-polaris-alpha.12.6')"),
                            original.replace('from fork_input import load', 'from unrelated import load'),
                            original.replace('        if: ' + NORMAL, '        if: false'),
                            original.replace('exit 1;', 'exit 0;'),
                            original.replace('echo "version=$v" >> "$GITHUB_OUTPUT"', 'echo "version=old" >> "$GITHUB_OUTPUT"')]:
                self.assertNotEqual(changed, original)
                with self.assertRaises(AssertionError):
                    restore_normal_version_diagnostic(changed)
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
    if suite.countTestCases() < 40:
        raise SystemExit('candidate fixture suite is empty/incomplete')
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    raise SystemExit(0 if result.wasSuccessful() and not result.skipped else 1)
