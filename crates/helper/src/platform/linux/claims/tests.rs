use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::Command;

#[test]
fn nonroot_provision_is_refused_without_creating_a_base() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let base = fixture.path().join("claims");
    assert!(provision_at(&base).is_err());
    assert!(!base.exists());
}

#[test]
fn user_owned_parent_is_not_a_trusted_deployment() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let base = fixture.path().join("claims");
    assert!(open_deployment_at(&base).is_err());
    assert!(!base.exists());
}

#[test]
fn path_requires_absolute_normal_components() {
    assert!(open_parent(Path::new("relative/claims")).is_err());
    assert!(open_parent(Path::new("/run/../claims")).is_err());
    assert!(open_parent(Path::new("/")).is_err());
}

struct RootFixture {
    _root: tempfile::TempDir,
    base: std::path::PathBuf,
}

impl RootFixture {
    fn new() -> Self {
        assert!(
            nix::unistd::geteuid().is_root(),
            "root file gate requires root"
        );
        let root = tempfile::Builder::new()
            .prefix("polaris-claims-h-")
            .tempdir_in("/var/tmp")
            .unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let base = root.path().join("claims");
        Self { _root: root, base }
    }

    fn provision(&self) -> ClaimsDeployment {
        provision_at(&self.base).unwrap()
    }
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_provision_publishes_exact_v2_without_rewriting_existing_claims() {
    let fixture = RootFixture::new();
    let deployment = fixture.provision();
    assert_eq!(std::fs::read(fixture.base.join(MARKER)).unwrap(), PROTOCOL);
    assert_eq!(
        std::fs::metadata(fixture.base.join(ALLOCATOR))
            .unwrap()
            .len(),
        0
    );
    let residual = fixture.base.join("existing-birth-permission-fixture");
    std::fs::write(&residual, b"unresolved responsibility").unwrap();
    let old_inode = std::fs::metadata(&residual).unwrap().ino();
    let second = fixture.provision();
    deployment.validate().unwrap();
    second.validate().unwrap();
    assert_eq!(std::fs::metadata(residual).unwrap().ino(), old_inode);
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_old_private_or_partial_layout_is_never_repaired() {
    let fixture = RootFixture::new();
    std::fs::create_dir(&fixture.base).unwrap();
    std::fs::set_permissions(&fixture.base, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(fixture.base.join(MARKER), &PROTOCOL[..10]).unwrap();
    let before = std::fs::metadata(&fixture.base).unwrap();
    assert!(provision_at(&fixture.base).is_err());
    assert_eq!(
        std::fs::metadata(&fixture.base).unwrap().mode(),
        before.mode()
    );
    assert_eq!(
        std::fs::read(fixture.base.join(MARKER)).unwrap(),
        &PROTOCOL[..10]
    );
    assert!(!fixture.base.join(ALLOCATOR).exists());
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_concurrent_initializers_only_accept_the_completed_layout() {
    let fixture = RootFixture::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let barrier = std::sync::Arc::clone(&barrier);
            let path = fixture.base.clone();
            std::thread::spawn(move || {
                barrier.wait();
                provision_at(&path)
            })
        })
        .collect();
    let mut completed = 0;
    for thread in threads {
        if let Ok(deployment) = thread.join().unwrap() {
            deployment.validate().unwrap();
            completed += 1;
        }
    }
    assert!(completed >= 1);
    open_deployment_at(&fixture.base)
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_existing_wrong_base_modes_and_owner_are_refused() {
    let fixture = RootFixture::new();
    let deployment = fixture.provision();
    for mode in [0o700, 0o777, 0o1770, 0o3777, 0o5777] {
        fchmod(&deployment.directory, Mode::from_bits_truncate(mode)).unwrap();
        assert!(deployment.validate().is_err(), "accepted {mode:o}");
        assert!(provision_at(&fixture.base).is_err(), "repaired {mode:o}");
        assert_eq!(
            deployment.directory.metadata().unwrap().mode() & 0o7777,
            mode
        );
    }
    fchmod(&deployment.directory, Mode::from_bits_truncate(0o1777)).unwrap();
    nix::unistd::fchown(
        &deployment.directory,
        Some(nix::unistd::Uid::from_raw(1000)),
        None,
    )
    .unwrap();
    assert!(deployment.validate().is_err());
    assert!(provision_at(&fixture.base).is_err());
    assert_eq!(deployment.directory.metadata().unwrap().uid(), 1000);
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_symlink_and_untrusted_parent_are_refused() {
    let fixture = RootFixture::new();
    let target = fixture._root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    symlink(&target, &fixture.base).unwrap();
    assert!(provision_at(&fixture.base).is_err());
    assert!(!target.join(MARKER).exists());
    std::fs::remove_file(&fixture.base).unwrap();
    let alias = fixture._root.path().join("alias");
    symlink(fixture._root.path(), &alias).unwrap();
    assert!(provision_at(&alias.join("claims")).is_err());
    std::fs::set_permissions(fixture._root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(provision_at(&fixture.base).is_err());
    assert!(!fixture.base.exists());
    std::fs::set_permissions(fixture._root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    nix::unistd::chown(
        fixture._root.path(),
        Some(nix::unistd::Uid::from_raw(1000)),
        None,
    )
    .unwrap();
    assert!(provision_at(&fixture.base).is_err());
    assert!(!fixture.base.exists());
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_marker_bytes_links_mode_owner_and_allocator_size_are_strict() {
    let fixture = RootFixture::new();
    let deployment = fixture.provision();
    let marker = fixture.base.join(MARKER);
    std::fs::write(&marker, &PROTOCOL[..PROTOCOL.len() - 1]).unwrap();
    assert!(deployment.validate().is_err());
    std::fs::write(&marker, PROTOCOL).unwrap();
    let mut incompatible = PROTOCOL.to_vec();
    incompatible[PROTOCOL.iter().position(|byte| *byte == b'2').unwrap()] = b'1';
    std::fs::write(&marker, incompatible).unwrap();
    assert!(deployment.validate().is_err());
    std::fs::write(&marker, PROTOCOL).unwrap();
    fchmod(&deployment.marker, Mode::from_bits_truncate(0o644)).unwrap();
    assert!(deployment.validate().is_err());
    assert!(provision_at(&fixture.base).is_err());
    fchmod(&deployment.marker, Mode::from_bits_truncate(0o444)).unwrap();
    let hardlink = fixture._root.path().join("hardlink");
    std::fs::hard_link(&marker, &hardlink).unwrap();
    assert!(deployment.validate().is_err());
    std::fs::remove_file(hardlink).unwrap();
    nix::unistd::fchown(
        &deployment.marker,
        Some(nix::unistd::Uid::from_raw(1000)),
        None,
    )
    .unwrap();
    assert!(deployment.validate().is_err());
    nix::unistd::fchown(
        &deployment.marker,
        Some(nix::unistd::Uid::from_raw(0)),
        None,
    )
    .unwrap();
    std::fs::write(fixture.base.join(ALLOCATOR), b"x").unwrap();
    assert!(deployment.validate().is_err());
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_marker_and_allocator_replacements_do_not_rebind_old_fds() {
    for name in [MARKER, ALLOCATOR] {
        let fixture = RootFixture::new();
        let deployment = fixture.provision();
        let path = fixture.base.join(name);
        std::fs::rename(&path, fixture._root.path().join("old-file")).unwrap();
        let replacement = create_file(
            &deployment.directory,
            name,
            if name == MARKER { PROTOCOL } else { b"" },
        )
        .unwrap();
        assert!(deployment.validate().is_err());
        assert_ne!(
            replacement.metadata().unwrap().ino(),
            if name == MARKER {
                deployment.marker.metadata().unwrap().ino()
            } else {
                deployment.allocator.metadata().unwrap().ino()
            }
        );
        assert!(open_deployment_at(&fixture.base).is_ok());
    }
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_base_and_parent_replacements_are_detected() {
    let fixture = RootFixture::new();
    let deployment = fixture.provision();
    std::fs::rename(&fixture.base, fixture._root.path().join("old-base")).unwrap();
    fixture.provision();
    assert!(deployment.validate().is_err());
    let parent_held = fixture.provision();
    let parent_path = fixture._root.path();
    let moved = parent_path.with_extension("moved");
    std::fs::rename(parent_path, &moved).unwrap();
    // Move it back immediately after validation so TempDir retains exact custody.
    let result = parent_held.validate();
    std::fs::rename(&moved, parent_path).unwrap();
    assert!(result.is_err());
}

#[test]
#[ignore = "root-only private filesystem gate; no host claims or network effects"]
fn root_symlink_and_fifo_marker_never_block_or_publish() {
    let fixture = RootFixture::new();
    fixture.provision();
    let marker = fixture.base.join(MARKER);
    std::fs::remove_file(&marker).unwrap();
    symlink(fixture.base.join(ALLOCATOR), &marker).unwrap();
    assert!(open_deployment_at(&fixture.base).is_err());
    std::fs::remove_file(&marker).unwrap();
    nix::unistd::mkfifo(&marker, Mode::from_bits_truncate(0o444)).unwrap();
    assert!(open_deployment_at(&fixture.base).is_err());
}

#[test]
#[ignore = "root-only actual publication and constructor handoff failure gate"]
fn root_publication_sync_errors_survive_statically_valid_reopen() {
    for failing_sync in [1, 2] {
        let fixture = RootFixture::new();
        let deployment = fixture.provision();
        fchmod(&deployment.directory, Mode::from_bits_truncate(0o700)).unwrap();
        let calls = std::cell::Cell::new(0);
        let result = publish_private_deployment(deployment, |file| {
            calls.set(calls.get() + 1);
            if calls.get() == failing_sync {
                Err(io::Error::other("injected post-publication sync failure"))
            } else {
                file.sync_all()
            }
        });
        assert!(result.is_err());
        // Metadata/bytes are valid despite this startup's failed durability.
        open_deployment_at(&fixture.base)
            .unwrap()
            .validate()
            .unwrap();
        let server = super::super::server::ConnServer::with_claims_provision(
            &super::super::server::ServerConfig::default(),
            result,
        );
        let error = server.validate_start_environment_for_test().unwrap_err();
        assert!(error
            .to_string()
            .contains("injected post-publication sync failure"));
        assert!(server.validate_start_environment_for_test().is_err());
    }
}

#[test]
#[ignore = "root-only actual successful constructor retains original FD gate"]
fn root_successful_handoff_keeps_the_original_deployment_fds() {
    let fixture = RootFixture::new();
    let result = provision_at(&fixture.base);
    let server = super::super::server::ConnServer::with_claims_provision(
        &super::super::server::ServerConfig::default(),
        result,
    );
    server.validate_start_environment_for_test().unwrap();
    std::fs::rename(&fixture.base, fixture._root.path().join("original-base")).unwrap();
    fixture.provision();
    open_deployment_at(&fixture.base)
        .unwrap()
        .validate()
        .unwrap();
    assert!(server.validate_start_environment_for_test().is_err());
}

#[test]
#[ignore = "root-only fresh original base replacement before publication gate"]
fn root_fresh_held_base_replacement_with_moved_files_is_not_published() {
    let fixture = RootFixture::new();
    let (parents, parent_names, name) = open_parent(&fixture.base).unwrap();
    let parent = parents.last().unwrap();
    mkdirat(parent, name.as_os_str(), Mode::from_bits_truncate(0o700)).unwrap();
    let directory =
        File::from(openat(parent, name.as_os_str(), DIRECTORY_FLAGS, Mode::empty()).unwrap());
    create_file(&directory, MARKER, PROTOCOL).unwrap();
    create_file(&directory, ALLOCATOR, b"").unwrap();
    let original_inode = directory.metadata().unwrap().ino();
    let original_path = fixture._root.path().join("original-base");
    std::fs::rename(&fixture.base, &original_path).unwrap();
    mkdirat(parent, name.as_os_str(), Mode::from_bits_truncate(0o700)).unwrap();
    for file in [MARKER, ALLOCATOR] {
        std::fs::rename(original_path.join(file), fixture.base.join(file)).unwrap();
    }
    let result = from_directory(parents, parent_names, name, directory)
        .and_then(|deployment| publish_private_deployment(deployment, File::sync_all));
    assert!(result.is_err());
    let replacement = std::fs::metadata(&fixture.base).unwrap();
    assert_ne!(replacement.ino(), original_inode);
    assert_eq!(replacement.mode() & 0o7777, 0o700);
}

fn uid_command(base: &Path, uid: u32, operation: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "platform::linux::claims::tests::uid_worker",
        "--ignored",
        "--nocapture",
    ]);
    command.env("POLARIS_CLAIMS_H_FIXTURE", base);
    command.env("POLARIS_CLAIMS_H_UID", uid.to_string());
    command.env("POLARIS_CLAIMS_H_OPERATION", operation);
    super::super::server::attach_privilege_drop(&mut command, uid, uid, Vec::new());
    command
}

#[test]
#[ignore = "root-only peer UID plus unchanged ambient capabilities file gate"]
fn root_peer_uids_with_original_ambient_caps_share_the_sticky_store() {
    let fixture = RootFixture::new();
    fixture.provision();
    let policy = std::fs::read_to_string("/proc/sys/fs/protected_regular").unwrap();
    assert!(
        policy.trim().parse::<u32>().unwrap() > 0,
        "protected_regular gate unsupported on this host"
    );
    for (uid, operation) in [
        (1000, "create"),
        (1001, "conflict"),
        (1000, "release"),
        (1001, "create"),
        (1000, "conflict"),
        (1001, "release"),
    ] {
        let output = uid_command(&fixture.base, uid, operation).output().unwrap();
        assert!(
            output.status.success(),
            "UID {uid} {operation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!fixture.base.join("h-permissions-only").exists());
}

/// Subprocess worker tests filesystem permissions only. The generic probe is
/// deliberately not the L resource-name or journal implementation.
#[test]
#[ignore = "only invoked by the isolated root peer UID gate"]
fn uid_worker() {
    let path = std::env::var_os("POLARIS_CLAIMS_H_FIXTURE").expect("worker fixture");
    let uid = std::env::var("POLARIS_CLAIMS_H_UID")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert_eq!(nix::unistd::geteuid().as_raw(), uid);
    assert_eq!(nix::unistd::getegid().as_raw(), uid);
    assert!(nix::unistd::getgroups().unwrap().is_empty());
    let required = super::super::server::ambient_caps();
    let expected: caps::CapsHashSet = required.into_iter().collect();
    assert_eq!(caps::read(None, caps::CapSet::Effective).unwrap(), expected);
    assert_eq!(caps::read(None, caps::CapSet::Ambient).unwrap(), expected);
    let deployment = open_deployment_at(Path::new(&path)).unwrap();
    assert!(provision_at(Path::new(&path)).is_err());
    let lock = nix::fcntl::Flock::lock(
        deployment.allocator.try_clone().unwrap(),
        nix::fcntl::FlockArg::LockExclusiveNonblock,
    )
    .expect("readonly allocator exclusive flock");
    let name = "h-permissions-only";
    match std::env::var("POLARIS_CLAIMS_H_OPERATION")
        .unwrap()
        .as_str()
    {
        "create" => {
            let mut file = File::from(
                openat(
                    &deployment.directory,
                    name,
                    OFlag::O_RDWR
                        | OFlag::O_CREAT
                        | OFlag::O_EXCL
                        | OFlag::O_NOFOLLOW
                        | OFlag::O_CLOEXEC,
                    Mode::from_bits_truncate(0o600),
                )
                .unwrap(),
            );
            file.write_all(b"permission probe residual").unwrap();
            file.sync_all().unwrap();
            assert_eq!(file.metadata().unwrap().uid(), uid);
            assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
            assert_eq!(file.metadata().unwrap().nlink(), 1);
            validate_link(&deployment.directory, std::ffi::OsStr::new(name), &file).unwrap();
        }
        "conflict" => {
            assert_eq!(
                openat(
                    &deployment.directory,
                    name,
                    OFlag::O_RDWR
                        | OFlag::O_CREAT
                        | OFlag::O_EXCL
                        | OFlag::O_NOFOLLOW
                        | OFlag::O_CLOEXEC,
                    Mode::from_bits_truncate(0o600),
                )
                .unwrap_err(),
                Errno::EEXIST
            );
            assert_eq!(
                nix::unistd::unlinkat(
                    &deployment.directory,
                    name,
                    nix::unistd::UnlinkatFlags::NoRemoveDir
                )
                .unwrap_err(),
                Errno::EPERM
            );
        }
        "release" => {
            let file = File::from(
                openat(
                    &deployment.directory,
                    name,
                    OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                    Mode::empty(),
                )
                .unwrap(),
            );
            assert_eq!(file.metadata().unwrap().uid(), uid);
            validate_link(&deployment.directory, std::ffi::OsStr::new(name), &file).unwrap();
            nix::unistd::unlinkat(
                &deployment.directory,
                name,
                nix::unistd::UnlinkatFlags::NoRemoveDir,
            )
            .unwrap();
        }
        _ => panic!("unknown fixture operation"),
    }
    deployment.validate().unwrap();
    drop(lock);
}
