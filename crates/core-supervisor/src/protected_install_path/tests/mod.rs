use super::*;
#[test]
fn acl_mutation_grants_are_never_inferred_away() {
    for bit in [2, 4, 5, 6, 8, 10, 12, 13] {
        assert!(grants_mutation(1, 1 << bit));
        assert!(!grants_mutation(2, 1 << bit));
    }
    for bit in [1, 3, 7, 9, 11, 20] {
        assert!(!grants_mutation(1, 1 << bit));
    }
}
#[test]
fn relative_and_parent_paths_are_rejected() {
    assert!(ProtectedPath::open(Path::new("relative"), false).is_err());
    assert!(ProtectedPath::open(Path::new("/Library/../etc/passwd"), false).is_err());
}

#[cfg(target_os = "macos")]
mod native_acl;
