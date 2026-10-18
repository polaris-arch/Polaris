"""Inert same-run handoff controls; no compiler, carrier or remote API."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import apple_handoff as handoff
from artifacts import file_pin, receipt_digest


class AppleHandoffControls(unittest.TestCase):
    ctx = {'candidate': 'a' * 40, 'runId': '123', 'attempt': '1', 'repository': 'polaris-arch/Polaris'}
    app = {'candidate': 'a' * 40, 'files': {'synthetic.py': {'bytes': 1, 'sha256': 'b' * 64}}}

    def setUp(self):
        environment = mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'})
        environment.start(); self.addCleanup(environment.stop)

    def test_scope_defaults_to_device_and_only_explicit_opt_in_selects_both(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(handoff.required_slots(), ('device-arm64',))
            self.assertEqual(handoff.scope_fields(),
                             {'carrierScope': 'device-only', 'requiredSlots': ['device-arm64']})
        self.assertEqual(handoff.SLOTS, ('device-arm64', 'sim-arm64'))
        self.assertEqual(handoff.required_slots('device-and-simulator'), handoff.SLOTS)
        with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'}):
            self.assertEqual(handoff.required_slots(), handoff.SLOTS)
            self.assertEqual(handoff.required_slots('device-only'), ('device-arm64',))
        for scope in ('', 'sim-only', 'device', 'both', 'true', 'false', 'DEVICE-ONLY',
                      'device-only ', ['device-arm64'], ('device-arm64',), True, 1):
            with self.subTest(scope=scope), self.assertRaisesRegex(RuntimeError, 'unknown Apple carrier scope'):
                handoff.required_slots(scope)
        for scope in ('', 'sim-only', 'true'):
            with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': scope}), \
                    self.assertRaisesRegex(RuntimeError, 'unknown Apple carrier scope'):
                handoff.required_slots()

    def test_aggregate_requires_explicit_valid_scope_before_paths_or_receipts(self):
        with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'}), \
                mock.patch.object(handoff, 'Path') as path, \
                mock.patch.object(handoff, 'read_receipt') as read, \
                mock.patch.object(handoff, 'write_json') as write:
            for scope in (None, '', 'unknown', 'sim-only', True, False, 0, [], ['device-arm64']):
                with self.subTest(scope=scope), self.assertRaisesRegex(RuntimeError, 'explicit aggregate scope required'):
                    handoff.aggregate('/inert', '/inert-output', scope=scope)
            path.assert_not_called(); read.assert_not_called(); write.assert_not_called()

    @staticmethod
    def scope_mutations(row):
        cases = []
        for field in ('carrierScope', 'requiredSlots'):
            changed = copy.deepcopy(row); del changed[field]
            cases.append(('missing-' + field, changed))
        for value in (None, '', 'unknown', 'sim-only', True):
            changed = copy.deepcopy(row); changed['carrierScope'] = value
            cases.append(('scope-' + repr(value), changed))
        slots = row['requiredSlots']
        for value in (None, [], 'device-arm64', slots + ['sim-amd64'], slots + [slots[0]],
                      ['sim-arm64'], ['unknown'], ['sim-arm64', 'device-arm64'],
                      ['device-arm64', 'sim-arm64'] if len(slots) == 1 else ['device-arm64']):
            changed = copy.deepcopy(row); changed['requiredSlots'] = value
            cases.append(('slots-' + repr(value), changed))
        return cases

    def scoped_aggregate_fixture(self, base, scope):
        folder = base / 'aggregate'; (folder / 'component').mkdir(parents=True)
        original_root = base / 'original'; original_root.mkdir()
        manifest = self.signed({'schema': 'polaris-apple-component-handoff-v2',
            **handoff.scope_fields(scope), 'context': self.ctx, 'producerJob': 'component',
            'App': self.app, 'sourceContract': {'source': 'fixed'}, 'componentFingerprint': 'c' * 64})
        manifest_path = folder / 'component/component-handoff.json'
        manifest_path.write_text(json.dumps(manifest))
        transport = self.signed({'schema': 'polaris-apple-handoff-transport-v1',
            'context': self.ctx, 'manifest': file_pin(manifest_path), 'archive': {'bytes': 1, 'sha256': 'e' * 64}})
        for path in (folder / 'component/component-transport.json', original_root / 'component-transport.json'):
            path.write_text(json.dumps(transport))
        for slot in handoff.required_slots(scope):
            work = original_root / slot; work.mkdir()
            target = 'aarch64-apple-ios' if slot == 'device-arm64' else 'aarch64-apple-ios-sim'
            rust = self.signed({'schema': 'polaris-formal-apple-rust-v1', 'App': self.app,
                'target': target, 'tools': {'fixed': 'bytes'}, 'rustc': 'fixed-version',
                'linkInputSha256': 'f' * 64, 'devicePassed': False})
            rust_path = work / 'rust-original.json'
            rust_path.write_text(json.dumps(rust, indent=2) + '\n')
            receipt = self.signed({'schema': 'polaris-mobile-apple-carrier-v1',
                'App': self.app, 'producerCandidate': self.ctx['candidate'],
                'selectedTarget': 'ios-arm64' if slot == 'device-arm64' else 'ios-arm64-simulator',
                'componentFingerprint': 'c' * 64, 'linkInputSha256': 'f' * 64,
                'rustInput': {'receipt': str(rust_path), **file_pin(rust_path)},
                'AppCarrierValidated': True, 'AppAccepted': False,
                'devicePassed': False, 'signatureTrustValidated': False})
            (work / 'app-carrier.json').write_text(json.dumps(receipt, indent=2) + '\n')
            handoff.carrier_summary(original_root, slot)
            (original_root / 'summaries' / slot).rename(folder / slot)
        return folder, original_root

    def test_scoped_export_and_aggregate_roundtrip_preserves_originals_and_no_device_claim(self):
        for scope in ('device-only', 'device-and-simulator'):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as temporary, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': scope}), \
                    mock.patch.object(handoff, 'context', return_value=self.ctx), \
                    mock.patch.object(handoff, 'producer_code', return_value=self.app), \
                    mock.patch.object(handoff, 'load', return_value={'source': 'fixed'}), \
                    mock.patch('consume.app_identity', return_value=self.app):
                base = Path(temporary)
                folder, original_root = self.scoped_aggregate_fixture(base, scope)
                originals = {path: path.read_bytes() for path in original_root.rglob('*') if path.is_file()}
                output = base / 'chain.json'
                handoff.aggregate(folder, output, scope=scope)
                chain = json.loads(output.read_bytes())
                slots = list(handoff.required_slots(scope))
                self.assertEqual(chain['schema'], 'polaris-apple-unsigned-chain-v3')
                self.assertEqual(chain['carrierScope'], scope)
                self.assertEqual(chain['requiredSlots'], slots)
                self.assertEqual(chain['validatedSlots'], slots)
                self.assertEqual(chain['completeSlots'], slots)
                self.assertEqual(chain['notRequestedSlots'], ['sim-arm64'] if len(slots) == 1 else [])
                self.assertEqual(list(chain['carriers']), slots)
                for flag in ('devicePassed', 'signatureTrustValidated', 'publicationEligible'):
                    self.assertIs(chain[flag], False)
                for slot in slots:
                    row = chain['carriers'][slot]
                    self.assertEqual(row['schema'], 'polaris-apple-carrier-handoff-v2')
                    self.assertEqual(row['carrierScope'], scope); self.assertEqual(row['requiredSlots'], slots)
                    self.assertEqual((folder / slot / 'summary-app-carrier.json').read_bytes(),
                                     originals[original_root / slot / 'app-carrier.json'])
                    self.assertEqual((folder / slot / 'summary-rust-receipt.json').read_bytes(),
                                     originals[original_root / slot / 'rust-original.json'])
                self.assertEqual({path: path.read_bytes() for path in originals}, originals)
                with self.assertRaises(TypeError):
                    handoff.aggregate(folder, base / 'implicit.json')
                other = 'device-and-simulator' if scope == 'device-only' else 'device-only'
                with self.assertRaisesRegex(RuntimeError, 'exactly component'):
                    handoff.aggregate(folder, base / 'cross-scope.json', scope=other)
                for absent in slots:
                    original = folder / absent; moved = base / ('held-' + absent)
                    original.rename(moved)
                    with self.subTest(missing=absent), self.assertRaisesRegex(RuntimeError, 'exactly component'):
                        handoff.aggregate(folder, base / 'missing.json', scope=scope)
                    moved.rename(original)
                extra = folder / ('sim-arm64' if scope == 'device-only' else 'sim-amd64'); extra.mkdir()
                with self.assertRaisesRegex(RuntimeError, 'exactly component'):
                    handoff.aggregate(folder, base / 'extra.json', scope=scope)

    def test_scoped_manifest_rejects_missing_extra_duplicate_inverted_unknown_and_fingerprint_changes(self):
        for scope in ('device-only', 'device-and-simulator'):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as temporary:
                root, checkout = Path(temporary) / 'root', Path(temporary) / 'checkout'
                root.mkdir(); checkout.mkdir(); (root / 'object.o').write_bytes(b'original')
                manifest = self.manifest(root, checkout)
                manifest.update(schema='polaris-apple-component-handoff-v2', **handoff.scope_fields(scope))
                self.signed(manifest)
                self.assertEqual(self.validate(manifest, root, checkout, scope=scope), manifest)
                with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': scope}):
                    self.assertEqual(self.validate(manifest, root, checkout, scope=None), manifest)
                for label, changed in self.scope_mutations(manifest):
                    self.signed(changed)
                    with self.subTest(mutation=label), self.assertRaisesRegex(RuntimeError, 'declared scope/requiredSlots'):
                        self.validate(changed, root, checkout, scope=scope)
                changed = copy.deepcopy(manifest)
                other = 'device-and-simulator' if scope == 'device-only' else 'device-only'
                changed.update(handoff.scope_fields(other))
                with self.assertRaisesRegex(RuntimeError, 'manifest changed'):
                    self.validate(changed, root, checkout, scope=other)
                legacy = self.manifest(root, checkout)
                with self.assertRaisesRegex(RuntimeError, 'legacy Apple handoff requires exact two-slot scope'):
                    self.validate(legacy, root, checkout, scope='device-only')
                legacy.update(handoff.scope_fields('device-and-simulator')); self.signed(legacy)
                with self.assertRaisesRegex(RuntimeError, 'legacy Apple handoff requires exact two-slot scope'):
                    self.validate(legacy, root, checkout, scope='device-and-simulator')

    def test_freeze_rejects_prepared_scope_changes_before_reading_or_packaging_component(self):
        for scope in ('device-only', 'device-and-simulator'):
            tools = handoff.scope_fields(scope)
            for label, changed in self.scope_mutations(tools):
                with self.subTest(scope=scope, mutation=label), tempfile.TemporaryDirectory() as temporary, \
                        mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': scope}), \
                        mock.patch.object(handoff, 'read_receipt', return_value=changed) as read, \
                        mock.patch.object(handoff, 'lease') as lease, \
                        mock.patch.object(handoff, 'write_payload') as package, \
                        self.assertRaisesRegex(RuntimeError, 'prepared Apple carrier scope differs before freeze'):
                    handoff.freeze(Path(temporary))
                read.assert_called_once_with(Path(temporary).resolve() / 'tools.json')
                lease.assert_not_called(); package.assert_not_called()

    def test_aggregate_rejects_forged_scope_and_forbidden_flags_without_rewriting_native_receipts(self):
        for scope in ('device-only', 'device-and-simulator'):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as temporary, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': scope}), \
                    mock.patch.object(handoff, 'context', return_value=self.ctx), \
                    mock.patch.object(handoff, 'producer_code', return_value=self.app), \
                    mock.patch.object(handoff, 'load', return_value={'source': 'fixed'}), \
                    mock.patch('consume.app_identity', return_value=self.app):
                base = Path(temporary)
                folder, original_root = self.scoped_aggregate_fixture(base, scope)
                saved = {path: path.read_bytes() for path in base.rglob('*') if path.is_file()}
                targets = [folder / 'component/component-handoff.json',
                           *[folder / slot / ('carrier-' + slot + '.json') for slot in handoff.required_slots(scope)]]
                with mock.patch.object(handoff, 'write_json') as write:
                    for path in targets:
                        original = json.loads(saved[path])
                        mutations = self.scope_mutations(original)
                        legacy = copy.deepcopy(original)
                        legacy['schema'] = legacy['schema'].replace('-v2', '-v1')
                        mutations.append(('legacy-with-scope-fields', legacy))
                        mixed = copy.deepcopy(legacy)
                        del mixed['carrierScope']; del mixed['requiredSlots']
                        mutations.append(('legacy-mixed-with-scoped-wrappers', mixed))
                        if path.parent.name != 'component':
                            for flag in ('devicePassed', 'signatureTrustValidated', 'publicationEligible'):
                                for value in (True, None, 0, ''):
                                    changed = copy.deepcopy(original); changed[flag] = value
                                    mutations.append((flag + '-' + repr(value), changed))
                                changed = copy.deepcopy(original); del changed[flag]
                                mutations.append(('missing-' + flag, changed))
                        for label, changed in mutations:
                            self.signed(changed); path.write_text(json.dumps(changed))
                            if path.parent.name == 'component':
                                transport_path = folder / 'component/component-transport.json'
                                transport = json.loads(saved[transport_path]); transport['manifest'] = file_pin(path)
                                self.signed(transport); transport_path.write_text(json.dumps(transport))
                            with self.subTest(scope=scope, target=path.name, mutation=label), self.assertRaises(RuntimeError):
                                handoff.aggregate(folder, base / 'rejected.json', scope=scope)
                            write.assert_not_called()
                            for native, raw in saved.items():
                                if native.name in ('app-carrier.json', 'rust-original.json',
                                                   'summary-app-carrier.json', 'summary-rust-receipt.json'):
                                    self.assertEqual(native.read_bytes(), raw)
                            for target, raw in saved.items(): target.write_bytes(raw)

    def test_two_arm64_same_host_exports_preserve_each_original_and_reject_repeat(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'component-transport.json').write_text(json.dumps({'original': 'same-run'}))
            retained = {}
            with mock.patch.object(handoff, 'context', return_value=self.ctx), \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'}):
                for slot in handoff.SLOTS:
                    work = root / slot; work.mkdir(mode=0o700)
                    rust = work / 'rust-original.json'; rust.write_text(json.dumps({'slot': slot}))
                    receipt = {'App': self.app, 'devicePassed': False,
                               'rustInput': {'receipt': str(rust), **file_pin(rust)}}
                    original = work / 'app-carrier.json'
                    original.write_text(json.dumps(receipt, indent=2) + '\n')
                    handoff.carrier_summary(root, slot)
                    export = root / 'summaries' / slot
                    self.assertEqual((export / 'summary-app-carrier.json').read_bytes(), original.read_bytes())
                    self.assertEqual((export / 'summary-rust-receipt.json').read_bytes(), rust.read_bytes())
                    retained[slot] = {f.name: f.read_bytes() for f in export.iterdir()}
                for slot in handoff.SLOTS:
                    self.assertEqual({f.name: f.read_bytes() for f in (root / 'summaries' / slot).iterdir()},
                                     retained[slot])
                with self.assertRaises(FileExistsError):
                    handoff.carrier_summary(root, 'device-arm64')
            self.assertFalse((root / 'summary-app-carrier.json').exists())
            self.assertFalse((root / 'summary-rust-receipt.json').exists())

    def test_local_component_adapter_uses_full_original_verifier_without_restore_or_reproduce(self):
        import ci_apple
        from contextlib import nullcontext
        root = Path('/same-host/fixed')
        paths = {n: str(root / n) for n in ('component-work', 'cache', 'source', 'go', 'gomobile', 'gobind', 'developer')}
        tools = {'paths': paths, 'rustc': 'exact-compiler-version'}
        with mock.patch.object(ci_apple, 'location', return_value=root), \
                mock.patch.object(ci_apple, 'selected_apple_tools'), \
                mock.patch.object(ci_apple, 'read_receipt', side_effect=[tools, {'original': 'transport'}]), \
                mock.patch.object(ci_apple, 'private_dir', side_effect=lambda p: p), \
                mock.patch.object(ci_apple, 'lease', return_value=nullcontext()), \
                mock.patch.object(ci_apple, 'phase_timing', return_value=nullcontext()), \
                mock.patch.object(ci_apple, 'run', return_value=tools['rustc']) as run, \
                mock.patch.object(ci_apple, 'write_json') as save, \
                mock.patch.object(handoff, 'context', return_value=self.ctx), \
                mock.patch.object(handoff, 'verify_import') as verify, \
                mock.patch.object(handoff, 'import_bundle') as restore:
            ci_apple.import_local()
            verify.assert_called_once_with(Path(paths['component-work']), Path(paths['cache']),
                                           *(paths[n] for n in ('source', 'go', 'gomobile', 'gobind', 'developer')))
            restore.assert_not_called()
            run.assert_called_once_with(['rustc', '-vV'])
            self.assertTrue(save.call_args.args[1]['completed'])
            self.assertEqual(save.call_args.args[1]['context'], self.ctx)
            verify.side_effect = RuntimeError('actual compiler bytes differ')
            save.reset_mock()
            # Reset the original input reader for the second rejected call.
            ci_apple.read_receipt.side_effect = [tools]
            with self.assertRaisesRegex(RuntimeError, 'actual compiler bytes differ'):
                ci_apple.import_local()
            save.assert_not_called()

    def manifest(self, root, checkout):
        return self.signed({'schema': 'polaris-apple-component-handoff-v1', 'context': self.ctx,
            'producerJob': 'component', 'root': str(root), 'checkout': str(checkout),
            'App': self.app, 'sourceContract': {'source': 'fixed'},
            'componentFingerprint': 'c' * 64,
            'files': {'root': {'object.o': handoff.selected_row(root / 'object.o', root)}, 'checkout': {}}})

    @staticmethod
    def signed(row):
        row['fingerprint'] = receipt_digest({k: v for k, v in row.items() if k != 'fingerprint'})
        return row

    def validate(self, manifest, root, checkout, *, scope='device-and-simulator'):
        with mock.patch.object(handoff, 'producer_code', return_value=self.app), \
                mock.patch.object(handoff, 'load', return_value={'source': 'fixed'}), \
                mock.patch.object(handoff, 'context', return_value=self.ctx):
            return handoff.validate_manifest(manifest, root, checkout, scope=scope)

    def test_manifest_has_no_path_relabelling_and_original_pin_is_rechecked(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checkout = Path(tmp) / 'root', Path(tmp) / 'checkout'
            root.mkdir(); checkout.mkdir(); (root / 'object.o').write_bytes(b'actual')
            manifest = self.manifest(root, checkout)
            self.assertEqual(self.validate(manifest, root, checkout), manifest)
            for key, value in [('producerJob', 'carrier'), ('App', {'candidate': 'd' * 40}),
                               ('sourceContract', {'source': 'different'}), ('root', str(checkout))]:
                changed = copy.deepcopy(manifest); changed[key] = value; self.signed(changed)
                with self.subTest(key=key), self.assertRaises(RuntimeError):
                    self.validate(changed, root, checkout)
            for key in ('candidate', 'runId', 'attempt'):
                changed = copy.deepcopy(manifest); changed['context'][key] += '0'; self.signed(changed)
                with self.subTest(context=key), self.assertRaises(RuntimeError):
                    self.validate(changed, root, checkout)
            (root / 'object.o').write_bytes(b'tamper')
            with self.assertRaisesRegex(RuntimeError, 'bytes/mode/link changed'):
                self.validate(manifest, root, checkout)

    def test_paths_and_framework_links_cannot_escape(self):
        for name in ('../x', '/x', 'a/../x', 'a//x', './x', 'a\\x', ''):
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                handoff.checked_relative(name)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'root'; root.mkdir()
            outside = Path(tmp) / 'outside'; outside.write_bytes(b'x')
            (root / 'link').symlink_to('../outside')
            with self.assertRaisesRegex(RuntimeError, 'escaped'):
                handoff.selected_row(root / 'link', root)
            (root / 'original').write_bytes(b'x')
            (root / 'safe').symlink_to('original')
            self.assertEqual(handoff.selected_row(root / 'safe', root)['link'], 'original')

    def test_transport_is_streamed_bounded_and_mutation_sensitive(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'transport'; path.write_bytes(b'original')
            self.assertEqual(handoff.transport_pin(path), file_pin(path))
            with mock.patch.object(handoff, 'MAX_BYTES', 3), self.assertRaises(RuntimeError):
                handoff.transport_pin(path)
            original = handoff.transport_pin(path)
            path.write_bytes(b'modified')
            self.assertNotEqual(handoff.transport_pin(path), original)

    def test_host_layout_rejected_before_any_payload_extraction(self):
        import zipfile
        with tempfile.TemporaryDirectory() as tmp:
            folder, root = Path(tmp) / 'download', Path(tmp) / 'restore'
            folder.mkdir()
            manifest = self.signed({'schema': 'polaris-apple-component-handoff-v1',
                'context': self.ctx, 'root': '/different/host/root', 'checkout': str(handoff.ROOT),
                'files': {'root': {'object.o': {'bytes': 1, 'sha256': 'f' * 64, 'mode': 0o600}}, 'checkout': {}}})
            raw = json.dumps(manifest).encode()
            archive_path = folder / 'component-handoff.zip'
            with zipfile.ZipFile(archive_path, 'w') as archive:
                archive.writestr('component-handoff.json', raw)
                archive.writestr('root/object.o', b'x')
            transport = self.signed({'schema': 'polaris-apple-handoff-transport-v1', 'context': self.ctx,
                'archive': handoff.transport_pin(archive_path),
                'manifest': {'bytes': len(raw), 'sha256': __import__('hashlib').sha256(raw).hexdigest()}})
            (folder / 'component-transport.json').write_text(json.dumps(transport))
            with mock.patch.object(handoff, 'context', return_value=self.ctx), \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'}), \
                    self.assertRaisesRegex(RuntimeError, 'host layout/context'):
                handoff.import_bundle(folder, root)
            self.assertFalse((root / 'object.o').exists())
            self.assertFalse(json.loads((root / 'import-observation.json').read_text())['completed'])

    def test_carrier_dispatches_only_the_selected_target_without_sdk_producer(self):
        import ci_apple
        from contextlib import nullcontext
        with tempfile.TemporaryDirectory() as tmp:
            for scope, slot in (('device-only', 'device-arm64'),
                                ('device-and-simulator', 'device-arm64'),
                                ('device-and-simulator', 'sim-arm64')):
                # This is an independent argument-dispatch control, not the
                # full same-host build/evidence lifecycle. Isolate each scope
                # rather than pretending its incomplete staging was preserved.
                root = Path(tmp) / scope / slot
                root.mkdir(mode=0o700, parents=True)
                (root / 'component-handoff.json').write_text('{}')
                paths = {name: str(root / name) for name in ('source', 'go', 'gomobile', 'gobind', 'developer',
                                                            'component-work', 'cache', 'rust-target')}
                (root / 'tools.json').write_text(json.dumps({'paths': paths}))
                (root / 'import-observation.json').write_text(json.dumps({
                    'completed': True, 'context': self.ctx}))
                with mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': scope}), \
                        mock.patch.object(handoff, 'context', return_value=self.ctx), \
                        mock.patch.object(ci_apple, 'phase_timing', return_value=nullcontext()), \
                        mock.patch.object(ci_apple, 'private_dir', side_effect=lambda p: p), \
                        mock.patch.object(ci_apple, 'run') as dispatch, \
                        mock.patch.object(ci_apple.os, 'dup', return_value=42), \
                        mock.patch.object(ci_apple.os, 'close'), \
                        mock.patch.object(handoff, 'carrier_summary') as summary:
                    ci_apple.carrier(slot)
                    command = [str(v) for v in dispatch.call_args.args[0]]
                    self.assertIn('--imported-component', command)
                    self.assertEqual(command[command.index('--slot') + 1], slot)
                    self.assertFalse(any(v.endswith('/producer.py') for v in command))
                    self.assertEqual('--simulator' in command, slot != 'device-arm64')
                    if slot != 'device-arm64':
                        self.assertEqual(command[command.index('--simulator-arch') + 1],
                                         'arm64')
                    summary.assert_called_once_with(root, slot)

    def test_real_payload_roundtrip_umask022_preserves_private_slots_empty_dirs_and_component_predicate(self):
        # Structural fixture bytes only, not a compiler/device acceptance claim.
        import os
        import plistlib
        import zipfile
        from artifacts import inspect_framework, module, tree_inventory
        from consume import verify_original
        from fork_input import load
        from native_inputs import member_references, source_table
        from producer import private_dir
        from test_apple_entry import single_fat_archive
        from test_outputs import FormalOutputs
        fixtures = module('handoff_real_framework_controls', 'scripts/apple-carrier.test.py')
        value = load()
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp).resolve(); root = base / 'owned'; checkout = base / 'checkout'
            root.mkdir(mode=0o700); checkout.mkdir()
            component = private_dir(root / 'component'); cache = private_dir(root / 'cache')
            scratch = private_dir(component / 'scratch')
            source = component / 'build-view'; source.mkdir(mode=0o755)
            generated = source / 'build'; (generated / 'empty').mkdir(parents=True)
            (generated / 'marker').write_bytes(b'actual generated structural fixture')
            from test_source_policy import current_mobile_graph_fixture
            graph, module_info = current_mobile_graph_fixture(value, source)
            binding = {'sourceContract': value, 'sourceObservation': {
                'sourceCommit': value['sourceCommit'], 'sourceTree': value['sourceTree']},
                'sourceOverlay': 0, 'moduleGraph': graph, 'producer': self.app}
            build_id = 'polaris-mobile-' + receipt_digest(binding)
            objects = {}
            def archive(target, arch, simulator):
                tags = set(value['apple']['tags']) | {'ios'} | ({'iossimulator'} if simulator else set())
                raw_info = module_info
                raw_info += ''.join('build\t' + k + '=' + v + '\n' for k, v in {
                    'GOOS': 'ios', 'GOARCH': arch, 'CGO_ENABLED': '1', '-buildmode': 'c-archive',
                    '-trimpath': 'true', '-tags': ','.join(sorted(tags))}.items())
                native = fixtures.mach_object(target)
                path = scratch / (target + '.o'); path.write_bytes(native)
                objects[str(path)] = {'kind': 'compiler-object', **file_pin(path)}
                return fixtures.archive([('go.o', fixtures.mach_object(target, fixtures.inline(raw_info, version='go' + value['goVersion']), build_id)),
                                         ('native.o', native)])
            device = single_fat_archive(archive('ios-arm64', 'arm64', False))
            simulator = archive('ios-arm64-simulator', 'arm64', True)
            framework = component / 'Libbox.xcframework'; framework.mkdir(mode=0o755)
            entries = []
            for identifier, variant, arches, raw in [('ios-arm64', None, ['arm64'], device),
                    ('ios-arm64-simulator', 'simulator', ['arm64'], simulator)]:
                entry = {'LibraryIdentifier': identifier, 'LibraryPath': 'Libbox.framework',
                         'SupportedPlatform': 'ios', 'SupportedArchitectures': arches}
                if variant: entry['SupportedPlatformVariant'] = variant
                entries.append(entry)
                lib = framework / identifier / 'Libbox.framework'
                (lib / 'Resources/empty').mkdir(parents=True)
                (lib / 'Headers').mkdir(mode=0o755)
                (lib / 'Libbox').write_bytes(raw)
                (lib / 'Resources/Info.plist').write_bytes(plistlib.dumps({}))
                symbols = ['NewManagedStrictCommandServer', 'exportTailscaleDirectoryCapability',
                           'activateTailscaleDirectoryDomain', 'exportTailscaleDirectoryRetirement',
                           'NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
                           'CheckConfigWithResult', 'ConfigValidationContractVersion', 'exportTailscaleStoreRetirement',
                           'wakeNow', 'recordScreenState']
                (lib / 'Headers/Libbox.h').write_text('\n'.join(symbols) + '\n' +
                    FormalOutputs.validation_result_objc_header(None))
                for directory in (lib, lib.parent, lib / 'Resources', lib / 'Resources/empty', lib / 'Headers'):
                    directory.chmod(0o755)
            (framework / 'Info.plist').write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
            output = inspect_framework(framework, value, graph, build_id)
            log = component / 'original-bind.log'; log.write_bytes(b'original fixture bind log')
            mapping, origins = member_references(output, source_table(objects))
            native = {'schema': 'polaris-mobile-native-inputs-v2', 'scratch': str(scratch), 'cache': str(cache),
                'files': objects, 'originalBindLog': file_pin(log), 'staticMemberOrigins': mapping,
                'originsByDigest': origins, 'generatorOutputRoot': str(generated),
                'generatorOutputInventory': tree_inventory(generated)}
            receipt = self.signed({'schema': 'polaris-mobile-component-v1', 'platform': 'apple',
                'input': binding, 'output': output, 'nativeInputs': native,
                'sourceOverlay': 0, 'AppAccepted': False, 'devicePassed': False})
            receipt_path = component / 'component-receipt.json'; receipt_path.write_text(json.dumps(receipt))
            verify_original(framework, receipt_path, 'apple', {}, scratch)
            rows = {'root': handoff.selected_inventory(set(root.rglob('*')), root), 'checkout': {}}
            payload = base / 'payload.zip'
            with zipfile.ZipFile(payload, 'w') as package:
                handoff.write_payload(package, rows, root, checkout)
            root.rename(base / 'original-preserved'); root.mkdir(mode=0o700)
            previous_umask = os.umask(0o022)
            try:
                with zipfile.ZipFile(payload) as package:
                    handoff.restore_payload(package, rows, root, checkout)
            finally:
                os.umask(previous_umask)
            self.assertEqual((root / 'cache').stat().st_mode & 0o777, 0o700)
            self.assertEqual((root / 'component').stat().st_mode & 0o777, 0o700)
            private_dir(root / 'cache'); private_dir(root / 'component')
            self.assertEqual((framework / entries[0]['LibraryIdentifier'] / 'Libbox.framework/Headers').stat().st_mode & 0o777, 0o755)
            self.assertTrue((generated / 'empty').is_dir())
            self.assertEqual(verify_original(framework, receipt_path, 'apple', {}, scratch), receipt)

    def test_real_local_replacement_snapshot_restore_missing_redirect_and_tamper(self):
        import os
        import zipfile
        from artifacts import local_replacement_aliases
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp).resolve(); root = base / 'owned'; root.mkdir(mode=0o700)
            checkout = base / 'checkout'; checkout.mkdir()
            component = root / 'component'; component.mkdir(mode=0o700)
            source = component / 'build-view'; source.mkdir(mode=0o755)
            replacement = source / 'third_party/sing-tun'; (replacement / 'empty').mkdir(parents=True)
            (source / 'go.mod').write_bytes(b'module github.com/sagernet/sing-box\nreplace github.com/sagernet/sing-tun => ./third_party/sing-tun\n')
            (source / 'go.sum').write_bytes(b'original source checksum fixture\n')
            (replacement / 'go.mod').write_bytes(b'module github.com/sagernet/sing-tun\n')
            (replacement / 'tun.go').write_bytes(b'package tun\n')
            graph = [{'Path': 'github.com/sagernet/sing-box', 'Main': True, 'Dir': str(source)},
                {'Path': 'github.com/sagernet/sing-tun', 'Replace': {
                    'Path': './third_party/sing-tun', 'Dir': str(replacement)}}]
            receipt = {'input': {'moduleGraph': graph, 'sourceContract': {
                'goModSha256': file_pin(source / 'go.mod')['sha256'],
                'goSumSha256': file_pin(source / 'go.sum')['sha256']}}}
            snapshots = handoff.replacement_snapshots(receipt, component)
            handoff.verify_replacements(snapshots, receipt, component)
            rows = {'root': handoff.selected_inventory(set(root.rglob('*')), root), 'checkout': {}}
            payload = base / 'replacement.zip'
            with zipfile.ZipFile(payload, 'w') as package: handoff.write_payload(package, rows, root, checkout)
            root.rename(base / 'preserved'); root.mkdir(mode=0o700)
            old = os.umask(0o022)
            try:
                with zipfile.ZipFile(payload) as package: handoff.restore_payload(package, rows, root, checkout)
            finally: os.umask(old)
            handoff.verify_replacements(snapshots, receipt, component)
            aliases, _ = local_replacement_aliases(graph[1]['Replace'], str(source), './third_party/sing-tun', graph[1]['Path'])
            self.assertIn(str(replacement), aliases)
            raw = (replacement / 'tun.go').read_bytes(); (replacement / 'tun.go').write_bytes(b'package changed\n')
            with self.assertRaisesRegex(RuntimeError, 'source bytes/modes changed'):
                handoff.verify_replacements(snapshots, receipt, component)
            (replacement / 'tun.go').write_bytes(raw)
            main_mod = (source / 'go.mod').read_bytes(); (source / 'go.mod').write_bytes(b'changed source identity\n')
            with self.assertRaisesRegex(RuntimeError, 'source bytes/modes changed'):
                handoff.verify_replacements(snapshots, receipt, component)
            (source / 'go.mod').write_bytes(main_mod)
            replacement.rename(source / 'third_party/preserved-tun')
            with self.assertRaisesRegex(RuntimeError, 'directory unavailable'):
                handoff.verify_replacements(snapshots, receipt, component)
            replacement.symlink_to('preserved-tun', target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, 'source redirected'):
                handoff.verify_replacements(snapshots, receipt, component)
            replacement.unlink(); (replacement / 'empty-only').mkdir(parents=True)
            with self.assertRaisesRegex(RuntimeError, 'source bytes required'):
                handoff.replacement_snapshots(receipt, component)
            (replacement / 'empty-only').rmdir(); replacement.rmdir()
            (source / 'third_party/preserved-tun').rename(replacement)
            (replacement / 'extra.go').write_bytes(b'package extra\n')
            with self.assertRaisesRegex(RuntimeError, 'source bytes/modes changed'):
                handoff.verify_replacements(snapshots, receipt, component)

    def test_versioned_framework_links_restore_after_dependencies_with_original_bytes_and_modes(self):
        import os
        import zipfile
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp).resolve(); root = base / 'owned'; checkout = base / 'checkout'
            root.mkdir(mode=0o700); checkout.mkdir()
            framework = root / 'Libbox.framework'
            version = framework / 'Versions/A'
            (version / 'Headers').mkdir(parents=True)
            (version / 'Resources/empty').mkdir(parents=True)
            (version / 'Modules').mkdir()
            (version / 'Libbox').write_bytes(b'original structural archive bytes')
            (version / 'Headers/Libbox.h').write_bytes(b'original header bytes')
            (version / 'Resources/Info.plist').write_bytes(b'original resources bytes')
            (version / 'Modules/module.modulemap').write_bytes(b'original module bytes')
            (framework / 'Versions/Current').symlink_to('A')
            for name in ('Headers', 'Libbox', 'Modules', 'Resources'):
                (framework / name).symlink_to('Versions/Current/' + name)
            if hasattr(os, 'lchmod'):
                os.lchmod(framework / 'Versions/Current', 0o701)
            (root / 'alias').symlink_to('Libbox.framework/Libbox')
            rows = {'root': handoff.selected_inventory(set(root.rglob('*')), root), 'checkout': {}}
            names = list(rows['root'])
            self.assertLess(names.index('Libbox.framework/Headers'), names.index('Libbox.framework/Versions/Current'))
            payload = base / 'versioned.zip'
            with zipfile.ZipFile(payload, 'w') as package:
                handoff.write_payload(package, rows, root, checkout)
            root.rename(base / 'original'); root.mkdir(mode=0o700)
            with zipfile.ZipFile(payload) as package:
                handoff.restore_payload(package, rows, root, checkout)
            self.assertEqual(handoff.selected_inventory(set(root.rglob('*')), root), rows['root'])
            self.assertEqual((framework / 'Libbox').read_bytes(), b'original structural archive bytes')
            self.assertEqual((framework / 'Headers/Libbox.h').read_bytes(), b'original header bytes')
            self.assertEqual(os.readlink(framework / 'Versions/Current'), 'A')
            self.assertEqual(os.readlink(framework / 'Headers'), 'Versions/Current/Headers')
            self.assertEqual((root / 'alias').read_bytes(), b'original structural archive bytes')

    def test_link_dependency_missing_cycle_and_escape_fail_without_installing_invalid_links(self):
        import stat
        import zipfile
        for links, reason in (({'first': 'second', 'second': 'first'}, 'dependency'),
                ({'first': 'missing'}, 'dependency'), ({'first': '../outside'}, 'escaped'),
                ({'first': 'second', 'second': '../outside'}, 'escaped')):
            with self.subTest(links=links), tempfile.TemporaryDirectory() as tmp:
                base = Path(tmp).resolve(); root = base / 'owned'; checkout = base / 'checkout'
                root.mkdir(mode=0o700); checkout.mkdir(); (base / 'outside').write_bytes(b'outside')
                rows = {'root': {name: {'link': target, 'mode': 0o777} for name, target in links.items()},
                        'checkout': {}}
                payload = base / 'invalid.zip'
                with zipfile.ZipFile(payload, 'w') as package:
                    for name, target in links.items():
                        info = zipfile.ZipInfo('root/' + name); info.create_system = 3
                        info.external_attr = (stat.S_IFLNK | 0o777) << 16
                        package.writestr(info, target.encode())
                with zipfile.ZipFile(payload) as package, self.assertRaisesRegex(RuntimeError, reason):
                    handoff.restore_payload(package, rows, root, checkout)
                self.assertFalse(any((root / name).is_symlink() for name in links))

    def test_exact_two_arm64_aggregate_rejects_missing_extra_legacy_and_identity_changes(self):
        self.assertEqual(handoff.SLOTS, ('device-arm64', 'sim-arm64'))
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / 'aggregate'; (folder / 'component').mkdir(parents=True)
            manifest = self.signed({'schema': 'polaris-apple-component-handoff-v1',
                'context': self.ctx, 'producerJob': 'component', 'App': self.app,
                'sourceContract': {'source': 'fixed'}, 'componentFingerprint': 'c' * 64})
            mp = folder / 'component/component-handoff.json'; mp.write_text(json.dumps(manifest))
            transport = self.signed({'schema': 'polaris-apple-handoff-transport-v1',
                'context': self.ctx, 'manifest': file_pin(mp), 'archive': {'bytes': 1, 'sha256': 'e' * 64}})
            tp = folder / 'component/component-transport.json'; tp.write_text(json.dumps(transport))
            wrappers = {}
            for slot, target in zip(handoff.SLOTS, ('ios-arm64', 'ios-arm64-simulator')):
                (folder / slot).mkdir()
                rust_target = {'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim'}[slot]
                rust = self.signed({'schema': 'polaris-formal-apple-rust-v1', 'App': self.app,
                    'target': rust_target, 'tools': {'fixed': 'bytes'}, 'rustc': 'fixed-version',
                    'linkInputSha256': 'f' * 64, 'devicePassed': False})
                rust_path = folder / slot / 'summary-rust-receipt.json'; rust_path.write_text(json.dumps(rust))
                receipt = self.signed({'schema': 'polaris-mobile-apple-carrier-v1',
                    'App': self.app, 'producerCandidate': self.ctx['candidate'],
                    'selectedTarget': target, 'componentFingerprint': 'c' * 64,
                    'linkInputSha256': 'f' * 64, 'rustInput': file_pin(rust_path),
                    'AppCarrierValidated': True, 'AppAccepted': False,
                    'devicePassed': False, 'signatureTrustValidated': False})
                actual = folder / slot / 'summary-app-carrier.json'; actual.write_text(json.dumps(receipt))
                row = self.signed({'schema': 'polaris-apple-carrier-handoff-v1', 'context': self.ctx,
                    'slot': slot, 'componentTransport': transport, 'receipt': receipt, 'receiptPin': file_pin(actual),
                    'devicePassed': False, 'signatureTrustValidated': False, 'publicationEligible': False})
                wrappers[slot] = row
                (folder / slot / ('carrier-' + slot + '.json')).write_text(json.dumps(row))
            original_files = {path: path.read_bytes() for path in folder.rglob('*') if path.is_file()}
            def restore():
                for path, raw in original_files.items():
                    path.write_bytes(raw)
            def update_wrapper(row, slot='sim-arm64'):
                actual = folder / slot / 'summary-app-carrier.json'
                self.signed(row['receipt']); actual.write_text(json.dumps(row['receipt']))
                row['receiptPin'] = file_pin(actual); self.signed(row)
                (folder / slot / ('carrier-' + slot + '.json')).write_text(json.dumps(row))
            with mock.patch.object(handoff, 'context', return_value=self.ctx), \
                    mock.patch.object(handoff, 'producer_code', return_value=self.app), \
                    mock.patch.object(handoff, 'load', return_value={'source': 'fixed'}), \
                    mock.patch('consume.app_identity', return_value=self.app), \
                    mock.patch.object(handoff, 'write_json') as write:
                handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                self.assertEqual(write.call_args.args[1]['schema'], 'polaris-apple-unsigned-chain-v2')
                self.assertNotIn('carrierScope', write.call_args.args[1])
                self.assertNotIn('requiredSlots', write.call_args.args[1])
                self.assertEqual(write.call_args.args[1]['completeSlots'], ['device-arm64', 'sim-arm64'])
                self.assertEqual(set(write.call_args.args[1]['carriers']), set(handoff.SLOTS))
                self.assertFalse(write.call_args.args[1]['publicationEligible'])
                for slot in handoff.SLOTS:
                    for flag in ('devicePassed', 'signatureTrustValidated', 'publicationEligible'):
                        for mutation, value in [('true', True), ('missing', None), ('zero', 0), ('null', None)]:
                            changed = copy.deepcopy(wrappers[slot])
                            if mutation == 'missing':
                                del changed[flag]
                            else:
                                changed[flag] = value
                            update_wrapper(changed, slot)
                            # Scope is the only changed admission fact: native
                            # receipts, byte pins and all run/source identities
                            # remain valid and original while the wrapper is
                            # independently re-fingerprinted.
                            for name in ('summary-app-carrier.json', 'summary-rust-receipt.json'):
                                original = folder / slot / name
                                self.assertEqual(original.read_bytes(), original_files[original])
                            write.reset_mock()
                            with self.subTest(slot=slot, flag=flag, mutation=mutation), \
                                    self.assertRaisesRegex(RuntimeError, 'slot/context/component/scope'):
                                handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                            write.assert_not_called()
                            restore()
                for extra in ('sim-amd64', 'legacy-summary'):
                    path = folder / extra; path.mkdir()
                    with self.subTest(extra=extra), self.assertRaisesRegex(RuntimeError, 'exactly component'):
                        handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                    path.rmdir()
                missing = folder / 'sim-arm64'; moved = folder.parent / 'preserved-sim-arm64'
                missing.rename(moved)
                with self.assertRaisesRegex(RuntimeError, 'exactly component'):
                    handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                moved.rename(missing)
                victim = folder / 'sim-arm64/carrier-sim-arm64.json'; victim.unlink()
                with self.assertRaisesRegex(RuntimeError, 'bounded regular original receipt required:.*bytes=0'):
                    handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                restore()
                for name in ('summary-app-carrier.json', 'summary-rust-receipt.json'):
                    actual = folder / 'sim-arm64' / name
                    actual.write_bytes(actual.read_bytes() + b' ')
                    with self.subTest(original=name), self.assertRaisesRegex(RuntimeError, 'receipt bytes differ'):
                        handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                    restore()
                for key, value in [('attempt', '2'), ('runId', '124'), ('candidate', 'b' * 40)]:
                    changed = copy.deepcopy(wrappers['sim-arm64'])
                    changed['context'] = {**self.ctx, key: value}; update_wrapper(changed)
                    with self.subTest(context=key), self.assertRaisesRegex(RuntimeError, 'slot/context/component'):
                        handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                    restore()
                for key, value in [('selectedTarget', 'ios-arm64'), ('selectedTarget', 'ios-x86_64-simulator'),
                                   ('componentFingerprint', 'd' * 64), ('producerCandidate', 'b' * 40),
                                   ('App', {**self.app, 'files': {'different': 'source'}}),
                                   ('schema', 'polaris-mobile-apple-carrier-v0'), ('AppAccepted', True),
                                   ('devicePassed', True), ('signatureTrustValidated', True)]:
                    changed = copy.deepcopy(wrappers['sim-arm64']); changed['receipt'][key] = value
                    update_wrapper(changed)
                    with self.subTest(receipt=key, value=value), self.assertRaisesRegex(RuntimeError, 'carrier identity/scope'):
                        handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                    restore()
                for key, value in [('target', 'aarch64-apple-ios'), ('target', 'x86_64-apple-ios'),
                                   ('tools', {'fixed': 'different'}), ('rustc', 'different-version'),
                                   ('App', {**self.app, 'files': {'different': 'source'}})]:
                    rust_path = folder / 'sim-arm64/summary-rust-receipt.json'
                    rust = json.loads(rust_path.read_bytes()); rust[key] = value
                    self.signed(rust); rust_path.write_text(json.dumps(rust))
                    changed = copy.deepcopy(wrappers['sim-arm64']); changed['receipt']['rustInput'] = file_pin(rust_path)
                    update_wrapper(changed)
                    with self.subTest(rust=key, value=value), self.assertRaisesRegex(RuntimeError, 'aggregate (Rust|actual Rust)'):
                        handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                    restore()
                for key, value in [('sourceContract', {'source': 'different'}),
                                   ('App', {**self.app, 'files': {'different': 'source'}}),
                                   ('schema', 'polaris-apple-component-handoff-v0')]:
                    changed = copy.deepcopy(manifest); changed[key] = value; self.signed(changed)
                    mp.write_text(json.dumps(changed))
                    moved_transport = copy.deepcopy(transport); moved_transport['manifest'] = file_pin(mp)
                    self.signed(moved_transport); tp.write_text(json.dumps(moved_transport))
                    with self.subTest(manifest=key), self.assertRaisesRegex(RuntimeError,
                            'declared scope/requiredSlots' if key == 'schema' else 'component/source'):
                        handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                    restore()
                changed = copy.deepcopy(wrappers['sim-arm64']); changed['schema'] = 'polaris-apple-carrier-handoff-v0'
                update_wrapper(changed)
                with self.assertRaisesRegex(RuntimeError, 'declared scope/requiredSlots'):
                    handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')
                restore()
                handoff.aggregate(folder, folder / 'unused.json', scope='device-and-simulator')


if __name__ == '__main__':
    unittest.main()
