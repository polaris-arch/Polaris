"""Private malformed archives; no network, native loading, or Git repositories."""
import hashlib
import gzip
import importlib.util
import io
import json
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release_archive', Path(__file__).with_name('release-archive.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
ROOT = 'polaris-box-1.15.0-alpha.12.polaris.1-linux-amd64'


class ArchiveBoundary(unittest.TestCase):
    def check(self, entries, kind, expected=None, rejected=True):
        with tempfile.TemporaryDirectory(prefix='polaris-archive-fixture-') as directory:
            archive = Path(directory) / ('native.zip' if kind == 'zip' else 'native.tar.gz')
            if kind == 'zip':
                with zipfile.ZipFile(archive, 'w') as output:
                    for name, data, mode, entry_kind in entries:
                        info = zipfile.ZipInfo(name)
                        info.external_attr = (mode | entry_kind) << 16
                        output.writestr(info, data)
            else:
                with tarfile.open(archive, 'w:gz') as output:
                    for name, data, mode, entry_kind in entries:
                        info = tarfile.TarInfo(name)
                        info.mode = mode
                        info.size = len(data)
                        info.type = entry_kind
                        info.linkname = ROOT + '/sing-box'
                        output.addfile(info, io.BytesIO(data))
            data = archive.read_bytes()
            record = {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(),
                      'members': expected if expected is not None else {
                          name: hashlib.sha256(data).hexdigest() for name, data, *_ in entries}}
            destination = Path(directory) / 'unpacked'
            if rejected:
                with self.assertRaises((ValueError, KeyError)):
                    module.extract(str(archive), str(destination), record, ROOT)
                self.assertFalse(destination.exists(), 'failed archive wrote an extraction tree')
            else:
                module.extract(str(archive), str(destination), record, ROOT)
                self.assertEqual((destination / ROOT / 'sing-box').read_bytes(), b'inert')

    def test_valid_archives_preserve_bytes_without_exec(self):
        for kind in ['zip', 'tar']:
            with self.subTest(kind=kind):
                entry_kind = stat.S_IFREG if kind == 'zip' else tarfile.REGTYPE
                self.check([(ROOT + '/sing-box', b'inert', 0o755, entry_kind)], kind, rejected=False)

    def test_unsafe_paths_duplicates_aliases_and_modes(self):
        bad_paths = ['../outside', '/absolute', ROOT + '/../../outside', ROOT + '/a\\b',
                     ROOT + '/NUL.txt', ROOT + '/COM1', ROOT + '/name.', ROOT + '/stream:ads',
                     ROOT + '//empty', ROOT + '/./dot', ROOT + '/white space', 'wrong-root/sing-box']
        for kind in ['zip', 'tar']:
            regular = stat.S_IFREG if kind == 'zip' else tarfile.REGTYPE
            for name in bad_paths:
                with self.subTest(kind=kind, path=name):
                    self.check([(name, b'inert', 0o644, regular)], kind)
            for entries in [[(ROOT + '/sing-box', b'inert', 0o755, regular)] * 2,
                            [(ROOT + '/a', b'inert', 0o644, regular), (ROOT + '/A', b'inert', 0o644, regular)],
                            [(ROOT + '/a', b'inert', 0o644, regular), (ROOT + '/a/b', b'inert', 0o644, regular)],
                            [(ROOT + '/a', b'inert', 0o644, regular), (ROOT + '/A/b', b'inert', 0o644, regular)],
                            [(ROOT + '/a/f', b'inert', 0o644, regular), (ROOT + '/A/g', b'inert', 0o644, regular)]]:
                with self.subTest(kind=kind, collision=True):
                    self.check(entries, kind)
            for mode in [0o4755, 0o2755, 0o1755, 0o777, 0o664]:
                with self.subTest(kind=kind, mode=mode):
                    self.check([(ROOT + '/sing-box', b'inert', mode, regular)], kind)

    def test_symlink_hardlink_devices_and_fifos(self):
        for entry_kind in [stat.S_IFLNK, stat.S_IFCHR, stat.S_IFIFO]:
            with self.subTest(kind='zip', type=entry_kind):
                self.check([(ROOT + '/sing-box', b'inert', 0o644, entry_kind)], 'zip')
        for entry_kind in [tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.CHRTYPE, tarfile.FIFOTYPE]:
            with self.subTest(kind='tar', type=entry_kind):
                self.check([(ROOT + '/sing-box', b'', 0o644, entry_kind)], 'tar')

    def test_missing_unexpected_and_tampered_members(self):
        for kind in ['zip', 'tar']:
            regular = stat.S_IFREG if kind == 'zip' else tarfile.REGTYPE
            entries = [(ROOT + '/sing-box', b'inert', 0o755, regular)]
            for expected in [{}, {ROOT + '/other': 'a' * 64}, {ROOT + '/sing-box': 'a' * 64},
                             {ROOT + '/sing-box': hashlib.sha256(b'inert').hexdigest(), ROOT + '/missing': 'a' * 64}]:
                with self.subTest(kind=kind, expected=expected):
                    self.check(entries, kind, expected)

    def test_outer_hash_and_size_checked_before_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'native.zip'
            archive.write_bytes(b'truncated inert')
            for record in [{'bytes': 1, 'sha256': 'a' * 64},
                           {'bytes': archive.stat().st_size, 'sha256': 'a' * 64}]:
                with self.assertRaisesRegex(ValueError, 'archive bytes differ'):
                    module.extract(str(archive), str(Path(directory) / 'out'), record, ROOT)

    def test_raw_zip_nul_name_cannot_be_truncated_to_a_reviewed_member(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'native.zip'
            name = ROOT + '/sing-boxXYZ'
            with zipfile.ZipFile(archive, 'w') as output:
                info = zipfile.ZipInfo(name)
                info.external_attr = 0o100755 << 16
                output.writestr(info, b'inert')
            original = name.encode()
            raw = archive.read_bytes().replace(original, (ROOT + '/sing-box\x00YZ').encode())
            archive.write_bytes(raw)
            record = {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
                      'members': {ROOT + '/sing-box': hashlib.sha256(b'inert').hexdigest()}}
            destination = Path(directory) / 'unpacked'
            with self.assertRaisesRegex(ValueError, 'raw NUL ZIP member name'):
                module.extract(str(archive), str(destination), record, ROOT)
            self.assertFalse(destination.exists())


class WireHandoffBoundary(unittest.TestCase):
    """Real inert transport bytes, not release receipts or native acceptance."""
    root = 'core-wire-inputs'
    platforms = ('linux', 'win', 'mac-x64', 'mac-arm64')

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='polaris-wire-archive-fixture-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.serial = 0

    def fresh(self, name):
        self.serial += 1
        return self.directory / (str(self.serial) + '-' + name)

    def files(self, counts=None, index='polaris-upstream-native-assets.json'):
        # Counts remain the complete per-platform file budget, now including
        # its two independent recovery data resources rather than enlarging it.
        counts = counts or dict.fromkeys(self.platforms, 3)
        receipts = self.root + '/resources/.release-receipts/'
        files = {self.root + '/handoff.json': (b'inert handoff', 0o600),
                 receipts + 'public-readback.json': (b'inert readback', 0o600),
                 receipts + index: (b'inert index', 0o600)}
        files.update({receipts + key + '.json': (b'inert receipt', 0o600) for key in self.platforms})
        for key in self.platforms:
            recovery = self.root + '/resources/core-recovery/' + key + '/'
            files[recovery + 'inventory.json'] = (b'inert inventory', 0o644)
            files[recovery + ('native.zip' if key == 'win' else 'native.tar.gz')] = (b'inert original archive', 0o644)
            for number in range(counts[key] - 2):
                name = ('sing-box.exe' if key == 'win' else 'sing-box') if number == 0 else (
                    'core-release/licenses/license-%04d.txt' % number)
                files[self.root + '/resources/' + key + '/' + name] = (b'inert', 0o755 if number == 0 else 0o644)
        return files

    def record(self, files):
        return {'members': {name: hashlib.sha256(raw).hexdigest() for name, (raw, _) in files.items()},
                'memberModes': {name: mode for name, (_, mode) in files.items()}}

    def member_bytes(self, files):
        return {name: len(raw) for name, (raw, _) in files.items()}

    def parents(self, files):
        return sorted({'/'.join(parts[:i]) for name in files for parts in [name.split('/')]
                       for i in range(1, len(parts))})

    def archive(self, files, *, kind='tar', directories=None, change=None):
        path = self.fresh('wire.zip' if kind == 'zip' else 'wire.tar.gz')
        directories = self.parents(files) if directories is None else directories
        entries = [(name, b'', 0o700, tarfile.DIRTYPE) for name in directories]
        entries += [(name, raw, mode, tarfile.REGTYPE) for name, (raw, mode) in files.items()]
        if change:
            entries = change(entries)
        if kind == 'zip':
            with zipfile.ZipFile(path, 'w') as output:
                for name, raw, mode, entry_kind in entries:
                    info = zipfile.ZipInfo(name + ('/' if entry_kind == tarfile.DIRTYPE else ''))
                    info.external_attr = (mode | (stat.S_IFDIR if entry_kind == tarfile.DIRTYPE else stat.S_IFREG)) << 16
                    output.writestr(info, raw)
        else:
            with tarfile.open(path, 'w:gz') as output:
                for name, raw, mode, entry_kind in entries:
                    info = tarfile.TarInfo(name)
                    info.mode, info.type = mode, entry_kind
                    info.size = len(raw) if entry_kind == tarfile.REGTYPE else 0
                    info.linkname = self.root + '/resources/linux/sing-box'
                    output.addfile(info, io.BytesIO(raw) if info.size else None)
        raw = path.read_bytes()
        return path, {**self.record(files), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}

    def rejected(self, archive, record, reason=None, **options):
        destination = self.fresh('rejected')
        assertion = self.assertRaisesRegex(ValueError, reason) if reason else self.assertRaises(ValueError)
        with assertion:
            module.extract(str(archive), str(destination), record, self.root, wire_handoff=True, **options)
        self.assertFalse(destination.exists(), 'rejected wire archive wrote an extraction tree')

    def cli(self, arguments, request):
        return subprocess.run([sys.executable, '-B', str(Path(__file__).with_name('release-archive.py')), *map(str, arguments)],
                              input=json.dumps(request), text=True, capture_output=True, timeout=10)

    def test_real_3081_file_tar_exceeds_old_global_caps_and_restores_every_member(self):
        files = self.files({'linux': 769, 'win': 769, 'mac-x64': 768, 'mac-arm64': 768})
        self.assertEqual(len(files), 3081)
        self.assertGreater(len(files) + len(self.parents(files)), 2048)
        archive, record = self.archive(files)
        self.assertEqual(module.wire_inventory(record, self.root, self.parents(files)), 8203)
        native = self.fresh('native-rejected')
        with self.assertRaisesRegex(ValueError, 'member inventory limit'):
            module.extract(str(archive), str(native), record, self.root)
        self.assertFalse(native.exists(), 'wire-looking root must not enlarge the native default')
        destination = self.fresh('expanded')
        module.extract(str(archive), str(destination), record, self.root, wire_handoff=True)
        self.assertEqual({p.relative_to(destination).as_posix() for p in destination.rglob('*') if p.is_file()}, set(files))
        for name, (raw, mode) in files.items():
            self.assertEqual((destination / name).read_bytes(), raw)
            if sys.platform != 'win32':
                self.assertEqual((destination / name).stat().st_mode & 0o777, mode)

    def test_native_1024_file_and_2048_entry_limits_remain(self):
        files = {self.root + '/d%04d/file' % number: (b'inert', 0o644) for number in range(1024)}
        # 1024 files + 1024 directory headers; the implicit root is unchanged
        # native behavior. One additional explicit root header must exceed 2048.
        directories = self.parents(files)
        archive, record = self.archive(files, directories=[name for name in directories if name != self.root])
        module.extract(str(archive), str(self.fresh('native-boundary')), record, self.root)
        archive, record = self.archive(files)
        destination = self.fresh('native-entries-rejected')
        with self.assertRaisesRegex(ValueError, 'entry limit'):
            module.extract(str(archive), str(destination), record, self.root)
        self.assertFalse(destination.exists())
        files[self.root + '/extra'] = (b'inert', 0o644)
        archive, record = self.archive(files, kind='zip')
        destination = self.fresh('native-files-rejected')
        with self.assertRaisesRegex(ValueError, 'member inventory limit'):
            module.extract(str(archive), str(destination), record, self.root)
        self.assertFalse(destination.exists())

    def test_one_platform_cannot_borrow_file_or_implicit_directory_budget(self):
        files = self.files({'linux': 1024, 'win': 3, 'mac-x64': 3, 'mac-arm64': 3})
        prefix = self.root + '/resources/linux/'
        linux = [name for name in files if name.startswith(prefix)]
        values = [files.pop(name) for name in linux]
        # Runtime root + 1021 child dirs + 1022 runtime files + recovery root
        # and two recovery files = 2047. One deeper runtime parent reaches2048.
        files[prefix + 'sing-box'] = values[0]
        for number, value in enumerate(values[1:]):
            files[prefix + 'd%04d/file' % number] = value
        files[prefix + 'd0000/deeper/file'] = files.pop(prefix + 'd0000/file')
        self.assertEqual(module.wire_inventory(self.record(files), self.root), 8203)
        files[prefix + 'last/file'] = files.pop(prefix + 'sing-box')
        archive, record = self.archive(files, directories=[])
        self.rejected(archive, record, 'wire group entry limit')
        # Fewer than 4103 total files is insufficient: Linux alone has 1025.
        files = self.files({'linux': 1025, 'win': 3, 'mac-x64': 3, 'mac-arm64': 3})
        self.assertLess(len(files), 4103)
        archive, record = self.archive(files)
        self.rejected(archive, record, 'wire platform member inventory limit')

    def test_closed_control_and_four_platform_inventory(self):
        for index in ['polaris-box-assets.json', 'polaris-upstream-native-assets.json']:
            files = self.files(index=index)
            self.assertEqual(module.wire_inventory(self.record(files), self.root, self.parents(files)), 8203)
        for mutation in ['unknown-platform', 'unknown-control', 'dual-index', 'missing-platform', 'missing-receipt', 'missing-handoff']:
            with self.subTest(mutation=mutation):
                files = self.files()
                if mutation == 'unknown-platform':
                    files[self.root + '/resources/android/core'] = (b'inert', 0o644)
                elif mutation == 'unknown-control':
                    files[self.root + '/resources/.release-receipts/extra.json'] = (b'inert', 0o600)
                elif mutation == 'dual-index':
                    files[self.root + '/resources/.release-receipts/polaris-box-assets.json'] = (b'inert', 0o600)
                elif mutation == 'missing-platform':
                    del files[self.root + '/resources/win/sing-box.exe']
                elif mutation == 'missing-receipt':
                    del files[self.root + '/resources/.release-receipts/win.json']
                else:
                    del files[self.root + '/handoff.json']
                archive, record = self.archive(files)
                self.rejected(archive, record)

    def test_recovery_is_mandatory_closed_and_charged_to_its_own_platform(self):
        prefix = self.root + '/resources/core-recovery/'
        archive_name = prefix + 'linux/native.tar.gz'
        self.assertEqual(module.wire_group(archive_name, False), 'linux')
        self.assertEqual(module.wire_group(prefix + 'win', True), 'win')
        for mutation in ['missing-archive', 'missing-inventory', 'legacy-no-recovery',
                         'nested-archive', 'unknown-platform', 'extra-archive', 'wrong-format']:
            with self.subTest(mutation=mutation):
                files = self.files()
                if mutation == 'missing-archive': del files[archive_name]
                elif mutation == 'missing-inventory': del files[prefix + 'linux/inventory.json']
                elif mutation == 'legacy-no-recovery':
                    files = {name: data for name, data in files.items() if not name.startswith(prefix)}
                elif mutation == 'nested-archive': files[prefix + 'linux/nested/native.tar.gz'] = files.pop(archive_name)
                elif mutation == 'unknown-platform': files[prefix + 'other/native.tar.gz'] = (b'inert', 0o644)
                elif mutation == 'extra-archive': files[prefix + 'linux/another.tar.gz'] = (b'inert', 0o644)
                else: files[prefix + 'linux/native.zip'] = files.pop(archive_name)
                archive, record = self.archive(files)
                self.rejected(archive, record)

    def test_wire_paths_aliases_and_file_directory_collisions_fail_before_writes(self):
        prefix = self.root + '/resources/linux/'
        for additions in [['../escape'], ['NUL.txt'], ['name.'], ['a\\b'], ['a', 'A'],
                          ['a', 'a/b'], ['a/file', 'A/other']]:
            with self.subTest(additions=additions):
                files = self.files()
                files.update({prefix + name: (b'inert', 0o644) for name in additions})
                archive, record = self.archive(files)
                self.rejected(archive, record)

    def test_actual_tar_empty_duplicate_missing_extra_and_link_entries_fail(self):
        files = self.files()
        core = self.root + '/resources/linux/sing-box'
        directory = self.root + '/resources/linux'
        changes = {
            'empty-directory': lambda rows: rows + [(directory + '/empty', b'', 0o700, tarfile.DIRTYPE)],
            'duplicate-directory': lambda rows: rows + [(directory, b'', 0o700, tarfile.DIRTYPE)],
            'duplicate-file': lambda rows: rows + [(core, b'inert', 0o755, tarfile.REGTYPE)],
            'unexpected-file': lambda rows: rows + [(directory + '/extra', b'inert', 0o644, tarfile.REGTYPE)],
            'missing-file': lambda rows: [row for row in rows if row[0] != core],
        }
        for entry_kind in [tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE]:
            changes['link-or-special-' + repr(entry_kind)] = lambda rows, kind=entry_kind: [
                (name, b'', mode, kind) if name == core else (name, raw, mode, old_kind)
                for name, raw, mode, old_kind in rows]
        for name, change in changes.items():
            with self.subTest(change=name):
                archive, record = self.archive(files, change=change)
                self.rejected(archive, record)

    def test_wire_modes_hashes_and_existing_byte_limits_remain(self):
        files = self.files()
        core = self.root + '/resources/linux/sing-box'
        archive, original = self.archive(files)
        for mutation in ['missing-modes', 'extra-mode', 'different-mode', 'unsafe-mode', 'boolean-mode',
                         'control-mode', 'member-hash', 'outer-hash', 'outer-size', 'outer-limit', 'unknown-record-field']:
            with self.subTest(mutation=mutation):
                record = json.loads(json.dumps(original))
                if mutation == 'missing-modes': del record['memberModes']
                elif mutation == 'extra-mode': record['memberModes'][self.root + '/extra'] = 0o600
                elif mutation == 'different-mode': record['memberModes'][core] = 0o644
                elif mutation == 'unsafe-mode': record['memberModes'][core] = 0o777
                elif mutation == 'boolean-mode': record['memberModes'][core] = False
                elif mutation == 'control-mode': record['memberModes'][self.root + '/handoff.json'] = 0o644
                elif mutation == 'member-hash': record['members'][core] = '0' * 64
                elif mutation == 'outer-hash': record['sha256'] = '0' * 64
                elif mutation == 'outer-size': record['bytes'] += 1
                elif mutation == 'outer-limit': record['bytes'] = 512 * 1024 * 1024 + 1
                else: record['profile'] = 'wire-handoff'
                self.rejected(archive, record)
        archive, record = self.archive(files, change=lambda rows: [
            (name, raw, 0o777 if name == core else mode, kind) for name, raw, mode, kind in rows])
        self.rejected(archive, record, 'unsafe member permissions')
        # A short malicious header claims an oversized member; allocate no huge payload.
        info = tarfile.TarInfo(core)
        info.mode, info.size = 0o755, 200 * 1024 * 1024 + 1
        raw = gzip.compress(info.tobuf() + b'\0' * 1024)
        archive = self.fresh('oversized.tar.gz')
        archive.write_bytes(raw)
        record = {**self.record(files), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
        self.rejected(archive, record, 'unexpected/oversized member')

    def test_wire_format_and_target_cannot_enter_native_zip_exceptions(self):
        files = self.files()
        archive, record = self.archive(files, kind='zip')
        self.rejected(archive, record, 'wire archive format differs')
        archive, record = self.archive(files)
        self.rejected(archive, record, 'wire archive format differs', target='windows/amd64')
        with self.assertRaisesRegex(ValueError, 'wire archive root differs'):
            module.wire_inventory(record, 'other-root')

    def test_expanded_budget_negative_spy_rejects_before_tar_parsing(self):
        # Boundary-only negative spy: no 1 GiB allocation or real gzip bomb.
        # The real inert archive still passes the ordinary size/SHA checks.
        class OversizedExpansion:
            def __len__(self):
                return 1024 ** 3 + 1

        archive, record = self.archive(self.files())
        for wire in [False, True]:
            with self.subTest(wire_handoff=wire):
                destination = self.fresh('expanded-budget-rejected')
                with patch.object(module.gzip, 'GzipFile') as compressed, patch.object(module.tarfile, 'open') as tar_open:
                    stream = compressed.return_value.__enter__.return_value
                    stream.read.return_value = OversizedExpansion()
                    with self.assertRaisesRegex(ValueError, 'expanded tar size limit'):
                        module.extract(str(archive), str(destination), record, self.root, wire_handoff=wire)
                    stream.read.assert_called_once_with(1024 ** 3 + 1)
                    tar_open.assert_not_called()
                self.assertFalse(destination.exists())

    def test_producer_inventory_checks_exact_directories_without_archive_io(self):
        files = self.files()
        record, directories = self.record(files), self.parents(files)
        request = {'root': self.root, 'archive': record, 'directories': directories,
                   'memberBytes': self.member_bytes(files)}
        before = list(self.directory.iterdir())
        result = self.cli(['--check-wire-handoff'], request)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(list(self.directory.iterdir()), before)
        for changed in [directories[:-1], directories + [directories[0]],
                        directories + [self.root + '/resources/linux/empty']]:
            with self.subTest(directories=changed):
                with self.assertRaisesRegex(ValueError, 'wire directory inventory differs'):
                    module.wire_inventory(record, self.root, changed)
        over = self.files({'linux': 1025, 'win': 3, 'mac-x64': 3, 'mac-arm64': 3})
        result = self.cli(['--check-wire-handoff'], {'root': self.root, 'archive': self.record(over),
                          'directories': self.parents(over), 'memberBytes': self.member_bytes(over)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('wire platform member inventory limit', result.stderr)

    def test_producer_byte_budget_metadata_exact_boundaries_without_payload_allocation(self):
        # Metadata-only planning proof: these sizes do not describe the inert
        # fixture bytes, and do not prove an archive of this size can be extracted.
        files = self.files({'linux': 6, 'win': 3, 'mac-x64': 3, 'mac-arm64': 3})
        record, directories = self.record(files), self.parents(files)
        names = [name for name in files if module.wire_group(name, False) == 'linux']
        member_limit, expanded_limit = 200 * 1024 * 1024, 1024 ** 3
        single = dict.fromkeys(files, 0)
        single[names[0]] = member_limit
        total = dict.fromkeys(files, 0)
        total.update(dict.fromkeys(names[:5], member_limit))
        total[names[5]] = expanded_limit - 5 * member_limit
        self.assertEqual(sum(total.values()), expanded_limit)
        for name, sizes in [('zero', dict.fromkeys(files, 0)), ('member', single), ('total', total)]:
            with self.subTest(boundary=name):
                before = list(self.directory.iterdir())
                self.assertEqual(module.wire_inventory(record, self.root, directories, member_bytes=sizes), 8203)
                result = self.cli(['--check-wire-handoff'], {'root': self.root, 'archive': record,
                                  'directories': directories, 'memberBytes': sizes})
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(list(self.directory.iterdir()), before, 'metadata precheck performed archive I/O')

    def test_producer_byte_budget_metadata_overflow_and_malformed_sizes_fail_closed(self):
        files = self.files({'linux': 6, 'win': 3, 'mac-x64': 3, 'mac-arm64': 3})
        record, directories = self.record(files), self.parents(files)
        names = [name for name in files if module.wire_group(name, False) == 'linux']
        member_limit, expanded_limit = 200 * 1024 * 1024, 1024 ** 3
        sizes = self.member_bytes(files)
        over_total = dict.fromkeys(files, 0)
        over_total.update(dict.fromkeys(names[:5], member_limit))
        over_total[names[5]] = expanded_limit - 5 * member_limit + 1
        cases = {'total-overflow': over_total,
                 'missing-key': {key: value for key, value in sizes.items() if key != names[0]},
                 'unknown-key': {**sizes, self.root + '/resources/linux/unreviewed': 0},
                 'empty-map': {}, 'list': [], 'string': 'not a size map'}
        for label, value in [('member-overflow', member_limit + 1), ('negative', -1),
                             ('true', True), ('false', False), ('float', 1.0), ('string-size', '1')]:
            cases[label] = {**sizes, names[0]: value}
        for name, changed in cases.items():
            with self.subTest(malformed=name):
                before = list(self.directory.iterdir())
                with self.assertRaises(ValueError):
                    module.wire_inventory(record, self.root, directories, member_bytes=changed)
                result = self.cli(['--check-wire-handoff'], {'root': self.root, 'archive': record,
                                  'directories': directories, 'memberBytes': changed})
                self.assertNotEqual(result.returncode, 0, 'malformed producer byte budget was accepted')
                self.assertEqual(list(self.directory.iterdir()), before)
        request = {'root': self.root, 'archive': record, 'directories': directories}
        # None is the internal consumer default, never a producer CLI opt-out.
        for changed in [request, {**request, 'memberBytes': None},
                        {**request, 'memberBytes': sizes, 'unexpected': True}]:
            with self.subTest(request=changed):
                result = self.cli(['--check-wire-handoff'], changed)
                self.assertNotEqual(result.returncode, 0, 'producer byte precheck was bypassed')
                self.assertFalse(list(self.directory.iterdir()))

    def test_cli_profile_is_explicit_and_unknown_arguments_fail_closed(self):
        files = self.files()
        archive, record = self.archive(files)
        request = {'root': self.root, 'archive': record}
        destination = self.fresh('explicit-wire')
        result = self.cli([archive, destination, '--wire-handoff'], request)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((destination / self.root / 'resources/linux/sing-box').read_bytes(), b'inert')
        for flags in [['--unknown-profile'], ['--check-wire-handoff'], ['--wire-handoff', '--unexpected']]:
            with self.subTest(flags=flags):
                destination = self.fresh('unknown-cli')
                result = self.cli([archive, destination, *flags], request)
                self.assertNotEqual(result.returncode, 0, 'unknown CLI selector was accepted')
                self.assertFalse(destination.exists())
        for extra in [{'target': 'windows/amd64'}, {'sourceArchive': {}}, {'wireTransport': {}},
                      {'validateOnly': True}, {'memberBytes': self.member_bytes(files)}]:
            with self.subTest(extra=extra):
                destination = self.fresh('unexpected-wire-fields')
                result = self.cli([archive, destination, '--wire-handoff'], {**request, **extra})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('unexpected wire extraction fields', result.stderr)
                self.assertFalse(destination.exists())
        larger = self.files({'linux': 257, 'win': 256, 'mac-x64': 256, 'mac-arm64': 256})
        archive, record = self.archive(larger)
        for fields, reason in [({'wire_handoff': True, 'validateOnly': True, 'check_wire_handoff': True,
                                'directories': self.parents(larger)}, 'member inventory limit'),
                               ({'profile': '--wire-handoff'}, 'unexpected archive profile field')]:
            with self.subTest(fields=fields):
                destination = self.fresh('json-cannot-select')
                result = self.cli([archive, destination], {'root': self.root, 'archive': record, **fields})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(reason, result.stderr)
                self.assertFalse(destination.exists())


if __name__ == '__main__':
    unittest.main()
