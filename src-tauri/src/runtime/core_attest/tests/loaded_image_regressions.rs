use super::super::*;
#[test]
fn atomically_replaced_path_is_not_the_open_object() {
    let dir = std::env::temp_dir().join(format!("polaris-loaded-image-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let expected = dir.join("core");
    std::fs::write(&expected, b"old").unwrap();
    let loaded = std::fs::File::open(&expected).unwrap();
    use std::os::fd::AsRawFd;
    let observed = PathBuf::from(format!("/proc/self/fd/{}", loaded.as_raw_fd()));
    assert!(same_file_path(&expected, &observed));
    let replacement = dir.join("replacement");
    std::fs::write(&replacement, b"new").unwrap();
    std::fs::rename(&replacement, &expected).unwrap();
    assert!(!matches!(
        attest_core_binary(&expected, Some(&observed), "", ""),
        CoreBinaryAttestation::SamePath
    ));
    std::fs::remove_file(&expected).unwrap();
    assert!(!same_file_path(&expected, &observed));
    drop(loaded);
    std::fs::remove_dir_all(dir).unwrap();
}
