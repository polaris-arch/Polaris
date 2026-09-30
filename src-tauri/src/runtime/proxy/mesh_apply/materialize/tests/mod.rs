use super::*;
use crate::test_support::TestDir;

#[test]
fn same_length_source_replaced_between_lstat_and_open_is_rejected() {
    let dir = TestDir::new("polaris-rule-source-race-");
    let source = dir.path().join("source.json");
    let replacement = dir.path().join("replacement.json");
    fs::write(&source, b"first").unwrap();
    fs::write(&replacement, b"other").unwrap();
    assert_eq!(
        read_trusted_source_after_lstat(&source, &[dir.path().to_path_buf()], || {
            fs::remove_file(&source).unwrap();
            fs::rename(&replacement, &source).unwrap();
        })
        .unwrap_err(),
        MaterializeError::SourceChanged
    );
}
