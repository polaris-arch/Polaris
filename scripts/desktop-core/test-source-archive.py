#!/usr/bin/env python3
"""Synthetic offline source-proof negative controls, not release evidence."""
import gzip
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
spec = importlib.util.spec_from_file_location('archive', Path(__file__).with_name('release-archive.py'))
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)
COMMIT = 'a' * 40
GITLINK = 'b' * 40

def oid(kind, data):
    return hashlib.sha1(kind.encode() + b' ' + str(len(data)).encode() + b'\0' + data).digest()

def git_tree(entries):
    return oid('tree', b''.join(mode + b' ' + name + b'\0' + digest for name, mode, digest in sorted(entries)))

class SourceArchiveTests(unittest.TestCase):
    def fixture(self, change=None, mod=None, license_content=b'synthetic license\n'):
        files = {'go.mod': b'module example.invalid/fixture\n\nreplace example.invalid/old => example.invalid/new v1.0.0\n',
                 'go.sum': b'example.invalid/new v1.0.0 h1:YQ=\n', 'LICENSE': license_content}
        if mod is not None:
            files['go.mod'] = mod
        entries = [(name.encode(), b'100644', oid('blob', data)) for name, data in files.items()]
        entries.append((b'client', b'160000', bytes.fromhex(GITLINK)))
        expected = git_tree(entries).hex()
        source_identity = {'sourceOverlay': 0, 'goModSha256': hashlib.sha256(files['go.mod']).hexdigest(),
                           'goSumSha256': hashlib.sha256(files['go.sum']).hexdigest(), 'replacements': []}
        if change:
            change(files)
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode='w') as tar:
            root = 'sing-box-polaris-source-' + COMMIT + '/'
            for name in [root, root + 'client/']:
                info = tarfile.TarInfo(name); info.type = tarfile.DIRTYPE; info.mode = 0o755; tar.addfile(info)
            for name, data in files.items():
                info = tarfile.TarInfo(root + name); info.mode = 0o644; info.size = len(data); tar.addfile(info, io.BytesIO(data))
        data = gzip.compress(raw.getvalue(), mtime=0)
        pin = {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
        linked = [{'module': 'example.invalid/old', 'version': 'v0.1.0', 'h1': '',
                   'replacement': {'module': 'example.invalid/new', 'version': 'v1.0.0', 'h1': 'h1:YQ='}}]
        return data, pin, expected, source_identity, linked

    def verify(self, fixture, links=None):
        data, pin, tree, source, linked = fixture
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'source.tar.gz'; path.write_bytes(data)
            return archive.verify_source_archive(path, pin, COMMIT, tree,
                [{'path': 'client', 'sha': GITLINK}] if links is None else links, source, linked)

    def test_source_tree_and_remote_replacement_are_independently_bound(self):
        fixture = self.fixture()
        self.assertEqual(self.verify(fixture), fixture[2])

    def test_fixed_commit_archive_keeps_tree_gitlink_and_legal_checks(self):
        data, pin, tree, source, linked = self.fixture()
        # Rebuild rather than patch tar headers/checksums.
        raw = io.BytesIO()
        with tarfile.open(fileobj=io.BytesIO(gzip.decompress(data))) as original, tarfile.open(fileobj=raw, mode='w') as target:
            for info in original:
                body = original.extractfile(info).read() if info.isfile() else None
                info.name = info.name.replace('sing-box-polaris-source-', 'sing-box-polaris-', 1)
                target.addfile(info, io.BytesIO(body) if body is not None else None)
        actual = gzip.compress(raw.getvalue(), mtime=0)
        pin = {'transport': 'github-commit-archive-v1', 'name': 'sing-box-polaris-' + COMMIT + '.tar.gz',
               'url': 'https://github.com/polaris-arch/sing-box-polaris/archive/' + COMMIT + '.tar.gz',
               'bytes': len(actual), 'sha256': hashlib.sha256(actual).hexdigest()}
        content = 'synthetic license\n'
        legal = [{'path': 'licenses/LICENSE', 'content': content, 'bytes': len(content),
                  'sha256': hashlib.sha256(content.encode()).hexdigest()}]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'source.tar.gz'; path.write_bytes(actual)
            def check(record=pin, files=legal, links=None):
                return archive.verify_source_archive(path, record, COMMIT, tree,
                    [{'path': 'client', 'sha': GITLINK}] if links is None else links, source, linked, None, files)
            self.assertEqual(check(), tree)
            for bad in [None, [], [{**legal[0], 'content': 'changed'}], [{**legal[0], 'path': 'licenses/missing'}]]:
                with self.assertRaises(ValueError): check(files=bad)
            for bad in [{**pin, 'transport': 'unknown'}, {**pin, 'url': pin['url'].replace(COMMIT, 'main')},
                        {**pin, 'name': 'arbitrary.tar.gz'}, {**pin, 'sha256': '0' * 64},
                        {k: v for k, v in pin.items() if k != 'transport'}]:
                with self.assertRaises(ValueError): check(record=bad)
            with self.assertRaises(ValueError): check(links=[])

    def test_cli_utf8_request_is_independent_of_stdin_code_page_and_remains_strict(self):
        content = 'Copyright “sample” — 中文 ©\n'
        data, pin, tree, source, linked = self.fixture(license_content=content.encode('utf-8'))
        request = {'sourceArchive': pin, 'sourceCommit': COMMIT, 'sourceTree': tree,
                   'sourceGitlinks': [{'path': 'client', 'sha': GITLINK}], 'sourceIdentity': source,
                   'buildReplacements': linked, 'legalFiles': [{'path': 'licenses/LICENSE', 'content': content,
                       'bytes': len(content.encode('utf-8')), 'sha256': hashlib.sha256(content.encode('utf-8')).hexdigest()}]}
        raw = json.dumps(request, ensure_ascii=False).encode('utf-8')
        # Deterministic reproduction of the locale-sensitive text-stream path:
        # the UTF-8 closing quote contains 0x9d, undefined in Windows cp1252.
        env = {**os.environ, 'PYTHONIOENCODING': 'cp1252:surrogateescape', 'PYTHONUTF8': '0'}
        old = subprocess.run([sys.executable, '-B', '-c',
            "import json,sys;json.load(sys.stdin)['legalFiles'][0]['content'].encode('utf-8')"],
            input=raw, capture_output=True, env=env, timeout=10)
        self.assertNotEqual(old.returncode, 0)
        self.assertIn(b'UnicodeEncodeError', old.stderr)
        self.assertIn(b'\\udc9d', old.stderr)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'source.tar.gz'; path.write_bytes(data)
            def check(payload=raw):
                return subprocess.run([sys.executable, '-B', str(Path(archive.__file__)), str(path)],
                    input=payload, capture_output=True, env=env, timeout=10)
            good = check()
            self.assertEqual(good.returncode, 0, good.stderr)
            # Invalid UTF-8 must not be replaced, ignored, or preserved as a surrogate.
            bad_utf8 = check(raw.replace('sample'.encode(), b'\xff', 1))
            self.assertNotEqual(bad_utf8.returncode, 0)
            self.assertIn(b'UnicodeDecodeError', bad_utf8.stderr)
            bad_surrogate = json.loads(raw)
            bad_surrogate['legalFiles'][0]['content'] = '\udc9d'
            self.assertNotEqual(check(json.dumps(bad_surrogate).encode()).returncode, 0)
            for field, value in [('content', content + 'changed'), ('bytes', 1), ('sha256', '0' * 64)]:
                changed = json.loads(raw); changed['legalFiles'][0][field] = value
                result = check(json.dumps(changed, ensure_ascii=False).encode('utf-8'))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'source legal identity differs', result.stderr)
            changed = json.loads(raw); changed['sourceTree'] = '0' * 40
            self.assertNotEqual(check(json.dumps(changed, ensure_ascii=False).encode('utf-8')).returncode, 0)

    def test_replace_tab_and_specific_version_precedence_follow_go_mod(self):
        for mod in [
            b'module fixture\nreplace\texample.invalid/old => example.invalid/new v1.0.0\n',
            b'module fixture\nreplace (\nexample.invalid/old => example.invalid/other v9.0.0\nexample.invalid/old v0.1.0 => example.invalid/new v1.0.0\n)\n',
        ]:
            fixture = self.fixture(mod=mod)
            self.assertEqual(self.verify(fixture), fixture[2])
        duplicate = b'module fixture\nreplace (\nexample.invalid/old v0.1.0 => example.invalid/new v1.0.0\nexample.invalid/old v0.1.0 => example.invalid/new v1.0.0\n)\n'
        with self.assertRaises(ValueError): self.verify(self.fixture(mod=duplicate))

    def test_changed_bytes_even_with_refreshed_tar_checksum_fail_git_tree(self):
        for change in [lambda files: files.update(LICENSE=b'changed'), lambda files: files.pop('LICENSE'),
                       lambda files: files.update(extra=b'extra'), lambda files: files.update({'../escape': b'x'})]:
            with self.assertRaises(ValueError): self.verify(self.fixture(change))

    def test_missing_or_wrong_gitlink_cannot_be_invented(self):
        for links in [[], [{'path': 'client', 'sha': 'c' * 40}], [{'path': 'missing', 'sha': GITLINK}]]:
            with self.assertRaises(ValueError): self.verify(self.fixture(), links)

    def test_source_metadata_and_remote_module_sum_are_not_self_proving(self):
        fixture = list(self.fixture()); fixture[3] = {**fixture[3], 'goModSha256': '0' * 64}
        with self.assertRaises(ValueError): self.verify(fixture)
        fixture = list(self.fixture()); fixture[4][0]['replacement']['h1'] = 'h1:Yg='
        with self.assertRaises(ValueError): self.verify(fixture)
        fixture = list(self.fixture()); fixture[4][0]['replacement']['module'] = 'example.invalid/other'
        with self.assertRaises(ValueError): self.verify(fixture)


    def test_reviewed_meta_source_proof_is_checked_inside_the_complete_downloaded_tar(self):
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'mobile-core'))
        from test_source_policy import source_fixture
        from source_policy import sha

        def tree_for(files):
            nodes = {}
            for path, raw in files.items():
                node = nodes
                parts = path.split('/')
                for part in parts[:-1]: node = node.setdefault(part, {})
                node[parts[-1]] = raw
            def digest(node):
                rows = []
                for name, value in node.items():
                    directory = isinstance(value, dict)
                    raw = name.encode()
                    rows.append((raw + (b'/' if directory else b''),
                        (b'40000' if directory else b'100644') + b' ' + raw + b'\0' +
                        (digest(value) if directory else oid('blob', value))))
                return oid('tree', b''.join(value for _, value in sorted(rows)))
            return digest(nodes).hex()

        for case in ('valid', 'changed-Meta-bytes', 'missing-Meta-file', 'changed-patch'):
            policy, files = source_fixture()
            files['third_party/sing-tun/tun.go'] = b'synthetic retained sing-tun source\n'
            meta = policy['metacubex']
            if case == 'changed-Meta-bytes': files['third_party/metacubex-utls/conn.go'] += b'changed'
            elif case == 'missing-Meta-file': del files['third_party/metacubex-utls/conn.go']
            elif case == 'changed-patch': files[meta['patchPath']] += b'changed'
            meta_tree = tree_for({n.removeprefix('third_party/metacubex-utls/'): raw
                                  for n, raw in files.items() if n.startswith('third_party/metacubex-utls/')})
            meta['replacementTree'] = meta_tree
            policy['sourceTree'] = tree_for(files)
            source = {'sourceOverlay': 0, 'goModSha256': policy['goModSha256'], 'goSumSha256': policy['goSumSha256'],
                      'replacements': [{'module': meta['module'], 'path': meta['replacementPath'], 'tree': meta_tree},
                        {'module': 'github.com/sagernet/sing-tun', 'path': './third_party/sing-tun',
                         'tree': tree_for({'tun.go': files['third_party/sing-tun/tun.go']})}]}
            linked = [{'module': meta['module'], 'version': meta['version'], 'h1': '',
                       'replacement': {'module': meta['replacementPath'], 'version': '(devel)', 'h1': ''}},
                      {'module': 'github.com/sagernet/sing-tun', 'version': 'v1.0.0', 'h1': '',
                       'replacement': {'module': './third_party/sing-tun', 'version': '(devel)', 'h1': ''}}]
            raw = io.BytesIO()
            with tarfile.open(fileobj=raw, mode='w') as tar:
                prefix = 'sing-box-polaris-source-' + policy['sourceCommit'] + '/'
                for name, content in files.items():
                    info = tarfile.TarInfo(prefix + name); info.mode = 0o644; info.size = len(content)
                    tar.addfile(info, io.BytesIO(content))
            data = gzip.compress(raw.getvalue(), mtime=0)
            pin = {'bytes': len(data), 'sha256': sha(data)}
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / 'source.tar.gz'; path.write_bytes(data)
                args = (path, pin, policy['sourceCommit'], policy['sourceTree'], [], source, linked, policy)
                if case == 'valid':
                    self.assertEqual(archive.verify_source_archive(*args), policy['sourceTree'])
                    from copy import deepcopy
                    controls = []
                    missing = deepcopy(linked); missing.pop(); controls.append(missing)
                    invented = deepcopy(linked); invented[0]['h1'] = meta['upstreamH1']; controls.append(invented)
                    wrong = deepcopy(linked); wrong[0]['replacement']['module'] = './outside'; controls.append(wrong)
                    for observed in controls:
                        with self.assertRaises(ValueError):
                            archive.verify_source_archive(*args[:6], observed, policy)
                else:
                    with self.assertRaises((ValueError, RuntimeError)): archive.verify_source_archive(*args)


if __name__ == '__main__': unittest.main()
