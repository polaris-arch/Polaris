"""Origin reference controls; inert bytes only, no compiler or native execution."""
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest

from artifacts import file_pin, receipt_digest
from native_inputs import member_references, source_table, verify


class OriginReferences(unittest.TestCase):
    def fixture(self, root):
        scratch = root / 'scratch'; scratch.mkdir()
        cache = root / 'cache'; cache.mkdir()
        paths = [scratch / 'a.o', scratch / 'b.o']
        for path in paths: path.write_bytes(b'inert identical compiler object')
        files = {str(path): {'kind': 'compiler-object', **file_pin(path)} for path in paths}
        digest = file_pin(paths[0])['sha256']
        output = {'format': 'static-XCFramework', 'slices': {'controlled': {'members': [
            {'ordinal': 1, 'sha256': digest, 'goCarrier': False},
            {'ordinal': 2, 'sha256': digest, 'goCarrier': False}]}}}
        refs, table = member_references(output, source_table(files))
        facts = {'schema': 'polaris-mobile-native-inputs-v2', 'scratch': str(scratch), 'cache': str(cache),
                 'files': files, 'staticMemberOrigins': refs, 'originsByDigest': table}
        return facts, output, paths, digest

    def test_two_equal_members_keep_two_distinct_source_identities_once_and_verify_original_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            facts, output, paths, digest = self.fixture(Path(temporary).resolve())
            frozen = deepcopy(facts)
            self.assertEqual(len(facts['originsByDigest'][digest]), 2)
            self.assertEqual(len(facts['staticMemberOrigins']['controlled']), 2)
            self.assertTrue(all('origins' not in row for row in facts['staticMemberOrigins']['controlled']))
            verify(facts, output=output)
            self.assertEqual(facts, frozen)
            self.assertEqual(receipt_digest(facts), receipt_digest(frozen))
            paths[0].write_bytes(b'changed actual source')
            with self.assertRaisesRegex(RuntimeError, 'actual native input bytes'): verify(facts, output=output)

    def test_changed_missing_duplicate_and_extra_origin_or_reference_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            facts, output, _, digest = self.fixture(Path(temporary).resolve())
            mutations = [
                lambda v: v['originsByDigest'][digest].pop(),
                lambda v: v['originsByDigest'][digest].append(v['originsByDigest'][digest][0]),
                lambda v: v['originsByDigest'][digest][0].update(path='/unowned/other.o'),
                lambda v: v['originsByDigest'].update({'0' * 64: []}),
                lambda v: v['staticMemberOrigins']['controlled'][0].update(memberOrdinal=99),
                lambda v: v['staticMemberOrigins']['controlled'][0].update(sha256='0' * 64),
                lambda v: v['staticMemberOrigins']['controlled'].pop(),
            ]
            for mutate in mutations:
                changed = deepcopy(facts); mutate(changed)
                with self.assertRaisesRegex(RuntimeError, 'table/member'): verify(changed, output=output)
            excessive = deepcopy(output)
            excessive['slices']['controlled']['members'].append({'ordinal': 3, 'sha256': digest, 'goCarrier': False})
            with self.assertRaisesRegex(RuntimeError, 'lacks actual'): member_references(excessive, facts['originsByDigest'])
            excessive['slices']['controlled']['members'][-1]['ordinal'] = 2
            with self.assertRaisesRegex(RuntimeError, 'ordinal'): member_references(excessive, facts['originsByDigest'])

    def test_legacy_inline_origins_are_verified_without_rewriting_the_old_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            facts, output, _, digest = self.fixture(Path(temporary).resolve())
            facts['schema'] = 'polaris-mobile-native-inputs-v1'
            table = facts.pop('originsByDigest')
            for row in facts['staticMemberOrigins']['controlled']:
                row['origins'] = list(reversed(table[digest]))
            frozen = deepcopy(facts)
            verify(facts, output=output)
            self.assertEqual(facts, frozen)
            facts['staticMemberOrigins']['controlled'][0]['origins'].pop()
            with self.assertRaisesRegex(RuntimeError, 'legacy'): verify(facts, output=output)


if __name__ == '__main__': unittest.main()
