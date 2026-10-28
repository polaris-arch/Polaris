use super::super::*;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
#[test]
fn private_publication_replaces_links_without_touching_their_targets() {
    let dir =
        std::env::temp_dir().join(format!("polaris-private-speedtest-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join(TEMP_CORE_CONFIG_NAME);
    let target = dir.join("outside");
    std::fs::write(&target, "untouched").unwrap();
    symlink(&target, &path).unwrap();
    write_private_temp_config(&path, b"synthetic credential").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&target, &path).unwrap();
    write_private_temp_config(&path, b"second synthetic credential").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    std::fs::remove_dir_all(&dir).unwrap();
}
#[test]
fn pre_spawn_history_is_pruned_but_unknown_factory_is_retained() {
    let custody = PcTempCoreCustody::default();
    for _ in 0..1000 {
        let producer = Arc::new(PcTempBatchProducer::default());
        producer.finished.store(true, Ordering::SeqCst);
        custody.producers.lock().unwrap().push(producer);
        custody.prune_terminal().unwrap();
    }
    assert!(custody.producers.lock().unwrap().is_empty());
    let unknown = Arc::new(PcTempBatchProducer::default());
    unknown.cancelled.store(true, Ordering::SeqCst);
    unknown.factory_started.store(true, Ordering::SeqCst);
    custody.producers.lock().unwrap().push(unknown);
    custody.prune_terminal().unwrap();
    assert_eq!(custody.producers.lock().unwrap().len(), 1);
}
