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
        self.samples(range(15, 376, 15))

    def samples(self, times):
        rows = ["t\twebproc\twindow\tcolors\tlog_lines\tidentity"]
        for elapsed in times:
            (self.evidence / f"t{elapsed}.png").write_bytes(b"fixture PNG")
            rows.append(f"{elapsed}\t1\t1400x900\t20\t3\t123:456")
        (self.evidence / "timeline.tsv").write_text("\n".join(rows) + "\n")
        self.seal()

    def seal(self):
        self.record["sha256"] = {
            name: hashlib.sha256((self.evidence / name).read_bytes()).hexdigest()
            for name in ("polaris.log", "timeline.tsv")
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
        self.assertTrue(source.rstrip().endswith('python3 "$HERE/assert.py" "$OUT"\nexit $?'))
        self.assertNotIn('LIVED=', source)
        self.assertIn('final_identity=sys.argv[7]', source)
        self.assertIn('exec /usr/bin/polaris', source)


if __name__ == "__main__":
    unittest.main()
