use super::*;
use std::os::unix::fs::{symlink, MetadataExt};

#[test]
fn ancestor_swap_after_open_cannot_redirect_descendants_or_chown() {
    let temp = tempfile::tempdir().unwrap();
    // macOS TMPDIR may have /var -> /private/var ancestors. Resolve
    // the fixture base before testing the deliberate no-follow policy.
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("root");
    let external = base.join("external");
    std::fs::create_dir_all(root.join("logs")).unwrap();
    std::fs::create_dir(&external).unwrap();
    std::fs::write(root.join("logs/owned"), "owned").unwrap();
    std::fs::write(external.join("foreign"), "foreign").unwrap();
    let owned = std::fs::metadata(root.join("logs/owned")).unwrap().ino();
    let foreign = std::fs::metadata(external.join("foreign")).unwrap().ino();
    let directory = open_directory(&root).unwrap();
    let mut visited = Vec::new();
    let mut swapped = false;
    let mut remaining = MAX_ENTRIES;
    walk_child(
        &directory,
        "logs".as_ref(),
        0,
        &mut remaining,
        &mut |file, metadata| {
            assert_eq!(fstat(file)?.st_ino, metadata.st_ino);
            visited.push(metadata.st_ino);
            if !swapped {
                swapped = true;
                std::fs::rename(root.join("logs"), root.join("captured"))?;
                symlink(&external, root.join("logs"))?;
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(visited.contains(&owned));
    assert!(!visited.contains(&foreign));
    assert_eq!(
        std::fs::read_to_string(external.join("foreign")).unwrap(),
        "foreign"
    );
}

#[test]
fn static_links_hardlinks_and_ancestor_links_are_rejected_before_visiting() {
    let temp = tempfile::tempdir().unwrap();
    // macOS TMPDIR may have /var -> /private/var ancestors. Resolve
    // the fixture base before testing the deliberate no-follow policy.
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("original"), "original").unwrap();
    std::fs::hard_link(root.join("original"), root.join("hard")).unwrap();
    symlink(root.join("original"), root.join("sym")).unwrap();
    let directory = open_directory(&root).unwrap();
    for name in ["hard", "sym"] {
        let mut remaining = MAX_ENTRIES;
        assert!(
            walk_child(&directory, name.as_ref(), 0, &mut remaining, &mut |_, _| {
                panic!("unsafe object must not reach the ownership action");
            })
            .is_err()
        );
    }
    symlink(&root, base.join("alias")).unwrap();
    assert!(open_directory(&base.join("alias")).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn nested_config_uses_registered_root_for_cache_preparation_and_ownership_tail() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("profiles")).unwrap();
    std::fs::write(root.join("profiles/current.json"), b"{}").unwrap();
    prepare_cache(&root).unwrap();
    assert!(root.join("cache.db").is_file());
    assert!(!root.join("profiles/cache.db").exists());
    let cache_inode = std::fs::metadata(root.join("cache.db")).unwrap().ino();
    return_runtime_ownership(&root).unwrap();
    let cache = std::fs::metadata(root.join("cache.db")).unwrap();
    assert_eq!(cache.ino(), cache_inode);
    assert_eq!(cache.uid(), nix::unistd::Uid::effective().as_raw());
    assert!(!root.join("profiles/cache.db").exists());
}
