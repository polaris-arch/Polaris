use super::*;
use std::sync::Mutex as StdMutex;

#[derive(Default)]
struct RecordingSink(StdMutex<Vec<TaildropTaskSnapshot>>);

impl TaildropTaskEventSink for RecordingSink {
    fn updated(&self, snapshot: &TaildropTaskSnapshot) {
        self.0.lock().unwrap().push(snapshot.clone());
    }
}

fn start(runtime: &TaildropRuntime, name: &str, size: u64) -> StartedTaildropTask {
    runtime
        .start_task(
            "server-a".into(),
            "peer-a".into(),
            vec![(name.into(), size)],
        )
        .unwrap()
}

#[test]
fn progress_is_monotonic_clamped_and_completion_is_a_full_terminal_snapshot() {
    let runtime = TaildropRuntime::default();
    let sink = RecordingSink::default();
    let task = start(&runtime, "a.bin", 10);
    runtime.mark_sending(&task.snapshot.task_id, &sink);
    runtime.record_progress(&task.snapshot.task_id, 0, 7, false, &sink);
    runtime.record_progress(&task.snapshot.task_id, 0, 3, false, &sink);
    runtime.record_progress(&task.snapshot.task_id, 0, 99, true, &sink);
    runtime.record_acknowledged(&task.snapshot.task_id, 4, &sink);
    runtime.record_acknowledged(&task.snapshot.task_id, 9, &sink);
    runtime.complete(&task.snapshot.task_id, &sink);

    let snapshot = runtime.snapshots(None).pop().unwrap();
    assert_eq!(snapshot.phase, TaildropTaskPhase::Completed);
    assert_eq!(snapshot.sent_bytes, 10);
    assert_eq!(snapshot.acknowledged_bytes, 10);
    assert_eq!(snapshot.files[0].sent_bytes, 10);
    assert!(snapshot.files[0].completed);
    assert!(snapshot.revision >= 5);
}

#[test]
fn cancellation_is_idempotent_and_signals_the_owned_task() {
    let runtime = TaildropRuntime::default();
    let sink = RecordingSink::default();
    let mut task = start(&runtime, "a.bin", 10);
    let first = runtime.cancel(&task.snapshot.task_id, &sink).unwrap();
    let second = runtime.cancel(&task.snapshot.task_id, &sink).unwrap();
    assert_eq!(first, second);
    assert!(task.cancel.has_changed().unwrap());
    assert!(*task.cancel.borrow_and_update());
    runtime.canceled(&task.snapshot.task_id, &sink);
    assert_eq!(
        runtime.snapshots(None)[0].phase,
        TaildropTaskPhase::Canceled
    );
}

#[test]
fn active_and_terminal_snapshots_are_strictly_bounded() {
    let runtime = TaildropRuntime::default();
    let sink = RecordingSink::default();
    let mut first_id = String::new();
    for i in 0..(MAX_TAILDROP_TASKS + 5) {
        let task = start(&runtime, &format!("{i}.bin"), 1);
        if i == 0 {
            first_id = task.snapshot.task_id.clone();
        }
        runtime.complete(&task.snapshot.task_id, &sink);
    }
    let snapshots = runtime.snapshots(None);
    assert_eq!(snapshots.len(), MAX_TAILDROP_TASKS);
    assert!(!snapshots
        .iter()
        .any(|snapshot| snapshot.task_id == first_id));

    let active = TaildropRuntime::default();
    let _tasks: Vec<_> = (0..MAX_ACTIVE_TAILDROP_TASKS)
        .map(|i| start(&active, &format!("active-{i}"), 1))
        .collect();
    assert_eq!(
        active
            .start_task(
                "server-a".into(),
                "peer-a".into(),
                vec![("extra".into(), 1)]
            )
            .err(),
        Some(TaildropTaskStartError::Busy)
    );
}

#[test]
fn dropping_the_owner_signals_every_active_task() {
    let mut receivers = {
        let runtime = TaildropRuntime::default();
        vec![
            start(&runtime, "a", 1).cancel,
            start(&runtime, "b", 1).cancel,
        ]
    };
    for receiver in &mut receivers {
        // sender 随 owner 一起 drop 时 `has_changed` 可回 RecvError；最后一个已发送的 true 仍可读。
        let _ = receiver.has_changed();
        assert!(*receiver.borrow_and_update());
    }
}

#[test]
fn file_count_is_bounded_before_any_task_enters_the_registry() {
    let runtime = TaildropRuntime::default();
    let files = (0..=MAX_TAILDROP_FILES_PER_TASK)
        .map(|i| (format!("{i}.bin"), 1))
        .collect();
    assert_eq!(
        runtime
            .start_task("server-a".into(), "peer-a".into(), files)
            .err(),
        Some(TaildropTaskStartError::TooManyFiles)
    );
    assert!(runtime.snapshots(None).is_empty());
}

#[test]
fn transport_readiness_changes_phase_without_inventing_progress_or_receipt() {
    let runtime = TaildropRuntime::default();
    let sink = RecordingSink::default();
    let task = start(&runtime, "same.bin", 65537);
    assert_eq!(task.snapshot.phase, TaildropTaskPhase::Connecting);
    runtime.mark_sending(&task.snapshot.task_id, &sink);
    let ready = runtime.snapshots(None).pop().unwrap();
    assert_eq!(ready.phase, TaildropTaskPhase::Sending);
    assert_eq!(ready.sent_bytes, 0);
    assert_eq!(ready.acknowledged_bytes, 0);
    assert_eq!(ready.files[0].sent_bytes, 0);
    assert!(!ready.files[0].completed);
}

#[test]
fn cancel_at_opening_or_resume_read_keeps_terminal_state_and_next_task_independent() {
    for headers_received in [false, true] {
        let runtime = TaildropRuntime::default();
        let sink = RecordingSink::default();
        let mut old = start(&runtime, "same.bin", 65537);
        if headers_received {
            runtime.mark_sending(&old.snapshot.task_id, &sink);
        }
        runtime.cancel(&old.snapshot.task_id, &sink).unwrap();
        assert!(*old.cancel.borrow_and_update());
        runtime.record_progress(&old.snapshot.task_id, 0, 17, false, &sink);
        runtime.record_acknowledged(&old.snapshot.task_id, 17, &sink);
        let canceling = runtime.snapshots(None).pop().unwrap();
        assert_eq!(canceling.phase, TaildropTaskPhase::Canceling);
        assert_eq!(canceling.sent_bytes, 0);
        assert_eq!(canceling.acknowledged_bytes, 0);
        runtime.canceled(&old.snapshot.task_id, &sink);
        let canceled = runtime.snapshots(None).pop().unwrap();
        runtime.mark_sending(&old.snapshot.task_id, &sink);
        runtime.record_progress(&old.snapshot.task_id, 0, 65537, true, &sink);
        runtime.record_acknowledged(&old.snapshot.task_id, 65537, &sink);
        runtime.complete(&old.snapshot.task_id, &sink);
        runtime.fail(&old.snapshot.task_id, "late-failure", &sink);
        assert_eq!(runtime.snapshots(None).pop().unwrap(), canceled);
        assert_eq!(canceled.phase, TaildropTaskPhase::Canceled);
        assert!(!canceled.files[0].completed);
        let next = start(&runtime, "same.bin", 65537);
        assert_ne!(next.snapshot.task_id, old.snapshot.task_id);
        assert!(!*next.cancel.borrow());
        assert_eq!(next.snapshot.sent_bytes, 0);
        assert_eq!(next.snapshot.acknowledged_bytes, 0);
        assert_eq!(next.snapshot.phase, TaildropTaskPhase::Connecting);
    }
}

#[test]
fn acknowledgements_are_stream_cumulative_across_files_and_ignore_duplicate_or_stale_values() {
    const MIB: u64 = 1024 * 1024;
    let runtime = TaildropRuntime::default();
    let sink = RecordingSink::default();
    let task = runtime
        .start_task(
            "server-a".into(),
            "peer-a".into(),
            vec![
                ("first.bin".into(), MIB + 3),
                ("second.bin".into(), 2 * MIB + 7),
            ],
        )
        .unwrap();
    let id = &task.snapshot.task_id;
    runtime.mark_sending(id, &sink);
    runtime.record_acknowledged(id, MIB as i64, &sink);
    assert_eq!(runtime.snapshots(None)[0].acknowledged_bytes, MIB);
    runtime.record_progress(id, 0, (MIB + 3) as i64, true, &sink);
    runtime.record_acknowledged(id, (2 * MIB) as i64, &sink);
    let second_ack = runtime.snapshots(None).pop().unwrap();
    assert_eq!(second_ack.acknowledged_bytes, 2 * MIB);
    assert!(second_ack.files[0].completed);
    assert!(!second_ack.files[1].completed);
    for stale in [(2 * MIB) as i64, MIB as i64, 0, -1, i64::MIN] {
        runtime.record_acknowledged(id, stale, &sink);
        assert_eq!(
            runtime.snapshots(None)[0],
            second_ack,
            "stale ACK changed snapshot/revision"
        );
    }
    runtime.record_progress(id, 1, 17, false, &sink);
    assert_eq!(runtime.snapshots(None)[0].acknowledged_bytes, 2 * MIB);
    runtime.record_acknowledged(id, i64::MAX, &sink);
    let clamped = runtime.snapshots(None).pop().unwrap();
    assert_eq!(clamped.acknowledged_bytes, 3 * MIB + 10);
    assert_eq!(clamped.phase, TaildropTaskPhase::Sending);
    assert!(
        !clamped.files[1].completed,
        "ACK is not a completion receipt"
    );
    runtime.record_acknowledged(id, (2 * MIB) as i64, &sink);
    assert_eq!(runtime.snapshots(None)[0], clamped);
    runtime.complete(id, &sink);
    let completed = runtime.snapshots(None).pop().unwrap();
    assert_eq!(completed.phase, TaildropTaskPhase::Completed);
    assert_eq!(completed.sent_bytes, completed.total_bytes);
    assert_eq!(completed.acknowledged_bytes, completed.total_bytes);
    assert!(completed.files.iter().all(|file| file.completed));
    runtime.record_acknowledged(id, 0, &sink);
    assert_eq!(runtime.snapshots(None)[0], completed);
}

#[test]
fn cumulative_ack_during_cancel_or_after_terminal_cannot_mutate_revision() {
    let runtime = TaildropRuntime::default();
    let sink = RecordingSink::default();
    let task = start(&runtime, "large.bin", 4 * 1024 * 1024);
    let id = &task.snapshot.task_id;
    runtime.record_acknowledged(id, 1024 * 1024, &sink);
    runtime.cancel(id, &sink).unwrap();
    let canceling = runtime.snapshots(None).pop().unwrap();
    for received in [2 * 1024 * 1024, i64::MAX, -1] {
        runtime.record_acknowledged(id, received, &sink);
        assert_eq!(runtime.snapshots(None)[0], canceling);
    }
    runtime.canceled(id, &sink);
    let canceled = runtime.snapshots(None).pop().unwrap();
    for received in [2 * 1024 * 1024, i64::MAX, -1] {
        runtime.record_acknowledged(id, received, &sink);
        assert_eq!(runtime.snapshots(None)[0], canceled);
    }
    assert_eq!(canceled.acknowledged_bytes, 1024 * 1024);
    assert_eq!(canceled.phase, TaildropTaskPhase::Canceled);
}
