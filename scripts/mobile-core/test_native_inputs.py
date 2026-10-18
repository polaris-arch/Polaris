"""Origin reference controls; inert bytes only, no compiler or native execution."""
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from artifacts import file_pin, receipt_digest
from native_inputs import member_references, source_table, verify


class OriginReferences(unittest.TestCase):
    def fixture(self, root, source_count=2, member_count=None):
        scratch = root / 'scratch'; scratch.mkdir()
        cache = root / 'cache'; cache.mkdir()
        paths = [scratch / f'{ordinal}.o' for ordinal in range(source_count)]
        for path in paths: path.write_bytes(b'inert identical compiler object')
        files = {str(path): {'kind': 'compiler-object', **file_pin(path)} for path in paths}
        digest = file_pin(paths[0])['sha256']
        output = {'format': 'static-XCFramework', 'slices': {'controlled': {'members': [
            {'ordinal': ordinal + 1, 'sha256': digest, 'goCarrier': False}
            for ordinal in range(source_count if member_count is None else member_count)]}}}
        refs, table = member_references(output, source_table(files))
        facts = {'schema': 'polaris-mobile-native-inputs-v2', 'scratch': str(scratch), 'cache': str(cache),
                 'files': files, 'staticMemberOrigins': refs, 'originsByDigest': table}
        return facts, output, paths, digest

    def legacy(self, facts, digest):
        facts['schema'] = 'polaris-mobile-native-inputs-v1'
        table = facts.pop('originsByDigest')
        for row in facts['staticMemberOrigins']['controlled']:
            row['origins'] = list(reversed(table[digest]))
        return table

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
            self.legacy(facts, digest)
            frozen = deepcopy(facts)
            verify(facts, output=output)
            self.assertEqual(facts, frozen)
            facts['staticMemberOrigins']['controlled'][0]['origins'].pop()
            with self.assertRaisesRegex(RuntimeError, 'legacy'): verify(facts, output=output)

    def test_static_output_requires_v1_and_v2_mapping_even_when_absent(self):
        with tempfile.TemporaryDirectory() as temporary:
            facts, output, _, digest = self.fixture(Path(temporary).resolve())
            for schema in ('polaris-mobile-native-inputs-v1', 'polaris-mobile-native-inputs-v2'):
                for mapping in ('absent', None, [], {}):
                    changed = deepcopy(facts); changed['schema'] = schema
                    if mapping == 'absent': changed.pop('staticMemberOrigins')
                    else: changed['staticMemberOrigins'] = mapping
                    with patch('native_inputs.source_table', side_effect=AssertionError('must reject before source inspection')):
                        with self.assertRaisesRegex(RuntimeError, 'table/member'): verify(changed, output=output)
            # AAR v1 has no static-member contract; this is not a schema-only gate.
            android = deepcopy(facts); android['schema'] = 'polaris-mobile-native-inputs-v1'
            android.pop('staticMemberOrigins'); android.pop('originsByDigest')
            verify(android, output={'format': 'AAR'})
            facts.pop('originsByDigest')
            with patch('native_inputs.source_table', side_effect=AssertionError('must reject before source inspection')):
                with self.assertRaisesRegex(RuntimeError, 'table/member'): verify(facts, output=output)

    def test_legacy_structure_rejects_targets_references_and_counts_before_source_inspection(self):
        with tempfile.TemporaryDirectory() as temporary:
            facts, output, _, digest = self.fixture(Path(temporary).resolve())
            self.legacy(facts, digest)
            mutations = [
                (lambda v: v['staticMemberOrigins'].update(extra=[]), 'static source table/member targets required'),
                (lambda v: v['staticMemberOrigins']['controlled'].pop(), 'static source table/member counts differ'),
                (lambda v: v['staticMemberOrigins']['controlled'][1].update(memberOrdinal=1), 'static source table/member references differ'),
                (lambda v: v['staticMemberOrigins']['controlled'][0].update(memberOrdinal=True), 'static source table/member references differ'),
                (lambda v: v['staticMemberOrigins']['controlled'][0].update(sha256='0' * 64), 'static source table/member references differ'),
                (lambda v: v['staticMemberOrigins']['controlled'][0].update(extra='unbound'), 'static source table/member references differ'),
                (lambda v: v['staticMemberOrigins']['controlled'][0].pop('origins'), 'static source table/member references differ'),
                (lambda v: v['staticMemberOrigins']['controlled'][0].update(origins=None), 'legacy actual source/member origins required'),
            ]
            for mutate, reason in mutations:
                changed = deepcopy(facts); mutate(changed)
                with self.subTest(reason=reason), patch('native_inputs.source_table', side_effect=AssertionError('must reject before source inspection')):
                    # Each mutation must hit its specific structural/type guard,
                    # not merely raise some downstream source-reading error.
                    with self.assertRaisesRegex(RuntimeError, '^' + reason + '$'): verify(changed, output=output)

    def test_many_equal_digest_legacy_origins_normalize_actual_candidates_once_and_scan_inline_lists_once(self):
        class OnePassOrigins(list):
            def __init__(self, values):
                super().__init__(values); self.iterations = 0
            def __iter__(self):
                self.iterations += 1
                if self.iterations > 1: raise AssertionError('origins list copied/sorted again')
                return super().__iter__()
        with tempfile.TemporaryDirectory() as temporary:
            facts, output, _, digest = self.fixture(Path(temporary).resolve(), source_count=512, member_count=384)
            table = self.legacy(facts, digest)
            candidates = OnePassOrigins(table[digest])
            for row in facts['staticMemberOrigins']['controlled']:
                row['origins'] = OnePassOrigins(row['origins'])
            with patch('native_inputs.source_table', return_value={digest: candidates}):
                verify(facts, output=output)
            self.assertEqual(candidates.iterations, 1)
            self.assertTrue(all(row['origins'].iterations == 1 for row in facts['staticMemberOrigins']['controlled']))
            # Equal list length must not hide a duplicated candidate/missing identity.
            rows = facts['staticMemberOrigins']['controlled']
            for row in rows: row['origins'].iterations = 0
            rows[-1]['origins'][0] = rows[-1]['origins'][1]
            with patch('native_inputs.source_table', return_value={digest: OnePassOrigins(table[digest])}):
                with self.assertRaisesRegex(RuntimeError, 'legacy'): verify(facts, output=output)


if __name__ == '__main__': unittest.main()
