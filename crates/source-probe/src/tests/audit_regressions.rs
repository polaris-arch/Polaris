use super::*;

#[test]
fn same_named_root_and_child_have_distinct_src_relative_keys() {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!("polaris-probe-audit-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    std::fs::create_dir_all(root.join("src/foo/tests")).unwrap();
    let _cleanup = Cleanup(root.clone());
    std::fs::write(root.join("src/foo.rs"), "root-marker").unwrap();
    std::fs::write(root.join("src/foo/foo.rs"), "child-marker").unwrap();
    std::fs::write(root.join("src/foo/tests/mod.rs"), "test-marker").unwrap();
    let production = module_files_in(&root, "foo");
    let all = module_files_with_tests_in(&root, "foo");
    assert_eq!(production.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["foo.rs", "foo/foo.rs"]);
    assert_eq!(all.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["foo.rs", "foo/foo.rs", "foo/tests/mod.rs"]);
    for (key, text) in &production {
        assert_eq!(all.iter().find(|(other, _)| other == key).map(|(_, value)| value), Some(text));
    }
    assert!(!module_source_in(&root, "foo").contains("test-marker"));
    assert!(module_source_with_tests_in(&root, "foo").contains("test-marker"));
}
