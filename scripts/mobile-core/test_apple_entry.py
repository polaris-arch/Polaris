"""Pure controls only: authored for the eventual whole-candidate Node gate."""
import unittest
import errno
import os
import tempfile
import json
import struct
import io
import zipfile
from copy import deepcopy
from pathlib import Path
from unittest import mock

import apple_rust
import build_apple_app
import ci_apple
import consume
from carriers import verify_apple_resources
from artifacts import module
from apple_handoff import scope_fields, transport_pin

# Fixed-path loader already used by these source controls; cache CLI stays kebab-case.
apple_cache = module('apple_dependency_cache_controls', 'scripts/mobile-core/apple-cache.py')


def object_archive(cpu, subtype, platform):
    obj = struct.pack('<8I', 0xfeedfacf, cpu, subtype, 1, 1, 24, 0, 0)
    obj += struct.pack('<6I', 0x32, 24, platform, 15 << 16, 27 << 16, 0)
    body = b'go.o' + obj
    header = f'{"#1/4":<16}{0:<12}{0:<6}{0:<6}{"644":<8}{len(body):<10}`\n'.encode()
    return b'!<arch>\n' + header + body


def single_fat_archive(raw, cpu=0x100000c, subtype=0):
    header = b'\xca\xfe\xba\xbe' + struct.pack('>I', 1)
    header += struct.pack('>5I', cpu, subtype, 32, len(raw), 4)
    return header + bytes(32 - len(header)) + raw


def fat_archive(arm_platform=7, intel_platform=7):
    arm = object_archive(0x100000c, 0, arm_platform)
    intel = object_archive(0x1000007, 3, intel_platform)
    header = b'\xca\xfe\xba\xbe' + struct.pack('>I', 2)
    header += struct.pack('>5I', 0x100000c, 0, 64, len(arm), 4)
    header += struct.pack('>5I', 0x1000007, 3, 64 + len(arm), len(intel), 4)
    return header + bytes(64 - len(header)) + arm + intel, arm, intel


class AppleEntryTests(unittest.TestCase):
    def setUp(self):
        environment = mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'})
        environment.start(); self.addCleanup(environment.stop)

    def test_default_scope_does_not_dispatch_simulator_or_unknown_carriers(self):
        with mock.patch.dict(ci_apple.os.environ, {}, clear=True), \
                mock.patch.object(ci_apple, 'location') as location, \
                mock.patch.object(ci_apple, 'run') as dispatch, \
                mock.patch('apple_handoff.carrier_summary') as summary:
            self.assertEqual(ci_apple.required_slots(), ('device-arm64',))
            for slot in ('sim-arm64', 'sim-amd64', '', None):
                with self.subTest(slot=slot), self.assertRaisesRegex(RuntimeError, 'unrequested Apple carrier slot'):
                    ci_apple.carrier(slot)
            location.assert_not_called(); dispatch.assert_not_called(); summary.assert_not_called()
        with mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'}):
            self.assertEqual(ci_apple.required_slots(), ('device-arm64', 'sim-arm64'))

    def test_default_scope_rejects_unrequested_retirement_before_location_or_deletion(self):
        with mock.patch.dict(ci_apple.os.environ, {}, clear=True), \
                mock.patch.object(ci_apple, 'location') as location, \
                mock.patch.object(ci_apple, 'slot_locks') as locks, \
                mock.patch.object(ci_apple.shutil, 'rmtree') as remove:
            for slot in ('sim-arm64', 'sim-amd64', '', None):
                with self.subTest(slot=slot), self.assertRaisesRegex(RuntimeError, 'unrequested Apple retirement slot'):
                    ci_apple.retire_slot(slot)
            location.assert_not_called(); locks.assert_not_called(); remove.assert_not_called()

    def test_prepare_passes_trusted_required_slots_to_admission_before_tool_install(self):
        candidate = 'a' * 40
        developer = Path('/selected/Contents/Developer')
        for scope, slots in ((None, ('device-arm64',)),
                             ('device-and-simulator', ('device-arm64', 'sim-arm64'))):
            env = {'POLARIS_APP_CANDIDATE': candidate}
            if scope is not None: env['POLARIS_APPLE_SCOPE'] = scope
            with self.subTest(scope=scope), mock.patch.dict(ci_apple.os.environ, env, clear=True), \
                    mock.patch.object(ci_apple, 'producer_code', return_value={}), \
                    mock.patch.object(ci_apple, 'location', return_value=Path('/isolated/entry')), \
                    mock.patch.object(ci_apple, 'run', return_value=candidate), \
                    mock.patch.object(ci_apple, 'selected_apple_tools', return_value=(developer, {'selected': 'tools'}, 'Xcode')), \
                    mock.patch('apple_admission.preflight') as admission, \
                    mock.patch.object(ci_apple, 'private_dir', side_effect=RuntimeError('stop before tool install')), \
                    self.assertRaisesRegex(RuntimeError, '^stop before tool install$'):
                ci_apple.prepare()
            admission.assert_called_once_with(Path('/isolated/entry'), {'selected': 'tools'}, expected_slots=slots)

    def test_build_rejects_prepared_scope_changes_before_frontend_or_native_dispatch(self):
        for scope in ('device-only', 'device-and-simulator'):
            other = 'device-and-simulator' if scope == 'device-only' else 'device-only'
            fields = ci_apple.scope_fields(scope)
            invalid = ({}, ci_apple.scope_fields(other),
                       {**fields, 'requiredSlots': []}, {**fields, 'requiredSlots': ['sim-arm64']})
            for tools in invalid:
                with self.subTest(scope=scope, tools=tools), \
                        mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': scope}), \
                        mock.patch.object(ci_apple, 'location', return_value=Path('/inert')), \
                        mock.patch.object(ci_apple, 'read_receipt', return_value=tools), \
                        mock.patch.object(ci_apple, 'phase_timing') as phase, \
                        mock.patch.object(ci_apple, 'run') as dispatch, \
                        mock.patch('apple_handoff.freeze') as freeze, \
                        self.assertRaisesRegex(RuntimeError, 'prepared Apple carrier scope differs before build'):
                    ci_apple.build()
                phase.assert_not_called(); dispatch.assert_not_called(); freeze.assert_not_called()

    def test_import_passes_same_required_slots_to_app_and_extension_admission(self):
        from contextlib import nullcontext
        for scope, slots in ((None, ('device-arm64',)),
                             ('device-and-simulator', ('device-arm64', 'sim-arm64'))):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                env = {'GITHUB_ENV': str(root / 'github-env')}
                if scope is not None: env['POLARIS_APPLE_SCOPE'] = scope
                paths = {'rust-target': str(root / 'rust-target'), 'developer': '/selected/Developer'}
                with mock.patch.dict(ci_apple.os.environ, env, clear=True), \
                        mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.object(ci_apple, 'selected_apple_tools'), \
                        mock.patch.object(ci_apple, 'phase_timing', return_value=nullcontext()), \
                        mock.patch('apple_handoff.import_bundle', return_value={'paths': paths, 'rustc': 'exact'}) as restore, \
                        mock.patch.object(ci_apple, 'private_dir'), \
                        mock.patch('apple_admission.preflight') as admission, \
                        mock.patch.object(ci_apple, 'run', return_value='exact'):
                    ci_apple.import_carrier(root / 'download')
                restore.assert_called_once_with(root / 'download', root)
                admission.assert_called_once_with(root, {**env, 'DEVELOPER_DIR': paths['developer']}, expected_slots=slots)

    def test_unrequested_evidence_scope_rejects_before_lease_or_snapshot(self):
        with mock.patch.dict(ci_apple.os.environ, {}, clear=True), \
                mock.patch.object(ci_apple, 'location', return_value=Path('/inert')), \
                mock.patch.object(ci_apple, 'private_dir') as private, \
                mock.patch.object(ci_apple, 'lease') as lease, \
                mock.patch.object(ci_apple, 'preserve_linked_archive') as preserve:
            for slot in ('sim-arm64', 'sim-amd64', ''):
                with self.subTest(slot=slot), self.assertRaisesRegex(RuntimeError, 'unrequested Apple evidence slot'):
                    ci_apple.evidence(slot=slot)
            with mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_CARRIER_SLOT': 'sim-arm64'}), \
                    self.assertRaisesRegex(RuntimeError, 'unrequested Apple evidence slot'):
                ci_apple.evidence()
            private.assert_not_called(); lease.assert_not_called(); preserve.assert_not_called()

    def test_evidence_records_only_required_slots_and_default_ignores_unrequested_sim_receipts(self):
        from contextlib import nullcontext
        for scope in ('device-only', 'device-and-simulator'):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                (root / 'cache').mkdir(mode=0o700)
                for slot, target in (('device-arm64', 'aarch64-apple-ios'),
                                     ('sim-arm64', 'aarch64-apple-ios-sim')):
                    work = root / slot; work.mkdir()
                    receipt = work / ('rust-' + target + '.json')
                    receipt.write_text(json.dumps({'target': target}) if scope != 'device-only' or slot == 'device-arm64'
                                       else 'unrequested malformed simulator receipt')
                    native = root / 'cache/rust-target' / target / 'release/libpolaris_lib.a'
                    native.parent.mkdir(parents=True); native.write_bytes(('original-' + target).encode())
                with mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': scope}, clear=True), \
                        mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                        mock.patch('builtins.print'):
                    ci_apple.evidence()
                with zipfile.ZipFile(root / 'apple-original-evidence.zip') as archive:
                    index = json.loads(archive.read('EVIDENCE-INDEX.json'))
                    slots = ['device-arm64'] if scope == 'device-only' else ['device-arm64', 'sim-arm64']
                    self.assertEqual(index['schema'], 'polaris-apple-original-evidence-v2')
                    self.assertEqual(index['carrierScope'], scope); self.assertEqual(index['requiredSlots'], slots)
                    self.assertFalse(index['receiptReadErrors'])
                    self.assertIs(index['devicePassed'], False); self.assertIs(index['publicationEligible'], False)
                    for slot, target in (('device-arm64', 'aarch64-apple-ios'),
                                         ('sim-arm64', 'aarch64-apple-ios-sim')):
                        member = 'cache/rust-target/' + target + '/release/libpolaris_lib.a'
                        self.assertEqual(member in archive.namelist(), slot in slots)
                        self.assertEqual(slot + '/rust-' + target + '.json' in archive.namelist(), slot in slots)
                        if slot in slots:
                            self.assertEqual(archive.read(member), (root / member).read_bytes())

    def same_host_slot_fixture(self, root, slot, raw):
        work = root / slot; work.mkdir(mode=0o700)
        (root / 'cache').mkdir(mode=0o700, exist_ok=True)
        target = 'aarch64-apple-ios' if slot == 'device-arm64' else 'aarch64-apple-ios-sim'
        native = root / 'cache/rust-target' / target / 'release/libpolaris_lib.a'
        native.parent.mkdir(parents=True); native.write_bytes(raw)
        linked = root / 'src-tauri/gen/apple/Externals/arm64/release/libapp.a'
        linked.parent.mkdir(parents=True, exist_ok=True); linked.write_bytes(raw)
        from artifacts import file_pin
        receipt = {'target': target, 'linkedArchive': str(linked), 'archive': file_pin(native)}
        (work / ('rust-' + target + '.json')).write_text(json.dumps(receipt))
        (work / 'original-rust-entry.log').write_text('inert compiler observation')
        return work, native, linked

    def test_evidence_uses_bounded_disk_reservation_below_unchanged_build_peak(self):
        import os
        import budget
        import producer
        import apple_handoff
        from types import SimpleNamespace
        for failed in (False, True):
            with self.subTest(failed=failed), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nnative')
                derived = work / 'derived-data'; derived.mkdir()
                (derived / 'object').write_bytes(b'rebuildable but not yet retired')
                streams = {'original-device-arm64.log.stdout.raw': b'out\x00\xffunfinished',
                           'original-device-arm64.log.stderr.raw': b'err\xfe\x00uncombined'}
                for name, raw in streams.items(): (root / name).write_bytes(raw)
                if failed: linked.write_bytes(b'unaccepted actual linked archive')
                value = ci_apple.load()
                build_peak = value['budget']['buildPeakReserveBytes']
                free_bytes = 10 * 1024 ** 3
                self.assertLess(ci_apple.EVIDENCE_RESERVE_BYTES + 1024 ** 3, free_bytes)
                self.assertGreater(build_peak + 1024 ** 3, free_bytes)
                with mock.patch.object(ci_apple, 'ROOT', root), \
                        mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}), \
                        mock.patch.dict(os.environ, {'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                        mock.patch.object(budget.os, 'statvfs', return_value=SimpleNamespace(f_bavail=free_bytes, f_frsize=1)):
                    with self.assertRaisesRegex(RuntimeError, 'peak reservations exhaust free space'):
                        with producer.lease(root / 'actual-build', root / 'cache', value):
                            self.fail('native build admitted below unchanged peak')
                    ci_apple.evidence(slot='device-arm64')
                    archive = root / 'apple-original-device-arm64-evidence.zip'
                    with zipfile.ZipFile(archive) as package:
                        for name, raw in streams.items(): self.assertEqual(package.read(name), raw)
                        self.assertEqual(package.read('device-arm64/original-linked-rust-archive.a'), linked.read_bytes())
                        index = json.loads(package.read('EVIDENCE-INDEX.json'))
                        self.assertEqual(bool(index['receiptReadErrors']), failed)
                    if failed:
                        with self.assertRaisesRegex(RuntimeError, 'incomplete or changed'):
                            ci_apple.retire_slot('device-arm64')
                    self.assertTrue(native.exists() and derived.exists())
                    for name, raw in streams.items(): self.assertEqual((root / name).read_bytes(), raw)
                    row = json.loads((root / 'budget' / (producer.sha(str(root / 'evidence-work').encode()) + '.reservation')).read_bytes())
                    self.assertEqual(row['reserveBytes'], ci_apple.EVIDENCE_RESERVE_BYTES)
                    self.assertEqual(row['groups'], [])
                    self.assertEqual(value['budget']['buildPeakReserveBytes'], build_peak)
                    self.assertEqual(producer.ACTIVE_FDS, [])

    def test_insufficient_evidence_space_or_active_cache_refuses_before_snapshot_or_zip(self):
        import os
        import budget
        import producer
        import apple_handoff
        from contextlib import nullcontext
        from types import SimpleNamespace
        for blocked in ('space', 'cache', 'retained-group'):
            with self.subTest(blocked=blocked), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nretained')
                raw_path = root / 'original-device-arm64.log.stderr.raw'; raw_path.write_bytes(b'failed raw\x00\xff')
                free_bytes = (ci_apple.EVIDENCE_RESERVE_BYTES + 1024 ** 3 - 1
                              if blocked == 'space' else 10 * 1024 ** 3)
                with mock.patch.object(ci_apple, 'ROOT', root), \
                        mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}), \
                        mock.patch.dict(os.environ, {'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                        mock.patch.object(budget.os, 'statvfs', return_value=SimpleNamespace(f_bavail=free_bytes, f_frsize=1)):
                    if blocked == 'retained-group':
                        with budget.reservation(root / 'evidence-work', ci_apple.load(), producer.ACTIVE_FDS,
                                                reserve_bytes=ci_apple.EVIDENCE_RESERVE_BYTES):
                            fd = int(os.environ['POLARIS_BUILD_RESERVATION_FD'])
                            row = json.loads(os.pread(fd, 8192, 0)); row['groups'] = [123456]
                            os.ftruncate(fd, 0); os.pwrite(fd, producer.canonical(row), 0)
                    held = (producer.slot_locks(root / 'active-native-work', root / 'cache')
                            if blocked == 'cache' else nullcontext())
                    live = (mock.patch.object(budget, 'group_alive', return_value=True)
                            if blocked == 'retained-group' else nullcontext())
                    with held, live, self.assertRaises((RuntimeError, BlockingIOError)):
                        ci_apple.evidence(slot='device-arm64')
                self.assertFalse((root / 'apple-original-device-arm64-evidence.zip').exists())
                self.assertFalse((work / 'original-linked-rust-archive.a').exists())
                self.assertFalse((root / 'staging-preservation-device-arm64.json').exists())
                self.assertEqual(raw_path.read_bytes(), b'failed raw\x00\xff')
                self.assertEqual(native.read_bytes(), b'!<arch>\nretained')
                self.assertEqual(linked.read_bytes(), native.read_bytes())
                self.assertEqual(producer.ACTIVE_FDS, [])

    def test_evidence_zip_and_copy_hard_bounds_keep_failed_originals(self):
        from artifacts import file_pin
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = root / 'original.raw'; original.write_bytes(b'unchanged original')
            pin = file_pin(original)
            output = io.BytesIO()
            with mock.patch.object(ci_apple, 'EVIDENCE_ZIP_BYTES', 8):
                writer = ci_apple.BoundedEvidenceZip(output)
                writer.write(b'12345678')
                with self.assertRaisesRegex(RuntimeError, 'ZIP exceeds reserved byte budget'):
                    writer.write(b'9')
                self.assertEqual(output.getvalue(), b'12345678')
                writer.seek(0); writer.write(b'AB')
                self.assertEqual(output.getvalue(), b'AB345678')
            original.write_bytes(b'a larger changed original')
            output = io.BytesIO()
            with self.assertRaisesRegex(RuntimeError, 'bounded regular original evidence required'):
                ci_apple.copy_pinned_evidence(original, output, pin)
            self.assertEqual(output.getvalue(), b'')
            self.assertEqual(original.read_bytes(), b'a larger changed original')
            original.write_bytes(b'initial bytes')
            pin = file_pin(original)
            class GrowingOutput(io.BytesIO):
                def write(self, raw):
                    with original.open('ab') as source: source.write(b'growth')
                    return super().write(raw)
            output = GrowingOutput()
            with self.assertRaisesRegex(RuntimeError, 'grew during preservation'):
                ci_apple.copy_pinned_evidence(original, output, pin)
            self.assertEqual(output.getvalue(), b'initial bytes')
            self.assertEqual(original.read_bytes(), b'initial bytesgrowth')

    def test_evidence_inventory_content_and_index_bounds_leave_sources_intact(self):
        from contextlib import nullcontext
        cases = [('EVIDENCE_ENTRIES', 0, 'entry budget'),
                 ('EVIDENCE_PATH_BYTES', 1, 'path budget'),
                 ('EVIDENCE_CONTENT_BYTES', 1, 'exceeds 4GiB budget'),
                 ('EVIDENCE_INDEX_BYTES', 1, 'index exceeds bound')]
        for constant, bound, reason in cases:
            with self.subTest(bound=constant), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                component = root / 'component'; component.mkdir()
                raw = component / 'original-failed.log'; raw.write_bytes(b'failed original output')
                with mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.object(ci_apple, 'lease', return_value=nullcontext()), \
                        mock.patch.object(ci_apple, constant, bound), \
                        mock.patch.dict('os.environ', {}, clear=True), \
                        self.assertRaisesRegex(RuntimeError, reason):
                    ci_apple.evidence()
                self.assertEqual(raw.read_bytes(), b'failed original output')
                self.assertFalse(list(root.glob('evidence-*-pin.json')))

    @mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'})
    def test_slot_evidence_freezes_shared_staging_before_next_arm64_platform_overwrite(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\ndevice')
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}):
                ci_apple.evidence(slot='device-arm64')
                first = (root / 'apple-original-device-arm64-evidence.zip').read_bytes()
                self.same_host_slot_fixture(root, 'sim-arm64', b'!<arch>\nsimulator')
                ci_apple.evidence(slot='sim-arm64')
                self.assertEqual((root / 'apple-original-device-arm64-evidence.zip').read_bytes(), first)
                with zipfile.ZipFile(io.BytesIO(first)) as package:
                    self.assertEqual(package.read('device-arm64/original-linked-rust-archive.a'),
                                     b'!<arch>\ndevice')
                    self.assertFalse(any(n.startswith('sim-arm64/') for n in package.namelist()))
                with self.assertRaisesRegex(RuntimeError, 'must not be overwritten'):
                    ci_apple.evidence(slot='device-arm64')

    def test_slot_retirement_keeps_originals_and_refuses_active_cache_or_bad_preservation(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nnative')
            derived = work / 'derived-data'; derived.mkdir(mode=0o700)
            (derived / 'rebuildable-object').write_bytes(b'object')
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}):
                ci_apple.evidence(slot='device-arm64')
                with mock.patch.object(ci_apple, 'slot_locks', side_effect=RuntimeError('active native cache')):
                    with self.assertRaisesRegex(RuntimeError, 'active native cache'):
                        ci_apple.retire_slot('device-arm64')
                self.assertTrue(native.exists() and derived.exists())
                original = native.read_bytes(); native.write_bytes(b'changed')
                with mock.patch.object(ci_apple, 'slot_locks', side_effect=lambda *args, **kwargs: nullcontext()):
                    with self.assertRaisesRegex(RuntimeError, 'missing from preserved'):
                        ci_apple.retire_slot('device-arm64')
                    self.assertTrue(native.exists() and derived.exists())
                    native.write_bytes(original)
                    ci_apple.retire_slot('device-arm64')
                self.assertFalse(native.exists() or derived.exists())
                self.assertTrue((root / 'apple-original-device-arm64-evidence.zip').exists())
                self.assertTrue((work / 'rust-aarch64-apple-ios.json').exists())
                self.assertTrue((work / 'original-linked-rust-archive.a').exists())
                self.assertTrue((work / 'original-rust-entry.log').exists())

    @mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'})
    def test_failed_slot_pack_preserves_bad_receipt_and_cache_without_false_retirement(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nnative')
            linked.write_bytes(b'wrong linked staging')
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(ci_apple, 'slot_locks', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}):
                ci_apple.evidence(slot='device-arm64')
                with zipfile.ZipFile(root / 'apple-original-device-arm64-evidence.zip') as package:
                    index = json.loads(package.read('EVIDENCE-INDEX.json'))
                    self.assertTrue(index['receiptReadErrors'])
                    self.assertFalse(index['componentAccepted'])
                    self.assertEqual(package.read(native.relative_to(root).as_posix()), native.read_bytes())
                    self.assertEqual(package.read('device-arm64/original-linked-rust-archive.a'),
                                     b'wrong linked staging')
                    observed = json.loads(package.read('device-arm64/linked-rust-preservation.json'))
                    self.assertFalse(observed['acceptedByReceipt'])
                    self.assertTrue(observed['preservationComplete'])
                    self.assertNotEqual(observed['expectedPin'], observed['originalPin'])
                with self.assertRaisesRegex(RuntimeError, 'incomplete or changed'):
                    ci_apple.retire_slot('device-arm64')
                self.assertTrue(native.exists())
                ci_apple.ensure_previous_staging_preserved(root, 'sim-arm64')
                self.same_host_slot_fixture(root, 'sim-arm64', b'!<arch>\nnext slot')
                ci_apple.evidence(slot='sim-arm64')
                with zipfile.ZipFile(root / 'apple-original-device-arm64-evidence.zip') as package:
                    self.assertEqual(package.read('device-arm64/original-linked-rust-archive.a'),
                                     b'wrong linked staging')

    @mock.patch.dict(ci_apple.os.environ, {'POLARIS_APPLE_SCOPE': 'device-and-simulator'})
    def test_unsafe_staging_preservation_blocks_shared_replacement_before_native_dispatch(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nnative')
            linked.unlink(); linked.symlink_to(native)
            (root / 'component-handoff.json').write_text('{}')
            (root / 'import-observation.json').write_text(json.dumps({
                'completed': True, 'context': {'runId': '1'}}))
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}), \
                    mock.patch.object(ci_apple, 'run') as dispatch:
                ci_apple.evidence(slot='device-arm64')
                self.assertFalse(json.loads((root / 'staging-preservation-device-arm64.json').read_text())['complete'])
                with self.assertRaisesRegex(RuntimeError, 'replacement blocked'):
                    ci_apple.carrier('sim-arm64')
                dispatch.assert_not_called()
                with self.assertRaisesRegex(RuntimeError, 'unknown Apple staging slot'):
                    ci_apple.ensure_previous_staging_preserved(root, 'sim-amd64')

    def test_claimed_staging_absence_marks_error_and_retains_target_but_empty_path_can_be_reused(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            work, native, linked = self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nnative')
            derived = work / 'derived-data'; derived.mkdir(mode=0o700)
            linked.unlink()  # The existing Rust receipt still claims these bytes.
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(ci_apple, 'slot_locks', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}):
                ci_apple.evidence(slot='device-arm64')
                with zipfile.ZipFile(root / 'apple-original-device-arm64-evidence.zip') as package:
                    index = json.loads(package.read('EVIDENCE-INDEX.json'))
                    self.assertIn('claimed original linked Rust archive missing',
                                  index['receiptReadErrors']['device-arm64:linked-preservation'])
                    absent = json.loads(package.read('device-arm64/linked-rust-preservation.json'))
                    self.assertTrue(absent['receiptClaimedArchive'] and absent['originalAbsent'])
                    self.assertFalse(absent['acceptedByReceipt'])
                with self.assertRaisesRegex(RuntimeError, 'incomplete or changed'):
                    ci_apple.retire_slot('device-arm64')
                self.assertTrue(native.exists() and derived.exists())
                # Absence is safely observed, so no bytes are overwritten by
                # the next platform even though the failed slot cannot retire.
                ci_apple.ensure_previous_staging_preserved(root, 'sim-arm64')

    def test_unproduced_staging_without_receipt_is_normal_observed_absence(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}):
                ci_apple.evidence(slot='device-arm64')
                with zipfile.ZipFile(root / 'apple-original-device-arm64-evidence.zip') as package:
                    index = json.loads(package.read('EVIDENCE-INDEX.json'))
                    self.assertFalse(index['receiptReadErrors'])
                    absent = json.loads(package.read('device-arm64/linked-rust-preservation.json'))
                    self.assertTrue(absent['originalAbsent'])
                    self.assertFalse(absent['receiptClaimedArchive'])

    def test_scoped_original_streams_survive_incomplete_combined_output_byte_exact(self):
        from contextlib import nullcontext
        import apple_handoff
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            self.same_host_slot_fixture(root, 'device-arm64', b'!<arch>\nnative')
            streams = {'original-device-arm64.log.stdout.raw': b'out\x00\xffuncombined',
                       'original-device-arm64.log.stderr.raw': b'err\xfe\x00unfinished'}
            (root / 'original-device-arm64.log').write_bytes(b'combined only prefix')
            for name, raw in streams.items(): (root / name).write_bytes(raw)
            (root / 'original-sim-arm64.log.stdout.raw').write_bytes(b'unrelated slot')
            with mock.patch.object(ci_apple, 'ROOT', root), \
                    mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                    mock.patch.object(apple_handoff, 'context', return_value={'runId': '1'}):
                ci_apple.evidence(slot='device-arm64')
            with zipfile.ZipFile(root / 'apple-original-device-arm64-evidence.zip') as package:
                for name, raw in streams.items(): self.assertEqual(package.read(name), raw)
                self.assertNotIn('original-sim-arm64.log.stdout.raw', package.namelist())

    def test_host_swift_controls_select_mac_sdk_target_without_changing_app_environment(self):
        from types import SimpleNamespace
        for host in ('arm64', 'x86_64'):
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); sdk = root / 'MacOSX.sdk'; sdk.mkdir()
                config = root / 'src-tauri/tauri.conf.json'; config.parent.mkdir()
                config.write_text(json.dumps({'bundle': {'macOS': {'minimumSystemVersion': '13.0'}}}))
                env = {'SDKROOT': '/selected/iPhoneOS.sdk', 'PLATFORM_NAME': 'iphoneos',
                       'ARCHS': 'arm64', 'IPHONEOS_DEPLOYMENT_TARGET': '17.0',
                       'DEVELOPER_DIR': '/selected/Xcode/Contents/Developer'}
                original = dict(env); calls = []
                def run(command, **kwargs):
                    calls.append(([str(x) for x in command], kwargs))
                    return str(sdk) if '--show-sdk-path' in command else ''
                with mock.patch.object(build_apple_app, 'ROOT', root), \
                        mock.patch.object(build_apple_app.os, 'uname', return_value=SimpleNamespace(machine=host)), \
                        mock.patch.object(build_apple_app, 'run', side_effect=run):
                    build_apple_app.host_swift_controls(root, env)
                command, options = calls[1]
                self.assertEqual(command[:4], ['xcrun', '--sdk', 'macosx', 'swiftc'])
                self.assertEqual(command[command.index('-sdk') + 1], str(sdk))
                self.assertEqual(command[command.index('-target') + 1], host + '-apple-macosx13.0')
                self.assertIn('-parse-as-library', command)
                self.assertEqual(options['env']['SDKROOT'], str(sdk))
                self.assertEqual(options['env']['ARCHS'], host)
                self.assertNotIn('IPHONEOS_DEPLOYMENT_TARGET', options['env'])
                self.assertEqual(calls[2][0], [str(root / 'measurement-conditions-test')])
                self.assertEqual(env, original)
                self.assertEqual(options['env']['DEVELOPER_DIR'], env['DEVELOPER_DIR'])

    def test_host_swift_control_rejects_unknown_host_before_compilation(self):
        from types import SimpleNamespace
        with mock.patch.object(build_apple_app.os, 'uname', return_value=SimpleNamespace(machine='unknown')), \
                mock.patch.object(build_apple_app, 'run') as run, \
                self.assertRaisesRegex(RuntimeError, 'unsupported actual Mac'):
            build_apple_app.host_swift_controls(Path('/not-used'), {})
        run.assert_not_called()

    def controlled_status(self, root, raw):
        facts = {'statusComplete': True, 'reason': None, 'observedStdoutBytes': len(raw),
                 'observedStderrBytes': 0, 'observedStatusPrefixSha256': consume.sha(raw)}
        with mock.patch.object(consume, 'ROOT', root), \
                mock.patch.object(consume, 'bounded_source_status', return_value=(raw, facts)):
            return consume.source_status_observation()

    def test_clean_derived_status_preserves_identity_without_original_export(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            log = root / 'source-status-before-rust.json'
            observed = self.controlled_status(root, b'')
            with mock.patch.object(consume, 'source_status_observation', return_value=observed), \
                    mock.patch.object(consume, 'run', return_value='a' * 40 + '\n'), \
                    mock.patch.object(consume, 'file_pin', return_value={'bytes': 1, 'sha256': 'b' * 64}):
                identity = consume.app_identity(status_log=log)
            self.assertEqual(identity['candidate'], 'a' * 40)
            saved = json.loads(log.read_bytes())
            self.assertTrue(saved['complete'] and saved['clean'])
            self.assertFalse(saved['rawStatusRetained'] or saved['fileContentsExported'])
            self.assertEqual(list(root.glob('*.log')), [])

    def test_dirty_generated_or_tracked_metadata_rejects_without_credentials_or_bodies(self):
        generated = 'crates/tauri-plugin-polaris-ios/ios/Package.resolved'
        tracked = 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj'
        secret = 'DO_NOT_EXPORT_CREDENTIAL_OR_SOURCE_BODY'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in (generated, tracked):
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('https://user:' + secret + '@example.invalid/private\n')
            raw = b'?? ' + generated.encode() + b'\0 M ' + tracked.encode() + b'\0'
            saved = self.controlled_status(root, raw)
            self.assertTrue(saved['complete']); self.assertFalse(saved['clean'])
            self.assertEqual([row['path'] for row in saved['paths']], [generated, tracked])
            self.assertTrue(all(row['sha256'] for row in saved['paths']))
            self.assertNotIn(secret, json.dumps(saved))
            self.assertNotIn('https://', json.dumps(saved))
            log = root / 'source-status-after-rust.json'
            with mock.patch.object(consume, 'source_status_observation', return_value=saved), \
                    self.assertRaisesRegex(RuntimeError, 'App consumer source is not frozen'):
                consume.app_identity(status_log=log)
            self.assertNotIn(secret, log.read_text())
            self.assertEqual(list(root.glob('*.log')), [])
            unknown = root / ('private-' + secret); unknown.write_bytes(b'inert')
            redacted = self.controlled_status(root, b'?? ' + unknown.name.encode() + b'\0')
            self.assertIsNone(redacted['paths'][0]['path'])
            self.assertNotIn(secret, json.dumps(redacted))

    def test_path_count_length_and_total_hash_budgets_are_incomplete_not_clean(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for i in range(consume.STATUS_PATHS + 1): (root / ('entry-' + str(i))).write_bytes(b'')
            many = b''.join(b'?? entry-' + str(i).encode() + b'\0' for i in range(consume.STATUS_PATHS + 1))
            for raw in (many, b'?? ' + b'x' * (consume.STATUS_PATH_BYTES + 1) + b'\0', b'?? partial'):
                observed = self.controlled_status(root, raw)
                self.assertFalse(observed['complete'] or observed['clean'])
                self.assertLessEqual(len(observed['paths']), consume.STATUS_PATHS)
            large = root / 'Cargo.lock'; large.write_bytes(b'x' * (consume.STATUS_HASH_BYTES + 1))
            observed = self.controlled_status(root, b' M Cargo.lock\0')
            self.assertFalse(observed['complete'])
            self.assertEqual(observed['hashBytesRead'], 0)
            self.assertIsNone(observed['paths'][0]['sha256'])

    def test_pipe_stdout_stderr_and_deadline_are_capped_during_collection(self):
        import sys
        import time
        cases = [
            ('import os; os.write(1, b"x" * 65536)', 'stdout-budget', consume.STATUS_BYTES + 1),
            ('import os; os.write(2, b"DO_NOT_EXPORT_SECRET" * 2048)', 'stderr-budget', consume.STATUS_STDERR_BYTES + 1),
            ('import time; time.sleep(2)', 'deadline', None),
        ]
        for code, reason, expected in cases:
            with self.subTest(reason=reason):
                raw, observed = consume.bounded_source_status(
                    [sys.executable, '-c', code], time.monotonic() + (0.1 if reason == 'deadline' else 5))
                self.assertFalse(observed['statusComplete'])
                self.assertEqual(observed['reason'], reason)
                self.assertLessEqual(len(raw), consume.STATUS_BYTES + 1)
                self.assertNotIn('DO_NOT_EXPORT_SECRET', json.dumps(observed))
                if expected is not None:
                    self.assertEqual(observed['observedStdoutBytes' if reason == 'stdout-budget' else 'observedStderrBytes'], expected)

    def test_collection_error_writes_incomplete_safe_metadata_and_rejects(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / 'source-status-before-rust.json'
            with mock.patch.object(consume, 'source_status_observation', side_effect=OSError('DO_NOT_EXPORT_SECRET')), \
                    self.assertRaisesRegex(RuntimeError, 'App consumer source is not frozen'):
                consume.app_identity(status_log=log)
            observed = json.loads(log.read_bytes())
            self.assertFalse(observed['complete'] or observed['clean'])
            self.assertEqual(observed['reason'], 'collector-error')
            self.assertNotIn('DO_NOT_EXPORT_SECRET', log.read_text())
            with mock.patch.object(consume.subprocess, 'Popen', side_effect=OSError('DO_NOT_EXPORT_SECRET')):
                raw, observed = consume.bounded_source_status(['git'], float('inf'))
            self.assertEqual(raw, b'')
            self.assertFalse(observed['statusComplete'])
            self.assertNotIn('DO_NOT_EXPORT_SECRET', json.dumps(observed))

    def test_report_budget_and_write_failure_cannot_accept_or_overwrite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            observed = self.controlled_status(root, b'')
            observed['paths'] = [{'pathSha256': 'a' * 64}] * 64
            observed.update(clean=False, complete=False, reason='controlled-dirty')
            log = root / 'source-status-after-rust.json'
            with mock.patch.object(consume, 'source_status_observation', return_value=observed), \
                    mock.patch.object(consume, 'STATUS_REPORT_BYTES', 1024), \
                    self.assertRaisesRegex(RuntimeError, 'App consumer source is not frozen'):
                consume.app_identity(status_log=log)
            self.assertEqual(json.loads(log.read_bytes())['reason'], 'report-budget')
            self.assertLessEqual(log.stat().st_size, 1024)
            original = log.read_bytes()
            with mock.patch.object(consume, 'source_status_observation', return_value=self.controlled_status(root, b'')), \
                    self.assertRaisesRegex(RuntimeError, 'already frozen'):
                consume.app_identity(status_log=log)
            self.assertEqual(log.read_bytes(), original)
            with mock.patch.object(consume, 'source_status_observation', return_value=self.controlled_status(root, b'')), \
                    self.assertRaisesRegex(RuntimeError, 'incomplete source status observation could not be written'):
                consume.app_identity(status_log=root / 'absent/observation.json')

    def test_preexisting_dirty_rejects_before_link_verification_after_slot_admission(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); target = root / 'target'; target.mkdir()
            env = {'POLARIS_FORMAL_APP_BUILD': '1', 'PLATFORM_NAME': 'iphoneos', 'ARCHS': 'arm64',
                   'CONFIGURATION': 'release', 'POLARIS_APP_WORK': str(root), 'POLARIS_APP_RUST_TARGET': str(target)}
            with mock.patch.dict(apple_rust.os.environ, env), \
                    mock.patch.object(apple_rust.os, 'uname', return_value=SimpleNamespace(sysname='Darwin')), \
                    mock.patch.object(apple_rust, 'load', return_value={}), \
                    mock.patch.object(apple_rust, 'private_dir', side_effect=lambda value: Path(value)), \
                    mock.patch.object(apple_rust, 'app_identity', side_effect=RuntimeError('controlled dirty')) as identity, \
                    mock.patch.object(apple_rust, 'verify_link') as link, \
                    self.assertRaisesRegex(RuntimeError, 'controlled dirty'):
                apple_rust.build_reserved()
            identity.assert_called_once_with(status_log=root / 'source-status-before-rust.json')
            link.assert_not_called()

    def test_exact_two_arm64_actual_target_mappings(self):
        self.assertEqual(apple_rust.target_for('iphoneos', 'arm64', 'release'), 'aarch64-apple-ios')
        self.assertEqual(apple_rust.target_for('iphonesimulator', 'arm64', 'release'), 'aarch64-apple-ios-sim')

    def test_host_arch_cannot_enable_intel_simulator(self):
        for host in ('arm64', 'x86_64'):
            with mock.patch.dict(apple_rust.os.environ, {'HOST_ARCH': host}), self.assertRaises(RuntimeError):
                apple_rust.target_for('iphonesimulator', 'x86_64', 'release')

    def test_partial_or_wrong_platform_arch_profile_refuses(self):
        for platform, arch, profile in [('iphoneos', 'x86_64', 'release'), ('macosx', 'arm64', 'release'),
                                        ('iphonesimulator', 'x86_64', 'release'),
                                        ('iphonesimulator', 'arm64 x86_64', 'release'), ('iphoneos', 'arm64', 'debug')]:
            with self.subTest(platform=platform, arch=arch, profile=profile), self.assertRaises(RuntimeError):
                apple_rust.target_for(platform, arch, profile)

    def test_supported_carriers_keep_both_arm64_capabilities_and_reject_intel(self):
        self.assertEqual(ci_apple.CARRIERS, [('device-arm64', None), ('sim-arm64', 'arm64')])
        with mock.patch.object(ci_apple, 'location') as location, self.assertRaises(RuntimeError):
            ci_apple.carrier('sim-amd64')
        location.assert_not_called()

    def test_legacy_intel_app_cli_rejects_before_source_or_work(self):
        required = [argument for name in ('source', 'go', 'gomobile', 'gobind', 'developer', 'work', 'cache')
                    for argument in ('--' + name, '/inert/' + name)]
        for selected in (['--simulator', '--simulator-arch', 'amd64'], ['--slot', 'sim-amd64']):
            with mock.patch.object(build_apple_app.sys, 'argv', ['build_apple_app.py', *required, *selected]), \
                    mock.patch.object(build_apple_app.sys, 'stderr', io.StringIO()), \
                    mock.patch.object(build_apple_app, 'load') as load, \
                    mock.patch.object(build_apple_app, 'private_dir') as work, \
                    mock.patch.object(build_apple_app, 'run') as run, self.assertRaises(SystemExit) as rejected:
                build_apple_app.main()
            self.assertEqual(rejected.exception.code, 2)
            load.assert_not_called(); work.assert_not_called(); run.assert_not_called()

    def test_local_entry_does_not_claim_hosted_admission(self):
        with mock.patch.dict(ci_apple.os.environ, {'GITHUB_ACTIONS': 'false'}), self.assertRaises(RuntimeError):
            ci_apple.location()

    def test_native_start_app_definition_is_required_and_tool_errors_propagate(self):
        archive, log = Path('/inert/libpolaris_lib.a'), Path('/inert/symbols.log')
        with mock.patch.object(apple_rust, 'run', return_value='00000000 T _start_app\n') as tool:
            apple_rust.verify_start_app(archive, log)
        tool.assert_called_once_with(['xcrun', 'nm', '--no-llvm-bc', '-gU', archive], log=log)
        for symbols in ['', '                 U _start_app\n', '00000000 T _start_app_extra\n']:
            with self.subTest(symbols=symbols), \
                    mock.patch.object(apple_rust, 'run', return_value=symbols), \
                    self.assertRaisesRegex(RuntimeError, '^actual Rust start_app definition missing$'):
                apple_rust.verify_start_app(archive, log)
        with mock.patch.object(apple_rust, 'run', side_effect=RuntimeError('invalid native object')), \
                self.assertRaisesRegex(RuntimeError, '^invalid native object$'):
            apple_rust.verify_start_app(archive, log)

    def test_all_eight_native_global_swift_entries_are_required_before_staging_app_archive(self):
        entries = apple_rust.SWIFT_BRIDGES
        complete = ''.join('00000000 T ' + name + '\n' for name in entries)
        self.assertEqual(apple_rust.verify_swift_bridges(complete), list(entries))
        # Each missing, undefined or local/private entry is a real link gap.
        for name in entries:
            for replacement in ('', '                 U ' + name + '\n',
                                '00000000 t ' + name + '\n', '00000000 T ' + name + '_extra\n'):
                with self.subTest(name=name, replacement=replacement), \
                        self.assertRaisesRegex(RuntimeError, 'actual global Swift bridge definition missing'):
                    apple_rust.verify_swift_bridges(complete.replace('00000000 T ' + name + '\n', replacement))
        original_partial_log = '00000000 T _start_app\n'
        with self.assertRaisesRegex(RuntimeError, 'actual global Swift bridge definition missing'):
            apple_rust.verify_swift_bridges(original_partial_log)

    def test_hosted_selection_rejects_default_xcode_wrong_build_and_either_old_sdk_before_tool_install(self):
        value = ci_apple.load()
        apple = value['apple']
        expected_xcode = 'Xcode ' + apple['xcodeVersion'] + '\nBuild version ' + apple['xcodeBuild']
        developer = Path('/selected/Xcode_27.1_Release_Candidate.app/Contents/Developer')
        candidate = 'a' * 40
        admitted = 'control: exact selected tools admitted'
        cases = [
            ('Xcode 27.0\nBuild version 27A266a', apple['iPhoneSDK'], apple['simulatorSDK'],
             'runner does not provide the exact selected Xcode; no fallback/skip'),
            ('Xcode ' + apple['xcodeVersion'] + '\nBuild version 27A266a', apple['iPhoneSDK'], apple['simulatorSDK'],
             'runner does not provide the exact selected Xcode; no fallback/skip'),
            ('Xcode ' + apple['xcodeVersion'] + '\nBuild version 27A9269', apple['iPhoneSDK'], apple['simulatorSDK'],
             'runner does not provide the exact selected Xcode; no fallback/skip'),
            (expected_xcode, '27.0', apple['simulatorSDK'], 'runner does not provide the exact selected SDK'),
            (expected_xcode, apple['iPhoneSDK'], '27.0', 'runner does not provide the exact selected SDK'),
            (expected_xcode, apple['iPhoneSDK'], apple['simulatorSDK'], admitted),
        ]
        for xcode, device_sdk, simulator_sdk, reason in cases:
            with self.subTest(xcode=xcode, device_sdk=device_sdk, simulator_sdk=simulator_sdk):
                answers = {('git', 'rev-parse', 'HEAD'): candidate, ('xcodebuild', '-version'): xcode,
                    ('xcrun', '--sdk', 'iphoneos', '--show-sdk-version'): device_sdk,
                    ('xcrun', '--sdk', 'iphonesimulator', '--show-sdk-version'): simulator_sdk}
                selected_path = mock.Mock()
                selected_path.resolve.return_value = developer
                with mock.patch.object(ci_apple, 'producer_code', return_value={}), \
                        mock.patch.object(ci_apple, 'location', return_value=Path('/isolated/entry')), \
                        mock.patch.dict(ci_apple.os.environ, {'POLARIS_APP_CANDIDATE': candidate}), \
                        mock.patch.object(ci_apple, 'Path', return_value=selected_path) as path, \
                        mock.patch.object(ci_apple, 'run', side_effect=lambda args, **_: answers[tuple(args)]) as run, \
                        mock.patch('apple_admission.preflight', return_value={}) as admission, \
                        mock.patch.object(ci_apple, 'private_dir', side_effect=RuntimeError(admitted)) as tools, \
                        self.assertRaisesRegex(RuntimeError, '^' + reason + '$'):
                    ci_apple.prepare()
                path.assert_called_once_with('/Applications/Xcode_27.1.app/Contents/Developer')
                selected_path.resolve.assert_called_once_with(strict=True)
                for call in run.call_args_list[1:]:
                    self.assertEqual(call.kwargs['env']['DEVELOPER_DIR'], str(developer))
                self.assertEqual(tools.call_count, int(reason == admitted))
                self.assertEqual(admission.call_count, int(reason == admitted))

    def test_early_carrier_admission_rejects_old_image_before_handoff_restoration(self):
        with mock.patch.object(ci_apple, 'location', return_value=Path('/isolated/entry')), \
                mock.patch.object(ci_apple, 'selected_apple_tools',
                                  side_effect=RuntimeError('exact selected Xcode required')) as select, \
                mock.patch('apple_handoff.import_bundle') as restore, \
                self.assertRaisesRegex(RuntimeError, '^exact selected Xcode required$'):
            ci_apple.import_carrier(Path('/not-read'))
        select.assert_called_once_with(ci_apple.load())
        restore.assert_not_called()

    def test_early_admission_records_scope_without_accepting_compiler_bytes_or_carrier(self):
        candidate = 'a' * 40
        value = ci_apple.load()
        xcode = 'Xcode ' + value['apple']['xcodeVersion'] + '\nBuild version ' + value['apple']['xcodeBuild']
        developer = Path('/selected/Contents/Developer')
        with mock.patch.dict(ci_apple.os.environ, {'POLARIS_APP_CANDIDATE': candidate}), \
                mock.patch.object(ci_apple, 'run', return_value=candidate), \
                mock.patch.object(ci_apple, 'selected_apple_tools', return_value=(developer, {}, xcode)), \
                mock.patch.object(ci_apple, 'location', return_value=Path('/isolated/entry')), \
                mock.patch.object(ci_apple, 'write_json') as save:
            ci_apple.admit()
        path, row = save.call_args.args
        self.assertEqual(path, Path('/isolated/entry/runner-tool-admission.json'))
        self.assertEqual(row['candidate'], candidate)
        self.assertEqual(row['xcode'], xcode)
        self.assertFalse(row['compilerByteIdentityChecked'])
        self.assertFalse(row['carrierAccepted'] or row['devicePassed'])

    def test_early_admission_rejects_wrong_candidate_before_selecting_or_recording_tools(self):
        with mock.patch.dict(ci_apple.os.environ, {'POLARIS_APP_CANDIDATE': 'a' * 40}), \
                mock.patch.object(ci_apple, 'run', return_value='b' * 40), \
                mock.patch.object(ci_apple, 'selected_apple_tools') as select, \
                mock.patch.object(ci_apple, 'write_json') as save, \
                self.assertRaisesRegex(RuntimeError, '^hosted App candidate differs$'):
            ci_apple.admit()
        select.assert_not_called()
        save.assert_not_called()

    def test_actual_rust_link_verifier_refuses_changed_archive_and_drops_xcode_fd(self):
        from artifacts import file_pin
        from fork_input import canonical, sha
        from producer import write_json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sdk = root / 'sdk'; sdk.mkdir()
            work = root / 'work'; work.mkdir(mode=0o700)
            archive = root / 'src-tauri/gen/apple/Externals/arm64/release/libapp.a'
            archive.parent.mkdir(parents=True); archive.write_bytes(b'!<arch>\ninert control')
            for name in ['Cargo.lock', 'src-tauri/tauri.ios.conf.json', *apple_rust.SWIFT_EXPORT_POLICY]:
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'inert source')
            tool = root / 'tool'; tool.write_bytes(b'inert tool')
            framework = root / 'Libbox.xcframework'
            original = framework / 'ios-arm64/Libbox.framework/Libbox'
            original.parent.mkdir(parents=True); original.write_bytes(object_archive(0x100000c, 0, 2))
            selected = {'libraryIdentifier': 'ios-arm64', 'binary': file_pin(original)}
            link = {'App': {'candidate': 'a' * 40}, 'framework': str(framework),
                    'componentOutput': {'slices': {'ios-arm64': selected}}}
            thin, selection = apple_rust.selected_wire_bytes(original.read_bytes(), 'aarch64-apple-ios')
            wire = work / 'wire-aarch64-apple-ios.a'; wire.write_bytes(thin)
            receipt = {'schema': 'polaris-formal-apple-rust-v1', 'target': 'aarch64-apple-ios',
                       'App': link['App'], 'linkInputSha256': 'b' * 64, 'sdk': str(sdk),
                       'linkedArchive': str(archive), 'archive': file_pin(archive),
                       'cargoLock': file_pin(root / 'Cargo.lock'),
                       'iosConfig': file_pin(root / 'src-tauri/tauri.ios.conf.json'),
                       'swiftBridgeDefinitions': list(apple_rust.SWIFT_BRIDGES),
                       'swiftExportPolicy': {name: file_pin(root / name) for name in apple_rust.SWIFT_EXPORT_POLICY},
                       'tools': {'cargo': {'path': str(tool), **file_pin(tool)}}, 'devicePassed': False,
                       'originalWireArchive': {'path': str(original), **file_pin(original)},
                       'wireArchive': {'path': str(wire), **file_pin(wire)}, 'wireSelection': selection}
            receipt['fingerprint'] = sha(canonical(receipt))
            write_json(work / 'rust-aarch64-apple-ios.json', receipt)
            env = {'POLARIS_FORMAL_APP_BUILD': '1', 'POLARIS_APP_WORK': str(work),
                   'PLATFORM_NAME': 'iphoneos', 'ARCHS': 'arm64', 'CONFIGURATION': 'release',
                   'SDKROOT': str(sdk), 'POLARIS_MOBILE_LINK_INPUT': str(work / 'link.json'),
                   'POLARIS_MOBILE_LINK_INPUT_SHA256': 'b' * 64, 'POLARIS_BUILD_RESERVATION_FD': '999999'}
            with mock.patch.object(apple_rust, 'ROOT', root), mock.patch.dict(apple_rust.os.environ, env), \
                    mock.patch.object(apple_rust, 'verify_link', return_value=link):
                apple_rust.verify()
                self.assertNotIn('POLARIS_BUILD_RESERVATION_FD', apple_rust.os.environ)
                for field in ('swiftBridgeDefinitions', 'swiftExportPolicy'):
                    changed = {key: val for key, val in receipt.items() if key not in (field, 'fingerprint')}
                    changed['fingerprint'] = sha(canonical(changed))
                    with mock.patch.object(apple_rust, 'read_receipt', return_value=changed), \
                            self.assertRaisesRegex(RuntimeError, 'input differs'):
                        apple_rust.verify()
                policy = root / apple_rust.SWIFT_EXPORT_POLICY[0]
                policy.write_bytes(b'changed export policy')
                with self.assertRaisesRegex(RuntimeError, 'input differs'):
                    apple_rust.verify()
                policy.write_bytes(b'inert source')
                archive.write_bytes(b'!<arch>\nchanged')
                with self.assertRaisesRegex(RuntimeError, 'input differs'):
                    apple_rust.verify()

    def test_arm64_wire_selection_keeps_full_container_validation_and_rejects_intel_target(self):
        raw, arm, intel = fat_archive()
        # Generic fat parsing still inspects legacy Intel bytes; formal Cargo
        # selection can only select the current arm64 platform.
        actual, selection = apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios-sim')
        self.assertEqual(actual, arm)
        self.assertEqual(selection['offset'], 64)
        self.assertEqual(apple_rust.selected_wire_bytes(arm, 'aarch64-apple-ios-sim')[0], arm)
        with self.assertRaisesRegex(RuntimeError, 'bounded selected wire target'):
            apple_rust.selected_wire_bytes(raw, 'x86_64-apple-ios')
        with self.assertRaisesRegex(RuntimeError, 'exact Cargo'):
            apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios')
        wrong_header = bytearray(raw); struct.pack_into('>I', wrong_header, 8, 0x1000007)
        with self.assertRaises(RuntimeError):
            apple_rust.selected_wire_bytes(bytes(wrong_header), 'aarch64-apple-ios-sim')
        overlap = bytearray(raw); struct.pack_into('>I', overlap, 36, 64)
        with self.assertRaises(RuntimeError):
            apple_rust.selected_wire_bytes(bytes(overlap), 'aarch64-apple-ios-sim')
        for invalid in [raw[:-1], b'!<arch>\n', b'\xcf\xfa\xed\xfe']:
            with self.subTest(raw=invalid[:8]), self.assertRaises(RuntimeError):
                apple_rust.selected_wire_bytes(invalid, 'aarch64-apple-ios-sim')

    def test_real_resource_materialization_and_final_carrier_require_28_geo_and_licenses(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / 'resources/data'; data.mkdir(parents=True)
            for n in range(28):
                (data / f'geo-{n}.srs').write_bytes(b'SRS' + bytes([n]))
            for name in ('LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'):
                (root / name).write_bytes(('actual ' + name).encode())
            config = root / 'src-tauri/tauri.ios.conf.json'; config.parent.mkdir(parents=True)
            config.write_text(json.dumps({'bundle': {'resources': ['../resources/data/',
                '../THIRD-PARTY-LICENSES.md', '../NOTICE', '../LICENSE']}}))
            (root / 'src-tauri/gen/apple').mkdir(parents=True)
            with mock.patch.object(consume, 'ROOT', root):
                expected = consume.materialize_apple_resources()
                self.assertEqual(len(expected), 31)
                self.assertEqual(consume.materialize_apple_resources(), expected)
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, 'w') as package:
                    for name, row in expected.items():
                        info = zipfile.ZipInfo('Payload/Polaris.app/assets/' + name)
                        info.create_system = 3; info.external_attr = (0o100000 | row['mode']) << 16
                        package.writestr(info, (root / row['source']).read_bytes())
                with zipfile.ZipFile(io.BytesIO(stream.getvalue())) as package:
                    observed = verify_apple_resources(package, 'Payload/Polaris.app/', expected)
                    self.assertEqual(len(observed), 31)
                    changed = {key: dict(row) for key, row in expected.items()}
                    key = next(iter(changed)); changed[key]['sha256'] = '0' * 64
                    with self.assertRaisesRegex(RuntimeError, 'bytes/mode'):
                        verify_apple_resources(package, 'Payload/Polaris.app/', changed)
                    with self.assertRaisesRegex(RuntimeError, 'inventory'):
                        verify_apple_resources(package, 'Payload/Polaris.app/', {})
                output = root / 'src-tauri/gen/apple/assets/_up_/NOTICE'
                output.write_bytes(b'wrong retained output')
                with self.assertRaisesRegex(RuntimeError, 'resource differs'):
                    consume.materialize_apple_resources()
                self.assertEqual(output.read_bytes(), b'wrong retained output')
                (data / 'geo-0.srs').unlink()
                with self.assertRaisesRegex(RuntimeError, '28 geo'):
                    consume.apple_resource_inputs()

    def test_resource_unknown_stage_is_preserved_and_never_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / 'src-tauri/gen/apple/assets.formal-stage'; stage.mkdir(parents=True)
            retained = stage / 'unknown'; retained.write_bytes(b'retained')
            with mock.patch.object(consume, 'ROOT', root), \
                    mock.patch.object(consume, 'apple_resource_inputs', return_value={}):
                with self.assertRaisesRegex(RuntimeError, 'stage retained'):
                    consume.materialize_apple_resources()
            self.assertEqual(retained.read_bytes(), b'retained')

    def test_single_fat_device_wrapper_selects_the_same_actual_target_bytes(self):
        device = object_archive(0x100000c, 0, 2)
        raw = single_fat_archive(device)
        thin, selection = apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios')
        self.assertEqual(thin, device)
        self.assertEqual(selection, {'target': 'aarch64-apple-ios', 'cpu': 0x100000c, 'platform': 2,
            'offset': 32, 'bytes': len(device), 'container': 'fat-ar'})
        with self.assertRaisesRegex(RuntimeError, 'exact Cargo'):
            apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios-sim')
        invalid_counts = []
        for count in (0, 3):
            changed = bytearray(raw); struct.pack_into('>I', changed, 4, count); invalid_counts.append(bytes(changed))
        for invalid in [raw[:-1], raw + b'\0', single_fat_archive(device, cpu=0x1000007, subtype=3), *invalid_counts]:
            with self.subTest(raw=invalid[:32]), self.assertRaises(RuntimeError):
                apple_rust.selected_wire_bytes(invalid, 'aarch64-apple-ios')

    def test_failed_staging_sdk_original_bytes_are_retained_without_acceptance(self):
        from contextlib import contextmanager
        @contextmanager
        def held(*args, **kwargs):
            yield
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework = root / 'component/staging/Libbox.xcframework'
            library = framework / 'ios-arm64/Libbox.framework'
            library.mkdir(parents=True)
            raw = single_fat_archive(object_archive(0x100000c, 0, 2))
            (library / 'Libbox').write_bytes(raw)
            (root / 'cache').mkdir(mode=0o700)
            rust_archive = root / 'cache/rust-target/aarch64-apple-ios/release/libpolaris_lib.a'
            rust_archive.parent.mkdir(parents=True)
            rust_archive.write_bytes(b'!<arch>\nunaccepted archive fixture')
            with mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', held), mock.patch('builtins.print'):
                ci_apple.evidence()
            with zipfile.ZipFile(root / 'apple-original-evidence.zip') as archive:
                member = 'component/staging/Libbox.xcframework/ios-arm64/Libbox.framework/Libbox'
                self.assertEqual(archive.read(member), raw)
                self.assertEqual(archive.read(rust_archive.relative_to(root).as_posix()), rust_archive.read_bytes())
                index = json.loads(archive.read('EVIDENCE-INDEX.json'))
                self.assertFalse(index['componentAccepted'])
                self.assertFalse(index['stagedComponentIsAccepted'])
                self.assertFalse(index['completeUnsignedChain'])
                self.assertEqual(index['files'][member]['sha256'], __import__('hashlib').sha256(raw).hexdigest())

    def test_original_framework_link_modes_match_transport_without_following_targets(self):
        import stat
        from contextlib import nullcontext
        from apple_handoff import selected_row
        for link_mode in (0o755, 0o777):
            with self.subTest(link_mode=oct(link_mode)), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                framework = root / 'component/Libbox.xcframework/ios-arm64/Libbox.framework'
                version = framework / 'Versions/A'; version.mkdir(parents=True)
                version.chmod(0o750)
                target = version / 'Libbox'; target.write_bytes(b'original framework binary')
                target.chmod(0o640)
                link = framework / 'Libbox'; link.symlink_to('Versions/A/Libbox')
                directory_link = framework / 'Current'; directory_link.symlink_to('Versions/A')
                real_lstat = Path.lstat

                def source_lstat(path):
                    observed = real_lstat(path)
                    if path in (link, directory_link):
                        # Linux links cannot be chmod'ed; model actual Darwin lstat
                        # while retaining real links, targets, and archive I/O.
                        fields = list(observed); fields[0] = stat.S_IFLNK | link_mode
                        return os.stat_result(fields)
                    return observed

                with mock.patch.object(Path, 'lstat', source_lstat), \
                        mock.patch.object(ci_apple, 'location', return_value=root), \
                        mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()), \
                        mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}, clear=True), \
                        mock.patch('builtins.print'):
                    transported = {path: selected_row(path, root) for path in (link, directory_link, target, version)}
                    ci_apple.evidence()
                with zipfile.ZipFile(root / 'apple-original-evidence.zip') as archive:
                    for path in (link, directory_link):
                        name = path.relative_to(root).as_posix()
                        info = archive.getinfo(name)
                        self.assertEqual(info.create_system, 3)
                        self.assertEqual(info.external_attr >> 16, stat.S_IFLNK | transported[path]['mode'])
                        self.assertEqual(archive.read(name), os.readlink(path).encode())
                    info = archive.getinfo(target.relative_to(root).as_posix())
                    self.assertEqual(info.external_attr >> 16, stat.S_IFREG | transported[target]['mode'])
                    self.assertEqual(archive.read(info), target.read_bytes())
                self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o640)
                self.assertEqual(stat.S_IMODE(version.stat().st_mode), transported[version]['mode'])
                self.assertEqual(transported[version]['mode'], 0o750)

    def test_unreadable_component_receipt_does_not_block_original_evidence_packaging(self):
        from contextlib import contextmanager
        @contextmanager
        def held(*args, **kwargs):
            yield
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            component = root / 'component'; component.mkdir()
            raw = b'{"schema":"polaris-mobile-component-v1","nativeInputs":'
            (component / 'component-receipt.json').write_bytes(raw)
            (component / 'original-bind.log').write_bytes(b'original tool output')
            with mock.patch.object(ci_apple, 'location', return_value=root), \
                    mock.patch.object(ci_apple, 'lease', held), mock.patch('builtins.print'):
                ci_apple.evidence()
            with zipfile.ZipFile(root / 'apple-original-evidence.zip') as archive:
                self.assertEqual(archive.read('component/component-receipt.json'), raw)
                self.assertEqual(archive.read('component/original-bind.log'), b'original tool output')
                index = json.loads(archive.read('EVIDENCE-INDEX.json'))
                self.assertIn('component/component-receipt.json', index['receiptReadErrors'])
                self.assertEqual(index['receiptBytes']['component/component-receipt.json'], len(raw))
                self.assertFalse(index['componentAccepted'])
                self.assertFalse(index['completeUnsignedChain'])
                self.assertFalse(index['publicationEligible'])

    def test_actual_bsd_unsorted_and_sorted_symbol_tables_bind_real_code_offsets(self):
        fixtures = module('formal_bsd_symbol_table_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_bsd_symbol_table_parser', 'scripts/apple-carrier.py')
        helper = module('formal_bsd_symbol_table_info', 'scripts/libbox-patches/android-source.py')
        for table_name in ('__.SYMDEF', '__.SYMDEF SORTED'):
            # For the unsorted table intentionally preserve zeta before alpha.
            # These are structural fixtures, not executed native/SDK proof.
            strings = b'zeta\0alpha\0'
            offsets = (0, 5) if table_name == '__.SYMDEF' else (5, 0)
            table = struct.pack('<6I', 16, offsets[0], 0, offsets[1], 0, len(strings)) + strings
            first = fixtures.archive([(table_name, table)])
            code_offset = len(first)
            table = struct.pack('<6I', 16, offsets[0], code_offset, offsets[1], code_offset, len(strings)) + strings
            code = fixtures.mach_object('ios-arm64')
            raw = fixtures.archive([(table_name, table), ('actual-code.o', code)])
            observed = parser.archive(raw, helper)
            self.assertEqual(observed['members'][0]['name'], table_name)
            self.assertEqual(observed['members'][0]['classification'], 'symbol-table')
            self.assertEqual(observed['members'][1]['headerOffset'], code_offset)
            wrapped = single_fat_archive(raw)
            thin, selected = apple_rust.selected_wire_bytes(wrapped, 'aarch64-apple-ios')
            self.assertEqual(thin, raw)
            self.assertEqual(selected['bytes'], len(raw))
            for bad_table in [table[:-1],
                    struct.pack('<I', 15) + table[4:],
                    table[:4] + struct.pack('<I', len(strings)) + table[8:],
                    table[:8] + struct.pack('<I', code_offset + 1) + table[12:],
                    table[:8] + struct.pack('<I', 8) + table[12:],
                    table[:-1] + b'x', table + bytes(8), table + b'bad']:
                with self.subTest(name=table_name, table=bad_table[:24]), self.assertRaises(RuntimeError):
                    parser.archive(fixtures.archive([(table_name, bad_table), ('actual-code.o', code)]), helper)
            with self.assertRaisesRegex(RuntimeError, 'position/payload'):
                parser.archive(fixtures.archive([('actual-code.o', code), (table_name, table)]), helper)
            with self.assertRaisesRegex(RuntimeError, 'position/payload'):
                parser.archive(fixtures.archive([(table_name, table), (table_name, table), ('actual-code.o', code)]), helper)
            with self.assertRaisesRegex(RuntimeError, 'unsupported code member format'):
                parser.archive(fixtures.archive([(table_name + ' UNKNOWN', table), ('actual-code.o', code)]), helper)

    def test_actual_literal_and_tlv_section_kinds_keep_width_and_zero_fill_bounds(self):
        fixtures = module('formal_section_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_section_parser', 'scripts/apple-carrier.py')
        helper = module('formal_section_info', 'scripts/libbox-patches/android-source.py')
        # All seven kinds occur in the exact ac6 three-slice SDK inventory.
        layouts = [(3, '__TEXT', '__literal4', 4), (4, '__TEXT', '__literal8', 8),
                   (5, '__DATA', '__objc_selrefs', 8), (14, '__TEXT', '__literal16', 16),
                   (17, '__DATA', '__thread_data', 9), (18, '__DATA', '__thread_bss', 8),
                   (19, '__DATA', '__thread_vars', 24)]
        section_header = 32 + 72 + 80  # second section in the anonymous MH_OBJECT segment
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            for kind, segment, section, length in layouts:
                raw = bytearray(fixtures.mach_object(target, extra=[(segment, section, bytes(length))]))
                struct.pack_into('<I', raw, section_header + 64, kind)
                if kind == 18:
                    struct.pack_into('<I', raw, section_header + 48, 0)
                observed = parser.macho(bytes(raw), helper)['sections'][1]
                self.assertEqual(observed['flags'] & 255, kind)
                self.assertEqual(observed['zeroFill'], kind == 18)
                self.assertEqual('sha256' in observed, kind != 18)
                bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 68, 1)
                with self.subTest(target=target, kind=kind, bad='reserved'), self.assertRaises(RuntimeError):
                    parser.macho(bytes(bad), helper)
                bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 64, 0x7f)
                with self.assertRaisesRegex(RuntimeError, 'unsupported section type'):
                    parser.macho(bytes(bad), helper)
                if kind in (3, 4, 5, 14, 19):
                    bad = bytearray(raw); struct.pack_into('<Q', bad, section_header + 40, length - 1)
                    with self.assertRaisesRegex(RuntimeError, 'element width'):
                        parser.macho(bytes(bad), helper)
                if kind == 18:
                    bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 48, 1)
                    with self.assertRaisesRegex(RuntimeError, 'zero-fill file offset'):
                        parser.macho(bytes(bad), helper)
                    bad = bytearray(raw); struct.pack_into('<I', bad, section_header + 60, 1)
                    with self.assertRaisesRegex(RuntimeError, 'TLV zero-fill relocation'):
                        parser.macho(bytes(bad), helper)

    def test_observed_optimization_hint_is_object_only_bounded_disjoint_and_recorded(self):
        fixtures = module('formal_optimization_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_optimization_parser', 'scripts/apple-carrier.py')
        helper = module('formal_optimization_info', 'scripts/libbox-patches/android-source.py')
        payload = bytes(8)  # structural fixture, not a compiler/linker run
        for target in ('ios-arm64', 'ios-arm64-simulator'):
            raw = bytearray(fixtures.mach_object(target, load_commands=[struct.pack('<4I', 0x2e, 16, 0, len(payload))]))
            command = 32 + 152 + 24  # one-section segment and LC_BUILD_VERSION
            struct.pack_into('<I', raw, command + 8, len(raw))
            raw += payload
            observed = [r for r in parser.macho(bytes(raw), helper)['loadCommands'] if r['command'] == 0x2e][0]
            self.assertEqual(observed['dataBytes'], len(payload))
            self.assertEqual(observed['payloadSha256'], helper.digest(payload))
            for field, value in [(4, 24), (8, 1), (8, len(raw) + 1), (12, len(raw)), (0, 0x8000002e)]:
                bad = bytearray(raw); struct.pack_into('<I', bad, command + field, value)
                with self.subTest(field=field, value=value), self.assertRaises(RuntimeError):
                    parser.macho(bytes(bad), helper)
            bad = bytearray(raw); struct.pack_into('<I', bad, 12, 2)
            with self.assertRaisesRegex(RuntimeError, 'optimization hint object'):
                parser.macho(bytes(bad), helper)
            bad = bytearray(raw)
            text_offset = struct.unpack_from('<I', bad, 32 + 72 + 48)[0]
            struct.pack_into('<I', bad, command + 8, text_offset)
            with self.assertRaisesRegex(RuntimeError, 'overlap'):
                parser.macho(bytes(bad), helper)
        duplicate = struct.pack('<4I', 0x2e, 16, 0, 0)
        with self.assertRaisesRegex(RuntimeError, 'duplicate'):
            parser.macho(fixtures.mach_object('ios-arm64', load_commands=[duplicate, duplicate]), helper)

    @staticmethod
    def code_signature_fixture(fixtures, target, *, kind=2, duplicate=False, overlap=False,
                               duplicate_linkedit=False):
        # Structural metadata only: the blob is opaque and is not a valid
        # authenticated signature or a compiler-generated CodeDirectory.
        payload = struct.pack('>7I', 0xfade0cc0, 28, 1, 0, 20, 0xfade0c02, 8)
        text = b'fixture!'
        cpu, subcpu, platform, _, _, _ = fixtures.carrier.TARGETS[target]
        data_commands = 1 + int(duplicate) + int(overlap)
        commands_bytes = 152 + 72 + int(duplicate_linkedit) * 72 + 24 + data_commands * 16
        text_offset = 32 + commands_bytes
        signature_offset = text_offset + len(text)
        section = struct.pack('<16s16sQQ8I', b'__text', b'__TEXT', text_offset,
                              len(text), text_offset, 0, 0, 0, 0, 0, 0, 0)
        text_segment = struct.pack('<II16s4Q4I', 0x19, 152, b'__TEXT', 0,
                                   signature_offset, 0, signature_offset, 5, 5, 1, 0) + section
        linkedit = struct.pack('<II16s4Q4I', 0x19, 72, b'__LINKEDIT', 0x1000,
                               len(payload), signature_offset, len(payload), 1, 1, 0, 0)
        build_version = struct.pack('<6I', 0x32, 24, platform, 17 << 16, (27 << 16) | (1 << 8), 0)
        signature = struct.pack('<4I', 0x1d, 16, signature_offset, len(payload))
        commands = text_segment + linkedit
        if duplicate_linkedit:
            commands += struct.pack('<II16s4Q4I', 0x19, 72, b'__LINKEDIT', 0x2000,
                                    0, signature_offset, 0, 1, 1, 0, 0)
        commands += build_version + signature
        if duplicate:
            commands += signature
        if overlap:
            commands += struct.pack('<4I', 0x26, 16, signature_offset, len(payload))
        header = struct.pack('<8I', 0xfeedfacf, cpu, subcpu, kind,
                             3 + data_commands + int(duplicate_linkedit), commands_bytes, 0, 0)
        return (header + commands + text + payload,
                32 + 152 + 72 + int(duplicate_linkedit) * 72 + 24, signature_offset, payload)

    def test_linked_code_signature_records_opaque_bytes_without_trust(self):
        fixtures = module('formal_signature_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_signature_parser', 'scripts/apple-carrier.py')
        helper = module('formal_signature_info', 'scripts/libbox-patches/android-source.py')
        # The command was observed in the actual8795 sim-arm64 App and
        # PacketTunnel; other targets here are structural controls, not builds.
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            raw, _, offset, payload = self.code_signature_fixture(fixtures, target)
            parsed = parser.macho(raw, helper)
            row = next(row for row in parsed['loadCommands'] if row['command'] == 0x1d)
            self.assertEqual((row['dataOffset'], row['dataBytes']), (offset, len(payload)))
            self.assertEqual(row['payloadSha256'], helper.digest(payload))
            self.assertEqual(row['payloadKind'], 'opaque-code-signature')
            self.assertIs(row['trustValidated'], False)
            changed = raw[:-1] + bytes([raw[-1] ^ 1])
            observed = next(row for row in parser.macho(changed, helper)['loadCommands'] if row['command'] == 0x1d)
            self.assertNotEqual(observed['payloadSha256'], row['payloadSha256'])
            self.assertIs(observed['trustValidated'], False)

    def test_code_signature_refuses_invalid_locus_duplicate_size_and_overlap(self):
        fixtures = module('formal_signature_negative_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_signature_negative_parser', 'scripts/apple-carrier.py')
        helper = module('formal_signature_negative_info', 'scripts/libbox-patches/android-source.py')
        raw, command, offset, payload = self.code_signature_fixture(fixtures, 'ios-arm64-simulator')
        bads = []
        for field, value in [(command + 4, 8), (command + 8, 0),
                             (command + 8, len(raw) + 1), (command + 12, 0),
                             (command + 12, len(payload) + 1)]:
            bad = bytearray(raw); struct.pack_into('<I', bad, field, value); bads.append(bytes(bad))
        missing = bytearray(raw); missing[32 + 152 + 8:32 + 152 + 24] = b'__MISSING'.ljust(16, b'\0'); bads.append(bytes(missing))
        short = bytearray(raw); struct.pack_into('<Q', short, 32 + 152 + 48, len(payload) - 1); bads.append(bytes(short))
        outside = bytearray(raw); struct.pack_into('<2I', outside, command + 8, offset - 8, 8); bads.append(bytes(outside))
        for bad in bads:
            with self.subTest(control=bad[command:command + 16].hex()), self.assertRaises(RuntimeError):
                parser.macho(bad, helper)
        for flags, reason in [({'kind': 1}, 'executable'), ({'duplicate': True}, 'duplicate'),
                              ({'overlap': True}, 'overlap'), ({'duplicate_linkedit': True}, 'unique')]:
            bad, _, _, _ = self.code_signature_fixture(fixtures, 'ios-arm64-simulator', **flags)
            with self.assertRaisesRegex(RuntimeError, reason):
                parser.macho(bad, helper)
        unknown = bytearray(raw); struct.pack_into('<I', unknown, command, 0x1e)
        with self.assertRaisesRegex(RuntimeError, 'unsupported load command'):
            parser.macho(bytes(unknown), helper)

    def test_actual_linked_weak_swift_libraries_and_rpaths_are_structurally_recorded(self):
        fixtures = module('formal_dyld_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_dyld_parser', 'scripts/apple-carrier.py')
        helper = module('formal_dyld_info', 'scripts/libbox-patches/android-source.py')
        # Exact device-arm64 App/PacketTunnel command bytes from ab8cf's IPA.
        weak = bytes.fromhex('18000080480000001800000002000000014d9209000001002f7573722f6c69622f73776966742f6c696273776966744156466f756e646174696f6e2e64796c696200000000000000')
        rpaths = [bytes.fromhex(raw) for raw in (
            '1c000080200000000c0000002f7573722f6c69622f7377696674000000000000',
            '1c000080280000000c0000004065786563757461626c655f706174682f4672616d65776f726b7300',
            '1c000080300000000c0000004065786563757461626c655f706174682f2e2e2f2e2e2f4672616d65776f726b73000000',
        )]
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            observed = parser.macho(fixtures.mach_object(target, kind=2, load_commands=[weak, *rpaths]), helper)
            rows = [r for r in observed['loadCommands'] if r['command'] in (0x80000018, 0x8000001c)]
            self.assertEqual([r['payloadHex'] for r in rows], [c.hex() for c in [weak, *rpaths]])
            self.assertEqual(rows[0]['name'], '/usr/lib/swift/libswiftAVFoundation.dylib')
            self.assertTrue(rows[0]['weak'])
            self.assertEqual((rows[0]['timestamp'], rows[0]['currentVersionRaw'], rows[0]['compatibilityVersionRaw']),
                             struct.unpack_from('<3I', weak, 12))
            self.assertEqual([r['name'] for r in rows[1:]],
                             ['/usr/lib/swift', '@executable_path/Frameworks', '@executable_path/../../Frameworks'])

    def test_weak_dylib_and_rpath_refuse_malformed_strings_versions_kind_and_unknown_commands(self):
        fixtures = module('formal_dyld_negative_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_dyld_negative_parser', 'scripts/apple-carrier.py')
        helper = module('formal_dyld_negative_info', 'scripts/libbox-patches/android-source.py')
        def command(kind, name, offset=None):
            base = 24 if kind == 0x80000018 else 12
            size = (base + len(name) + 1 + 7) // 8 * 8
            head = struct.pack('<3I', kind, size, base if offset is None else offset)
            if kind == 0x80000018:
                head += struct.pack('<3I', 2, 0x10000, 0x10000)
            return head + name + bytes(size - base - len(name))
        for kind, valid in [(0x80000018, b'/usr/lib/swift/libswiftCoreFoundation.dylib'),
                            (0x8000001c, b'@executable_path/Frameworks')]:
            good = command(kind, valid)
            bads = [command(kind, valid, 0), command(kind, valid, len(good)), command(kind, valid, 32),
                    command(kind, valid + bytes(160)), command(kind, b''),
                    command(kind, b'\xff'), command(kind, b'/private/evil.dylib'),
                    command(kind, b'/usr/lib/swift/../evil.dylib'), command(kind, b'@rpath/evil.dylib')]
            no_nul = bytearray(good); no_nul[24 if kind == 0x80000018 else 12:] = b'x' * (len(good) - (24 if kind == 0x80000018 else 12)); bads.append(bytes(no_nul))
            padding = bytearray(good + bytes(8)); struct.pack_into('<I', padding, 4, len(padding)); bads.append(bytes(padding))
            trailing = bytearray(command(kind, valid + b'\0x')); bads.append(bytes(trailing))
            undersized = struct.pack('<4I', kind, 16, 12, 0); bads.append(undersized)
            unknown = bytearray(good); struct.pack_into('<I', unknown, 0, 0x80000017); bads.append(bytes(unknown))
            for bad in bads:
                with self.subTest(kind=kind, bad=bad.hex()), self.assertRaises(RuntimeError):
                    parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[bad]), helper)
            with self.assertRaisesRegex(RuntimeError, 'linked executable'):
                parser.macho(fixtures.mach_object('ios-arm64', load_commands=[good]), helper)
            if kind == 0x80000018:
                wrong = bytearray(good); struct.pack_into('<I', wrong, 16, 0)
                with self.assertRaisesRegex(RuntimeError, 'version relationship'):
                    parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[bytes(wrong)]), helper)
            else:
                with self.assertRaisesRegex(RuntimeError, 'duplicate executable rpath'):
                    parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[good, good]), helper)

    def test_observed_linker_option_command_bytes_are_parsed_and_recorded_for_object_only(self):
        fixtures = module('formal_linker_option_fixtures', 'scripts/apple-carrier.test.py')
        parser = module('formal_linker_option_parser', 'scripts/apple-carrier.py')
        helper = module('formal_linker_option_info', 'scripts/libbox-patches/android-source.py')
        # Exact command bytes observed in the original three slices (efe artifact).
        commands = {
            'Foundation': '2d00000028000000020000002d6672616d65776f726b00466f756e646174696f6e00000000000000',
            'CFNetwork': '2d00000028000000020000002d6672616d65776f726b0043464e6574776f726b0000000000000000',
            'Security': '2d00000020000000020000002d6672616d65776f726b00536563757269747900',
            'CoreFoundation': '2d00000028000000020000002d6672616d65776f726b00436f7265466f756e646174696f6e000000',
        }
        for target in ('ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'):
            raw = fixtures.mach_object(target, load_commands=[bytes.fromhex(value) for value in commands.values()])
            observed = parser.macho(raw, helper)
            actual = [row for row in observed['loadCommands'] if row['command'] == 0x2d]
            self.assertEqual(len(actual), 4)
            for row, (framework, payload) in zip(actual, commands.items()):
                self.assertEqual(row['count'], 2)
                self.assertEqual(row['options'], ['-framework', framework])
                self.assertEqual(row['payloadHex'], payload)
                self.assertLess(row['paddingBytes'], 8)
        foundation = bytes.fromhex(commands['Foundation'])
        controls = []
        for count in (0, 1, 3, 0xffffffff):
            bad = bytearray(foundation); struct.pack_into('<I', bad, 8, count); controls.append(bytes(bad))
        bad = bytearray(foundation); bad[-1] = 1; controls.append(bytes(bad))
        bad = bytearray(foundation); bad[12] = 0xff; controls.append(bytes(bad))
        controls.append(struct.pack('<3I', 0x2d, 40, 2) + b'x' * 28)  # no NUL
        for option in [b'-L\0Foundation\0', b'-framework\0/private/Foundation\0', b'-framework\0Unknown\0',
                       b'-framework\0../Foundation\0', b'-framework\0\0']:
            size = (12 + len(option) + 7) // 8 * 8
            controls.append(struct.pack('<3I', 0x2d, size, 2) + option + bytes(size - 12 - len(option)))
        controls.append(foundation + bytes(8))
        enlarged = bytearray(foundation + bytes(8)); struct.pack_into('<I', enlarged, 4, 48)
        controls.append(bytes(enlarged))
        for command in controls:
            with self.subTest(command=command.hex()), self.assertRaises(RuntimeError):
                parser.macho(fixtures.mach_object('ios-arm64', load_commands=[command]), helper)
        with self.assertRaisesRegex(RuntimeError, 'object/size'):
            parser.macho(fixtures.mach_object('ios-arm64', kind=2, load_commands=[foundation]), helper)
        unknown = bytearray(foundation); struct.pack_into('<I', unknown, 0, 0x7ffffffe)
        with self.assertRaisesRegex(RuntimeError, 'unsupported load command'):
            parser.macho(fixtures.mach_object('ios-arm64', load_commands=[bytes(unknown)]), helper)


class AppleCacheIdentityTests(unittest.TestCase):
    @staticmethod
    def pin(value):
        raw = value.encode()
        return {'bytes': len(raw), 'sha256': apple_cache.sha(raw)}

    def inputs(self):
        return {'tools': {'xcode': 'Xcode 27.1\nBuild version selected',
            'App': {'candidate': 'a' * 40,
                    'files': {name: self.pin(name) for name in apple_cache.PRODUCER_FILES}}},
            'rustc': 'rustc 1.98.0\ncommit-hash: actual-rustc', 'cargo': 'cargo 1.98.0 (actual-cargo)',
            'tool_pins': {name: self.pin(name + ' executable') for name in ('cargo', 'rustc')},
            'config': {name: self.pin(name) for name in ('Cargo.lock', 'Cargo.toml',
                'src-tauri/Cargo.toml', 'crates/new-dependency/Cargo.toml',
                '.cargo/config.toml', 'src-tauri/.cargo/config', 'rust-toolchain.toml',
                '.github/workflows/apple.yml', 'src-tauri/tauri.conf.json', 'src-tauri/tauri.ios.conf.json')},
            'environment': {}}

    def test_only_reviewed_android_only_files_are_omitted_without_editing_provenance(self):
        inputs = self.inputs(); baseline = apple_cache.cache_identities(**inputs)
        self.assertEqual(apple_cache.APPLE_RUST_UNRELATED_CODE, frozenset({
            'scripts/mobile-core/android_sdk.py', 'scripts/libbox-patches/prepare-ndk-linux.py',
            'scripts/libbox-patches/ndk-linux-archive.json'}))
        for name in apple_cache.APPLE_RUST_UNRELATED_CODE:
            with self.subTest(name=name):
                changed = deepcopy(inputs)
                changed['tools']['App']['files'][name] = self.pin('changed Android-only bytes')
                original = deepcopy(changed['tools']['App'])
                self.assertEqual(apple_cache.cache_identities(**changed), baseline)
                self.assertEqual(changed['tools']['App'], original)
        projected = apple_cache.apple_rust_build_code(inputs['tools']['App'])
        self.assertEqual(set(projected['files']), set(apple_cache.PRODUCER_FILES) -
                         apple_cache.APPLE_RUST_UNRELATED_CODE)
        self.assertEqual(projected['policy'], apple_cache.APPLE_RUST_BUILD_CODE_POLICY)
        name = 'scripts/mobile-core/apple_rust.py'
        projected['files'][name]['bytes'] += 1
        self.assertEqual(inputs['tools']['App']['files'][name], self.pin(name))

    def test_shared_apple_helpers_and_unknown_producer_files_still_invalidate_dependencies(self):
        inputs = self.inputs(); dependency, registry = apple_cache.cache_identities(**inputs)
        # In particular ci_android.prepare_source and android-source's Mach-O
        # parser are shared Apple inputs despite their misleading basenames.
        names = (set(apple_cache.PRODUCER_FILES) - apple_cache.APPLE_RUST_UNRELATED_CODE) | {
            'scripts/mobile-core/future-producer.py'}
        for name in names:
            with self.subTest(name=name):
                changed = deepcopy(inputs)
                changed['tools']['App']['files'][name] = self.pin('changed producer')
                new_dependency, new_registry = apple_cache.cache_identities(**changed)
                self.assertNotEqual(new_dependency, dependency)
                self.assertEqual(new_registry, registry)

    def test_projection_rejects_incomplete_inventory_bad_pins_and_unknown_policy(self):
        original = self.inputs()['tools']['App']
        missing = deepcopy(original); del missing['files']['scripts/mobile-core/android_sdk.py']
        bad = [None, {}, {'files': []}, missing]
        for pin in ({'bytes': 0, 'sha256': 'a' * 64}, {'bytes': True, 'sha256': 'a' * 64},
                    {'bytes': 3, 'sha256': 'A' * 64}, {'bytes': 3, 'sha256': 'x'},
                    {'bytes': 3, 'sha256': 'a' * 64, 'ignored': True}):
            identity = deepcopy(original)
            identity['files']['scripts/mobile-core/android_sdk.py'] = pin
            bad.append(identity)
        unknown = deepcopy(original); unknown['files']['future.py'] = {'bytes': 1, 'sha256': 'bad'}
        bad.append(unknown)
        for identity in bad:
            with self.subTest(identity=identity), self.assertRaises(RuntimeError):
                apple_cache.apple_rust_build_code(identity)
        with self.assertRaisesRegex(RuntimeError, 'unknown Apple Rust build code policy'):
            apple_cache.apple_rust_build_code(original, policy='unreviewed')

    def test_lock_manifest_and_source_configuration_changes_invalidate_both_layers(self):
        inputs = self.inputs(); baseline = apple_cache.cache_identities(**inputs)
        names = [name for name in inputs['config'] if apple_cache.cargo_source_config(name)]
        self.assertEqual(set(names), {'Cargo.lock', 'Cargo.toml', 'src-tauri/Cargo.toml',
            'crates/new-dependency/Cargo.toml', '.cargo/config.toml', 'src-tauri/.cargo/config'})
        for name in [*names, 'crates/future/.cargo/config.toml']:
            with self.subTest(name=name):
                changed = deepcopy(inputs); changed['config'][name] = self.pin('new source/lock/config')
                actual = apple_cache.cache_identities(**changed)
                self.assertTrue(all(left != right for left, right in zip(actual, baseline)))
        for name in ('Cargo.lock', 'src-tauri/Cargo.toml'):
            changed = deepcopy(inputs); del changed['config'][name]
            with self.subTest(missing=name), self.assertRaisesRegex(RuntimeError, 'lock/config inputs missing'):
                apple_cache.cache_identities(**changed)

    def test_workflow_ios_and_toolchain_configuration_stay_in_compiled_identity(self):
        inputs = self.inputs(); dependency, registry = apple_cache.cache_identities(**inputs)
        for name in ('.github/workflows/apple.yml', 'src-tauri/tauri.conf.json',
                     'src-tauri/tauri.ios.conf.json', 'rust-toolchain.toml'):
            with self.subTest(name=name):
                changed = deepcopy(inputs); changed['config'][name] = self.pin('changed build configuration')
                new_dependency, new_registry = apple_cache.cache_identities(**changed)
                self.assertNotEqual(new_dependency, dependency)
                self.assertEqual(new_registry, registry)

    def test_actual_cargo_tools_invalidate_both_and_rust_or_xcode_only_compiled_layer(self):
        inputs = self.inputs(); dependency, registry = apple_cache.cache_identities(**inputs)
        for field in ('cargo-version', 'cargo-bytes', 'rustc-version', 'rustc-bytes', 'xcode'):
            with self.subTest(field=field):
                changed = deepcopy(inputs)
                if field.endswith('-bytes'):
                    changed['tool_pins'][field.split('-')[0]] = self.pin('different actual executable')
                elif field == 'xcode':
                    changed['tools']['xcode'] = 'different selected Xcode/SDK build'
                else:
                    changed[field.split('-')[0]] = 'different actual version'
                new_dependency, new_registry = apple_cache.cache_identities(**changed)
                self.assertNotEqual(new_dependency, dependency)
                if field.startswith('cargo-'):
                    self.assertNotEqual(new_registry, registry)
                else:
                    self.assertEqual(new_registry, registry)

    def test_rust_native_profile_and_target_flags_remain_dependency_only_inputs(self):
        inputs = self.inputs(); dependency, registry = apple_cache.cache_identities(**inputs)
        for name in ('RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
                     'CARGO_PROFILE_RELEASE_LTO', 'CARGO_TARGET_AARCH64_APPLE_IOS_RUSTFLAGS',
                     'CFLAGS', 'CFLAGS_aarch64_apple_ios', 'CXXFLAGS_aarch64_apple_darwin',
                     'OBJC_INCLUDE_PATH_aarch64_apple_ios'):
            with self.subTest(name=name):
                changed = deepcopy(inputs); changed['environment'][name] = 'changed flag or SDK path'
                new_dependency, new_registry = apple_cache.cache_identities(**changed)
                self.assertNotEqual(new_dependency, dependency)
                self.assertEqual(new_registry, registry)

    def test_registry_source_overrides_invalidate_both_without_hashing_credentials(self):
        inputs = self.inputs(); baseline = apple_cache.cache_identities(**inputs)
        for name in ('CARGO_REGISTRY_DEFAULT', 'CARGO_REGISTRY_INDEX',
                     'CARGO_REGISTRIES_PRIVATE_INDEX', 'CARGO_REGISTRIES_CRATES_IO_PROTOCOL'):
            with self.subTest(name=name):
                changed = deepcopy(inputs); changed['environment'][name] = 'changed source setting'
                actual = apple_cache.cache_identities(**changed)
                self.assertTrue(all(left != right for left, right in zip(actual, baseline)))
        changed = deepcopy(inputs)
        changed['environment'] = {'CARGO_REGISTRY_TOKEN': 'fixture-token',
            'CARGO_REGISTRIES_PRIVATE_TOKEN': 'fixture-private-token',
            'CARGO_REGISTRIES_PRIVATE_CREDENTIAL_PROVIDER': 'fixture-provider'}
        self.assertEqual(apple_cache.registry_source_overrides(changed['environment']), {})
        self.assertEqual(apple_cache.cache_identities(**changed), baseline)

    def test_candidate_run_scope_and_private_target_path_are_not_download_or_object_inputs(self):
        inputs = self.inputs(); baseline = apple_cache.cache_identities(**inputs)
        changed = deepcopy(inputs); changed['tools']['App']['candidate'] = 'b' * 40
        changed['environment'] = {'POLARIS_APPLE_SCOPE': 'device-and-simulator',
            'GITHUB_RUN_ID': '999', 'GITHUB_RUN_ATTEMPT': '3', 'CARGO_TARGET_DIR': '/other/run/cache'}
        self.assertEqual(apple_cache.cache_identities(**changed), baseline)
        # Compiled transport still binds scope/slot independently of lookup inputs.
        first = {'identity': baseline[0], 'scope': scope_fields('device-only')}
        second = {'identity': baseline[0], 'scope': scope_fields('device-and-simulator')}
        self.assertNotEqual(apple_cache.manifest(first, 'device-arm64'),
                            apple_cache.manifest(second, 'device-arm64'))

    def test_prepare_emits_separate_keys_and_ignores_unrelated_tracked_test_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve(); source = base / 'checkout'; source.mkdir()
            root = base / 'run'; root.mkdir(); home = base / 'home'; (home / '.cargo').mkdir(parents=True)
            inputs = self.inputs(); tools = inputs['tools']; tools['rustc'] = inputs['rustc']
            for name in inputs['config']:
                path = source / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text(name)
            paths = {}
            for name in ('cargo', 'rustc'):
                paths[name] = base / name; paths[name].write_text(name + ' actual executable')
            tracked = [*inputs['config'], 'scripts/mobile-core/test_apple_entry.py',
                       'scripts/mobile-core/android_sdk.py', 'ui/test-fixture.ts']
            def run(command, **kwargs):
                key = tuple(command)
                values = {('git', 'check-ignore', '--', apple_cache.STAGE.as_posix()): apple_cache.STAGE.as_posix(),
                    ('git', 'ls-files', '-z'): '\0'.join(tracked) + '\0', ('rustc', '-vV'): inputs['rustc'],
                    ('cargo', '-V'): inputs['cargo']}
                values.update({('rustup', 'which', name): str(path) for name, path in paths.items()})
                self.assertIn(key, values, 'cache preparation must not compile or fetch')
                return values[key]
            env = {'RUNNER_TEMP': str(base), 'GITHUB_OUTPUT': str(base / 'outputs')}
            original = deepcopy(tools)
            with mock.patch.dict(os.environ, env, clear=True), \
                    mock.patch.object(apple_cache, 'ROOT', source), \
                    mock.patch.object(apple_cache, 'location', return_value=root), \
                    mock.patch.object(apple_cache.Path, 'home', return_value=home), \
                    mock.patch.object(apple_cache, 'read_receipt', return_value=tools), \
                    mock.patch.object(apple_cache, 'run', side_effect=run), \
                    mock.patch.object(apple_cache, 'context', return_value={'candidate': 'a' * 40}), \
                    mock.patch.object(apple_cache, 'check_space', return_value=40 * 1024 ** 3), \
                    mock.patch.object(apple_cache, 'note') as note:
                apple_cache.prepare()
            emitted = dict(line.split('=', 1) for line in (base / 'outputs').read_text().splitlines())
            self.assertEqual(set(emitted), {'identity', 'registry_identity'})
            self.assertNotEqual(emitted['identity'], emitted['registry_identity'])
            owner = json.loads((source / apple_cache.STAGE / 'owner.json').read_text())
            self.assertEqual(owner['identity'], emitted['identity'])
            self.assertEqual(owner['registryIdentity'], emitted['registry_identity'])
            self.assertEqual(note.call_args.kwargs['registryIdentity'], emitted['registry_identity'])
            self.assertEqual(tools, original)

    def test_workflow_uses_exact_independent_versioned_keys_without_restore_prefixes(self):
        workflow = (apple_cache.ROOT / '.github/workflows/apple.yml').read_text()
        registry = 'key: apple-registry-v2-${{ runner.os }}-${{ runner.arch }}-${{ steps.rust_cache.outputs.registry_identity }}'
        self.assertEqual(workflow.count(registry), 1)
        for target in ('aarch64-apple-ios', 'aarch64-apple-ios-sim'):
            key = 'key: apple-rust-deps-v2-${{ runner.os }}-${{ runner.arch }}-${{ env.POLARIS_APPLE_SCOPE }}-' + \
                  target + '-release-${{ steps.rust_cache.outputs.identity }}'
            self.assertEqual(workflow.count(key), 1)
        for name in ('rust_registry_restore', 'rust_device_restore', 'rust_sim_restore'):
            step = workflow.split('id: ' + name + '\n', 1)[1].split('\n      - ', 1)[0]
            self.assertNotIn('restore-keys:', step)
        self.assertNotIn('apple-rust-deps-v1-', workflow)
        self.assertNotIn('apple-registry-v1-', workflow)


class AppleDependencyCacheTests(unittest.TestCase):
    def test_actual_app_package_and_library_outputs_are_never_cache_dependencies(self):
        for name in ('polaris-0123456789abcdef', 'libpolaris_lib-0123456789abcdef.a',
                     'polaris_lib-0123456789abcdef.d', 'libpolaris_lib.a', 'polaris.d'):
            self.assertIsNotNone(apple_cache.APP_ENTRY.match(name), name)
        for name in ('serde-0123456789abcdef', 'libserde-0123456789abcdef.rlib',
                     'polaris-config-engine-0123456789abcdef', 'libpolaris_config_engine-0123456789abcdef.rlib'):
            self.assertIsNone(apple_cache.APP_ENTRY.match(name), name)

    def test_tree_allows_only_complete_internal_hardlink_alias_sets(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            work = root / 'build'; work.mkdir()
            first = work / 'build_script_build-0123456789abcdef'; first.write_bytes(b'host build executable')
            os.link(first, work / 'build-script-build')
            self.assertEqual(apple_cache.checked_tree(work), (2 * first.stat().st_size, 3))
            os.link(first, root / 'external-alias')
            with self.assertRaises(apple_cache.ExternalCacheAlias):
                apple_cache.checked_tree(work)

    def test_tree_rejects_symlinks_special_files_and_byte_or_entry_overflow(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            data = root / 'data'; data.write_bytes(b'four')
            with self.assertRaisesRegex(RuntimeError, 'bytes'):
                apple_cache.checked_tree(root, maximum=3)
            with mock.patch.object(apple_cache, 'MAX_ENTRIES', 1):
                with self.assertRaisesRegex(RuntimeError, 'count'):
                    apple_cache.checked_tree(root)
            link = root / 'link'; link.symlink_to(data)
            with self.assertRaisesRegex(RuntimeError, 'symlink'):
                apple_cache.checked_tree(root)
            link.unlink()
            fifo = root / 'pipe'; os.mkfifo(fifo)
            with self.assertRaisesRegex(RuntimeError, 'regular'):
                apple_cache.checked_tree(root)

    def test_rename_failure_rolls_back_without_copy_or_overwrite(self):
        for failure in (OSError('interrupted rename'), OSError(errno.EXDEV, 'cross-device move')):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp).resolve()
                source = root / 'source'; source.mkdir()
                dest = root / 'stage'; dest.mkdir()
                one, two = source / 'one', source / 'two'
                one.write_bytes(b'first'); two.write_bytes(b'second')
                original = os.rename
                def rename(old, new):
                    if old == two:
                        raise failure
                    return original(old, new)
                with mock.patch.object(apple_cache.os, 'rename', side_effect=rename):
                    with self.assertRaises(OSError):
                        apple_cache.transfer([(one, dest / 'one'), (two, dest / 'two')])
                self.assertEqual(one.read_bytes(), b'first')
                self.assertEqual(two.read_bytes(), b'second')
                self.assertEqual(list(dest.iterdir()), [])
                (dest / 'two').write_bytes(b'existing bytes')
                with mock.patch.object(apple_cache.os, 'rename') as move:
                    with self.assertRaisesRegex(RuntimeError, 'already exists'):
                        apple_cache.transfer([(one, dest / 'one'), (two, dest / 'two')])
                    move.assert_not_called()
                self.assertEqual((dest / 'two').read_bytes(), b'existing bytes')

    def fixture(self, base):
        source = base / 'checkout'; source.mkdir()
        root = base / 'run-11-2'; root.mkdir(mode=0o700)
        cache = root / 'cache'; cache.mkdir(mode=0o700)
        stage = source / apple_cache.STAGE; stage.mkdir(mode=0o700, parents=True)
        context = {'candidate': 'a' * 40, 'runId': '11', 'runAttempt': '2'}
        owner = {'context': context, 'scope': scope_fields(), 'identity': 'b' * 64,
                 'actualCargoTargetDir': str(root / 'cache/rust-target')}
        (stage / 'owner.json').write_text(json.dumps(owner))
        target = root / 'cache/rust-target/aarch64-apple-ios/release'
        deps = target / 'deps'; deps.mkdir(parents=True, mode=0o755)
        deps.chmod(0o755)
        dep = deps / 'libserde-0123456789abcdef.rlib'; dep.write_bytes(b'compiled dependency')
        app = deps / 'libpolaris_lib-0123456789abcdef.a'; app.write_bytes(b'App native archive')
        final = target / 'libpolaris_lib.a'; final.write_bytes(b'original final staticlib')
        fingerprint = target / '.fingerprint/polaris-0123456789abcdef'
        fingerprint.mkdir(parents=True); (fingerprint / 'lib-polaris_lib').write_bytes(b'App fingerprint')
        host = root / 'cache/rust-target/release/build/serde-0123456789abcdef'
        host.mkdir(parents=True)
        build = host / 'build_script_build-0123456789abcdef'; build.write_bytes(b'host compiler output')
        os.link(build, host / 'build-script-build')
        original = root / 'apple-original-device-arm64-evidence.zip'; original.write_bytes(b'immutable retained original')
        (root / 'evidence-device-arm64-pin.json').write_text(json.dumps({
            'slot': 'device-arm64', 'context': context, 'archive': transport_pin(original), 'receiptReadErrors': {}}))
        summary = root / 'summaries/device-arm64'; summary.mkdir(parents=True)
        (summary / 'carrier-device-arm64.json').write_text('{}')
        return source, root, stage, context, dep, app, final, host

    def patched(self, source, root, context):
        return (mock.patch.object(apple_cache, 'ROOT', source),
                mock.patch.object(apple_cache, 'location', return_value=root),
                mock.patch.object(apple_cache, 'context', return_value=context),
                mock.patch.object(apple_cache, 'check_space', return_value=40 * 1024 ** 3))

    def test_roundtrip_preserves_original_app_bytes_inode_aliases_and_native_directory_mode(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            inode = dep.stat().st_ino
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                apple_cache.export_cache('device-arm64')
                self.assertFalse(dep.exists())
                self.assertEqual(app.read_bytes(), b'App native archive')
                self.assertEqual(final.read_bytes(), b'original final staticlib')
                self.assertFalse(any(p.name.startswith('libpolaris_lib') for p in (stage / 'device-arm64').rglob('*')))
                self.assertEqual((stage / 'device-arm64/host/build/serde-0123456789abcdef/build-script-build').stat().st_nlink, 2)
                apple_cache.import_cache('device-arm64', 'staged')
                observation = json.loads((root / 'apple-cache-return-device-arm64.json').read_text())
                self.assertEqual(observation['cacheState'], 'local-staged-return')
            self.assertEqual(dep.stat().st_ino, inode)
            self.assertEqual(dep.parent.stat().st_mode & 0o777, 0o755)
            self.assertEqual((host / 'build-script-build').stat().st_nlink, 2)
            self.assertEqual(app.read_bytes(), b'App native archive')
            self.assertEqual(final.read_bytes(), b'original final staticlib')
            self.assertFalse((stage / 'device-arm64').exists())

    def test_external_dependency_alias_stays_native_and_is_explicitly_reported_uncached(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            os.link(dep, root / 'external-dependency-alias')
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                apple_cache.export_cache('device-arm64')
                observation = json.loads((root / 'apple-cache-export-device-arm64.json').read_text())
                self.assertEqual(observation['uncachedExternalAliasEntries'],
                                 ['aarch64-apple-ios/release/deps/' + dep.name])
                self.assertTrue(dep.exists())
                apple_cache.import_cache('device-arm64', 'hit')
            self.assertEqual(dep.read_bytes(), b'compiled dependency')

    def test_import_refuses_cross_scope_identity_extra_roles_app_products_and_false_hits(self):
        for mutation in ('scope', 'identity', 'extra-role', 'app', 'unexpected-miss'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                p1, p2, p3, p4 = self.patched(source, root, context)
                with p1, p2, p3, p4:
                    apple_cache.export_cache('device-arm64')
                    payload = stage / 'device-arm64'
                    if mutation in ('scope', 'identity'):
                        marker = payload / 'cache.json'; row = json.loads(marker.read_text())
                        row[mutation] = 'wrong'
                        marker.write_text(json.dumps(row))
                    elif mutation == 'extra-role':
                        (payload / 'simulator').mkdir()
                    elif mutation == 'app':
                        (payload / 'target/deps/libpolaris_lib-1111111111111111.a').write_bytes(b'never reuse App')
                    with self.assertRaises(RuntimeError):
                        apple_cache.import_cache('device-arm64', 'miss' if mutation == 'unexpected-miss' else 'hit')
                    self.assertFalse(dep.exists())
                    self.assertEqual(app.read_bytes(), b'App native archive')
                    self.assertEqual(final.read_bytes(), b'original final staticlib')

    def test_low_space_wrong_owner_and_active_cache_lock_refuse_before_moves(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            with mock.patch.object(apple_cache.os, 'statvfs', return_value=mock.Mock(f_bavail=0, f_frsize=4096)), \
                    mock.patch.object(apple_cache.os, 'rename') as move:
                with self.assertRaisesRegex(RuntimeError, 'headroom'):
                    apple_cache.check_space(root, 1024)
                move.assert_not_called()
            wrong_owner = os.getuid() + 1
            with mock.patch.object(apple_cache.os, 'getuid', return_value=wrong_owner):
                with self.assertRaisesRegex(RuntimeError, 'ownership'):
                    apple_cache.checked_tree(dep)
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4, \
                    mock.patch.object(apple_cache, 'slot_locks', side_effect=BlockingIOError('active native cache')), \
                    mock.patch.object(apple_cache.os, 'rename') as move:
                with self.assertRaisesRegex(BlockingIOError, 'active native cache'):
                    apple_cache.export_cache('device-arm64')
                move.assert_not_called()
            self.assertEqual(dep.read_bytes(), b'compiled dependency')
            self.assertEqual(app.read_bytes(), b'App native archive')
            self.assertEqual(final.read_bytes(), b'original final staticlib')
            self.assertFalse((stage / 'device-arm64').exists())

    def test_export_requires_preserved_originals_and_never_accepts_unrequested_simulator(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            (root / 'apple-original-device-arm64-evidence.zip').write_bytes(b'changed original')
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                with self.assertRaisesRegex(RuntimeError, 'preserved'):
                    apple_cache.export_cache('device-arm64')
                self.assertTrue(dep.exists())
                self.assertFalse((stage / 'device-arm64').exists())
                with self.assertRaisesRegex(RuntimeError, 'unrequested'):
                    apple_cache.import_cache('sim-arm64', 'miss')
                with self.assertRaisesRegex(RuntimeError, 'no dependency payload'):
                    apple_cache.import_cache('device-arm64', 'hit')
                apple_cache.import_cache('device-arm64', 'miss')


    def test_local_export_leaves_whole_nested_symlink_entry_and_returns_safe_original_inodes(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            build = dep.parent.parent / 'build'; build.mkdir()
            skipped = build / 'linked-crate-0123456789abcdef'
            (skipped / 'out').mkdir(parents=True)
            sibling = skipped / 'ordinary-output'; sibling.write_bytes(b'keep the whole dependency entry')
            link = skipped / 'out/link'; link.symlink_to('../ordinary-output')
            safe = build / 'safe-crate-0123456789abcdef'; safe.mkdir()
            (safe / 'output').write_bytes(b'cache this complete dependency')
            originals = [app, final, root / 'apple-original-device-arm64-evidence.zip',
                         root / 'evidence-device-arm64-pin.json',
                         root / 'summaries/device-arm64/carrier-device-arm64.json']
            preserved = {p: (p.read_bytes(), p.stat().st_ino, p.stat().st_mode) for p in originals}
            inodes = {p: p.stat().st_ino for p in (dep, safe, safe / 'output', sibling)}
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                apple_cache.export_cache('device-arm64')
                self.assertTrue(link.is_symlink()); self.assertEqual(sibling.read_bytes(), b'keep the whole dependency entry')
                self.assertFalse(dep.exists()); self.assertFalse(safe.exists())
                self.assertFalse((stage / 'device-arm64/target/build' / skipped.name).exists())
                observation = json.loads((root / 'apple-cache-export-device-arm64.json').read_text())
                self.assertEqual(observation['uncachedSymlinkEntries'],
                                 [str(skipped.relative_to(root / 'cache/rust-target'))])
                self.assertEqual(observation['uncachedSymlinkEntryCount'], 1)
                self.assertEqual(observation['uncachedSymlinkSampleLimit'], 64)
                self.assertIs(observation['uncachedSymlinkTruncated'], False)
                self.assertIs(observation['carrierAccepted'], False); self.assertIs(observation['devicePassed'], False)
                self.assertEqual({p: (p.read_bytes(), p.stat().st_ino, p.stat().st_mode) for p in originals}, preserved)
                apple_cache.import_cache('device-arm64', 'staged')
            self.assertEqual({p: p.stat().st_ino for p in inodes}, inodes)
            self.assertEqual({p: (p.read_bytes(), p.stat().st_ino, p.stat().st_mode) for p in originals}, preserved)
            self.assertTrue(link.is_symlink()); self.assertFalse((stage / 'device-arm64').exists())

    def test_local_top_and_nested_links_never_dereference_internal_external_dangling_or_cycle_targets(self):
        for nested in (False, True):
            for kind in ('internal', 'external', 'dangling', 'cycle'):
                with self.subTest(nested=nested, kind=kind), tempfile.TemporaryDirectory() as tmp, \
                        mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                    source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                    external = root / 'private-link-target'; external.write_bytes(b'do not inspect target contents')
                    if nested:
                        candidate = dep.parent.parent / 'build/linked-crate-0123456789abcdef'
                        (candidate / 'out').mkdir(parents=True)
                        (candidate / 'sibling').write_bytes(b'uncached sibling')
                        link = candidate / 'out/link'
                        internal = '../sibling'
                    else:
                        candidate = dep.parent / 'liblinked-0123456789abcdef.rlib'
                        link = candidate; internal = app.name
                    target = {'internal': internal, 'external': str(external),
                              'dangling': str(root / 'missing-target'), 'cycle': link.name}[kind]
                    link.symlink_to(target)
                    real_resolve, real_scandir, real_stat = Path.resolve, os.scandir, os.stat
                    def guarded_resolve(path, *args, **kwargs):
                        if path in (candidate, link, external):
                            raise AssertionError('local candidate or link target was resolved')
                        return real_resolve(path, *args, **kwargs)
                    def guarded_scandir(path):
                        if Path(path) in (link, external):
                            raise AssertionError('link target was enumerated')
                        return real_scandir(path)
                    def guarded_stat(path, *args, **kwargs):
                        if Path(path) in (link, external) and kwargs.get('follow_symlinks', True):
                            raise AssertionError('link target was stat-ed')
                        return real_stat(path, *args, **kwargs)
                    with mock.patch.object(Path, 'resolve', guarded_resolve), \
                            mock.patch.object(apple_cache.os, 'scandir', side_effect=guarded_scandir), \
                            mock.patch.object(apple_cache.os, 'stat', side_effect=guarded_stat), \
                            mock.patch.object(apple_cache.os, 'readlink', side_effect=AssertionError('link target read')), \
                            mock.patch.object(Path, 'open', side_effect=AssertionError('file contents read')), \
                            self.assertRaises(apple_cache.UncacheableSymlink) as rejected:
                        apple_cache.checked_tree(candidate, local_export=True)
                    self.assertEqual(rejected.exception.entries, 4 if nested else 1)
                    self.assertEqual(str(rejected.exception), 'cache symlink entry left uncached')
                    p1, p2, p3, p4 = self.patched(source, root, context)
                    with p1, p2, p3, p4:
                        apple_cache.export_cache('device-arm64')
                    self.assertTrue(link.is_symlink())
                    self.assertEqual(external.read_bytes(), b'do not inspect target contents')
                    observation = json.loads((root / 'apple-cache-export-device-arm64.json').read_text())
                    self.assertEqual(observation['uncachedSymlinkEntryCount'], 1)
                    self.assertEqual(observation['uncachedSymlinkEntries'],
                                     [str(candidate.relative_to(root / 'cache/rust-target'))])
                    if nested:
                        self.assertEqual((candidate / 'sibling').read_bytes(), b'uncached sibling')

    def test_external_alias_and_symlink_skips_coexist_without_relaxing_internal_hardlinks(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            os.link(dep, root / 'external-dependency-alias')
            skipped = dep.parent / 'liblinked-0123456789abcdef.rlib'; skipped.symlink_to(app.name)
            original_inode = (host / 'build-script-build').stat().st_ino
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                apple_cache.export_cache('device-arm64')
                row = json.loads((root / 'apple-cache-export-device-arm64.json').read_text())
                self.assertEqual(row['uncachedExternalAliasCount'], 1)
                self.assertEqual(row['uncachedExternalAliasEntries'],
                                 [str(dep.relative_to(root / 'cache/rust-target'))])
                self.assertIs(row['uncachedExternalAliasTruncated'], False)
                self.assertEqual(row['uncachedSymlinkEntryCount'], 1)
                self.assertEqual(row['uncachedSymlinkEntries'],
                                 [str(skipped.relative_to(root / 'cache/rust-target'))])
                staged = stage / 'device-arm64/host/build' / host.name
                self.assertEqual((staged / 'build-script-build').stat().st_ino, original_inode)
                self.assertEqual((staged / 'build-script-build').stat().st_nlink, 2)
                self.assertTrue(dep.exists()); self.assertTrue(skipped.is_symlink())
                apple_cache.import_cache('device-arm64', 'staged')
            self.assertEqual((host / 'build-script-build').stat().st_ino, original_inode)
            self.assertEqual(dep.read_bytes(), b'compiled dependency')
            self.assertEqual(app.read_bytes(), b'App native archive')

    def test_restored_hit_staged_return_and_registry_symlinks_remain_strict_before_transfer(self):
        for restored in ('hit', 'staged'):
            for position in ('payload', 'top', 'nested'):
                with self.subTest(restored=restored, position=position), tempfile.TemporaryDirectory() as tmp, \
                        mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                    source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                    p1, p2, p3, p4 = self.patched(source, root, context)
                    with p1, p2, p3, p4:
                        apple_cache.export_cache('device-arm64')
                        payload = stage / 'device-arm64'
                        if position == 'payload':
                            payload.rename(stage / 'held-payload'); payload.symlink_to('held-payload')
                        elif position == 'top':
                            (payload / 'injected-link').symlink_to(app)
                        else:
                            (payload / 'host/build' / host.name / 'injected-link').symlink_to(app)
                        with mock.patch.object(apple_cache, 'transfer') as transfer, \
                                mock.patch.object(apple_cache.os, 'rename') as move, \
                                self.assertRaisesRegex(RuntimeError, 'symlink|redirected'):
                            apple_cache.import_cache('device-arm64', restored)
                        transfer.assert_not_called(); move.assert_not_called()
                    self.assertFalse(dep.exists()); self.assertEqual(app.read_bytes(), b'App native archive')
                    self.assertEqual(final.read_bytes(), b'original final staticlib')
        for position in ('root', 'nested'):
            with self.subTest(registry=position), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp).resolve(); home = root / 'home'; home.mkdir()
                folder = home / '.cargo/registry/index'; folder.mkdir(parents=True)
                (folder / 'ordinary').write_bytes(b'registry bytes')
                if position == 'root':
                    folder.rename(home / 'held-index'); folder.symlink_to(home / 'held-index')
                else:
                    (folder / 'injected-link').symlink_to(root / 'missing-registry-target')
                with mock.patch.object(Path, 'home', return_value=home), \
                        mock.patch.object(apple_cache, 'location', return_value=root), \
                        mock.patch.object(apple_cache, 'check_space') as space, \
                        mock.patch.object(apple_cache, 'note') as note, \
                        self.assertRaisesRegex(RuntimeError, 'symlink|redirected'):
                    apple_cache.registry_ready()
                space.assert_not_called(); note.assert_not_called()

    def test_export_redirected_stage_native_ancestor_base_and_group_are_hard_failures(self):
        for position in ('stage', 'native-ancestor', 'native-base', 'group', 'dangling-group'):
            with self.subTest(position=position), tempfile.TemporaryDirectory() as tmp, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                redirected = {'stage': stage, 'native-ancestor': root / 'cache/rust-target',
                              'native-base': dep.parent.parent, 'group': dep.parent,
                              'dangling-group': dep.parent}[position]
                retained = redirected.with_name(redirected.name + '-retained')
                redirected.rename(retained)
                redirected.symlink_to(retained if position != 'dangling-group' else retained.with_name('missing-group'))
                p1, p2, p3, p4 = self.patched(source, root, context)
                with p1, p2, p3, p4, mock.patch.object(apple_cache, 'transfer') as transfer, \
                        mock.patch.object(apple_cache.os, 'rename') as move, self.assertRaises(RuntimeError) as rejected:
                    apple_cache.export_cache('device-arm64')
                self.assertNotIsInstance(rejected.exception, apple_cache.UncacheableSymlink)
                self.assertNotIsInstance(rejected.exception, apple_cache.ExternalCacheAlias)
                self.assertIn('redirect', str(rejected.exception))
                transfer.assert_not_called(); move.assert_not_called()
                self.assertFalse((stage / 'device-arm64').exists())

    def test_local_symlink_classification_never_masks_owner_special_alias_change_or_tree_limits(self):
        from contextlib import ExitStack, contextmanager
        import socket
        import stat
        for failure in ('parent-owner', 'parent-type', 'child-owner', 'fifo', 'socket', 'bytes', 'count', 'alias-change'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
                root = Path(tmp).resolve(); candidate = root / 'candidate'; candidate.mkdir()
                bad = candidate / 'ordinary'
                if failure == 'fifo':
                    os.mkfifo(bad)
                elif failure == 'socket':
                    endpoint = held.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)); endpoint.bind(str(bad))
                else:
                    bad.write_bytes(b'four')
                if failure == 'alias-change':
                    os.link(bad, candidate / 'second')
                (candidate / 'link').symlink_to('unread-dangling-target')
                original_scan, original_lstat = os.scandir, Path.lstat
                @contextmanager
                def links_last(path):
                    with original_scan(path) as scanned:
                        children = list(scanned)
                    # checked_tree uses a LIFO queue: encounter the link first,
                    # then prove the remaining ordinary siblings still get checked.
                    yield iter(sorted(children, key=lambda row: row.name == 'link'))
                def observed_lstat(path):
                    info = original_lstat(path); values = list(info)
                    if failure == 'parent-owner' and path == root or failure == 'child-owner' and path == bad:
                        values[4] = os.getuid() + 1
                    if failure == 'parent-type' and path == root:
                        values[0] = stat.S_IFREG | 0o600
                    if failure == 'alias-change' and path == candidate / 'second':
                        values[3] += 1
                    return os.stat_result(values)
                with mock.patch.object(apple_cache.os, 'scandir', side_effect=links_last), \
                        mock.patch.object(Path, 'lstat', observed_lstat), \
                        mock.patch.object(apple_cache, 'MAX_ENTRIES', 2 if failure == 'count' else apple_cache.MAX_ENTRIES), \
                        self.assertRaises(RuntimeError) as rejected:
                    apple_cache.checked_tree(candidate, maximum=3 if failure == 'bytes' else apple_cache.MAX_BYTES,
                                             local_export=True)
                self.assertNotIsInstance(rejected.exception, apple_cache.UncacheableSymlink)
                self.assertNotIsInstance(rejected.exception, apple_cache.ExternalCacheAlias)
                expected = {'parent-owner': 'ownership', 'parent-type': 'ownership/type', 'child-owner': 'ownership',
                            'fifo': 'regular', 'socket': 'regular', 'bytes': 'bytes', 'count': 'count',
                            'alias-change': 'links changed'}[failure]
                self.assertIn(expected, str(rejected.exception))
                self.assertTrue((candidate / 'link').is_symlink())

    def test_skips_share_global_scan_budget_and_all_uncacheable_sources_never_create_payload(self):
        for failure in ('combined-count', 'combined-bytes', 'all-skipped'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                if failure == 'all-skipped':
                    os.link(dep, root / 'external-dependency-alias')
                    (host / 'link').symlink_to('missing-target')
                else:
                    for name in ('aaa-skipped', 'bbb-skipped'):
                        entry = host.parent / name; (entry / 'out').mkdir(parents=True)
                        (entry / 'sibling').write_bytes(b'keep local')
                        (entry / 'out/link').symlink_to('../sibling')
                p1, p2, p3, p4 = self.patched(source, root, context)
                with p1, p2, p3, p4, \
                        mock.patch.object(apple_cache, 'MAX_ENTRIES', 8 if failure == 'combined-count' else apple_cache.MAX_ENTRIES), \
                        mock.patch.object(apple_cache, 'MAX_BYTES', 1 if failure == 'combined-bytes' else apple_cache.MAX_BYTES), \
                        mock.patch.object(apple_cache, 'transfer') as transfer, \
                        mock.patch.object(apple_cache.os, 'rename') as move, self.assertRaises(RuntimeError) as rejected:
                    apple_cache.export_cache('device-arm64')
                reason = {'combined-count': 'examined cache entries exceed bound',
                          'combined-bytes': 'combined dependency cache exceeds bound',
                          'all-skipped': 'no dependency entries available for cache export'}[failure]
                self.assertIn(reason, str(rejected.exception))
                transfer.assert_not_called(); move.assert_not_called()
                self.assertFalse((stage / 'device-arm64').exists())
                self.assertFalse((root / 'apple-cache-export-device-arm64.json').exists())
                self.assertEqual(dep.read_bytes(), b'compiled dependency')
                self.assertEqual(app.read_bytes(), b'App native archive')
                self.assertEqual(final.read_bytes(), b'original final staticlib')

    def test_uncached_symlink_and_alias_samples_are_utf8_bounded_escaped_and_single_line(self):
        from contextlib import redirect_stdout
        self.assertEqual(apple_cache.SAMPLE_LIMIT, 64); self.assertEqual(apple_cache.SAMPLE_PATH_BYTES, 1024)
        short = 'aarch64-apple-ios/release/deps/libserde-0123456789abcdef.rlib'
        samples = []
        self.assertIs(apple_cache.cache_sample(samples, short), False); self.assertEqual(samples, [short])
        samples = []; truncated = False
        for index in range(70):
            relative = 'release/build/' + ('雪' * 400 + '\n\r\x1b' if index == 0 else 'entry-' + str(index))
            truncated = apple_cache.cache_sample(samples, relative) or truncated
        self.assertIs(truncated, True); self.assertEqual(len(samples), 64)
        self.assertTrue(all(isinstance(sample, str) and len(sample.encode('utf-8')) <= 1024 for sample in samples))
        self.assertTrue(all(sample.encode('utf-8').decode('utf-8') == sample for sample in samples))
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            link_target = '/private/never-print-or-read-this-link-target'
            for index in range(70):
                suffix = str(index).zfill(3) + '-雪\n\r\x1b'
                (dep.parent / ('symlink-' + suffix)).symlink_to(link_target)
                alias = dep.parent / ('alias-' + suffix); alias.write_bytes(b'not reusable with external alias')
                os.link(alias, root / ('external-alias-' + str(index)))
            p1, p2, p3, p4 = self.patched(source, root, context)
            output = io.StringIO()
            with p1, p2, p3, p4, redirect_stdout(output):
                apple_cache.export_cache('device-arm64')
            text = output.getvalue()
            self.assertEqual(len(text.splitlines()), 1)
            self.assertNotIn('\r', text); self.assertNotIn('\x1b', text); self.assertNotIn(link_target, text)
            self.assertIn('\\n', text); self.assertIn('\\r', text); self.assertIn('\\u001b', text)
            self.assertLessEqual(len(text.encode('utf-8')), 2 * 64 * 1024 * 6 + 16384)
            prefix = 'APPLE_DEPENDENCY_CACHE='
            self.assertTrue(text.startswith(prefix))
            row = json.loads(text[len(prefix):])
            self.assertEqual(row, json.loads((root / 'apple-cache-export-device-arm64.json').read_text()))
            for label, count_key in (('Symlink', 'uncachedSymlinkEntryCount'),
                                     ('ExternalAlias', 'uncachedExternalAliasCount')):
                self.assertEqual(row[count_key], 70)
                self.assertEqual(row['uncached' + label + 'SampleLimit'], 64)
                self.assertIs(row['uncached' + label + 'Truncated'], True)
                bounded = row['uncached' + label + 'Entries']
                self.assertEqual(len(bounded), 64)
                self.assertTrue(all(isinstance(value, str) and not value.startswith('/') and
                                    len(value.encode('utf-8')) <= 1024 for value in bounded))
                self.assertNotIn(str(Path.home()), json.dumps(bounded))
            self.assertIs(row['carrierAccepted'], False); self.assertIs(row['devicePassed'], False)

    def test_export_rejects_dangling_and_empty_native_role_ancestry_before_group_shortcuts(self):
        for position in ('host-base', 'target-base', 'target-ancestor', 'shared-ancestor'):
            for destination in ('dangling', 'empty'):
                with self.subTest(position=position, destination=destination), tempfile.TemporaryDirectory() as tmp, \
                        mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                    source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                    redirected = {'host-base': host.parent.parent, 'target-base': dep.parent.parent,
                                  'target-ancestor': dep.parent.parent.parent,
                                  'shared-ancestor': root / 'cache/rust-target'}[position]
                    redirected.rename(redirected.with_name(redirected.name + '-retained'))
                    target = root / 'empty-or-missing-native-target'
                    if destination == 'empty':
                        target.mkdir()
                    redirected.symlink_to(target)
                    # In each role-specific case, the other role still has valid
                    # cache candidates: it must not hide this unsafe empty root.
                    if position == 'host-base':
                        self.assertTrue(dep.is_file())
                    elif position != 'shared-ancestor':
                        self.assertTrue((host / 'build-script-build').is_file())
                    original = root / 'apple-original-device-arm64-evidence.zip'
                    original_pin = transport_pin(original)
                    p1, p2, p3, p4 = self.patched(source, root, context)
                    with p1, p2, p3, p4, mock.patch.object(apple_cache, 'transfer') as transfer, \
                            mock.patch.object(apple_cache.os, 'rename') as move, \
                            self.assertRaisesRegex(RuntimeError, 'base/ancestry redirected') as rejected:
                        apple_cache.export_cache('device-arm64')
                    self.assertNotIsInstance(rejected.exception, apple_cache.UncacheableSymlink)
                    self.assertNotIsInstance(rejected.exception, apple_cache.ExternalCacheAlias)
                    transfer.assert_not_called(); move.assert_not_called()
                    self.assertFalse((stage / 'device-arm64').exists())
                    self.assertFalse((root / 'apple-cache-export-device-arm64.json').exists())
                    self.assertEqual(transport_pin(original), original_pin)
                    self.assertTrue(redirected.is_symlink())
                    if destination == 'empty':
                        self.assertEqual(list(target.iterdir()), [])
                    else:
                        self.assertFalse(target.exists())

    def test_export_allows_legitimately_missing_native_role_directories_without_creating_them(self):
        for position in ('host-base', 'target-base', 'target-ancestor'):
            with self.subTest(position=position), tempfile.TemporaryDirectory() as tmp, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                absent = {'host-base': host.parent.parent, 'target-base': dep.parent.parent,
                          'target-ancestor': dep.parent.parent.parent}[position]
                absent.rename(absent.with_name(absent.name + '-retained'))
                p1, p2, p3, p4 = self.patched(source, root, context)
                with p1, p2, p3, p4:
                    apple_cache.export_cache('device-arm64')
                    self.assertFalse(absent.exists()); self.assertFalse(absent.is_symlink())
                    observation = json.loads((root / 'apple-cache-export-device-arm64.json').read_text())
                    self.assertGreater(observation['movedEntries'], 0)
                    self.assertEqual(observation['uncachedSymlinkEntryCount'], 0)
                    apple_cache.import_cache('device-arm64', 'staged')
                self.assertFalse(absent.exists()); self.assertFalse(absent.is_symlink())
                self.assertFalse((stage / 'device-arm64').exists())

if __name__ == '__main__':
    unittest.main()
