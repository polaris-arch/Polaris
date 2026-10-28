use super::*;
#[test]
fn empty_absent_deferred_journal_is_success_but_other_errors_remain() {
    let temp = crate::test_support::TestDir::new("polaris-empty-journal-");
    let config = ConfigManager::new(temp.path().to_path_buf());
    assert!(config
        .save_deferred_deletions_locked(&DeferredDeletionJournal::default())
        .is_ok());
    let path = config.deferred_deletions_path();
    assert!(!path.exists());
    std::fs::create_dir(&path).unwrap();
    assert!(config
        .save_deferred_deletions_locked(&DeferredDeletionJournal::default())
        .is_err());
    assert!(path.is_dir());
}
