"""Pure injected proc/child fixtures. Never launch or signal any real process."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
spec = importlib.util.spec_from_file_location('guard', Path(__file__).with_name('evidence_guard.py'))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class GuardTests(unittest.TestCase):
    def test_default_strips_all_inherited_escape_flags(self):
        inherited = dict.fromkeys(guard.FLAGS, '1')
        inherited['DISPLAY'] = ':99'
        self.assertEqual(guard.graphics_environment(inherited, 'default'), {'DISPLAY': ':99'})
        self.assertEqual(guard.graphics_environment(inherited, 'escape'), inherited)

    def records(self):
        return {10:dict(identity='10:100', parent=1, executable='polaris', env={'DISPLAY':':99'}),
                11:dict(identity='11:101', parent=10, executable='WebKitWebProcess', env={'DISPLAY':':99'}),
                12:dict(identity='12:102', parent=1, executable='WebKitWebProcess', env={'DISPLAY':':99'})}

    def test_only_owned_same_display_renderer_counts(self):
        records = self.records()
        sample = guard.graphics_sample('10:100', ':99', 'default', records, records.__getitem__)
        self.assertEqual(sample['witnesses'], [['11:101', '10:100']])
        records[11]['env']['DISPLAY'] = ':98'
        self.assertEqual(guard.graphics_sample('10:100', ':99', 'default', records, records.__getitem__)['witnesses'], [])

    def test_reused_root_or_escape_flag_fails(self):
        records = self.records()
        with self.assertRaises(ValueError):
            guard.graphics_sample('10:999', ':99', 'default', records, records.__getitem__)
        records[10]['env'][guard.FLAGS[0]] = '1'
        with self.assertRaises(ValueError):
            guard.graphics_sample('10:100', ':99', 'default', records, records.__getitem__)

    def test_owned_child_term_then_kill_and_bounded_waits(self):
        class Child:
            def __init__(self): self.calls=[]
            def poll(self): return None
            def terminate(self): self.calls.append('TERM')
            def kill(self): self.calls.append('KILL')
            def wait(self, timeout):
                self.calls.append(('wait', timeout))
                if 'KILL' not in self.calls: raise subprocess.TimeoutExpired('mock', timeout)
        child = Child()
        self.assertTrue(guard.settle(child))
        self.assertEqual(child.calls, ['TERM', ('wait',5), 'KILL', ('wait',5)])

    def test_unreapable_child_is_unknown_after_finite_wait(self):
        class Child:
            def poll(self): return None
            def terminate(self): pass
            def kill(self): pass
            def wait(self, timeout): raise subprocess.TimeoutExpired('mock', timeout)
        self.assertFalse(guard.settle(Child()))

    def test_failed_xvfb_startup_is_reaped_but_never_success(self):
        class Child:
            returncode = 1
            def poll(self): return 1
            def wait(self, timeout): return 1
            def terminate(self): raise AssertionError('must not signal exited child')
            def kill(self): raise AssertionError('must not signal exited child')
        receipt = guard.monitor_owned(Child(), '20:200', 10, lambda: True,
                                      clock=lambda: 0, read=lambda _: self.fail('exited child cannot be live'))
        self.assertTrue(receipt['root_reaped'])
        self.assertEqual(receipt['reason'], 'natural_exit')
        self.assertEqual(receipt['exit_code'], 1)

    def test_owned_live_cancellation_binds_cleanup_to_birth(self):
        class Child:
            returncode = None
            def poll(self): return self.returncode
            def terminate(self): self.returncode = -15
            def kill(self): raise AssertionError('normal teardown settled')
            def wait(self, timeout): return self.returncode
        receipt = guard.monitor_owned(Child(), '20:200', 10, lambda: True,
                                      clock=lambda: 0, read=lambda _: {'identity':'20:200'})
        self.assertEqual(receipt, dict(root_reaped=True, identity='20:200', reason='cancelled', exit_code=-15))
        with self.assertRaises(ValueError):
            guard.live_identity('20:200', lambda _: {'identity':'20:999'})

    def test_owned_deadline_is_not_requested_teardown(self):
        class Child:
            returncode = None
            def poll(self): return self.returncode
            def terminate(self): self.returncode = -15
            def wait(self, timeout): return self.returncode
        times = iter([0, 11])
        receipt = guard.monitor_owned(Child(), '20:200', 10, lambda: False,
                                      clock=lambda: next(times), read=lambda _: {'identity':'20:200'})
        self.assertEqual(receipt['reason'], 'deadline')
        self.assertTrue(receipt['root_reaped'])


if __name__ == '__main__':
    unittest.main()
