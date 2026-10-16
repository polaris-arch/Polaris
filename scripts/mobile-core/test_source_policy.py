"""Synthetic source-policy controls only; no Go, SDK, native tool or publication."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from source_policy import canonical, sha, validate_source_policy, verify_source_policy_files


def source_fixture():
    prefix = 'third_party/metacubex-utls/'
    files = {prefix + name: ('synthetic ' + name).encode()
             for name in ['conn.go', 'reality.go'] + ['fixture-%03d.go' % n for n in range(353)]}
    mod = (b'module github.com/sagernet/sing-box\ngo 1.27.0\ntoolchain go1.27.2\nrequire (\n'
           b' github.com/metacubex/utls v1.8.8\n github.com/sagernet/gomobile v0.1.13\n)\n'
           b'replace github.com/metacubex/utls => ./third_party/metacubex-utls\n'
           b'replace github.com/sagernet/sing-tun => ./third_party/sing-tun\n')
    files.update({'go.mod': mod, 'go.sum': b'synthetic baseline sums\n'})
    patch_bytes = b'synthetic reviewed REALITY patch\n'
    binding_path = 'release/dependency-sources/metacubex-utls-replacement.json'
    patch_path = 'release/dependency-sources/metacubex-utls-reality-alpn.patch'
    meta = {'module': 'github.com/metacubex/utls', 'version': 'v1.8.8',
            'replacementPath': './third_party/metacubex-utls', 'replacementTree': 'c' * 40,
            'replacementFileCount': 355, 'bindingPath': binding_path, 'bindingSha256': '0' * 64,
            'patchPath': patch_path, 'patchSha256': sha(patch_bytes), 'upstreamCommit': 'd' * 40,
            'upstreamH1': 'h1:' + 'A' * 43 + '=', 'upstreamZipSha256': 'e' * 64}
    inventory = {name[len(prefix):]: {'bytes': len(raw), 'afterSHA256': sha(raw),
                 'beforeSHA256': 'f' * 64 if name[len(prefix):] in ['conn.go', 'reality.go'] else sha(raw)}
                 for name, raw in files.items() if name.startswith(prefix)}
    binding = {'schema': 'polaris-repository-module-replacement-v1', 'module': meta['module'],
               'upstreamRepository': 'https://github.com/MetaCubeX/utls', 'upstreamCommit': meta['upstreamCommit'],
               'upstreamVersion': meta['version'], 'upstreamH1': meta['upstreamH1'],
               'upstreamModuleZipSHA256': meta['upstreamZipSha256'], 'replacementPath': meta['replacementPath'],
               'patch': patch_path, 'patchSHA256': meta['patchSha256'], 'files': inventory,
               'changedFiles': ['conn.go', 'reality.go'], 'reason': 'synthetic test input only'}
    files[binding_path] = canonical(binding)
    files[patch_path] = patch_bytes
    meta['bindingSha256'] = sha(files[binding_path])
    policy = {'schema': 'polaris-reviewed-kernel-source-v1', 'sourceCommit': 'a' * 40,
              'sourceTree': 'b' * 40, 'goDirective': '1.27.0', 'goVersion': '1.27.2',
              'toolchain': 'go1.27.2', 'goModSha256': sha(mod), 'goSumSha256': sha(files['go.sum']),
              'metacubex': meta}
    return policy, files


def future_mobile_fixture():
    value = json.loads((Path(__file__).with_name('fixtures') / 'legacy-source-contract.json').read_bytes())
    policy, files = source_fixture()
    value.update(repository='polaris-arch/sing-box-polaris', repositoryId=1421657509,
                 sourceURL='https://github.com/polaris-arch/sing-box-polaris',
                 version='1.15.0-polaris-alpha.12', sourceTag='v1.15.0-polaris-alpha.12',
                 sourceTagObject='e' * 40, goVersion='1.27.2', sourcePolicy=policy)
    for key in ('sourceCommit', 'sourceTree', 'goModSha256', 'goSumSha256'):
        value[key] = policy[key]
    for platform in ('android', 'apple'):
        value[platform]['outputName'] = value[platform]['outputName'].replace('1.15.0-alpha.12', value['version'])
    declaration = value['declaredTagsSource']['path']
    files[declaration] = b'synthetic source-owned tags declaration\n'
    value['declaredTagsSource']['sha256'] = sha(files[declaration])
    return value, files


class ReviewedSourcePolicy(unittest.TestCase):
    def test_exact_source_pair_and_toolchain_do_not_follow_version_labels(self):
        policy, _ = source_fixture()
        self.assertIs(validate_source_policy(policy, policy), policy)
        for key, bad in [('sourceCommit', '0' * 40), ('sourceTree', '0' * 40),
                         ('goModSha256', '0' * 64), ('goSumSha256', '0' * 64)]:
            context = dict(policy); context[key] = bad
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                validate_source_policy(policy, context)
        for key, bad in [('goDirective', '1.27.2'), ('goVersion', '1.28.0'), ('toolchain', 'go1.27.1')]:
            changed = deepcopy(policy); changed[key] = bad
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                validate_source_policy(changed, policy)
        with self.assertRaises(RuntimeError):
            validate_source_policy(None, policy)

    def test_full_355_file_binding_and_patch_are_verified_as_source_only(self):
        policy, files = source_fixture()
        result = verify_source_policy_files(policy, policy, files, policy['metacubex']['replacementTree'])
        self.assertEqual(result['replacementFileCount'], 355)
        self.assertEqual(result['sourcePolicySha256'], sha(canonical(policy)))
        self.assertIn('local replacement has no h1', result['baselineH1Role'])

    def test_missing_extra_changed_files_patch_and_subtree_are_rejected(self):
        for case in ('missing', 'extra', 'bytes', 'patch', 'tree'):
            policy, files = source_fixture(); tree = policy['metacubex']['replacementTree']
            if case == 'missing':
                del files['third_party/metacubex-utls/conn.go']
            elif case == 'extra':
                files['third_party/metacubex-utls/unreviewed.go'] = b'extra'
            elif case == 'bytes':
                files['third_party/metacubex-utls/reality.go'] += b'changed'
            elif case == 'patch':
                files[policy['metacubex']['patchPath']] += b'changed'
            else:
                tree = '0' * 40
            with self.subTest(case=case), self.assertRaises(RuntimeError):
                verify_source_policy_files(policy, policy, files, tree)

    def test_binding_semantics_are_checked_even_with_a_matching_container_hash(self):
        for case in ('path', 'baseline', 'changed-list', 'unlisted-change'):
            policy, files = source_fixture(); meta = policy['metacubex']
            binding = json.loads(files[meta['bindingPath']])
            if case == 'path':
                binding['replacementPath'] = '../outside'
            elif case == 'baseline':
                binding['upstreamH1'] = 'h1:' + 'B' * 43 + '='
            elif case == 'changed-list':
                binding['changedFiles'] = ['conn.go']
            else:
                binding['files']['fixture-000.go']['beforeSHA256'] = '0' * 64
            files[meta['bindingPath']] = canonical(binding)
            meta['bindingSha256'] = sha(files[meta['bindingPath']])
            with self.subTest(case=case), self.assertRaises(RuntimeError):
                verify_source_policy_files(policy, policy, files, meta['replacementTree'])

    def test_actual_go_directives_and_local_replace_inventory_are_checked(self):
        for case in ('directive', 'toolchain', 'extra-replace', 'Meta-version'):
            policy, files = source_fixture()
            if case == 'directive':
                files['go.mod'] = files['go.mod'].replace(b'go 1.27.0', b'go 1.27.2')
            elif case == 'toolchain':
                files['go.mod'] = files['go.mod'].replace(b'go1.27.2', b'go1.27.1')
            elif case == 'extra-replace':
                files['go.mod'] += b'replace example.invalid/extra => ./extra\n'
            else:
                files['go.mod'] = files['go.mod'].replace(b'utls v1.8.8', b'utls v1.8.7')
            policy['goModSha256'] = sha(files['go.mod'])
            with self.subTest(case=case), self.assertRaises(RuntimeError):
                verify_source_policy_files(policy, policy, files, policy['metacubex']['replacementTree'])

    def test_module_directives_require_dependencies_and_all_local_replacements_are_exact(self):
        additions = {
            'parent-local': b'replace example.invalid/extra => ../outside\n',
            'absolute-local': b'replace example.invalid/extra => /outside\n',
            'windows-local': b'replace example.invalid/extra => C:\\outside\n',
            'versioned-local': b'replace example.invalid/extra v1.0.0 => ./outside\n',
            'duplicate-go': b'go 1.27.0\n',
            'duplicate-toolchain': b'toolchain go1.27.2\n',
            'duplicate-module': b'module github.com/sagernet/sing-box\n',
            'duplicate-Meta': b'require github.com/metacubex/utls v1.8.8\n',
            'duplicate-replace': b'replace github.com/metacubex/utls v1.8.8 => github.com/example/utls v1.0.0\n',
            'unterminated-block': b'replace (\n',
        }
        for label, extra in additions.items():
            policy, files = source_fixture()
            files['go.mod'] += extra; policy['goModSha256'] = sha(files['go.mod'])
            with self.subTest(label=label), self.assertRaises(RuntimeError):
                verify_source_policy_files(policy, policy, files, policy['metacubex']['replacementTree'])
        policy, files = source_fixture()
        files['go.mod'] = files['go.mod'].replace(b'github.com/sagernet/gomobile v0.1.13',
                                                 b'github.com/sagernet/gomobile v0.1.12')
        policy['goModSha256'] = sha(files['go.mod'])
        with self.assertRaises(RuntimeError):
            verify_source_policy_files(policy, policy, files, policy['metacubex']['replacementTree'])

    def test_replace_block_retains_exact_local_inventory_without_running_go(self):
        policy, files = source_fixture()
        files['go.mod'] = files['go.mod'].replace(
            b'replace github.com/metacubex/utls => ./third_party/metacubex-utls\n'
            b'replace github.com/sagernet/sing-tun => ./third_party/sing-tun\n',
            b'replace (\n github.com/metacubex/utls => ./third_party/metacubex-utls\n'
            b' github.com/sagernet/sing-tun => ./third_party/sing-tun\n'
            b' github.com/sagernet/tailscale => github.com/polaris-arch/tailscale v1.0.0\n)\n')
        policy['goModSha256'] = sha(files['go.mod'])
        result = verify_source_policy_files(policy, policy, files, policy['metacubex']['replacementTree'])
        self.assertEqual(result['replacementFileCount'], 355)

    def test_new_repository_cannot_omit_source_policy_to_fall_back_to_legacy_go(self):
        from fork_input import validate
        value, _ = future_mobile_fixture()
        del value['sourcePolicy']
        for go_version in ('1.27.2', '1.25.5'):
            value['goVersion'] = go_version
            with self.subTest(go_version=go_version), self.assertRaisesRegex(RuntimeError, 'explicit reviewed'):
                validate(value)

    def test_mobile_old_contract_remains_valid_and_future_policy_is_exact(self):
        from fork_input import CONTRACT, validate
        old = json.loads((Path(__file__).with_name('fixtures') / 'legacy-source-contract.json').read_bytes())
        self.assertIs(validate(old), old)
        current = json.loads(CONTRACT.read_bytes())
        self.assertIs(validate(current), current)
        value, _ = future_mobile_fixture()
        self.assertIs(validate(value), value)
        self.assertIsNone(value['artifactPins'])
        self.assertFalse(value['AppAccepted'] or value['devicePassed'] or value['publicationEligible'])
        for key, bad in [('sourceCommit', '0' * 40), ('sourceTree', '0' * 40), ('goVersion', '1.25.5')]:
            changed = deepcopy(value); changed[key] = bad
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                validate(changed)

    def test_mobile_verify_source_checks_actual_meta_tree_and_regular_files(self):
        from fork_input import verify_source
        for case in ('valid', 'wrong-tree', 'redirected-file'):
            value, files = future_mobile_fixture(); meta = value['sourcePolicy']['metacubex']
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                for name, raw in files.items():
                    path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(raw)
                if case == 'redirected-file':
                    path = root / 'third_party/metacubex-utls/conn.go'
                    outside = root / 'outside'; outside.write_bytes(path.read_bytes())
                    path.unlink(); path.symlink_to(outside)
                def git(_, *args):
                    if args[0] == 'cat-file': return 'tag'
                    if args[0] == 'status' or args[0] == 'ls-files': return ''
                    if args[0] == 'ls-tree': return '\n'.join(sorted(n for n in files if n.startswith('third_party/metacubex-utls/')))
                    selected = args[1]
                    if selected == 'HEAD:third_party/metacubex-utls': return '0' * 40 if case == 'wrong-tree' else meta['replacementTree']
                    if selected.endswith('^{tag}'): return value['sourceTagObject']
                    if selected.endswith('^{tree}'): return value['sourceTree']
                    return value['sourceCommit']
                with patch('fork_input.git', side_effect=git):
                    if case == 'valid':
                        result = verify_source(root, value)
                        self.assertEqual(result['reviewedSource']['replacementFileCount'], 355)
                    else:
                        with self.assertRaises(RuntimeError): verify_source(root, value)


if __name__ == '__main__':
    unittest.main()
