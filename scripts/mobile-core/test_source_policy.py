"""Synthetic source-policy controls only; no Go, SDK, native tool or publication."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from source_policy import (VENDOR_DIRECTORIES, canonical, sha, validate_source_policy, verify_source_policy_files,
                           vendor_tree)


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
    value['android'].update(ndkRevision='30.0.16248370', ndkSourceRevision='30.0.16248370',
                            ndkPackage='android-ndk-r30', ndkArchivePath='scripts/mobile-core/ndk-r30-linux.json')
    declaration = value['declaredTagsSource']['path']
    files[declaration] = b'synthetic source-owned tags declaration\n'
    value['declaredTagsSource']['sha256'] = sha(files[declaration])
    return value, files


def nine_vendor_fixture():
    """Synthetic v2 facts only, never a selected final tag or source declaration."""
    policy, files = source_fixture()
    vendors, modes, trees = {}, {}, {}
    for name, directory in VENDOR_DIRECTORIES.items():
        prefix = 'third_party/' + directory + '/'
        if name != 'github.com/metacubex/utls':
            files[prefix + 'fixture.go'] = ('synthetic module ' + name).encode()
        if directory == 'tailscale':
            files[prefix + '.gitignore'] = b'fixture-ignore\n'
            files[prefix + '.stignore'] = b'.gitignore'
        if directory == 'sing-tun':
            files[prefix + 'fixture.sh'] = b'#!/bin/sh\n# inert fixture only\n'
        names = [path for path in files if path.startswith(prefix)]
        for path in names:
            modes[path] = '120000' if path.endswith('/.stignore') else '100755' if path.endswith('.sh') else '100644'
        tree = vendor_tree({path[len(prefix):]: (modes[path], files[path]) for path in names})
        version = 'v1.8.8' if name == 'github.com/metacubex/utls' else 'v0.0.0-fixture'
        vendors[name] = dict(version=version, replacementPath='./' + prefix[:-1],
                             replacementTree=tree, replacementFileCount=len(names))
        trees[name] = tree
    files['go.mod'] = (b'module github.com/sagernet/sing-box\ngo 1.27.0\ntoolchain go1.27.2\nrequire (\n'
        b' github.com/sagernet/gomobile v0.1.13\n' +
        ''.join(' ' + name + ' ' + pin['version'] + '\n' for name, pin in vendors.items()).encode() + b')\nreplace (\n' +
        ''.join(' ' + name + ' => ' + pin['replacementPath'] + '\n' for name, pin in vendors.items()).encode() + b')\n')
    policy.update(schema='polaris-reviewed-kernel-source-v2', vendors=vendors, goModSha256=sha(files['go.mod']))
    policy['metacubex']['replacementTree'] = trees['github.com/metacubex/utls']
    return policy, files, modes, trees


def future_mobile_fixture_v2():
    value, old_files = future_mobile_fixture()
    policy, files, modes, trees = nine_vendor_fixture()
    declaration = value['declaredTagsSource']['path']
    files[declaration] = old_files[declaration]
    value['sourcePolicy'] = policy
    for key in ('sourceCommit', 'sourceTree', 'goModSha256', 'goSumSha256'):
        value[key] = policy[key]
    return value, files, modes, trees


class NineVendorSourcePolicy(unittest.TestCase):
    def check(self, policy, files, modes=None, trees=None):
        return verify_source_policy_files(policy, policy, files, policy['metacubex']['replacementTree'],
                                          vendor_modes=modes, vendor_trees=trees)

    def test_v2_is_explicit_complete_and_preserves_all_meta_provenance(self):
        policy, files, modes, trees = nine_vendor_fixture()
        self.assertIs(validate_source_policy(policy, policy), policy)
        result = self.check(policy, files, modes, trees)
        self.assertEqual(result['schema'], 'polaris-reviewed-source-observation-v2')
        self.assertEqual(set(result['vendors']), set(VENDOR_DIRECTORIES))
        self.assertEqual(result['replacementFileCount'], 355)
        self.assertEqual(result['sourcePolicySha256'], sha(canonical(policy)))
        from fork_input import validate
        value, _, _, _ = future_mobile_fixture_v2()
        self.assertIs(validate(value), value)
        self.assertIsNone(value['artifactPins'])
        self.assertFalse(value['AppAccepted'] or value['devicePassed'] or value['publicationEligible'])

    def test_v2_cannot_fall_back_to_meta_only_archive_evidence(self):
        policy, files, modes, trees = nine_vendor_fixture()
        for actual_modes, actual_trees in [(None, None), (modes, None), (None, trees), ({}, trees), (modes, {})]:
            with self.subTest(modes=actual_modes is None, trees=actual_trees is None), self.assertRaises(RuntimeError):
                self.check(policy, files, actual_modes, actual_trees)

    def test_schema_rejects_missing_extra_ambiguous_and_incoherent_vendor_pins(self):
        for case in ('missing', 'extra', 'path', 'tree', 'count', 'bool-count', 'version', 'field', 'meta', 'v1-extra'):
            policy, _, _, _ = nine_vendor_fixture()
            vendor = policy['vendors']['github.com/sagernet/sing-tun']
            if case == 'missing': policy['vendors'].pop('github.com/sagernet/sing')
            elif case == 'extra': policy['vendors']['example.invalid/extra'] = deepcopy(vendor)
            elif case == 'path': vendor['replacementPath'] = '../sing-tun'
            elif case == 'tree': vendor['replacementTree'] = 'main'
            elif case == 'count': vendor['replacementFileCount'] = 0
            elif case == 'bool-count': vendor['replacementFileCount'] = True
            elif case == 'version': vendor['version'] = '(devel)'
            elif case == 'field': vendor['unknown'] = True
            elif case == 'meta': policy['vendors']['github.com/metacubex/utls']['replacementTree'] = 'f' * 40
            else: policy['schema'] = 'polaris-reviewed-kernel-source-v1'
            with self.subTest(case=case), self.assertRaises(RuntimeError): validate_source_policy(policy, policy)

    def test_all_nine_versions_must_match_actual_go_mod_bytes(self):
        for name in VENDOR_DIRECTORIES:
            policy, files, modes, trees = nine_vendor_fixture()
            files['go.mod'] = files['go.mod'].replace((name + ' ' + policy['vendors'][name]['version']).encode(),
                                                     (name + ' v9.9.9').encode())
            policy['goModSha256'] = sha(files['go.mod'])
            with self.subTest(name=name), self.assertRaises(RuntimeError): self.check(policy, files, modes, trees)

    def test_complete_bytes_modes_paths_counts_and_trees_are_bound(self):
        for case in ('bytes', 'missing', 'extra', 'mode', 'missing-mode', 'extra-mode', 'tree', 'count', 'escape', 'gitlink', 'symlink'):
            policy, files, modes, trees = nine_vendor_fixture()
            path = 'third_party/sing-tun/fixture.go'
            if case == 'bytes': files[path] += b'changed'
            elif case == 'missing': files.pop(path)
            elif case == 'extra': files['third_party/sing-tun/extra.go'] = b'extra'; modes['third_party/sing-tun/extra.go'] = '100644'
            elif case == 'mode': modes[path] = '100755'
            elif case == 'missing-mode': modes.pop(path)
            elif case == 'extra-mode': modes['outside.go'] = '100644'
            elif case == 'tree': trees['github.com/sagernet/sing-tun'] = '0' * 40
            elif case == 'count': policy['vendors']['github.com/sagernet/sing-tun']['replacementFileCount'] += 1
            elif case == 'escape': files['third_party/sing-tun/../outside'] = b'extra'; modes['third_party/sing-tun/../outside'] = '100644'
            elif case == 'gitlink': modes[path] = '160000'
            else: modes[path] = '120000'; files[path] = b'fixture.go'
            with self.subTest(case=case), self.assertRaises(RuntimeError): self.check(policy, files, modes, trees)

    def test_allowed_tailscale_symlink_is_exact_and_mode_bound(self):
        policy, files, modes, trees = nine_vendor_fixture()
        self.check(policy, files, modes, trees)
        path = 'third_party/tailscale/.stignore'
        for raw, mode in [(b'../.gitignore', '120000'), (b'.gitignore', '100644'), (b'/.gitignore', '120000')]:
            changed_files, changed_modes = dict(files), dict(modes)
            changed_files[path], changed_modes[path] = raw, mode
            with self.subTest(raw=raw, mode=mode), self.assertRaises(RuntimeError):
                self.check(policy, changed_files, changed_modes, trees)

    def test_extra_or_scoped_local_replacements_are_rejected(self):
        for extra in [b'replace example.invalid/extra => ../outside\n',
                      b'replace example.invalid/extra v1.0.0 => ./outside\n',
                      b'replace example.invalid/extra => /outside\n']:
            policy, files, modes, trees = nine_vendor_fixture()
            files['go.mod'] += extra; policy['goModSha256'] = sha(files['go.mod'])
            with self.subTest(extra=extra), self.assertRaises(RuntimeError): self.check(policy, files, modes, trees)

    def test_git_tree_reconstruction_known_empty_and_one_file_objects(self):
        import hashlib
        self.assertEqual(vendor_tree({}), '4b825dc642cb6eb9a060e54bf8d69288fbee4904')
        raw = b'fixture'
        blob = hashlib.sha1(b'blob 7\0' + raw).digest()
        tree = b'100644 a\0' + blob
        expected = hashlib.sha1(b'tree ' + str(len(tree)).encode() + b'\0' + tree).hexdigest()
        self.assertEqual(vendor_tree({'a': ('100644', raw)}), expected)

    def test_git_tree_directory_order_modes_and_collisions_match_git_encoding(self):
        import hashlib
        def object_hash(kind, raw):
            return hashlib.sha1(kind + b' ' + str(len(raw)).encode() + b'\0' + raw).digest()
        entries = {'a/run': ('100755', b'run'), 'a.txt': ('100644', b'text'),
                   'a0': ('120000', b'.gitignore'), 'z/deep/file': ('100644', b'deep')}
        child_a = object_hash(b'tree', b'100755 run\0' + object_hash(b'blob', b'run'))
        child_deep = object_hash(b'tree', b'100644 file\0' + object_hash(b'blob', b'deep'))
        child_z = object_hash(b'tree', b'40000 deep\0' + child_deep)
        # Git orders a directory as if its basename ends in '/': a.txt, a/, a0.
        raw = (b'100644 a.txt\0' + object_hash(b'blob', b'text') + b'40000 a\0' + child_a +
               b'120000 a0\0' + object_hash(b'blob', b'.gitignore') + b'40000 z\0' + child_z)
        self.assertEqual(vendor_tree(entries), object_hash(b'tree', raw).hex())
        self.assertEqual(vendor_tree(dict(reversed(list(entries.items())))), vendor_tree(entries))
        for path in entries:
            changed = dict(entries); mode, content = changed[path]
            changed[path] = ('100755' if mode != '100755' else '100644', content)
            self.assertNotEqual(vendor_tree(changed), vendor_tree(entries))
        for entries in [{'a': ('100644', b'file'), 'a/b': ('100644', b'child')},
                        {'a/b': ('100644', b'child'), 'a': ('100644', b'file')}]:
            with self.assertRaisesRegex(RuntimeError, 'collision|duplicate'): vendor_tree(entries)

    def test_git_tree_matches_existing_desktop_archive_closure_without_running_archive(self):
        import ast
        import hashlib
        from source_policy import require
        # Reuse only the two existing pure nested definitions as the reference;
        # do not import or execute the desktop archive workflow or change it.
        path = Path(__file__).resolve().parents[1] / 'desktop-core/release-archive.py'
        module = ast.parse(path.read_text())
        archive = next(node for node in module.body if isinstance(node, ast.FunctionDef) and node.name == 'verify_source_archive')
        helpers = [node for node in archive.body if isinstance(node, ast.FunctionDef) and node.name in ('object_hash', 'tree_hash')]
        self.assertEqual({node.name for node in helpers}, {'object_hash', 'tree_hash'})
        namespace = {'hashlib': hashlib, 'require': require}
        exec(compile(ast.Module(body=helpers, type_ignores=[]), str(path), 'exec'), namespace)
        entries = {'a/run': ('100755', b'run'), 'a.txt': ('100644', b'text'),
                   'a0': ('100644', b'zero'), 'tailscale/.gitignore': ('100644', b'ignore'),
                   'tailscale/.stignore': ('120000', b'.gitignore'),
                   'z/deep/file': ('100644', b'deep')}
        tree = {}
        for path, (mode, raw) in entries.items():
            parts, node = path.split('/'), tree
            for part in parts[:-1]: node = node.setdefault(part, {})
            node[parts[-1]] = (mode.encode(), namespace['object_hash']('blob', raw))
        self.assertEqual(vendor_tree(entries), namespace['tree_hash'](tree).hex())


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


def current_mobile_graph_fixture(value, source):
    """Inert source directories and Go modinfo matching the selected contract."""
    source = Path(source).resolve()
    source.mkdir(parents=True, exist_ok=True)
    graph = [{'Path': value['goModule'], 'Main': True, 'Dir': str(source)}]
    raw = 'path\tsynthetic/main\nmod\t' + value['goModule'] + '\t(devel)\t\n'
    if 'sourcePolicy' in value:
        meta = value['sourcePolicy']['metacubex']
        vendors = value['sourcePolicy'].get('vendors')
        declarations = ([(name, pin['version'], pin['replacementPath']) for name, pin in vendors.items()]
                        if vendors is not None else
                        [(meta['module'], meta['version'], meta['replacementPath']),
                         ('github.com/sagernet/sing-tun', 'v0.0.0-fixture', './third_party/sing-tun')])
        for name, version, relative in declarations:
            directory = source / relative[2:]
            directory.mkdir(parents=True, exist_ok=True)
            graph.append({'Path': name, 'Version': version,
                          'Replace': {'Path': relative, 'Dir': str(directory)}})
            raw += 'dep\t' + name + '\t' + version + '\n=>\t' + relative + '\t(devel)\t\n'
    return graph, raw


class CurrentMobileGraphFixture(unittest.TestCase):
    def test_v2_selected_contract_fixture_keeps_exact_nine_source_replacements(self):
        from fork_input import load
        from source_policy import verify_vendor_graph
        value = load()
        self.assertEqual(value['sourcePolicy']['schema'], 'polaris-reviewed-kernel-source-v2')
        vendors = value['sourcePolicy']['vendors']
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'source'
            graph, raw = current_mobile_graph_fixture(value, source)
            self.assertEqual(len(graph), 10)  # Main plus nine reviewed vendors.
            self.assertIs(verify_vendor_graph(graph, source, vendors), graph)
            for row in graph[1:]:
                self.assertEqual(row['Version'], vendors[row['Path']]['version'])
                self.assertIn('dep\t' + row['Path'] + '\t' + row['Version'] + '\n', raw)
            for index in range(1, len(graph)):
                with self.subTest(missing=graph[index]['Path']), self.assertRaisesRegex(RuntimeError, 'actual graph replacements differ'):
                    verify_vendor_graph(graph[:index] + graph[index + 1:], source, vendors)
                wrong = deepcopy(graph); wrong[index]['Version'] = 'v0.0.0-fixture'
                with self.subTest(version=graph[index]['Path']), self.assertRaisesRegex(RuntimeError, 'actual vendor graph version differs'):
                    verify_vendor_graph(wrong, source, vendors)
                wrong = deepcopy(graph); wrong[index]['Replace']['Path'] += '-other'
                with self.subTest(path=graph[index]['Path']), self.assertRaisesRegex(RuntimeError, 'local graph path differs'):
                    verify_vendor_graph(wrong, source, vendors)

    def test_v1_and_legacy_fixture_shapes_remain_historical(self):
        value, _ = future_mobile_fixture()
        legacy = json.loads((Path(__file__).with_name('fixtures') / 'legacy-source-contract.json').read_bytes())
        with tempfile.TemporaryDirectory() as temporary:
            graph, _ = current_mobile_graph_fixture(value, Path(temporary) / 'v1')
            self.assertEqual([row['Path'] for row in graph],
                             [value['goModule'], 'github.com/metacubex/utls', 'github.com/sagernet/sing-tun'])
            self.assertEqual(graph[-1]['Version'], 'v0.0.0-fixture')
            old_graph, _ = current_mobile_graph_fixture(legacy, Path(temporary) / 'legacy')
            self.assertEqual(len(old_graph), 1)
            self.assertTrue(old_graph[0]['Main'])


if __name__ == '__main__':
    unittest.main()
