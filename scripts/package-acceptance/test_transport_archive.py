import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location('archive', Path(__file__).with_name('transport_archive.py'))
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)


class TransportArchive(unittest.TestCase):
    def fixture(self, entries):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        path = root / 'artifact.zip'
        with zipfile.ZipFile(path, 'w') as out:
            for name, content in entries:
                out.writestr(name, content)
        pin = {'bytes': path.stat().st_size, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        return path, root / 'output', pin

    def test_normal(self):
        path, dest, pin = self.fixture([('nested/Polaris_1.0.0_amd64-linux.deb', b'test')])
        archive.extract(path, dest, pin)
        self.assertEqual((dest / 'nested/Polaris_1.0.0_amd64-linux.deb').read_bytes(), b'test')
        with self.assertRaises(ValueError):
            archive.extract(path, dest, pin)

    def test_zip_mutations(self):
        for entries in [[('../escape', b'x')], [('/absolute', b'x')], [('x\\y', b'x')],
                        [('x', b'a'), ('x', b'b')], [('x', b'x'), ('x/y', b'y')]]:
            with self.subTest(entries=entries):
                path, dest, pin = self.fixture(entries)
                with self.assertRaises(ValueError):
                    archive.extract(path, dest, pin)
                self.assertFalse(dest.exists())

    def test_digest_and_size(self):
        path, dest, pin = self.fixture([('transport.json', b'{}')])
        for change in [{'sha256': 'a' * 64}, {'bytes': pin['bytes'] + 1}]:
            with self.assertRaises(ValueError):
                archive.extract(path, dest, {**pin, **change})


if __name__ == '__main__':
    unittest.main()
