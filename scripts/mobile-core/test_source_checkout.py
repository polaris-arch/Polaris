"""Real local Git transport controls; no Go/SDK/build/publication acceptance."""
from copy import deepcopy
from pathlib import Path
import json
import hashlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import ci_android
from fork_input import git_environment, verify_source
from test_source_policy import future_mobile_fixture, future_mobile_fixture_v2


def fixture_git(args):
    # Disposable repositories need no auto maintenance. Git may otherwise detach
    # a writer that outlives a completed commit/fetch and races strict cleanup.
    # Command-scoped settings also cover fetch-created repositories and children.
    assert str(args[0]) == 'git'
    return ['git', '-c', 'gc.auto=0', '-c', 'maintenance.auto=false', *args[1:]]


class ImmutableSourceCheckout(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.remote = self.root / 'remote'
        self.remote.mkdir()
        self.value, files = future_mobile_fixture()
        self.git('init', '--quiet')
        for name, raw in files.items():
            target = self.remote / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(raw)
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'commit', '-qm', 'pinned source')
        self.value['sourceCommit'] = self.git('rev-parse', 'HEAD')
        self.value['sourceTree'] = self.git('rev-parse', 'HEAD^{tree}')
        policy = self.value['sourcePolicy']
        policy.update(sourceCommit=self.value['sourceCommit'], sourceTree=self.value['sourceTree'])
        policy['metacubex']['replacementTree'] = self.git('rev-parse', 'HEAD:third_party/metacubex-utls')
        self.tag()
        self.value['sourceTagObject'] = self.git('rev-parse', self.value['sourceTag'])
        # Advance and replace the public tag, leaving the pinned annotated object
        # unadvertised. Depth-one fetching the name no longer contains old HEAD.
        (self.remote / 'later.txt').write_text('newer source must never be selected\n')
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'commit', '-qm', 'later source')
        self.tag()
        self.calls = []
        run = ci_android.run
        self.real_run = lambda args, **kwargs: run(fixture_git(args), **kwargs)

    def git(self, *args):
        return subprocess.check_output(fixture_git(['git', '-C', str(self.remote), *args]),
                env=git_environment(), text=True, stderr=subprocess.PIPE).strip()

    def tag(self):
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'tag', '-fa', self.value['sourceTag'], '-m', 'fixture annotated source')

    def transport(self, args, **kwargs):
        args = list(args)
        self.calls.append(args[:])
        if 'fetch' in args:
            self.assertEqual(args[-2], self.value['sourceURL'])
            args[-2] = self.remote.as_uri()
        return self.real_run(args, **kwargs)

    def prepare(self, value=None, name='checkout'):
        with patch('ci_android.run', side_effect=self.transport):
            return ci_android.prepare_source(self.root / name, value or self.value)

    def test_moved_tag_depth_one_reproduces_and_fixed_object_checks_full_source(self):
        old = self.root / 'old-fetch'
        self.real_run(['git', 'init', '-q', old])
        ref = 'refs/tags/' + self.value['sourceTag']
        self.real_run(['git', '-C', old, 'fetch', '--depth=1', '--no-tags', self.remote.as_uri(), ref + ':' + ref])
        with self.assertRaisesRegex(RuntimeError, 'unable to read tree'):
            self.real_run(['git', '-C', old, 'checkout', '--detach', self.value['sourceCommit']])
        result = self.prepare()
        self.assertEqual(result['sourceCommit'], self.value['sourceCommit'])
        self.assertEqual(result['reviewedSource']['replacementFileCount'], 355)
        self.assertFalse(result['artifactAcceptance'])
        self.assertTrue((self.root / 'checkout/.git/shallow').is_file())
        self.assertFalse((self.root / 'checkout/later.txt').exists())
        self.assertEqual(verify_source(self.root / 'checkout', self.value), result)
        self.assertEqual([args[-1] for args in self.calls if 'fetch' in args],
                         [self.value['sourceTagObject'] + ':' + ref])
        self.calls.clear()
        self.assertEqual(self.prepare(), result)
        self.assertEqual(self.calls, [])  # Existing exact source is read-only.

    def test_wrong_commit_tree_module_and_tag_object_fail_closed(self):
        for key in ('sourceCommit', 'sourceTree', 'goModSha256', 'goSumSha256', 'sourceTagObject'):
            value = deepcopy(self.value)
            value[key] = ('f' * 64 if 'Sha256' in key else 'f' * 40)
            if key in value['sourcePolicy']:
                value['sourcePolicy'][key] = value[key]
            with self.subTest(key=key), self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
                self.prepare(value, key)

    def test_present_but_wrong_annotated_object_cannot_select_new_commit(self):
        value = deepcopy(self.value)
        value['sourceTagObject'] = self.git('rev-parse', self.value['sourceTag'])
        with self.assertRaises(RuntimeError):
            self.prepare(value, 'wrong-annotation')

    def test_server_refusal_has_no_moving_tag_or_head_fallback(self):
        def refused(args, **kwargs):
            self.calls.append(list(args))
            if 'fetch' in args:
                raise RuntimeError('server refuses unadvertised object')
            return self.real_run(args, **kwargs)
        with patch('ci_android.run', side_effect=refused), self.assertRaisesRegex(RuntimeError, 'unadvertised'):
            ci_android.prepare_source(self.root / 'refused', self.value)
        self.assertEqual(len([c for c in self.calls if 'fetch' in c]), 1)
        self.assertFalse(any('checkout' in c for c in self.calls))

    def test_low_threshold_auto_gc_is_disabled_for_fixture_writers(self):
        self.git('config', 'gc.auto', '1')
        self.git('config', 'gc.autoDetach', 'false')
        self.git('config', 'maintenance.autoDetach', 'false')
        self.git('config', 'maintenance.auto', 'true')
        # Git 2.55 defaults to geometric maintenance; select the GC task
        # explicitly and trigger it by exact pack count, not sampled loose OIDs.
        self.git('config', 'maintenance.strategy', 'gc')
        self.git('config', 'maintenance.gc.enabled', 'true')
        self.git('config', 'gc.autoPackLimit', '1')
        pack_dir = self.remote / '.git/objects/pack'
        pack_dir.mkdir(exist_ok=True)
        for revision in ('HEAD', 'HEAD^'):
            subprocess.run(fixture_git(['git', '-C', str(self.remote), 'pack-objects', str(pack_dir / 'pack')]),
                           input=self.git('rev-parse', revision) + '\n', env=git_environment(),
                           text=True, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(len(list(pack_dir.glob('*.idx'))), 2)
        trace = self.root / 'git-trace.json'
        env = dict(git_environment(), GIT_TRACE2_EVENT=str(trace))
        command = ['git', '-C', str(self.remote), '-c', 'user.name=Fixture',
                   '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '-qm', 'trace']
        # An exact, exceeded pack threshold cannot override fixture policy.
        subprocess.run(fixture_git(command), env=env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        guarded = [json.loads(line) for line in trace.read_text().splitlines()]
        self.assertFalse(any(row.get('event') == 'child_start' and
                             any(arg in ('maintenance', 'gc') for arg in row.get('argv', []))
                             for row in guarded))
        trace.unlink()
        # Negative control: same repository/threshold without the guard really
        # starts maintenance. Detach is explicitly off, so this probe also waits.
        subprocess.run(command, env=env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        unguarded = [json.loads(line) for line in trace.read_text().splitlines()]
        for child in ('maintenance', 'gc'):
            self.assertTrue(any(row.get('event') == 'child_start' and
                                child in row.get('argv', []) for row in unguarded),
                            f'missing {child} child: {unguarded}')

    def test_invalid_contract_fails_before_source_mutation(self):
        value = deepcopy(self.value)
        value['sourceTagObject'] = 'HEAD'
        with self.assertRaises(RuntimeError):
            self.prepare(value, 'invalid')
        self.assertFalse((self.root / 'invalid').exists())
        self.assertEqual(self.calls, [])

    def test_existing_dirty_or_wrong_source_is_not_reset_or_replaced(self):
        self.prepare()
        source = self.root / 'checkout'
        (source / 'go.mod').write_text('dirty')
        self.calls.clear()
        with self.assertRaises(RuntimeError):
            self.prepare()
        self.assertEqual((source / 'go.mod').read_text(), 'dirty')
        self.assertEqual(self.calls, [])


class NineVendorCheckoutFixtures(unittest.TestCase):
    """Filesystem and fake Git observations only; native transport is not invoked."""
    def setUp(self):
        guard = patch('subprocess.Popen', side_effect=AssertionError('fixture may not execute tools'))
        guard.start(); self.addCleanup(guard.stop)
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.source = Path(self.tmp.name).resolve()
        self.value, self.files, self.modes, self.trees = future_mobile_fixture_v2()
        self.calls, self.overrides = [], {}
        for name, raw in self.files.items():
            path = self.source / name; path.parent.mkdir(parents=True, exist_ok=True)
            if self.modes.get(name) == '120000': path.symlink_to(raw.decode())
            else:
                path.write_bytes(raw)
                path.chmod(0o755 if self.modes.get(name) == '100755' else 0o644)

    def git(self, source, *args):
        self.assertEqual(Path(source), self.source)
        self.calls.append(args)
        if args in self.overrides: return self.overrides[args]
        if args[0] == 'cat-file': return 'tag'
        if args[0] in ('status', 'ls-files'): return ''
        if args[0] == 'ls-tree':
            prefix = args[-1] + '/'
            rows = []
            for name in sorted(self.modes):
                if name.startswith(prefix):
                    raw = self.files[name]
                    blob = hashlib.sha1(b'blob ' + str(len(raw)).encode() + b'\0' + raw).hexdigest()
                    rows.append(self.modes[name] + ' blob ' + blob + '\t' + name)
            return '\n'.join(rows)
        self.assertEqual(args[0], 'rev-parse')
        selected = args[1]
        if selected.startswith('HEAD:'):
            for name, pin in self.value['sourcePolicy']['vendors'].items():
                if selected == 'HEAD:' + pin['replacementPath'][2:]: return self.trees[name]
            self.fail('unknown subtree: ' + selected)
        if selected.endswith('^{tag}'): return self.value['sourceTagObject']
        if selected.endswith('^{tree}'): return self.value['sourceTree']
        return self.value['sourceCommit']

    def check(self):
        with patch('fork_input.git', side_effect=self.git): return verify_source(self.source, self.value)

    def test_checkout_collects_every_tracked_vendor_and_no_acceptance(self):
        result = self.check()
        self.assertEqual(set(result['reviewedSource']['vendors']), set(self.trees))
        self.assertFalse(result['artifactAcceptance'])
        roots = {args[-1] for args in self.calls if args[0] == 'ls-tree'}
        self.assertEqual(roots, {pin['replacementPath'][2:] for pin in self.value['sourcePolicy']['vendors'].values()})

    def test_tracked_bytes_modes_links_and_parent_redirects_fail_closed(self):
        path = self.source / 'third_party/sing-tun/fixture.go'
        original = path.read_bytes()
        for case in ('bytes', 'exec', 'symlink', 'missing'):
            if case == 'bytes': path.write_bytes(original + b'changed')
            elif case == 'exec': path.chmod(0o755)
            elif case == 'symlink': path.unlink(); path.symlink_to('fixture.sh')
            else: path.unlink()
            with self.subTest(case=case), self.assertRaises(RuntimeError): self.check()
            if path.is_symlink() or path.exists(): path.unlink()
            path.write_bytes(original); path.chmod(0o644)
        parent = path.parent; target = self.source / 'redirected'
        parent.rename(target); parent.symlink_to(target, target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, 'directory redirected'): self.check()

    def test_allowed_symlink_target_and_git_modes_are_verified(self):
        path = self.source / 'third_party/tailscale/.stignore'
        path.unlink(); path.symlink_to('../outside')
        with self.assertRaisesRegex(RuntimeError, 'vendor symlink'): self.check()
        path.unlink(); path.write_bytes(b'.gitignore')
        with self.assertRaisesRegex(RuntimeError, 'vendor symlink'): self.check()

    def test_tag_commit_tree_dirty_and_untracked_guards_remain_before_inventory(self):
        cases = {
            ('cat-file', '-t', self.value['sourceTagObject']): 'commit',
            ('rev-parse', self.value['sourceTag'] + '^{tag}'): '0' * 40,
            ('rev-parse', self.value['sourceTagObject'] + '^{commit}'): '0' * 40,
            ('rev-parse', self.value['sourceCommit'] + '^{tree}'): '0' * 40,
            ('rev-parse', 'HEAD'): '0' * 40,
            ('status', '--porcelain=v1', '--untracked-files=no'): ' M go.mod',
            ('ls-files', '--others', '--exclude-standard'): 'unexpected.bin',
            ('ls-files', '--others', '--ignored', '--exclude-standard'): 'ignored.embed',
        }
        for args, bad in cases.items():
            self.calls.clear(); self.overrides = {args: bad}
            with self.subTest(args=args), self.assertRaises(RuntimeError): self.check()
            self.assertFalse(any(call[0] == 'ls-tree' for call in self.calls))
        self.overrides = {}

    def test_wrong_vendor_tree_and_tracked_count_fail_closed(self):
        for args, bad in [(('rev-parse', 'HEAD:third_party/sing-tun'), '0' * 40),
                          (('ls-tree', '-r', 'HEAD', '--', 'third_party/sing-tun'), '')]:
            self.overrides = {args: bad}
            with self.subTest(args=args), self.assertRaises(RuntimeError): self.check()


if __name__ == '__main__':
    unittest.main()
