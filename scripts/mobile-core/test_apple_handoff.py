"""Inert same-run handoff controls; no compiler, carrier or remote API."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import apple_handoff as handoff
from artifacts import file_pin, receipt_digest


class AppleHandoffControls(unittest.TestCase):
    ctx = {'candidate': 'a' * 40, 'runId': '123', 'attempt': '1', 'repository': 'polaris-arch/Polaris'}
    app = {'candidate': 'a' * 40, 'files': {'synthetic.py': {'bytes': 1, 'sha256': 'b' * 64}}}

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

    def validate(self, manifest, root, checkout):
        with mock.patch.object(handoff, 'producer_code', return_value=self.app), \
                mock.patch.object(handoff, 'load', return_value={'source': 'fixed'}), \
                mock.patch.object(handoff, 'context', return_value=self.ctx):
            return handoff.validate_manifest(manifest, root, checkout)

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
                    self.assertRaisesRegex(RuntimeError, 'host layout/context'):
                handoff.import_bundle(folder, root)
            self.assertFalse((root / 'object.o').exists())
            self.assertFalse(json.loads((root / 'import-observation.json').read_text())['completed'])

    def test_carrier_dispatches_only_the_selected_target_without_sdk_producer(self):
        import ci_apple
        from contextlib import nullcontext
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'component-handoff.json').write_text('{}')
            paths = {name: str(root / name) for name in ('source', 'go', 'gomobile', 'gobind', 'developer',
                                                        'component-work', 'cache', 'rust-target')}
            (root / 'tools.json').write_text(json.dumps({'paths': paths}))
            for slot in handoff.SLOTS:
                with mock.patch.object(ci_apple, 'location', return_value=root), \
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
                                         'amd64' if slot == 'sim-amd64' else 'arm64')
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
            graph = [{'Path': value['goModule'], 'Main': True, 'Dir': str(source)}]
            binding = {'sourceContract': value, 'sourceObservation': {
                'sourceCommit': value['sourceCommit'], 'sourceTree': value['sourceTree']},
                'sourceOverlay': 0, 'moduleGraph': graph, 'producer': self.app}
            build_id = 'polaris-mobile-' + receipt_digest(binding)
            objects = {}
            def archive(target, arch, simulator):
                tags = set(value['apple']['tags']) | {'ios'} | ({'iossimulator'} if simulator else set())
                raw_info = 'path\tsynthetic/main\nmod\t' + value['goModule'] + '\t(devel)\t\n'
                raw_info += ''.join('build\t' + k + '=' + v + '\n' for k, v in {
                    'GOOS': 'ios', 'GOARCH': arch, 'CGO_ENABLED': '1', '-buildmode': 'c-archive',
                    '-trimpath': 'true', '-tags': ','.join(sorted(tags))}.items())
                native = fixtures.mach_object(target)
                path = scratch / (target + '.o'); path.write_bytes(native)
                objects[str(path)] = {'kind': 'compiler-object', **file_pin(path)}
                return fixtures.archive([('go.o', fixtures.mach_object(target, fixtures.inline(raw_info), build_id)),
                                         ('native.o', native)])
            device = single_fat_archive(archive('ios-arm64', 'arm64', False))
            simulator = fixtures.fat(archive('ios-x86_64-simulator', 'amd64', True),
                                    archive('ios-arm64-simulator', 'arm64', True))
            framework = component / 'Libbox.xcframework'; framework.mkdir(mode=0o755)
            entries = []
            for identifier, variant, arches, raw in [('ios-arm64', None, ['arm64'], device),
                    ('ios-arm64_x86_64-simulator', 'simulator', ['arm64', 'x86_64'], simulator)]:
                entry = {'LibraryIdentifier': identifier, 'LibraryPath': 'Libbox.framework',
                         'SupportedPlatform': 'ios', 'SupportedArchitectures': arches}
                if variant: entry['SupportedPlatformVariant'] = variant
                entries.append(entry)
                lib = framework / identifier / 'Libbox.framework'
                (lib / 'Resources/empty').mkdir(parents=True)
                (lib / 'Headers').mkdir(mode=0o755)
                (lib / 'Libbox').write_bytes(raw)
                (lib / 'Resources/Info.plist').write_bytes(plistlib.dumps({}))
                symbols = ['NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
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

    def test_exact_three_slot_aggregate_rejects_missing_wrong_target_and_other_attempt(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp); (folder / 'component').mkdir()
            manifest = self.signed({'schema': 'polaris-apple-component-handoff-v1',
                'context': self.ctx, 'producerJob': 'component', 'App': self.app,
                'sourceContract': {'source': 'fixed'}, 'componentFingerprint': 'c' * 64})
            mp = folder / 'component/component-handoff.json'; mp.write_text(json.dumps(manifest))
            transport = self.signed({'schema': 'polaris-apple-handoff-transport-v1',
                'context': self.ctx, 'manifest': file_pin(mp), 'archive': {'bytes': 1, 'sha256': 'e' * 64}})
            (folder / 'component/component-transport.json').write_text(json.dumps(transport))
            wrappers = {}
            for slot, target in zip(handoff.SLOTS, ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator')):
                (folder / slot).mkdir()
                rust_target = {'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim',
                               'sim-amd64': 'x86_64-apple-ios'}[slot]
                rust = self.signed({'schema': 'polaris-formal-apple-rust-v1', 'App': self.app,
                    'target': rust_target, 'tools': {'fixed': 'bytes'}, 'rustc': 'fixed-version',
                    'linkInputSha256': 'f' * 64, 'devicePassed': False})
                rust_path = folder / slot / 'summary-rust-receipt.json'; rust_path.write_text(json.dumps(rust))
                receipt = self.signed({'App': self.app, 'producerCandidate': self.ctx['candidate'],
                    'selectedTarget': target, 'componentFingerprint': 'c' * 64,
                    'linkInputSha256': 'f' * 64, 'rustInput': file_pin(rust_path),
                    'AppCarrierValidated': True, 'AppAccepted': False,
                    'devicePassed': False, 'signatureTrustValidated': False})
                actual = folder / slot / 'summary-app-carrier.json'; actual.write_text(json.dumps(receipt))
                row = self.signed({'schema': 'polaris-apple-carrier-handoff-v1', 'context': self.ctx,
                    'slot': slot, 'componentTransport': transport, 'receipt': receipt, 'receiptPin': file_pin(actual)})
                wrappers[slot] = row
                (folder / slot / ('carrier-' + slot + '.json')).write_text(json.dumps(row))
            with mock.patch.object(handoff, 'context', return_value=self.ctx), \
                    mock.patch.object(handoff, 'producer_code', return_value=self.app), \
                    mock.patch.object(handoff, 'load', return_value={'source': 'fixed'}), \
                    mock.patch('consume.app_identity', return_value=self.app), \
                    mock.patch.object(handoff, 'write_json') as write:
                handoff.aggregate(folder, folder / 'unused.json')
                self.assertEqual(write.call_args.args[1]['completeSlots'], list(handoff.SLOTS))
                actual_pin_path = folder / 'device-arm64/summary-app-carrier.json'
                original_bytes = actual_pin_path.read_bytes()
                actual_pin_path.write_bytes(original_bytes + b' ')
                with self.assertRaisesRegex(RuntimeError, 'receipt bytes differ'):
                    handoff.aggregate(folder, folder / 'unused.json')
                actual_pin_path.write_bytes(original_bytes)
                victim = folder / 'sim-amd64/carrier-sim-amd64.json'
                original = victim.read_bytes(); victim.unlink()
                with self.assertRaisesRegex(RuntimeError, 'bounded regular original receipt required:.*bytes=0'):
                    handoff.aggregate(folder, folder / 'unused.json')
                victim.write_bytes(original)
                changed = copy.deepcopy(wrappers['sim-amd64'])
                changed['receipt']['selectedTarget'] = 'ios-arm64'
                self.signed(changed['receipt']); self.signed(changed)
                (folder / 'sim-amd64/summary-app-carrier.json').write_text(json.dumps(changed['receipt']))
                changed['receiptPin'] = file_pin(folder / 'sim-amd64/summary-app-carrier.json'); self.signed(changed)
                victim.write_text(json.dumps(changed))
                with self.assertRaisesRegex(RuntimeError, 'carrier identity/scope'):
                    handoff.aggregate(folder, folder / 'unused.json')
                changed = copy.deepcopy(wrappers['sim-arm64']); changed['context'] = {**self.ctx, 'attempt': '2'}
                self.signed(changed)
                (folder / 'sim-arm64/carrier-sim-arm64.json').write_text(json.dumps(changed))
                with self.assertRaisesRegex(RuntimeError, 'slot/context/component'):
                    handoff.aggregate(folder, folder / 'unused.json')


if __name__ == '__main__':
    unittest.main()
