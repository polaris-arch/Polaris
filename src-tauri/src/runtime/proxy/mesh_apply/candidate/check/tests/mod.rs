#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_tests {
    use super::super::{linux, selected_core_for_test, CandidateCheckError, LinuxCheckProfile};
    use crate::runtime::proxy::mesh_apply::file_snapshot::FileSnapshot;
    use crate::test_support::TestDir;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn check_profiles_bind_separate_historical_and_native_dns_identities() {
        use polaris_core_supervisor::exact_spawn::LinuxInputProfile;
        let historical = LinuxCheckProfile::B609;
        let candidate = LinuxCheckProfile::FE182;
        assert_eq!(
            historical.binary_sha256(),
            "64f6d8613f9c7d42ef9a8e90dd9fca7290f176c5b482714915c04353911559c0"
        );
        assert_eq!(
            candidate.binary_sha256(),
            "7420d9777154754b9182a4269d12348544d925269b700a4f34a29647a4155399"
        );
        assert_eq!(historical.input_profile(), LinuxInputProfile::B609);
        assert_eq!(candidate.input_profile(), LinuxInputProfile::FE182);
        assert_eq!(
            historical.execution_profile(),
            "linux-x86_64-b609:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0"
        );
        assert_eq!(
            candidate.execution_profile(),
            "linux-x86_64-fe182:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0"
        );
        assert!(matches!(
            historical.input_profile(),
            LinuxInputProfile::PlainTcpCheckV1(_)
        ));
        assert!(matches!(
            candidate.input_profile(),
            LinuxInputProfile::PlainTcpCheckV1(_)
        ));
        assert_ne!(historical.binary_sha256(), candidate.binary_sha256());
        assert_ne!(
            historical.execution_profile(),
            candidate.execution_profile()
        );
        let binding: serde_json::Value = serde_json::from_str(&crate::test_support::repo_file(
            "scripts/desktop-core/native-dns-candidate.json",
        ))
        .unwrap();
        assert_eq!(
            binding["sourceCommit"],
            "fe182431495711c09bdb3b074ed6aa6309c00cf8"
        );
        assert_eq!(
            binding["sourceTree"],
            "9b2fe41af73c5768dccff079ded1e4a489cfa9e1"
        );
        assert_eq!(
            binding["platforms"]["linux"]["core"]["sha256"],
            candidate.binary_sha256()
        );
    }

    #[test]
    fn compiled_check_identity_requires_exact_reviewed_source_and_contract() {
        use super::super::{reviewed_compiled_profile, REVIEWED_CHECK_IDENTITIES};
        let historical = &REVIEWED_CHECK_IDENTITIES[0];
        let candidate = &REVIEWED_CHECK_IDENTITIES[1];
        let contract = candidate.contract_sha256.unwrap();
        assert_eq!(historical.contract_sha256, None);
        assert_eq!(
            reviewed_compiled_profile(candidate.source_commit, contract),
            Ok(LinuxCheckProfile::FE182)
        );
        for (source, digest) in [
            (historical.source_commit, contract),
            (candidate.source_commit, historical.binary_sha256),
            (candidate.source_commit, candidate.binary_sha256),
            (candidate.source_commit, ""),
            (candidate.source_commit, "unknown"),
            ("", contract),
            ("unknown", contract),
            // c002 has a separately pinned ELF and compiled contract. It must
            // not inherit fe182 evidence under that old contract digest.
            ("c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e", contract),
        ] {
            assert_eq!(
                reviewed_compiled_profile(source, digest),
                Err(CandidateCheckError::Unsupported)
            );
        }
        let binding =
            crate::test_support::repo_file("scripts/desktop-core/native-dns-candidate.json");
        assert_eq!(
            polaris_updater::verify::sha256_hex(binding.as_bytes()),
            contract
        );
        assert_eq!(
            reviewed_compiled_profile(&candidate.source_commit.to_ascii_uppercase(), contract),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            reviewed_compiled_profile(candidate.source_commit, &contract.to_ascii_uppercase()),
            Err(CandidateCheckError::Unsupported)
        );
    }

    #[test]
    fn historical_c002_retains_its_exact_compiled_source_and_contract() {
        use super::super::{reviewed_compiled_profile, REVIEWED_CHECK_IDENTITIES};
        use polaris_core_supervisor::exact_spawn::LinuxInputProfile;
        let final_release = &REVIEWED_CHECK_IDENTITIES[2];
        let contract = final_release.contract_sha256.unwrap();
        assert_eq!(
            reviewed_compiled_profile(final_release.source_commit, contract),
            Ok(LinuxCheckProfile::C002)
        );
        assert_eq!(
            LinuxCheckProfile::C002.input_profile(),
            LinuxInputProfile::C002
        );
        assert_eq!(
            LinuxCheckProfile::C002.binary_sha256(),
            "b754ac8223b27f4c20b40ced82d9ad5d58fee8fb4c5bfa48c38566e805c570ae"
        );
        assert_eq!(
            LinuxCheckProfile::C002.execution_profile(),
            "linux-x86_64-c002:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0"
        );
        assert!(matches!(
            LinuxCheckProfile::C002.input_profile(),
            LinuxInputProfile::PlainTcpCheckV1(_)
        ));
        assert_eq!(
            contract,
            "c8230c97b5e2c8ccdcdebc29cf08dd2ade302e48b5ec6d77a3d726a452edcec2"
        );
        assert_eq!(
            final_release.source_commit,
            "c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e"
        );
        for (source, digest) in [
            (final_release.source_commit, ""),
            (final_release.source_commit, "unknown"),
            (final_release.source_commit, final_release.binary_sha256),
            (
                final_release.source_commit,
                REVIEWED_CHECK_IDENTITIES[1].contract_sha256.unwrap(),
            ),
            ("", contract),
            ("unknown", contract),
            (REVIEWED_CHECK_IDENTITIES[0].source_commit, contract),
            (REVIEWED_CHECK_IDENTITIES[1].source_commit, contract),
        ] {
            assert_eq!(
                reviewed_compiled_profile(source, digest),
                Err(CandidateCheckError::Unsupported)
            );
        }
        assert_eq!(
            reviewed_compiled_profile(&final_release.source_commit.to_ascii_uppercase(), contract),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            reviewed_compiled_profile(final_release.source_commit, &contract.to_ascii_uppercase()),
            Err(CandidateCheckError::Unsupported)
        );
    }

    #[test]
    fn published_repack_requires_its_exact_source_contract_and_actual_elf() {
        use super::super::{reviewed_compiled_profile, REVIEWED_CHECK_IDENTITIES};
        use polaris_core_supervisor::exact_spawn::LinuxInputProfile;
        let replacement = &REVIEWED_CHECK_IDENTITIES[3];
        assert_eq!(
            replacement.source_commit,
            "a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae"
        );
        let contract = replacement.contract_sha256.unwrap();
        assert_eq!(
            contract,
            "f99daa91345df468b18e9c61451cc6465ffdce137fb1adc33829053ab2b194f3"
        );
        assert_eq!(
            reviewed_compiled_profile(replacement.source_commit, contract),
            Ok(LinuxCheckProfile::A4D085)
        );
        let binding = crate::test_support::repo_file("scripts/desktop-core/release-contract.json");
        let current_digest = polaris_updater::verify::sha256_hex(binding.as_bytes());
        assert_ne!(current_digest, contract, "historical contract is not current");
        let binding: serde_json::Value = serde_json::from_str(&binding).unwrap();
        assert_ne!(binding["sourceCommit"], replacement.source_commit);
        assert_eq!(
            reviewed_compiled_profile(replacement.source_commit, &current_digest),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            LinuxCheckProfile::A4D085.binary_sha256(),
            "2a7495faeb18d2666dfcc77f678057c35cc7a7bb36b03a8a16ff9ac6249b4db8"
        );
        assert_eq!(
            LinuxCheckProfile::A4D085.input_profile(),
            LinuxInputProfile::A4D085
        );
        assert!(matches!(
            LinuxCheckProfile::A4D085.input_profile(),
            LinuxInputProfile::PlainTcpCheckV1(_)
        ));
        assert!(LinuxCheckProfile::A4D085
            .execution_profile()
            .contains("a4d085:runtime-evidence-pending"));
        for digest in [
            "",
            "unknown",
            replacement.binary_sha256,
            REVIEWED_CHECK_IDENTITIES[1].contract_sha256.unwrap(),
            REVIEWED_CHECK_IDENTITIES[2].contract_sha256.unwrap(),
        ] {
            assert_eq!(
                reviewed_compiled_profile(replacement.source_commit, digest),
                Err(CandidateCheckError::Unsupported)
            );
        }
        for source in [
            "",
            "unknown",
            REVIEWED_CHECK_IDENTITIES[0].source_commit,
            REVIEWED_CHECK_IDENTITIES[1].source_commit,
            REVIEWED_CHECK_IDENTITIES[2].source_commit,
        ] {
            assert_eq!(
                reviewed_compiled_profile(source, contract),
                Err(CandidateCheckError::Unsupported)
            );
        }
        assert_eq!(
            reviewed_compiled_profile(&replacement.source_commit.to_ascii_uppercase(), contract),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            reviewed_compiled_profile(replacement.source_commit, &contract.to_ascii_uppercase()),
            Err(CandidateCheckError::Unsupported)
        );
    }

    #[test]
    fn published_patch_requires_its_exact_source_contract_and_actual_elf() {
        use super::super::{reviewed_compiled_profile, REVIEWED_CHECK_IDENTITIES};
        use polaris_core_supervisor::exact_spawn::LinuxInputProfile;
        let replacement = &REVIEWED_CHECK_IDENTITIES[4];
        assert_eq!(
            replacement.source_commit,
            "814da362838e96e54f904836a92c2b0d8c3ae29b"
        );
        let contract = replacement.contract_sha256.unwrap();
        assert_eq!(
            contract,
            "c6c1a11c1f9c7eebe8e424dbcb6e9737dfa9828e3613012622a5e0069a9ee681"
        );
        assert_eq!(
            reviewed_compiled_profile(replacement.source_commit, contract),
            Ok(LinuxCheckProfile::E814DA)
        );
        let binding = crate::test_support::repo_file("scripts/desktop-core/release-contract.json");
        assert_eq!(
            polaris_updater::verify::sha256_hex(binding.as_bytes()),
            contract
        );
        let binding: serde_json::Value = serde_json::from_str(&binding).unwrap();
        assert_eq!(binding["sourceCommit"], replacement.source_commit);
        assert_eq!(
            binding["platforms"]["linux"]["core"]["sha256"],
            replacement.binary_sha256
        );
        assert_eq!(
            LinuxCheckProfile::E814DA.binary_sha256(),
            "363261029cea32333adcdd67288b23e4ae4084efa96b7f2b157af6e9d39946ee"
        );
        assert_eq!(
            LinuxCheckProfile::E814DA.input_profile(),
            LinuxInputProfile::E814DA
        );
        assert!(matches!(
            LinuxCheckProfile::E814DA.input_profile(),
            LinuxInputProfile::PlainTcpCheckV1(_)
        ));
        assert!(LinuxCheckProfile::E814DA
            .execution_profile()
            .contains("814da:runtime-evidence-pending"));
        for digest in [
            "",
            "unknown",
            replacement.binary_sha256,
            REVIEWED_CHECK_IDENTITIES[1].contract_sha256.unwrap(),
            REVIEWED_CHECK_IDENTITIES[2].contract_sha256.unwrap(),
            REVIEWED_CHECK_IDENTITIES[3].contract_sha256.unwrap(),
        ] {
            assert_eq!(
                reviewed_compiled_profile(replacement.source_commit, digest),
                Err(CandidateCheckError::Unsupported)
            );
        }
        for source in [
            "",
            "unknown",
            REVIEWED_CHECK_IDENTITIES[0].source_commit,
            REVIEWED_CHECK_IDENTITIES[1].source_commit,
            REVIEWED_CHECK_IDENTITIES[2].source_commit,
            REVIEWED_CHECK_IDENTITIES[3].source_commit,
        ] {
            assert_eq!(
                reviewed_compiled_profile(source, contract),
                Err(CandidateCheckError::Unsupported)
            );
        }
        assert_eq!(
            reviewed_compiled_profile(&replacement.source_commit.to_ascii_uppercase(), contract),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            reviewed_compiled_profile(replacement.source_commit, &contract.to_ascii_uppercase()),
            Err(CandidateCheckError::Unsupported)
        );
    }

    #[test]
    fn candidate_test_selection_requires_exact_opt_in_or_reviewed_compiled_identity() {
        use super::super::select_test_core;
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let core = OsStr::new("/explicit/selected/core");
        let compiled = Ok(LinuxCheckProfile::A4D085);
        let unreviewed = Err(CandidateCheckError::Unsupported);
        for selection in [compiled, unreviewed] {
            assert_eq!(select_test_core(None, None, false, selection), Ok(None));
        }
        for profile in [
            LinuxCheckProfile::FE182,
            LinuxCheckProfile::C002,
            LinuxCheckProfile::A4D085,
        ] {
            for required in [false, true] {
                assert_eq!(
                    select_test_core(None, Some(core), required, Ok(profile)),
                    Ok(Some((PathBuf::from(core), profile)))
                );
            }
        }
        for selection in [compiled, unreviewed] {
            assert_eq!(
                select_test_core(Some(OsStr::new("1")), Some(core), false, selection),
                Ok(Some((PathBuf::from(core), LinuxCheckProfile::FE182)))
            );
            for mode in [
                "",
                "0",
                "true",
                "fe182",
                "f99daa91345df468b18e9c61451cc6465ffdce137fb1adc33829053ab2b194f3",
                "c002",
                "a4d085",
                "a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae",
                "2a7495faeb18d2666dfcc77f678057c35cc7a7bb36b03a8a16ff9ac6249b4db8",
                " 1",
                "1 ",
                "c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e",
                "c8230c97b5e2c8ccdcdebc29cf08dd2ade302e48b5ec6d77a3d726a452edcec2",
                "b754ac8223b27f4c20b40ced82d9ad5d58fee8fb4c5bfa48c38566e805c570ae",
            ] {
                assert_eq!(
                    select_test_core(Some(OsStr::new(mode)), Some(core), false, selection),
                    Err(CandidateCheckError::Unsupported),
                    "{mode:?}"
                );
            }
            assert_eq!(
                select_test_core(
                    Some(OsStr::from_bytes(b"\xff")),
                    Some(core),
                    false,
                    selection
                ),
                Err(CandidateCheckError::Unsupported)
            );
            for mode in [None, Some(OsStr::new("1"))] {
                for bad_path in [OsStr::new(""), OsStr::new("relative/core")] {
                    assert_eq!(
                        select_test_core(mode, Some(bad_path), false, selection),
                        Err(CandidateCheckError::Unsupported)
                    );
                }
                assert_eq!(
                    select_test_core(mode, None, true, selection),
                    Err(CandidateCheckError::Unsupported)
                );
            }
        }
        assert_eq!(
            select_test_core(None, Some(core), false, unreviewed),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            select_test_core(None, Some(core), true, unreviewed),
            Err(CandidateCheckError::Unsupported)
        );
        assert_eq!(
            select_test_core(Some(OsStr::new("1")), None, false, compiled),
            Err(CandidateCheckError::Unsupported)
        );
    }

    #[test]
    fn all_profiles_reject_missing_wrong_and_symlinked_binary_sources() {
        let dir = TestDir::new("polaris-check-profile-rejection-");
        let wrong = dir.join("wrong");
        fs::write(&wrong, b"wrong binary").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&wrong, &link).unwrap();
        for profile in [
            LinuxCheckProfile::B609,
            LinuxCheckProfile::FE182,
            LinuxCheckProfile::C002,
            LinuxCheckProfile::A4D085,
        ] {
            for path in [dir.join("missing"), wrong.clone(), link.clone()] {
                assert!(matches!(
                    linux::sealed_binary(&path, profile),
                    Err(CandidateCheckError::Unsupported)
                ));
            }
        }
    }

    #[test]
    fn protected_preparation_rejects_substituted_profile_before_reading_binary() {
        use super::super::CheckedCandidate;
        use crate::runtime::proxy::mesh_apply::candidate::{
            generate_direct_vless_candidate, protected_inputs, tests::fixture,
        };
        for (profile, substituted) in [
            (LinuxCheckProfile::B609, LinuxCheckProfile::FE182),
            (LinuxCheckProfile::FE182, LinuxCheckProfile::B609),
            (LinuxCheckProfile::C002, LinuxCheckProfile::B609),
            (LinuxCheckProfile::B609, LinuxCheckProfile::C002),
            (LinuxCheckProfile::C002, LinuxCheckProfile::FE182),
            (LinuxCheckProfile::FE182, LinuxCheckProfile::C002),
            (LinuxCheckProfile::A4D085, LinuxCheckProfile::B609),
            (LinuxCheckProfile::B609, LinuxCheckProfile::A4D085),
            (LinuxCheckProfile::A4D085, LinuxCheckProfile::FE182),
            (LinuxCheckProfile::FE182, LinuxCheckProfile::A4D085),
            (LinuxCheckProfile::A4D085, LinuxCheckProfile::C002),
            (LinuxCheckProfile::C002, LinuxCheckProfile::A4D085),
        ] {
            let (dir, runtime, snapshot, input, plan) = fixture();
            let candidate =
                generate_direct_vless_candidate(&runtime, &snapshot, &input, &plan).unwrap();
            // Synthetic inconsistent evidence is only a rejection fixture. It
            // cannot leave this test or substitute for an actual check result.
            let checked = CheckedCandidate {
                config_sha256: candidate.facts.final_config_sha256.clone(),
                binary_sha256: profile.binary_sha256().into(),
                candidate,
                profile: substituted,
            };
            assert!(matches!(
                protected_inputs::prepare_checked_linux_inputs(
                    checked,
                    &snapshot,
                    &plan,
                    &dir.join("missing")
                ),
                Err(protected_inputs::ProtectedCandidateError::Unsupported)
            ));
            assert!(!dir.join("mesh-routes").exists());
            assert!(!dir.join("singbox-runtime.json").exists());
        }
    }

    struct ReapedChild(std::process::Child);

    impl Drop for ReapedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn wait_for_ready(marker: &Path, timeout: Duration) -> std::io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match fs::read(marker) {
                Ok(bytes) if bytes == b"ready" => return Ok(()),
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "shell did not acknowledge completed exec",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn spawn_ready_child(stdin: Stdio) -> (TestDir, ReapedChild) {
        let dir = TestDir::new("polaris-check-fd-probe-");
        let marker = dir.join("ready");
        let child = ReapedChild(
            Command::new("/bin/sh")
                .args(["-c", "printf ready > \"$1\" || exit 1; exec sleep 5", "sh"])
                .arg(&marker)
                .stdin(stdin)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // A vfork-based spawn may return while exec is still closing CLOEXEC
        // descriptors. Only code running in the new shell can write this marker.
        wait_for_ready(&marker, Duration::from_secs(2)).unwrap();
        (dir, child)
    }

    #[derive(Debug, PartialEq, Eq)]
    struct InheritedConfig {
        fd: u32,
        identity: (u64, u64),
        bytes_sha256: String,
    }

    fn inherited_configs(child: &ReapedChild) -> Vec<InheritedConfig> {
        let fd_dir = format!("/proc/{}/fd", child.0.id());
        let mut inherited = Vec::new();
        for entry in fs::read_dir(fd_dir).unwrap() {
            let path = entry.unwrap().path();
            let target = match fs::read_link(&path) {
                Ok(target) => target,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!(
                    "cannot inspect child descriptor {}: {error}",
                    path.display()
                ),
            };
            // Keep the original all-config gate, including another test's file.
            if target
                .to_string_lossy()
                .contains("polaris-strict-check-config")
            {
                let file = fs::File::open(&path).unwrap();
                let metadata = file.metadata().unwrap();
                let bytes_sha256 =
                    polaris_updater::verify::sha256_reader_hex(&mut file.take(8 * 1024 * 1024 + 1))
                        .unwrap();
                inherited.push(InheritedConfig {
                    fd: path.file_name().unwrap().to_str().unwrap().parse().unwrap(),
                    identity: (metadata.dev(), metadata.ino()),
                    bytes_sha256,
                });
            }
        }
        inherited
    }

    fn assert_no_inherited_configs(child: &ReapedChild) {
        let inherited = inherited_configs(child);
        assert!(
            inherited.is_empty(),
            "inherited sealed configs: {inherited:?}"
        );
    }

    #[test]
    fn sealed_config_disallows_writes_after_hashing() {
        let mut config =
            linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                .unwrap();
        config.seek(SeekFrom::Start(0)).unwrap();
        assert!(config.write_all(b"x").is_err());
        assert!(config.set_len(0).is_err());
    }

    #[test]
    fn unrelated_spawn_does_not_inherit_sealed_config() {
        use nix::fcntl::{fcntl, FcntlArg};

        let config = linux::sealed_config(b"{\"fixture\":\"owned\"}").unwrap();
        assert_ne!(
            fcntl(&config, FcntlArg::F_GETFD).unwrap() & nix::libc::FD_CLOEXEC,
            0
        );
        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            scope.spawn(move || {
                let other = linux::sealed_config(b"{\"fixture\":\"parallel\"}").unwrap();
                let copies: Vec<_> = (0..64)
                    .map(|_| rustix::io::fcntl_dupfd_cloexec(&other, 3).unwrap())
                    .collect();
                for fd in &copies {
                    assert_ne!(
                        fcntl(fd, FcntlArg::F_GETFD).unwrap() & nix::libc::FD_CLOEXEC,
                        0
                    );
                }
                ready_tx.send(()).unwrap();
                // Dropping release_tx during a panic also releases this worker.
                let _ = release_rx.recv();
            });
            ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let (_dir, child) = spawn_ready_child(Stdio::null());
            assert_no_inherited_configs(&child);
            release_tx.send(()).unwrap();
        });
    }

    #[test]
    fn fd_probe_rejects_owned_and_other_configs_after_exec() {
        let owned = linux::sealed_config(b"{\"fixture\":\"owned\"}").unwrap();
        let other = linux::sealed_config(b"{\"fixture\":\"other\"}").unwrap();
        assert_ne!(
            owned.metadata().unwrap().ino(),
            other.metadata().unwrap().ino()
        );
        for (config, bytes) in [
            (&owned, b"{\"fixture\":\"owned\"}".as_slice()),
            (&other, b"{\"fixture\":\"other\"}".as_slice()),
        ] {
            let stdin = fs::File::from(rustix::io::fcntl_dupfd_cloexec(config, 3).unwrap());
            let (_dir, child) = spawn_ready_child(Stdio::from(stdin));
            let metadata = config.metadata().unwrap();
            assert_eq!(
                inherited_configs(&child),
                vec![InheritedConfig {
                    fd: 0,
                    identity: (metadata.dev(), metadata.ino()),
                    bytes_sha256: polaris_updater::verify::sha256_hex(bytes),
                }]
            );
            let rejection = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_no_inherited_configs(&child);
            }));
            assert!(
                rejection.is_err(),
                "the all-config gate must reject either inode"
            );
        }
    }

    #[test]
    fn fd_probe_readiness_timeout_is_bounded_and_reaps_child() {
        let dir = TestDir::new("polaris-check-fd-timeout-");
        let child = ReapedChild(
            Command::new("sleep")
                .arg("5")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id();
        let started = Instant::now();
        assert_eq!(
            wait_for_ready(&dir.join("missing"), Duration::from_millis(20))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(child);
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }

    #[test]
    fn fd_probe_reaps_child_during_assertion_unwind() {
        let mut pid = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_dir, child) = spawn_ready_child(Stdio::null());
            pid = Some(child.0.id());
            panic!("exercise FD probe cleanup");
        }));
        assert!(result.is_err());
        assert!(!PathBuf::from(format!("/proc/{}", pid.unwrap())).exists());
    }

    #[test]
    fn sealed_binary_stat_fifo_replacement_is_bounded_without_writer() {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = TestDir::new("polaris-check-stat-fifo-");
        let source = dir.join("core");
        fs::write(&source, b"original regular bytes").unwrap();
        let worker_path = source.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result =
                linux::sealed_binary_with_stat_hook(&worker_path, LinuxCheckProfile::B609, || {
                    fs::remove_file(&worker_path).unwrap();
                    nix::unistd::mkfifo(
                        &worker_path,
                        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
                    )
                    .unwrap();
                    ready_tx.send(()).unwrap();
                })
                .map(|_| ());
            result_tx.send(result).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(1));
        // A writer exists only to settle a regressed blocking open after the
        // rejection deadline has already failed, never on the passing path.
        let _unblock_on_failure = result.is_err().then(|| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
                .open(&source)
                .unwrap()
        });
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Err(CandidateCheckError::Unsupported));
    }

    #[tokio::test]
    async fn timeout_reaps_without_publishing_any_artifact() {
        let Some((binary_path, profile)) = selected_core_for_test() else {
            return;
        };
        let dir = TestDir::new("polaris-check-timeout-");
        let (binary, _) = linux::sealed_binary(&binary_path, profile).unwrap();
        let config =
            linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                .unwrap();
        assert_eq!(
            linux::run_check(binary, config, Duration::ZERO).await,
            Err(CandidateCheckError::TimedOut)
        );
        assert!(!dir.join("mesh-routes").exists());
    }

    #[test]
    fn selected_binary_is_rejected_under_every_other_profile() {
        let Some((binary_path, profile)) = selected_core_for_test() else {
            return;
        };
        for substituted in [
            LinuxCheckProfile::B609,
            LinuxCheckProfile::FE182,
            LinuxCheckProfile::C002,
            LinuxCheckProfile::A4D085,
        ] {
            if substituted != profile {
                assert!(matches!(
                    linux::sealed_binary(&binary_path, substituted),
                    Err(CandidateCheckError::Unsupported)
                ));
            }
        }
    }

    #[tokio::test]
    async fn sealed_binary_execution_survives_source_path_replacement_but_identity_detects_it() {
        let Some((binary_path, profile)) = selected_core_for_test() else {
            return;
        };
        let dir = TestDir::new("polaris-check-source-swap-");
        let source = dir.join("core");
        let shared_before = FileSnapshot::path(&binary_path).unwrap();
        // A hard link shares ctime with the real core used by parallel tests.
        // Creating or replacing it would invalidate their strict snapshots.
        // This private copy is only read into a memfd, never executed by path.
        assert_eq!(
            fs::copy(&binary_path, &source).unwrap(),
            shared_before.len()
        );
        let copied = FileSnapshot::path(&source).unwrap();
        assert!(
            !shared_before.same_identity(&copied),
            "source-swap fixture must own a different inode from the shared core"
        );
        assert!(shared_before.same_snapshot(&FileSnapshot::path(&binary_path).unwrap()));
        let (binary, before) = linux::sealed_binary(&source, profile).unwrap();
        let replacement = dir.join("replacement");
        fs::write(&replacement, b"wrong binary").unwrap();
        fs::rename(&replacement, &source).unwrap();
        let config =
            linux::sealed_config(b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}")
                .unwrap();
        assert_eq!(
            linux::run_check(binary, config, Duration::from_secs(8)).await,
            Ok(())
        );
        let after = FileSnapshot::path(&source).unwrap();
        assert!(!before.same_snapshot(&after));
        assert!(!dir.join("mesh-routes").exists());
        assert!(shared_before.same_snapshot(&FileSnapshot::path(&binary_path).unwrap()));
        drop(dir);
        assert!(shared_before.same_snapshot(&FileSnapshot::path(&binary_path).unwrap()));
    }

    #[tokio::test]
    async fn closed_standard_descriptors_still_execute_the_sealed_binary() {
        let child_marker = std::env::var_os("POLARIS_CLOSED_STD_CHILD_MARKER");
        let Some((binary_path, profile)) = selected_core_for_test() else {
            assert!(
                child_marker.is_none(),
                "closed-stdio child must select its real check core"
            );
            return;
        };
        if let Some(marker) = child_marker {
            use std::os::fd::OwnedFd;
            struct RestoreStandard([OwnedFd; 3]);
            impl RestoreStandard {
                fn restore(&self) -> nix::Result<()> {
                    nix::unistd::dup2_stdin(&self.0[0])?;
                    nix::unistd::dup2_stdout(&self.0[1])?;
                    nix::unistd::dup2_stderr(&self.0[2])
                }
            }
            impl Drop for RestoreStandard {
                fn drop(&mut self) {
                    let _ = self.restore();
                }
            }
            // Initialize this one-test harness and its Tokio runtime first. Only
            // the tested checker gets closed 0/1/2, with no shell/startup ambiguity.
            let saved = RestoreStandard([
                rustix::io::fcntl_dupfd_cloexec(std::io::stdin(), 3).unwrap(),
                rustix::io::fcntl_dupfd_cloexec(std::io::stdout(), 3).unwrap(),
                rustix::io::fcntl_dupfd_cloexec(std::io::stderr(), 3).unwrap(),
            ]);
            let mut diagnostic =
                fs::File::create(PathBuf::from(&marker).with_extension("stage")).unwrap();
            writeln!(diagnostic, "selected {profile:?} {}", binary_path.display()).unwrap();
            let checked: Result<(), String> = async {
                for fd in 0..=2 {
                    nix::unistd::close(fd).map_err(|e| e.to_string())?;
                }
                for fd in 0..=2 {
                    let result = fs::symlink_metadata(format!("/proc/self/fd/{fd}"));
                    if !matches!(result, Err(ref e) if e.kind() == std::io::ErrorKind::NotFound) {
                        return Err(format!("standard descriptor {fd} was not closed"));
                    }
                }
                writeln!(diagnostic, "0/1/2 verified closed").map_err(|e| e.to_string())?;
                let (binary, _) = linux::sealed_binary(&binary_path, profile)
                    .map_err(|e| format!("sealed binary: {e:?}"))?;
                let config = linux::sealed_config(
                    b"{\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}]}",
                )
                .map_err(|e| format!("sealed config: {e:?}"))?;
                writeln!(diagnostic, "sealed inputs ready").map_err(|e| e.to_string())?;
                linux::run_check(binary, config, Duration::from_secs(8))
                    .await
                    .map_err(|e| format!("sealed check: {e:?}"))?;
                writeln!(diagnostic, "sealed check succeeded").map_err(|e| e.to_string())?;
                Ok(())
            }
            .await;
            saved.restore().unwrap();
            writeln!(diagnostic, "result: {checked:?}").unwrap();
            checked.unwrap();
            fs::write(marker, b"checked with 0/1/2 closed").unwrap();
            return;
        }
        let dir = TestDir::new("polaris-check-closed-std-");
        let marker = dir.join("checked");
        let test_name = "runtime::proxy::mesh_apply::candidate::check::tests::linux_tests::closed_standard_descriptors_still_execute_the_sealed_binary";
        let mut child = ReapedChild(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
                .env("POLARIS_CLOSED_STD_CHILD_MARKER", &marker)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut timed_out = false;
        while child.0.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                timed_out = true;
                let _ = child.0.kill();
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let status = child.0.wait().unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut stdout)
            .unwrap();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        let diagnostic = fs::read_to_string(marker.with_extension("stage"))
            .unwrap_or_else(|e| format!("no stage record: {e}"));
        assert!(!timed_out && status.success(),
            "closed-stdio child status={} timed_out={timed_out}; {diagnostic}; stdout={}; stderr={}",
            status, String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr));
        assert_eq!(fs::read(marker).unwrap(), b"checked with 0/1/2 closed");
    }

    fn sealed_fixture_binary(path: &str) -> fs::File {
        use nix::fcntl::{fcntl, FcntlArg, SealFlag};
        use nix::sys::memfd::{memfd_create, MFdFlags};
        let actual = fs::canonicalize(path).unwrap();
        let bytes =
            polaris_core_supervisor::exact_spawn::read_linux_binary_source(&actual).unwrap();
        let mut file = fs::File::from(
            memfd_create(
                "polaris-check-no-network-fixture",
                MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
            )
            .unwrap(),
        );
        file.write_all(&bytes).unwrap();
        nix::sys::stat::fchmod(
            &file,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IXUSR,
        )
        .unwrap();
        fcntl(
            &file,
            FcntlArg::F_ADD_SEALS(
                SealFlag::F_SEAL_WRITE
                    | SealFlag::F_SEAL_GROW
                    | SealFlag::F_SEAL_SHRINK
                    | SealFlag::F_SEAL_SEAL,
            ),
        )
        .unwrap();
        fs::File::from(rustix::io::fcntl_dupfd_cloexec(&file, 3).unwrap())
    }

    #[tokio::test]
    async fn caller_cancellation_before_and_after_spawn_leaves_no_child() {
        let dir = TestDir::new("polaris-check-cancel-");
        let marker = dir.join("should-not-exist");
        let binary = sealed_fixture_binary("/bin/sh");
        let mut before =
            tokio::process::Command::new(format!("/proc/self/fd/{}", binary.as_raw_fd()));
        before.arg("-c").arg("touch \"$1\"").arg("sh").arg(&marker);
        let config = linux::sealed_config(b"{}").unwrap();
        let (notifier, notified) = tokio::sync::oneshot::channel();
        let never_polled = linux::supervise(
            before,
            binary,
            config,
            Duration::from_secs(8),
            Some(notifier),
        );
        drop(never_polled);
        assert!(notified.await.is_err());
        assert!(!marker.exists());

        let binary = sealed_fixture_binary("/bin/sleep");
        let mut after =
            tokio::process::Command::new(format!("/proc/self/fd/{}", binary.as_raw_fd()));
        // Rust coreutils is a multicall ELF on this host; preserve the original
        // sleep fixture's argv0. This never alters the strict production profile.
        after.as_std_mut().arg0("sleep");
        after
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let config = linux::sealed_config(b"{}").unwrap();
        let (notifier, notified) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(linux::supervise(
            after,
            binary,
            config,
            Duration::from_secs(8),
            Some(notifier),
        ));
        let pid = notified.await.unwrap();
        assert!(PathBuf::from(format!("/proc/{pid}")).exists());
        caller.abort();
        tokio::time::timeout(Duration::from_secs(3), async {
            while PathBuf::from(format!("/proc/{pid}")).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled child must be killed and reaped");
        assert!(caller.await.unwrap_err().is_cancelled());
    }
}
