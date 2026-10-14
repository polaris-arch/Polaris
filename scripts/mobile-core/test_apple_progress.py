"""Inert Python children control pipe progress; no native producer runs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import producer
import build_apple_app
import native_inputs


class AppleProgress(unittest.TestCase):
    def test_raw_log_flushes_before_exit_and_stdout_decodes_split_utf8(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); log = root / 'original.log'; release = root / 'release'
            stdout_log, stderr_log = Path(str(log)+'.stdout.raw'), Path(str(log)+'.stderr.raw')
            result, failures = [], []
            code = ('import os,time; os.write(1,b"first\\r\\n\\xe2"); os.write(2,b"error-side"); '
                    'end=time.monotonic()+5\n'
                    'while not os.path.exists(' + repr(str(release)) + ') and time.monotonic()<end: time.sleep(.01)\n'
                    'os.write(1,b"\\x82\\xac\\nlast")')
            def invoke():
                try:
                    result.append(producer.run([sys.executable, '-c', code], log=log))
                except BaseException as error:
                    failures.append(error)
            with patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False):
                worker = threading.Thread(target=invoke); worker.start()
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    if stdout_log.exists() and stderr_log.exists() and b'first' in stdout_log.read_bytes() and b'error-side' in stderr_log.read_bytes():
                        break
                    time.sleep(.01)
                observed = stdout_log.read_bytes() if stdout_log.exists() else b''
                observed_stderr = stderr_log.read_bytes() if stderr_log.exists() else b''
                alive_before_release = worker.is_alive()
                release.touch(); worker.join(timeout=6)
            self.assertFalse(worker.is_alive()); self.assertEqual(failures, [])
            self.assertTrue(alive_before_release)
            self.assertIn(b'first\r\n\xe2', observed); self.assertIn(b'error-side', observed_stderr)
            self.assertEqual(result, ['first\n€\nlast'])
            self.assertEqual(log.read_bytes(), b'first\r\n\xe2\x82\xac\nlast' + b'error-side')
            self.assertEqual(stdout_log.read_bytes(), b'first\r\n\xe2\x82\xac\nlast')
            self.assertEqual(stderr_log.read_bytes(), b'error-side')

    def test_nonzero_and_timeout_remain_failures_with_full_original_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False):
                with self.assertRaisesRegex(RuntimeError, 'actual tool failed'):
                    producer.run([sys.executable, '-c', 'import os; os.write(1,b"out");os.write(2,b"err");raise SystemExit(7)'], log=root/'failed.log')
                self.assertIn(b'out', (root/'failed.log').read_bytes()); self.assertIn(b'err', (root/'failed.log').read_bytes())
                with patch.object(producer, 'RUN_SECONDS', .15), self.assertRaises(subprocess.TimeoutExpired):
                    producer.run([sys.executable, '-c', 'import os,time;os.write(1,b"before-timeout");time.sleep(8)'], log=root/'timeout.log')
                self.assertIn(b'before-timeout', (root/'timeout.log').read_bytes())

    def test_safe_events_have_actual_bytes_without_tool_payload_and_no_native_fd(self):
        with tempfile.TemporaryFile() as events, tempfile.TemporaryDirectory() as temporary:
            fd = events.fileno()
            code = ('import os\ntry: os.fstat(' + str(fd) + '); print("LEAK")\n'
                    'except OSError: print("closed")\n'
                    'os.write(1,b"INJECTED_SECRET_ORIGINAL_TEXT")')
            with patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False):
                returned = producer.run([sys.executable, '-c', code], log=Path(temporary)/'native.log',
                    observation_fd=fd, observation_phase='rust-target-build', observation_slot='sim-amd64')
            events.seek(0); raw = events.read().decode()
            self.assertNotIn('INJECTED_SECRET', raw); self.assertNotIn(temporary, raw)
            self.assertIn('closed\n', returned); self.assertNotIn('LEAK', returned)
            rows = [json.loads(line.split('=', 1)[1]) for line in raw.splitlines()]
            self.assertEqual(rows[-1]['event'], 'end'); self.assertGreater(rows[-1]['stdoutBytes'], 0)
            self.assertIsNone(rows[-1]['peakRSSBytes']); self.assertFalse(rows[-1]['processRunning'])
            self.assertEqual(rows[-1]['slot'], 'sim-amd64')

    def test_cleanup_failures_do_not_replace_original_or_release_live_group(self):
        class BadLog:
            def __enter__(self): return self
            def __exit__(self, *args): raise OSError('SECONDARY_LOG_CLOSE_FAILURE')
            def write(self, raw): raise OSError('PRIMARY_LOG_FAILURE')
            def flush(self): pass
        real_kill = os.killpg
        calls = []
        def fail_first(group, sig):
            calls.append(sig)
            if len(calls) == 1: raise PermissionError('SECONDARY_CLEANUP_FAILURE')
            return real_kill(group, sig)
        with patch.object(Path, 'open', return_value=BadLog()), patch.object(producer.os, 'killpg', side_effect=fail_first), \
                patch.object(producer, 'group_alive', return_value=True), patch.object(producer, 'register_group') as register:
            with self.assertRaisesRegex(OSError, 'PRIMARY_LOG_FAILURE') as raised:
                producer.run([sys.executable, '-c', 'import os,time;os.write(1,b"trigger");time.sleep(5)'], log='unused')
        self.assertGreaterEqual(len(calls), 2)
        self.assertFalse(any(call.kwargs.get('finished') for call in register.call_args_list))
        self.assertIn('cleanup incomplete', ' '.join(raised.exception.__notes__))

    def test_broken_progress_cancels_child_and_preserves_primary_error(self):
        with patch.object(producer.os, 'write', side_effect=BrokenPipeError('PRIMARY_PROGRESS_FAILURE')), \
                patch.object(producer, 'group_alive', return_value=False), patch.object(producer, 'register_group') as register:
            with self.assertRaisesRegex(BrokenPipeError, 'PRIMARY_PROGRESS_FAILURE'):
                producer.run([sys.executable, '-c', 'import time;time.sleep(5)'], observation_fd=99)
        self.assertTrue(any(call.kwargs.get('finished') for call in register.call_args_list))

    def test_phase_end_observer_does_not_hide_operation_failure(self):
        def fail(): raise ValueError('PRIMARY_OPERATION')
        with patch.object(build_apple_app, 'phase_progress', side_effect=[None, BrokenPipeError('SECONDARY')]):
            with self.assertRaisesRegex(ValueError, 'PRIMARY_OPERATION'):
                build_apple_app.timed_phase('final-carrier-inspection', fail)
        self.assertIsNone(build_apple_app.CURRENT_PHASE)

    def test_cancellation_keeps_original_and_waits_child(self):
        with patch.object(producer, 'progress_write', side_effect=KeyboardInterrupt('CONTROL_CANCELLED')), \
                patch.object(producer, 'group_alive', return_value=False), patch.object(producer, 'register_group') as register:
            with self.assertRaisesRegex(KeyboardInterrupt, 'CONTROL_CANCELLED'):
                producer.run([sys.executable, '-c', 'import time;time.sleep(5)'], observation_fd=99)
        self.assertTrue(any(call.kwargs.get('finished') for call in register.call_args_list))

    def test_original_native_parser_receives_complete_utf8_work_and_merge_lines(self):
        # Only downstream archive/member classification is inert. collect's
        # real read_text, WORK regex, shlex merge parsing and file pinning run.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); scratch = root/'scratch'; cache = root/'cache'
            work = scratch/'gomobile-work'; (work/'ios').mkdir(parents=True); cache.mkdir()
            (work/'ios/object.o').write_bytes(b'inert native object')
            archive = cache/'runtime.a'; archive.write_bytes(b'inert static archive')
            log = root/'bind.log'
            stdout_parts = [b'# UTF8 \xe2', b'\x82\xac\nWO',
                ('RK=' + str(work) + '\nlibtool -static -o $WORK/merged.a $WORK/go.a ').encode(),
                str(archive).encode(), b'\n']
            stderr_parts = [b'# diagnostic-one\n', b'# diagnostic-two\n', b'# diagnostic-three\n']
            operations = []
            for index, part in enumerate(stdout_parts):
                operations.append('os.write(1,'+repr(part)+');time.sleep(.03)')
                if index < len(stderr_parts): operations.append('os.write(2,'+repr(stderr_parts[index])+');time.sleep(.03)')
            with patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False):
                producer.run([sys.executable, '-c', 'import os,time;'+ ';'.join(operations)], log=log)
            wanted_stdout, wanted_stderr = b''.join(stdout_parts), b''.join(stderr_parts)
            self.assertEqual(Path(str(log)+'.stdout.raw').read_bytes(), wanted_stdout)
            self.assertEqual(Path(str(log)+'.stderr.raw').read_bytes(), wanted_stderr)
            self.assertEqual(log.read_bytes(), wanted_stdout + wanted_stderr)
            with patch.object(native_inputs, 'source_table', return_value={}), \
                    patch.object(native_inputs, 'member_references', return_value=({}, {})):
                actual = native_inputs.collect(scratch, log, 'apple', {}, cache)
                self.assertEqual(actual['workRoots'], [str(work.resolve())])
                self.assertEqual(actual['files'][str(archive.resolve())]['kind'], 'external-static-archive')
                self.assertIn(str((work/'ios/object.o').resolve()), actual['files'])
                # Recreate the old arbitrary-stream concatenation failures.
                broken_utf8 = root/'broken-utf8.log'
                broken_utf8.write_bytes(stdout_parts[0] + stderr_parts[0] + b''.join(stdout_parts[1:]))
                with self.assertRaises(UnicodeError):
                    native_inputs.collect(scratch, broken_utf8, 'apple', {}, cache)
                broken_work = root/'broken-work.log'
                broken_work.write_bytes(wanted_stdout.replace(b'WORK=', b'WO# error\nRK=', 1))
                with self.assertRaisesRegex(RuntimeError, 'WORK roots'):
                    native_inputs.collect(scratch, broken_work, 'apple', {}, cache)
                broken_merge = root/'broken-merge.log'
                broken_merge.write_bytes(wanted_stdout.replace(str(archive).encode(), b'# stderr\n'+str(archive).encode(), 1))
                with self.assertRaises((OSError, RuntimeError)):
                    native_inputs.collect(scratch, broken_merge, 'apple', {}, cache)
