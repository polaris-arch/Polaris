"""Offline exact-source, original-byte and fail-closed reuse controls; no native build."""
import ast
import copy
import contextlib
import functools
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import tempfile
import urllib.error
import unittest
from unittest import mock
import zipfile

import apple_merge_reuse as reuse
import apple_rust
import ci_apple
from apple_handoff import validate_aggregate
from artifacts import file_pin
from fork_input import canonical, load


class FakeAPI:
    def __init__(self, routes, paths):
        self.routes, self.paths, self.calls, self.downloads = routes, paths, [], []

    def get(self, path):
        self.calls.append(path)
        return copy.deepcopy(self.routes[path])

    def artifact(self, row):
        self.downloads.append(row['id'])
        path = self.paths[row['id']]
        raw = path.read_bytes()
        reuse.require(len(raw) == row['size_in_bytes'] and 'sha256:' + reuse.sha(raw) == row['digest'],
                      'provider artifact digest/size differs')
        return path


def zip_bytes(files, *, payload=False):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, 'w', compression=zipfile.ZIP_DEFLATED) as package:
        for name, raw in files.items():
            info = zipfile.ZipInfo(name)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            package.writestr(info, raw)
    return stream.getvalue()


def raw_json(value):
    return canonical(value) + b'\n'


def original_zip(files, ctx, root, slot):
    index = {'schema': 'polaris-apple-original-evidence-v2', 'carrierScope': 'device-only',
             'requiredSlots': ['device-arm64'], 'slot': slot, 'receiptReadErrors': {},
             'completeUnsignedChain': False, 'componentAccepted': True,
             'componentOriginalsInThisArchive': slot is None,
             'componentOriginalsReferencedByTransport': slot is not None,
             'sameRunComponentArtifact': reuse.artifact_name('component-original', ctx),
             'devicePassed': False, 'publicationEligible': False,
             'files': {name: {'originalPath': root + '/' + name, **reuse.pin_bytes(raw)} for name, raw in files.items()}}
    return zip_bytes({**files, 'EVIDENCE-INDEX.json': raw_json(index)})


class Fixture:
    def __init__(self, folder):
        self.folder = Path(folder)
        self.head, self.merge, self.tree, self.workflow, self.base = ('b' * 40, 'a' * 40, 'c' * 40, 'd' * 40, 'e' * 40)
        self.ctx = {'repository': reuse.REPOSITORY, 'candidate': self.head, 'runId': '100', 'attempt': '2'}
        self.current = {**self.ctx, 'candidate': self.merge, 'runId': '200', 'attempt': '1'}
        self.root = '/Users/runner/Code/polaris-mobile-apple-ci/100-2'
        self.source = {'candidate': self.merge, 'tree': self.tree,
                       'producer': {'candidate': self.merge, 'files': {'producer.py': reuse.pin_bytes(b'code')}},
                       'app': {'candidate': self.merge, 'consumerCode': {'consumer.py': reuse.pin_bytes(b'code')}},
                       'contract': load(), 'sourceContract': reuse.pin_bytes(b'source-contract'),
                       'producerCode': '1' * 64, 'consumerCode': '2' * 64,
                       'verifier': reuse.pin_bytes(b'verifier'),
                       'workflows': {name: reuse.pin_bytes(name.encode()) for name in reuse.WORKFLOWS}}
        self.producer = {**self.source['producer'], 'candidate': self.head}
        self.app = {**self.source['app'], 'candidate': self.head}
        tools = {}
        versions = {'node': 'v26.1.2', 'pnpm': '11.24.0', 'go': 'go version go1.27.2 darwin/arm64',
                    'gomobile': 'source-contract:v0.1.13', 'gobind': 'source-contract:v0.1.13',
                    'cargo': 'cargo 1.99', 'rustc': 'rustc 1.99\nhost: aarch64-apple-darwin',
                    'xcodebuild': 'Xcode 27.1\nBuild version 27A9275', 'xcrun': 'xcrun version 72',
                    'swiftc': 'Swift 6.4', 'clang': 'clang 22', 'protoc': 'libprotoc 35.1'}
        for name, version in versions.items():
            tools[name] = {'path': '/tools/' + name, **reuse.pin_bytes(name.encode()), 'version': version}
        self.tools = tools
        sdk = {name: {'version': '27.1', 'settings': reuse.pin_bytes(name.encode())}
               for name in ('iphoneos', 'iphonesimulator')}
        self.snapshots = {}
        for phase in reuse.PHASES:
            self.snapshots[phase] = reuse.signed({'schema': reuse.SNAPSHOT_SCHEMA, 'context': self.ctx,
                'phase': phase, 'slot': 'device-arm64' if phase.startswith('carrier-') else None,
                'workflowSha': self.workflow, 'source': reuse.source_summary(self.source),
                'tools': {k: v for k, v in tools.items() if phase.startswith('carrier-') or k != 'protoc'},
                'sdk': sdk, 'overrides': {},
                'cargoConfig': {'.cargo/config.toml': file_pin(reuse.ROOT / '.cargo/config.toml')}, **reuse.FALSE_CLAIMS})
        native_tools = {'executables': {name: {k: tools[name][k] for k in ('bytes', 'sha256')}
                                      for name in ('go', 'gomobile', 'gobind')},
                        'xcode': versions['xcodebuild'] + '\n',
                        'sdk': {name: {**row, 'compiler': {k: tools['clang'][k] for k in ('bytes', 'sha256')}}
                                for name, row in sdk.items()}}
        binary_name = 'component/Libbox.xcframework/ios-arm64/Libbox.framework/Libbox'
        object_name = 'component/build-scratch/work/input.o'
        generated_name = 'component/build-view/build/ios/Libbox.objc'
        self.native_files = {'component/original-bind.log': b'accepted-log',
                             object_name: b'accepted-object', generated_name: b'generated-source'}
        digest = reuse.sha(self.native_files[object_name])
        output = {'format': 'static-XCFramework', 'slices': {'ios-arm64': {'members': [
            {'ordinal': 1, 'sha256': digest, 'goCarrier': False}]}},
            'inventory': [{'path': 'ios-arm64/Libbox.framework/Libbox', 'type': 'file',
                           'mode': 0o644, **reuse.pin_bytes(b'framework')}]}
        native = {'schema': 'polaris-mobile-native-inputs-v2',
            'scratch': self.root + '/component/build-scratch', 'cache': self.root + '/cache',
            'workRoots': [self.root + '/component/build-scratch/work'],
            'originalBindLog': reuse.pin_bytes(b'accepted-log'),
            'files': {self.root + '/' + object_name: {'kind': 'compiler-object', **reuse.pin_bytes(b'accepted-object')}},
            'staticMemberOrigins': {'ios-arm64': [{'memberOrdinal': 1, 'sha256': digest}]},
            'originsByDigest': {digest: [{'kind': 'compiler-object', 'path': self.root + '/' + object_name, 'sha256': digest}]},
            'generatorOutputRoot': self.root + '/component/build-view/build',
            'generatorOutputInventory': [{'path': 'ios/Libbox.objc', 'type': 'file', 'mode': 0o644,
                                          **reuse.pin_bytes(b'generated-source')}]}
        component_receipt = reuse.signed({'schema': 'polaris-mobile-component-v1', 'platform': 'apple',
            'input': {'producer': self.producer, 'sourceContract': self.source['contract'], 'tools': native_tools,
                      'moduleGraph': [{'Path': 'github.com/sagernet/sing-box', 'Main': True,
                                       'Dir': self.root + '/component/build-view'}]},
            'nativeInputs': native, 'output': output,
            'sourceOverlay': 0, 'AppAccepted': False, 'devicePassed': False})
        prepared = {'App': self.producer, 'carrierScope': 'device-only', 'requiredSlots': ['device-arm64'],
                    'tools': native_tools['executables'], 'rustc': versions['rustc'] + '\n'}
        transported_files = {binary_name: b'framework', **self.native_files}
        manifest = reuse.signed({'schema': 'polaris-apple-component-handoff-v2', 'carrierScope': 'device-only',
            'requiredSlots': ['device-arm64'], 'context': self.ctx, 'producerJob': 'component',
            'root': self.root, 'checkout': '/checkout', 'App': self.producer,
            'sourceContract': self.source['contract'], 'componentFingerprint': component_receipt['fingerprint'],
            'componentReceipt': reuse.pin_bytes(raw_json(component_receipt)),
            'files': {'root': {name: {**reuse.pin_bytes(raw), 'mode': 0o644} for name, raw in transported_files.items()},
                      'checkout': {}}, 'localReplacementSources': [],
            'tools': native_tools['executables'], 'resources': {'NOTICE': reuse.pin_bytes(b'notice')},
            **reuse.FALSE_CLAIMS})
        payload = zip_bytes({**{'root/' + name: raw for name, raw in transported_files.items()},
                             'component-handoff.json': raw_json(manifest)})
        transport = reuse.signed({'schema': 'polaris-apple-handoff-transport-v1', 'context': self.ctx,
                                  'manifest': reuse.pin_bytes(raw_json(manifest)), 'archive': reuse.pin_bytes(payload)})
        link = reuse.signed({'App': self.app, 'componentFingerprint': component_receipt['fingerprint'],
                            'producerInput': component_receipt['input'], 'resources': manifest['resources']})
        link_pin = reuse.pin_bytes(raw_json(link))
        rust = reuse.signed({'schema': 'polaris-formal-apple-rust-v1', 'App': self.app,
            'target': 'aarch64-apple-ios', 'tools': {name: {k: tools[name][k] for k in ('path', 'bytes', 'sha256')}
                                                 for name in ('cargo', 'rustc')},
            'rustc': versions['rustc'] + '\n', 'linkInputSha256': link_pin['sha256'], 'devicePassed': False,
            'sdkVersion': '27.1', 'cargoLock': file_pin(reuse.ROOT / 'Cargo.lock'),
            'iosConfig': file_pin(reuse.ROOT / 'src-tauri/tauri.ios.conf.json'),
            'command': ['/tools/cargo', 'rustc', '--locked', '--release', '--timings', '--lib', '--package', 'polaris',
                        '--target', 'aarch64-apple-ios', '--features', 'tauri/custom-protocol', '--crate-type', 'staticlib'],
            'archive': reuse.pin_bytes(b'rustarchive'),
            'executionInputs': {'overrides': {'RUSTC': reuse.pin_bytes(b'/tools/rustc'),
                'CARGO_TARGET_DIR': reuse.pin_bytes((self.root + '/cache/rust-target').encode())},
                'cargoConfig': {'.cargo/config.toml': file_pin(reuse.ROOT / '.cargo/config.toml')}}})
        carrier = reuse.signed({'schema': 'polaris-mobile-apple-carrier-v1', 'App': self.app,
            'producerCandidate': self.head, 'componentFingerprint': component_receipt['fingerprint'],
            'selectedTarget': 'ios-arm64', 'linkInputSha256': link_pin['sha256'],
            'rustInput': {'receipt': self.root + '/device-arm64/rust-aarch64-apple-ios.json', **reuse.pin_bytes(raw_json(rust))},
            'AppCarrierValidated': True, 'AppAccepted': False, 'devicePassed': False,
            'signatureTrustValidated': False, 'ipa': reuse.pin_bytes(b'ipa')})
        wrapper = reuse.signed({'schema': 'polaris-apple-carrier-handoff-v2', 'carrierScope': 'device-only',
            'requiredSlots': ['device-arm64'], 'context': self.ctx, 'slot': 'device-arm64',
            'componentTransport': transport, 'receipt': carrier, 'receiptPin': reuse.pin_bytes(raw_json(carrier)),
            **reuse.FALSE_CLAIMS})
        summaries = self.folder / 'fixture-summaries'
        (summaries / 'component').mkdir(parents=True)
        (summaries / 'device-arm64').mkdir()
        self.summary_files = {'carrier-device-arm64.json': raw_json(wrapper),
                              'summary-app-carrier.json': raw_json(carrier), 'summary-rust-receipt.json': raw_json(rust)}
        for name, value in self.summary_files.items():
            (summaries / 'device-arm64' / name).write_bytes(value)
        self.manifest_files = {'component-handoff.json': raw_json(manifest), 'component-transport.json': raw_json(transport)}
        for name, value in self.manifest_files.items():
            (summaries / 'component' / name).write_bytes(value)
        chain = validate_aggregate(summaries, scope='device-only', expected_context=self.ctx,
                                    expected_producer=self.producer, expected_app=self.app,
                                    expected_source_contract=self.source['contract'])
        self.component_files = {'component/component-receipt.json': raw_json(component_receipt),
            'tools.json': raw_json(prepared), binary_name: b'framework', **self.native_files,
            **{'execution-' + phase + '.json': raw_json(row) for phase, row in self.snapshots.items() if phase.startswith('frontend-')}}
        self.carrier_files = {'device-arm64/app-carrier.json': raw_json(carrier),
            'device-arm64/rust-aarch64-apple-ios.json': raw_json(rust),
            'device-arm64/apple-link-input.json': raw_json(link),
            'cache/rust-target/aarch64-apple-ios/release/libpolaris_lib.a': b'rustarchive',
            'device-arm64/original-linked-rust-archive.a': b'rustarchive',
            'device-arm64/polaris-box-' + self.source['contract']['version'] + '-app-link-probe.ipa': b'ipa',
            **{'device-arm64/execution-' + phase + '.json': raw_json(row) for phase, row in self.snapshots.items() if phase.startswith('carrier-')}}
        self.archive_files = {
            'component': {'component-handoff.zip': payload, 'component-transport.json': raw_json(transport)},
            'component-manifest': self.manifest_files,
            'component-original': {'apple-original-evidence.zip': original_zip(self.component_files, self.ctx, self.root, None)},
            'carrier-summary-device-arm64': self.summary_files,
            'carrier-original-device-arm64': {'apple-original-device-arm64-evidence.zip': original_zip(self.carrier_files, self.ctx, self.root, 'device-arm64')},
            'chain': {'apple-chain.json': raw_json(chain)}}
        self.paths, self.artifacts = {}, {}
        for role, files in self.archive_files.items():
            self.set_artifact(role, files)
        self.provider = {'workflowId': 55, 'workflowPath': reuse.WORKFLOWS[0], 'workflowSha': self.workflow,
                         'workflowRef': 'refs/pull/15/merge', 'headBranch': 'reviewed-source',
                         'jobs': {reuse.COMPONENT_JOB: 66, reuse.CHAIN_JOB: 67}}
        run = {'id': 100, 'run_attempt': 2, 'head_sha': self.head, 'path': reuse.WORKFLOWS[0],
               'repository': {'id': reuse.REPOSITORY_ID}, 'head_repository': {'id': reuse.REPOSITORY_ID},
               'workflow_id': 55, 'head_branch': 'reviewed-source', 'event': 'pull_request', 'status': 'completed', 'conclusion': 'success',
               'referenced_workflows': [{'path': reuse.REPOSITORY + '/' + reuse.WORKFLOWS[1] + '@' + self.workflow,
                                         'sha': self.workflow, 'ref': 'refs/pull/15/merge'}]}
        pr = {'number': 15, 'merged': True, 'merged_at': '2026-10-18T10:00:00Z', 'merge_commit_sha': self.merge,
              'head': {'sha': self.head, 'ref': 'reviewed-source', 'repo': {'id': reuse.REPOSITORY_ID}},
              'base': {'sha': self.base, 'ref': 'main', 'repo': {'id': reuse.REPOSITORY_ID}}, 'user': {'id': 1}}
        self.routes = {
            '/git/commits/' + self.merge: {'sha': self.merge, 'tree': {'sha': self.tree}, 'parents': [{'sha': self.base}, {'sha': self.head}]},
            '/git/commits/' + self.head: {'sha': self.head, 'tree': {'sha': self.tree}},
            '/git/commits/' + self.workflow: {'sha': self.workflow, 'tree': {'sha': self.tree}},
            '/commits/' + self.merge + '/pulls?per_page=100': [pr],
            '/pulls/15': copy.deepcopy(pr),
            '/actions/runs/100/attempts/2': run,
            '/actions/runs/100/attempts/2/jobs?per_page=100': {'total_count': 2, 'jobs': [
                {'id': identifier, 'name': name, 'head_sha': self.head, 'run_attempt': 2, 'run_id': 100,
                 'status': 'completed', 'conclusion': 'success'} for name, identifier in self.provider['jobs'].items()]},
            '/actions/workflows/release-risk.yml/runs?event=pull_request&status=success&head_sha=' + self.head + '&per_page=100':
                {'total_count': 1, 'workflow_runs': [copy.deepcopy(run)]}}
        self.api = FakeAPI(self.routes, self.paths)
        with tempfile.TemporaryDirectory() as work:
            self.evidence = reuse.read_evidence(self.api, self.artifacts, self.ctx, self.source, work)
        self.contract = reuse.signed({'schema': reuse.CONTRACT_SCHEMA, 'policy': reuse.POLICY,
            'context': self.ctx, 'repositoryId': reuse.REPOSITORY_ID, 'scope': 'device-only',
            'origin': 'full-native-build', 'provider': self.provider, 'evidence': self.evidence,
            'artifacts': {role: reuse.artifact_reference(self.artifacts[role]) for role in reuse.ARTIFACT_ROLES},
            **reuse.FALSE_CLAIMS})
        self.set_artifact('execution-contract', {'apple-execution-contract.json': raw_json(self.contract)})
        self.refresh_list()

    def set_artifact(self, role, files):
        identifier = self.artifacts.get(role, {}).get('id', len(self.artifacts) + 1)
        path = self.folder / (str(identifier) + '.zip')
        raw = zip_bytes(files)
        path.write_bytes(raw)
        self.paths[identifier] = path
        self.artifacts[role] = {'id': identifier, 'name': reuse.artifact_name(role, self.ctx),
            'size_in_bytes': len(raw), 'digest': 'sha256:' + reuse.sha(raw), 'expired': False,
            'workflow_run': {'id': 100, 'repository_id': reuse.REPOSITORY_ID,
                             'head_repository_id': reuse.REPOSITORY_ID, 'head_sha': self.head}}

    def refresh_list(self):
        self.routes['/actions/runs/100/artifacts?per_page=100'] = {'total_count': len(self.artifacts),
                                                                'artifacts': list(self.artifacts.values())}

    def replace_rust_execution(self, execution, *, command=None):
        rust = reuse.safe_json(self.carrier_files['device-arm64/rust-aarch64-apple-ios.json'])
        rust.pop('fingerprint')
        if execution is None:
            rust.pop('executionInputs')
        else:
            rust['executionInputs'] = execution
        if command is not None:
            rust['command'] = command
        rust = reuse.signed(rust)
        carrier = reuse.safe_json(self.carrier_files['device-arm64/app-carrier.json'])
        carrier.pop('fingerprint'); carrier['rustInput'].update(reuse.pin_bytes(raw_json(rust)))
        carrier = reuse.signed(carrier)
        wrapper = reuse.safe_json(self.summary_files['carrier-device-arm64.json'])
        wrapper.pop('fingerprint'); wrapper.update(receipt=carrier, receiptPin=reuse.pin_bytes(raw_json(carrier)))
        wrapper = reuse.signed(wrapper)
        self.summary_files.update({'carrier-device-arm64.json': raw_json(wrapper),
            'summary-app-carrier.json': raw_json(carrier), 'summary-rust-receipt.json': raw_json(rust)})
        self.carrier_files.update({'device-arm64/rust-aarch64-apple-ios.json': raw_json(rust),
                                   'device-arm64/app-carrier.json': raw_json(carrier)})
        summaries = self.folder / 'fixture-summaries'
        for name, raw in self.summary_files.items():
            (summaries / 'device-arm64' / name).write_bytes(raw)
        chain = validate_aggregate(summaries, scope='device-only', expected_context=self.ctx,
            expected_producer=self.producer, expected_app=self.app, expected_source_contract=self.source['contract'])
        self.set_artifact('carrier-summary-device-arm64', self.summary_files)
        self.set_artifact('carrier-original-device-arm64', {'apple-original-device-arm64-evidence.zip':
            original_zip(self.carrier_files, self.ctx, self.root, 'device-arm64')})
        self.archive_files['chain'] = {'apple-chain.json': raw_json(chain)}
        self.set_artifact('chain', self.archive_files['chain'])
        self.refresh_list()

    def exact_head_merge(self):
        # A multi-commit PR fast-forward: the final commit's parent is not the
        # old main base. Original producer run/attempt/receipts stay unchanged.
        old_target = self.merge
        pr = copy.deepcopy(self.routes['/pulls/15'])
        self.merge = self.head
        self.current['candidate'] = self.head
        self.source['candidate'] = self.head
        self.source['producer']['candidate'] = self.head
        self.source['app']['candidate'] = self.head
        pr['merge_commit_sha'] = self.head
        self.routes.pop('/git/commits/' + old_target)
        self.routes.pop('/commits/' + old_target + '/pulls?per_page=100')
        self.routes['/git/commits/' + self.head] = {
            'sha': self.head, 'tree': {'sha': self.tree}, 'parents': [{'sha': '9' * 40}]}
        self.routes['/commits/' + self.head + '/pulls?per_page=100'] = [pr]
        self.routes['/pulls/15'] = copy.deepcopy(pr)

    def resolve(self):
        with tempfile.TemporaryDirectory() as work:
            return reuse.resolve_evidence(self.api, self.current, self.source, work)


def both_merge_forms(test):
    @functools.wraps(test)
    def run(self):
        original = self.fx
        try:
            for exact_head in (False, True):
                with self.subTest(exact_head=exact_head):
                    if not exact_head:
                        test(self)
                    else:
                        with tempfile.TemporaryDirectory() as folder:
                            self.fx = Fixture(folder)
                            self.fx.exact_head_merge()
                            test(self)
        finally:
            self.fx = original
    return run


class AppleMergeReuseControls(unittest.TestCase):
    def setUp(self):
        self.error_capture = contextlib.redirect_stderr(io.StringIO())
        self.error_capture.__enter__()
        self.addCleanup(self.error_capture.__exit__, None, None, None)
        self.output_capture = contextlib.redirect_stdout(io.StringIO())
        self.output_capture.__enter__()
        self.addCleanup(self.output_capture.__exit__, None, None, None)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.fx = Fixture(self.temp.name)

    def test_cargo_timing_command_remains_exact_not_an_extra_flags_allowance(self):
        for kind in ('missing-timings', 'duplicate-timings', 'verbose'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as folder:
                fx = Fixture(folder)
                rust = reuse.safe_json(fx.carrier_files['device-arm64/rust-aarch64-apple-ios.json'])
                command = rust['command'][:]
                if kind == 'missing-timings':
                    command.remove('--timings')
                else:
                    command.append('--timings' if kind == 'duplicate-timings' else '-vv')
                fx.replace_rust_execution(rust['executionInputs'], command=command)
                with tempfile.TemporaryDirectory() as work, \
                        self.assertRaisesRegex(RuntimeError, 'Rust command contract differs'):
                    reuse.read_evidence(fx.api, fx.artifacts, fx.ctx, fx.source, work)

    @both_merge_forms
    def test_positive_retains_all_original_bytes_and_reference_only_receipt(self):
        before = {path: path.read_bytes() for path in self.fx.paths.values()}
        result = self.fx.resolve()
        self.assertEqual(result['producer'], self.fx.ctx)
        self.assertEqual(result['context'], self.fx.current)
        self.assertEqual(result['proof']['tree'], self.fx.tree)
        self.assertEqual(result['result'], 'verified-reuse')
        proof = result['proof']
        self.assertEqual(proof['producerHead'], self.fx.head)
        if self.fx.merge == self.fx.head:
            self.assertEqual(proof['mergeCommit'], proof['producerHead'])
            self.assertEqual(proof['parents'], ['9' * 40])
            self.assertNotEqual(proof['parents'][0], self.fx.base)
        else:
            self.assertEqual(proof['parents'], [self.fx.base, self.fx.head])
        self.assertLess(len(canonical(result)), reuse.MAX_JSON)
        self.assertNotIn('carriers', result)
        self.assertNotIn('evidence', result)
        for key in reuse.FALSE_CLAIMS:
            self.assertIs(result[key], False)
        with tempfile.TemporaryDirectory() as folder:
            output, receipt = Path(folder) / 'output', Path(folder) / 'result.json'
            env = {'POLARIS_APPLE_ALLOW_MERGE_REUSE': 'true', 'GITHUB_EVENT_NAME': 'push',
                   'GITHUB_REF': 'refs/heads/main', 'POLARIS_APPLE_SCOPE': 'device-only',
                   'POLARIS_APP_CANDIDATE': self.fx.merge, 'GITHUB_SHA': self.fx.merge,
                   'GITHUB_OUTPUT': str(output)}
            with mock.patch.dict(os.environ, env), \
                    mock.patch.object(reuse, 'current_context', return_value=self.fx.current), \
                    mock.patch.object(reuse, 'source_bindings', return_value=self.fx.source), \
                    mock.patch.object(reuse, 'GitHub', return_value=self.fx.api):
                reuse.resolve(receipt)
                fields = dict(line.split('=', 1) for line in output.read_text().splitlines())
                self.assertEqual(fields['reused'], 'true')
                self.assertEqual(fields['reason'], 'verified-exact-tree-producer')
                self.assertEqual(output.read_text().splitlines(), ['reused=true',
                    'receipt_sha256=' + fields['receipt_sha256'], 'reason=verified-exact-tree-producer'])
                reuse.check_result(receipt, fields['receipt_sha256'])
            self.assertEqual(json.loads(receipt.read_text()), result)
        self.assertEqual(before, {path: path.read_bytes() for path in self.fx.paths.values()})

    def closure(self, *, files=None, manifest=None, receipt=None, index_change=None, mode_change=None):
        files = copy.deepcopy(self.fx.component_files if files is None else files)
        manifest = copy.deepcopy(manifest or reuse.safe_json(self.fx.manifest_files['component-handoff.json']))
        receipt = copy.deepcopy(receipt or reuse.safe_json(files['component/component-receipt.json']))
        raw = original_zip(files, self.fx.ctx, self.fx.root, None)
        with zipfile.ZipFile(io.BytesIO(raw)) as package:
            index = reuse.check_original(package, self.fx.ctx, None)
            if index_change:
                index_change(index)
            if mode_change:
                mode_change(package)
            reuse.validate_native_closure(package, index, manifest, receipt)

    @both_merge_forms
    def test_original_review_four_probes_reject_sealing_and_resolving(self):
        # Provider ZIP/index/digest are all coherently updated. Immutable receipt
        # and transport remain untouched; both trust boundaries must reject.
        chain = Path(self.temp.name) / 'chain-for-negative.json'
        chain.write_bytes(self.fx.archive_files['chain']['apple-chain.json'])
        env = {'POLARIS_APPLE_SCOPE': 'device-only', 'GITHUB_JOB': 'unsigned-chain',
               'GITHUB_EVENT_NAME': 'pull_request',
               'GITHUB_WORKFLOW_REF': reuse.REPOSITORY + '/' + reuse.WORKFLOWS[0] + '@refs/pull/15/merge'}
        run = self.fx.routes['/actions/runs/100/attempts/2']
        job = self.fx.routes['/actions/runs/100/attempts/2/jobs?per_page=100']['jobs'][1]
        for number, (name, bad) in enumerate([
                ('component/original-bind.log', b'changed-log'),
                ('component/build-scratch/work/input.o', b'changed-object'),
                ('component/original-bind.log', None),
                ('component/build-scratch/work/input.o', None)]):
            files = copy.deepcopy(self.fx.component_files)
            if bad is None:
                del files[name]
            else:
                files[name] = bad
            self.fx.set_artifact('component-original', {'apple-original-evidence.zip':
                                 original_zip(files, self.fx.ctx, self.fx.root, None)})
            self.fx.refresh_list()
            run.update(status='in_progress', conclusion=None)
            job.update(status='in_progress', conclusion=None)
            output = Path(self.temp.name) / ('invalid-seal-' + str(number) + '.json')
            with self.subTest(path=name, mutation='delete' if bad is None else 'bytes'), \
                    mock.patch.dict(os.environ, env), \
                    mock.patch.object(reuse, 'current_context', return_value=self.fx.ctx), \
                    mock.patch.object(reuse, 'source_bindings', return_value=self.fx.source), \
                    mock.patch.object(reuse, 'GitHub', return_value=self.fx.api):
                with self.assertRaisesRegex(RuntimeError, 'required original|original/transport'):
                    reuse.seal(Path(self.temp.name) / 'fixture-summaries', chain, output)
                self.assertFalse(output.exists())
            run.update(status='completed', conclusion='success')
            job.update(status='completed', conclusion='success')
            with self.subTest(boundary='resolve', path=name, bad=bad), self.assertRaises(RuntimeError):
                self.fx.resolve()
            self.assertEqual(files['component/component-receipt.json'],
                             self.fx.component_files['component/component-receipt.json'])

    def test_native_original_closure_missing_generated_extra_objects_and_overlap(self):
        self.closure()
        for name in self.fx.native_files:
            for action in ('delete', 'change'):
                files = copy.deepcopy(self.fx.component_files)
                if action == 'delete':
                    files.pop(name)
                else:
                    files[name] = b'other original'
                with self.subTest(name=name, action=action), self.assertRaises(RuntimeError):
                    self.closure(files=files)
        for name in ('component/build-scratch/work/extra.o', 'cache/extra.a',
                     'component/build-view/build/extra.go'):
            files = {**self.fx.component_files, name: b'extra'}
            with self.subTest(extra=name), self.assertRaises(RuntimeError):
                self.closure(files=files)
        # Any overlapping original/transported file must agree, not only native files.
        files = {**self.fx.component_files, 'component/extra.log': b'original'}
        manifest = reuse.safe_json(self.fx.manifest_files['component-handoff.json'])
        manifest['files']['root']['component/extra.log'] = {**reuse.pin_bytes(b'changed'), 'mode': 0o644}
        with self.assertRaisesRegex(RuntimeError, 'original/transport'):
            self.closure(files=files, manifest=manifest)

    def test_native_receipt_missing_mapping_hash_path_mode_and_cross_attempt_reject(self):
        receipt = reuse.safe_json(self.fx.component_files['component/component-receipt.json'])
        path = self.fx.root + '/component/build-scratch/work/input.o'
        mutations = [lambda row: row.pop('nativeInputs'),
            lambda row: row['nativeInputs'].pop('files'),
            lambda row: row['nativeInputs']['files'].pop(path),
            lambda row: row['nativeInputs']['files'][path].update(sha256='0' * 64),
            lambda row: row['nativeInputs'].update(originalBindLog=reuse.pin_bytes(b'wrong')),
            lambda row: row['nativeInputs'].update(staticMemberOrigins={}),
            lambda row: row['nativeInputs'].update(originsByDigest={}),
            lambda row: row['nativeInputs'].update(workRoots=[self.fx.root + '/component/other']),
            lambda row: row['nativeInputs'].update(generatorOutputInventory=[]),
            lambda row: row['nativeInputs']['generatorOutputInventory'][0].update(mode=0o600),
            lambda row: row['nativeInputs']['generatorOutputInventory'][0].update(path='../outside'),
            lambda row: row['nativeInputs'].update(generatorOutputRoot=self.fx.root.replace('100-2', '100-1') + '/component/build-view/build'),
            lambda row: row['output']['inventory'][0].update(sha256='0' * 64),
            lambda row: row['nativeInputs']['files'].update({self.fx.root + '/component/build-scratch/work/../input.o':
                row['nativeInputs']['files'][path]})]
        for number, mutate in enumerate(mutations):
            changed = copy.deepcopy(receipt); mutate(changed)
            with self.subTest(number=number), self.assertRaises((KeyError, RuntimeError)):
                self.closure(receipt=changed)
        for name in self.fx.native_files:
            with self.subTest(path=name), self.assertRaisesRegex(RuntimeError, 'relabelled'):
                self.closure(index_change=lambda index: index['files'][name].update(
                    originalPath=self.fx.root.replace('100-2', '100-1') + '/' + name))
            with self.subTest(mode=name), self.assertRaisesRegex(RuntimeError, 'mode'):
                self.closure(mode_change=lambda package: setattr(package.getinfo(name), 'external_attr',
                    (stat.S_IFREG | 0o600) << 16))
            with self.subTest(type=name), self.assertRaisesRegex(RuntimeError, 'type'):
                self.closure(mode_change=lambda package: setattr(package.getinfo(name), 'external_attr',
                    (stat.S_IFLNK | 0o644) << 16))

    def test_external_static_origin_uses_only_bounded_archive_bytes(self):
        from test_apple_entry import object_archive
        from native_inputs import source_table, member_references
        raw = object_archive(0x100000c, 0, 2)
        name = 'cache/third-party/runtime.a'
        path = self.fx.root + '/' + name
        files = {**self.fx.component_files, name: raw}
        manifest = reuse.safe_json(self.fx.manifest_files['component-handoff.json'])
        manifest['files']['root'][name] = {**reuse.pin_bytes(raw), 'mode': 0o644}
        receipt = reuse.safe_json(files['component/component-receipt.json'])
        native = receipt['nativeInputs']
        native['files'][path] = {'kind': 'external-static-archive', **reuse.pin_bytes(raw)}
        table = source_table(native['files'], reader=lambda unused: (raw, reuse.pin_bytes(raw)))
        digest = next(key for key, values in table.items() if values[0]['kind'] == 'external-static-archive')
        receipt['output']['slices']['ios-arm64']['members'].append({'ordinal': 2, 'sha256': digest, 'goCarrier': False})
        refs, origins = member_references(receipt['output'], table)
        native.update(staticMemberOrigins=refs, originsByDigest=origins)
        with mock.patch('native_inputs.read_pinned', side_effect=AssertionError('no producer host reads')):
            self.closure(files=files, manifest=manifest, receipt=receipt)
        for mutation in ('bytes', 'delete', 'pin', 'member', 'path'):
            changed_files, changed = copy.deepcopy(files), copy.deepcopy(receipt)
            if mutation == 'bytes': changed_files[name] = raw + b'changed'
            if mutation == 'delete': changed_files.pop(name)
            if mutation == 'pin': changed['nativeInputs']['files'][path]['sha256'] = '0' * 64
            if mutation == 'member': changed['nativeInputs']['originsByDigest'][digest][0]['memberOrdinal'] = 99
            if mutation == 'path': changed['nativeInputs']['originsByDigest'][digest][0]['path'] = '/outside/runtime.a'
            with self.subTest(mutation=mutation), self.assertRaises((KeyError, RuntimeError)):
                self.closure(files=changed_files, manifest=manifest, receipt=changed)

    def test_replacement_source_inventory_and_source_contract_closure(self):
        receipt = reuse.safe_json(self.fx.component_files['component/component-receipt.json'])
        manifest = reuse.safe_json(self.fx.manifest_files['component-handoff.json'])
        source = self.fx.root + '/component/build-view'
        directory = source + '/local-module'
        receipt['input']['moduleGraph'].append({'Path': 'example.local/library',
            'Replace': {'Path': './local-module', 'Dir': directory}})
        rows = [{'path': 'local.go', 'type': 'file', 'mode': 0o644, **reuse.pin_bytes(b'local source')}]
        snapshot = {'module': 'example.local/library', 'path': './local-module', 'sourceRoot': source,
                    'directory': directory, 'inventory': rows,
                    'goMod': reuse.pin_bytes(b'go mod'), 'goSum': reuse.pin_bytes(b'go sum')}
        manifest['localReplacementSources'] = [snapshot]
        receipt['input']['sourceContract'].update(goModSha256=snapshot['goMod']['sha256'],
                                                  goSumSha256=snapshot['goSum']['sha256'])
        for name, raw in [('local-module/local.go', b'local source'), ('go.mod', b'go mod'), ('go.sum', b'go sum')]:
            manifest['files']['root']['component/build-view/' + name] = {**reuse.pin_bytes(raw), 'mode': 0o644}
        self.closure(manifest=manifest, receipt=receipt)
        for mutate in [lambda row: row['localReplacementSources'].clear(),
                       lambda row: row['localReplacementSources'][0]['inventory'][0].update(sha256='0' * 64),
                       lambda row: row['localReplacementSources'][0].update(directory=directory.replace('100-2','100-1')),
                       lambda row: row['files']['root'].pop('component/build-view/local-module/local.go'),
                       lambda row: row['localReplacementSources'][0]['goMod'].update(sha256='0' * 64),
                       lambda row: row['localReplacementSources'][0]['inventory'][0].update(path='../outside'),
                       lambda row: row['files']['root'].update({'component/build-view/local-module/extra.go':
                           {**reuse.pin_bytes(b'extra'), 'mode': 0o644}})]:
            changed = copy.deepcopy(manifest); mutate(changed)
            with self.assertRaises((KeyError, RuntimeError)):
                self.closure(manifest=changed, receipt=receipt)

    def test_cargo_semantic_aliases_are_bound_or_explicitly_refused(self):
        for name in ['CARGO_BUILD_RUSTFLAGS', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
                     'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS', 'CARGO_BUILD_RUSTDOCFLAGS',
                     'CARGO_TARGET_AARCH64_APPLE_IOS_RUSTFLAGS', 'RUSTC_WRAPPER',
                     'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER',
                     'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTDOC',
                     'RUSTDOC', 'RUST_TARGET_PATH', 'RUSTC_BOOTSTRAP', 'CARGO_UNSTABLE_BUILD_STD',
                     'CARGO_TARGET_AARCH64_APPLE_IOS_LINKER', 'CARGO_TARGET_AARCH64_APPLE_IOS_RUNNER',
                     'CARGO_BUILD_TARGET', 'CARGO_RESOLVER_LOCKFILE_PATH', 'CARGO', 'CARGO_REGISTRIES_PRIVATE_INDEX']:
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, 'unsupported'):
                reuse.environment_pins({name: 'unretained-input'})
        for name in ['CARGO_PROFILE_RELEASE_OPT_LEVEL', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS',
                     'CARGO_PROFILE_RELEASE_LTO', 'CARGO_PROFILE_RELEASE_PANIC',
                     'CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL', 'CARGO_INCREMENTAL',
                     'CARGO_BUILD_INCREMENTAL', 'CARGO_HOME', 'CARGO_TARGET_DIR',
                     'CARGO_BUILD_DEP_INFO_BASEDIR', 'CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS',
                     'CARGO_NET_OFFLINE', 'RUSTUP_TOOLCHAIN', 'RUSTC', 'CFLAGS']:
            before = copy.deepcopy(self.fx.snapshots['carrier-before'])
            before['overrides'] = reuse.environment_pins({name: 'before'})
            after = copy.deepcopy(before); after['overrides'] = reuse.environment_pins({name: 'after'})
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, 'changed during'):
                reuse.snapshot_pair(before, after)
            self.assertNotIn('before', json.dumps(before['overrides']))

    @both_merge_forms
    def test_cargo_runtime_handles_and_current_run_environment_preserve_positive_reuse(self):
        base = {'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3'}
        first = {**base, 'CARGO_BUILD_JOBS': '4', 'CARGO_MAKEFLAGS': '--jobserver-auth=3,4 -j',
                 'MAKEFLAGS': '--jobserver-auth=3,4', 'TMPDIR': '/private/producer/temp'}
        second = {**base, 'CARGO_BUILD_JOBS': '8', 'CARGO_MAKEFLAGS': '--jobserver-auth=21,22 -j',
                  'MAKEFLAGS': '--jobserver-auth=21,22', 'TMPDIR': '/private/other/temp'}
        self.assertEqual(reuse.environment_pins(first), reuse.environment_pins(second))
        self.assertEqual(first['CARGO_BUILD_JOBS'], '4')  # no env clearing/mutation
        self.assertEqual(second['CARGO_BUILD_JOBS'], '8')
        with mock.patch.dict(os.environ, {**second, 'HOME': '/home/current-linux-runner',
                'CARGO_HOME': '/home/current-linux-runner/.cargo', 'GITHUB_RUN_ID': '200',
                'GITHUB_RUN_ATTEMPT': '1'}, clear=True):
            result = self.fx.resolve()
        self.assertEqual(result['producer'], self.fx.ctx)
        self.assertEqual(result['context'], self.fx.current)
        self.assertNotEqual(result['producer']['runId'], result['context']['runId'])
        self.assertEqual(result['result'], 'verified-reuse')
        proof = result['proof']
        self.assertEqual(proof['producerHead'], self.fx.head)
        if self.fx.merge == self.fx.head:
            self.assertEqual(proof['mergeCommit'], proof['producerHead'])
            self.assertEqual(proof['parents'], ['9' * 40])
            self.assertNotEqual(proof['parents'][0], self.fx.base)
        else:
            self.assertEqual(proof['parents'], [self.fx.base, self.fx.head])

    def test_cargo_credentials_never_exported_even_as_hashes(self):
        secret = 'synthetic-secret-never-retain'
        env = {'CARGO_REGISTRY_TOKEN': secret, 'CARGO_REGISTRIES_PRIVATE_TOKEN': secret,
               'CARGO_REGISTRIES_PRIVATE_CREDENTIAL_PROVIDER': secret, 'GITHUB_TOKEN': secret,
               'CARGO_CREDENTIAL_ALIAS_PRIVATE': secret, 'CARGO_HTTP_PROXY': secret,
               'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3'}
        pins = reuse.environment_pins(env)
        self.assertEqual(set(pins), {'CARGO_PROFILE_RELEASE_OPT_LEVEL'})
        self.assertNotIn(secret, json.dumps(pins))
        self.assertNotIn(reuse.sha(secret.encode()), json.dumps(pins))
        for name in ('CARGO_REGISTRY_TOKEN', 'CARGO_BUILD_JOBS', 'RUSTC_WRAPPER'):
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                reuse.validate_environment_pins({name: reuse.pin_bytes(secret.encode())})

    def test_cargo_configuration_hierarchy_and_unsupported_external_edges(self):
        with tempfile.TemporaryDirectory() as temporary:
            top = Path(temporary).resolve(); root = top / 'repo'; cwd = root / 'src-tauri'
            cwd.mkdir(parents=True); (root / '.cargo').mkdir()
            env = {'HOME': str(top / 'home'), 'CARGO_HOME': str(top / 'cargo-home')}
            config = root / '.cargo/config.toml'
            config.write_text('[profile.release]\nopt-level = 3\n')
            pins = reuse.cargo_configuration(env, cwd, root)
            self.assertEqual(pins, {'.cargo/config.toml': file_pin(config)})
            with mock.patch.object(reuse, 'ROOT', root):
                reuse.validate_cargo_configuration(pins)
                with self.assertRaisesRegex(RuntimeError, 'inventory'):
                    reuse.validate_cargo_configuration({})
                config.write_text('[profile.release]\nopt-level = 0\n')
                with self.assertRaisesRegex(RuntimeError, 'bytes'):
                    reuse.validate_cargo_configuration(pins)
            legacy = root / '.cargo/config'; legacy.write_text('[build]\nincremental = false\n')
            self.assertEqual(set(reuse.cargo_configuration(env, cwd, root)), {'.cargo/config', '.cargo/config.toml'})
            for content in ['include = "outside.toml"\n', '[env]\nRUSTFLAGS = "--cfg hidden"\n',
                            'paths = ["../outside"]\n', '[source.crates-io]\nreplace-with="outside"\n',
                            '[build]\nrustc-wrapper="outside"\n', '[target.aarch64-apple-ios]\nlinker="outside"\n',
                            '[profile.release.build-override]\nrustflags=["--sysroot","/outside"]\n',
                            '[target.aarch64-apple-ios]\nrustflags=["--sysroot","/outside"]\n',
                            '[target.aarch64-apple-ios.some-links]\nrustc-link-lib=["outside"]\n']:
                config.write_text(content)
                with self.subTest(content=content), self.assertRaisesRegex(RuntimeError, 'unsupported'):
                    reuse.cargo_configuration(env, cwd, root)
            config.write_text('[build]\nincremental = false\n')
            for external in [top / '.cargo/config', top / 'cargo-home/config.toml', cwd / '.cargo/config.toml']:
                external.parent.mkdir(exist_ok=True)
                if external.is_relative_to(root):
                    external.symlink_to(config)
                else:
                    external.write_text('[env]\nSECRET="never-read"\n')
                with self.subTest(path=external), self.assertRaisesRegex(RuntimeError, 'external or redirected'):
                    reuse.cargo_configuration(env, cwd, root)
                external.unlink()

    def test_real_snapshot_rejects_original_three_override_probes_and_profile_drift(self):
        for name in ('CARGO_BUILD_RUSTFLAGS', 'RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER',
                     'CARGO_PROFILE_RELEASE_OPT_LEVEL'):
            case = AppleMergeReuseControls('test_snapshot_collects_actual_binary_and_sdk_bytes_before_and_after')
            case.setUp()
            actual = reuse.snapshot
            def collect(phase, slot=None):
                os.environ[name] = '3' if phase.endswith('before') else '0'
                return actual(phase, slot)
            try:
                with mock.patch.object(reuse, 'snapshot', side_effect=collect):
                    case.collect_snapshot_fixture(unavailable=name != 'CARGO_PROFILE_RELEASE_OPT_LEVEL')
                    root = Path(case.temp.name) / 'snapshot-root/device-arm64'
                    before = reuse.safe_json((root / 'execution-carrier-before.json').read_bytes())
                    after = reuse.safe_json((root / 'execution-carrier-after.json').read_bytes())
                    if name != 'CARGO_PROFILE_RELEASE_OPT_LEVEL':
                        for phase, row in [('carrier-before', before), ('carrier-after', after)]:
                            self.assertEqual(row['schema'], reuse.UNAVAILABLE_SCHEMA)
                            with self.subTest(name=name, phase=phase), self.assertRaises(reuse.UnsupportedCargoInputs):
                                reuse.validate_snapshot(row, phase, self.fx.ctx,
                                    reuse.source_summary(self.fx.source), self.fx.source['contract'])
                    else:
                        self.assertEqual(before['schema'], reuse.SNAPSHOT_SCHEMA)
                        self.assertEqual(after['schema'], reuse.SNAPSHOT_SCHEMA)
                        with self.assertRaisesRegex(RuntimeError, 'changed during'):
                            reuse.snapshot_pair(before, after)
            finally:
                case.doCleanups()

    def test_missing_standard_library_parser_is_ineligible_without_masking_corruption(self):
        # Reproduce the actual Python 3.10 module-absence condition without a
        # replacement parser, test skip or weakening the supported-input tests.
        with mock.patch.dict(reuse.sys.modules, {'tomllib': None}):
            with self.assertRaisesRegex(reuse.UnsupportedCargoInputs, 'parser unavailable'):
                reuse.cargo_configuration({})
            self.assertEqual(reuse.observe_cargo_execution_inputs({}),
                             {'unavailable': reuse.UNAVAILABLE_REASON})
            self.collect_snapshot_fixture(unavailable=True)
        snapshot_root = Path(self.temp.name) / 'snapshot-root'
        for corrupted in (False, True):
            with tempfile.TemporaryDirectory() as folder, self.subTest(corrupted=corrupted):
                fx = Fixture(folder)
                def install_actual(component, carrier):
                    for phase in reuse.PHASES:
                        prefix = 'device-arm64/' if phase.startswith('carrier-') else ''
                        path = prefix + 'execution-' + phase + '.json'
                        (carrier if prefix else component)[path] = (snapshot_root / path).read_bytes()
                    if corrupted:
                        carrier['device-arm64/original-linked-rust-archive.a'] = b'corrupted original'
                self.install_qualification_case(fx, execution=True, corrupt=install_actual)
                if corrupted:
                    with self.assertRaises(RuntimeError) as failure:
                        self.seal_qualification_case(fx)
                    self.assertNotIsInstance(failure.exception, reuse.UnsupportedCargoInputs)
                    self.assertFalse((fx.folder / 'ghout').exists())
                else:
                    output, fields = self.seal_qualification_case(fx)
                    self.assertFalse(output.exists())
                    self.assertEqual(fields, 'eligible=false\nreason=' + reuse.UNAVAILABLE_REASON + '\n')
                self.assertFalse((fx.folder / 'new-contract.json').exists())
                with self.assertRaises(RuntimeError):
                    fx.resolve()

    def test_actual_cargo_execution_receipt_is_required_and_matches_admission(self):
        original = reuse.safe_json(self.fx.carrier_files['device-arm64/rust-aarch64-apple-ios.json'])['executionInputs']
        for mutation in ['missing', 'compiler', 'directory', 'profile', 'wrapper', 'config', 'runtime', 'unavailable']:
            execution = copy.deepcopy(original)
            if mutation == 'missing': execution = None
            if mutation == 'compiler': execution['overrides']['RUSTC'] = reuse.pin_bytes(b'/other/rustc')
            if mutation == 'directory': execution['overrides']['CARGO_TARGET_DIR'] = reuse.pin_bytes(b'/other/run')
            if mutation == 'profile': execution['overrides']['CARGO_PROFILE_RELEASE_OPT_LEVEL'] = reuse.pin_bytes(b'0')
            if mutation == 'wrapper': execution['overrides']['RUSTC_WRAPPER'] = reuse.pin_bytes(b'/outside/wrapper')
            if mutation == 'config': execution['cargoConfig'] = {}
            if mutation == 'runtime': execution['overrides']['CARGO_MAKEFLAGS'] = reuse.pin_bytes(b'--jobserver-auth=3,4')
            if mutation == 'unavailable': execution = {'unavailable': reuse.UNAVAILABLE_REASON}
            self.fx.replace_rust_execution(execution)
            with tempfile.TemporaryDirectory() as work, self.subTest(mutation=mutation):
                if mutation == 'unavailable':
                    evidence = reuse.read_evidence(self.fx.api, self.fx.artifacts, self.fx.ctx, self.fx.source, work)
                    self.assertIs(evidence['eligible'], False)
                    self.assertIsNone(evidence['toolContract'])
                else:
                    with self.assertRaises((RuntimeError, KeyError)):
                        reuse.read_evidence(self.fx.api, self.fx.artifacts, self.fx.ctx, self.fx.source, work)
        self.fx.replace_rust_execution(original)
        with tempfile.TemporaryDirectory() as work:
            evidence = reuse.read_evidence(self.fx.api, self.fx.artifacts, self.fx.ctx, self.fx.source, work)
        self.assertEqual(evidence, self.fx.evidence)

    def test_unsupported_cargo_is_explicitly_ineligible_and_does_not_change_environment(self):
        env = {'RUSTC_WRAPPER': '/unretained/wrapper', 'CARGO_BUILD_JOBS': '7'}
        self.assertEqual(reuse.observe_cargo_execution_inputs(env), {'unavailable': reuse.UNAVAILABLE_REASON})
        self.assertEqual(env, {'RUSTC_WRAPPER': '/unretained/wrapper', 'CARGO_BUILD_JOBS': '7'})
        output = Path(self.temp.name) / 'never-contract.json'
        args = ['apple_merge_reuse.py', 'seal', '--summaries', str(self.temp.name),
                '--chain', str(Path(self.temp.name) / 'chain.json'), '--output', str(output)]
        with mock.patch.object(reuse.sys, 'argv', args), \
                mock.patch.object(reuse, 'seal', side_effect=reuse.UnsupportedCargoInputs('no eligibility')), \
                mock.patch.object(reuse, 'output_fields') as fields:
            reuse.main()
            fields.assert_called_once_with(eligible='false', reason=reuse.UNAVAILABLE_REASON)
            self.assertFalse(output.exists())
        # A corrupt original is never downgraded to a harmless qualification miss.
        with mock.patch.object(reuse.sys, 'argv', args), \
                mock.patch.object(reuse, 'seal', side_effect=RuntimeError('tampered original')), \
                mock.patch.object(reuse, 'output_fields') as fields, self.assertRaisesRegex(RuntimeError, 'tampered'):
            reuse.main()
        fields.assert_not_called()

    def install_qualification_case(self, fx, phases=(), *, execution=False, corrupt=None):
        if execution:
            fx.replace_rust_execution({'unavailable': reuse.UNAVAILABLE_REASON})
        component, carrier = copy.deepcopy(fx.component_files), copy.deepcopy(fx.carrier_files)
        for phase in phases:
            original = fx.snapshots[phase]
            row = {key: original[key] for key in ('context', 'phase', 'slot', 'workflowSha', 'source', *reuse.FALSE_CLAIMS)}
            row.update(schema=reuse.UNAVAILABLE_SCHEMA, reason=reuse.UNAVAILABLE_REASON)
            members = carrier if phase.startswith('carrier-') else component
            prefix = 'device-arm64/' if phase.startswith('carrier-') else ''
            members[prefix + 'execution-' + phase + '.json'] = raw_json(reuse.signed(row))
        if corrupt:
            corrupt(component, carrier)
        fx.set_artifact('component-original', {'apple-original-evidence.zip':
            original_zip(component, fx.ctx, fx.root, None)})
        fx.set_artifact('carrier-original-device-arm64', {'apple-original-device-arm64-evidence.zip':
            original_zip(carrier, fx.ctx, fx.root, 'device-arm64')})
        fx.refresh_list()

    def seal_qualification_case(self, fx, *, local=None):
        run = fx.routes['/actions/runs/100/attempts/2']
        job = fx.routes['/actions/runs/100/attempts/2/jobs?per_page=100']['jobs'][1]
        run.update(status='in_progress', conclusion=None); job.update(status='in_progress', conclusion=None)
        chain = fx.folder / 'local-chain.json'
        chain.write_bytes(fx.archive_files['chain']['apple-chain.json'])
        summaries = fx.folder / 'fixture-summaries'
        if local:
            local(chain, summaries)
        output, ghout = fx.folder / 'new-contract.json', fx.folder / 'ghout'
        env = {'POLARIS_APPLE_SCOPE': 'device-only', 'GITHUB_JOB': 'unsigned-chain',
               'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_OUTPUT': str(ghout),
               'GITHUB_WORKFLOW_REF': reuse.REPOSITORY + '/' + reuse.WORKFLOWS[0] + '@refs/pull/15/merge'}
        argv = ['apple_merge_reuse.py', 'seal', '--summaries', str(summaries), '--chain', str(chain), '--output', str(output)]
        try:
            with mock.patch.dict(os.environ, env), mock.patch.object(reuse.sys, 'argv', argv), \
                    mock.patch.object(reuse, 'current_context', return_value=fx.ctx), \
                    mock.patch.object(reuse, 'source_bindings', return_value=fx.source), \
                    mock.patch.object(reuse, 'GitHub', return_value=fx.api):
                reuse.main()
        finally:
            run.update(status='completed', conclusion='success'); job.update(status='completed', conclusion='success')
        return output, ghout.read_text()

    def test_unavailable_qualification_waits_for_every_archived_and_local_integrity_check(self):
        names = ['rust-change', 'rust-delete', 'ipa-change', 'later-snapshot', 'local-chain',
                 'local-manifest', 'execution-unavailable-rust', 'local-transport',
                 'local-carrier-wrapper', 'local-carrier-receipt', 'local-rust-receipt']
        for kind in names:
            with tempfile.TemporaryDirectory() as folder, self.subTest(kind=kind):
                fx = Fixture(folder)
                def corrupt(component, carrier):
                    if kind in ('rust-change', 'execution-unavailable-rust'):
                        carrier['device-arm64/original-linked-rust-archive.a'] = b'corrupted original'
                    elif kind == 'rust-delete':
                        del carrier['device-arm64/original-linked-rust-archive.a']
                    elif kind == 'ipa-change':
                        carrier['device-arm64/polaris-box-' + fx.source['contract']['version'] + '-app-link-probe.ipa'] = b'corrupted IPA'
                    elif kind == 'later-snapshot':
                        carrier['device-arm64/execution-carrier-after.json'] = b'{"invalid":true}\n'
                self.install_qualification_case(fx, () if kind == 'execution-unavailable-rust' else ('frontend-before',),
                                                execution=kind == 'execution-unavailable-rust', corrupt=corrupt)
                def local(chain, summaries):
                    if kind == 'local-chain': chain.write_bytes(b'corrupted current chain')
                    paths = {'local-manifest': 'component/component-handoff.json',
                             'local-transport': 'component/component-transport.json',
                             'local-carrier-wrapper': 'device-arm64/carrier-device-arm64.json',
                             'local-carrier-receipt': 'device-arm64/summary-app-carrier.json',
                             'local-rust-receipt': 'device-arm64/summary-rust-receipt.json'}
                    if kind in paths: (summaries / paths[kind]).write_bytes(b'corrupted current input')
                with self.assertRaises((RuntimeError, KeyError)) as failure:
                    self.seal_qualification_case(fx, local=local)
                self.assertNotIsInstance(failure.exception, reuse.UnsupportedCargoInputs)
                self.assertFalse((fx.folder / 'new-contract.json').exists())
                self.assertFalse((fx.folder / 'ghout').exists())
                with self.assertRaises((RuntimeError, KeyError)):
                    fx.resolve()

    def test_unavailable_at_each_phase_all_phases_or_actual_execution_keeps_full_build(self):
        cases = [((phase,), False) for phase in reuse.PHASES] + [(reuse.PHASES, True), ((), True)]
        for phases, execution in cases:
            with tempfile.TemporaryDirectory() as folder, self.subTest(phases=phases, execution=execution):
                fx = Fixture(folder)
                self.install_qualification_case(fx, phases, execution=execution)
                output, fields = self.seal_qualification_case(fx)
                self.assertFalse(output.exists())
                self.assertIn('eligible=false\n', fields)
                with tempfile.TemporaryDirectory() as work:
                    partial = reuse.read_evidence(fx.api, fx.artifacts, fx.ctx, fx.source, work)
                forged = copy.deepcopy(fx.contract); forged.pop('fingerprint')
                forged.update(evidence=partial, artifacts={role: reuse.artifact_reference(fx.artifacts[role])
                                                          for role in reuse.ARTIFACT_ROLES})
                with self.assertRaisesRegex(RuntimeError, 'complete eligible'):
                    reuse.validate_execution_contract(reuse.signed(forged), fx.ctx, fx.source,
                                                      fx.provider, partial, fx.artifacts)
                with self.assertRaises(RuntimeError):
                    fx.resolve()

    def test_unavailable_does_not_hide_remaining_snapshot_execution_or_provider_binding(self):
        cases = ['later-fingerprint', 'later-shape', 'later-source', 'later-context', 'later-workflow',
                 'later-tool', 'later-sdk', 'later-profile', 'actual-profile', 'actual-shape',
                 'all-markers-rust', 'all-markers-local', 'provider-workflow']
        for kind in cases:
            with tempfile.TemporaryDirectory() as folder, self.subTest(kind=kind):
                fx = Fixture(folder)
                if kind.startswith('actual-'):
                    value = reuse.safe_json(fx.carrier_files['device-arm64/rust-aarch64-apple-ios.json'])['executionInputs']
                    if kind == 'actual-profile': value['overrides']['CARGO_PROFILE_RELEASE_OPT_LEVEL'] = reuse.pin_bytes(b'0')
                    else: value = {'unavailable': reuse.UNAVAILABLE_REASON, 'unbound': True}
                    fx.replace_rust_execution(value)
                def corrupt(component, carrier):
                    if kind.startswith('later-'):
                        path = 'device-arm64/execution-carrier-after.json'
                        row = reuse.safe_json(carrier[path]); row.pop('fingerprint')
                        if kind == 'later-fingerprint': row['fingerprint'] = '0' * 64
                        if kind == 'later-shape': row['unexpected'] = True
                        if kind == 'later-source': row['source']['tree'] = '0' * 40
                        if kind == 'later-context': row['context']['attempt'] = '1'
                        if kind == 'later-workflow': row['workflowSha'] = '0' * 40
                        if kind == 'later-tool': row['tools']['cargo']['sha256'] = '0' * 64
                        if kind == 'later-sdk': row['sdk']['iphoneos']['settings']['sha256'] = '0' * 64
                        if kind == 'later-profile': row['overrides']['CARGO_PROFILE_RELEASE_OPT_LEVEL'] = reuse.pin_bytes(b'0')
                        carrier[path] = raw_json(row if kind == 'later-fingerprint' else reuse.signed(row))
                    if kind == 'all-markers-rust': carrier['device-arm64/original-linked-rust-archive.a'] = b'bad'
                    if kind == 'provider-workflow':
                        for phase in reuse.PHASES:
                            members = carrier if phase.startswith('carrier-') else component
                            path = ('device-arm64/' if phase.startswith('carrier-') else '') + 'execution-' + phase + '.json'
                            row = reuse.safe_json(members[path]); row.pop('fingerprint'); row['workflowSha'] = '0' * 40
                            members[path] = raw_json(reuse.signed(row))
                phases = reuse.PHASES if kind.startswith('all-markers') or kind == 'provider-workflow' else ('frontend-before',)
                self.install_qualification_case(fx, phases, corrupt=corrupt)
                local = (lambda chain, summaries: chain.write_bytes(b'bad')) if kind == 'all-markers-local' else None
                with self.assertRaises((RuntimeError, KeyError)) as failure:
                    self.seal_qualification_case(fx, local=local)
                self.assertNotIsInstance(failure.exception, reuse.UnsupportedCargoInputs)
                self.assertFalse((fx.folder / 'ghout').exists())
                self.assertFalse((fx.folder / 'new-contract.json').exists())

    def test_merge_tree_parents_and_authoritative_pr_negatives(self):
        route = '/git/commits/' + self.fx.merge
        mutations = [
            (route, lambda row: row['tree'].update(sha='f' * 40)),
            (route, lambda row: row.update(parents=[{'sha': self.fx.head}])),
            (route, lambda row: row['parents'].reverse()),
            ('/git/commits/' + self.fx.head, lambda row: row['tree'].update(sha='f' * 40)),
            ('/commits/' + self.fx.merge + '/pulls?per_page=100', lambda rows: rows[0]['head']['repo'].update(id=99)),
            ('/pulls/15', lambda row: row.update(merged=False)),
            ('/pulls/15', lambda row: row.update(merge_commit_sha='f' * 40)),
            ('/pulls/15', lambda row: row['head'].update(sha='f' * 40)),
            ('/pulls/15', lambda row: row['head'].update(ref='different-branch')),
            ('/pulls/15', lambda row: row['base'].update(ref='release')),
            ('/pulls/15', lambda row: row['base'].update(sha='f' * 40)),
            ('/pulls/15', lambda row: row['base']['repo'].update(id=99))]
        for route, mutate in mutations:
            original = copy.deepcopy(self.fx.routes[route])
            mutate(self.fx.routes[route])
            with self.subTest(route=route), self.assertRaises(RuntimeError):
                self.fx.resolve()
            self.fx.routes[route] = original

    def test_workflow_output_accepts_contract_hash_key_and_rejects_injection_before_writing(self):
        output = Path(self.temp.name) / 'workflow-output'
        with mock.patch.dict(os.environ, {'GITHUB_OUTPUT': str(output)}):
            reuse.output_fields(reused='true', receipt_sha256='f' * 64, reason='verified-exact-tree-producer')
            expected = ('reused=true\nreceipt_sha256=' + 'f' * 64 +
                        '\nreason=verified-exact-tree-producer\n')
            self.assertEqual(output.read_text(), expected)
            for fields in [{'reused': 'true', 'unexpected': 'value'},
                           {'reused': 'true', 'receipt_sha256\nreused': 'false'},
                           {'reused': 'true', 'reason': 'good\nreused=true'},
                           {'reused': 'true', 'reason': 'good\rreused=true'}]:
                with self.subTest(fields=fields), self.assertRaisesRegex(RuntimeError, 'unsafe workflow output'):
                    reuse.output_fields(**fields)
                self.assertEqual(output.read_text(), expected)
            reuse.output_fields(eligible='false', reason=reuse.UNAVAILABLE_REASON)
            self.assertEqual(output.read_text(), expected + 'eligible=false\nreason=' + reuse.UNAVAILABLE_REASON + '\n')

    def test_exact_head_fast_forward_identity_negatives_fall_back_without_receipt(self):
        with tempfile.TemporaryDirectory() as folder:
            fx = Fixture(folder); fx.exact_head_merge()
            commit_route = '/git/commits/' + fx.head
            list_route = '/commits/' + fx.head + '/pulls?per_page=100'
            mutations = [
                (commit_route, lambda row: row.update(sha='f' * 40)),
                (commit_route, lambda row: row['tree'].update(sha='f' * 40)),
                (commit_route, lambda row: row.update(parents=[])),
                (commit_route, lambda row: row.update(parents=[{'sha': fx.head}])),
                (commit_route, lambda row: row.update(parents=[{'sha': '9' * 40}] * 2)),
                (commit_route, lambda row: row.update(parents=[{'sha': '9' * 40}] * 3)),
                (commit_route, lambda row: row.update(parents=[{'sha': 'bad'}])),
                (list_route, lambda rows: rows.clear()),
                (list_route, lambda rows: rows.append(copy.deepcopy(rows[0]))),
                (list_route, lambda rows: rows.extend(copy.deepcopy(rows) * 99)),
                (list_route, lambda rows: rows[0].update(merged_at=None)),
                (list_route, lambda rows: rows[0].update(merge_commit_sha='f' * 40)),
                (list_route, lambda rows: rows[0]['head'].update(sha='9' * 40)),
                (list_route, lambda rows: rows[0]['head']['repo'].update(id=99)),
                (list_route, lambda rows: rows[0]['base']['repo'].update(id=99)),
                (list_route, lambda rows: rows[0]['base'].update(ref='release')),
                ('/pulls/15', lambda row: row.update(merged=False)),
                ('/pulls/15', lambda row: row.update(number=16)),
                ('/pulls/15', lambda row: row.update(merge_commit_sha='f' * 40)),
                ('/pulls/15', lambda row: row['head'].update(sha='f' * 40)),
                ('/pulls/15', lambda row: row['head'].update(ref='other')),
                ('/pulls/15', lambda row: row['head']['repo'].update(id=99)),
                ('/pulls/15', lambda row: row['base']['repo'].update(id=99)),
                ('/pulls/15', lambda row: row['base'].update(ref='release')),
                ('/pulls/15', lambda row: row['base'].update(sha='f' * 40))]
            env = {'POLARIS_APPLE_ALLOW_MERGE_REUSE': 'true', 'GITHUB_EVENT_NAME': 'push',
                   'GITHUB_REF': 'refs/heads/main', 'POLARIS_APPLE_SCOPE': 'device-only',
                   'POLARIS_APP_CANDIDATE': fx.head, 'GITHUB_SHA': fx.head}
            for index, (route, mutate) in enumerate(mutations):
                before = copy.deepcopy(fx.routes[route]); mutate(fx.routes[route])
                with self.subTest(exact_head=True, mutation=index):
                    with self.assertRaises(RuntimeError): fx.resolve()
                    output = Path(folder) / ('output-' + str(index))
                    receipt = Path(folder) / ('result-' + str(index))
                    with mock.patch.dict(os.environ, {**env, 'GITHUB_OUTPUT': str(output)}), \
                            mock.patch.object(reuse, 'current_context', return_value=fx.current), \
                            mock.patch.object(reuse, 'source_bindings', return_value=fx.source), \
                            mock.patch.object(reuse, 'GitHub', return_value=fx.api):
                        reuse.resolve(receipt)
                    self.assertEqual(output.read_text(),
                        'reused=false\nreason=evidence-unavailable-or-incompatible\n')
                    self.assertFalse(receipt.exists())
                fx.routes[route] = before
            for key in ('candidate', 'tree'):
                before = fx.source[key]; fx.source[key] = 'f' * 40
                with self.subTest(exact_head=True, source=key), self.assertRaises(RuntimeError): fx.resolve()
                fx.source[key] = before

    @both_merge_forms
    def test_run_attempt_workflow_jobs_must_be_exact_success(self):
        routes = self.fx.routes
        run_path = '/actions/runs/100/attempts/2'
        job_path = '/actions/runs/100/attempts/2/jobs?per_page=100'
        for key, value in [('id', 101), ('run_attempt', 1), ('head_sha', 'f' * 40),
                           ('event', 'push'), ('status', 'in_progress'), ('conclusion', 'failure'),
                           ('path', '.github/workflows/other.yml'), ('referenced_workflows', []), ('head_branch', 'other')]:
            before = copy.deepcopy(routes[run_path])
            routes[run_path][key] = value
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.fx.resolve()
            routes[run_path] = before
        for job in range(2):
            for key, value in [('conclusion', 'skipped'), ('conclusion', 'cancelled'), ('run_attempt', 1),
                               ('run_id', 99), ('head_sha', 'f' * 40), ('status', 'in_progress')]:
                before = copy.deepcopy(routes[job_path])
                routes[job_path]['jobs'][job][key] = value
                with self.subTest(job=job, key=key), self.assertRaises(RuntimeError):
                    self.fx.resolve()
                routes[job_path] = before
        routes['/git/commits/' + self.fx.workflow]['tree']['sha'] = 'f' * 40
        with self.assertRaisesRegex(RuntimeError, 'workflow tree'):
            self.fx.resolve()

    @both_merge_forms
    def test_artifact_identity_expiry_missing_duplicates_and_hashes_reject(self):
        for role in self.fx.artifacts:
            for key, value in [('expired', True), ('digest', 'sha256:' + '0' * 64), ('size_in_bytes', 0)]:
                old = self.fx.artifacts[role][key]
                self.fx.artifacts[role][key] = value
                self.fx.refresh_list()
                with self.subTest(role=role, key=key), self.assertRaises(RuntimeError):
                    self.fx.resolve()
                self.fx.artifacts[role][key] = old
        self.fx.refresh_list()
        route = '/actions/runs/100/artifacts?per_page=100'
        before = copy.deepcopy(self.fx.routes[route])
        self.fx.routes[route]['artifacts'].pop()
        with self.assertRaises(RuntimeError):
            self.fx.resolve()
        self.fx.routes[route] = before
        self.fx.routes[route]['artifacts'].append(copy.deepcopy(before['artifacts'][0]))
        self.fx.routes[route]['total_count'] += 1
        with self.assertRaisesRegex(RuntimeError, 'duplicate'):
            self.fx.resolve()

    @both_merge_forms
    def test_missing_sidecar_never_downloads_large_originals(self):
        del self.fx.artifacts['execution-contract']
        self.fx.refresh_list()
        self.fx.api.downloads.clear()
        with self.assertRaisesRegex(RuntimeError, 'missing/duplicate'):
            self.fx.resolve()
        self.assertEqual(self.fx.api.downloads, [])

    @both_merge_forms
    def test_old_reused_and_relabelled_contracts_reject_even_with_new_fingerprint(self):
        for mutate in [lambda row: row.update(schema='old-v0'), lambda row: row.update(origin='verified-reuse'),
                       lambda row: row['context'].update(attempt='1'),
                       lambda row: row['evidence']['source'].update(tree='f' * 40),
                       lambda row: row['evidence'].update(toolContract='f' * 64),
                       lambda row: row['evidence'].update(inputContract='f' * 64),
                       lambda row: row['provider'].update(workflowSha='f' * 40),
                       lambda row: row.update(devicePassed=True),
                       lambda row: row.update(publicationEligible=True),
                       lambda row: row.update(previousFullEvidence={'huge': 'never permitted'})]:
            row = copy.deepcopy(self.fx.contract)
            mutate(row)
            row.pop('fingerprint')
            self.fx.set_artifact('execution-contract', {'apple-execution-contract.json': raw_json(reuse.signed(row))})
            self.fx.refresh_list()
            with self.subTest(contract=row), self.assertRaises(RuntimeError):
                self.fx.resolve()

    def test_snapshot_tools_sdk_scope_and_hashes_reject(self):
        baseline = self.fx.snapshots['frontend-before']
        for mutate in [lambda row: row['tools']['node'].update(version='v25.0.0'),
                       lambda row: row['tools']['pnpm'].update(version='11.23.0'),
                       lambda row: row['tools']['go'].update(version='go version go1.25.5 darwin/arm64'),
                       lambda row: row['tools']['xcodebuild'].update(version='Xcode 27.0\nBuild version old'),
                       lambda row: row['sdk']['iphoneos'].update(version='27.0'),
                       lambda row: row['tools'].pop('swiftc'),
                       lambda row: row['context'].update(candidate=self.fx.merge),
                       lambda row: row.update(devicePassed=True),
                       lambda row: row.update(slot='sim-arm64')]:
            row = copy.deepcopy(baseline)
            mutate(row)
            row.pop('fingerprint')
            with self.subTest(row=row), self.assertRaises(RuntimeError):
                reuse.validate_snapshot(reuse.signed(row), 'frontend-before', self.fx.ctx,
                                        reuse.source_summary(self.fx.source), self.fx.source['contract'])
        row = copy.deepcopy(baseline)
        row['tools']['clang']['sha256'] = 'f' * 64
        with self.assertRaisesRegex(RuntimeError, 'fingerprint'):
            reuse.validate_snapshot(row, 'frontend-before', self.fx.ctx,
                                    reuse.source_summary(self.fx.source), self.fx.source['contract'])
        row.pop('fingerprint')
        changed = reuse.signed(row)
        with self.assertRaisesRegex(RuntimeError, 'changed during'):
            reuse.snapshot_pair(baseline, changed)

    @both_merge_forms
    def test_original_bytes_missing_snapshots_and_relabelled_paths_reject(self):
        files = copy.deepcopy(self.fx.carrier_files)
        snapshot = files.pop('device-arm64/execution-carrier-before.json')
        self.fx.set_artifact('carrier-original-device-arm64', {'apple-original-device-arm64-evidence.zip':
                             original_zip(files, self.fx.ctx, self.fx.root, 'device-arm64')})
        self.fx.refresh_list()
        with self.assertRaises((KeyError, RuntimeError)):
            self.fx.resolve()
        files['device-arm64/execution-carrier-before.json'] = snapshot
        files['device-arm64/original-linked-rust-archive.a'] = b'different archive'
        self.fx.set_artifact('carrier-original-device-arm64', {'apple-original-device-arm64-evidence.zip':
                             original_zip(files, self.fx.ctx, self.fx.root, 'device-arm64')})
        self.fx.refresh_list()
        with self.assertRaisesRegex(RuntimeError, 'linked Rust'):
            self.fx.resolve()
        self.fx.set_artifact('carrier-original-device-arm64', {'apple-original-device-arm64-evidence.zip':
                             original_zip(self.fx.carrier_files, self.fx.ctx, '/relabelled', 'device-arm64')})
        self.fx.refresh_list()
        with self.assertRaisesRegex(RuntimeError, 'path identity'):
            self.fx.resolve()

    def test_zip_duplicate_traversal_special_and_budget_controls(self):
        for name in ('../outside', '/outside', 'a/../b', 'a\\b', 'C:/outside', 'a//b'):
            with self.subTest(name=name), zipfile.ZipFile(io.BytesIO(zip_bytes({name: b'x'}))) as archive:
                with self.assertRaisesRegex(RuntimeError, 'unsafe'):
                    reuse.zip_inventory(archive)
        with zipfile.ZipFile(io.BytesIO(zip_bytes({'safe': b'abc'}))) as archive:
            archive.filelist.append(archive.filelist[0])
            with self.assertRaisesRegex(RuntimeError, 'duplicates'):
                reuse.zip_inventory(archive)
        with zipfile.ZipFile(io.BytesIO(zip_bytes({'safe': b'abc'}))) as archive:
            archive.filelist[0].external_attr = stat.S_IFIFO << 16
            with self.assertRaisesRegex(RuntimeError, 'special'):
                reuse.zip_inventory(archive)
        with zipfile.ZipFile(io.BytesIO(zip_bytes({'safe': b'abc'}))) as archive:
            with self.assertRaisesRegex(RuntimeError, 'budget'):
                reuse.member_pin(archive, 'safe', 2)
            archive.filelist[0].file_size = reuse.MAX_EXPANDED + 1
            with self.assertRaisesRegex(RuntimeError, 'budget'):
                reuse.zip_inventory(archive)

    def test_json_duplicate_nonfinite_and_reference_budget_controls(self):
        for raw in (b'{"x":1,"x":2}', b'{"x":NaN}', b'{"x":Infinity}'):
            with self.subTest(raw=raw), self.assertRaises((RuntimeError, ValueError)):
                reuse.safe_json(raw)
        with self.assertRaisesRegex(RuntimeError, 'budget'):
            reuse.write_new(Path(self.temp.name) / 'too-large.json', {'data': 'x' * reuse.MAX_JSON})

    def test_resolver_failures_emit_only_rebuild_and_ineligible_never_reads_api(self):
        output = Path(self.temp.name) / 'workflow-output'
        result_path = Path(self.temp.name) / 'result.json'
        env = {'POLARIS_APPLE_ALLOW_MERGE_REUSE': 'true', 'GITHUB_EVENT_NAME': 'push',
               'GITHUB_REF': 'refs/heads/main', 'POLARIS_APPLE_SCOPE': 'device-only',
               'POLARIS_APP_CANDIDATE': self.fx.merge, 'GITHUB_SHA': self.fx.merge,
               'GITHUB_OUTPUT': str(output)}
        for failure in (RuntimeError('missing'), KeyError('wrong attempt'), OSError('API unavailable')):
            output.write_text('')
            with mock.patch.dict(os.environ, env), mock.patch.object(reuse, 'current_context', return_value=self.fx.current), \
                    mock.patch.object(reuse, 'source_bindings', return_value=self.fx.source), \
                    mock.patch.object(reuse, 'GitHub', return_value=self.fx.api), \
                    mock.patch.object(reuse, 'resolve_evidence', side_effect=failure):
                reuse.resolve(result_path)
            self.assertIn('reused=false\n', output.read_text())
            self.assertNotIn('reused=true', output.read_text())
            self.assertFalse(result_path.exists())
        for key, value in [('POLARIS_APPLE_ALLOW_MERGE_REUSE', 'false'), ('GITHUB_EVENT_NAME', 'pull_request'),
                           ('GITHUB_REF', 'refs/tags/v1'), ('POLARIS_APPLE_SCOPE', 'device-and-simulator'),
                           ('GITHUB_SHA', 'f' * 40)]:
            output.write_text('')
            with mock.patch.dict(os.environ, {**env, key: value}), mock.patch.object(reuse, 'GitHub') as api:
                reuse.resolve(result_path)
                api.assert_not_called()
            self.assertIn('reused=false\n', output.read_text())

    @both_merge_forms
    def test_current_result_requires_exact_hash_context_source_and_no_recursive_payload(self):
        row = self.fx.resolve()
        path = Path(self.temp.name) / 'reuse-result.json'
        env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': 'refs/heads/main', 'POLARIS_APPLE_SCOPE': 'device-only',
               'POLARIS_APPLE_ALLOW_MERGE_REUSE': 'true', 'POLARIS_APP_CANDIDATE': self.fx.merge,
               'GITHUB_SHA': self.fx.merge}
        def check(value):
            path.write_bytes(raw_json(value))
            with mock.patch.dict(os.environ, env), mock.patch.object(reuse, 'current_context', return_value=self.fx.current), \
                    mock.patch.object(reuse, 'source_bindings', return_value=self.fx.source):
                reuse.check_result(path, file_pin(path)['sha256'])
        check(row)
        for mutate in [lambda value: value['context'].update(attempt='2'),
                       lambda value: value['source'].update(tree='f' * 40),
                       lambda value: value.update(signatureTrustValidated=True),
                       lambda value: value.update(previousEvidence=self.fx.contract),
                       lambda value: value['proof'].update(parents=[]),
                       lambda value: value['proof'].update(parents=[value['proof']['mergeCommit']]),
                       lambda value: value['proof'].update(parents=['bad']),
                       lambda value: value['proof'].update(parents=['9' * 40] * 2),
                       lambda value: value['proof'].update(parents=['9' * 40] * 3),
                       lambda value: value['proof'].update(producerHead='f' * 40),
                       lambda value: value['proof'].update(mergeCommit='f' * 40),
                       lambda value: value['proof'].update(pullRequest=True),
                       lambda value: value['proof'].update(merged=False),
                       lambda value: value['proof'].update(headBranch=''),
                       lambda value: value['proof'].update(previousFullEvidence=self.fx.contract)]:
            changed = copy.deepcopy(row)
            mutate(changed)
            changed.pop('fingerprint')
            with self.assertRaises(RuntimeError):
                check(reuse.signed(changed))
        path.write_bytes(raw_json(row))
        with mock.patch.dict(os.environ, env), self.assertRaisesRegex(RuntimeError, 'bytes differ'):
            reuse.check_result(path, '0' * 64)

    def test_snapshot_collects_actual_binary_and_sdk_bytes_before_and_after(self):
        self.collect_snapshot_fixture()

    def collect_snapshot_fixture(self, *, unavailable=False, before_each=None, after_each=None):
        root = (Path(self.temp.name) / 'snapshot-root').resolve()
        root.mkdir(mode=0o700)
        self.assertFalse((root / 'device-arm64').exists())
        binaries = root / 'tools'
        binaries.mkdir()
        for name in self.fx.tools:
            (binaries / name).write_bytes(('binary-' + name).encode())
        for sdk in ('iphoneos', 'iphonesimulator'):
            (root / sdk).mkdir()
            (root / sdk / 'SDKSettings.json').write_text(sdk)
        def command(args):
            if args[:2] == ['rustup', 'which']:
                return str(binaries / args[2])
            if args[:2] == ['xcrun', '--find']:
                return str(binaries / args[2])
            if args[:2] == ['xcrun', '--sdk']:
                return {'--find': str(binaries / 'clang'), '--show-sdk-path': str(root / args[2]),
                        '--show-sdk-version': '27.1'}[args[3]]
            return self.fx.tools[Path(args[0]).name]['version']
        prepared = {'paths': {name: str(binaries / name) for name in ('go', 'gomobile', 'gobind')}}
        env = {'POLARIS_APPLE_SCOPE': 'device-only', 'GITHUB_WORKFLOW_SHA': self.fx.workflow}
        with mock.patch.dict(os.environ, env, clear=True), \
                mock.patch('ci_apple.location', return_value=root), \
                mock.patch('ci_apple.selected_apple_tools'), \
                mock.patch.object(reuse, 'read_receipt', return_value=prepared), \
                mock.patch.object(reuse, 'command', side_effect=command), \
                mock.patch.object(reuse.shutil, 'which', side_effect=lambda name: str(binaries / name)), \
                mock.patch.object(reuse, 'current_context', return_value=self.fx.ctx), \
                mock.patch.object(reuse, 'source_bindings', return_value=self.fx.source):
            for phase in reuse.PHASES:
                if before_each: before_each(root, phase)
                reuse.snapshot(phase, 'device-arm64' if phase.startswith('carrier-') else None)
                if after_each: after_each(root, phase)
        for phase in reuse.PHASES:
            path = root / ('device-arm64' if phase.startswith('carrier-') else '') / ('execution-' + phase + '.json')
            row = reuse.safe_json(path.read_bytes())
            self.assertEqual(row['context'], self.fx.ctx)
            self.assertEqual(row['schema'], reuse.UNAVAILABLE_SCHEMA if unavailable else reuse.SNAPSHOT_SCHEMA)
            if unavailable:
                self.assertEqual(row['reason'], reuse.UNAVAILABLE_REASON)
                with self.assertRaises(reuse.UnsupportedCargoInputs):
                    reuse.validate_snapshot(row, phase, self.fx.ctx,
                        reuse.source_summary(self.fx.source), self.fx.source['contract'])
                continue
            self.assertEqual(row['tools']['go']['sha256'], file_pin(binaries / 'go')['sha256'])
            self.assertEqual(row['sdk']['iphoneos']['settings'], file_pin(root / 'iphoneos/SDKSettings.json'))
            self.assertEqual('protoc' in row['tools'], phase.startswith('carrier-'))
        return root

    def test_snapshot_creates_private_slot_before_real_carrier_admission(self):
        import ci_apple
        from producer import private_dir
        for unavailable in (False, True):
            for mask in (0o000, 0o002, 0o022, 0o077):
                for existing in (False, True):
                    with self.subTest(unavailable=unavailable, umask=oct(mask), existing=existing):
                        case = AppleMergeReuseControls('test_snapshot_collects_actual_binary_and_sdk_bytes_before_and_after')
                        case.setUp(); previous = os.umask(mask); reached = []
                        def before(root, phase):
                            if phase == 'carrier-before' and existing:
                                private_dir(root / 'device-arm64')
                                (root / 'device-arm64/retained-input').write_bytes(b'unchanged')
                        def after(root, phase):
                            if phase != 'carrier-before': return
                            slot = root / 'device-arm64'; info = slot.stat()
                            self.assertEqual(stat.S_IMODE(info.st_mode), 0o700)
                            self.assertEqual(info.st_uid, os.getuid())
                            self.assertEqual(private_dir(slot), slot)
                            snapshot = (slot / 'execution-carrier-before.json').read_bytes()
                            (root / 'component-handoff.json').write_bytes(b'fixture presence')
                            imported = {'completed': True, 'context': case.fx.ctx}
                            with (root / 'fixture-native-progress.log').open('w') as progress, \
                                    mock.patch.object(ci_apple.sys, 'stdout', progress), \
                                    mock.patch.object(ci_apple, 'read_receipt', side_effect=lambda path:
                                    imported if Path(path).name == 'import-observation.json' else {'paths': {}}), \
                                    mock.patch('apple_handoff.context', return_value=case.fx.ctx), \
                                    mock.patch('apple_handoff.carrier_summary') as summary, \
                                    mock.patch.object(ci_apple, 'phase_timing', return_value=contextlib.nullcontext()), \
                                    mock.patch.object(ci_apple, 'run') as launch:
                                # Actual carrier() and private_dir; only native launch/summary are mocked.
                                ci_apple.carrier('device-arm64')
                            launch.assert_called_once(); summary.assert_called_once_with(root, 'device-arm64')
                            args = launch.call_args.args[0]
                            self.assertEqual(args[args.index('--work') + 1], slot)
                            self.assertEqual(slot.stat().st_ino, info.st_ino)
                            self.assertEqual(stat.S_IMODE(slot.stat().st_mode), 0o700)
                            self.assertEqual((slot / 'execution-carrier-before.json').read_bytes(), snapshot)
                            reached.append(True)
                        try:
                            parser = mock.patch.dict(reuse.sys.modules, {'tomllib': None}) if unavailable else contextlib.nullcontext()
                            with parser:
                                root = case.collect_snapshot_fixture(unavailable=unavailable, before_each=before, after_each=after)
                            self.assertEqual(reached, [True])
                            self.assertEqual(stat.S_IMODE((root / 'device-arm64').stat().st_mode), 0o700)
                            if existing: self.assertEqual((root / 'device-arm64/retained-input').read_bytes(), b'unchanged')
                        finally:
                            os.umask(previous); case.doCleanups()

    def test_snapshot_refuses_unsafe_existing_slot_without_repair_or_overwrite(self):
        kinds = ('mode-755', 'mode-775', 'file', 'symlink', 'dangling', 'parent-symlink',
                 'wrong-owner', 'existing-receipt', 'mode-changed-after')
        for unavailable in (False, True):
            for kind in kinds:
                with self.subTest(unavailable=unavailable, kind=kind):
                    case = AppleMergeReuseControls('test_snapshot_collects_actual_binary_and_sdk_bytes_before_and_after')
                    case.setUp(); observed = {}; ownership = None
                    def before(root, phase):
                        nonlocal ownership
                        slot = root / 'device-arm64'
                        if phase == 'carrier-before':
                            if kind.startswith('mode-'):
                                mode = {'mode-755': 0o755, 'mode-775': 0o775, 'mode-changed-after': 0o700}[kind]
                                slot.mkdir(mode=0o700); slot.chmod(mode); observed['mode'] = mode
                            elif kind == 'file': slot.write_bytes(b'retained file')
                            elif kind in ('symlink', 'dangling'):
                                target = root / 'redirected-slot'
                                if kind == 'symlink': target.mkdir(mode=0o700)
                                slot.symlink_to(target); observed['target'] = target
                            elif kind == 'parent-symlink':
                                real = root.with_name('original-snapshot-root'); root.rename(real); root.symlink_to(real)
                                observed['target'] = real / 'device-arm64'
                            elif kind == 'wrong-owner':
                                slot.mkdir(mode=0o700)
                                ownership = mock.patch('producer.os.getuid', return_value=os.getuid() + 1)
                                ownership.start()
                            elif kind == 'existing-receipt':
                                slot.mkdir(mode=0o700); (slot / 'execution-carrier-before.json').write_bytes(b'original receipt')
                        elif phase == 'carrier-after' and kind == 'mode-changed-after':
                            observed['before'] = (slot / 'execution-carrier-before.json').read_bytes()
                            slot.chmod(0o755); observed['mode'] = 0o755
                    try:
                        parser = mock.patch.dict(reuse.sys.modules, {'tomllib': None}) if unavailable else contextlib.nullcontext()
                        with parser, self.assertRaises((RuntimeError, OSError)):
                            case.collect_snapshot_fixture(unavailable=unavailable, before_each=before)
                        root = Path(case.temp.name) / 'snapshot-root'; slot = root / 'device-arm64'
                        if kind.startswith('mode-'):
                            self.assertEqual(stat.S_IMODE(slot.stat().st_mode), observed['mode'])
                        if kind == 'file': self.assertEqual(slot.read_bytes(), b'retained file')
                        elif kind == 'existing-receipt':
                            self.assertEqual((slot / 'execution-carrier-before.json').read_bytes(), b'original receipt')
                        elif kind == 'mode-changed-after':
                            self.assertEqual((slot / 'execution-carrier-before.json').read_bytes(), observed['before'])
                        else: self.assertFalse((slot / 'execution-carrier-before.json').exists())
                        self.assertFalse((slot / 'execution-carrier-after.json').exists())
                        if 'target' in observed:
                            self.assertFalse((observed['target'] / 'execution-carrier-before.json').exists())
                    finally:
                        if ownership: ownership.stop()
                        case.doCleanups()
        # Scope rejection precedes location, directory admission, or receipt writes.
        with mock.patch('ci_apple.location') as location, mock.patch.object(reuse, 'write_new') as write:
            for phase, slot in [('unknown', None), ('carrier-before', 'sim-arm64'), ('frontend-before', 'device-arm64')]:
                with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}), self.assertRaises(RuntimeError):
                    reuse.snapshot(phase, slot)
            location.assert_not_called(); write.assert_not_called()

    def test_full_producer_seal_requires_active_aggregate_and_uploaded_chain_bytes(self):
        jobs = self.fx.routes['/actions/runs/100/attempts/2/jobs?per_page=100']['jobs']
        jobs[1].update(status='in_progress', conclusion=None)
        self.fx.routes['/actions/runs/100/attempts/2'].update(status='in_progress', conclusion=None)
        path = Path(self.temp.name) / 'execution-contract.json'
        chain = Path(self.temp.name) / 'apple-chain.json'
        chain.write_bytes(self.fx.archive_files['chain']['apple-chain.json'])
        env = {'POLARIS_APPLE_SCOPE': 'device-only', 'GITHUB_JOB': 'unsigned-chain',
               'GITHUB_EVENT_NAME': 'pull_request',
               'GITHUB_WORKFLOW_REF': reuse.REPOSITORY + '/' + reuse.WORKFLOWS[0] + '@refs/pull/15/merge'}
        with mock.patch.dict(os.environ, env), \
                mock.patch.object(reuse, 'current_context', return_value=self.fx.ctx), \
                mock.patch.object(reuse, 'source_bindings', return_value=self.fx.source), \
                mock.patch.object(reuse, 'GitHub', return_value=self.fx.api):
            reuse.seal(Path(self.temp.name) / 'fixture-summaries', chain, path)
            self.assertEqual(reuse.safe_json(path.read_bytes()), self.fx.contract)
            chain.write_bytes(b'changed')
            with self.assertRaisesRegex(RuntimeError, 'just-built chain'):
                reuse.seal(Path(self.temp.name) / 'fixture-summaries', chain, path.with_name('bad-contract.json'))
            jobs[1].update(status='completed', conclusion='success')
            with self.assertRaisesRegex(RuntimeError, 'active aggregate'):
                reuse.seal(Path(self.temp.name) / 'fixture-summaries', chain, path.with_name('bad-contract.json'))

    def test_artifact_redirect_never_forwards_token_and_wrong_bytes_fail_closed(self):
        row = {'id': 123, 'size_in_bytes': 3, 'digest': 'sha256:' + reuse.sha(b'zip')}
        for index, url in enumerate(('https://artifacts.blob.core.windows.net/path?sig=provider',
                                     'https://evil.example/path', 'http://artifacts.blob.core.windows.net/path',
                                     'https://user:password@artifacts.blob.core.windows.net/path')):
            folder = Path(self.temp.name) / ('download-' + str(index))
            folder.mkdir()
            api = reuse.GitHub('private-token', folder)
            redirect = urllib.error.HTTPError('https://api.github.com/example', 302, 'redirect', {'Location': url}, None)
            api.opener = mock.Mock()
            api.opener.open.side_effect = [redirect, io.BytesIO(b'zip')]
            if index == 0:
                self.assertEqual(api.artifact(row).read_bytes(), b'zip')
                request = api.opener.open.call_args_list[1].args[0]
                self.assertIsNone(request.get_header('Authorization'))
                self.assertEqual(request.method, 'GET')
            else:
                with self.assertRaisesRegex(RuntimeError, 'admitted provider'):
                    api.artifact(row)
                self.assertEqual(api.opener.open.call_count, 1)
        api = reuse.GitHub('private-token', self.temp.name)
        api.opener = mock.Mock()
        api.opener.open.return_value = io.BytesIO(b'wrong')
        with self.assertRaisesRegex(RuntimeError, 'digest/size'):
            api.artifact(row)

    def test_provider_routes_are_read_only_and_credentials_not_in_payload(self):
        api = reuse.GitHub('private-token', self.temp.name)
        request = api.request('/actions/runs/100/attempts/2')
        self.assertEqual(request.method, 'GET')
        self.assertEqual(request.host, 'api.github.com')
        self.assertIn(reuse.REPOSITORY, request.full_url)
        self.assertNotIn('private-token', request.full_url)
        for route in ('https://evil.example', '//evil.example', '/path\\evil',
                      '/../../other/repository', '/actions/runs/%2e%2e/artifacts',
                      '/actions/artifacts/123/zip?token=unexpected', '/actions/runs/0/attempts/1'):
            with self.assertRaises(RuntimeError):
                api.request(route)
        for ctx, repository_id in [(self.fx.ctx, '123'),
                                   ({**self.fx.ctx, 'repository': 'other/repository'}, str(reuse.REPOSITORY_ID))]:
            with mock.patch('apple_handoff.context', return_value=ctx), \
                    mock.patch.dict(os.environ, {'GITHUB_REPOSITORY_ID': repository_id}), \
                    self.assertRaisesRegex(RuntimeError, 'repository'):
                reuse.current_context()


# Bounded Cargo diagnostics; no Cargo, Xcode or native compiler runs.
class AppleCargoTimingTests(unittest.TestCase):
    triple = 'aarch64-apple-ios'

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.target = self.root / 'cache/rust-target'
        self.report = self.target / 'cargo-timings/cargo-timing.html'
        self.report.parent.mkdir(parents=True)
        (self.root / 'cache').chmod(0o700)
        self.work = self.root / 'device-arm64'
        self.work.mkdir()
        self.output = self.work / ('diagnostic-cargo-timing-' + self.triple + '.html')
        printer = mock.patch('builtins.print')
        self.printed = printer.start()
        self.addCleanup(printer.stop)

    def observe(self):
        return apple_rust.cargo_timing_report(self.target, self.work, self.triple)

    def test_single_report_keeps_exact_bytes_and_ignores_history(self):
        history = self.report.with_name('cargo-timing-old.html')
        history.write_bytes(b'old unrelated history')
        with self.observe():
            self.report.write_bytes(b'<html>actual units</html>')
        self.assertEqual(self.output.read_bytes(), self.report.read_bytes())
        self.assertEqual(list(self.work.iterdir()), [self.output])
        self.assertEqual(history.read_bytes(), b'old unrelated history')
        self.printed.assert_not_called()

    def test_failed_cargo_retains_report_without_replacing_primary_exception(self):
        primary = RuntimeError('original cargo failure')
        with self.assertRaises(RuntimeError) as caught:
            with self.observe():
                self.report.write_bytes(b'<html>partial build</html>')
                raise primary
        self.assertIs(caught.exception, primary)
        self.assertEqual(self.output.read_bytes(), self.report.read_bytes())

    def test_missing_or_unchanged_report_cannot_impersonate_current_slot(self):
        for previous in (None, b'<html>previous slot</html>'):
            with self.subTest(previous=previous):
                if previous is not None:
                    self.report.write_bytes(previous)
                with self.observe():
                    pass
                self.assertFalse(self.output.exists())

    def test_invalid_size_is_diagnostic_only_on_success_and_failure(self):
        for raw in (b'', b'12345'):
            for failed in (False, True):
                with self.subTest(raw=raw, failed=failed), \
                        mock.patch.object(apple_rust, 'CARGO_TIMING_BYTES', 4):
                    self.report.unlink(missing_ok=True)
                    primary = RuntimeError('original failure')
                    try:
                        with self.observe():
                            self.report.write_bytes(raw)
                            if failed:
                                raise primary
                    except RuntimeError as caught:
                        self.assertIs(caught, primary)
                    self.assertFalse(self.output.exists())

    def test_report_file_and_parent_redirects_are_not_copied(self):
        external = self.root / 'outside.html'
        external.write_bytes(b'do not copy')
        with self.observe():
            self.report.symlink_to(external)
        self.assertFalse(self.output.exists())
        self.report.unlink()
        self.report.parent.rmdir()
        external_dir = self.root / 'outside-dir'
        external_dir.mkdir()
        self.report.parent.symlink_to(external_dir, target_is_directory=True)
        with self.observe():
            self.report.write_bytes(b'do not copy parent redirect')
        self.assertFalse(self.output.exists())

    def test_existing_diagnostic_is_never_overwritten(self):
        self.output.write_bytes(b'prior retained diagnostic')
        with self.observe():
            self.report.write_bytes(b'new report')
        self.assertEqual(self.output.read_bytes(), b'prior retained diagnostic')

    def test_exact_bound_allowed_and_copy_failure_preserves_cargo_error(self):
        with mock.patch.object(apple_rust, 'CARGO_TIMING_BYTES', 4), self.observe():
            self.report.write_bytes(b'1234')
        self.assertEqual(self.output.read_bytes(), b'1234')
        primary = RuntimeError('original failure')
        with self.assertRaises(RuntimeError) as caught:
            with self.observe():
                self.report.write_bytes(b'changed report')
                raise primary
        self.assertIs(caught.exception, primary)

    def test_existing_formal_cargo_command_adds_one_timing_flag_without_rerun(self):
        tree = ast.parse(Path(apple_rust.__file__).read_text())
        build = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'build_reserved')
        commands = [node.value for node in ast.walk(build) if isinstance(node, ast.Assign) and
                    any(isinstance(target, ast.Name) and target.id == 'command' for target in node.targets)]
        self.assertEqual(len(commands), 1)
        flags = [item.value for item in commands[0].elts if isinstance(item, ast.Constant)]
        self.assertEqual(flags, ['rustc', '--locked', '--release', '--timings', '--lib', '--package',
                                 'polaris', '--target', '--features', 'tauri/custom-protocol',
                                 '--crate-type', 'staticlib'])
        calls = [node for node in ast.walk(build) if isinstance(node, ast.Call) and
                 isinstance(node.func, ast.Name) and node.func.id == 'run' and node.args and
                 isinstance(node.args[0], ast.Name) and node.args[0].id == 'command']
        self.assertEqual(len(calls), 1)

    def test_originals_zip_selects_one_bounded_diagnostic_without_acceptance(self):
        self.output.write_bytes(b'<html>one report</html>')
        (self.work / 'diagnostic-cargo-timing-history.html').write_bytes(b'history')
        self.report.write_bytes(b'not selected from shared cache')
        with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}, clear=True), \
                mock.patch.object(ci_apple, 'location', return_value=self.root), \
                mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: contextlib.nullcontext()):
            ci_apple.evidence()
        with zipfile.ZipFile(self.root / 'apple-original-evidence.zip') as package:
            html = [name for name in package.namelist() if name.endswith('.html')]
            self.assertEqual(html, [self.output.relative_to(self.root).as_posix()])
            self.assertEqual(package.read(html[0]), self.output.read_bytes())
            index = json.loads(package.read('EVIDENCE-INDEX.json'))
            self.assertFalse(index['componentAccepted'])
            self.assertFalse(index['completeUnsignedChain'])
            self.assertFalse(index['devicePassed'])
            self.assertFalse(index['publicationEligible'])

    def preserve_originals(self):
        with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}, clear=True), \
                mock.patch.object(ci_apple, 'location', return_value=self.root), \
                mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: contextlib.nullcontext()):
            ci_apple.evidence()
        path = self.root / 'apple-original-evidence.zip'
        with zipfile.ZipFile(path) as package:
            members = {name: package.read(name) for name in package.namelist()}
        size = path.stat().st_size
        path.unlink()
        return members, size

    def test_diagnostic_cannot_consume_mandatory_entry_content_index_path_or_zip_budget(self):
        mandatory = self.root / 'mandatory.log'
        mandatory.write_bytes(b'original mandatory bytes')
        before, zip_size = self.preserve_originals()
        index_size = len(before['EVIDENCE-INDEX.json'])
        self.output.write_bytes(b'<html>optional diagnostic</html>')
        limits = {'EVIDENCE_ENTRIES': 1, 'EVIDENCE_CONTENT_BYTES': mandatory.stat().st_size,
                  'EVIDENCE_INDEX_BYTES': index_size, 'EVIDENCE_PATH_BYTES': len(mandatory.name),
                  'EVIDENCE_ZIP_BYTES': zip_size}
        for constant, value in limits.items():
            with self.subTest(budget=constant), mock.patch.object(ci_apple, constant, value):
                after, _ = self.preserve_originals()
            self.assertEqual(after, before)
        # With ordinary remaining budgets, exactly the optional member is added.
        after, _ = self.preserve_originals()
        name = self.output.relative_to(self.root).as_posix()
        self.assertEqual(set(after), set(before) | {name})
        self.assertEqual(after['mandatory.log'], before['mandatory.log'])
        index = json.loads(after['EVIDENCE-INDEX.json'])
        self.assertTrue(index['files'][name]['diagnosticOnly'])
        self.assertEqual(index['receiptReadErrors'], {})

    def test_optional_read_or_validation_errors_preserve_identical_mandatory_result(self):
        (self.root / 'mandatory.log').write_bytes(b'mandatory')
        before, _ = self.preserve_originals()
        self.output.write_bytes(b'optional report')
        original_open = os.open
        def deny_diagnostic(path, *args, **kwargs):
            if Path(path) == self.output:
                raise PermissionError('diagnostic read denied')
            return original_open(path, *args, **kwargs)
        with mock.patch.object(ci_apple.os, 'open', side_effect=deny_diagnostic):
            after, _ = self.preserve_originals()
        self.assertEqual(after, before)
        with mock.patch.object(apple_rust, 'CARGO_TIMING_BYTES', 1):
            after, _ = self.preserve_originals()
        self.assertEqual(after, before)
        self.output.unlink()
        self.output.symlink_to(self.root / 'mandatory.log')
        after, _ = self.preserve_originals()
        self.assertEqual(after, before)

    def test_optional_report_does_not_hide_mandatory_budget_or_read_failure(self):
        mandatory = self.root / 'mandatory.log'
        mandatory.write_bytes(b'mandatory')
        self.output.write_bytes(b'optional report')
        with mock.patch.object(ci_apple, 'EVIDENCE_ENTRIES', 0), \
                self.assertRaisesRegex(RuntimeError, 'selected original evidence entry budget exceeded'):
            self.preserve_originals()
        original_open = os.open
        def deny_mandatory(path, *args, **kwargs):
            if Path(path) == mandatory:
                raise PermissionError('mandatory read denied')
            return original_open(path, *args, **kwargs)
        with mock.patch.object(ci_apple.os, 'open', side_effect=deny_mandatory), \
                self.assertRaisesRegex(PermissionError, 'mandatory read denied'):
            self.preserve_originals()


if __name__ == '__main__':
    unittest.main()
