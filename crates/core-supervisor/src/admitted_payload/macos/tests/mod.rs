use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "polaris-private-copy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn expected(bytes: &[u8]) -> polaris_helper_proto::core_manifest::CoreFile {
    polaris_helper_proto::core_manifest::CoreFile {
        name: "sing-box".into(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    }
}
#[test]
fn root_scope_cannot_be_selected_by_an_ordinary_input_or_purpose() {
    for purpose in [
        ExecPurpose::CheckConfig,
        ExecPurpose::TlsInventory,
        ExecPurpose::RunMain,
        ExecPurpose::RunLogin,
        ExecPurpose::RunSpeedtest,
    ] {
        let error =
            admit_selected(UntrustedPayload::bundled("/missing/sing-box"), purpose).unwrap_err();
        assert_eq!(error.kind, AdmissionErrorKind::UnsupportedExactObject);
        assert_eq!(error.purpose, Some(purpose));
    }
    if require_root().is_err() {
        assert_eq!(
            admit_macos_root_helper_main(UntrustedPayload::bundled("/missing/sing-box"))
                .unwrap_err()
                .kind,
            AdmissionErrorKind::UnsupportedExactObject
        );
    }
}
#[test]
fn root_policy_is_exactly_main_and_five_fields() {
    let source = "a".repeat(40);
    let tree = "b".repeat(40);
    let valid = serde_json::json!({"schema":"polaris-trusted-execution-v1",
        "sourceCommit":source,"sourceTree":tree,"loader":"macos-root-private-v1",
        "purposes":["run-main-v1"]});
    assert!(SelectedPolicy::declaration(&valid, &source, &tree, "mac-arm64").is_ok());
    for platform in ["mac-arm64", "mac-x64"] {
        for loader in ["linux-sealed-fd-v1", "macos-path-v1", ""] {
            let mut bad = valid.clone();
            bad["loader"] = serde_json::json!(loader);
            assert!(SelectedPolicy::declaration(&bad, &source, &tree, platform).is_err());
        }
        for purposes in [
            serde_json::json!([]),
            serde_json::json!(["run-main-v1", "run-login-v1"]),
            serde_json::json!(["check-config-v1"]),
            serde_json::json!(["run-main-v1", "run-main-v1"]),
        ] {
            let mut bad = valid.clone();
            bad["purposes"] = purposes;
            assert!(SelectedPolicy::declaration(&bad, &source, &tree, platform).is_err());
        }
    }
    let mut extra = valid.clone();
    extra["root"] = serde_json::json!(true);
    assert!(SelectedPolicy::declaration(&extra, &source, &tree, "mac-arm64").is_err());
    let mut other = valid.clone();
    other["sourceTree"] = serde_json::json!("c".repeat(40));
    assert!(SelectedPolicy::declaration(&other, &source, &tree, "mac-arm64").is_err());
}
#[test]
fn private_copy_is_a_new_inode_and_old_writer_cannot_change_its_bytes() {
    use std::os::unix::fs::OpenOptionsExt;
    let temp = Temp::new();
    let source = temp.0.join("source");
    let output = temp.0.join("copy");
    let bytes = vec![b'a'; 128 * 1024];
    std::fs::write(&source, &bytes).unwrap();
    let mut old = std::fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .unwrap();
    let mut input = File::open(&source).unwrap();
    let mut copy = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&output)
        .unwrap();
    assert_ne!(
        input.metadata().unwrap().ino(),
        copy.metadata().unwrap().ino()
    );
    copy_checked(&mut input, &mut copy, &expected(&bytes)).unwrap();
    old.write_all(&vec![b'z'; bytes.len()]).unwrap();
    old.sync_all().unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    assert_ne!(std::fs::read(&source).unwrap(), bytes);
    // This tests the production copy primitive, not root/private admission.
}
#[test]
fn private_copy_checks_actual_output_and_bounded_length() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    let output = temp.0.join("copy");
    std::fs::write(&source, b"different").unwrap();
    let mut input = File::open(&source).unwrap();
    let mut copy = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&output)
        .unwrap();
    assert_eq!(
        copy_checked(&mut input, &mut copy, &expected(b"unchanged"))
            .unwrap_err()
            .kind,
        AdmissionErrorKind::Mismatch
    );
    let mut oversized = expected(b"different");
    oversized.bytes = MAX_MEMBER + 1;
    assert_eq!(
        copy_checked(&mut input, &mut copy, &oversized)
            .unwrap_err()
            .kind,
        AdmissionErrorKind::Mismatch
    );
}
#[test]
fn warm_member_observation_rejects_replacement_and_mutation() {
    let temp = Temp::new();
    let path = temp.0.join("member");
    std::fs::write(&path, b"selected").unwrap();
    let file = File::open(&path).unwrap();
    let mut member = Member {
        stamp: stamp(&file.metadata().unwrap()),
        file,
        path: path.clone(),
    };
    assert!(member.current());
    // Warm observation uses only metadata/ACL/namespace, never a content read.
    let position = member.file.stream_position().unwrap();
    assert!(member.current());
    assert_eq!(member.file.stream_position().unwrap(), position);
    let other = temp.0.join("other");
    std::fs::write(&other, b"selected").unwrap();
    std::fs::rename(other, &path).unwrap();
    assert!(!member.current());
}
fn macho(commands: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![0u8; 32];
    header[0..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
    let cpu: u32 = if cfg!(target_arch = "aarch64") {
        0x0100000c
    } else {
        0x01000007
    };
    header[4..8].copy_from_slice(&cpu.to_le_bytes());
    header[12..16].copy_from_slice(&2u32.to_le_bytes());
    header[16..20].copy_from_slice(&(commands.len() as u32).to_le_bytes());
    let size: usize = commands.iter().map(Vec::len).sum();
    header[20..24].copy_from_slice(&(size as u32).to_le_bytes());
    for command in commands {
        header.extend(command);
    }
    header.extend([0u8; 16]);
    header
}
fn name_command(id: u32, name: &str) -> Vec<u8> {
    let base = if id == 0xc { 24 } else { 12 };
    let len = (base + name.len() + 1 + 7) & !7;
    let mut c = vec![0u8; len];
    c[0..4].copy_from_slice(&id.to_le_bytes());
    c[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    c[8..12].copy_from_slice(&(base as u32).to_le_bytes());
    c[base..base + name.len()].copy_from_slice(name.as_bytes());
    c
}
#[test]
fn loader_graph_rejects_non_system_search_and_unsigned_members() {
    let temp = Temp::new();
    let path = temp.0.join("metadata");
    let linker = name_command(0xe, "/usr/lib/dyld");
    let dependency = name_command(0xc, "/usr/lib/libSystem.B.dylib");
    let mut sig = vec![0u8; 16];
    sig[0..4].copy_from_slice(&0x1du32.to_le_bytes());
    sig[4..8].copy_from_slice(&16u32.to_le_bytes());
    let offset = (32 + linker.len() + dependency.len() + sig.len()) as u32;
    sig[8..12].copy_from_slice(&offset.to_le_bytes());
    sig[12..16].copy_from_slice(&16u32.to_le_bytes());
    std::fs::write(
        &path,
        macho(&[linker.clone(), dependency.clone(), sig.clone()]),
    )
    .unwrap();
    assert!(validate_macho(&mut File::open(&path).unwrap()).is_ok());
    for name in [
        "@rpath/libcustom.dylib",
        "/Users/a/custom.dylib",
        "/usr/lib/../../Users/a/custom.dylib",
    ] {
        std::fs::write(
            &path,
            macho(&[linker.clone(), name_command(0xc, name), sig.clone()]),
        )
        .unwrap();
        assert!(validate_macho(&mut File::open(&path).unwrap()).is_err());
    }
    std::fs::write(&path, macho(&[linker, dependency])).unwrap();
    assert!(validate_macho(&mut File::open(&path).unwrap()).is_err());
    // Metadata only: this is not an executable, signature or root identity proof.
}

#[test]
#[allow(
    unsafe_code,
    reason = "map only an owned temporary source, and unmap it with an RAII guard"
)]
fn old_shared_mapping_only_changes_source_after_private_copy() {
    struct Mapping {
        address: *mut nix::libc::c_void,
        length: usize,
    }
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: this guard owns the sole successful mmap result below.
            assert_eq!(unsafe { nix::libc::munmap(self.address, self.length) }, 0);
        }
    }
    let temp = Temp::new();
    let source = temp.0.join("mapped-source");
    let target = temp.0.join("private-copy");
    let bytes = vec![b'a'; 128 * 1024];
    std::fs::write(&source, &bytes).unwrap();
    let mut old = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&source)
        .unwrap();
    // SAFETY: owned writable file has exactly bytes.len() bytes, zero offset.
    let address = unsafe {
        nix::libc::mmap(
            std::ptr::null_mut(),
            bytes.len(),
            nix::libc::PROT_READ | nix::libc::PROT_WRITE,
            nix::libc::MAP_SHARED,
            old.as_raw_fd(),
            0,
        )
    };
    assert_ne!(address, nix::libc::MAP_FAILED);
    let mapping = Mapping {
        address,
        length: bytes.len(),
    };
    let mut output = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&target)
        .unwrap();
    copy_checked(&mut old, &mut output, &expected(&bytes)).unwrap();
    // SAFETY: the mapping guard retains this allocated writable range.
    unsafe { std::ptr::write_bytes(mapping.address.cast::<u8>(), b'z', mapping.length) };
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    assert_ne!(std::fs::read(&source).unwrap(), bytes);
}

#[test]
fn independent_occupancy_refuses_exclusive_and_last_owner_retains_shared() {
    let temp = Temp::new();
    let path = temp.0.join("lease");
    let writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    writer.try_lock().unwrap();
    let denied = shared_occupancy(File::open(&path).unwrap()).unwrap_err();
    assert_eq!(denied.kind, AdmissionErrorKind::Busy);
    // A failed fresh-OFD attempt did not downgrade the original exclusive lock.
    assert!(matches!(
        File::open(&path).unwrap().try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    writer.unlock().unwrap();
    let owner = shared_occupancy(File::open(&path).unwrap()).unwrap();
    let retained = owner.clone();
    drop(owner);
    assert!(matches!(
        writer.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(retained);
    writer.try_lock().unwrap();
}

#[test]
fn nested_config_keeps_registered_runtime_directory_for_relative_resources() {
    let temp = Temp::new();
    let root = temp.0.canonicalize().unwrap();
    let profile = root.join("profiles/current.json");
    std::fs::create_dir(root.join("profiles")).unwrap();
    std::fs::create_dir(root.join("rules")).unwrap();
    std::fs::write(&profile, b"{}").unwrap();
    std::fs::write(root.join("rules/local.srs"), b"inert rule").unwrap();
    for pipe in [false, true] {
        // Command inspection only: no PreparedExecution, policy or child is made.
        let mut command = std::process::Command::new(root.join("not-an-executable"));
        configure_main_command(&mut command, &profile, &root, pipe);
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
        assert_ne!(command.get_current_dir(), profile.parent());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                std::ffi::OsStr::new("run"),
                std::ffi::OsStr::new("-c"),
                profile.as_os_str()
            ]
        );
        assert_eq!(
            command.get_current_dir().unwrap().join("cache.db"),
            root.join("cache.db")
        );
        assert_eq!(
            std::fs::read(command.get_current_dir().unwrap().join("rules/local.srs")).unwrap(),
            b"inert rule"
        );
    }
}
