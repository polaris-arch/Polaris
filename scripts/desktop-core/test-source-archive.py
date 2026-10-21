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
from copy import deepcopy
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('archive', Path(__file__).with_name('release-archive.py'))
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)
COMMIT = 'a' * 40
GITLINK = 'b' * 40

def oid(kind, data):
    return hashlib.sha1(kind.encode() + b' ' + str(len(data)).encode() + b'\0' + data).digest()

def git_tree(entries):
    return oid('tree', b''.join(mode + b' ' + name + b'\0' + digest for name, mode, digest in sorted(entries)))


def tree_for(files, modes=None, gitlinks=()):
    """Independent archive fixture tree; does not call the shared vendor hasher."""
    nodes = {}
    values = {name: ((modes or {}).get(name, '100644').encode(), oid('blob', raw))
              for name, raw in files.items()}
    values.update({link['path']: (b'160000', bytes.fromhex(link['sha'])) for link in gitlinks})
    for path, value in values.items():
        node = nodes
        parts = path.split('/')
        for part in parts[:-1]: node = node.setdefault(part, {})
        node[parts[-1]] = value
    def digest(node):
        rows = []
        for name, value in node.items():
            directory = isinstance(value, dict)
            raw = name.encode()
            mode, content = (b'40000', digest(value)) if directory else value
            rows.append((raw + (b'/' if directory else b''), mode + b' ' + raw + b'\0' + content))
        return oid('tree', b''.join(value for _, value in sorted(rows)))
    return digest(nodes).hex()

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

    def test_explicit_legal_pins_cli_keeps_archive_inventory_and_source_proofs(self):
        data, pin, tree, source, linked = self.fixture()
        content = 'synthetic license\n'
        legal = {'path': 'licenses/LICENSE', 'bytes': len(content.encode()),
                 'sha256': hashlib.sha256(content.encode()).hexdigest()}
        request = {'sourceArchive': pin, 'sourceCommit': COMMIT, 'sourceTree': tree,
                   'sourceGitlinks': [{'path': 'client', 'sha': GITLINK}], 'sourceIdentity': source,
                   'buildReplacements': linked, 'legalFiles': [legal]}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'source.tar.gz'; path.write_bytes(data)
            def check(payload=request, flag=True):
                return subprocess.run([sys.executable, '-B', str(Path(archive.__file__)), str(path),
                                       *(['--source-legal-pins'] if flag else [])],
                    input=json.dumps(payload).encode(), capture_output=True, timeout=10)
            good = check()
            self.assertEqual(good.returncode, 0, good.stderr)
            self.assertIn(b'stage=stdin-start', good.stderr)
            self.assertIn(b'stage=complete', good.stderr)
            self.assertNotIn(content.encode(), good.stderr)
            # No optional-content fallback or request-controlled opt-in.
            self.assertNotEqual(check(flag=False).returncode, 0)
            self.assertNotEqual(check({**request, 'legal_pins': True}, flag=False).returncode, 0)
            for files in [None, [], [legal, legal],
                          [{**legal, 'content': content}],
                          [{k: v for k, v in legal.items() if k != 'sha256'}],
                          [{**legal, 'bytes': True}], [{**legal, 'bytes': -1}],
                          [{**legal, 'bytes': 2 * 1024 * 1024 + 1}],
                          [{**legal, 'bytes': legal['bytes'] + 1}],
                          [{**legal, 'sha256': 'malformed'}], [{**legal, 'sha256': '0' * 64}],
                          [{**legal, 'path': 'licenses/../LICENSE'}],
                          [legal, {**legal, 'path': 'licenses/extra'}],
                          [{**legal, 'path': 'licenses/missing'}]]:
                with self.subTest(files=files):
                    self.assertNotEqual(check({**request, 'legalFiles': files}).returncode, 0)
            for field, value in [('sourceTree', '0' * 40), ('sourceGitlinks', []),
                                 ('sourceIdentity', {**source, 'goModSha256': '0' * 64}),
                                 ('buildReplacements', [{**linked[0], 'replacement': {
                                     **linked[0]['replacement'], 'h1': 'h1:Yg='}}])]:
                with self.subTest(field=field):
                    self.assertNotEqual(check({**request, field: value}).returncode, 0)
            # The old entry still requires and verifies the complete content.
            full = {**request, 'legalFiles': [{**legal, 'content': content}]}
            self.assertEqual(check(full, flag=False).returncode, 0)
            self.assertNotEqual(check({**full, 'legalFiles': [{**legal, 'content': 'changed'}]},
                                      flag=False).returncode, 0)

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

        for case in ('valid', 'changed-Meta-bytes', 'missing-Meta-file', 'changed-patch', 'oversized-Meta', 'oversized-patch'):
            policy, files = source_fixture()
            files['third_party/sing-tun/tun.go'] = b'synthetic retained sing-tun source\n'
            meta = policy['metacubex']
            if case == 'changed-Meta-bytes': files['third_party/metacubex-utls/conn.go'] += b'changed'
            elif case == 'missing-Meta-file': del files['third_party/metacubex-utls/conn.go']
            elif case == 'changed-patch': files[meta['patchPath']] += b'changed'
            elif case == 'oversized-Meta': files['third_party/metacubex-utls/conn.go'] = b'x' * (2 * 1024 * 1024 + 1)
            elif case == 'oversized-patch': files[meta['patchPath']] = b'x' * (2 * 1024 * 1024 + 1)
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

class ReviewedVendorArchiveTests(unittest.TestCase):
    def fixture(self):
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'mobile-core'))
        from test_source_policy import nine_vendor_fixture
        policy, files, modes, _ = nine_vendor_fixture()
        # Reuse the shared nine-vendor data, but derive the complete archive tree
        # independently, including non-vendor files, legal bytes and a gitlink.
        files['LICENSE'] = b'synthetic legal bytes\n'
        files['go.mod'] += b'replace example.invalid/old => example.invalid/new v1.0.0\n'
        files['go.sum'] += b'example.invalid/new v1.0.0 h1:YQ=\n'
        for name, key in [('go.mod', 'goModSha256'), ('go.sum', 'goSumSha256')]:
            policy[key] = hashlib.sha256(files[name]).hexdigest()
        gitlinks = [{'path': 'client', 'sha': GITLINK}]
        policy['sourceTree'] = tree_for(files, modes, gitlinks)
        source = {'sourceOverlay': 0, 'goModSha256': policy['goModSha256'], 'goSumSha256': policy['goSumSha256'],
                  'replacements': [{'module': name, 'path': pin['replacementPath'], 'tree': pin['replacementTree']}
                                   for name, pin in policy['vendors'].items()]}
        linked = [{'module': name, 'version': pin['version'], 'h1': '',
                   'replacement': {'module': pin['replacementPath'], 'version': '(devel)', 'h1': ''}}
                  for name, pin in policy['vendors'].items()]
        linked.append({'module': 'example.invalid/old', 'version': 'v0.1.0', 'h1': '',
                       'replacement': {'module': 'example.invalid/new', 'version': 'v1.0.0', 'h1': 'h1:YQ='}})
        legal = [{'path': 'licenses/LICENSE', 'bytes': len(files['LICENSE']),
                  'sha256': hashlib.sha256(files['LICENSE']).hexdigest(), 'content': files['LICENSE'].decode()}]
        return dict(policy=policy, files=files, modes=modes, source=source, linked=linked, gitlinks=gitlinks, legal=legal)

    def pack(self, fixture, mutate=None):
        raw = io.BytesIO()
        prefix = 'sing-box-polaris-' + fixture['policy']['sourceCommit'] + '/'
        with tarfile.open(fileobj=raw, mode='w') as tar:
            for link in fixture['gitlinks']:
                info = tarfile.TarInfo(prefix + link['path']); info.type = tarfile.DIRTYPE
                tar.addfile(info)
            for name, content in fixture['files'].items():
                info = tarfile.TarInfo(prefix + name)
                mode = fixture['modes'].get(name, '100644')
                info.mode = 0o755 if mode == '100755' else 0o644
                if mode == '120000':
                    info.type = tarfile.SYMTYPE; info.linkname = content.decode()
                else:
                    info.size = len(content)
                if mutate:
                    mutate(name, info)
                tar.addfile(info, io.BytesIO(content) if info.isfile() else None)
        return gzip.compress(raw.getvalue(), mtime=0)

    def verify(self, fixture, *, data=None, record_change=None, legal_pins=False):
        policy = fixture['policy']
        data = self.pack(fixture) if data is None else data
        record = {'transport': 'github-commit-archive-v1',
                  'name': 'sing-box-polaris-' + policy['sourceCommit'] + '.tar.gz',
                  'url': 'https://github.com/polaris-arch/sing-box-polaris/archive/' + policy['sourceCommit'] + '.tar.gz',
                  'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
        if record_change:
            record.update(record_change)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'original-source.tar.gz'; path.write_bytes(data)
            before = hashlib.sha256(path.read_bytes()).hexdigest()
            with patch.object(tarfile.TarFile, 'extract', side_effect=AssertionError('extraction forbidden')), \
                    patch.object(tarfile.TarFile, 'extractall', side_effect=AssertionError('extraction forbidden')):
                try:
                    return archive.verify_source_archive(path, record, policy['sourceCommit'], policy['sourceTree'],
                        fixture['gitlinks'], fixture['source'], fixture['linked'], policy, fixture['legal'], legal_pins=legal_pins)
                finally:
                    self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), before)
                    self.assertEqual(list(Path(directory).iterdir()), [path])

    def refresh_tree(self, fixture):
        # Deliberately update only the top-level expected tree to reach the
        # independently retained vendor pins in negative controls.
        fixture['policy']['sourceTree'] = tree_for(fixture['files'], fixture['modes'], fixture['gitlinks'])

    def test_v2_observes_all_nine_roots_modes_and_original_archive_bytes(self):
        fixture = self.fixture()
        from source_policy import verify_source_policy_files
        with patch('source_policy.verify_source_policy_files', wraps=verify_source_policy_files) as verify:
            self.assertEqual(self.verify(fixture), fixture['policy']['sourceTree'])
        args, kwargs = verify.call_args
        self.assertEqual(kwargs['vendor_modes'], fixture['modes'])
        self.assertEqual(kwargs['vendor_trees'], {name: pin['replacementTree'] for name, pin in fixture['policy']['vendors'].items()})
        self.assertEqual(args[2]['third_party/tailscale/.stignore'], b'.gitignore')
        self.assertEqual(kwargs['vendor_modes']['third_party/tailscale/.stignore'], '120000')
        self.assertEqual(kwargs['vendor_modes']['third_party/sing-tun/fixture.sh'], '100755')
        fixture['legal'] = [{key: value for key, value in pin.items() if key != 'content'} for pin in fixture['legal']]
        self.assertEqual(self.verify(fixture, legal_pins=True), fixture['policy']['sourceTree'])

    def test_v2_four_platform_flattened_replacements_preserve_each_observation(self):
        fixture = self.fixture(); per_platform = deepcopy(fixture['linked'])
        data = self.pack(fixture)
        # Acquisition flattens four platforms (36 local rows plus the remote
        # rows); packaged verification passes one platform. Repeated identical
        # identities must pass, but no conflicting platform may be deduplicated.
        fixture['linked'] = [deepcopy(row) for _ in range(4) for row in per_platform]
        self.assertEqual(sum(row['replacement']['module'].startswith('./') for row in fixture['linked']), 36)
        self.assertEqual(self.verify(fixture, data=data), fixture['policy']['sourceTree'])
        for vendor_index, row in enumerate(per_platform):
            if not row['replacement']['module'].startswith('./'):
                continue
            for case in ('version', 'path', 'sum', 'replacement-sum'):
                changed = deepcopy(fixture)
                observed = changed['linked'][2 * len(per_platform) + vendor_index]
                if case == 'version': observed['version'] = 'v9.9.9'
                elif case == 'path': observed['replacement']['module'] = './third_party/elsewhere'
                elif case == 'sum': observed['h1'] = 'h1:YQ='
                else: observed['replacement']['h1'] = 'h1:YQ='
                with self.subTest(vendor=row['module'], platform=2, case=case), self.assertRaises(ValueError):
                    self.verify(changed, data=data)

    def test_v2_declarations_reject_omitted_extra_wrong_vendor_path_and_tree(self):
        for case in ('missing', 'extra', 'wrong-path', 'wrong-tree', 'wrong-mode', 'wrong-count'):
            fixture = self.fixture(); vendors = fixture['policy']['vendors']
            pin = vendors['github.com/sagernet/sing-tun']
            if case == 'missing': del vendors['github.com/sagernet/sing']
            elif case == 'extra': vendors['example.invalid/extra'] = deepcopy(pin)
            elif case == 'wrong-path': pin['replacementPath'] = './third_party/elsewhere'
            elif case == 'wrong-tree': pin['replacementTree'] = 'f' * 40
            elif case == 'wrong-mode': pin['mode'] = '100644'
            else: pin['replacementFileCount'] += 1
            with self.subTest(case=case), self.assertRaises((ValueError, RuntimeError)):
                self.verify(fixture)

    def test_v2_each_vendor_file_inventory_bytes_and_modes_are_bound(self):
        baseline = self.fixture()
        for name, pin in baseline['policy']['vendors'].items():
            prefix = pin['replacementPath'][2:] + '/'
            path = next(path for path in baseline['files'] if path.startswith(prefix) and baseline['modes'][path] == '100644')
            for case in ('bytes', 'omitted-file', 'extra-file', 'mode', 'omitted-root'):
                fixture = deepcopy(baseline)
                if case == 'bytes': fixture['files'][path] += b'changed'
                elif case == 'omitted-file': fixture['files'].pop(path)
                elif case == 'extra-file': fixture['files'][prefix + 'extra.go'] = b'undeclared'
                elif case == 'mode': fixture['modes'][path] = '100755'
                else:
                    fixture['files'] = {p: raw for p, raw in fixture['files'].items() if not p.startswith(prefix)}
                self.refresh_tree(fixture)
                with self.subTest(vendor=name, case=case), self.assertRaises((ValueError, RuntimeError)):
                    self.verify(fixture)

    def test_v2_vendor_paths_links_and_special_tar_members_fail_closed(self):
        for case in ('parent', 'backslash', 'control', 'link-target', 'link-regularized', 'other-link', 'hardlink', 'special-mode'):
            fixture = self.fixture(); data = None
            prefix = 'third_party/sing-tun/'
            if case in ('parent', 'backslash', 'control'):
                path = prefix + {'parent': '../escape', 'backslash': 'bad\\name', 'control': 'bad\nname'}[case]
                fixture['files'][path] = b'unsafe'
                self.refresh_tree(fixture)
            elif case == 'link-target': fixture['files']['third_party/tailscale/.stignore'] = b'../.gitignore'
            elif case == 'link-regularized': fixture['modes']['third_party/tailscale/.stignore'] = '100644'
            elif case == 'other-link':
                fixture['modes'][prefix + 'fixture.go'] = '120000'; fixture['files'][prefix + 'fixture.go'] = b'fixture.sh'
            else:
                def mutate(name, info):
                    if name == prefix + 'fixture.go':
                        if case == 'hardlink': info.type = tarfile.LNKTYPE; info.linkname = 'fixture.sh'; info.size = 0
                        else: info.mode = 0o4644
                data = self.pack(fixture, mutate)
            if case in ('link-target', 'link-regularized', 'other-link'):
                self.refresh_tree(fixture)
                # Match the changed subtree pin to reach the shared link
                # whitelist, except regularization, which must fail identity.
                if case != 'link-regularized':
                    root = 'tailscale' if case == 'link-target' else 'sing-tun'
                    vendor = 'github.com/sagernet/' + root
                    vendor_prefix = 'third_party/' + root + '/'
                    files = {p[len(vendor_prefix):]: raw for p, raw in fixture['files'].items() if p.startswith(vendor_prefix)}
                    modes = {p[len(vendor_prefix):]: mode for p, mode in fixture['modes'].items() if p.startswith(vendor_prefix)}
                    tree = tree_for(files, modes)
                    fixture['policy']['vendors'][vendor]['replacementTree'] = tree
                    next(pin for pin in fixture['source']['replacements'] if pin['module'] == vendor)['tree'] = tree
            with self.subTest(case=case), self.assertRaises((ValueError, RuntimeError)):
                self.verify(fixture, data=data)

    def test_v2_vendor_file_budget_is_checked_before_reading_body(self):
        fixture = self.fixture()
        info = tarfile.TarInfo('sing-box-polaris-' + fixture['policy']['sourceCommit'] + '/third_party/sing-tun/oversize')
        info.size = 32 * 1024 * 1024 + 1; info.mode = 0o644
        data = gzip.compress(info.tobuf() + b'\0' * 1024, mtime=0)
        with patch.object(tarfile.TarFile, 'extractfile', side_effect=AssertionError('oversized body must not be read')):
            with self.assertRaisesRegex(ValueError, 'vendor file exceeds byte budget'):
                self.verify(fixture, data=data)

    def test_v2_vendor_aggregate_budget_stops_before_next_member_read(self):
        fixture = self.fixture()
        # Bounded-memory negative probe of the 512 MiB collection guard. Each
        # stream returns the same real 16 MiB bytes object. This is not a valid
        # archive/positive source proof and never bypasses the verifier.
        content = b'x' * (16 * 1024 * 1024)
        prefix = 'sing-box-polaris-' + fixture['policy']['sourceCommit'] + '/third_party/sing-tun/'
        members = []
        for index in range(33):
            info = tarfile.TarInfo(prefix + str(index)); info.mode = 0o644; info.size = len(content)
            members.append(info)
        class Stream:
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def read(self, size): return content
        class Archive:
            reads = 0
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def __iter__(self): return iter(members)
            def extractfile(self, info):
                self.reads += 1
                if self.reads > 32: raise AssertionError('over-budget member must not be read')
                return Stream()
        observed = Archive()
        with patch.object(tarfile, 'open', return_value=observed):
            with self.assertRaisesRegex(ValueError, 'vendor inventory exceeds byte budget'):
                self.verify(fixture, data=gzip.compress(b'placeholder', mtime=0))
        self.assertEqual(observed.reads, 32)

    def test_v2_preserves_transport_tree_module_legal_gitlink_and_remote_sum_gates(self):
        for case in ('archive-sha', 'archive-size', 'transport', 'url', 'tree', 'module', 'overlay', 'legal', 'gitlink', 'remote-sum'):
            fixture = self.fixture(); data = self.pack(fixture); record = {}
            if case == 'archive-sha': record['sha256'] = '0' * 64
            elif case == 'archive-size': record['bytes'] = len(data) + 1
            elif case == 'transport': record['transport'] = 'invented'
            elif case == 'url': record['url'] = 'https://github.com/polaris-arch/sing-box-polaris/archive/main.tar.gz'
            elif case == 'tree': fixture['policy']['sourceTree'] = '0' * 40
            elif case == 'module': fixture['source']['goModSha256'] = '0' * 64
            elif case == 'overlay': fixture['source']['sourceOverlay'] = 1
            elif case == 'legal': fixture['legal'][0]['content'] = 'changed'
            elif case == 'gitlink': fixture['gitlinks'] = []
            else: fixture['linked'][-1]['replacement']['h1'] = 'h1:Yg='
            with self.subTest(case=case), self.assertRaises((ValueError, RuntimeError)):
                self.verify(fixture, data=data, record_change=record)

    def test_v2_retains_actual_module_legal_and_meta_proof_bytes(self):
        for case in ('go.mod', 'go.sum', 'LICENSE', 'binding', 'patch', 'missing-binding', 'missing-patch'):
            fixture = self.fixture(); meta = fixture['policy']['metacubex']
            path = (meta['bindingPath'] if case.endswith('binding') else
                    meta['patchPath'] if case.endswith('patch') else case)
            if case.startswith('missing-'): fixture['files'].pop(path)
            else: fixture['files'][path] += b'changed'
            self.refresh_tree(fixture)
            with self.subTest(case=case), self.assertRaises((ValueError, RuntimeError)):
                self.verify(fixture)

    def test_v2_source_identity_is_exactly_the_reviewed_nine_roots(self):
        for case in ('duplicate', 'unknown', 'wrong-tree', 'wrong-path'):
            fixture = self.fixture(); replacement = fixture['source']['replacements'][0]
            if case == 'duplicate': fixture['source']['replacements'].append(deepcopy(replacement))
            elif case == 'unknown': replacement['module'] = 'example.invalid/unknown'
            elif case == 'wrong-tree': replacement['tree'] = 'f' * 40
            else: replacement['path'] = './third_party/tailscale'
            with self.subTest(case=case), self.assertRaises(ValueError):
                self.verify(fixture)

    def test_v2_vendor_collection_uses_32_mib_not_the_legacy_meta_budget(self):
        fixture = self.fixture(); prefix = 'third_party/sing-tun/'
        fixture['files'][prefix + 'fixture.go'] = b'x' * (2 * 1024 * 1024 + 1)
        tree = tree_for({p[len(prefix):]: raw for p, raw in fixture['files'].items() if p.startswith(prefix)},
                        {p[len(prefix):]: mode for p, mode in fixture['modes'].items() if p.startswith(prefix)})
        fixture['policy']['vendors']['github.com/sagernet/sing-tun']['replacementTree'] = tree
        fixture['source']['replacements'][0]['tree'] = tree
        self.refresh_tree(fixture)
        self.assertEqual(self.verify(fixture), fixture['policy']['sourceTree'])

    def test_v2_retains_complete_linked_local_inventory_and_no_invented_h1(self):
        for case in ('pruned', 'source-and-linked-pruned', 'extra', 'path', 'sum', 'version', 'vendor-version'):
            fixture = self.fixture()
            if case == 'pruned': fixture['linked'].pop(0)
            elif case == 'source-and-linked-pruned':
                fixture['linked'].pop(0); fixture['source']['replacements'].pop(0)
            elif case == 'extra': fixture['linked'].append({'module': 'example.invalid/extra', 'version': 'v1.0.0', 'h1': '',
                'replacement': {'module': './third_party/sing', 'version': '(devel)', 'h1': ''}})
            elif case == 'path': fixture['linked'][0]['replacement']['module'] = './elsewhere'
            elif case == 'sum': fixture['linked'][0]['replacement']['h1'] = 'h1:YQ='
            elif case == 'version': fixture['linked'][0]['replacement']['version'] = 'v1.0.0'
            else: fixture['linked'][0]['version'] = 'v9.9.9'
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, 'local replacement|vendor'):
                self.verify(fixture)


if __name__ == '__main__': unittest.main()
