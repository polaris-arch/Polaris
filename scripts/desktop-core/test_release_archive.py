"""Private malformed archives; no network, native loading, or Git repositories."""
import hashlib
import importlib.util
import io
import json
import stat
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

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


if __name__ == '__main__':
    unittest.main()
