use super::*;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Cursor;

fn encode(format: RecoveryFormat, files: &[(&str, &[u8])]) -> Vec<u8> {
    match format {
        RecoveryFormat::TarGz => {
            let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
            let mut directory = tar::Header::new_ustar();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_size(0);
            directory.set_mode(0o700);
            directory.set_cksum();
            archive
                .append_data(&mut directory, "native", io::empty())
                .unwrap();
            for (name, bytes) in files {
                let mut header = tar::Header::new_ustar();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o600);
                // Raw name injection is intentional for adversarial data tests;
                // production never builds or trusts a second archive format.
                header.as_mut_bytes()[..100].fill(0);
                header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
                header.set_cksum();
                archive.append(&header, *bytes).unwrap();
            }
            archive.into_inner().unwrap().finish().unwrap()
        }
        RecoveryFormat::Zip => {
            let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o600);
            for (name, bytes) in files {
                archive.start_file(name, options).unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap().into_inner()
        }
    }
}
fn spec(
    format: RecoveryFormat,
    archive: &[u8],
    files: &[(&str, &[u8])],
    payload: &[&str],
) -> CoreRecoverySpec {
    let platform = if format == RecoveryFormat::Zip {
        "win"
    } else {
        "linux"
    };
    CoreRecoverySpec {
        schema: "polaris-core-recovery-v1".into(),
        platform: platform.into(),
        core_id: "1".repeat(64),
        archive: RecoveryArchive {
            resource: format!(
                "core-recovery/{platform}/native.{}",
                if format == RecoveryFormat::Zip {
                    "zip"
                } else {
                    "tar.gz"
                }
            ),
            format,
            bytes: archive.len() as u64,
            sha256: crate::sha256_hex(archive),
            root: "native".into(),
        },
        members: files
            .iter()
            .map(|(name, bytes)| RecoveryMember {
                name: (*name).into(),
                bytes: bytes.len() as u64,
                sha256: crate::sha256_hex(bytes),
            })
            .collect(),
        payload_members: payload.iter().map(|s| (*s).into()).collect(),
    }
}
fn fixture(format: RecoveryFormat) -> (Vec<u8>, CoreRecoverySpec) {
    let files: &[(&str, &[u8])] = &[
        ("native/core", b"inert core data"),
        ("native/sidecar", b"inert library data"),
        ("native/LICENSE", b"license"),
    ];
    let bytes = encode(format, files);
    let expected = spec(format, &bytes, files, &["native/core", "native/sidecar"]);
    (bytes, expected)
}
fn parent() -> TempDir {
    tempfile::tempdir().unwrap()
}
fn canonical(parent: &TempDir) -> PathBuf {
    fs::canonicalize(parent.path()).unwrap()
}
fn names(parent: &TempDir) -> Vec<String> {
    let mut names = fs::read_dir(parent.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn both_native_formats_verify_every_member_and_return_only_private_payload_data() {
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        let (bytes, expected) = fixture(format);
        let parent = parent();
        fs::write(parent.path().join("installed-original"), b"keep").unwrap();
        fs::write(parent.path().join("transaction-state"), b"keep-state").unwrap();
        let mut source = Cursor::new(bytes.clone());
        let stage =
            stage_core_recovery(&mut source, &expected, &canonical(&parent), || false).unwrap();
        assert_eq!(stage.core_id(), expected.core_id);
        assert_eq!(stage.members().len(), 2);
        assert_eq!(
            fs::read(stage.directory().join("core")).unwrap(),
            b"inert core data"
        );
        assert_eq!(
            fs::read(stage.directory().join("sidecar")).unwrap(),
            b"inert library data"
        );
        assert!(!stage.directory().join("LICENSE").exists());
        assert!(!stage.directory().join("native").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(stage.directory().join("core"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(stage.directory())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        drop(stage);
        assert_eq!(names(&parent), ["installed-original", "transaction-state"]);
        assert_eq!(
            fs::read(parent.path().join("installed-original")).unwrap(),
            b"keep"
        );
        assert_eq!(source.into_inner(), bytes); // Never patch/repack the actual source.
    }
}

#[test]
fn mac_style_static_payload_has_no_invented_sidecar_and_no_execution() {
    let parent = parent();
    let marker = parent.path().join("payload-was-executed");
    let escaped = marker.to_string_lossy().replace('\'', "'\\''");
    let data = format!("#!/bin/sh\n: > '{escaped}'\n");
    let files = [
        ("native/sing-box", data.as_bytes()),
        ("native/LICENSE", b"license".as_slice()),
    ];
    let bytes = encode(RecoveryFormat::TarGz, &files);
    let mut expected = spec(RecoveryFormat::TarGz, &bytes, &files, &["native/sing-box"]);
    expected.platform = "mac-arm64".into();
    expected.archive.resource = "core-recovery/mac-arm64/native.tar.gz".into();
    let stage = stage_core_recovery(
        &mut Cursor::new(bytes),
        &expected,
        &canonical(&parent),
        || false,
    )
    .unwrap();
    assert_eq!(fs::read_dir(stage.directory()).unwrap().count(), 1);
    assert_eq!(
        fs::read(stage.directory().join("sing-box")).unwrap(),
        data.as_bytes()
    );
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn fresh_stage_is_owner_only_before_any_payload_member_is_written() {
    use std::os::unix::fs::PermissionsExt;
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        let (bytes, expected) = fixture(format);
        let parent = parent();
        let mut saw_stage = false;
        let error = stage_core_recovery(
            &mut Cursor::new(bytes),
            &expected,
            &canonical(&parent),
            || {
                for entry in fs::read_dir(parent.path()).unwrap() {
                    let entry = entry.unwrap();
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("core-recovery-")
                    {
                        assert_eq!(
                            entry.metadata().unwrap().permissions().mode() & 0o777,
                            0o700
                        );
                        assert_eq!(fs::read_dir(entry.path()).unwrap().count(), 0);
                        saw_stage = true;
                        return true;
                    }
                }
                false
            },
        )
        .unwrap_err();
        assert!(saw_stage);
        assert!(matches!(error, CoreRecoveryError::Cancelled));
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn invalid_compiled_schema_identity_member_sets_and_budgets_fail_before_reading_source() {
    let (bytes, expected) = fixture(RecoveryFormat::TarGz);
    let mut cases = Vec::new();
    let mut s = expected.clone();
    s.schema.push('x');
    cases.push(s);
    let mut s = expected.clone();
    s.core_id = "unknown".into();
    cases.push(s);
    let mut s = expected.clone();
    s.archive.resource = "../other.tar.gz".into();
    cases.push(s);
    let mut s = expected.clone();
    s.archive.bytes = MAX_ARCHIVE + 1;
    cases.push(s);
    let mut s = expected.clone();
    s.members[0].bytes = MAX_MEMBER + 1;
    cases.push(s);
    let mut s = expected.clone();
    s.members = (0..6)
        .map(|i| RecoveryMember {
            name: format!("native/file{i}"),
            bytes: MAX_MEMBER,
            sha256: "0".repeat(64),
        })
        .collect();
    s.payload_members = vec!["native/file0".into()];
    cases.push(s);
    let mut s = expected.clone();
    s.members = (0..=MAX_MEMBERS)
        .map(|i| RecoveryMember {
            name: format!("native/file{i}"),
            bytes: 0,
            sha256: crate::sha256_hex(b""),
        })
        .collect();
    s.payload_members = vec!["native/file0".into()];
    cases.push(s);
    let mut s = expected.clone();
    s.members.push(s.members[0].clone());
    cases.push(s);
    let mut s = expected.clone();
    s.members[0].name = "native/Core".into();
    s.members.push(RecoveryMember {
        name: "native/core".into(),
        bytes: 0,
        sha256: crate::sha256_hex(b""),
    });
    cases.push(s);
    let mut s = expected.clone();
    s.payload_members.push("native/missing".into());
    cases.push(s);
    let mut s = expected.clone();
    s.payload_members[0] = "native/LICENSE/child".into();
    cases.push(s);
    for expected in cases {
        let parent = parent();
        let mut source = Cursor::new(&bytes);
        assert!(
            stage_core_recovery(&mut source, &expected, &canonical(&parent), || false).is_err()
        );
        assert_eq!(source.position(), 0);
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn compressed_hash_size_and_legal_member_hash_are_all_required() {
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        let (bytes, expected) = fixture(format);
        for change in 0..4 {
            let parent = parent();
            let mut broken = expected.clone();
            match change {
                0 => broken.archive.sha256 = "0".repeat(64),
                1 => broken.archive.bytes += 1,
                2 => broken.members[2].sha256 = "0".repeat(64),
                _ => broken.members[2].bytes += 1,
            }
            assert!(stage_core_recovery(
                &mut Cursor::new(&bytes),
                &broken,
                &canonical(&parent),
                || false
            )
            .is_err());
            assert!(names(&parent).is_empty());
        }
    }
}

#[test]
fn cancellation_before_and_during_extraction_preserves_original_and_transaction_state() {
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        let (bytes, expected) = fixture(format);
        for limit in [0, 6] {
            let parent = parent();
            fs::write(parent.path().join("old"), b"original").unwrap();
            fs::write(parent.path().join("journal"), b"original-state").unwrap();
            let mut calls = 0;
            let error = stage_core_recovery(
                &mut Cursor::new(&bytes),
                &expected,
                &canonical(&parent),
                || {
                    calls += 1;
                    calls > limit
                },
            )
            .unwrap_err();
            assert!(matches!(error, CoreRecoveryError::Cancelled));
            assert_eq!(names(&parent), ["journal", "old"]);
            assert_eq!(
                fs::read(parent.path().join("journal")).unwrap(),
                b"original-state"
            );
        }
    }
}

#[test]
fn cancellation_at_final_acceptance_cannot_return_a_successful_stage() {
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        let (bytes, expected) = fixture(format);
        let parent = parent();
        let mut total = 0;
        let stage = stage_core_recovery(
            &mut Cursor::new(&bytes),
            &expected,
            &canonical(&parent),
            || {
                total += 1;
                false
            },
        )
        .unwrap();
        drop(stage);
        let mut calls = 0;
        let error = stage_core_recovery(
            &mut Cursor::new(&bytes),
            &expected,
            &canonical(&parent),
            || {
                calls += 1;
                calls == total
            },
        )
        .unwrap_err();
        assert!(matches!(error, CoreRecoveryError::Cancelled));
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn source_read_failure_preserves_original_files_and_discards_only_new_data() {
    struct Broken(Cursor<Vec<u8>>);
    impl Read for Broken {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0.position() >= 8 {
                return Err(io::Error::other("bounded source read fixture"));
            }
            let length = buffer.len().min(8);
            self.0.read(&mut buffer[..length])
        }
    }
    impl Seek for Broken {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.0.seek(position)
        }
    }
    let (bytes, expected) = fixture(RecoveryFormat::TarGz);
    let parent = parent();
    fs::write(parent.path().join("old"), b"untouched").unwrap();
    let error = stage_core_recovery(
        &mut Broken(Cursor::new(bytes)),
        &expected,
        &canonical(&parent),
        || false,
    )
    .unwrap_err();
    assert!(matches!(error, CoreRecoveryError::Io(_)));
    assert_eq!(names(&parent), ["old"]);
    assert_eq!(fs::read(parent.path().join("old")).unwrap(), b"untouched");
}

#[test]
fn parser_uses_owned_snapshot_when_the_original_source_changes_after_hashing() {
    struct MutableSource {
        data: Cursor<Vec<u8>>,
        swapped: bool,
        seeks: usize,
    }
    impl Read for MutableSource {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.data.read(buffer)?;
            if count == 0 && !self.swapped {
                self.data =
                    Cursor::new(b"changed source must not reach the archive parser".to_vec());
                self.swapped = true;
            }
            Ok(count)
        }
    }
    impl Seek for MutableSource {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.seeks += 1;
            self.data.seek(position)
        }
    }
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        let (bytes, expected) = fixture(format);
        let parent = parent();
        let mut source = MutableSource {
            data: Cursor::new(bytes),
            swapped: false,
            seeks: 0,
        };
        let stage =
            stage_core_recovery(&mut source, &expected, &canonical(&parent), || false).unwrap();
        assert!(source.swapped);
        assert_eq!(source.seeks, 1);
        assert_eq!(
            fs::read(stage.directory().join("core")).unwrap(),
            b"inert core data"
        );
        drop(stage);
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn unsafe_archive_names_and_extra_or_missing_members_never_escape_private_stage() {
    for format in [RecoveryFormat::TarGz, RecoveryFormat::Zip] {
        for bad in [
            "../escape",
            "/absolute",
            "native/../escape",
            "native/a\\b",
            "native/CON",
            "native/name.",
            "native/core:stream",
            "Native/core",
            "native/sub/core",
        ] {
            let files: &[(&str, &[u8])] = &[("native/core", b"core"), (bad, b"bad")];
            let bytes = encode(format, files);
            let expected = spec(format, &bytes, &files[..1], &["native/core"]);
            let parent = parent();
            assert!(
                stage_core_recovery(
                    &mut Cursor::new(bytes),
                    &expected,
                    &canonical(&parent),
                    || false
                )
                .is_err(),
                "{format:?}: {bad}"
            );
            assert!(names(&parent).is_empty());
        }
        let files: &[(&str, &[u8])] = &[("native/core", b"core")];
        let bytes = encode(format, files);
        let expected = spec(
            format,
            &bytes,
            &[("native/core", b"core"), ("native/LICENSE", b"missing")],
            &["native/core"],
        );
        let parent = parent();
        assert!(stage_core_recovery(
            &mut Cursor::new(bytes),
            &expected,
            &canonical(&parent),
            || false
        )
        .is_err());
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn tar_links_special_headers_duplicate_paths_and_gzip_trailing_streams_are_rejected() {
    for kind in [b'1', b'2', b'3', b'6', b'x', b'L', b'S'] {
        let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut header = tar::Header::new_ustar();
        header.set_path("native/core").unwrap();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(0);
        header.set_mode(0o600);
        header.set_cksum();
        archive.append(&header, io::empty()).unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let expected = spec(
            RecoveryFormat::TarGz,
            &bytes,
            &[("native/core", b"")],
            &["native/core"],
        );
        let parent = parent();
        assert!(
            stage_core_recovery(
                &mut Cursor::new(bytes),
                &expected,
                &canonical(&parent),
                || false
            )
            .is_err(),
            "type {kind}"
        );
        assert!(names(&parent).is_empty());
    }
    let files: &[(&str, &[u8])] = &[("native/core", b"core"), ("native/core", b"core")];
    let bytes = encode(RecoveryFormat::TarGz, files);
    let expected = spec(RecoveryFormat::TarGz, &bytes, &files[..1], &["native/core"]);
    let parent = parent();
    assert!(stage_core_recovery(
        &mut Cursor::new(bytes),
        &expected,
        &canonical(&parent),
        || false
    )
    .is_err());
    let (mut bytes, mut expected) = fixture(RecoveryFormat::TarGz);
    bytes.extend_from_slice(&bytes.clone());
    expected.archive.bytes = bytes.len() as u64;
    expected.archive.sha256 = crate::sha256_hex(&bytes);
    assert!(stage_core_recovery(
        &mut Cursor::new(bytes),
        &expected,
        &canonical(&parent),
        || false
    )
    .is_err());
    assert!(names(&parent).is_empty());
}

#[test]
fn zip_links_duplicate_metadata_encryption_and_unsupported_methods_are_rejected() {
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("native/core", "../outside", options)
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let expected = spec(
        RecoveryFormat::Zip,
        &bytes,
        &[("native/core", b"../outside")],
        &["native/core"],
    );
    let parent = parent();
    assert!(stage_core_recovery(
        &mut Cursor::new(bytes),
        &expected,
        &canonical(&parent),
        || false
    )
    .is_err());
    let files: &[(&str, &[u8])] = &[("native/core", b"core"), ("native/care", b"core")];
    let mut bytes = encode(RecoveryFormat::Zip, files);
    for index in 0..bytes.len().saturating_sub(11) {
        if &bytes[index..index + 11] == b"native/care" {
            bytes[index..index + 11].copy_from_slice(b"native/core");
        }
    }
    let expected = spec(RecoveryFormat::Zip, &bytes, &files[..1], &["native/core"]);
    assert!(stage_core_recovery(
        &mut Cursor::new(bytes),
        &expected,
        &canonical(&parent),
        || false
    )
    .is_err());
    for encrypted in [false, true] {
        let (mut bytes, mut expected) = fixture(RecoveryFormat::Zip);
        for offset in 0..bytes.len().saturating_sub(12) {
            let local = &bytes[offset..offset + 4] == b"PK\x03\x04";
            let central = &bytes[offset..offset + 4] == b"PK\x01\x02";
            if local || central {
                let at = offset + if central { 8 } else { 6 } + if encrypted { 0 } else { 2 };
                bytes[at..at + 2]
                    .copy_from_slice(&(if encrypted { 1u16 } else { 12u16 }).to_le_bytes());
            }
        }
        expected.archive.bytes = bytes.len() as u64;
        expected.archive.sha256 = crate::sha256_hex(&bytes);
        assert!(stage_core_recovery(
            &mut Cursor::new(bytes),
            &expected,
            &canonical(&parent),
            || false
        )
        .is_err());
    }
    assert!(names(&parent).is_empty());
}

#[test]
fn zip_extra_metadata_cannot_supply_an_alternate_member_or_link_interpretation() {
    for central_only in [false, true] {
        let mut options = zip::write::FullFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        options
            .add_extra_data(0xbeef, b"alternate native/other", central_only)
            .unwrap();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("native/core", options).unwrap();
        writer.write_all(b"core").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let expected = spec(
            RecoveryFormat::Zip,
            &bytes,
            &[("native/core", b"core")],
            &["native/core"],
        );
        let parent = parent();
        assert!(stage_core_recovery(
            &mut Cursor::new(bytes),
            &expected,
            &canonical(&parent),
            || false
        )
        .is_err());
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn zip_duplicate_central_records_and_local_name_aliases_are_not_hidden_by_the_index() {
    let original = encode(RecoveryFormat::Zip, &[("native/core", b"core")]);
    let footer = original.len() - 22;
    // Adversarial fixture only: duplicate a complete central record while both
    // records still point to the same one local file. ZipArchive's named index
    // alone would retain one entry and lose this duplicate.
    let directory =
        u32::from_le_bytes(original[footer + 16..footer + 20].try_into().unwrap()) as usize;
    let mut duplicate = original[..footer].to_vec();
    duplicate.extend_from_slice(&original[directory..footer]);
    let mut end = original[footer..].to_vec();
    end[8..10].copy_from_slice(&2u16.to_le_bytes());
    end[10..12].copy_from_slice(&2u16.to_le_bytes());
    end[12..16].copy_from_slice(&((2 * (footer - directory)) as u32).to_le_bytes());
    duplicate.extend_from_slice(&end);
    let mut alias = original.clone();
    let at = alias
        .windows(b"native/core".len())
        .position(|name| name == b"native/core")
        .unwrap();
    alias[at..at + b"native/core".len()].copy_from_slice(b"native/evil");
    let mut central_crc = original.clone();
    central_crc[directory + 16] ^= 1;
    for bytes in [duplicate, alias, central_crc] {
        let expected = spec(
            RecoveryFormat::Zip,
            &bytes,
            &[("native/core", b"core")],
            &["native/core"],
        );
        let parent = parent();
        assert!(stage_core_recovery(
            &mut Cursor::new(bytes),
            &expected,
            &canonical(&parent),
            || false
        )
        .is_err());
        assert!(names(&parent).is_empty());
    }
}

#[test]
fn zip_descriptor_flags_are_rejected_in_central_local_and_both_headers() {
    let (original, selected) = fixture(RecoveryFormat::Zip);
    let mut archive = zip::ZipArchive::new(Cursor::new(&original)).unwrap();
    let headers: Vec<_> = (0..archive.len())
        .map(|i| {
            let file = archive.by_index_raw(i).unwrap();
            (
                file.header_start() as usize,
                file.central_header_start() as usize,
            )
        })
        .collect();
    for (local, central) in [(false, true), (true, false), (true, true)] {
        let mut bytes = original.clone();
        // Malformed fixture bytes only. Production reads flags through the
        // mature library's public API and never parses ZIP bit fields itself.
        for (local_header, central_header) in &headers {
            if local {
                bytes[local_header + 6] |= 1 << 3;
            }
            if central {
                bytes[central_header + 8] |= 1 << 3;
            }
        }
        let mut expected = selected.clone();
        expected.archive.sha256 = crate::sha256_hex(&bytes);
        let parent = parent();
        let error = stage_core_recovery(
            &mut Cursor::new(bytes),
            &expected,
            &canonical(&parent),
            || false,
        )
        .unwrap_err();
        assert!(
            matches!(error, CoreRecoveryError::InvalidArchive),
            "local={local} central={central}"
        );
        assert!(names(&parent).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn linked_staging_parent_is_refused_without_touching_its_target() {
    let parent = parent();
    let target = parent.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = parent.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (bytes, expected) = fixture(RecoveryFormat::TarGz);
    assert!(stage_core_recovery(&mut Cursor::new(bytes), &expected, &link, || false).is_err());
    assert!(fs::read_dir(target).unwrap().next().is_none());
}

#[test]
fn spec_json_has_closed_fields_and_exact_format_tags() {
    let (_, expected) = fixture(RecoveryFormat::TarGz);
    let mut value = serde_json::to_value(&expected).unwrap();
    assert_eq!(value["archive"]["format"], "tar.gz");
    assert!(value["payloadMembers"][0]
        .as_str()
        .unwrap()
        .starts_with("native/"));
    value["authority"] = true.into();
    assert!(serde_json::from_value::<CoreRecoverySpec>(value).is_err());
}
