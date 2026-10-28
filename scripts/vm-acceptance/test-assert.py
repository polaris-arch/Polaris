"""Synthetic evidence only. No VM, application, network or screenshot commands."""
import hashlib
import importlib.util
import json
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("vm_assert", pathlib.Path(__file__).with_name("assert.py"))
subject = importlib.util.module_from_spec(spec)
spec.loader.exec_module(subject)


class ObservationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.evidence = pathlib.Path(self.tmp.name) / "ts-no-exit"
        self.evidence.mkdir()
        self.log = "\n".join(["sing-box 已就绪", subject.BLOCKED, subject.MESH_FALLBACK]) + "\n"
        (self.evidence / "polaris.log").write_text(self.log)
        (self.evidence / "graphics.txt").write_text("graphics=escape\n")
        self.record = dict(start=1000.0, end=1375.1, warmup=75, requested=300,
                           identity="123:456", final_identity="123:456", completed=True)
        self.provenance = dict(batch='a'*32, boot_id='12345678-1234-1234-1234-123456789abc', graphics='escape', source_sha256='b'*64,
                               binary_sha256='c'*64, common_config_sha256='d'*64)
        (self.evidence / 'provenance.json').write_text(json.dumps(self.provenance))
        for label in ('app', 'xvfb'):
            (self.evidence / f'{label}.cleanup.json').write_text(json.dumps(dict(root_reaped=True, reason='cancelled', identity='123:456' if label=='app' else '120:450', exit_code=-15)))
        self.samples(range(15, 376, 15))

    def samples(self, times):
        samples = []
        rows = ["t\twebproc\twindow\tcolors\tlog_lines\tidentity"]
        for elapsed in times:
            (self.evidence / f"t{elapsed}.png").write_bytes(b"fixture PNG")
            samples.append(dict(xvfb_identity='120:450', identity='123:456', display=':99', leg='escape',
                                effective={key: '1' for key in subject.FLAGS}, elapsed=str(elapsed),
                                witnesses=[['124:457', '123:456']]))
            rows.append(f"{elapsed}\t1\t1400x900\t20\t3\t123:456")
        (self.evidence / "timeline.tsv").write_text("\n".join(rows) + "\n")
        (self.evidence / 'graphics-samples.jsonl').write_text(''.join(json.dumps(sample)+'\n' for sample in samples))
        self.seal()

    def seal(self):
        self.record["sha256"] = {
            name: hashlib.sha256((self.evidence / name).read_bytes()).hexdigest()
            for name in subject.BOUND_FILES
        }
        (self.evidence / "observation.json").write_text(json.dumps(self.record))

    def test_complete_requested_window(self):
        self.assertEqual(subject.check(self.evidence), 0)

    def test_early_exit_with_five_early_frames(self):
        self.samples(range(15, 76, 15))
        self.record.update(end=1080.0, completed=False, final_identity="")
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_enough_elapsed_time_without_terminal_sample(self):
        self.samples(range(15, 76, 15))
        self.assertEqual(subject.check(self.evidence), 2)

    def test_pid_reuse_is_not_liveness(self):
        self.record["final_identity"] = "123:999"
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_truncated_log_cannot_reuse_completion(self):
        (self.evidence / "polaris.log").write_text(self.log.rstrip("\n"))
        self.assertEqual(subject.check(self.evidence), 2)

    def test_truncated_timeline_cannot_reuse_completion(self):
        (self.evidence / "timeline.tsv").write_text("t\twebproc\n15\t1\n")
        self.assertEqual(subject.check(self.evidence), 2)

    def test_resealed_log_shorter_than_sampled_log_is_invalid(self):
        (self.evidence / "polaris.log").write_text("sing-box 已就绪\n")
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_missing_terminal_screenshot(self):
        (self.evidence / "t375.png").unlink()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_invalid_window_values_fail_closed(self):
        for key, value in [("requested", 0), ("requested", -1), ("end", float("nan")), ("warmup", -1)]:
            with self.subTest(key=key, value=value):
                old = self.record[key]
                self.record[key] = value
                self.seal()
                self.assertEqual(subject.check(self.evidence), 2)
                self.record[key] = old
        self.seal()

    def test_legacy_markerless_evidence_is_unavailable(self):
        (self.evidence / "observation.json").unlink()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_runner_delegates_final_exit_to_same_assertion(self):
        # Static contract supplements consumer fixtures; this never invokes run.sh.
        source = pathlib.Path(__file__).with_name("run.sh").read_text()
        self.assertIn('python3 "$HERE/assert.py" "$CONTROL_EVIDENCE" "$OUT"', source)
        self.assertNotIn('pgrep', source)
        self.assertIn('evidence_guard.py" own', source)

    def test_default_rejects_inherited_escape_environment(self):
        (self.evidence / 'graphics.txt').write_text('graphics=default\n')
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_unowned_renderer_chain_rejected(self):
        path = self.evidence / 'graphics-samples.jsonl'
        path.write_text(path.read_text().replace('123:456"]', '999:456"]'))
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_failed_root_cleanup_rejected(self):
        (self.evidence / 'app.cleanup.json').write_text('{"root_reaped": false}')
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_reaped_but_failed_xvfb_startup_is_unavailable(self):
        (self.evidence / 'xvfb.cleanup.json').write_text(json.dumps(dict(
            root_reaped=True, reason='natural_exit', identity='120:450', exit_code=1)))
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_replaced_xvfb_birth_is_unavailable(self):
        path = self.evidence / 'xvfb.cleanup.json'
        path.write_text(path.read_text().replace('120:450', '120:999'))
        self.seal()
        self.assertEqual(subject.check(self.evidence), 2)

    def test_standalone_silence_cannot_pass_batch(self):
        self.assertEqual(subject.check_batch([self.evidence]), 2)

    def make_control(self):
        import shutil
        control = self.evidence.parent / 'plain-broken'
        shutil.copytree(self.evidence, control)
        log = '\n'.join(['sing-box 已就绪', subject.ENABLED, subject.PROBE_FAIL, subject.TRIGGERED])+'\n'
        (control / 'polaris.log').write_text(log)
        record = dict(self.record, start=0.0, end=375.1)
        record['sha256'] = {name: hashlib.sha256((control/name).read_bytes()).hexdigest() for name in subject.BOUND_FILES}
        (control / 'observation.json').write_text(json.dumps(record))
        return control

    def test_same_batch_positive_and_silence(self):
        self.assertEqual(subject.check_batch([self.make_control(), self.evidence]), 0)

    def test_mismatched_or_late_control(self):
        control = self.make_control()
        for key in self.provenance:
            original = self.provenance[key]
            self.provenance[key] = 'f'*len(original)
            (self.evidence / 'provenance.json').write_text(json.dumps(self.provenance))
            self.seal()
            self.assertEqual(subject.check_batch([control, self.evidence]), 2)
            self.provenance[key] = original
        (self.evidence / 'provenance.json').write_text(json.dumps(self.provenance))
        self.record['start'], self.record['end'] = 0, 375.1
        self.seal()
        self.assertEqual(subject.check_batch([control, self.evidence]), 2)

    def test_failed_positive_control_invalidates_negative(self):
        control = self.make_control()
        (control / 'polaris.log').write_text('sing-box 已就绪\n')
        self.assertNotEqual(subject.check_batch([control, self.evidence]), 0)



if __name__ == "__main__":
    unittest.main()
