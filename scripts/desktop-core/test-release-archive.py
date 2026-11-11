"""Private malformed archives; no network, native loading, or Git repositories."""
import hashlib
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

spec = importlib.util.spec_from_file_location('release_archive', Path(__file__).with_name('release-archive.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
ROOT = 'sing-box-1.15.0-polaris-alpha.12-linux-amd64'


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


class NativeArchiveLimits(unittest.TestCase):
    """Real inert archive bytes, not release receipts or native acceptance."""
    root = ROOT

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='polaris-native-archive-fixture-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.serial = 0

    def fresh(self, name):
        self.serial += 1
        return self.directory / (str(self.serial) + '-' + name)

    def record(self, files):
        return {'members': {name: hashlib.sha256(raw).hexdigest() for name, (raw, _) in files.items()},
                'memberModes': {name: mode for name, (_, mode) in files.items()}}

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

    def cli(self, arguments, request):
        return subprocess.run([sys.executable, '-B', str(Path(__file__).with_name('release-archive.py')), *map(str, arguments)],
                              input=json.dumps(request), text=True, capture_output=True, timeout=10)

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

    def test_cli_unknown_arguments_and_fields_fail_closed(self):
        archive, record = self.archive({self.root + '/sing-box': (b'inert', 0o755)})
        request = {'root': self.root, 'archive': record}
        destination = self.fresh('native-cli')
        result = self.cli([archive, destination], request)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((destination / self.root / 'sing-box').read_bytes(), b'inert')
        for flags in [['--wire-handoff'], ['--unknown-profile']]:
            with self.subTest(flags=flags):
                destination = self.fresh('unknown-cli')
                result = self.cli([archive, destination, *flags], request)
                self.assertNotEqual(result.returncode, 0, 'unknown CLI selector was accepted')
                self.assertIn('unexpected native archive arguments', result.stderr)
                self.assertFalse(destination.exists())
        for extra in [{'wire_handoff': True}, {'profile': '--wire-handoff'}, {'validateOnly': True}]:
            with self.subTest(extra=extra):
                destination = self.fresh('json-cannot-select')
                result = self.cli([archive, destination], {**request, **extra})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('unexpected archive request fields', result.stderr)
                self.assertFalse(destination.exists())

if __name__ == '__main__':
    unittest.main()
