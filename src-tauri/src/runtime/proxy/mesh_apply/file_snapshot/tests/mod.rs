use super::*;
use crate::test_support::TestDir;

#[test]
fn same_length_replacement_has_different_file_identity() {
    let dir = TestDir::new("polaris-file-identity-");
    let path = dir.path().join("source");
    let replacement = dir.path().join("replacement");
    fs::write(&path, b"first").unwrap();
    fs::write(&replacement, b"other").unwrap();
    let before = FileSnapshot::path(&path).unwrap();
    let opened = FileSnapshot::opened(&File::open(&path).unwrap()).unwrap();
    assert!(before.same_snapshot(&opened));
    fs::remove_file(&path).unwrap();
    fs::rename(&replacement, &path).unwrap();
    let after = FileSnapshot::path(&path).unwrap();
    assert_eq!(before.len(), after.len());
    assert!(!before.same_identity(&after));
    assert!(!before.same_snapshot(&after));
}

#[test]
fn directory_replacement_has_different_file_identity() {
    let dir = TestDir::new("polaris-directory-identity-");
    let path = dir.path().join("source");
    let replacement = dir.path().join("replacement");
    fs::create_dir(&path).unwrap();
    fs::create_dir(&replacement).unwrap();
    let before = FileSnapshot::path(&path).unwrap();
    assert!(before.is_dir());
    fs::remove_dir(&path).unwrap();
    fs::rename(&replacement, &path).unwrap();
    let after = FileSnapshot::path(&path).unwrap();
    assert!(!before.same_identity(&after));
}
