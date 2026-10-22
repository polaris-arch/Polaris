#!/usr/bin/env python3
"""Synthetic offline controls; never native execution or acceptance evidence."""
from copy import deepcopy
import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import warnings
import zipfile

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('native_dns_source', HERE / 'native-dns-source.py')
source = importlib.util.module_from_spec(spec)
spec.loader.exec_module(source)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def oid(kind, raw):
    return hashlib.sha1(kind.encode() + b' ' + str(len(raw)).encode() + b'\0' + raw).digest()


class SourceBindingTests(unittest.TestCase):
    def binding(self):
        return source.decode(source.BINDING.read_bytes(), 65536)

    def fixture(self):
        binding = self.binding()
        files = {'go.mod': b'module example.invalid/synthetic\n\ngo 1.27.0\n',
                 'go.sum': b'synthetic source only\n', 'LICENSE': b'synthetic license\n'}
        body = b''.join(b'100644 ' + name.encode() + b'\0' + oid('blob', raw)
                        for name, raw in sorted(files.items()))
        binding['sourceTree'] = oid('tree', body).hex()
        binding['sourceGitlinks'] = []
        binding['sourceIdentity'] = {'sourceOverlay': 0, 'goModSha256': sha(files['go.mod']),
                                     'goSumSha256': sha(files['go.sum']), 'replacements': []}
        tar_bytes = io.BytesIO()
        with tarfile.open(fileobj=tar_bytes, mode='w') as archive:
            for name, raw in files.items():
                info = tarfile.TarInfo('sing-box-polaris-source-' + binding['sourceCommit'] + '/' + name)
                info.size, info.mode = len(raw), 0o644
                archive.addfile(info, io.BytesIO(raw))
        source_bytes = gzip.compress(tar_bytes.getvalue(), mtime=0)
        binding['sourceArchive'].update(bytes=len(source_bytes), sha256=sha(source_bytes))
        index = {key: deepcopy(binding[key]) for key in
                 ('repository', 'repositoryId', 'version', 'sourceCommit', 'sourceTree', 'sourceArchive')}
        index.update(schema='polaris-upstream-native-staging-v1', tag='v' + binding['version'], tagObject=None,
                     producer={**binding['producer'], 'repository': binding['repository'],
                               'repositoryId': binding['repositoryId']}, platforms=[])
        for i in range(44):
            index['platforms'].append({
                'id': 'synthetic-' + str(i),
                **{key: deepcopy(binding[key]) for key in ('sourceCommit', 'sourceTree', 'version', 'sourceIdentity')},
                'buildGraph': {'settings': {'vcs.revision': binding['sourceCommit'], 'vcs.modified': 'false'},
                               'localReplacements': []}})
        return binding, index, source_bytes

    def package(self, directory, binding, index, source_bytes, extra=None):
        raw = json.dumps(index).encode()
        binding['index'].update(bytes=len(raw), sha256=sha(raw))
        path = Path(directory) / 'input.zip'
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', UserWarning)
            with zipfile.ZipFile(path, 'w') as package:
                package.writestr(binding['index']['name'], raw)
                package.writestr(binding['sourceArchive']['name'], source_bytes)
                if extra:
                    package.writestr(extra, b'extra')
        binding['indexArtifact'].update(bytes=path.stat().st_size, sha256=sha(path.read_bytes()))
        return path

    def test_actual_binding_is_exact_inactive_new_source(self):
        binding = source.validate(self.binding())
        self.assertEqual(binding['sourceCommit'], 'fe182431495711c09bdb3b074ed6aa6309c00cf8')
        self.assertEqual(binding['sourceTree'], '9b2fe41af73c5768dccff079ded1e4a489cfa9e1')
        self.assertEqual(binding['producer']['runId'], 39075665574)
        self.assertEqual(binding['producer']['runAttempt'], 1)
        self.assertEqual(binding['indexArtifact']['id'], 12117466241)

    def test_shared_version_corpus_keeps_staging_inactive_and_alpha_only(self):
        cases = json.loads((HERE.parent / 'lib/polaris-version-fixtures.json').read_bytes())
        for version in cases['accepted'] + cases['rejected']:
            binding = self.binding()
            binding['version'] = version
            with self.subTest(version=version):
                if version in cases['accepted'] and '-polaris-alpha.' in version:
                    self.assertEqual(source.validate(binding)['version'], version)
                    for key in ('active', 'AppAccepted', 'publicationEligible'):
                        self.assertIs(binding[key], False)
                        changed = deepcopy(binding)
                        changed[key] = True
                        with self.assertRaises(ValueError): source.validate(changed)
                else:
                    with self.assertRaisesRegex(ValueError, 'source identity invalid'): source.validate(binding)

    def test_binding_rejects_promotion_extra_policy_and_bad_identities(self):
        changes = [('active', True), ('AppAccepted', True), ('publicationEligible', True),
                   ('tagObject', 'a' * 40), ('state', 'release'), ('sourcePolicy', {}),
                   ('repository', 'polaris-arch/Polaris'), ('repositoryId', True),
                   ('sourceCommit', '0' * 40), ('sourceTree', 'A' * 40), ('nativeRows', 43)]
        for key, value in changes:
            with self.subTest(key=key):
                binding = self.binding()
                binding[key] = value
                with self.assertRaises(ValueError):
                    source.validate(binding)

    def test_binding_rejects_ambiguous_producer_archive_and_source(self):
        for mutate in [lambda b: b['producer'].update(runAttempt=True),
                       lambda b: b['producer'].update(workflowCommit='a' * 40),
                       lambda b: b['index'].update(name='../index.json'),
                       lambda b: b['sourceArchive'].update(bytes=65 * 1024 ** 2),
                       lambda b: b['sourceIdentity'].update(sourceOverlay=True),
                       lambda b: b['sourceIdentity']['replacements'].append(deepcopy(b['sourceIdentity']['replacements'][0])),
                       lambda b: b['sourceGitlinks'].append(deepcopy(b['sourceGitlinks'][0]))]:
            binding = self.binding()
            mutate(binding)
            with self.assertRaises(ValueError):
                source.validate(binding)

    def test_json_duplicate_nonfinite_and_size_controls(self):
        for raw in [b'{"a":1,"a":2}', b'{"a":NaN}', b'{"a":Infinity}', b'']:
            with self.assertRaises(ValueError):
                source.decode(raw, 100)
        with self.assertRaises(ValueError):
            source.decode(b'{"a":1}', 2)

    def test_complete_synthetic_source_proof_makes_no_binary_claims(self):
        binding, index, raw = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            result = source.verify(self.package(directory, binding, index, raw), binding)
        self.assertTrue(result['sourceVerified'])
        self.assertEqual(result['nativeIndexRows'], 44)
        self.assertEqual(result['scope'], 'offline-source-and-index-only')
        for key in ('binaryBytesVerified', 'runtimeValidated', 'mobileArtifactsVerified', 'AppAccepted', 'publicationEligible'):
            self.assertIs(result[key], False)

    def test_zip_bytes_are_pinned_before_parsing(self):
        binding, index, raw = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            path = self.package(directory, binding, index, raw)
            damaged = bytearray(path.read_bytes())
            damaged[-1] ^= 1
            path.write_bytes(damaged)
            with self.assertRaisesRegex(ValueError, 'pinned artifact bytes'):
                source.verify(path, binding)

    def test_zip_duplicate_and_extra_members_fail_even_with_rebound_zip_digest(self):
        for extra in ('polaris-upstream-native-assets.json', '../unexpected'):
            binding, index, raw = self.fixture()
            with tempfile.TemporaryDirectory() as directory:
                path = self.package(directory, binding, index, raw, extra)
                with self.assertRaisesRegex(ValueError, 'two-file'):
                    source.verify(path, binding)

    def test_index_source_run_rows_and_replacement_drift_reject(self):
        binding, index, _ = self.fixture()
        controls = [lambda d: d.update(schema='polaris-upstream-native-release-v1'),
                    lambda d: d.update(tagObject='a' * 40),
                    lambda d: d['producer'].update(runId=1),
                    lambda d: d['producer'].update(runAttempt=2),
                    lambda d: d['platforms'].pop(),
                    lambda d: d['platforms'][0].update(id=d['platforms'][1]['id']),
                    lambda d: d['platforms'][0].update(sourceCommit='b' * 40),
                    lambda d: d['platforms'][0]['sourceIdentity'].update(sourceOverlay=1),
                    lambda d: d['platforms'][0]['buildGraph']['settings'].update({'vcs.modified': 'true'}),
                    lambda d: d['platforms'][0]['buildGraph'].update(localReplacements=[{}])]
        for mutate in controls:
            changed = deepcopy(index)
            mutate(changed)
            with self.assertRaises(ValueError):
                source.verify_index(changed, binding)

    def test_source_tree_is_reconstructed_not_trusted_from_index(self):
        binding, index, raw = self.fixture()
        binding['sourceTree'] = index['sourceTree'] = 'b' * 40
        for row in index['platforms']:
            row['sourceTree'] = binding['sourceTree']
        with tempfile.TemporaryDirectory() as directory:
            path = self.package(directory, binding, index, raw)
            with self.assertRaisesRegex(ValueError, 'Git tree differs'):
                source.verify(path, binding)

    def test_source_module_bytes_are_independent_of_index_assertions(self):
        binding, index, raw = self.fixture()
        binding['sourceIdentity']['goModSha256'] = 'b' * 64
        for row in index['platforms']:
            row['sourceIdentity'] = deepcopy(binding['sourceIdentity'])
        with tempfile.TemporaryDirectory() as directory:
            path = self.package(directory, binding, index, raw)
            with self.assertRaisesRegex(ValueError, 'module bytes differ'):
                source.verify(path, binding)


if __name__ == '__main__':
    unittest.main()
