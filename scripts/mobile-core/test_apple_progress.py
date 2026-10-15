"""Inert Python children control pipe progress; no native producer runs."""
import json
import io
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
        for capture in ('full', 'none'):
            with self.subTest(capture=capture):
                self._check_raw_log_flushes_before_exit(capture)

    def _check_raw_log_flushes_before_exit(self, capture):
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
                    kwargs = {} if capture == 'full' else {'capture': capture}
                    result.append(producer.run([sys.executable, '-c', code], log=log, **kwargs))
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
            self.assertEqual(result, ['first\n€\nlast'] if capture == 'full' else [None])
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
                    observation_fd=fd, observation_phase='rust-target-build', observation_slot='sim-arm64')
            events.seek(0); raw = events.read().decode()
            self.assertNotIn('INJECTED_SECRET', raw); self.assertNotIn(temporary, raw)
            self.assertIn('closed\n', returned); self.assertNotIn('LEAK', returned)
            rows = [json.loads(line.split('=', 1)[1]) for line in raw.splitlines()]
            self.assertEqual(rows[-1]['event'], 'end'); self.assertGreater(rows[-1]['stdoutBytes'], 0)
            self.assertIsNone(rows[-1]['peakRSSBytes']); self.assertFalse(rows[-1]['processRunning'])
            self.assertEqual(rows[-1]['slot'], 'sim-arm64')

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
            none_log = root/'bind-none.log'
            with patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False):
                self.assertIsNone(producer.run([sys.executable, '-c', 'import os,time;'+ ';'.join(operations)],
                                              log=none_log, capture='none'))
            for suffix in ('', '.stdout.raw', '.stderr.raw'):
                self.assertEqual(Path(str(none_log)+suffix).read_bytes(), Path(str(log)+suffix).read_bytes())
            with patch.object(native_inputs, 'source_table', return_value={}), \
                    patch.object(native_inputs, 'member_references', return_value=({}, {})):
                actual = native_inputs.collect(scratch, log, 'apple', {}, cache)
                self.assertEqual(native_inputs.collect(scratch, none_log, 'apple', {}, cache), actual)
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


class BoundedRunCapture(unittest.TestCase):
    class FaultLog:
        """One storage fault over a real private file; never a native tool."""
        def __init__(self, stream, operation):
            self.stream, self.operation, self.triggered = stream, operation, False

        def __getattr__(self, name):
            return getattr(self.stream, name)

        def __enter__(self):
            return self

        def __exit__(self, *args):
            self.stream.close()
            self.fail('close')

        def fail(self, operation):
            if self.operation == operation and not self.triggered:
                self.triggered = True
                raise OSError('CONTROL_STORAGE_' + operation.upper())

        def write(self, raw):
            self.fail('write')
            if self.operation == 'short-write' and not self.triggered:
                self.triggered = True
                return self.stream.write(raw[:-1])
            return self.stream.write(raw)

        def flush(self):
            self.fail('flush')
            return self.stream.flush()

        def read(self, size):
            self.fail('read')
            if self.operation == 'early-eof' and not self.triggered:
                self.triggered = True
                return b''
            return self.stream.read(size)

        def seek(self, offset):
            self.fail('seek')
            return self.stream.seek(offset)

    @staticmethod
    def command(stdout, stderr, exit_code=0):
        # Both inert pipes can exceed their OS capacity without serial-drain
        # deadlock. Each stream's bytes remain deterministic.
        return [sys.executable, '-c',
                'import os,threading\n'
                'def emit(fd,data):\n'
                ' while data:\n'
                '  size=os.write(fd,data[:4093]);data=data[size:]\n'
                'a=threading.Thread(target=emit,args=(1,' + repr(stdout) + '))\n'
                'b=threading.Thread(target=emit,args=(2,' + repr(stderr) + '))\n'
                'a.start();b.start();a.join();b.join()\n'
                'raise SystemExit(' + str(exit_code) + ')']

    def test_none_matches_default_and_explicit_full_for_empty_and_long_dual_streams(self):
        # Large bytes are generated inside the child to avoid argv size limits.
        cases = [
            ('empty', b'', b''),
            ('split', b'head\r\n\xe2\x82\xac\rtail\r', b'error\r\n\xe2\x82\xac\r'),
            ('unterminated', b'x' * 32000, b'y' * 32000),
        ]
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'register_group'), \
                patch.object(producer, 'group_alive', return_value=False):
            root = Path(temporary)
            for name, stdout, stderr in cases:
                results = []
                for mode in (None, 'full', 'none'):
                    log = root / (name + '-' + str(mode))
                    kwargs = {} if mode is None else {'capture': mode}
                    with tempfile.TemporaryFile() as events, patch.object(producer, 'RUN_CHUNK_BYTES', 7 if name == 'split' else 65536):
                        result = producer.run(self.command(stdout, stderr), log=log,
                                              observation_fd=events.fileno(), **kwargs)
                        events.seek(0)
                        rows = [json.loads(line.split(b'=', 1)[1]) for line in events]
                    self.assertEqual(rows[-1]['stdoutBytes'], len(stdout))
                    self.assertEqual(rows[-1]['stderrBytes'], len(stderr))
                    self.assertIsNone(rows[-1]['peakRSSBytes'])
                    self.assertEqual(log.read_bytes(), stdout + stderr)
                    self.assertEqual(Path(str(log)+'.stdout.raw').read_bytes(), stdout)
                    self.assertEqual(Path(str(log)+'.stderr.raw').read_bytes(), stderr)
                    if mode == 'none':
                        self.assertIsNone(result)
                    else:
                        self.assertEqual(result, stdout.decode().replace('\r\n', '\n').replace('\r', '\n'))
                    results.append(log.read_bytes())
                self.assertEqual(results, [stdout + stderr] * 3)
            stdout, stderr = b'A' * (2 * 1024 * 1024), b'B' * (2 * 1024 * 1024)
            code = ('import os,threading\n'
                    'def emit(fd,byte):\n'
                    ' for unused in range(512): os.write(fd,byte*4096)\n'
                    'a=threading.Thread(target=emit,args=(1,b"A"));b=threading.Thread(target=emit,args=(2,b"B"))\n'
                    'a.start();b.start();a.join();b.join()')
            for mode in ('full', 'none'):
                log = root / ('large-' + mode)
                result = producer.run([sys.executable, '-c', code], log=log, capture=mode)
                self.assertEqual(result, stdout.decode() if mode == 'full' else None)
                self.assertEqual(log.read_bytes(), stdout + stderr)
                self.assertEqual(Path(str(log)+'.stdout.raw').read_bytes(), stdout)
                self.assertEqual(Path(str(log)+'.stderr.raw').read_bytes(), stderr)

    def test_none_append_projects_only_each_calls_raw_offsets(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'register_group'), \
                patch.object(producer, 'group_alive', return_value=False):
            log = Path(temporary)/'repeated.log'
            old_out, old_err, old_merged = b'\xffold-out', b'older-stderr\xfe', b'existing projection\n'
            Path(str(log)+'.stdout.raw').write_bytes(old_out)
            Path(str(log)+'.stderr.raw').write_bytes(old_err)
            log.write_bytes(old_merged)
            for stdout, stderr in ((b'first\r\n', b'error-one'), (b'second', b'\rerror-two\r\n'), (b'', b'')):
                self.assertIsNone(producer.run(self.command(stdout, stderr), log=log, capture='none'))
                old_out += stdout; old_err += stderr; old_merged += stdout + stderr
                self.assertEqual(Path(str(log)+'.stdout.raw').read_bytes(), old_out)
                self.assertEqual(Path(str(log)+'.stderr.raw').read_bytes(), old_err)
                self.assertEqual(log.read_bytes(), old_merged)

    def test_none_strict_decoding_rejects_both_streams_without_losing_original_bytes(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'register_group'), \
                patch.object(producer, 'group_alive', return_value=False):
            root = Path(temporary)
            for stream in ('stdout', 'stderr'):
                for bad in (b'prefix\xffsuffix', b'trailing\xe2\x82'):
                    stdout, stderr = (bad, b'valid stderr') if stream == 'stdout' else (b'valid stdout', bad)
                    for mode in ('full', 'none'):
                        log = root / (stream + '-' + str(len(bad)) + '-' + mode)
                        with patch.object(producer, 'RUN_CHUNK_BYTES', 2), self.assertRaises(UnicodeError):
                            producer.run(self.command(stdout, stderr), log=log, capture=mode)
                        self.assertEqual(log.read_bytes(), stdout + stderr)
                        self.assertEqual(Path(str(log)+'.stdout.raw').read_bytes(), stdout)
                        self.assertEqual(Path(str(log)+'.stderr.raw').read_bytes(), stderr)

    def test_none_nonzero_uses_same_bounded_decoded_stderr_tail_as_full(self):
        stderr = ('discarded-head\n' + ('€\r\n界\r' * 2500) + 'last\r').encode()
        stdout = b'ignored output\n'
        expected = stderr.decode().replace('\r\n', '\n').replace('\r', '\n')[-producer.RUN_ERROR_CHARS:]
        failures = []
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'register_group'), \
                patch.object(producer, 'group_alive', return_value=False):
            for mode in ('full', 'none'):
                log = Path(temporary)/mode
                with patch.object(producer, 'RUN_CHUNK_BYTES', 127), self.assertRaises(RuntimeError) as raised:
                    producer.run(self.command(stdout, stderr, 7), log=log, capture=mode)
                failures.append(str(raised.exception))
                self.assertEqual(str(raised.exception), 'actual tool failed (' + sys.executable + '): ' + expected)
                self.assertNotIn('discarded-head', str(raised.exception))
                self.assertEqual(log.read_bytes(), stdout + stderr)
            self.assertEqual(failures[0], failures[1])

    def test_projection_uses_fixed_read_sizes_and_tail_not_previous_or_unbounded_output(self):
        class RangeReader(io.BytesIO):
            def read(self, size=-1):
                self.assert_read(size)
                return super().read(size)
        stdout, stderr = b'x' * 10000 + b'\r\n', ('€\r\n' * 5000).encode()
        sources = {'stdout': RangeReader(b'\xffold' + stdout), 'stderr': RangeReader(b'\xfeold' + stderr)}
        sizes = []
        for source in sources.values():
            def checked(size):
                sizes.append(size)
                self.assertGreater(size, 0)
                self.assertLessEqual(size, 113)
            source.assert_read = checked
        sink = io.BytesIO()
        with patch.object(producer, 'RUN_CHUNK_BYTES', 113):
            tail = producer._project_run_logs(sink, sources, {'stdout': 4, 'stderr': 4},
                                             {'stdout': len(stdout), 'stderr': len(stderr)}, 'utf-8', 'strict')
        self.assertGreater(len(sizes), 2)
        self.assertEqual(sink.getvalue(), stdout + stderr)
        self.assertEqual(tail, ('€\n' * 5000)[-producer.RUN_ERROR_CHARS:])

    def test_none_timeout_escalates_term_to_kill_without_replacing_failure(self):
        calls, processes = [], []
        real_kill, real_spawn = os.killpg, subprocess.Popen
        def kill(group, sig):
            calls.append(sig)
            return real_kill(group, sig)
        def spawn(*args, **kwargs):
            process = real_spawn(*args, **kwargs); processes.append(process); return process
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'register_group') as register, \
                patch.object(producer, 'group_alive', return_value=False), patch.object(producer, 'RUN_SECONDS', .2), \
                patch.object(producer, 'RUN_TERM_SECONDS', .1), patch.object(producer.os, 'killpg', side_effect=kill), \
                patch.object(producer.subprocess, 'Popen', side_effect=spawn):
            log = Path(temporary)/'timeout.log'
            code = 'import os,signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);os.write(1,b"ready");time.sleep(8)'
            with self.assertRaises(subprocess.TimeoutExpired):
                producer.run([sys.executable, '-c', code], log=log, capture='none')
            self.assertEqual(log.read_bytes(), b'ready')
        self.assertIn(producer.signal.SIGTERM, calls); self.assertIn(producer.signal.SIGKILL, calls)
        self.assertTrue(any(call.kwargs.get('finished') for call in register.call_args_list))
        self.assertIsNotNone(processes[0].poll())
        self.assertTrue(processes[0].stdout.closed and processes[0].stderr.closed)

    def test_none_progress_failure_and_cancellation_preserve_primary_and_close_pipes(self):
        real_spawn = subprocess.Popen
        for failure in (BrokenPipeError('PRIMARY_PROGRESS_FAILURE'), KeyboardInterrupt('CONTROL_CANCELLED')):
            processes = []
            def spawn(*args, **kwargs):
                process = real_spawn(*args, **kwargs); processes.append(process); return process
            with self.subTest(failure=type(failure)), tempfile.TemporaryDirectory() as temporary, \
                    patch.object(producer, 'progress_write', side_effect=failure), \
                    patch.object(producer, 'group_alive', return_value=False), \
                    patch.object(producer, 'register_group') as register, \
                    patch.object(producer.subprocess, 'Popen', side_effect=spawn), \
                    self.assertRaises(type(failure)) as raised:
                producer.run([sys.executable, '-c', 'import time;time.sleep(5)'],
                             log=Path(temporary)/'cancel.log', capture='none', observation_fd=99)
            self.assertIs(raised.exception, failure)
            self.assertTrue(any(call.kwargs.get('finished') for call in register.call_args_list))
            self.assertIsNotNone(processes[0].poll())
            self.assertTrue(processes[0].stdout.closed and processes[0].stderr.closed)

    def test_none_live_descendants_keep_ledger_ownership_and_primary_failure(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'group_alive', return_value=True), \
                patch.object(producer.os, 'killpg'), patch.object(producer, 'register_group') as register, \
                self.assertRaisesRegex(RuntimeError, 'still owns descendants') as raised:
            producer.run(self.command(b'out', b'err'), log=Path(temporary)/'group.log', capture='none')
        self.assertFalse(any(call.kwargs.get('finished') for call in register.call_args_list))
        self.assertIn('cleanup incomplete', ' '.join(raised.exception.__notes__))

    def test_none_keeps_lease_fd_but_not_borrowed_progress_in_native_child(self):
        with tempfile.TemporaryFile() as lease, tempfile.TemporaryFile() as events, tempfile.TemporaryDirectory() as temporary:
            active = [lease.fileno()]
            code = ('import os\nos.fstat(' + str(lease.fileno()) + ');print("lease-open")\n'
                    'try:os.fstat(' + str(events.fileno()) + ');print("LEAK")\n'
                    'except OSError:print("progress-closed")\n'
                    'os.write(2,b"PRIVATE_PAYLOAD")')
            log = Path(temporary)/'fd.log'
            with patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False), \
                    patch.object(producer, 'ACTIVE_FDS', active), patch.object(producer, 'BORROWED_PROGRESS_FD', events.fileno()), \
                    patch.object(producer, 'BORROWED_PROGRESS_PHASE', 'rust-target-build'):
                self.assertIsNone(producer.run([sys.executable, '-c', code], log=log, capture='none'))
            self.assertEqual(active, [lease.fileno()]); os.fstat(lease.fileno()); os.fstat(events.fileno())
            self.assertIn(b'lease-open\nprogress-closed\n', log.read_bytes()); self.assertNotIn(b'LEAK', log.read_bytes())
            events.seek(0); observations = events.read()
            self.assertNotIn(b'PRIVATE_PAYLOAD', observations)
            rows = [json.loads(line.split(b'=', 1)[1]) for line in observations.splitlines()]
            self.assertEqual(rows[-1]['stderrBytes'], len(b'PRIVATE_PAYLOAD'))
            self.assertIsNone(rows[-1]['peakRSSBytes'])

    def test_none_storage_failures_cannot_pass_and_close_all_owned_files_and_pipes(self):
        real_open, real_spawn = Path.open, subprocess.Popen
        cases = [(suffix, operation) for suffix in ('', '.stdout.raw', '.stderr.raw')
                 for operation in ('write', 'short-write', 'flush', 'close')]
        cases += [(suffix, operation) for suffix in ('.stdout.raw', '.stderr.raw')
                  for operation in ('read', 'early-eof', 'seek')]
        for suffix, operation in cases:
            opened, processes = [], []
            with self.subTest(suffix=suffix, operation=operation), tempfile.TemporaryDirectory() as temporary:
                log = Path(temporary)/'original.log'
                def open_log(path, *args, **kwargs):
                    stream = self.FaultLog(real_open(path, *args, **kwargs),
                                           operation if str(path) == str(log)+suffix else None)
                    opened.append(stream); return stream
                def spawn(*args, **kwargs):
                    process = real_spawn(*args, **kwargs); processes.append(process); return process
                with patch.object(Path, 'open', new=open_log), patch.object(producer.subprocess, 'Popen', side_effect=spawn), \
                        patch.object(producer, 'register_group'), patch.object(producer, 'group_alive', return_value=False), \
                        self.assertRaises((OSError, RuntimeError)) as raised:
                    producer.run(self.command(b'complete-out', b'complete-err'), log=log, capture='none')
                self.assertTrue(all(stream.closed for stream in opened))
                self.assertTrue(any(stream.triggered for stream in opened))
                self.assertIsNotNone(processes[0].poll())
                self.assertTrue(processes[0].stdout.closed and processes[0].stderr.closed)
                self.assertIn('incomplete', ' '.join(getattr(raised.exception, '__notes__', [])))

    def test_none_secondary_close_and_cleanup_failures_do_not_replace_primary_storage_error(self):
        real_open, real_kill = Path.open, os.killpg
        calls, opened = [], []
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary)/'original.log'
            class FailingWriteAndClose(self.FaultLog):
                def __exit__(self, *args):
                    self.stream.close()
                    raise OSError('SECONDARY_CLOSE')
            def open_log(path, *args, **kwargs):
                stream = FailingWriteAndClose(real_open(path, *args, **kwargs),
                                             'write' if str(path).endswith('.stdout.raw') else None)
                opened.append(stream); return stream
            def kill(group, sig):
                calls.append(sig)
                if len(calls) == 1:
                    raise PermissionError('SECONDARY_CLEANUP')
                return real_kill(group, sig)
            with patch.object(Path, 'open', new=open_log), patch.object(producer.os, 'killpg', side_effect=kill), \
                    patch.object(producer, 'group_alive', return_value=True), patch.object(producer, 'register_group') as register, \
                    self.assertRaisesRegex(OSError, 'CONTROL_STORAGE_WRITE') as raised:
                producer.run([sys.executable, '-c', 'import os,time;os.write(1,b"trigger");time.sleep(5)'],
                             log=log, capture='none')
            self.assertGreaterEqual(len(calls), 2)
            self.assertTrue(all(stream.closed for stream in opened))
            self.assertFalse(any(call.kwargs.get('finished') for call in register.call_args_list))
            self.assertIn('cleanup incomplete', ' '.join(raised.exception.__notes__))
            self.assertIn('secondary close', ' '.join(raised.exception.__notes__))

    def test_none_decoding_failure_stays_primary_when_log_close_also_fails(self):
        real_open = Path.open
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary)/'invalid.log'
            def open_log(path, *args, **kwargs):
                return self.FaultLog(real_open(path, *args, **kwargs), 'close')
            with patch.object(Path, 'open', new=open_log), patch.object(producer, 'register_group'), \
                    patch.object(producer, 'group_alive', return_value=False), self.assertRaises(UnicodeError) as raised:
                producer.run(self.command(b'bad\xffout', b'complete err'), log=log, capture='none')
            self.assertEqual(log.read_bytes(), b'bad\xffoutcomplete err')
            self.assertIn('secondary close', ' '.join(raised.exception.__notes__))

    def test_none_open_and_spawn_failures_do_not_leak_resources_or_replace_primary(self):
        real_open = Path.open
        for suffix in ('', '.stdout.raw', '.stderr.raw'):
            opened = []
            with self.subTest(suffix=suffix), tempfile.TemporaryDirectory() as temporary:
                log = Path(temporary)/'open.log'
                def open_log(path, *args, **kwargs):
                    if str(path) == str(log)+suffix:
                        raise OSError('PRIMARY_OPEN_FAILURE')
                    stream = real_open(path, *args, **kwargs); opened.append(stream); return stream
                with patch.object(Path, 'open', new=open_log), patch.object(producer.subprocess, 'Popen') as spawn, \
                        self.assertRaisesRegex(OSError, 'PRIMARY_OPEN_FAILURE'):
                    producer.run(['not-executed'], log=log, capture='none')
                spawn.assert_not_called()
                self.assertTrue(all(stream.closed for stream in opened))
        opened = []
        def open_log(path, *args, **kwargs):
            stream = real_open(path, *args, **kwargs); opened.append(stream); return stream
        with tempfile.TemporaryDirectory() as temporary, patch.object(Path, 'open', new=open_log), \
                patch.object(producer.subprocess, 'Popen', side_effect=OSError('PRIMARY_SPAWN_FAILURE')), \
                patch.object(producer.selectors, 'DefaultSelector') as selector, \
                patch.object(producer, 'register_group') as register:
            selector.return_value.close.side_effect = OSError('SECONDARY_SELECTOR_CLOSE')
            with self.assertRaisesRegex(OSError, 'PRIMARY_SPAWN_FAILURE'):
                producer.run(['not-executed'], log=Path(temporary)/'spawn.log', capture='none')
            selector.return_value.close.assert_called_once(); register.assert_not_called()
        self.assertTrue(all(stream.closed for stream in opened))

    def test_none_selector_close_failure_still_closes_both_pipes(self):
        real_selector, real_spawn = producer.selectors.DefaultSelector, subprocess.Popen
        processes = []
        class BadCloseSelector:
            def __init__(self): self.selector = real_selector()
            def __getattr__(self, name): return getattr(self.selector, name)
            def close(self):
                self.selector.close()
                raise OSError('CONTROL_SELECTOR_CLOSE')
        def spawn(*args, **kwargs):
            process = real_spawn(*args, **kwargs); processes.append(process); return process
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'register_group'), \
                patch.object(producer, 'group_alive', return_value=False), \
                patch.object(producer.selectors, 'DefaultSelector', new=BadCloseSelector), \
                patch.object(producer.subprocess, 'Popen', side_effect=spawn), \
                self.assertRaisesRegex(OSError, 'CONTROL_SELECTOR_CLOSE'):
            producer.run(self.command(b'out', b'err'), log=Path(temporary)/'close.log', capture='none')
        self.assertTrue(processes[0].stdout.closed and processes[0].stderr.closed)
