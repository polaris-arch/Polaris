"""Real local Git transport controls; no Go/SDK/build/publication acceptance."""
from copy import deepcopy
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import ci_android
from fork_input import git_environment, verify_source
from test_source_policy import future_mobile_fixture


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
        self.real_run = ci_android.run

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.remote), *args],
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


if __name__ == '__main__':
    unittest.main()
