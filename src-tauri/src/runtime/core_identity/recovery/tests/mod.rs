use super::*;
use polaris_helper_proto::core_manifest::CoreFile;
use polaris_updater::core_recovery::{RecoveryArchive, RecoveryFormat, RecoveryMember};
mod fixtures;
use fixtures::{LINUX, MAC_ARM64, MAC_X64, WIN, WIN_EXTRA};

pub(crate) struct Fixture {
    pub root: PathBuf,
    pub config: PathBuf,
    pub helper: PathBuf,
    pub selected: CoreManifest,
    pub spec: CoreRecoverySpec,
    pub archive: PathBuf,
}
fn write_fixture(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).unwrap().write_all(bytes).unwrap();
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    pub fn new(platform: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "polaris-explicit-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        create_private_directory(&root).unwrap();
        #[cfg(unix)]
        let root = fs::canonicalize(root).unwrap();
        let config = root.join("config");
        create_private_directory(&config).unwrap();
        let resources = root.join("resources");
        create_private_directory(&resources).unwrap();
        let dir = resources.join(platform);
        create_private_directory(&dir).unwrap();
        let helper = dir.join(if platform == "win" {
            "polaris-helper.exe"
        } else {
            "polaris-helper"
        });
        write_fixture(&helper, b"inert selected helper");
        let core = if platform == "win" {
            "sing-box.exe"
        } else {
            "sing-box"
        };
        let side = match platform {
            "linux" => Some("libcronet.so"),
            "win" => Some("libcronet.dll"),
            _ => None,
        };
        let bodies = std::iter::once((core, b"inert core fixture".as_slice()))
            .chain(side.map(|name| (name, b"inert sidecar fixture".as_slice())))
            .collect::<Vec<_>>();
        let selected = CoreManifest {
            platform: platform.into(),
            version: "1.2.3".into(),
            provider_fingerprint: "a".repeat(64),
            files: bodies
                .iter()
                .map(|(name, bytes)| CoreFile {
                    name: (*name).into(),
                    bytes: bytes.len() as u64,
                    sha256: polaris_updater::sha256_hex(bytes),
                })
                .collect(),
            helper_sha256: Some(polaris_updater::sha256_hex(b"inert selected helper")),
            cleaner_sha256: (platform == "win")
                .then(|| polaris_updater::sha256_hex(b"inert selected cleaner")),
        };
        if platform == "win" {
            write_fixture(&dir.join("polaris-cleaner.exe"), b"inert selected cleaner");
        }
        let bytes = match platform {
            "linux" => LINUX,
            "win" => WIN,
            "mac-x64" => MAC_X64,
            _ => MAC_ARM64,
        };
        let recovery_root = resources.join("core-recovery");
        create_private_directory(&recovery_root).unwrap();
        let recovery = recovery_root.join(platform);
        create_private_directory(&recovery).unwrap();
        let name = if platform == "win" {
            "fixture.zip"
        } else {
            "fixture.tar.gz"
        };
        let archive = recovery.join(name);
        write_fixture(&archive, bytes);
        let spec = CoreRecoverySpec {
            schema: "polaris-core-recovery-v1".into(),
            platform: platform.into(),
            core_id: selected
                .id(polaris_updater::sha256_hex)
                .unwrap()
                .as_str()
                .into(),
            archive: RecoveryArchive {
                resource: format!("core-recovery/{platform}/{name}"),
                format: if platform == "win" {
                    RecoveryFormat::Zip
                } else {
                    RecoveryFormat::TarGz
                },
                bytes: bytes.len() as u64,
                sha256: polaris_updater::sha256_hex(bytes),
                root: "inert-core".into(),
            },
            members: bodies
                .iter()
                .copied()
                .chain(std::iter::once((
                    "LICENSE",
                    b"inert license fixture".as_slice(),
                )))
                .map(|(name, bytes)| RecoveryMember {
                    name: format!("inert-core/{name}"),
                    bytes: bytes.len() as u64,
                    sha256: polaris_updater::sha256_hex(bytes),
                })
                .collect(),
            payload_members: bodies
                .iter()
                .map(|(name, _)| format!("inert-core/{name}"))
                .collect(),
        };
        Self {
            root,
            config,
            helper,
            selected,
            spec,
            archive,
        }
    }
    pub fn stage(&self) -> Result<InstallStage, String> {
        stage_from_bundle(
            &self.helper,
            &self.config,
            &self.selected,
            &self.spec,
            &AtomicBool::new(false),
        )
    }
}

#[test]
fn actual_strict_stager_and_existing_installer_compose_all_four_inert_data_rows() {
    for platform in ["linux", "win", "mac-x64", "mac-arm64"] {
        let f = Fixture::new(platform);
        // The runtime copies are deliberately damaged/absent. Recovery must not
        // consult them, execute a tool, or write the signed resource directory.
        fs::write(
            f.helper.parent().unwrap().join(&f.selected.files[0].name),
            b"damaged expanded core",
        )
        .unwrap();
        let archive = fs::read(&f.archive).unwrap();
        let stage = f.stage().unwrap();
        assert_eq!(stage.manifest.core, f.selected);
        for row in &f.selected.files {
            assert_eq!(
                polaris_updater::sha256_hex(&fs::read(stage.directory.join(&row.name)).unwrap()),
                row.sha256
            );
        }
        assert_eq!(fs::read(&f.archive).unwrap(), archive);
        assert_eq!(
            fs::read(f.helper.parent().unwrap().join(&f.selected.files[0].name)).unwrap(),
            b"damaged expanded core"
        );
        let directory = stage.directory.clone();
        drop(stage);
        assert!(!directory.exists());
    }
}
#[test]
fn absent_bad_archive_bad_helper_and_extra_member_preserve_input_and_user_data() {
    for case in ["absent", "hash", "helper", "extra", "selection"] {
        let mut f = Fixture::new("win");
        fs::write(f.config.join("profiles.json"), b"keep user data").unwrap();
        match case {
            "absent" => fs::remove_file(&f.archive).unwrap(),
            "hash" => {
                let mut raw = fs::read(&f.archive).unwrap();
                raw[0] ^= 1;
                fs::write(&f.archive, raw).unwrap();
            }
            "helper" => fs::write(&f.helper, b"wrong helper").unwrap(),
            "selection" => f.spec.core_id = "b".repeat(64),
            _ => {
                fs::write(&f.archive, WIN_EXTRA).unwrap();
                f.spec.archive.bytes = WIN_EXTRA.len() as u64;
                f.spec.archive.sha256 = polaris_updater::sha256_hex(WIN_EXTRA);
            }
        }
        let before = fs::read(&f.archive).ok();
        assert!(f.stage().unwrap_err().starts_with("CORE_BUNDLE_MISMATCH:"));
        assert_eq!(fs::read(&f.archive).ok(), before);
        assert_eq!(
            fs::read(f.config.join("profiles.json")).unwrap(),
            b"keep user data"
        );
        if let Ok(entries) = fs::read_dir(f.config.join("helper-install-stage")) {
            assert_eq!(entries.count(), 0);
        }
    }
}
#[test]
fn cancellation_is_not_success_and_never_publishes_or_deletes_user_data() {
    let f = Fixture::new("linux");
    let cancel = AtomicBool::new(true);
    assert!(
        stage_from_bundle(&f.helper, &f.config, &f.selected, &f.spec, &cancel)
            .unwrap_err()
            .starts_with("CORE_RECOVERY_CANCELLED:")
    );
    assert!(f.archive.exists());
    assert!(!f.config.join(super::super::user_store::USER_STORE).exists());
}
