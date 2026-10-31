use super::*;

#[test]
fn exclusive_rename_preserves_an_occupied_destination_and_held_object_identity() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let target = root.path().join("target");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"target").unwrap();
    let held = open_nofollow(&source, false).unwrap();
    let selected = identity(&held).unwrap();
    assert!(rename_exclusive(&source, &target).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&target).unwrap(), b"target");
    fs::remove_file(&target).unwrap();
    rename_exclusive(&source, &target).unwrap();
    assert_eq!(
        identity(&open_nofollow(&target, false).unwrap()).unwrap(),
        selected
    );
    assert_eq!(identity(&held).unwrap(), selected);
}

#[test]
fn native_hardlink_count_and_directory_barrier_type_are_observed() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let alias = root.path().join("alias");
    fs::write(&source, b"keep").unwrap();
    fs::hard_link(&source, &alias).unwrap();
    let held = open_nofollow(&source, false).unwrap();
    assert_eq!(
        identity(&held).unwrap(),
        identity(&open_nofollow(&alias, false).unwrap()).unwrap()
    );
    #[cfg(windows)]
    assert_eq!(link_count(&held).unwrap(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(held.metadata().unwrap().nlink(), 2);
    }
    assert!(sync_directory(&source).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"keep");
}
