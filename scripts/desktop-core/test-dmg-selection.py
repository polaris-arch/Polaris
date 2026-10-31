"""DMG listing regressions; actual native-container acceptance is separate."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('dmg_selection', Path(__file__).with_name('dmg-selection.py'))
dmg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dmg)


class DmgListing(unittest.TestCase):
    def parse(self, body, read_link=None, limit=10000, newline='\n'):
        listing = 'Type = HFS' + newline + '----------' + newline + body
        return dmg.member_records(listing, read_link or self.no_link, limit)

    @staticmethod
    def no_link(*args):
        raise AssertionError('ordinary members must not read a symlink target')

    def test_native_empty_sizes_modes_and_empty_apfs_link_properties(self):
        for properties in ['Folder = +\nSize = ', 'Mode = drwxr-xr-x\nSize = \nSymbolic Link = ']:
            with self.subTest(properties=properties):
                rows = self.parse('Path = Contents\n' + properties + '\n\nPath = Contents/Polaris\nSize = 3\nMode = -rw-r--r--\nSymbolic Link = \n')
                self.assertEqual(rows['Contents']['kind'], 'dir')
                self.assertEqual(rows['Contents/Polaris'], {'kind': 'file', 'bytes': 3, 'target': ''})

    def test_hfs_links_read_target_but_apfs_links_use_the_listed_target(self):
        reads = []
        def read(name, size):
            reads.append((name, size))
            return b'target'
        rows = self.parse('Path = Polaris\nMode = lrwxr-xr-x\nSize = 6\n', read)
        self.assertEqual(rows['Polaris']['target'], 'target')
        self.assertEqual(reads, [('Polaris', 6)])
        rows = self.parse('Path = Polaris\nMode = lrwxr-xr-x\nSize = 7\nSymbolic Link = target\n')
        self.assertEqual(rows['Polaris']['kind'], 'link')
        self.assertEqual(rows['Polaris']['target'], 'target')

    def test_hfs_reserved_cr_directory_is_not_renamed_to_another_namespace(self):
        for newline in ['\n', '\r\n']:
            with self.subTest(newline=newline):
                body = newline.join(['Path = Volume/.HFS+ Private Directory Data\r', 'Folder = +', 'Size = ', '', 'Path = Volume/Polaris', 'Size = 3', ''])
                self.assertEqual(list(self.parse(body, newline=newline)), ['Volume/Polaris'])
        rows = self.parse('Path = Volume/other\r\nSize = 3\n')
        self.assertIn('Volume/other\r', rows)  # Existing namespace validation must reject it.

    def test_invalid_records_fail_closed(self):
        for body in [
            'Path = Polaris\nSize = \n',
            'Path = Polaris\nSize = x\n',
            'Path = Polaris\nSize = 3\nPath = other\n',
            'Path = Polaris\nSize = 3\ninjected newline\n',
            'Path = Polaris\nSize = 3\nMode = prw-------\n',
            'Path = Polaris\nSize = 3\nFolder = +\nSymbolic Link = target\n',
            'Path = Polaris\nSize = 3\n\nPath = Polaris\nSize = 3\n',
            'Path = Volume/.HFS+ Private Directory Data\r\nSize = 3\n',
        ]:
            with self.subTest(body=body), self.assertRaises(ValueError):
                self.parse(body)

    def test_reserved_metadata_also_counts_towards_member_budget(self):
        with self.assertRaisesRegex(ValueError, 'member count budget'):
            self.parse('Path = Volume/.HFS+ Private Directory Data\r\nFolder = +\nSize = \n\nPath = Polaris\nSize = 3\n', limit=1)

    def test_empty_hfs_hardlink_stub_cannot_stand_for_payload_bytes(self):
        for size in [None, 0]:
            with self.subTest(size=size), self.assertRaisesRegex(ValueError, 'unresolved hard-linked'):
                dmg.require_executable_data([('Contents/MacOS/Polaris', 'Contents/MacOS/Polaris', size)])
        dmg.require_executable_data([('Contents/MacOS/Polaris', 'Contents/MacOS/Polaris', 17)])


if __name__ == '__main__':
    unittest.main()
